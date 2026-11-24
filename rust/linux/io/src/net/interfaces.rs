//! This PC's name, addresses, LAN interfaces and default gateways on Linux, read from
//! `getifaddrs`, `/sys/class/net` and the kernel's route tables in `/proc/net`.
use super::route::{ethernet, hardware, lowers};
use anyhow::{Context, Result};
use std::{
    collections::BTreeSet,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
};

/// The PC's host name, which Vibepollo shows to clients by default.
pub fn host_name() -> Option<String> {
    let mut name = [0u8; 256];
    // SAFETY: `name` is writable for the length passed, one less than it holds, so the result
    // is NUL-terminated even when the kernel truncates it.
    if unsafe { libc::gethostname(name.as_mut_ptr().cast(), name.len() - 1) } != 0 {
        return None;
    }
    let length = name.iter().position(|byte| *byte == 0)?;
    Some(String::from_utf8_lossy(&name[..length]).into_owned())
        .filter(|name| !name.trim().is_empty())
}
/// The MAC a magic packet wakes this PC with, for a request that reached it
/// at `address` (IPv4-mapped IPv6 included): the physical NIC carrying it, or
/// the one under a virtual or VPN adapter (`wake::primary`). Zeros for
/// loopback or when no physical adapter is connected.
pub fn local_mac(address: IpAddr) -> Result<String> {
    if address.to_canonical().is_loopback() {
        return Ok(butterpollo_session::wake::format_mac(None));
    }
    Ok(butterpollo_session::wake::reported_mac(
        &Vec::new(), // TEMPORARY-WAKE &crate::wake::adapters()?,
        address,
        // No adapter's wake settings have been read: each is as yet unknown.
        |_| None,
    ))
}

/// One address of one interface, as `getifaddrs` lists it.
struct Address {
    interface: String,
    /// The interface is up and has a carrier (`IFF_UP` and `IFF_RUNNING`).
    running: bool,
    address: IpAddr,
}

/// Every IPv4 and IPv6 address of every interface.
fn addresses() -> std::io::Result<Vec<Address>> {
    let mut list = std::ptr::null_mut();
    // SAFETY: getifaddrs fills `list`, a live out-pointer, with a list it allocates.
    if unsafe { libc::getifaddrs(&mut list) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut found = Vec::new();
    let mut next = list;
    // SAFETY: the list's records stay valid until freeifaddrs below; each name is a
    // NUL-terminated string, and an address is read as the sockaddr its family says it is.
    unsafe {
        while let Some(row) = next.as_ref() {
            next = row.ifa_next;
            let Some(socket) = row.ifa_addr.as_ref() else {
                continue;
            };
            let address = match i32::from(socket.sa_family) {
                libc::AF_INET => {
                    let v4 = &*row.ifa_addr.cast::<libc::sockaddr_in>();
                    IpAddr::V4(Ipv4Addr::from(v4.sin_addr.s_addr.to_ne_bytes()))
                }
                libc::AF_INET6 => {
                    let v6 = &*row.ifa_addr.cast::<libc::sockaddr_in6>();
                    IpAddr::V6(Ipv6Addr::from(v6.sin6_addr.s6_addr))
                }
                _ => continue,
            };
            let wanted = (libc::IFF_UP | libc::IFF_RUNNING) as u32;
            found.push(Address {
                interface: std::ffi::CStr::from_ptr(row.ifa_name)
                    .to_string_lossy()
                    .into_owned(),
                running: row.ifa_flags & wanted == wanted,
                address,
            });
        }
        libc::freeifaddrs(list);
    }
    Ok(found)
}

/// The unicast addresses of interfaces that are up, without loopback and
/// link-local ones.
pub fn unicast_addresses() -> Result<Vec<IpAddr>> {
    let mut addresses: Vec<IpAddr> = addresses()?
        .into_iter()
        .filter(|row| row.running)
        .map(|row| row.address.to_canonical())
        .filter(|address| {
            !address.is_loopback()
                && !address.is_unspecified()
                && !matches!(address, IpAddr::V4(v4) if v4.is_link_local())
                && !matches!(address, IpAddr::V6(v6) if v6.is_unicast_link_local())
        })
        .collect();
    addresses.sort();
    addresses.dedup();
    Ok(addresses)
}

/// One IPv4 route of the kernel's main table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Route4 {
    pub(super) interface: String,
    /// The router the route goes through; None for a route straight to its destination.
    pub(super) gateway: Option<Ipv4Addr>,
    /// The route is to everywhere (0.0.0.0/0).
    pub(super) default: bool,
    pub(super) metric: u32,
}

