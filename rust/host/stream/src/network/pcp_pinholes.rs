//! PCP/NAT-PMP IPv4 fallback mapping, IPv6 firewall/PCP pinhole renewal, and startup
//! guest-lease cleanup.

use super::forward_loop::Held;
use super::leases::{GuestLease, LeaseMethod, LeaseRecord, Mapping, recorded_leases, report};
use super::upnp_forward::{lan_interfaces, pcp_route, remove_guest_leftovers, upnp_gateway};
use super::*;

/// Deletes the recorded leases through `mapper`, and drops only the ones it answered for.
pub(super) async fn delete_recorded(
    mapper: &mut portmap::PortMapper,
    record: &std::sync::Mutex<LeaseRecord>,
    leases: &[GuestLease],
) {
    let mappings: Vec<Mapping> = leases.iter().map(|lease| lease.mapping()).collect();
    let answered = mapper.delete(&mappings).await;
    record.lock().unwrap().remove_confirmed(
        |method| matches!(method, LeaseMethod::Pcp | LeaseMethod::NatPmp),
        &answered,
    );
}

/// Deletes the recorded PCP or NAT-PMP guest leases, and drops each from the record once the
/// router answered its delete. PCP checks this host's nonce, so another program's mapping on
/// the port is refused rather than deleted; NAT-PMP has no nonce, so only the recorded ports
/// are asked.
async fn remove_mapped(
    record: &std::sync::Mutex<LeaseRecord>,
    bind: IpAddr,
    leases: &[GuestLease],
    secret: &[u8],
) {
    if leases.is_empty() {
        return;
    }
    let Some((gateway, local)) = default_gateways()
        .ok()
        .and_then(|gateways| pcp_route(&lan_interfaces(), &gateways, bind))
    else {
        tracing::debug!("no default gateway for the PCP or NAT-PMP cleanup");
        return;
    };
    let Ok(mut mapper) = portmap::PortMapper::ipv4(gateway, local, secret).await else {
        return;
    };
    delete_recorded(&mut mapper, record, leases).await;
}

/// The router's IPv6 gateways, for PCP pinholes.
fn ipv6_gateways() -> Vec<std::net::Ipv6Addr> {
    default_gateways()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|gateway| match gateway {
            IpAddr::V6(ip) => Some(ip),
            IpAddr::V4(_) => None,
        })
        .collect()
}

/// Deletes the recorded PCP pinholes through every IPv6 router and address this PC has, and
/// drops each from the record only when every router asked answered its delete.
async fn remove_pinholes(
    record: &std::sync::Mutex<LeaseRecord>,
    leases: &[GuestLease],
    secret: &[u8],
) {
    if leases.is_empty() {
        return;
    }
    let locals = portmap::ipv6_locals(IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED));
    let mappings: Vec<Mapping> = leases.iter().map(|lease| lease.mapping()).collect();
    // A mapping is gone only when every router and address asked confirmed it.
    let mut confirmed: Option<Vec<Mapping>> = None;
    for (local, index) in locals {
        for gateway in &ipv6_gateways() {
            let Ok(mut mapper) = portmap::PortMapper::ipv6(*gateway, local, index, secret).await
            else {
                continue;
            };
            let answered = mapper.delete(&mappings).await;
            confirmed = Some(match confirmed {
                None => answered,
                Some(previous) => previous
                    .into_iter()
                    .filter(|mapping| answered.contains(mapping))
                    .collect(),
            });
        }
    }
    if let Some(confirmed) = confirmed {
        record
            .lock()
            .unwrap()
            .remove_confirmed(|method| method == LeaseMethod::PcpV6, &confirmed);
    }
}

