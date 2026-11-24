//! UPnP IGD gateway discovery, interface search ordering, mapping table inspection, and
//! per-round port addition/removal.

use super::forward_loop::UPNP_LEASE;
use super::leases::{GuestLease, LeaseMethod, LeaseRecord, Mapping};
use super::*;

/// Whether a failed UPnP request was answered by the router: a UPnP error it sent. A request that
/// could not reach it, or got something that is no UPnP answer, was not.
pub(super) fn upnp_answered(error: &igd_next::RequestError) -> bool {
    matches!(
        error,
        igd_next::RequestError::ErrorCode(..) | igd_next::RequestError::UnsupportedAction(_)
    )
}

/// The router's mapping table, whether it was read to the end, and whether the router answered
/// at all. Routers answer the index past the end with 713, but some hide their table (606),
/// answer 714, 402 or 501, or time out: that ends the list without stopping the host from adding
/// its own mappings.
pub(super) async fn entries<P: Provider>(
    gateway: &Gateway<P>,
) -> (Vec<igd_next::PortMappingEntry>, bool, bool) {
    let mut entries = Vec::new();
    for index in 0..4096 {
        match tokio::time::timeout(
            Duration::from_secs(3),
            gateway.get_generic_port_mapping_entry(index),
        )
        .await
        {
            Ok(Ok(entry)) => entries.push(entry),
            Ok(Err(igd_next::GetGenericPortMappingEntryError::SpecifiedArrayIndexInvalid)) => {
                return (entries, true, true);
            }
            Ok(Err(error)) => {
                tracing::debug!(%error, index, "UPnP router stopped listing its mappings");
                let answered = !entries.is_empty()
                    || !matches!(
                        &error,
                        igd_next::GetGenericPortMappingEntryError::RequestError(error)
                            if !upnp_answered(error)
                    );
                return (entries, false, answered);
            }
            Err(_) => {
                tracing::debug!(index, "UPnP router timed out listing its mappings");
                let answered = !entries.is_empty();
                return (entries, false, answered);
            }
        }
    }
    tracing::debug!("UPnP router mapping table exceeds its limit");
    (entries, false, true)
}

pub(super) fn owned(entry: &igd_next::PortMappingEntry, local: IpAddr, description: &str) -> bool {
    entry.internal_client.parse::<IpAddr>().ok() == Some(local)
        && entry.port_mapping_description == description
        && entry.internal_port == entry.external_port
}

/// Whether another mapping already forwards this port to the same port on
/// this PC, as one left by another streaming host does: clients reach the
/// host through it, so it is not a conflict. Only an enabled mapping from any
/// address does: one limited to a remote host, or switched off, forwards no
/// guest of this link.
pub(super) fn forwards_here(entry: &igd_next::PortMappingEntry, local: IpAddr) -> bool {
    entry.enabled
        && matches!(entry.remote_host.trim(), "" | "0.0.0.0" | "*")
        && entry.internal_client.parse::<IpAddr>().ok() == Some(local)
        && entry.internal_port == entry.external_port
}

/// The router's entry for `mapping`, whichever application owns it.
pub(super) fn entry<'a>(
    entries: &'a [igd_next::PortMappingEntry],
    mapping: &Mapping,
) -> Option<&'a igd_next::PortMappingEntry> {
    entries
        .iter()
        .find(|entry| entry.protocol == mapping.protocol && entry.external_port == mapping.port)
}

/// Whether a mapping is this host's to take down: one under its description, or one an
/// unreadable table cannot show, which stays this host's until the table shows another owner.
pub(super) fn ours(
    entries: &[igd_next::PortMappingEntry],
    complete: bool,
    local: IpAddr,
    description: &str,
    mapping: &Mapping,
) -> bool {
    match entry(entries, mapping) {
        Some(entry) => owned(entry, local, description),
        None => !complete,
    }
}

