//! Unified host registry, polling lifecycle, address pinning, add/invite/remove flows,
//! and change-detecting persistence shared across Android `ComputerManagerService` and Desktop.

use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use nightjar_policy::hosts::{
    EndpointSlot, HostReachState, PollTupleState, SERVERINFO_POLL_PERIOD_MS,
    is_internet_play_read_due, probe_plan, punches_now,
};
use serde::{Deserialize, Serialize};

use crate::http::{ClientIdentity, Error, HostApiError, ServerInfo};
use crate::internet;
use crate::stun::{MOONLIGHT_STUN_PORT, MOONLIGHT_STUN_SERVER, STUN_TIMEOUT, find_external_ipv4};

use super::client_pool::HostClientPool;
use super::identity::IdentityStore;
use super::meta::HostMetaStore;
use super::poll::{
    is_site_local_host, parallel_poll_direct, parse_internet_reply, poll_over_the_internet,
    refresh_internet_play,
};
use super::store::{HostEntry, HostStore};

/// Snapshot of a managed host's persisted entry and live reachability/session fields.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostSnapshot {
    pub entry: HostEntry,
    pub state: HostReachState,
    pub active_endpoint: Option<EndpointSlot>,
    pub paired: bool,
    pub another_certificate: bool,
    pub running_game_id: i32,
    pub running_game_uuid: Option<String>,
    pub external_port: u16,
    pub unreachable: Option<String>,
    pub capabilities_json: Option<String>,
    pub alias: Option<String>,
    pub pinned_endpoint: Option<EndpointSlot>,
}

/// Outcome of [`HostManager::add_computer_for_link`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkAddOutcome {
    Added,
    Unreachable,
    CertificateMismatch,
}

/// Hook for resolving external IPv4 via STUN (used by Android when VPN requires `Network.bindSocket`).
pub type ExternalIpv4Resolver = Arc<dyn Fn(u16) -> Option<EndpointSlot> + Send + Sync>;

/// Callback invoked whenever a host snapshot changes meaningfully.
pub type HostUpdateListener = Arc<dyn Fn(&HostSnapshot) + Send + Sync>;

pub(super) struct ManagedTuple {
    pub(super) snapshot: HostSnapshot,
    pub(super) poll_state: PollTupleState,
    last_internet_read_ms: Option<u64>,
    network_lock: Arc<Mutex<()>>,
    pub(super) stop_signal: Arc<(Mutex<bool>, Condvar)>,
    pub(super) poll_thread: Option<JoinHandle<()>>,
    pub(super) removal_tombstone: bool,
}

pub(super) struct Inner {
    epoch: Instant,
    pub(super) store: HostStore,
    pub(super) meta: HostMetaStore,
    #[allow(dead_code)]
    identity_store: IdentityStore,
    pub(super) pool: Arc<HostClientPool>,
    pub(super) tuples: Mutex<HashMap<String, Arc<Mutex<ManagedTuple>>>>,
    pub(super) removal_lock: Mutex<()>,
    polling_active: AtomicBool,
    listener: Mutex<Option<HostUpdateListener>>,
    pub(super) stun_resolver: Mutex<ExternalIpv4Resolver>,
}

/// Default non-VPN direct UDP STUN resolver.
pub fn refresh_external_ipv4_direct(external_port: u16) -> Option<EndpointSlot> {
    let port = if external_port == 0 {
        nightjar_policy::hosts::DEFAULT_HTTP_PORT
    } else {
        external_port
    };
    let ip = find_external_ipv4(MOONLIGHT_STUN_SERVER, MOONLIGHT_STUN_PORT, STUN_TIMEOUT)
        .ok()
        .flatten()?;
    Some(EndpointSlot {
        address: ip.to_string(),
        port,
    })
}

/// Unified host manager coordinating [`HostStore`], [`HostMetaStore`], [`HostClientPool`],
/// per-host polling threads, and host lifecycle mutations.
#[derive(Clone)]
pub struct HostManager {
    pub(super) inner: Arc<Inner>,
}

