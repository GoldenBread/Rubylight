//! Public address classification, NAT diagnostics, and browser-guest reachability.

use super::{MappingStatus, PortState, enabled, external_ipv4, mapping, service};
use crate::state::Shared;
use anyhow::Result;
use rubylight_traverse::record::Nat;
use serde::Serialize;
use std::net::{IpAddr, SocketAddr};

/// Private, link-local and carrier-grade NAT (100.64/10) addresses: none of them is reachable
/// from the internet.
pub(super) fn public_ipv4(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(ip) => {
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || (ip.octets()[0] == 100 && ip.octets()[1] & 0xc0 == 64))
        }
        IpAddr::V6(_) => false,
    }
}

/// What holds a router whose own internet address is not public, for the note that says why
/// the forwarded ports are out of reach.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InFront {
    /// The provider shares one address among its customers: carrier-grade NAT (100.64/10).
    CarrierGrade,
    /// Another router or modem forwards first (192.168/16 and 172.16/12).
    AnotherRouter,
    /// Either: 10/8 is used both inside homes and by providers behind another NAT.
    Either,
}

fn in_front(address: IpAddr) -> InFront {
    match address {
        IpAddr::V4(ip) if ip.octets()[0] == 100 && ip.octets()[1] & 0xc0 == 64 => {
            InFront::CarrierGrade
        }
        IpAddr::V4(ip)
            if (ip.octets()[0] == 192 && ip.octets()[1] == 168)
                || (ip.octets()[0] == 172 && ip.octets()[1] & 0xf0 == 16) =>
        {
            InFront::AnotherRouter
        }
        _ => InFront::Either,
    }
}

/// Why a router whose own address is not public can't be reached from the internet, in plain
/// words, and what to do about it.
fn not_public(address: IpAddr) -> String {
    match in_front(address) {
        InFront::CarrierGrade => "your internet provider shares one address among its customers (carrier-grade NAT), so nothing on the internet can open a port on it".to_owned(),
        InFront::AnotherRouter => "another router or modem is in front of this one, so nothing on the internet can open a port on it; forward the ports there too, or put it in bridge mode".to_owned(),
        InFront::Either => "your internet provider may share one address among its customers (carrier-grade NAT), or another router or modem may be in front of this one, so nothing on the internet can open a port on it; if it is another router or modem, forward the ports there too or put it in bridge mode".to_owned(),
    }
}

/// Plain-English findings across the mapping and STUN: a router behind carrier-grade NAT, or a
/// second NAT in front of the router.
pub(super) fn notes(mapping: &MappingStatus, stun: &[SocketAddr], nat: Nat) -> Vec<String> {
    let mut notes = Vec::new();
    if let Some(external) = mapping.external_ipv4 {
        if !public_ipv4(external) {
            notes.push(format!(
                "The router's own internet address ({external}) is not public: {}. Devices use a direct internet path instead.",
                not_public(external)
            ));
        } else if let Some(seen) = stun.iter().map(SocketAddr::ip).find(|ip| ip.is_ipv4())
            && seen != external
        {
            notes.push(format!(
                "STUN servers see this PC at {seen}, but the router reports {external}: another router or NAT is in front of it, and forwarded ports may not be reachable."
            ));
        }
    }
    if nat == Nat::Symmetric {
        notes.push(
            "This network gives each destination another port (symmetric NAT): a direct internet path opens only to devices on more open networks."
                .to_owned(),
        );
    }
    notes
}

/// What a browser guest link made for the internet can use (docs/browser-guests.md), for the
/// console's invite dialog: what the router did with the guest ports, the public addresses, and
/// the address the link should carry.
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GuestReach {
    /// The router has not been asked yet since the dialog asked for the ports.
    pub checking: bool,
    /// Whether the port mapping task runs at all.
    pub runs: bool,
    /// UPnP, PCP or NAT-PMP: whichever the router answered.
    pub method: Option<String>,
    /// Why nothing could be mapped.
    pub error: Option<String>,
    /// Each guest port, mapped (with its external port) or refused with the reason.
    pub ports: Vec<PortState>,
    /// The public IPv4 address: the router's own when public, else what STUN servers see.
    pub public_ipv4: Option<IpAddr>,
    /// The router's own internet address, public or not.
    pub router_ipv4: Option<IpAddr>,
    /// This PC's IPv6 address whose pinholes let every guest port in.
    pub ipv6: Option<std::net::Ipv6Addr>,
    /// The address and guest port the link carries; none when nothing on the internet reaches
    /// this PC's guest ports.
    pub address: Option<String>,
    /// Plain-English findings: carrier-grade NAT, another NAT in front, changed port numbers.
    pub notes: Vec<String>,
    /// Whether Play over the internet runs, which lets paired Nightjar devices in too.
    pub internet_play: bool,
    /// Whether the host listens on IPv6, so the router can open the guest ports to it.
    pub ipv6_listening: bool,
    /// When another device joins the running game: only `off` keeps a paired friend from
    /// joining as a guest (`ask` or `allow` lets one in).
    pub guest_join: String,
}

