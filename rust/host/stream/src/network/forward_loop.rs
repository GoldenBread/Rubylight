//! Port forwarding renewal loop, held router state, and UPnP renewal coordination.

use super::leases::{LeaseMethod, LeaseRecord, Mapping, mapped_ports, report, wanted_now};
use super::pcp_pinholes::{cleanup_guests, renew_pcp_v4, renew_pinholes};
use super::upnp_forward::{
    answers, apply_round, has_address, remove, remove_stale, searching, upnp_gateway,
};
use super::*;

/// The UPnP and IPv6 pinhole task, which sleeps until its next renewal or until
/// it is told that the host stops.
pub struct PortForward {
    task: tokio::task::JoinHandle<()>,
    stop: std::sync::Arc<tokio::sync::Notify>,
}
impl PortForward {
    /// The host stops: after `h.stop` is set, this wakes the task, which takes its
    /// mappings down and ends.
    pub fn stop(&self) {
        self.stop.notify_one();
    }
    /// Waits up to `wait` for the task to take its mappings down, then ends it.
    pub async fn finish(mut self, wait: Duration) {
        if tokio::time::timeout(wait, &mut self.task).await.is_err() {
            self.task.abort();
        }
    }
}

/// How often the port mapping task renews the mappings.
pub(super) const RENEWAL: Duration = Duration::from_secs(60);
/// The lease of a UPnP mapping, in seconds: two renewals.
pub(super) const UPNP_LEASE: u32 = 120;
/// The renewals in a row a UPnP gateway may leave unanswered before it is given up and the
/// router looked for again: as many as its lease spans. A router that misses fewer still holds
/// the mappings it last granted (one silent round costs nothing); by then their lease has run
/// out, so keeping the gateway keeps nothing open, and an answer in between starts the count again.
pub(super) const UPNP_SILENT_RENEWALS: u32 = UPNP_LEASE / RENEWAL.as_secs() as u32;

/// Waits until `renewal` or until `stop` is notified, whichever comes first.
pub(super) async fn until_renewal_or_stop(stop: &tokio::sync::Notify, renewal: Instant) {
    let wait = renewal.saturating_duration_since(Instant::now());
    tokio::select! {
        () = stop.notified() => {}
        () = tokio::time::sleep(wait) => {}
    }
}

/// What the port mapping task holds on the routers: the UPnP gateway and what it mapped there,
/// the PCP or NAT-PMP mapper, and the IPv6 pinholes.
#[derive(Default)]
pub(super) struct Held {
    pub(super) upnp: Option<(Gateway<upnp::Bounded>, IpAddr, Vec<Mapping>)>,
    /// The renewals in a row the UPnP gateway answered nothing to.
    pub(super) upnp_misses: u32,
    /// When the first of those rounds started.
    pub(super) upnp_silent_since: Option<Instant>,
    /// The address and control URL of the UPnP gateway last given up for answering nothing: a
    /// search that finds it again takes it back only once it answers.
    pub(super) upnp_silent: Option<(SocketAddr, String)>,
    /// The gateway's address and the address of this PC's that its mappings were made for, once
    /// this PC no longer has that address: the same gateway found again has them deleted, as it
    /// would not forward their ports to the new address.
    pub(super) upnp_stale: Option<(SocketAddr, IpAddr)>,
    pub(super) mapper: Option<portmap::PortMapper>,
    pub(super) firewall: Option<ipv6::Firewall>,
    pub(super) pinholes: Vec<portmap::PortMapper>,
}
impl Held {
    pub(super) fn any(&self) -> bool {
        self.upnp.is_some()
            || self.mapper.is_some()
            || self.firewall.is_some()
            || !self.pinholes.is_empty()
    }
    /// Takes every mapping and pinhole down: nothing is wanted any more, or the host stops.
    /// A recorded guest lease leaves `record` only when the router confirmed its delete; one it
    /// did not answer for stays listed for the next start.
    pub(super) async fn release(
        &mut self,
        description: &str,
        record: &std::sync::Mutex<LeaseRecord>,
    ) {
        if let Some(mut firewall) = self.firewall.take()
            && let Err(error) = firewall.close().await
        {
            tracing::debug!(%error, "IPv6 pinholes will expire after their finite lease");
        }
        // A pinhole is gone only when every (address, gateway) mapper confirmed its delete.
        let mut confirmed: Option<Vec<Mapping>> = None;
        for mut mapper in self.pinholes.drain(..) {
            let answered = mapper.close().await;
            confirmed = Some(match confirmed {
                None => answered,
                Some(previous) => previous
                    .into_iter()
                    .filter(|mapping| answered.contains(mapping))
                    .collect(),
            });
        }
        if let Some(confirmed) = confirmed
            && !confirmed.is_empty()
        {
            record
                .lock()
                .unwrap()
                .remove_confirmed(|method| method == LeaseMethod::PcpV6, &confirmed);
        }
        if let Some(mut mapper) = self.mapper.take() {
            let confirmed = mapper.close().await;
            record.lock().unwrap().remove_confirmed(
                |method| matches!(method, LeaseMethod::Pcp | LeaseMethod::NatPmp),
                &confirmed,
            );
        }
        if let Some((gateway, local, applied)) = self.upnp.take() {
            let confirmed = remove(&gateway, local, &applied, description).await;
            record
                .lock()
                .unwrap()
                .remove_confirmed(|method| method == LeaseMethod::Upnp, &confirmed);
        }
    }
}