impl HostManager {
    /// Opens stores in `config_dir`, initializes [`HostClientPool`] with `identity` (or loads from
    /// [`IdentityStore`]), and seeds tuples from `hosts.json`.
    pub fn open(config_dir: &Path, identity: Option<Arc<ClientIdentity>>) -> io::Result<Self> {
        let store = HostStore::open(config_dir);
        let meta = HostMetaStore::open(config_dir);
        let identity_store = IdentityStore::open(config_dir);
        let resolved_identity = match identity {
            Some(id) => id,
            None => {
                let loaded = identity_store
                    .load_or_create(None, None, None)
                    .map_err(io::Error::other)?;
                Arc::new(
                    ClientIdentity::from_pem(
                        &loaded.unique_id,
                        &loaded.certificate_pem,
                        &loaded.private_key_pem,
                    )
                    .map_err(io::Error::other)?,
                )
            }
        };
        let pool = Arc::new(HostClientPool::new(resolved_identity));
        let mgr = Self {
            inner: Arc::new(Inner {
                epoch: Instant::now(),
                store,
                meta,
                identity_store,
                pool,
                tuples: Mutex::new(HashMap::new()),
                removal_lock: Mutex::new(()),
                polling_active: AtomicBool::new(false),
                listener: Mutex::new(None),
                stun_resolver: Mutex::new(Arc::new(refresh_external_ipv4_direct)),
            }),
        };
        mgr.reload_from_store();
        Ok(mgr)
    }

    pub fn store(&self) -> &HostStore {
        &self.inner.store
    }

    pub fn meta(&self) -> &HostMetaStore {
        &self.inner.meta
    }

    pub fn pool(&self) -> &Arc<HostClientPool> {
        &self.inner.pool
    }

    pub fn set_listener(&self, listener: Option<HostUpdateListener>) {
        *self.inner.listener.lock().unwrap_or_else(|e| e.into_inner()) = listener;
    }

    pub fn set_stun_resolver(&self, resolver: ExternalIpv4Resolver) {
        *self
            .inner
            .stun_resolver
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = resolver;
    }

    pub(super) fn now_ms(&self) -> u64 {
        self.inner.epoch.elapsed().as_millis() as u64
    }

    pub(super) fn emit(&self, snapshot: &HostSnapshot) {
        let cb = self
            .inner
            .listener
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(cb) = cb {
            cb(snapshot);
        }
    }

    /// Loads saved entries from `hosts.json` into in-memory tuples.
    pub fn reload_from_store(&self) {
        let saved = self.inner.store.all();
        for mut entry in saved {
            if entry.shared.unique_id.is_empty() {
                continue;
            }
            if !entry.has_android_slots() {
                entry.sync_slots_from_shared();
            }
            self.upsert_tuple(entry);
        }
    }

    /// Inserts or refreshes a tuple in memory and spawns its poll thread if polling is active.
    pub fn upsert_tuple(&self, mut entry: HostEntry) -> HostSnapshot {
        let uuid = entry.shared.unique_id.clone();
        if entry.has_android_slots() {
            entry.sync_shared_from_slots();
        }
        let alias = self.inner.meta.get_alias(&uuid).or(entry.shared.alias.clone());
        let pinned = self
            .inner
            .meta
            .get_pinned(&uuid)
            .or(entry.pinned_endpoint.clone());
        if pinned.is_some() {
            entry.pinned_endpoint = pinned.clone();
            entry.sync_shared_from_slots();
        }
        let capabilities_json = self.inner.meta.load_capabilities(&uuid);

        let tuple_arc = {
            let mut tuples = self.inner.tuples.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(existing) = tuples.get(&uuid) {
                let existing = Arc::clone(existing);
                drop(tuples);
                let mut t = existing.lock().unwrap_or_else(|e| e.into_inner());
                if !t.removal_tombstone {
                    merge_entry_in_memory(&mut t.snapshot.entry, entry);
                    t.snapshot.alias = alias;
                    t.snapshot.pinned_endpoint = pinned.clone();
                    if pinned.is_some() {
                        t.snapshot.active_endpoint = pinned;
                    }
                    if let Some(caps) = capabilities_json {
                        t.snapshot.capabilities_json = Some(caps);
                    }
                }
                let snap = t.snapshot.clone();
                drop(t);
                self.maybe_start_tuple_thread(&existing);
                return snap;
            }
            let active_endpoint = pinned.clone().or_else(|| entry.local_endpoint.clone());
            let snap = HostSnapshot {
                paired: entry.shared.server_certificate.is_some(),
                entry,
                state: HostReachState::Unknown,
                active_endpoint,
                another_certificate: false,
                running_game_id: 0,
                running_game_uuid: None,
                external_port: 0,
                unreachable: None,
                capabilities_json,
                alias,
                pinned_endpoint: pinned,
            };
            let arc = Arc::new(Mutex::new(ManagedTuple {
                snapshot: snap.clone(),
                poll_state: PollTupleState::default(),
                last_internet_read_ms: None,
                network_lock: Arc::new(Mutex::new(())),
                stop_signal: Arc::new((Mutex::new(false), Condvar::new())),
                poll_thread: None,
                removal_tombstone: false,
            }));
            tuples.insert(uuid, Arc::clone(&arc));
            arc
        };
        self.maybe_start_tuple_thread(&tuple_arc);
        tuple_arc
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .snapshot
            .clone()
    }