/// What [`guest_reach_of`] knows about this host besides the mapping.
pub(super) struct GuestHost {
    /// The last request for the guest ports.
    pub(super) requested: u64,
    /// The public IPv4 address shown (the router's own when public, else STUN's).
    pub(super) public_ipv4: Option<IpAddr>,
    pub(super) guest_port: u16,
    /// What STUN servers see, with Play over the internet on.
    pub(super) stun: Vec<SocketAddr>,
    pub(super) internet_play: bool,
    pub(super) ipv6_listening: bool,
    pub(super) guest_join: String,
}

/// [`GuestReach`] from the mapping status and the host. A link takes the IPv4 address only when
/// the router that forwarded the ports holds it: its own address is public, and STUN servers,
/// when asked, see the PC there too. Behind carrier-grade NAT or another router the forwarded
/// ports are out of reach whatever STUN sees.
pub(super) fn guest_reach_of(mapping: &MappingStatus, host: &GuestHost) -> GuestReach {
    let GuestHost {
        requested,
        public_ipv4: shown_ipv4,
        guest_port,
        ref stun,
        internet_play,
        ipv6_listening,
        ref guest_join,
    } = *host;
    let mut notes = Vec::new();
    // A guest's browser opens the link's port and offers media to the guest ports by number: a
    // router that maps them to other numbers cannot be used.
    let renumbered = mapping
        .guest_mapped
        .iter()
        .any(|port| port.external != Some(port.port));
    let ipv4_mapped = !mapping.guest_mapped.is_empty() && mapping.guest_failed.is_empty();
    if renumbered {
        notes.push("The router mapped the guest ports to other port numbers, which guests' browsers can't use. Forward them by hand under their own numbers.".to_owned());
    }
    let mut holder = None;
    if let Some(router) = mapping.external_ipv4 {
        if !public_ipv4(router) {
            notes.push(format!(
                "The router's own internet address ({router}) is not public: {}.",
                not_public(router)
            ));
        } else if let Some(seen) = stun.iter().map(SocketAddr::ip).find(IpAddr::is_ipv4)
            && seen != router
        {
            notes.push(format!("STUN servers see this PC at {seen}, but the router reports {router}: another router or NAT is in front of it, so the forwarded ports are out of reach."));
        } else {
            holder = Some(router);
        }
    } else if ipv4_mapped {
        notes.push("The router forwarded the ports but did not say its internet address, so Rubylight can't tell whether guests reach it. Type your public address if you know it works.".to_owned());
    }
    let address = match (holder, mapping.guest_ipv6) {
        (Some(ip), _) if ipv4_mapped && !renumbered => Some(format!("{ip}:{guest_port}")),
        (_, Some(ip)) => Some(format!("[{ip}]:{guest_port}")),
        _ => None,
    };
    let mut ports = mapping.guest_mapped.clone();
    ports.extend(mapping.guest_failed.iter().cloned());
    let guest_join = match guest_join.as_str() {
        "ask" | "allow" => guest_join.clone(),
        _ => "off".to_owned(),
    };
    GuestReach {
        checking: mapping.runs && mapping.guest_round < requested,
        runs: mapping.runs,
        method: mapping.method.clone(),
        error: mapping.error.clone(),
        ports,
        public_ipv4: shown_ipv4,
        router_ipv4: mapping.external_ipv4,
        ipv6: mapping.guest_ipv6,
        address,
        notes,
        internet_play,
        ipv6_listening,
        guest_join,
    }
}

/// `GET /api/guests/internet`: [`GuestReach`] now.
pub fn guest_reach(h: &Shared) -> Result<GuestReach> {
    let config = h.config_snapshot();
    let guest_port = config.ports()?.guest;
    let stun = service()
        .map(|service| service.status().public)
        .unwrap_or_default();
    let ipv6_listening = crate::network::bind_address(&config, None)
        .is_ok_and(|bind| bind.is_ipv6() && !bind.is_loopback());
    Ok(guest_reach_of(
        &mapping(),
        &GuestHost {
            requested: h.guest_net.internet_round(),
            public_ipv4: external_ipv4(),
            guest_port,
            stun,
            internet_play: enabled(&config) && service().is_some(),
            ipv6_listening,
            guest_join: config.get("guest_join", "off").to_owned(),
        },
    ))
}
