//! Host addition, invite reconciliation, removal, address pinning, certificate clearing,
//! renaming, network-change reset, and unit tests for [`super::manager::HostManager`].

use std::io;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use nightjar_policy::hosts::{EndpointSlot, HostReachState, probe_plan, punches_now};

use crate::http::{certificate_sha256_der, pem_decode_certificate};
use crate::internet;

use super::manager::{HostManager, HostSnapshot, LinkAddOutcome, apply_server_info};
use super::poll::{
    compute_internet_path, is_site_local_host, parallel_poll_direct, parse_internet_reply,
    poll_over_the_internet, PollIpSuccess,
};
use super::store::HostEntry;

/// Outcome of an ephemeral [`HostManager::poll_details`] check (Android `@VisibleForTesting pollComputer`).
#[derive(Clone, Debug)]
pub struct EphemeralPollOutcome {
    pub ok: bool,
    pub snapshot: HostSnapshot,
    pub should_clear_cert: bool,
}

impl HostManager {
    /// Probes a user-entered/discovered candidate entry, verifies it, persists it, and begins polling.
    pub fn add_computer_blocking(&self, mut candidate: HostEntry) -> Option<HostSnapshot> {
        if !candidate.has_android_slots() {
            candidate.sync_slots_from_shared();
        }
        let slots = candidate.slots();
        let probes = probe_plan(&slots, true, None, 0);
        let direct = parallel_poll_direct(&self.inner.pool, &probes, None, None, true);
        let win = direct.winner?;
        let uuid = win.info.unique_id.clone();

        let existing_cert = self
            .get_snapshot(&uuid)
            .and_then(|s| s.entry.server_certificate_der());
        if existing_cert.is_some() {
            let mut updated_cand = win.candidate.clone();
            updated_cand.https_port = win.info.https_port;
            let pinned_direct = parallel_poll_direct(
                &self.inner.pool,
                &[updated_cand],
                Some(&uuid),
                existing_cert.as_deref(),
                true,
            );
            if let Some(pwin) = pinned_direct.winner {
                return Some(self.finish_adding(candidate, existing_cert, pwin));
            }
        }
        Some(self.finish_adding(candidate, existing_cert, win))
    }

    /// Adds a PC for a pairing link, refusing if the responder claims a known UUID pinned to
    /// another certificate, while preserving an existing matching certificate and returning the snapshot.
    pub fn add_computer_for_link(
        &self,
        mut candidate: HostEntry,
        link_cert_sha256: [u8; 32],
    ) -> (LinkAddOutcome, Option<HostSnapshot>) {
        if !candidate.has_android_slots() {
            candidate.sync_slots_from_shared();
        }
        let slots = candidate.slots();
        let probes = probe_plan(&slots, true, None, 0);
        let direct = parallel_poll_direct(&self.inner.pool, &probes, None, None, true);
        let Some(win) = direct.winner else {
            return (LinkAddOutcome::Unreachable, None);
        };
        let uuid = win.info.unique_id.clone();
        let existing_cert = self
            .get_snapshot(&uuid)
            .and_then(|s| s.entry.server_certificate_der());
        if let Some(ref pinned_der) = existing_cert
            && certificate_sha256_der(pinned_der) != link_cert_sha256
        {
            return (LinkAddOutcome::CertificateMismatch, None);
        }
        let final_win = if existing_cert.is_some() {
            let mut updated_cand = win.candidate.clone();
            updated_cand.https_port = win.info.https_port;
            let pinned_direct = parallel_poll_direct(
                &self.inner.pool,
                &[updated_cand],
                Some(&uuid),
                existing_cert.as_deref(),
                true,
            );
            pinned_direct.winner.unwrap_or(win)
        } else {
            win
        };
        let snap = self.finish_adding(candidate, existing_cert, final_win);
        (LinkAddOutcome::Added, Some(snap))
    }

