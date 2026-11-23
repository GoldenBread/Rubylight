//! Wake-on-LAN planning (with DNS resolution), UDP burst sending, and persistent
//! `wake_targets.json` storage shared across Nightjar clients.

pub mod store;
#[cfg(test)]
mod tests;

use std::io;
use std::net::{Ipv4Addr, SocketAddr, ToSocketAddrs, UdpSocket};

pub use nightjar_policy::wol::{
    Links, MAX_BROADCASTS, MAX_MACS, NO_MAC, Plan, WAKE_POLL_INTERVAL_MS, WakeLaunch, WakeStep,
    WakeTargetsPollState, WakeWait, Wakes, Waking, build_plan, extract_targets, magic_packet,
    normal_mac, parse_ipv4_literal, parse_macs, ports, targets,
};
pub use store::{WakeTargetEntry, WakeTargetsStore};

/// Host inputs needed to plan a Wake-on-LAN burst.
pub struct WakeHost<'a> {
    pub mac: Option<&'a str>,
    pub endpoints: &'a [(String, u16)],
    pub wake_macs: &'a [String],
    pub wake_broadcasts: &'a [String],
    pub own_wake_macs: &'a [String],
    pub fallback_http_port: u16,
}

/// Resolves `host.endpoints` via DNS and delegates to [`build_plan`].
pub fn plan(host: &WakeHost<'_>, links: &Links) -> Result<Plan, String> {
    let macs: Vec<&str> = host
        .mac
        .into_iter()
        .chain(host.wake_macs.iter().map(String::as_str))
        .chain(host.own_wake_macs.iter().map(String::as_str))
        .collect();

    let mut resolved_endpoints = Vec::new();
    let mut endpoint_ports = Vec::new();
    for (address, port) in host.endpoints {
        endpoint_ports.push(*port);
        match (address.as_str(), 9).to_socket_addrs() {
            Ok(resolved) => {
                for addr in resolved {
                    resolved_endpoints.push((addr.ip(), *port));
                }
            }
            Err(e) => eprintln!("nightjar: Wake-on-LAN skips {address}: {e}"),
        }
    }
    if endpoint_ports.is_empty() {
        endpoint_ports.push(host.fallback_http_port);
    }

    let wake_broadcasts: Vec<&str> = host.wake_broadcasts.iter().map(String::as_str).collect();
    build_plan(
        &macs,
        &resolved_endpoints,
        &endpoint_ports,
        &wake_broadcasts,
        links,
    )
}

/// Sends `plan` in a 3-burst loop using `send`; fails only when no primary destination
/// succeeded (`best_effort` IPv6 all-nodes failures alone do not fail the wake).
pub fn send(
    plan: &Plan,
    mut send: impl FnMut(&[u8], SocketAddr) -> io::Result<()>,
) -> Result<(), String> {
    let mut sent = false;
    let mut error = String::from("No Wake-on-LAN destination could be reached.");
    let mut best_effort_failures = 0usize;
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

/// Binds IPv4 (broadcast enabled) and optional IPv6 UDP sockets and transmits `plan`.
pub fn send_udp(plan: &Plan) -> Result<(), String> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).map_err(|e| e.to_string())?;
    socket.set_broadcast(true).map_err(|e| e.to_string())?;
    let ipv6 = UdpSocket::bind("[::]:0").ok();
    send(plan, |payload, destination| {
        let sock = if destination.is_ipv4() {
            &socket
        } else {
            ipv6.as_ref()
                .ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported, "no IPv6 socket"))?
        };
        sock.send_to(payload, destination).map(|_| ())
    })
}