/// This PC's address as the router sees it, for the mappings `gateway` holds.
pub(super) fn local_for(gateway: &Gateway<upnp::Bounded>, bind: IpAddr) -> Result<IpAddr> {
    let socket = UdpSocket::bind((IpAddr::from([0, 0, 0, 0]), 0))?;
    socket.connect(gateway.addr)?;
    Ok(if bind.is_unspecified() || bind.is_ipv6() {
        socket.local_addr()?.ip()
    } else {
        bind
    })
}

/// The interfaces to look for this PC's router from, of its Ethernet and Wi-Fi `interfaces`: the
/// uplinks ([`LanInterface::uplink`](butterpollo_win_io::net::LanInterface::uplink): physical
/// adapters, and Hyper-V's on an external switch) when there are any, as Hyper-V's Default
/// Switch, WSL's and some VPN adapters (TAP, ZeroTier) identify as Ethernet too; otherwise all of
/// them. Lowest IPv4 metric first, as Windows prefers it for its own traffic, so guests' traffic
/// arrives where this PC's leaves; of equal metrics (gigabit Ethernet and fast Wi-Fi both get 25),
/// first the one whose address Windows routes to its default gateway from, then the lower address.
/// A tunnel's or a PPP link's are never among them.
pub(super) fn search_order(
    interfaces: &[butterpollo_win_io::net::LanInterface],
) -> Vec<&butterpollo_win_io::net::LanInterface> {
    let uplink = interfaces.iter().any(|interface| interface.uplink());
    let mut order: Vec<_> = interfaces
        .iter()
        .filter(|interface| interface.uplink() || !uplink)
        .collect();
    order.sort_by_cached_key(|interface| {
        let routed = interface
            .gateways
            .first()
            .and_then(|gateway| route_source(*gateway))
            == Some(IpAddr::V4(interface.address));
        (interface.metric, !routed, interface.address)
    });
    order
}

/// The address Windows sends from towards `gateway`. A UDP connect sends nothing; it only picks
/// the route.
#[cfg(not(test))]
fn route_source(gateway: std::net::Ipv4Addr) -> Option<IpAddr> {
    let socket = UdpSocket::bind((IpAddr::from([0, 0, 0, 0]), 0)).ok()?;
    socket.connect((gateway, 9)).ok()?;
    Some(socket.local_addr().ok()?.ip())
}
#[cfg(test)]
fn route_source(gateway: std::net::Ipv4Addr) -> Option<IpAddr> {
    TEST_NET.with(|net| {
        net.borrow()
            .routes
            .iter()
            .find(|(to, _)| *to == gateway)
            .map(|(_, source)| *source)
    })
}

/// This PC's Ethernet and Wi-Fi interfaces; none when they cannot be read.
#[cfg(not(test))]
pub(super) fn lan_interfaces() -> Vec<butterpollo_win_io::net::LanInterface> {
    butterpollo_win_io::net::lan_interfaces().unwrap_or_else(|error| {
        tracing::debug!(error = %format!("{error:#}"), "the LAN addresses could not be read");
        Vec::new()
    })
}
#[cfg(test)]
pub(super) fn lan_interfaces() -> Vec<butterpollo_win_io::net::LanInterface> {
    TEST_NET.with(|net| net.borrow().lan.clone())
}

/// The addresses to look for this PC's router from, in [`search_order`].
fn lan_ipv4() -> Vec<IpAddr> {
    search_order(&lan_interfaces())
        .into_iter()
        .map(|interface| IpAddr::V4(interface.address))
        .collect()
}

/// The IPv4 router PCP and NAT-PMP ask, and this PC's address towards it: the default gateway of
/// the adapter with the bound address, or for a host listening on every address, of the first in
/// [`search_order`] that has one; a VPN's default route may be listed before the LAN router's.
/// Without such an adapter, the first of the default `gateways`.
pub(super) fn pcp_route(
    interfaces: &[butterpollo_win_io::net::LanInterface],
    gateways: &[IpAddr],
    bind: IpAddr,
) -> Option<(std::net::Ipv4Addr, std::net::Ipv4Addr)> {
    let route = |interface: &butterpollo_win_io::net::LanInterface| {
        interface
            .gateways
            .first()
            .map(|gateway| (*gateway, interface.address))
    };
    match bind {
        IpAddr::V4(bound) if !bound.is_unspecified() => interfaces
            .iter()
            .filter(|interface| interface.address == bound)
            .find_map(route),
        _ => search_order(interfaces).into_iter().find_map(route),
    }
    .or_else(|| portmap::ipv4_route(gateways, bind))
}

