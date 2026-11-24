//! The PC's network adapters, as the wake readiness and `/serverinfo`'s MAC read them: from
//! `/sys/class/net`, with the addresses and routes the `net` module reads, and the burned-in
//! address from the driver (`ETHTOOL_GPERMADDR`, which needs no privilege).
use crate::net::Route4;
use anyhow::{Context, Result};
use butterpollo_session::wake::{Adapter, Medium};
use std::{
    net::{IpAddr, Ipv4Addr, UdpSocket},
    path::Path,
};

/// `ARPHRD_ETHER`: Ethernet and Wi-Fi interfaces.
const ETHERNET: u32 = 1;

pub fn read() -> Result<Vec<Adapter>> {
    let routes = crate::net::routes4().unwrap_or_default();
    let addresses = crate::net::prefixed_addresses().context("getifaddrs")?;
    let mut adapters = Vec::new();
    for entry in std::fs::read_dir("/sys/class/net").context("listing /sys/class/net")? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let found = addresses.get(&name).map_or(&[][..], Vec::as_slice);
        if let Some(adapter) = adapter(&entry.path(), &name, found, &routes, permanent(&name)) {
            adapters.push(adapter);
        }
    }
    adapters.sort_by_key(|adapter| adapter.index);
    Ok(adapters)
}

/// The interface `name` whose sysfs folder is `folder`.
fn adapter(
    folder: &Path,
    name: &str,
    addresses: &[(IpAddr, u8)],
    routes: &[Route4],
    permanent: Option<[u8; 6]>,
) -> Option<Adapter> {
    let read = |file: &str| std::fs::read_to_string(folder.join(file)).ok();
    let index = read("ifindex")?.trim().parse().ok()?;
    let kind: u32 = read("type")?.trim().parse().ok()?;
    // A virtual interface (a bridge, a tunnel, a VPN, veth) has no device, or one under
    // /sys/devices/virtual.
    let hardware = std::fs::canonicalize(folder.join("device"))
        .is_ok_and(|device| !device.starts_with("/sys/devices/virtual"));
    let medium = match kind {
        ETHERNET if folder.join("wireless").exists() || folder.join("phy80211").exists() => {
            Medium::Wifi
        }
        ETHERNET => Medium::Wired,
        _ => Medium::Other,
    };
    let mac = (kind == ETHERNET)
        .then(|| read("address").and_then(|text| parse_mac(&text)))
        .flatten();
    Some(Adapter {
        index,
        // The key to the NIC settings: the interface's name, which is stable on Linux.
        guid: name.to_owned(),
        name: name.to_owned(),
        mac,
        permanent: permanent.or(mac),
        hardware,
        connector: hardware,
        medium,
        connected: read("operstate").is_some_and(|state| state.trim() == "up"),
        metric: metric(routes, name).unwrap_or(u32::MAX),
        ipv4: addresses
            .iter()
            .filter_map(|&(address, prefix)| match address {
                IpAddr::V4(address) => Some((address, prefix)),
                IpAddr::V6(_) => None,
            })
            .collect(),
        addresses: addresses.iter().map(|&(address, _)| address).collect(),
    })
}

fn parse_mac(text: &str) -> Option<[u8; 6]> {
    let bytes: Vec<u8> = text
        .trim()
        .split(':')
        .map(|byte| u8::from_str_radix(byte, 16).ok())
        .collect::<Option<_>>()?;
    bytes.try_into().ok()
}

/// The lowest metric of `name`'s routes, its default routes' first.
fn metric(routes: &[Route4], name: &str) -> Option<u32> {
    let lowest = |defaults: bool| {
        routes
            .iter()
            .filter(|route| route.interface == name && (route.default || !defaults))
            .map(|route| route.metric)
            .min()
    };
    lowest(true).or_else(|| lowest(false))
}

/// The PC's address on its preferred default route: the one a socket connected to that
/// route's gateway leaves from. Connecting a UDP socket sends nothing.
pub fn default_route_source() -> Result<Option<IpAddr>> {
    let Some(gateway) = crate::net::routes4()?
        .into_iter()
        .filter(|route| route.default)
        .min_by_key(|route| route.metric)
        .and_then(|route| route.gateway)
    else {
        return Ok(None);
    };
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    socket.connect((gateway, 9))?;
    Ok(Some(socket.local_addr()?.ip()))
}