    pub fn snapshots(&self) -> Vec<HostSnapshot> {
        let tuples = self.inner.tuples.lock().unwrap_or_else(|e| e.into_inner());
        let mut out: Vec<HostSnapshot> = tuples
            .values()
            .filter_map(|arc| {
                let t = arc.lock().unwrap_or_else(|e| e.into_inner());
                (!t.removal_tombstone).then(|| t.snapshot.clone())
            })
            .collect();
        out.sort_by(|a, b| a.entry.shared.unique_id.cmp(&b.entry.shared.unique_id));
        out
    }

    pub fn get_snapshot(&self, uuid: &str) -> Option<HostSnapshot> {
        let arc = self
            .inner
            .tuples
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(uuid)
            .cloned()?;
        let t = arc.lock().unwrap_or_else(|e| e.into_inner());
        (!t.removal_tombstone).then(|| t.snapshot.clone())
    }

    /// Starts periodic background polling for all current and future tuples.
    pub fn start_polling(&self) {
        self.inner.polling_active.store(true, Ordering::Release);
        let arcs: Vec<_> = self
            .inner
            .tuples
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect();
        for arc in arcs {
            self.maybe_start_tuple_thread(&arc);
        }
    }

    /// Stops periodic background polling and joins every tuple's worker thread before returning.
    pub fn stop_polling(&self) {
        self.inner.polling_active.store(false, Ordering::Release);
        let arcs: Vec<_> = self
            .inner
            .tuples
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect();
        let mut handles = Vec::new();
        for arc in arcs {
            let mut t = arc.lock().unwrap_or_else(|e| e.into_inner());
            let (lock, cvar) = &*t.stop_signal;
            *lock.lock().unwrap_or_else(|e| e.into_inner()) = true;
            cvar.notify_all();
            if let Some(h) = t.poll_thread.take() {
                handles.push(h);
            }
        }
        self.inner.pool.clear();
        for h in handles {
            let _ = h.join();
        }
    }

    fn maybe_start_tuple_thread(&self, tuple_arc: &Arc<Mutex<ManagedTuple>>) {
        if !self.inner.polling_active.load(Ordering::Acquire) {
            return;
        }
        let mut t = tuple_arc.lock().unwrap_or_else(|e| e.into_inner());
        if t.removal_tombstone || t.poll_thread.is_some() {
            return;
        }
        let now_ms = self.now_ms();
        t.poll_state.enforce_ttl(now_ms);
        *t.stop_signal.0.lock().unwrap_or_else(|e| e.into_inner()) = false;

        let uuid = t.snapshot.entry.shared.unique_id.clone();
        let stop_signal = Arc::clone(&t.stop_signal);
        let mgr = self.clone();
        let handle = std::thread::Builder::new()
            .name(format!("nj-host-poll-{uuid}"))
            .spawn(move || {
                loop {
                    let _ = mgr.poll_host(&uuid, false);
                    let (lock, cvar) = &*stop_signal;
                    let guard = lock.lock().unwrap_or_else(|e| e.into_inner());
                    if *guard || !mgr.inner.polling_active.load(Ordering::Acquire) {
                        break;
                    }
                    let (guard, _) = cvar
                        .wait_timeout(guard, Duration::from_millis(SERVERINFO_POLL_PERIOD_MS))
                        .unwrap_or_else(|e| e.into_inner());
                    if *guard || !mgr.inner.polling_active.load(Ordering::Acquire) {
                        break;
                    }
                }
            })
            .ok();
        t.poll_thread = handle;
    }