/// `RTF_UP` and `RTF_GATEWAY`: the route is usable, and goes through a router.
const UP: u32 = 1;
const GATEWAY: u32 = 2;

/// The routes in `/proc/net/route`'s text: a line of column names, then one line per route
/// with its addresses as the hexadecimal of the bytes in memory.
pub(super) fn parse_routes4(text: &str) -> Vec<Route4> {
    let address = |field: &str| {
        u32::from_str_radix(field, 16)
            .ok()
            .map(|value| Ipv4Addr::from(value.to_ne_bytes()))
    };
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let [interface, destination, gateway, flags, _, _, metric, mask, ..] = fields[..]
            else {
                return None;
            };
            let flags = u32::from_str_radix(flags, 16).ok()?;
            if flags & UP == 0 {
                return None;
            }
            Some(Route4 {
                interface: interface.to_owned(),
                gateway: address(gateway)
                    .filter(|gateway| flags & GATEWAY != 0 && !gateway.is_unspecified()),
                default: address(destination)?.is_unspecified() && address(mask)?.is_unspecified(),
                metric: metric.parse().ok()?,
            })
        })
        .collect()
}

/// The routers of the IPv6 default routes in `/proc/net/ipv6_route`'s text: per route the
/// destination and its prefix length, the source and its, the next hop, the metric, two
/// counters, the flags and the interface, the numbers in hexadecimal.
pub(super) fn parse_gateways6(text: &str) -> Vec<IpAddr> {
    text.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let [destination, prefix, _, _, next, _, _, _, flags, _] = fields[..] else {
                return None;
            };
            let flags = u32::from_str_radix(flags, 16).ok()?;
            let unspecified = |field: &str| field.len() == 32 && field.bytes().all(|b| b == b'0');
            if flags & (UP | GATEWAY) != UP | GATEWAY
                || !unspecified(destination)
                || u8::from_str_radix(prefix, 16).ok()? != 0
            {
                return None;
            }
            let next = IpAddr::V6(Ipv6Addr::from(u128::from_str_radix(next, 16).ok()?));
            Some(next.to_canonical()).filter(|next| !next.is_unspecified())
        })
        .collect()
}

fn routes4() -> Result<Vec<Route4>> {
    let text = std::fs::read_to_string("/proc/net/route").context("reading /proc/net/route")?;
    Ok(parse_routes4(&text))
}