    fn finish_adding(
        &self,
        mut candidate: HostEntry,
        existing_cert: Option<Vec<u8>>,
        win: PollIpSuccess,
    ) -> HostSnapshot {
        candidate.shared.unique_id = win.info.unique_id.clone();
        candidate.set_server_certificate_der(existing_cert.as_deref());
        if win.should_clear_cert {
            candidate.shared.server_certificate = None;
            candidate.shared.internet_play = None;
        }
        let mut needs_vpn_stun = false;
        if candidate.remote_endpoint.is_none()
            && win.info.external_ip.is_none()
            && is_site_local_host(&win.candidate.address)
        {
            if self.inner.vpn_active.load(Ordering::Acquire) {
                needs_vpn_stun = true;
            } else {
                let resolver = self
                    .inner
                    .stun_resolver
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();
                if let Some(remote) = resolver(win.info.external_port) {
                    candidate.remote_endpoint = Some(remote);
                }
            }
        }
        candidate.sync_shared_from_slots();
        let mut snap = self.upsert_tuple(candidate);
        let uuid = win.info.unique_id.clone();
        if let Some(arc) = self
            .inner
            .tuples
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&uuid)
            .cloned()
        {
            let mut t = arc.lock().unwrap_or_else(|e| e.into_inner());
            t.poll_state.record_poll_success(self.now_ms());
            apply_server_info(&mut t.snapshot, &win.info, win.candidate.port, &win.candidate);
            t.snapshot.state = HostReachState::Online;
            t.snapshot.another_certificate = false;
            t.snapshot.unreachable = None;
            t.snapshot.needs_vpn_stun =
                needs_vpn_stun && t.snapshot.entry.remote_endpoint.is_none();
            if win.should_clear_cert {
                t.snapshot.entry.shared.server_certificate = None;
                t.snapshot.entry.shared.internet_play = None;
                t.snapshot.paired = false;
                internet::stop(&uuid);
            } else {
                t.snapshot.paired = win.info.paired;
            }
            if let Some(caps_json) = win.info.capabilities.to_versioned_json() {
                let _ = self.inner.meta.save_capabilities(&uuid, &caps_json);
                t.snapshot.capabilities_json = Some(caps_json);
            }
            let reply = parse_internet_reply(t.snapshot.entry.shared.internet_play.as_deref());
            t.snapshot.internet_path = compute_internet_path(
                t.snapshot.active_endpoint.as_ref(),
                t.snapshot.entry.local_endpoint.as_ref(),
                t.snapshot.entry.manual_endpoint.as_ref(),
                reply.as_ref(),
            );
            t.snapshot.entry.sync_shared_from_slots();
            let _ = self.inner.store.upsert(t.snapshot.entry.clone());
            snap = t.snapshot.clone();
        }
        self.emit(&snap);
        snap
    }

    /// Saves a host redeemed via an invite link, refusing if another certificate is already pinned.
    pub fn add_invited_computer(
        &self,
        mut entry: HostEntry,
        internet_play_reply: &str,
    ) -> io::Result<bool> {
        let uuid = entry.shared.unique_id.clone();
        let incoming_der = entry.server_certificate_der();
        if let Some(existing_snap) = self.get_snapshot(&uuid)
            && let Some(existing_der) = existing_snap.entry.server_certificate_der()
            && incoming_der.as_deref() != Some(existing_der.as_slice())
        {
            return Ok(false);
        }
        let _rem_guard = self.inner.removal_lock.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(saved) = self.inner.store.get_by_uuid(&uuid)
            && let Some(saved_der) = saved.server_certificate_der()
            && incoming_der.as_deref() != Some(saved_der.as_slice())
        {
            return Ok(false);
        }
        entry.shared.internet_play = Some(internet_play_reply.to_owned());
        entry.sync_shared_from_slots();
        self.inner.store.upsert(entry.clone())?;
        let snap = self.upsert_tuple(entry);
        self.emit(&snap);
        Ok(true)
    }

    /// Checks whether `link_fp_sha256` conflicts with an already-pinned host UUID.
    pub fn invite_claims_known_pc(&self, uuid: &str, link_fp_sha256: [u8; 32]) -> bool {
        let cert_der = self
            .get_snapshot(uuid)
            .and_then(|s| s.entry.server_certificate_der())
            .or_else(|| {
                self.inner
                    .store
                    .get_by_uuid(uuid)
                    .and_then(|e| e.server_certificate_der())
            });
        let existing_sha = cert_der.map(|der| certificate_sha256_der(&der));
        nightjar_policy::hosts::invite_claims_known_pc(existing_sha, link_fp_sha256)
    }

    /// Stops polling, marks tombstone, deletes host from stores, and clears pool entries.
    pub fn remove_computer(&self, uuid: &str) -> io::Result<()> {
        let removed_tuple = {
            let mut tuples = self.inner.tuples.lock().unwrap_or_else(|e| e.into_inner());
            tuples.remove(uuid)
        };
        if let Some(arc) = removed_tuple {
            let handle = {
                let mut t = arc.lock().unwrap_or_else(|e| e.into_inner());
                t.removal_tombstone = true;
                t.poll_state.removed = true;
                let (lock, cvar) = &*t.stop_signal;
                *lock.lock().unwrap_or_else(|e| e.into_inner()) = true;
                cvar.notify_all();
                t.poll_thread.take()
            };
            self.inner.pool.clear();
            if let Some(h) = handle {
                let _ = h.join();
            }
        }
        let _rem_guard = self.inner.removal_lock.lock().unwrap_or_else(|e| e.into_inner());
        let _ = self.inner.store.delete_by_uuid(uuid)?;
        let _ = self.inner.meta.forget(uuid)?;
        internet::stop(uuid);
        Ok(())
    }

    pub fn pin_address(&self, uuid: &str, endpoint: Option<EndpointSlot>) -> io::Result<()> {
        let _ = self.inner.meta.set_pinned(uuid, endpoint.clone())?;
        if let Some(arc) = self
            .inner
            .tuples
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(uuid)
            .cloned()
        {
            let snap = {
                let mut t = arc.lock().unwrap_or_else(|e| e.into_inner());
                t.snapshot.pinned_endpoint = endpoint.clone();
                t.snapshot.entry.pinned_endpoint = endpoint.clone();
                t.snapshot.entry.shared.pinned_address =
                    endpoint.as_ref().map(|e| e.address.clone());
                if let Some(ref ep) = endpoint {
                    t.snapshot.active_endpoint = Some(ep.clone());
                }
                let _ = self.inner.store.upsert(t.snapshot.entry.clone());
                t.snapshot.clone()
            };
            self.emit(&snap);
        }
        Ok(())
    }

    pub fn forget_certificate(&self, uuid: &str) -> io::Result<()> {
        if let Some(arc) = self
            .inner
            .tuples
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(uuid)
            .cloned()
        {
            let snap = {
                let mut t = arc.lock().unwrap_or_else(|e| e.into_inner());
                t.snapshot.entry.shared.server_certificate = None;
                t.snapshot.entry.shared.internet_play = None;
                t.snapshot.paired = false;
                t.snapshot.clone()
            };
            self.emit(&snap);
        }
        let _rem_guard = self.inner.removal_lock.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(mut saved) = self.inner.store.get_by_uuid(uuid) {
            let changed = saved.shared.server_certificate.take().is_some()
                | saved.shared.internet_play.take().is_some();
            if changed {
                let _ = self.inner.store.upsert(saved)?;
            }
        }
        internet::stop(uuid);
        self.inner.pool.clear();
        Ok(())
    }

    /// Updates a host's STUN-resolved remote endpoint and clears `needs_vpn_stun`.
    pub fn set_remote_endpoint(&self, uuid: &str, remote: Option<EndpointSlot>) -> io::Result<()> {
        if let Some(arc) = self
            .inner
            .tuples
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(uuid)
            .cloned()
        {
            let snap = {
                let mut t = arc.lock().unwrap_or_else(|e| e.into_inner());
                t.snapshot.entry.remote_endpoint = remote.clone();
                t.snapshot.needs_vpn_stun = false;
                t.snapshot.entry.sync_shared_from_slots();
                let _ = self.inner.store.upsert(t.snapshot.entry.clone())?;
                t.snapshot.clone()
            };
            self.emit(&snap);
        }
        Ok(())
    }

    /// Resets a single host's reachability state to [`HostReachState::Unknown`].
    pub fn invalidate_state(&self, uuid: &str) {
        let Some(arc) = self
            .inner
            .tuples
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(uuid)
            .cloned()
        else {
            return;
        };
        let net_lock = {
            let t = arc.lock().unwrap_or_else(|e| e.into_inner());
            Arc::clone(&t.network_lock)
        };
        let _net_guard = net_lock.lock().unwrap_or_else(|e| e.into_inner());
        let snap = {
            let mut t = arc.lock().unwrap_or_else(|e| e.into_inner());
            if t.removal_tombstone {
                return;
            }
            t.poll_state.has_polled_successfully = false;
            t.snapshot.state = HostReachState::Unknown;
            t.snapshot.server_info = None;
            t.snapshot.clone()
        };
        self.emit(&snap);
    }

    /// Imports or clears stored `internet_play` JSON for `uuid` (e.g. legacy Android migration).
    pub fn import_internet_play(&self, uuid: &str, reply_json: Option<&str>) -> io::Result<()> {
        if let Some(arc) = self
            .inner
            .tuples
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(uuid)
            .cloned()
        {
            let mut t = arc.lock().unwrap_or_else(|e| e.into_inner());
            if t.snapshot.entry.shared.internet_play.as_deref() != reply_json {
                t.snapshot.entry.shared.internet_play = reply_json.map(ToOwned::to_owned);
                let _ = self.inner.store.upsert(t.snapshot.entry.clone())?;
            }
        }
        let _rem_guard = self.inner.removal_lock.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(mut saved) = self.inner.store.get_by_uuid(uuid)
            && saved.shared.internet_play.as_deref() != reply_json
        {
            saved.shared.internet_play = reply_json.map(ToOwned::to_owned);
            self.inner.store.upsert(saved)?;
        }
        Ok(())
    }

    /// Updates a host's pinned PEM certificate in both memory and `hosts.json`.
    pub fn set_server_certificate_pem(&self, uuid: &str, pem: Option<&str>) -> io::Result<()> {
        let der = pem
            .map(pem_decode_certificate)
            .transpose()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
        if let Some(arc) = self
            .inner
            .tuples
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(uuid)
            .cloned()
        {
            let snap = {
                let mut t = arc.lock().unwrap_or_else(|e| e.into_inner());
                t.poll_state.has_polled_successfully = false;
                t.snapshot.entry.set_server_certificate_der(der.as_deref());
                t.snapshot.paired = der.is_some();
                let _ = self.inner.store.upsert(t.snapshot.entry.clone());
                t.snapshot.clone()
            };
            self.emit(&snap);
        }
        self.inner.pool.clear();
        Ok(())
    }

    pub fn rename_computer(&self, uuid: &str, alias: Option<&str>) -> io::Result<()> {
        let _ = self.inner.meta.set_alias(uuid, alias)?;
        let normalized = nightjar_policy::hosts::HostNames::normalize_alias(alias, None);
        if let Some(arc) = self
            .inner
            .tuples
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(uuid)
            .cloned()
        {
            let snap = {
                let mut t = arc.lock().unwrap_or_else(|e| e.into_inner());
                t.snapshot.alias = normalized.clone();
                t.snapshot.entry.shared.alias = normalized;
                let _ = self.inner.store.upsert(t.snapshot.entry.clone());
                t.snapshot.clone()
            };
            self.emit(&snap);
        }
        Ok(())
    }

    /// Clears pooled clients and internet tunnels after an OS network transition, skipping any
    /// host that already succeeded polling at or after `at_ms`.
    pub fn reset_network_state(&self, available: bool, at_ms: u64) {
        self.inner.pool.clear();
        let arcs: Vec<_> = self
            .inner
            .tuples
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect();
        for arc in arcs {
            let net_lock = {
                let t = arc.lock().unwrap_or_else(|e| e.into_inner());
                std::sync::Arc::clone(&t.network_lock)
            };
            let _net_guard = net_lock.lock().unwrap_or_else(|e| e.into_inner());
            let (uuid, snap) = {
                let mut t = arc.lock().unwrap_or_else(|e| e.into_inner());
                if t.removal_tombstone {
                    continue;
                }
                let uuid = t.snapshot.entry.shared.unique_id.clone();
                if t.poll_state.has_polled_successfully
                    && t.poll_state.last_successful_poll_ms >= at_ms
                {
                    (uuid, None)
                } else {
                    t.poll_state.has_polled_successfully = false;
                    t.snapshot.state = if available {
                        HostReachState::Unknown
                    } else {
                        HostReachState::Offline
                    };
                    t.snapshot.server_info = None;
                    (uuid, Some(t.snapshot.clone()))
                }
            };
            internet::stop(&uuid);
            if let Some(snap) = snap {
                self.emit(&snap);
            }
        }
    }

    /// Polls an ad-hoc `candidate` through the manager's pool and internet-play fallback without
    /// mutating tracked tuples (backing Android `@VisibleForTesting pollComputer`).
    pub fn poll_details(
        &self,
        mut candidate: HostEntry,
        initial_state: HostReachState,
        active_endpoint: Option<EndpointSlot>,
    ) -> EphemeralPollOutcome {
        if !candidate.has_android_slots() {
            candidate.sync_slots_from_shared();
        }
        let uuid = candidate.shared.unique_id.clone();
        if candidate.shared.internet_play.is_none() && !uuid.is_empty() {
            candidate.shared.internet_play = self
                .get_snapshot(&uuid)
                .and_then(|s| s.entry.shared.internet_play);
        }
        let cert_der = candidate.server_certificate_der();
        let slots = candidate.slots();
        let active_port = active_endpoint.as_ref().map(|e| e.port);
        let probes = probe_plan(
            &slots,
            initial_state == HostReachState::Online,
            active_port,
            candidate.shared.https_port,
        );
        let expected_uuid = (!uuid.is_empty()).then_some(uuid.as_str());
        let likely_online = initial_state == HostReachState::Online;
        let direct = parallel_poll_direct(
            &self.inner.pool,
            &probes,
            expected_uuid,
            cert_der.as_deref(),
            likely_online,
        );
        let mut winner = direct.winner;
        let mut unreachable_msg = None;
        let mut internet_another_cert = false;

        if winner.is_none()
            && slots.pinned.is_none()
            && let Some(reply_json) = candidate.shared.internet_play.as_deref()
        {
            let active_is_tunnel = active_endpoint
                .as_ref()
                .is_some_and(|e| nightjar_policy::hosts::is_tunnel_address(&e.address));
            let allow_punch = punches_now(
                slots.local.is_some(),
                initial_state == HostReachState::Offline,
                active_is_tunnel,
            );
            match poll_over_the_internet(
                &self.inner.pool,
                &uuid,
                reply_json,
                expected_uuid,
                cert_der.as_deref(),
                &probes,
                direct.tried,
                likely_online,
                allow_punch,
            ) {
                Ok((_, win)) => winner = Some(win),
                Err(fail) => {
                    internet_another_cert = fail.another_certificate;
                    unreachable_msg = Some(fail.reason);
                }
            }
        }

        let reply = parse_internet_reply(candidate.shared.internet_play.as_deref());
        let pinned_endpoint = candidate.pinned_endpoint.clone();
        let alias = candidate.shared.alias.clone();
        let mut snap = HostSnapshot {
            paired: candidate.shared.server_certificate.is_some(),
            entry: candidate,
            state: initial_state,
            active_endpoint,
            another_certificate: false,
            running_game_id: 0,
            running_game_uuid: None,
            external_port: 0,
            unreachable: None,
            capabilities_json: None,
            alias,
            pinned_endpoint,
            internet_path: None,
            needs_vpn_stun: false,
            server_info: None,
        };
        if let Some(win) = winner {
            apply_server_info(&mut snap, &win.info, win.candidate.port, &win.candidate);
            snap.state = HostReachState::Online;
            snap.paired = win.info.paired && !win.should_clear_cert;
            if win.should_clear_cert {
                snap.entry.shared.server_certificate = None;
                snap.entry.shared.internet_play = None;
            }
            snap.capabilities_json = win.info.capabilities.to_versioned_json();
            snap.internet_path = compute_internet_path(
                snap.active_endpoint.as_ref(),
                snap.entry.local_endpoint.as_ref(),
                snap.entry.manual_endpoint.as_ref(),
                reply.as_ref(),
            );
            EphemeralPollOutcome {
                ok: true,
                snapshot: snap,
                should_clear_cert: win.should_clear_cert,
            }
        } else {
            snap.another_certificate = direct.another_certificate || internet_another_cert;
            snap.unreachable = unreachable_msg;
            EphemeralPollOutcome {
                ok: false,
                snapshot: snap,
                should_clear_cert: false,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::identity::tests::fixture;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Arc;

    fn temp_dir(label: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("nightjar-hm-{label}-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn poll_saves_only_changes_and_remove_tombstones_cleanly() {
        let dir = temp_dir("saves");
        let mgr = HostManager::open(&dir, Some(Arc::new(fixture()))).unwrap();
        mgr.set_stun_resolver(Arc::new(|_| None));

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let srv = std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut sock, _) = listener.accept().unwrap();
                let mut buf = [0u8; 2048];
                let _ = sock.read(&mut buf);
                let body = r#"<root status_code="200"><hostname>StudioPC</hostname><uniqueid>pc-save</uniqueid><state>MJ_SERVER_FREE</state><PairStatus>0</PairStatus><HttpsPort>47984</HttpsPort><ExternalPort>47989</ExternalPort></root>"#;
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                sock.write_all(reply.as_bytes()).unwrap();
            }
        });

        let mut initial = HostEntry::default();
        initial.shared.unique_id = "pc-save".into();
        initial.shared.name = "Initial".into();
        initial.local_endpoint = EndpointSlot::new("127.0.0.1", port);
        initial.sync_shared_from_slots();
        mgr.store().upsert(initial.clone()).unwrap();
        mgr.reload_from_store();

        assert!(mgr.poll_host("pc-save", true), "first poll updates name + online state");
        assert!(!mgr.poll_host("pc-save", true), "identical second poll reports no change");

        mgr.remove_computer("pc-save").unwrap();
        assert!(mgr.get_snapshot("pc-save").is_none());
        assert!(mgr.store().get_by_uuid("pc-save").is_none());
        assert!(!mgr.poll_host("pc-save", true));

        srv.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn start_and_stop_polling_join_worker_threads_cleanly() {
        let dir = temp_dir("lifecycle");
        let mgr = HostManager::open(&dir, Some(Arc::new(fixture()))).unwrap();
        mgr.set_stun_resolver(Arc::new(|_| None));
        let mut entry = HostEntry::default();
        entry.shared.unique_id = "pc-bg".into();
        entry.local_endpoint = EndpointSlot::new("127.0.0.1", 1);
        mgr.upsert_tuple(entry);

        mgr.start_polling();
        mgr.stop_polling();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