/// Whether an interface of this PC that is up has `address`; true when they cannot be read.
#[cfg(not(test))]
pub(super) fn has_address(address: IpAddr) -> bool {
    if_addrs::get_if_addrs().map_or(true, |interfaces| {
        interfaces.iter().any(|interface| {
            interface.oper_status == if_addrs::IfOperStatus::Up && interface.ip() == address
        })
    })
}
#[cfg(test)]
pub(super) fn has_address(address: IpAddr) -> bool {
    TEST_NET.with(|net| {
        net.borrow()
            .addresses
            .as_ref()
            .is_none_or(|addresses| addresses.contains(&address))
    })
}

/// A UPnP search from `from`: the router that answers it.
#[cfg(not(test))]
async fn upnp_search(from: IpAddr) -> Result<Gateway<upnp::Bounded>> {
    upnp::discover(igd_next::SearchOptions {
        timeout: Some(Duration::from_secs(3)),
        bind_addr: (from, 0).into(),
        ..Default::default()
    })
    .await
}
#[cfg(test)]
async fn upnp_search(from: IpAddr) -> Result<Gateway<upnp::Bounded>> {
    let found = TEST_NET.with(|net| {
        let mut net = net.borrow_mut();
        net.searches.push(from);
        net.in_flight += 1;
        net.overlap = net.overlap.max(net.in_flight);
        match net.routers.iter().position(|(on, _)| *on == from) {
            Some(index) => Ok(net.routers.remove(index).1),
            None => anyhow::bail!("no UPnP router answers a search from {from}"),
        }
    });
    // A search waits for answers: others may start, and answer, meanwhile.
    let delay = TEST_NET.with(|net| {
        let net = net.borrow();
        net.delays
            .iter()
            .find(|(on, _)| *on == from)
            .map_or(0, |(_, turns)| *turns)
    });
    for _ in 0..=delay {
        tokio::task::yield_now().await;
    }
    TEST_NET.with(|net| {
        let mut net = net.borrow_mut();
        net.in_flight -= 1;
        net.finished.push(from);
    });
    found
}

/// The address Windows sends a multicast search from. A UDP connect sends nothing; it only
/// picks the route.
#[cfg(not(test))]
fn multicast_source() -> Option<IpAddr> {
    let socket = UdpSocket::bind((IpAddr::from([0, 0, 0, 0]), 0)).ok()?;
    socket.connect(("239.255.255.250", 1900)).ok()?;
    Some(socket.local_addr().ok()?.ip())
}
#[cfg(test)]
fn multicast_source() -> Option<IpAddr> {
    TEST_NET.with(|net| net.borrow().multicast_from)
}