/// The interface's burned-in address, which a randomised or set address hides; any user may
/// read it.
fn permanent(name: &str) -> Option<[u8; 6]> {
    const SIOCETHTOOL: libc::c_ulong = 0x8946;
    const ETHTOOL_GPERMADDR: u32 = 0x20;
    #[repr(C)]
    struct PermanentAddress {
        cmd: u32,
        size: u32,
        data: [u8; 32],
    }
    let mut request = PermanentAddress {
        cmd: ETHTOOL_GPERMADDR,
        size: 32,
        data: [0; 32],
    };
    // SAFETY: plain data; zero is a valid start.
    let mut ifreq: libc::ifreq = unsafe { std::mem::zeroed() };
    let bytes = name.as_bytes();
    if bytes.len() >= ifreq.ifr_name.len() {
        return None;
    }
    for (to, from) in ifreq.ifr_name.iter_mut().zip(bytes) {
        *to = *from as libc::c_char;
    }
    ifreq.ifr_ifru.ifru_data = (&raw mut request).cast();
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    // SAFETY: an ethtool request on a socket this function owns; `request` outlives the call.
    let status = unsafe {
        libc::ioctl(
            std::os::fd::AsRawFd::as_raw_fd(&socket),
            SIOCETHTOOL as _,
            &raw mut ifreq,
        )
    };
    (status == 0 && request.size == 6)
        .then(|| request.data[..6].try_into().ok())
        .flatten()
        .filter(|mac: &[u8; 6]| *mac != [0; 6])
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROUTES: &str = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
        enp7s0\t00000000\t010AA8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0\n\
        enp7s0\t000AA8C0\t00000000\t0001\t0\t0\t100\t00FFFFFF\t0\t0\t0\n\
        wlan0\t00000000\t0101A8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0\n\
        tun0\t0008000A\t00000000\t0001\t0\t0\t50\t00FFFFFF\t0\t0\t0\n";

    #[test]
    fn an_adapters_metric_is_its_default_routes() {
        let routes = crate::net::parse_routes4(ROUTES);
        assert_eq!(metric(&routes, "enp7s0"), Some(100));
        assert_eq!(metric(&routes, "wlan0"), Some(600));
        assert_eq!(metric(&routes, "tun0"), Some(50));
        assert_eq!(metric(&routes, "eth9"), None);
    }

    /// An interface as sysfs shows it: a wired NIC, a Wi-Fi one with a randomised address,
    /// loopback and a bridge.
    #[test]
    fn an_interfaces_sysfs_folder_is_an_adapter() {
        let root = std::env::temp_dir().join(format!("rubylight-net-{}", std::process::id()));
        let make = |name: &str, files: &[(&str, &str)], device: bool| {
            let folder = root.join(name);
            std::fs::create_dir_all(&folder).unwrap();
            for (file, text) in files {
                std::fs::write(folder.join(file), format!("{text}\n")).unwrap();
            }
            if device {
                // Any real folder outside /sys/devices/virtual stands for a PCI device.
                std::os::unix::fs::symlink(&root, folder.join("device")).unwrap();
            }
            folder
        };
        let wired = make(
            "enp7s0",
            &[
                ("ifindex", "2"),
                ("type", "1"),
                ("address", "f0:2f:74:2e:9e:87"),
                ("operstate", "up"),
            ],
            true,
        );
        let wifi = make(
            "wlan0",
            &[
                ("ifindex", "3"),
                ("type", "1"),
                ("address", "be:ef:73:8b:f5:41"),
                ("operstate", "down"),
            ],
            true,
        );
        std::fs::create_dir_all(wifi.join("wireless")).unwrap();
        let lo = make(
            "lo",
            &[
                ("ifindex", "1"),
                ("type", "772"),
                ("address", "00:00:00:00:00:00"),
                ("operstate", "unknown"),
            ],
            false,
        );
        let bridge = make(
            "virbr0",
            &[
                ("ifindex", "4"),
                ("type", "1"),
                ("address", "52:54:00:12:34:56"),
                ("operstate", "up"),
            ],
            false,
        );
        let address = [(IpAddr::V4(Ipv4Addr::new(192, 168, 10, 230)), 24)];
        let routes = crate::net::parse_routes4(ROUTES);
        let burned = [0x10, 0x20, 0x30, 0x40, 0x50, 0x60];
        let wired = adapter(&wired, "enp7s0", &address, &routes, None).unwrap();
        let wifi = adapter(&wifi, "wlan0", &[], &routes, Some(burned)).unwrap();
        let lo = adapter(&lo, "lo", &[], &routes, None).unwrap();
        let bridge = adapter(&bridge, "virbr0", &[], &routes, None).unwrap();
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(wired.medium, Medium::Wired);
        assert!(wired.physical() && wired.connected);
        assert_eq!(wired.mac, Some([0xf0, 0x2f, 0x74, 0x2e, 0x9e, 0x87]));
        assert_eq!(wired.permanent, wired.mac);
        assert_eq!((wired.index, wired.metric), (2, 100));
        assert_eq!(wired.broadcasts(), [Ipv4Addr::new(192, 168, 10, 255)]);
        assert_eq!(wifi.medium, Medium::Wifi);
        assert!(wifi.physical() && !wifi.connected);
        assert_eq!(wifi.permanent, Some(burned));
        assert_eq!(lo.medium, Medium::Other);
        assert!(!lo.physical() && !bridge.physical());
    }

    /// This PC's adapters, and the default route's source, one of their addresses, whose
    /// adapter's MAC `/serverinfo` reports.
    #[test]
    fn this_pcs_adapters_are_read() {
        let adapters = read().unwrap();
        for adapter in &adapters {
            eprintln!(
                "{} medium {:?} physical {} connected {} metric {} mac {:?} permanent {:?} {:?}",
                adapter.name,
                adapter.medium,
                adapter.physical(),
                adapter.connected,
                adapter.metric,
                adapter.mac,
                adapter.permanent,
                adapter.ipv4
            );
        }
        assert!(
            adapters
                .iter()
                .any(|adapter| adapter.name == "lo" && !adapter.physical())
        );
        if let Some(source) = default_route_source().unwrap() {
            eprintln!("default route from {source}");
            let owner = adapters
                .iter()
                .find(|adapter| adapter.addresses.contains(&source))
                .expect("an adapter has the address");
            // What `/serverinfo` tells a paired client that reached the PC there.
            let reported = crate::net::local_mac(source).unwrap();
            eprintln!("serverinfo MAC for {source}: {reported}");
            if owner.physical() {
                assert_eq!(reported, butterpollo_session::wake::format_mac(owner.mac));
            }
        }
    }
}