    /// Runs a single poll pass for an existing tracked host UUID.
    ///
    /// Returns `true` if visible state or persisted details changed.
    pub fn poll_host(&self, uuid: &str, force: bool) -> bool {
        let Some(arc) = self
            .inner
            .tuples
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(uuid)
            .cloned()
        else {
            return false;
        };
        let (network_lock, initial_state, mut snap_before, last_internet_read_ms) = {
            let t = arc.lock().unwrap_or_else(|e| e.into_inner());
            if t.removal_tombstone {
                return false;
            }
            if !force && !t.poll_state.is_poll_due(self.now_ms(), false) {
                return false;
            }
            (
                Arc::clone(&t.network_lock),
                t.snapshot.state,
                t.snapshot.clone(),
                t.last_internet_read_ms,
            )
        };

        let _net_guard = network_lock.lock().unwrap_or_else(|e| e.into_inner());
        let cert_der = snap_before.entry.server_certificate_der();
        let slots = snap_before.entry.slots();
        let active_port = snap_before.active_endpoint.as_ref().map(|e| e.port);
        let candidates = probe_plan(
            &slots,
            initial_state == HostReachState::Online,
            active_port,
            snap_before.entry.shared.https_port,
        );
        let likely_online = initial_state == HostReachState::Online;
        let direct = parallel_poll_direct(
            &self.inner.pool,
            &candidates,
            Some(uuid),
            cert_der.as_deref(),
            likely_online,
        );

        let now_ms = self.now_ms();
        let mut winner = direct.winner;
        let mut unreachable_msg: Option<String> = None;

        if winner.is_none() && !direct.another_certificate {
            let active_is_tunnel = snap_before
                .active_endpoint
                .as_ref()
                .is_some_and(|e| nightjar_policy::hosts::is_tunnel_address(&e.address));
            if punches_now(
                slots.local.is_some(),
                initial_state == HostReachState::Offline,
                active_is_tunnel,
            ) && let Some(reply_json) = snap_before.entry.shared.internet_play.as_deref()
            {
                match poll_over_the_internet(
                    &self.inner.pool,
                    uuid,
                    reply_json,
                    Some(uuid),
                    cert_der.as_deref(),
                    direct.tried,
                    likely_online,
                ) {
                    Ok((_, win)) => winner = Some(win),
                    Err(msg) => unreachable_msg = Some(msg),
                }
            } else if let Some(reply) =
                parse_internet_reply(snap_before.entry.shared.internet_play.as_deref())
            {
                unreachable_msg = Some(internet::unreachable(&reply, direct.tried, None));
            }
        }

        let mut internet_play_update: Option<Option<String>> = None;
        let mut resolved_stun_remote: Option<EndpointSlot> = None;
        if let Some(ref win) = winner {
            if snap_before.entry.remote_endpoint.is_none()
                && is_site_local_host(&win.candidate.address)
            {
                let resolver = self
                    .inner
                    .stun_resolver
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();
                resolved_stun_remote = resolver(win.info.external_port);
            }
            if win.info.paired
                && !win.should_clear_cert
                && is_internet_play_read_due(last_internet_read_ms, now_ms)
                && is_site_local_host(&win.candidate.address)
                && let Some(ref cert) = cert_der
            {
                match refresh_internet_play(
                    &self.inner.pool,
                    &win.candidate.address,
                    win.candidate.port,
                    win.info.https_port,
                    cert,
                    &win.info.capabilities,
                ) {
                    Ok(raw) => internet_play_update = Some(Some(raw)),
                    Err(HostApiError::Host(Error::NotFound(_))) => internet_play_update = Some(None),
                    Err(_) => {}
                }
            }
        }

        let _rem_guard = self.inner.removal_lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut t = arc.lock().unwrap_or_else(|e| e.into_inner());
        if t.removal_tombstone {
            return false;
        }
        let changed = match winner {
            Some(win) => {
                t.poll_state.record_poll_success(now_ms);
                let prev = t.snapshot.clone();
                apply_server_info(&mut t.snapshot, &win.info, win.candidate.port, &win.candidate);
                t.snapshot.state = HostReachState::Online;
                t.snapshot.another_certificate = false;
                t.snapshot.unreachable = None;
                if win.should_clear_cert {
                    t.snapshot.entry.shared.server_certificate = None;
                    t.snapshot.paired = false;
                } else {
                    t.snapshot.paired = win.info.paired;
                }
                if let Some(remote) = resolved_stun_remote {
                    t.snapshot.entry.remote_endpoint = Some(remote);
                }
                if let Some(ref raw_update) = internet_play_update {
                    t.snapshot.entry.shared.internet_play = raw_update.clone();
                    t.last_internet_read_ms = Some(now_ms);
                }
                if let Some(caps_json) = win.info.capabilities.to_versioned_json() {
                    let _ = self.inner.meta.save_capabilities(uuid, &caps_json);
                    t.snapshot.capabilities_json = Some(caps_json);
                }
                t.snapshot.entry.sync_shared_from_slots();
                let _ = self.inner.store.upsert(t.snapshot.entry.clone());
                t.snapshot != prev
            }
            None => {
                let offline_now =
                    t.poll_state
                        .record_poll_failure(initial_state == HostReachState::Unknown);
                let prev_state = t.snapshot.state;
                let prev_cert_flag = t.snapshot.another_certificate;
                let prev_unreach = t.snapshot.unreachable.clone();
                if offline_now {
                    t.snapshot.state = HostReachState::Offline;
                }
                t.snapshot.another_certificate = direct.another_certificate;
                if let Some(msg) = unreachable_msg {
                    t.snapshot.unreachable = Some(msg);
                }
                t.snapshot.state != prev_state
                    || t.snapshot.another_certificate != prev_cert_flag
                    || t.snapshot.unreachable != prev_unreach
            }
        };
        let emitted = changed.then(|| t.snapshot.clone());
        drop(t);
        if let Some(snap) = emitted {
            self.emit(&snap);
            return true;
        }
        let _ = &mut snap_before;
        false
    }
}