/// The UPnP gateway on this network, and this PC's address towards it. A host that listens on
/// every address searches from each of this PC's Ethernet and Wi-Fi addresses in [`search_order`],
/// and its mappings are for the address the router answered on: a search from no address in
/// particular goes out where Windows sends multicast, which a VPN may have taken. When none of
/// them finds a router, as when it is reached through an adapter of another kind (a network
/// bridge, a NIC team, a VLAN adapter), that search still comes last, as it did before, for the
/// address Windows routes to the router from, unless it would leave from an address already
/// searched from. The searches run at once, so a round without a UPnP router takes one search's
/// time, and the first in that order that finds a router wins.
pub(super) async fn upnp_gateway(bind: IpAddr) -> Result<(Gateway<upnp::Bounded>, IpAddr)> {
    let lan: Vec<IpAddr> = if bind.is_ipv4() && !bind.is_unspecified() {
        Vec::new()
    } else {
        lan_ipv4().into_iter().filter(IpAddr::is_ipv4).collect()
    };
    let mut from = lan.clone();
    if !multicast_source().is_some_and(|source| lan.contains(&source)) {
        from.push(if bind.is_ipv4() {
            bind
        } else {
            IpAddr::from([0, 0, 0, 0])
        });
    }
    let mut searches = tokio::task::JoinSet::new();
    for (rank, address) in from.iter().copied().enumerate() {
        searches.spawn(async move { (rank, upnp_search(address).await) });
    }
    let mut results: Vec<Option<Result<Gateway<upnp::Bounded>>>> =
        from.iter().map(|_| None).collect();
    while let Some(done) = searches.join_next().await {
        let (rank, result) = done?;
        if let Err(error) = &result {
            tracing::debug!(%error, from = %from[rank], "no UPnP router found from this address");
        }
        results[rank] = Some(result);
        // Decided once every address before the first that found a router has found none.
        match results
            .iter()
            .position(|result| !matches!(result, Some(Err(_))))
        {
            Some(rank) if results[rank].is_some() => {
                let Some(Ok(gateway)) = results[rank].take() else {
                    unreachable!()
                };
                let local = match lan.get(rank) {
                    Some(address) => *address,
                    None => local_for(&gateway, bind)?,
                };
                return Ok((gateway, local));
            }
            Some(_) => {}
            None => break,
        }
    }
    results
        .into_iter()
        .rev()
        .find_map(|result| result.and_then(Result::err))
        .map_or_else(|| Err(anyhow::anyhow!("no address to search from")), Err)
}

/// What one UPnP round did: the mappings this host holds, and the ports another mapping
/// already forwards here. A map request the router did not answer is in neither: unknown.
#[derive(Default)]
pub(super) struct Applied {
    pub(super) held: Vec<Mapping>,
    pub(super) forwarded: Vec<Mapping>,
    /// Whether the router answered any request of the round, if only with a refusal.
    pub(super) answered: bool,
    /// The map requests it did not answer.
    pub(super) unanswered: Vec<Mapping>,
}

/// Adds the guest leases among `guest` that this host holds.
fn record_held(record: &std::sync::Mutex<LeaseRecord>, guest: &[Mapping], held: &[Mapping]) {
    record.lock().unwrap().add_all(
        guest
            .iter()
            .filter(|mapping| held.contains(mapping))
            .map(|mapping| GuestLease::new(LeaseMethod::Upnp, *mapping)),
    );
}

/// Drops the recorded leases for `mappings` once the router confirmed them gone: it refused the
/// map request, its delete was answered, or its table shows another owner.
fn record_forget(record: &std::sync::Mutex<LeaseRecord>, mappings: &[Mapping]) {
    record
        .lock()
        .unwrap()
        .remove_confirmed(|method| method == LeaseMethod::Upnp, mappings);
}

/// How long a UPnP map request may take; short in tests, whose routers answer at once.
const UPNP_WAIT: Duration = if cfg!(test) {
    Duration::from_secs(1)
} else {
    Duration::from_secs(3)
};