/// Maps the ports [`wanted_now`] names, renews them every minute and takes them down when they
/// are no longer wanted or the host stops. Runs with `upnp` on, and with the WebRTC server on for
/// browser guests' links made for the internet, which map only the guest ports.
pub fn port_forward(h: Shared, bind: IpAddr) -> Option<PortForward> {
    let config = h.config.read().unwrap().clone();
    let upnp = config.boolean("upnp", false) && !bind.is_loopback();
    let runs = !bind.is_loopback() && (upnp || config.webrtc_server());
    crate::internet_play::update_mapping(|status| {
        *status = crate::internet_play::MappingStatus {
            enabled: upnp,
            runs,
            ..Default::default()
        };
    });
    if !runs {
        // A record can name guest leases a previous run left on the router, even when the
        // settings that asked for them were turned off since: take them down once, then end.
        if bind.is_loopback() {
            return None;
        }
        let record = std::sync::Arc::new(std::sync::Mutex::new(LeaseRecord::load(&h.directory)));
        if record.lock().unwrap().leases().is_empty() {
            return None;
        }
        let description = format!("Butterpollo Rust {}", h.paired.read().unwrap().unique_id);
        let nonce_secret = h.identity.private_pem.clone();
        let stop = std::sync::Arc::new(tokio::sync::Notify::new());
        let stopped = stop.clone();
        let task = tokio::spawn(async move {
            tokio::select! {
                () = stopped.notified() => {}
                _ = cleanup_guests(&record, bind, &description, nonce_secret.as_bytes()) => {}
            }
        });
        return Some(PortForward { task, stop });
    }
    let stop = std::sync::Arc::new(tokio::sync::Notify::new());
    let stopped = stop.clone();
    let task = tokio::spawn(async move {
        let ports = config.ports().unwrap();
        let description = format!("Butterpollo Rust {}", h.paired.read().unwrap().unique_id);
        // Makes the PCP mapping nonce, which must survive new mappers and restarts.
        let nonce_secret = h.identity.private_pem.clone();
        let record = std::sync::Arc::new(std::sync::Mutex::new(LeaseRecord::load(&h.directory)));
        // Take down what an unclean stop left in its own task, beside this loop: the stream
        // ports map at once, and a guest link waits only for this one cleanup round.
        let cleaning = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
            !record.lock().unwrap().leases().is_empty(),
        ));
        let cleanup_done = std::sync::Arc::new(tokio::sync::Notify::new());
        let cleanup = {
            let record = record.clone();
            let stopped = stopped.clone();
            let cleaning = cleaning.clone();
            let done = cleanup_done.clone();
            let description = description.clone();
            let secret = nonce_secret.clone();
            tokio::spawn(async move {
                if cleaning.load(Ordering::Acquire) {
                    tokio::select! {
                        () = stopped.notified() => {}
                        () = cleanup_guests(&record, bind, &description, secret.as_bytes()) => {}
                    }
                }
                cleaning.store(false, Ordering::Release);
                done.notify_one();
            })
        };
        let mut next_search = Instant::now();
        let mut held = Held::default();
        let mut reported = Vec::new();
        while !h.stop.load(Ordering::Acquire) {
            if Instant::now() >= next_search {
                next_search = Instant::now() + RENEWAL;
                // Read before the guests are asked: a request made during this round is
                // served by the next one.
                let round = h.guest_net.internet_round();
                let cleaning_now = cleaning.load(Ordering::Acquire);
                let (stream, mut guest) = wanted_now(
                    &config,
                    ports,
                    upnp,
                    h.guest_net.open(),
                    h.guest_net.internet_wanted(),
                );
                // The cleanup deletes guest ports this host held: a guest link's map requests
                // wait for it, so its deletes never race them.
                if cleaning_now {
                    guest.clear();
                }
                let wanted: Vec<Mapping> = stream.iter().chain(&guest).copied().collect();
                if wanted.is_empty() {
                    if held.any() {
                        held.release(&description, &record).await;
                        reported.clear();
                    }
                    // A guest question is answered only by a round that could map its ports.
                    let guest_round = if cleaning_now { 0 } else { round };
                    crate::internet_play::update_mapping(|status| {
                        *status = crate::internet_play::MappingStatus {
                            enabled: upnp,
                            runs,
                            guest_round,
                            ..Default::default()
                        };
                    });
                } else {
                    renew(
                        &mut held,
                        bind,
                        &stream,
                        &guest,
                        &wanted,
                        &description,
                        nonce_secret.as_bytes(),
                        &mut reported,
                        &record,
                    )
                    .await;
                    if !cleaning_now {
                        crate::internet_play::update_mapping(|status| status.guest_round = round);
                    }
                }
            }
            let wake_at = h
                .guest_net
                .next_expiry()
                .map_or(next_search, |expiry| next_search.min(expiry));
            tokio::select! {
                () = until_renewal_or_stop(&stopped, wake_at) => {
                    if wake_at < next_search && Instant::now() >= wake_at {
                        next_search = Instant::now();
                    }
                }
                // A link made or revoked for the internet: map or unmap the guest ports now.
                () = h.guest_net.mapping_changed() => next_search = Instant::now(),
                // The start cleanup finished: a guest link asked for meanwhile goes now.
                () = cleanup_done.notified() => next_search = Instant::now(),
            }
        }
        // The cleanup shares the record, so let it finish before the final release: neither
        // then races the other on the same guest ports. The loop may have taken the stop
        // notification, so wake the cleanup too.
        stopped.notify_waiters();
        let _ = cleanup.await;
        held.release(&description, &record).await;
    });
    Some(PortForward { task, stop })
}