impl Drop for HostManager {
    fn drop(&mut self) {
        if Arc::strong_count(&self.inner) == 1 {
            self.stop_polling();
        }
    }
}

pub(super) fn apply_server_info(
    snap: &mut HostSnapshot,
    info: &ServerInfo,
    answered_port: u16,
    candidate: &nightjar_policy::hosts::ProbeCandidate,
) {
    if !info.hostname.is_empty() && info.hostname != "UNKNOWN" {
        snap.entry.shared.name = info.hostname.clone();
    }
    if let Some(ref mac) = info.mac_address {
        snap.entry.shared.mac = Some(mac.clone());
    }
    if info.https_port != 0 {
        snap.entry.shared.https_port = info.https_port;
    }
    snap.external_port = info.external_port;
    snap.running_game_id = info.current_game;
    snap.running_game_uuid = info.current_game_uuid.clone();
    snap.active_endpoint = Some(EndpointSlot {
        address: candidate.address.clone(),
        port: answered_port,
    });
    if let Some(ref local_ip) = info.local_ip
        && is_site_local_host(&candidate.address)
    {
        let _ = local_ip;
        snap.entry.local_endpoint = Some(EndpointSlot {
            address: candidate.address.clone(),
            port: answered_port,
        });
    }
}

fn merge_entry_in_memory(dst: &mut HostEntry, src: HostEntry) {
    if !src.shared.name.is_empty() {
        dst.shared.name = src.shared.name;
    }
    if src.shared.mac.is_some() {
        dst.shared.mac = src.shared.mac;
    }
    if src.local_endpoint.is_some() {
        dst.local_endpoint = src.local_endpoint;
    }
    if src.manual_endpoint.is_some() {
        dst.manual_endpoint = src.manual_endpoint;
    }
    if src.ipv6_endpoint.is_some() {
        dst.ipv6_endpoint = src.ipv6_endpoint;
    }
    if src.remote_endpoint.is_some() {
        dst.remote_endpoint = src.remote_endpoint;
    }
    if src.pinned_endpoint.is_some() {
        dst.pinned_endpoint = src.pinned_endpoint;
    }
    if src.shared.server_certificate.is_some() {
        dst.shared.server_certificate = src.shared.server_certificate;
    }
    dst.sync_shared_from_slots();
}