/// `reported` holds the ports already warned about: the router is asked
/// again every minute, but a continuing conflict is only warned once.
/// `previous` is what the last round left mapped: a router that hides its table still gets the
/// ones no longer wanted removed.
/// A guest lease enters `record` before its map request goes and leaves it only when the router
/// answered (a refusal, a delete it confirmed) or shows another owner.
/// On `trial`, for a router given up for silence, the map requests stop at the first it does
/// not answer, and the rest count as unanswered too: one that holds its requests open costs one
/// wait, not one per port.
#[allow(clippy::too_many_arguments)]
pub(super) async fn apply_round<P: Provider>(
    gateway: &Gateway<P>,
    local: IpAddr,
    wanted: &[Mapping],
    description: &str,
    reported: &mut Vec<Mapping>,
    previous: &[Mapping],
    record: &std::sync::Mutex<LeaseRecord>,
    guest: &[Mapping],
    trial: bool,
) -> Applied {
    let (entries, complete, mut answered) = entries(gateway).await;
    let mut unanswered = Vec::new();
    // The leases listed before this round, to check against a table read to the end below.
    let recorded: Vec<GuestLease> = record.lock().unwrap().leases().to_vec();
    // A table read to the end also shows which recorded guest ports this host no longer holds:
    // gone, or now another owner's. A table that stopped early cannot say.
    if complete {
        let gone: Vec<Mapping> = recorded
            .iter()
            .filter(|lease| lease.method == LeaseMethod::Upnp)
            .map(|lease| lease.mapping())
            .filter(|mapping| {
                !entries.iter().any(|entry| {
                    entry.protocol == mapping.protocol
                        && entry.external_port == mapping.port
                        && owned(entry, local, description)
                })
            })
            .collect();
        record
            .lock()
            .unwrap()
            .remove_confirmed(|method| method == LeaseMethod::Upnp, &gone);
    }
    // The stable host description lets a restarted host reclaim its own
    // permanent leases, including ports removed from a later configuration.
    let mut applied: Vec<_> = entries
        .iter()
        .filter(|entry| owned(entry, local, description))
        .map(|entry| Mapping {
            protocol: entry.protocol,
            port: entry.external_port,
        })
        .collect();
    // A guest port the table shows under this host's description is still this host's.
    record_held(record, guest, &applied);
    let mut forwarded = Vec::new();
    // A table read to the end shows which of them are still this host's; an unreadable one
    // can't, so they stay this host's, as `remove` takes them at shutdown.
    if !complete {
        for mapping in previous {
            if !applied.contains(mapping) {
                applied.push(*mapping);
            }
        }
    }
    for (index, mapping) in wanted.iter().enumerate() {
        if trial && !unanswered.is_empty() {
            unanswered.extend_from_slice(&wanted[index..]);
            break;
        }
        if let Some(entry) = entries.iter().find(|entry| {
            entry.protocol == mapping.protocol
                && entry.external_port == mapping.port
                && !owned(entry, local, description)
        }) {
            // The table answers for this port, and it is not this host's mapping.
            record_forget(record, &[*mapping]);
            if forwards_here(entry, local) {
                // Another mapping already forwards the port to this PC, as a manual forward
                // does: guests reach it at its own number, so it counts as mapped, and the
                // mapping is not this host's to renew or take down.
                tracing::debug!(port=mapping.port, protocol=?mapping.protocol, owner=%entry.port_mapping_description, "UPnP port already forwards to this PC under another mapping");
                reported.retain(|reported| reported != mapping);
                if !forwarded.contains(mapping) {
                    forwarded.push(*mapping);
                }
            } else if reported.contains(mapping) {
                tracing::debug!(port=mapping.port, protocol=?mapping.protocol, "UPnP port is still owned by another mapping");
            } else {
                reported.push(*mapping);
                tracing::warn!(port=mapping.port, protocol=?mapping.protocol, client=%entry.internal_client, owner=%entry.port_mapping_description, "UPnP port is already owned by another mapping; remove it on the router so clients outside your network reach this PC");
            }
            continue;
        }
        // The entry is added before the request: a stop between the two leaves it listed for
        // the next start to take down.
        if guest.contains(mapping) {
            record
                .lock()
                .unwrap()
                .add_all([GuestLease::new(LeaseMethod::Upnp, *mapping)]);
        }
        let result = tokio::time::timeout(
            UPNP_WAIT,
            gateway.add_port(
                mapping.protocol,
                mapping.port,
                (local, mapping.port).into(),
                UPNP_LEASE,
                description,
            ),
        )
        .await;
        let result = if matches!(
            result,
            Ok(Err(igd_next::AddPortError::OnlyPermanentLeasesSupported))
        ) {
            tokio::time::timeout(
                UPNP_WAIT,
                gateway.add_port(
                    mapping.protocol,
                    mapping.port,
                    (local, mapping.port).into(),
                    0,
                    description,
                ),
            )
            .await
        } else {
            result
        };
        match result {
            Ok(Ok(())) => {
                answered = true;
                reported.retain(|reported| reported != mapping);
                if !applied.contains(mapping) {
                    applied.push(*mapping);
                }
            }
            // No answer: whether the mapping exists is unknown, so its lease stays listed. The
            // round says so once, not once per port.
            Ok(Err(igd_next::AddPortError::RequestError(error))) if !upnp_answered(&error) => {
                tracing::debug!(%error, port = mapping.port, "UPnP router did not answer a mapping");
                unanswered.push(*mapping);
            }
            Ok(Err(error)) => {
                answered = true;
                // The router answered, but a refused renewal of a mapping this host still
                // holds is not a confirmation it is gone.
                if !applied.contains(mapping) {
                    record_forget(record, &[*mapping]);
                }
                if matches!(&error, igd_next::AddPortError::PortInUse) {
                    if !reported.contains(mapping) {
                        reported.push(*mapping);
                        tracing::warn!(port=mapping.port, protocol=?mapping.protocol, "UPnP port is already owned by another mapping; remove it on the router so clients outside your network reach this PC");
                    }
                } else {
                    tracing::warn!(%error,port=mapping.port,"UPnP mapping failed");
                }
            }
            Err(error) => {
                tracing::debug!(%error, port = mapping.port, "UPnP mapping timed out");
                unanswered.push(*mapping);
            }
        }
    }
    // This host's mappings that are no longer wanted (the guests' once their links are gone, or
    // a port a later configuration dropped) go now; one the router keeps is tried at shutdown.
    let unwanted: Vec<Mapping> = applied
        .iter()
        .filter(|mapping| !wanted.contains(mapping))
        .copied()
        .collect();
    for mapping in unwanted {
        // A mapping the router now shows under another owner is not this host's: it stays on
        // the router, and stops counting here now that nothing wants it.
        if !ours(&entries, complete, local, description, &mapping) {
            applied.retain(|held| *held != mapping);
            record_forget(record, &[mapping]);
            continue;
        }
        match tokio::time::timeout(
            Duration::from_secs(3),
            gateway.remove_port(mapping.protocol, mapping.port),
        )
        .await
        {
            Ok(Ok(())) => {
                answered = true;
                applied.retain(|held| *held != mapping);
                record_forget(record, &[mapping]);
            }
            Ok(Err(error)) => {
                answered |= !matches!(
                    &error,
                    igd_next::RemovePortError::RequestError(error) if !upnp_answered(error)
                );
                tracing::debug!(%error, port = mapping.port, "UPnP mapping no longer wanted could not be removed");
            }
            Err(_) => {
                tracing::debug!(
                    port = mapping.port,
                    "UPnP router timed out removing a mapping"
                );
                break;
            }
        }
    }
    Applied {
        held: applied,
        forwarded,
        answered,
        unanswered,
    }
}

