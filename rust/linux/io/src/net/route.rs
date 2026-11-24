//! The route to a client on Linux: the interface it leaves by, that interface's medium and speed
//! (`/sys/class/net`), its MTU, and whether the client is on this host's link.
use super::netlink;
use std::{
    collections::BTreeSet,
    net::SocketAddr,
    path::{Path, PathBuf},
};

/// The medium of the route to a client.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Link {
    /// Physical Ethernet speed; zero means unknown.
    pub bps: u64,
    pub wireless: bool,
    /// The kernel's index of the interface the route leaves by; zero means unknown. A host
    /// that changes network adapter has a different path to the same client. It names the
    /// adapter, not the network it is joined to.
    pub interface: u64,
}

/// What `/sys/class/net/<name>` says about one interface.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Interface {
    pub(super) index: u64,
    /// The interface carries traffic (`operstate`).
    pub(super) up: bool,
    /// A device backs it (`device`): a network card rather than a bridge, a VLAN or a tunnel.
    pub(super) hardware: bool,
    /// It frames as Ethernet (`type` 1), which Wi-Fi and most virtual interfaces also do.
    pub(super) ethernet: bool,
    /// Wi-Fi or mobile broadband.
    pub(super) wireless: bool,
    /// The negotiated speed (`speed`); zero when the driver names none.
    pub(super) bps: u64,
}

fn directory(name: &str) -> PathBuf {
    Path::new("/sys/class/net").join(name)
}

fn read(name: &str, file: &str) -> Option<String> {
    std::fs::read_to_string(directory(name).join(file))
        .ok()
        .map(|text| text.trim().to_owned())
}

/// Whether a device backs the interface `name`.
pub(super) fn hardware(name: &str) -> bool {
    directory(name).join("device").exists()
}

/// Whether the interface `name` frames as Ethernet.
pub(super) fn ethernet(name: &str) -> bool {
    read(name, "type").as_deref() == Some("1")
}

/// The interfaces `name` is stacked on: a bridge's ports, a bond's members, a VLAN's parent.
pub(super) fn lowers(name: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(directory(name)) else {
        return Vec::new();
    };
    let mut lowers: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            entry
                .file_name()
                .to_str()?
                .strip_prefix("lower_")
                .map(str::to_owned)
        })
        .collect();
    lowers.sort();
    lowers
}

pub(super) fn interface(name: &str) -> Option<Interface> {
    let index = read(name, "ifindex")?.parse().ok()?;
    let state = read(name, "operstate").unwrap_or_default();
    // Some virtual interfaces never say more than "unknown"; their carrier does.
    let up = state == "up" || (state == "unknown" && read(name, "carrier").as_deref() == Some("1"));
    let device = read(name, "uevent").unwrap_or_default();
    let wireless = directory(name).join("wireless").exists()
        || directory(name).join("phy80211").exists()
        || device
            .lines()
            .any(|line| matches!(line, "DEVTYPE=wlan" | "DEVTYPE=wwan"));
    // Megabits per second; -1, or an error, where the driver names no speed.
    let speed = read(name, "speed")
        .and_then(|speed| speed.parse::<i64>().ok())
        .filter(|speed| *speed > 0)
        .map_or(0, |speed| speed as u64 * 1_000_000);
    Some(Interface {
        index,
        up,
        hardware: hardware(name),
        ethernet: ethernet(name),
        wireless,
        bps: speed,
    })
}

pub(super) fn interface_link(row: &Interface) -> Link {
    if !row.up {
        return Link::default();
    }
    Link {
        interface: row.index,
        bps: if row.ethernet && row.hardware && !row.wireless {
            row.bps
        } else {
            0
        },
        wireless: row.wireless,
    }
}

/// The network card under the virtual interface `name`, when the stack of interfaces below it
/// names exactly one: a team or an ambiguous binding cannot establish the route's medium. A
/// bridge's ports that are neither a card nor stacked on anything (a virtual machine's tap, a
/// container's veth) lead to no network and are not counted.
pub(super) fn underlying_link(
    name: &str,
    lowers: impl Fn(&str) -> Vec<String>,
    mut entry: impl FnMut(&str) -> Option<Interface>,
) -> Link {
    let mut seen = BTreeSet::new();
    let mut name = name.to_owned();
    while seen.insert(name.clone()) {
        let mut below: Vec<(String, Option<Interface>)> = lowers(&name)
            .into_iter()
            .map(|lower| {
                let row = entry(&lower);
                (lower, row)
            })
            .collect();
        if below.len() > 1 {
            below.retain(|(lower, row)| {
                row.is_some_and(|row| row.hardware) || !lowers(lower).is_empty()
            });
        }
        let [(lower, row)] = below.as_slice() else {
            break;
        };
        name = lower.clone();
        let Some(row) = row else { break };
        if !row.up {
            break;
        }
        if row.hardware {
            return interface_link(row);
        }
    }
    Link::default()
}

/// The name of the interface with the kernel's index `index`.
fn interface_name(index: u32) -> Option<String> {
    let mut name = [0u8; libc::IF_NAMESIZE];
    // SAFETY: `name` holds IF_NAMESIZE bytes, the most if_indextoname writes, NUL included.
    let found = unsafe { libc::if_indextoname(index, name.as_mut_ptr().cast()) };
    if found.is_null() {
        return None;
    }
    let length = name.iter().position(|byte| *byte == 0)?;
    String::from_utf8(name[..length].to_vec()).ok()
}

/// The kernel's route to `peer`, IPv4-mapped addresses as IPv4.
fn route(peer: SocketAddr) -> Option<netlink::Route> {
    let scope = match peer {
        SocketAddr::V6(peer) => peer.scope_id(),
        SocketAddr::V4(_) => 0,
    };
    netlink::route(peer.ip().to_canonical(), scope).ok()
}

/// Classify the route, using a virtual Ethernet interface's network card
/// when Linux shows an unambiguous stack of interfaces below it.
pub fn routed_link(peer: SocketAddr) -> Link {
    if peer.ip().to_canonical().is_loopback() {
        return Link::default();
    }
    let Some(name) = route(peer).and_then(|route| interface_name(route.interface)) else {
        return Link::default();
    };
    let Some(row) = interface(&name).filter(|row| row.up) else {
        return Link::default();
    };
    if !row.ethernet || row.hardware || row.wireless {
        return interface_link(&row);
    }
    // The interface the route leaves by, whatever the medium under it.
    Link {
        interface: row.index,
        ..underlying_link(&name, lowers, interface)
    }
}
/// Physical Ethernet speed on the route to this client; zero means unknown.
pub fn routed_link_bps(peer: SocketAddr) -> u64 {
    routed_link(peer).bps
}
/// Whether Linux reaches `peer` without a gateway, so it is on this host's
/// link. A route with a next hop crosses a router that may carry a smaller MTU,
/// which the host cannot see, so jumbo packets are offered only on-link.
pub fn routed_same_link(peer: SocketAddr) -> bool {
    route(peer).is_some_and(|route| route.delivers && !route.gateway)
}

/// The payload MTU of the route Linux sends `peer` by, IPv4 or IPv6:
/// the size a datagram may have before the network fragments it. It is the route's own MTU
/// where it has one (a setting, or what the path reported), else the interface's. None when
/// the lookup fails or there is no route.
pub fn routed_mtu(peer: SocketAddr) -> Option<u32> {
    let route = route(peer).filter(|route| route.delivers)?;
    route.mtu.or_else(|| {
        let name = interface_name(route.interface)?;
        read(&name, "mtu")?.parse().ok()
    })
}
