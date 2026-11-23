//! Wake-on-LAN socket sender (`if_addrs` + `nightjar_core::wol`), re-exporting pure
//! MAC/target/wait types from `nightjar_core::wol`.

pub use nightjar_core::wol::{WakeStep as Step, WakeWait, Wakes, targets};

use nightjar_core::wol::{Links, WakeHost, plan, send_udp};
use nightjar_desktop_config::config::HostRecord;

fn read_links() -> Links {
    let mut links = Links::default();
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

/// Sends the magic packets for the PC (blocks). Fails when the PC has no usable MAC, or when no
/// packet could be sent at all.
pub fn wake(host: &HostRecord) -> Result<(), String> {
    let endpoints: Vec<(String, u16)> = std::iter::once(&host.address)
        .chain(&host.other_addresses)
        .map(|addr| (addr.clone(), host.http_port))
        .collect();
    let wake_host = WakeHost {
        mac: host.mac.as_deref(),
        endpoints: &endpoints,
        wake_macs: &host.wake_macs,
        wake_broadcasts: &host.wake_broadcasts,
        own_wake_macs: &host.own_wake_macs,
        fallback_http_port: host.http_port,
    };
    let plan = plan(&wake_host, &read_links())?;
    send_udp(&plan)
}