/// Deletes the mappings `gateway`'s table shows under this host's description for `old`, an
/// address this PC no longer has. Only those it shows: a hidden table keeps them, rather than a
/// guess deleting a port another device holds now.
pub(super) async fn remove_stale(
    gateway: &Gateway<upnp::Bounded>,
    old: IpAddr,
    description: &str,
    record: &std::sync::Mutex<LeaseRecord>,
) {
    let (entries, _, _) = entries(gateway).await;
    let mut gone = Vec::new();
    for entry in entries
        .iter()
        .filter(|entry| owned(entry, old, description))
    {
        match tokio::time::timeout(
            Duration::from_secs(3),
            gateway.remove_port(entry.protocol, entry.external_port),
        )
        .await
        {
            Ok(Ok(())) => gone.push(Mapping {
                protocol: entry.protocol,
                port: entry.external_port,
            }),
            Ok(Err(error)) => {
                tracing::debug!(%error, port = entry.external_port, "UPnP mapping for an old address could not be removed");
            }
            Err(_) => break,
        }
    }
    record_forget(record, &gone);
}

/// Whether `gateway` answers a UPnP request now: one request, for its address.
pub(super) async fn answers(gateway: &Gateway<upnp::Bounded>) -> bool {
    match tokio::time::timeout(Duration::from_secs(3), gateway.get_external_ip()).await {
        Ok(Ok(_)) => true,
        Ok(Err(igd_next::GetExternalIpError::RequestError(error))) => upnp_answered(&error),
        Ok(Err(_)) => true,
        Err(_) => false,
    }
}