/// The next hops of every default route (0.0.0.0/0 and ::/0) in the kernel's main tables: the
/// routers this PC sends the internet through. One read of each table, no network traffic.
pub fn default_gateways() -> Result<Vec<IpAddr>> {
    let mut gateways: Vec<IpAddr> = routes4()?
        .into_iter()
        .filter(|route| route.default)
        .filter_map(|route| route.gateway.map(IpAddr::V4))
        .collect();
    // Absent on a PC with IPv6 turned off.
    if let Ok(text) = std::fs::read_to_string("/proc/net/ipv6_route") {
        gateways.extend(parse_gateways6(&text));
    }
    gateways.sort();
    gateways.dedup();
    Ok(gateways)
}
/// Active Ethernet/Wi-Fi IPv4 addresses for Moonlight's manual Add PC flow.
pub fn lan_addresses() -> Result<Vec<String>> {
    Ok(lan_interfaces()?
        .into_iter()
        .map(|interface| interface.address.to_string())
        .collect())
}
/// An active Ethernet or Wi-Fi interface's IPv4 address, as [`lan_interfaces`] lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LanInterface {
    pub address: Ipv4Addr,
    /// A physical adapter: a device backs the interface (`/sys/class/net/<name>/device`).
    pub hardware: bool,
    /// A virtual interface stacked on a physical adapter (a bridge, bond or VLAN over one):
    /// Linux's counterpart of Hyper-V's adapter on an external switch, which carries the
    /// address of the card it shares.
    pub hyper_v: bool,
    /// The metric of the interface's IPv4 default route, else the lowest of its other
    /// routes: of two routes alike, Linux takes the lower. 0 without any route.
    pub metric: u32,
    /// The interface's IPv4 default gateways.
    pub gateways: Vec<Ipv4Addr>,
}
impl LanInterface {
    /// Whether this PC reaches its router through the interface: a physical adapter, or
    /// a bridge, bond or VLAN over one that carries the default gateway. A container's or a
    /// virtual machine manager's bridge has no default gateway (this PC is theirs), and VPN
    /// interfaces that frame as Ethernet (TAP, ZeroTier) are neither.
    pub fn uplink(&self) -> bool {
        self.hardware || (self.hyper_v && !self.gateways.is_empty())
    }
}

/// Whether the virtual interface `name` is stacked, at any depth, on a physical adapter.
fn over_hardware(name: &str) -> bool {
    let mut seen = BTreeSet::new();
    let mut pending = vec![name.to_owned()];
    while let Some(name) = pending.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        for lower in lowers(&name) {
            if hardware(&lower) {
                return true;
            }
            pending.push(lower);
        }
    }
    false
}

/// The IPv4 metric and default gateways of `interface` among `routes`.
pub(super) fn routed(routes: &[Route4], interface: &str) -> (u32, Vec<Ipv4Addr>) {
    let own = || routes.iter().filter(|route| route.interface == interface);
    let mut gateways: Vec<Ipv4Addr> = Vec::new();
    for gateway in own().filter(|route| route.default).filter_map(|route| route.gateway) {
        if !gateways.contains(&gateway) {
            gateways.push(gateway);
        }
    }
    let lowest = |default: bool| {
        own()
            .filter(|route| route.default == default)
            .map(|route| route.metric)
            .min()
    };
    (lowest(true).or(lowest(false)).unwrap_or(0), gateways)
}

/// Active Ethernet/Wi-Fi IPv4 addresses with what tells their interfaces apart: bridges,
/// container and virtual machine networks and some VPN interfaces (TAP, ZeroTier) also frame
/// as Ethernet. Uplinks ([`LanInterface::uplink`]) first, then private addresses first.
pub fn lan_interfaces() -> Result<Vec<LanInterface>> {
    let routes = routes4().unwrap_or_default();
    let mut interfaces: Vec<LanInterface> = Vec::new();
    for row in addresses()? {
        let IpAddr::V4(address) = row.address else {
            continue;
        };
        if !row.running
            || address.is_loopback()
            || address.is_link_local()
            || address.is_unspecified()
            || !ethernet(&row.interface)
        {
            continue;
        }
        let hardware = hardware(&row.interface);
        let (metric, gateways) = routed(&routes, &row.interface);
        interfaces.push(LanInterface {
            address,
            hardware,
            hyper_v: !hardware && over_hardware(&row.interface),
            metric,
            gateways,
        });
    }
    // Bridges and container networks also frame as Ethernet. Prefer an interface this PC
    // reaches its router through, so first-run instructions point at a reachable LAN address.
    interfaces.sort_by_key(|interface| {
        (
            !interface.uplink(),
            !interface.address.is_private(),
            interface.address,
        )
    });
    interfaces.dedup_by_key(|interface| interface.address);
    Ok(interfaces)
}