/// One round of mapping `wanted` (the streaming ports `stream` and the guests' `guest`): UPnP
/// IGD, else PCP or NAT-PMP, for IPv4, and IPv6 pinholes on a host that listens on IPv6.
#[allow(clippy::too_many_arguments)]
pub(super) async fn renew(
    held: &mut Held,
    bind: IpAddr,
    stream: &[Mapping],
    guest: &[Mapping],
    wanted: &[Mapping],
    description: &str,
    nonce_secret: &[u8],
    reported: &mut Vec<Mapping>,
    record: &std::sync::Mutex<LeaseRecord>,
) {
    // The rounds' times count from their start: a round waits on the routers for a while.
    let started = Instant::now();
    let mut upnp_error = None;
    // This PC no longer has the address the mappings were made for (another network, another
    // address from the router): look for the router again. Only the interfaces are read, not the
    // routes, so a VPN that takes the default route leaves a router that answers alone.
    if let Some((gateway, local, _)) = held.upnp.take_if(|(_, local, _)| !has_address(*local)) {
        tracing::info!(address = %local, "this PC no longer has the address its UPnP port mappings were made for; looking for the router again");
        held.upnp_stale = Some((gateway.addr, local));
        held.upnp_misses = 0;
        held.upnp_silent_since = None;
        held.upnp_silent = None;
        crate::internet_play::update_mapping(searching);
    }
    let mut searched = false;
    let mut on_trial = false;
    loop {
        if held.upnp.is_none() {
            searched = true;
            match upnp_gateway(bind).await {
                Ok((gateway, local)) => {
                    let given_up = held.upnp_silent.as_ref().is_some_and(|(addr, control)| {
                        *addr == gateway.addr && *control == gateway.control_url
                    });
                    if given_up && !answers(&gateway).await {
                        // A busy router may miss that one request: it gets the round's port
                        // requests too, and is taken back only if it answers one of them.
                        on_trial = true;
                        held.upnp = Some((gateway, local, Vec::new()));
                    } else {
                        if given_up {
                            tracing::info!("the UPnP router answers again");
                        } else {
                            tracing::info!(router = %gateway.addr, address = %local, "UPnP router found");
                        }
                        held.upnp_silent = None;
                        // The same router still forwards ports to the address this PC had: it
                        // would refuse them to the new one, so those go first. Nothing made for
                        // another address or router is made again as it was.
                        if let Some((router, old)) = held.upnp_stale.take()
                            && router == gateway.addr
                            && old != local
                        {
                            remove_stale(&gateway, old, description, record).await;
                        }
                        held.upnp = Some((gateway, local, Vec::new()));
                    }
                }
                Err(error) => {
                    tracing::debug!(%error, "UPnP gateway unavailable; retrying");
                    upnp_error = Some(format!("{error:#}"));
                }
            }
        }
        let Some((gateway, local, applied)) = held.upnp.as_mut() else {
            break;
        };
        let outcome = apply_round(
            gateway,
            *local,
            wanted,
            description,
            reported,
            applied,
            record,
            guest,
            on_trial,
        )
        .await;
        *applied = outcome.held;
        if std::mem::take(&mut on_trial) {
            if outcome.answered {
                tracing::info!("the UPnP router answers again");
                held.upnp_silent = None;
            } else {
                // It answers the search and nothing else: PCP and NAT-PMP get the round, and it
                // is tried again next round.
                tracing::debug!("the UPnP router found answers no requests");
                upnp_error = Some("the UPnP router found answers no requests".to_owned());
                held.upnp = None;
                break;
            }
        }
        if outcome.answered {
            if held.upnp_misses > 0 {
                tracing::info!("the UPnP router answers again");
            }
            held.upnp_misses = 0;
            held.upnp_silent_since = None;
        } else {
            held.upnp_misses += 1;
            held.upnp_silent_since.get_or_insert(started);
            if held.upnp_misses == 1 {
                tracing::warn!(
                    "the UPnP router did not answer the port mapping renewal; its mappings stay as they are while it may still hold them"
                );
            }
        }
        let forwarded = outcome.forwarded;
        // A router that answered nothing is not asked for its address: the one it gave stands.
        let external = if outcome.answered {
            tokio::time::timeout(Duration::from_secs(3), gateway.get_external_ip())
                .await
                .ok()
                .and_then(|result| result.ok())
        } else {
            None
        };
        let mapped = mapped_ports(applied, &forwarded);
        let refused: Vec<(Mapping, String)> = reported
            .iter()
            .map(|mapping| {
                (
                    *mapping,
                    "another mapping on the router already uses this port".to_owned(),
                )
            })
            .chain(
                outcome
                    .unanswered
                    .iter()
                    .map(|mapping| (*mapping, "the router did not answer".to_owned())),
            )
            .collect();
        crate::internet_play::update_mapping(|status| {
            status.method = Some("UPnP".into());
            if outcome.answered {
                status.external_ipv4 = external;
            }
            report(status, stream, guest, &mapped, &refused);
            status.error = (!outcome.answered && mapped.is_empty())
                .then(|| "the UPnP router found on this network does not answer".to_owned());
        });
        // Missed renewals count only as far apart as renewals are: a round a guest link asked
        // for in between adds none of the lease's time.
        let lease_out = held.upnp_misses >= UPNP_SILENT_RENEWALS
            && held.upnp_silent_since.is_some_and(|since| {
                // A second's slack: a round due a renewal later may start a hair early.
                started.duration_since(since) + Duration::from_secs(1)
                    >= RENEWAL * (UPNP_SILENT_RENEWALS - 1)
            });
        if !lease_out {
            break;
        }
        // Its mappings' lease has run out unrenewed: look for the router again now, and without
        // one, PCP and NAT-PMP get this round.
        tracing::warn!(
            renewals = held.upnp_misses,
            "the UPnP router answered none of the last renewals; looking for the router again"
        );
        if let Some((gateway, _, _)) = held.upnp.take() {
            held.upnp_silent = Some((gateway.addr, gateway.control_url));
        }
        held.upnp_misses = 0;
        held.upnp_silent_since = None;
        crate::internet_play::update_mapping(searching);
        if searched {
            break;
        }
    }
    if held.upnp.is_none() {
        renew_pcp_v4(
            held,
            bind,
            stream,
            guest,
            wanted,
            nonce_secret,
            reported,
            record,
            upnp_error.as_deref(),
        )
        .await;
    }
    if bind.is_ipv6() {
        renew_pinholes(held, bind, guest, wanted, nonce_secret, record).await;
    }
}
