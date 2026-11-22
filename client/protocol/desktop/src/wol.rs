//! Wake-on-LAN socket sender (`if_addrs` + `UdpSocket`), re-exporting pure MAC/target/wait
//! types from `nightjar-desktop-config::wol_types`.

use std::collections::BTreeSet;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6, ToSocketAddrs, UdpSocket};

pub use nightjar_desktop_config::wol_types::{
    magic_packet, normal_mac, ports, targets, Step, WakeWait, Wakes, NO_MAC,
};

use nightjar_desktop_config::config::HostRecord;

#[derive(Default)]
struct Links {
    broadcasts: BTreeSet<Ipv4Addr>,
    ipv6: BTreeSet<u32>,
}

impl Links {
    fn read() -> Self {
        let mut links = Self::default();
        match if_addrs::get_if_addrs() {
            Ok(interfaces) => {
                for interface in interfaces
                    .iter()
                    .filter(|i| i.is_oper_up() && !i.is_loopback())
                {
                    match &interface.addr {
                        if_addrs::IfAddr::V4(v4) => links.broadcasts.extend(v4.broadcast),
                        if_addrs::IfAddr::V6(_) => links.ipv6.extend(interface.index),
                    }
                }
            }
            Err(e) => eprintln!("nightjar: Wake-on-LAN cannot list the network interfaces: {e}"),
        }
        links
    }
}

struct Plan {
    payloads: Vec<[u8; 102]>,
    destinations: BTreeSet<SocketAddr>,
    best_effort: BTreeSet<SocketAddr>,
}

fn plan(host: &HostRecord, links: &Links) -> Result<Plan, String> {
    let mut macs: Vec<String> = Vec::new();
    for mac in host
        .mac
        .iter()
        .chain(&host.wake_macs)
        .chain(&host.own_wake_macs)
        .filter_map(|mac| normal_mac(mac))
    {
        if !macs.contains(&mac) {
            macs.push(mac);
        }
    }
    let payloads: Vec<_> = macs.iter().filter_map(|mac| magic_packet(mac).ok()).collect();
    if payloads.is_empty() {
        return Err(NO_MAC.into());
    }
    let ports = ports(host.http_port);
    let mut addresses = BTreeSet::new();
    for address in std::iter::once(&host.address).chain(&host.other_addresses) {
        match (address.as_str(), 9).to_socket_addrs() {
            Ok(resolved) => addresses.extend(resolved.map(|address| address.ip())),
            Err(e) => eprintln!("nightjar: Wake-on-LAN skips {address}: {e}"),
        }
    }
    addresses.insert(IpAddr::V4(Ipv4Addr::BROADCAST));
    addresses.extend(links.broadcasts.iter().copied().map(IpAddr::V4));
    addresses.extend(
        host.wake_broadcasts
            .iter()
            .filter_map(|b| b.parse::<Ipv4Addr>().ok())
            .map(IpAddr::V4),
    );
    let destinations = addresses
        .iter()
        .flat_map(|address| ports.iter().map(|port| SocketAddr::new(*address, *port)))
        .collect();
    let best_effort = links
        .ipv6
        .iter()
        .flat_map(|scope| {
            ports.iter().map(|port| {
                SocketAddr::V6(SocketAddrV6::new(
                    Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 1),
                    *port,
                    0,
                    *scope,
                ))
            })
        })
        .collect();
    Ok(Plan { payloads, destinations, best_effort })
}

fn send(
    plan: &Plan,
    mut send: impl FnMut(&[u8], SocketAddr) -> io::Result<()>,
) -> Result<(), String> {
    let mut sent = false;
    let mut error = String::from("No Wake-on-LAN destination could be reached.");
    let mut best_effort_failures = 0;
    for _ in 0..3 {
        for payload in &plan.payloads {
            for destination in &plan.destinations {
                match send(payload, *destination) {
                    Ok(()) => sent = true,
                    Err(e) => error = e.to_string(),
                }
            }
            for destination in &plan.best_effort {
                if send(payload, *destination).is_err() {
                    best_effort_failures += 1;
                }
            }
        }
    }
    if best_effort_failures > 0 {
        eprintln!("nightjar: Wake-on-LAN: {best_effort_failures} IPv6 all-nodes sends failed");
    }
    if sent { Ok(()) } else { Err(error) }
}

/// Sends the magic packets for the PC (blocks). Fails when the PC has no usable MAC, or when no
/// packet could be sent at all.
pub fn wake(host: &HostRecord) -> Result<(), String> {
    let plan = plan(host, &Links::read())?;
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).map_err(|e| e.to_string())?;
    socket.set_broadcast(true).map_err(|e| e.to_string())?;
    let ipv6 = UdpSocket::bind("[::]:0").ok();
    send(&plan, |payload, destination| {
        let socket = if destination.is_ipv4() {
            &socket
        } else {
            ipv6.as_ref()
                .ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported, "no IPv6 socket"))?
        };
        socket.send_to(payload, destination).map(|_| ())
    })
}
