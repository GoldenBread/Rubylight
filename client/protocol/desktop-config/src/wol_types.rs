//! Wake-on-LAN pure targets, magic packets, resend schedule and burst planning without
//! interface discovery or socket I/O (delegating to `nightjar_policy::wol`).

use std::io;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};

pub use nightjar_policy::wol::{
    Links, MAX_MACS, NO_MAC, Plan, WakeStep as Step, WakeWait, Wakes, Waking, build_plan,
    magic_packet, normal_mac, parse_macs, ports, targets,
};

use crate::config::HostRecord;

/// The packets for the PC: one payload for serverinfo's MAC and each MAC its `/wake` listed, to
/// the PC's addresses, to 255.255.255.255, to the subnet broadcasts of this device and of the PC,
/// and to `ff02::1`. Resolving the PC's addresses may block on DNS.
pub fn plan(host: &HostRecord, links: &Links) -> Result<Plan, String> {
    let macs: Vec<&str> = host
        .mac
        .as_deref()
        .into_iter()
        .chain(host.wake_macs.iter().map(String::as_str))
        .chain(host.own_wake_macs.iter().map(String::as_str))
        .collect();
    let mut resolved_endpoints: Vec<(IpAddr, u16)> = Vec::new();
    for address in std::iter::once(&host.address).chain(&host.other_addresses) {
        match (address.as_str(), 9).to_socket_addrs() {
            Ok(resolved) => {
                for addr in resolved {
                    resolved_endpoints.push((addr.ip(), host.http_port));
                }
            }
            Err(e) => eprintln!("nightjar: Wake-on-LAN skips {address}: {e}"),
        }
    }
    let wake_broadcasts: Vec<&str> = host.wake_broadcasts.iter().map(String::as_str).collect();
    build_plan(
        &macs,
        &resolved_endpoints,
        &[host.http_port],
        &wake_broadcasts,
        links,
    )
}

/// Sends the plan through `send`; fails only when no packet reached a destination that is not
/// best effort.
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

#[cfg(test)]
mod tests;