/// The console's status while the UPnP router is looked for again: nothing mapped, and no
/// address or error from the router given up.
pub(super) fn searching(status: &mut crate::internet_play::MappingStatus) {
    status.method = None;
    status.external_ipv4 = None;
    status.mapped.clear();
    status.failed.clear();
    status.guest_mapped.clear();
    status.guest_failed.clear();
    status.error = None;
}

/// Removes this host's mappings in `applied` that the router holds, and returns those confirmed
/// gone: a delete it answered, or a table showing another owner. A mapping the router would not
/// remove must not keep the later ones open: all are tried. A router that stopped answering
/// ends it: the next removal would wait as long, and the host's shutdown gives this task ten
/// seconds.
pub(super) async fn remove<P: Provider>(
    gateway: &Gateway<P>,
    local: IpAddr,
    applied: &[Mapping],
    description: &str,
) -> Vec<Mapping> {
    let (entries, complete, _) = entries(gateway).await;
    let mut gone = Vec::new();
    for mapping in applied {
        // A mapping this host added that an unreadable table cannot show is
        // still removed; one the table shows under another owner is not.
        if !ours(&entries, complete, local, description, mapping) {
            gone.push(*mapping);
            continue;
        }
        match tokio::time::timeout(
            Duration::from_secs(3),
            gateway.remove_port(mapping.protocol, mapping.port),
        )
        .await
        {
            Ok(Ok(())) => gone.push(*mapping),
            Ok(Err(error)) => {
                tracing::debug!(%error, port = mapping.port, "UPnP mapping could not be removed");
            }
            Err(_) => {
                tracing::debug!(
                    port = mapping.port,
                    "UPnP router timed out removing a mapping"
                );
                break;
            }
        }
    }
    if gone.len() < applied.len() {
        tracing::warn!(
            "UPnP cleanup left permanent mappings behind; this host reclaims them at its next start"
        );
    }
    gone
}

/// Deletes the recorded UPnP guest leases and drops each from the record only once the router
/// confirmed it gone: a delete it answered, a table showing another owner, or a table read to
/// the end with no entry. A table read that stopped early leaves the rest listed.
pub(super) async fn remove_guest_leftovers<P: Provider>(
    gateway: &Gateway<P>,
    local: IpAddr,
    description: &str,
    record: &std::sync::Mutex<LeaseRecord>,
    recorded: &[GuestLease],
) {
    let (entries, complete, _) = entries(gateway).await;
    for lease in recorded {
        let mapping = lease.mapping();
        let entry = entry(&entries, &mapping);
        if entry.is_some_and(|entry| owned(entry, local, description)) {
            match tokio::time::timeout(
                Duration::from_secs(3),
                gateway.remove_port(mapping.protocol, mapping.port),
            )
            .await
            {
                Ok(Ok(())) => {
                    tracing::debug!(
                        port = mapping.port,
                        "guest port left by an earlier run was removed"
                    );
                    record
                        .lock()
                        .unwrap()
                        .remove_confirmed(|method| method == LeaseMethod::Upnp, &[mapping]);
                }
                Ok(Err(error)) => {
                    tracing::debug!(%error, port = mapping.port, "guest port left by an earlier run could not be removed");
                }
                Err(_) => {
                    tracing::debug!(
                        port = mapping.port,
                        "UPnP router timed out removing a guest port left by an earlier run"
                    );
                    break;
                }
            }
        } else if entry.is_some() {
            // The table shows another owner: the router confirms the port is not this host's.
            record
                .lock()
                .unwrap()
                .remove_confirmed(|method| method == LeaseMethod::Upnp, &[mapping]);
        } else if complete {
            // Read to the end, with no entry: the router no longer holds it.
            record
                .lock()
                .unwrap()
                .remove_confirmed(|method| method == LeaseMethod::Upnp, &[mapping]);
        }
        // A table read that stopped early cannot say: the lease stays listed.
    }
}