/// One round at host start, in its own task beside the renewal loop: takes down the guest ports
/// an unclean stop left forwarded, and only the leases `record` lists, so another
/// application's mapping on the same port stays. Nothing here holds the renewal loop back, and
/// each lease leaves the record only when a router confirmed it gone. With no record it asks
/// the router nothing.
pub(super) async fn cleanup_guests(
    record: &std::sync::Mutex<LeaseRecord>,
    bind: IpAddr,
    description: &str,
    secret: &[u8],
) {
    let leases = record.lock().unwrap().leases().to_vec();
    if leases.is_empty() {
        return;
    }
    let upnp = recorded_leases(&leases, |method| method == LeaseMethod::Upnp);
    let upnp_answered = if upnp.is_empty() {
        false
    } else {
        match upnp_gateway(bind).await {
            Ok((gateway, local)) => {
                remove_guest_leftovers(&gateway, local, description, record, &upnp).await;
                true
            }
            Err(error) => {
                tracing::debug!(%error, "UPnP gateway unavailable for the guest port cleanup");
                false
            }
        }
    };
    let mapped = recorded_leases(&leases, |method| {
        matches!(method, LeaseMethod::Pcp | LeaseMethod::NatPmp)
    });
    if !mapped.is_empty() {
        if upnp_answered {
            // The router already answered UPnP: asking it for PCP or NAT-PMP too would hold
            // the cleanup back for one timeout per port on a router that ignores them. Those
            // leases stay listed and expire with their own lifetime.
            tracing::debug!("PCP or NAT-PMP guest leases kept: the router answered UPnP");
        } else {
            remove_mapped(record, bind, &mapped, secret).await;
        }
    }
    // UPnP IGDv2 pinholes last 120 seconds, so they are not recorded: only PCP pinholes are.
    let pinholes = recorded_leases(&leases, |method| method == LeaseMethod::PcpV6);
    if !pinholes.is_empty() {
        remove_pinholes(record, &pinholes, secret).await;
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn renew_pcp_v4(
    held: &mut Held,
    bind: IpAddr,
    stream: &[Mapping],
    guest: &[Mapping],
    wanted: &[Mapping],
    nonce_secret: &[u8],
    reported: &mut Vec<Mapping>,
    record: &std::sync::Mutex<LeaseRecord>,
    upnp_error: Option<&str>,
) {
    let route = default_gateways()
        .ok()
        .and_then(|gateways| pcp_route(&lan_interfaces(), &gateways, bind));
    if let Some((gateway, local)) = route {
        // Another router or address: dropping the old mapper asks its router to delete the
        // leases, without waiting on a router that may be gone.
        if held
            .mapper
            .as_ref()
            .is_some_and(|mapper| !mapper.serves(gateway, local))
        {
            held.mapper = None;
        }
        if held.mapper.is_none() {
            held.mapper = portmap::PortMapper::ipv4(gateway, local, nonce_secret)
                .await
                .ok();
        }
    }
    // The mapper's own method once known: a NAT-PMP round then neither rewrites the record
    // nor adds a second method every minute. While unknown, PCP is what it asks first.
    let known = held.mapper.as_ref().and_then(portmap::PortMapper::method);
    let preadded = match known {
        Some(portmap::Method::NatPmp) => LeaseMethod::NatPmp,
        _ => LeaseMethod::Pcp,
    };
    let outcome = match held.mapper.as_mut() {
        Some(mapper) => {
            // Entries go in before the map requests: a stop in between leaves them listed.
            record.lock().unwrap().add_all(
                guest
                    .iter()
                    .map(|mapping| GuestLease::new(preadded, *mapping)),
            );
            mapper.renew(wanted).await
        }
        None => portmap::Outcome {
            error: Some(anyhow::anyhow!("no default gateway")),
            ..Default::default()
        },
    };
    // What the router answered before any silence still holds: refused mappings are not
    // this host's, and answered deletes are gone.
    if let Some(method) = held.mapper.as_ref().and_then(portmap::PortMapper::method) {
        let method = match method {
            portmap::Method::Pcp => LeaseMethod::Pcp,
            portmap::Method::NatPmp => LeaseMethod::NatPmp,
        };
        let mapped: Vec<Mapping> = guest
            .iter()
            .filter(|mapping| outcome.mapped.iter().any(|(done, _)| done == *mapping))
            .copied()
            .collect();
        let renewed: Vec<Mapping> = guest
            .iter()
            .filter(|mapping| outcome.renewed.contains(mapping))
            .copied()
            .collect();
        let refused = &outcome.refused;
        let removed = &outcome.removed;
        let mut record = record.lock().unwrap();
        if method == LeaseMethod::NatPmp && known != Some(portmap::Method::NatPmp) {
            // The pre-added PCP entries are these leases under the protocol the router
            // answered with: rewrite them once, not every round.
            record.remove_confirmed(
                |held| matches!(held, LeaseMethod::Pcp | LeaseMethod::NatPmp),
                &mapped,
            );
            record.add_all(
                mapped
                    .iter()
                    .map(|mapping| GuestLease::new(LeaseMethod::NatPmp, *mapping)),
            );
        }
        record.granted(method, &renewed);
        record.remove_confirmed(
            |held| matches!(held, LeaseMethod::Pcp | LeaseMethod::NatPmp),
            refused,
        );
        record.remove_confirmed(
            |held| matches!(held, LeaseMethod::Pcp | LeaseMethod::NatPmp),
            removed,
        );
    }
    if let Some(error) = &outcome.error {
        // The mapper and its leases stay for next time: the router may answer then, and
        // the same nonce renews what it mapped.
        let message = format!(
            "the router answers none of UPnP, PCP and NAT-PMP: turn UPnP on in the router, or forward the ports by hand ({})",
            upnp_error.unwrap_or("no UPnP router found")
        );
        tracing::debug!(error = %format!("{error:#}"), "PCP and NAT-PMP unavailable; retrying");
        crate::internet_play::update_mapping(|status| {
            status.method = None;
            // No router maps anything: an address a router gave before is not this one's.
            status.external_ipv4 = None;
            report(status, stream, guest, &outcome.mapped, &outcome.failed);
            status.error = Some(message);
        });
    } else {
        for (mapping, reason) in &outcome.failed {
            if !reported.contains(mapping) {
                reported.push(*mapping);
                tracing::warn!(port = mapping.port, protocol = ?mapping.protocol, %reason, "port mapping refused");
            }
        }
        let method = held
            .mapper
            .as_ref()
            .and_then(portmap::PortMapper::method)
            .map(|method| method.name().to_owned());
        let external = held.mapper.as_ref().and_then(|mapper| mapper.external);
        crate::internet_play::update_mapping(|status| {
            status.method = method;
            status.external_ipv4 = external;
            report(status, stream, guest, &outcome.mapped, &outcome.failed);
            status.error = None;
        });
    }
}

/// IPv6 pinholes for `wanted` through the router's UPnP IGDv2 firewall, else PCP. UPnP IGDv2
/// pinholes last 120 seconds and are not recorded; PCP pinholes are, before their requests go,
/// and leave the record only when the router answered.
pub(super) async fn renew_pinholes(
    held: &mut Held,
    bind: IpAddr,
    guest: &[Mapping],
    wanted: &[Mapping],
    nonce_secret: &[u8],
    record: &std::sync::Mutex<LeaseRecord>,
) {
    if held.firewall.is_none() {
        held.firewall = ipv6::discover(bind).await.ok();
    }
    if let Some(firewall) = held.firewall.as_mut() {
        let result = firewall.renew(wanted).await;
        if let Err(error) = &result {
            tracing::debug!(%error, "IPv6 firewall pinholes could not be renewed");
        }
        let renewed = result.is_ok();
        let pinholes = if renewed { wanted.len() } else { 0 };
        let guest_ipv6 = (renewed && !guest.is_empty()).then(|| firewall.local());
        let error = result.err().map(|error| format!("{error:#}"));
        crate::internet_play::update_mapping(|status| {
            status.ipv6_method = Some("UPnP".into());
            status.ipv6_pinholes = pinholes;
            status.ipv6_error = error;
            status.guest_ipv6 = guest_ipv6;
        });
        return;
    }
    let gateways = ipv6_gateways();
    let locals = portmap::ipv6_locals(bind);
    held.pinholes.retain(|mapper| {
        locals.iter().any(|&(local, index)| {
            gateways
                .iter()
                .any(|&gateway| mapper.serves_ipv6(gateway, local, index))
        })
    });
    for (local, index) in locals {
        for &gateway in &gateways {
            if held
                .pinholes
                .iter()
                .any(|mapper| mapper.serves_ipv6(gateway, local, index))
            {
                continue;
            }
            if let Ok(mapper) = portmap::PortMapper::ipv6(gateway, local, index, nonce_secret).await
            {
                held.pinholes.push(mapper);
            }
        }
    }
    if !held.pinholes.is_empty() {
        // Entries go in before the map requests: a stop in between leaves them listed.
        record.lock().unwrap().add_all(
            guest
                .iter()
                .map(|mapping| GuestLease::new(LeaseMethod::PcpV6, *mapping)),
        );
    }
    let mut opened = 0;
    let mut last_error = None;
    let mut guest_ipv6 = None;
    let mut renewed = Vec::new();
    // A pinhole is gone only when every (address, gateway) mapper confirmed it, as the start
    // cleanup does.
    let mut gone: Option<Vec<Mapping>> = None;
    for mapper in &mut held.pinholes {
        let outcome = mapper.renew(wanted).await;
        if let Some(error) = &outcome.error {
            last_error = Some(format!("{error:#}"));
        }
        opened += outcome.mapped.len();
        for mapping in &outcome.renewed {
            if guest.contains(mapping) {
                renewed.push(*mapping);
            }
        }
        let mut mapper_gone: Vec<Mapping> = outcome.refused.clone();
        mapper_gone.extend(outcome.removed.iter().copied());
        gone = Some(match gone {
            None => mapper_gone,
            Some(previous) => previous
                .into_iter()
                .filter(|mapping| mapper_gone.contains(mapping))
                .collect(),
        });
        let all_guests = guest
            .iter()
            .all(|port| outcome.mapped.iter().any(|(done, _)| done == port));
        if !guest.is_empty()
            && all_guests
            && let IpAddr::V6(local) = mapper.local()
        {
            guest_ipv6.get_or_insert(local);
        }
    }
    if !renewed.is_empty() {
        record.lock().unwrap().granted(LeaseMethod::PcpV6, &renewed);
    }
    if let Some(gone) = gone
        && !gone.is_empty()
    {
        record
            .lock()
            .unwrap()
            .remove_confirmed(|method| method == LeaseMethod::PcpV6, &gone);
    }
    if opened == 0 {
        // Search again next time: the router may answer later.
        held.pinholes.clear();
    }
    crate::internet_play::update_mapping(|status| {
        status.ipv6_method = (opened > 0).then(|| "PCP".into());
        status.ipv6_pinholes = opened;
        status.ipv6_error = (opened == 0).then(|| {
            last_error.unwrap_or_else(|| {
                "the router opens no IPv6 pinholes (neither UPnP IGDv2 nor PCP)".to_owned()
            })
        });
        status.guest_ipv6 = guest_ipv6;
    });
}
