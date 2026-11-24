//! Readable waits, QoS, interface and route queries on loopback, and change notifications.
use super::super::{
    InterfaceChanges, LanInterface, Link, QosFlow, default_gateways,
    interfaces::{parse_gateways6, parse_routes4, routed},
    lan_addresses, lan_interfaces, local_mac,
    route::{Interface, interface_link, underlying_link},
    routed_link, routed_link_bps, routed_mtu, routed_same_link, set_dont_fragment, wait_readable,
    wait_readable_or_wake, warm_up_qos,
};
use anyhow::Result;
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    os::fd::AsRawFd,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

#[test]
fn waiting_for_input_wakes_when_a_datagram_arrives() -> Result<()> {
    let receiver = UdpSocket::bind("127.0.0.1:0")?;
    let sender = UdpSocket::bind("127.0.0.1:0")?;
    assert!(!wait_readable(receiver.as_raw_fd(), 0)?);
    sender.send_to(b"input", receiver.local_addr()?)?;
    // Generous: it returns at once on a datagram; only a missed wake would take the second.
    assert!(wait_readable(receiver.as_raw_fd(), 5000)?);
    Ok(())
}

#[test]
fn a_wait_on_a_descriptor_that_is_no_socket_is_an_error_not_a_timeout() {
    // No test process has a million descriptors open, so this number names nothing.
    let error = wait_readable(1_000_000, 0).expect_err("unopened descriptor");
    assert_eq!(error.raw_os_error(), Some(libc::EBADF));
}

#[test]
fn control_wait_wakes_for_feedback_without_client_input() -> Result<()> {
    let control = UdpSocket::bind("127.0.0.1:0")?;
    let feedback = UdpSocket::bind("127.0.0.1:0")?;
    let sender = UdpSocket::bind("127.0.0.1:0")?;
    let wait = |timeout| wait_readable_or_wake(control.as_raw_fd(), feedback.as_raw_fd(), timeout);
    assert!(!wait(0)?);
    let address = feedback.local_addr()?;
    let thread = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(20));
        sender.send_to(&[1], address)
    });
    assert!(wait(5000)?);
    thread
        .join()
        .map_err(|_| anyhow::anyhow!("sender panicked"))??;
    feedback.recv(&mut [0])?;
    assert!(!wait(0)?);
    let sender = UdpSocket::bind("127.0.0.1:0")?;
    sender.send_to(&[1], control.local_addr()?)?;
    assert!(wait(5000)?);
    Ok(())
}

#[test]
fn qos_leaves_clients_it_cannot_tag_alone() -> Result<()> {
    assert!(!warm_up_qos());
    let socket = UdpSocket::bind("127.0.0.1:0")?;
    assert!(QosFlow::new(&socket, "127.0.0.1:9".parse()?, false)?.is_none());
    let dual = UdpSocket::bind("[::]:0");
    if let Ok(dual) = dual {
        assert!(QosFlow::new(&dual, "[::ffff:192.0.2.10]:9".parse()?, true)?.is_none());
    }
    Ok(())
}

#[test]
fn loopback_including_mapped_ipv4_keeps_fast_pacing_without_a_link_cap() {
    for peer in ["127.0.0.1:9", "[::1]:9", "[::ffff:127.0.0.1]:9"] {
        let link = routed_link(peer.parse().unwrap_or(SocketAddr::from(([0; 4], 0))));
        assert_eq!(link, Link::default(), "{peer}");
        assert_eq!(
            butterpollo_media_policy::network_pacing::rate_bps(
                0,
                50_000,
                link.bps,
                link.wireless,
                65_000_000
            ),
            800_000_000
        );
        assert_eq!(
            routed_link_bps(peer.parse().unwrap_or(SocketAddr::from(([0; 4], 0)))),
            0
        );
    }
}

#[test]
fn a_loopback_route_reports_an_mtu_and_a_socket_can_refuse_fragmentation() -> Result<()> {
    let mut peers = vec![("127.0.0.1:9".parse::<SocketAddr>()?, "127.0.0.1:0")];
    if UdpSocket::bind("[::1]:0").is_ok() {
        peers.push(("[::1]:9".parse()?, "[::1]:0"));
    }
    for (peer, bind) in peers {
        // The loopback pseudo-interface carries any datagram size.
        let mtu = routed_mtu(peer).unwrap_or(0);
        assert!(mtu >= 1500, "{peer}: {mtu}");
        // Loopback is on-link, so jumbo packets may be offered there.
        assert!(routed_same_link(peer), "{peer}");
        assert_eq!(
            rubylight_protocol::video_packet::for_mtu(mtu),
            rubylight_protocol::video_packet::MAX_VIDEO_PACKET,
            "{peer}: {mtu}"
        );
        let socket = UdpSocket::bind(bind)?;
        // A jumbo stream refuses fragmentation; a later stock stream must be able to clear
        // it again on the same process-wide socket.
        set_dont_fragment(&socket, true)?;
        if peer.is_ipv4() {
            assert_eq!(
                option(&socket, libc::IPPROTO_IP, libc::IP_MTU_DISCOVER),
                Some(libc::IP_PMTUDISC_DO)
            );
        }
        set_dont_fragment(&socket, false)?;
    }
    Ok(())
}

/// An integer socket option, None where the socket's family does not have it.
fn option(socket: &UdpSocket, level: i32, name: i32) -> Option<i32> {
    let mut value = 0i32;
    let mut length = size_of::<i32>() as libc::socklen_t;
    // SAFETY: `value` and `length` are live locals of the size passed; the socket is open.
    let read = unsafe {
        libc::getsockopt(
            socket.as_raw_fd(),
            level,
            name,
            (&raw mut value).cast(),
            &mut length,
        )
    };
    (read == 0).then_some(value)
}

#[test]
fn a_dual_stack_socket_refuses_fragmentation_for_both_families() -> Result<()> {
    let dual = socket2::Socket::new(
        socket2::Domain::IPV6,
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )?;
    dual.set_only_v6(false)?;
    dual.bind(&"[::]:0".parse::<SocketAddr>()?.into())?;
    let dual: UdpSocket = dual.into();
    set_dont_fragment(&dual, true)?;
    assert_eq!(
        option(&dual, libc::IPPROTO_IP, libc::IP_MTU_DISCOVER),
        Some(libc::IP_PMTUDISC_DO)
    );
    assert_eq!(
        option(&dual, libc::IPPROTO_IPV6, libc::IPV6_DONTFRAG),
        Some(1)
    );
    set_dont_fragment(&dual, false)?;
    assert_eq!(
        option(&dual, libc::IPPROTO_IP, libc::IP_MTU_DISCOVER),
        Some(libc::IP_PMTUDISC_WANT)
    );
    assert_eq!(
        option(&dual, libc::IPPROTO_IPV6, libc::IPV6_DONTFRAG),
        Some(0)
    );
    Ok(())
}

#[test]
fn the_default_gateways_are_the_next_hops_of_the_default_routes() -> Result<()> {
    // A PC with a default route has its next hop here, and none is a placeholder.
    let gateways = default_gateways()?;
    assert!(
        gateways
            .iter()
            .all(|gateway| !gateway.is_unspecified() && !gateway.is_loopback()),
        "{gateways:?}"
    );
    Ok(())
}

#[test]
fn the_kernels_route_tables_parse_into_routes_and_gateways() {
    // 0100A8C0 is 192.168.0.1 as the bytes lie in memory on a little-endian PC.
    let table = "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\tMTU\tWindow\tIRTT\n\
        eth0\t00000000\t0100A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0\n\
        eth0\t0000A8C0\t00000000\t0001\t0\t0\t100\t00FFFFFF\t0\t0\t0\n\
        wg0\t00000000\t00000000\t0001\t0\t0\t50\t00000000\t0\t0\t0\n\
        eth1\t00000000\t0100A8C0\t0002\t0\t0\t10\t00000000\t0\t0\t0\n";
    let routes = parse_routes4(table);
    // A route that is not up is not one; a default route without a router has no gateway.
    assert_eq!(routes.len(), 3);
    assert_eq!((routes[0].default, routes[0].metric), (true, 100));
    assert_eq!(
        routes[0].gateway.map(|gateway| gateway.octets()),
        Some(u32::from_str_radix("0100A8C0", 16).map_or([0; 4], |v| v.to_ne_bytes()))
    );
    assert!(!routes[1].default && routes[1].gateway.is_none());
    assert!(routes[2].default && routes[2].gateway.is_none());
    let (metric, gateways) = routed(&routes, "eth0");
    assert_eq!((metric, gateways.len()), (100, 1));
    assert_eq!(routed(&routes, "wg0"), (50, Vec::new()));
    assert_eq!(routed(&routes, "lo"), (0, Vec::new()));
    let v6 = "00000000000000000000000000000000 00 00000000000000000000000000000000 00 fe800000000000000000000000000001 00000064 00000001 00000000 00000003 eth0\n\
        fe800000000000000000000000000000 40 00000000000000000000000000000000 00 00000000000000000000000000000000 00000100 00000001 00000000 00000001 eth0\n";
    assert_eq!(
        parse_gateways6(v6),
        ["fe80::1"
            .parse::<IpAddr>()
            .map_or(Vec::new(), |gateway| vec![gateway])]
        .concat()
    );
}

fn card(index: u64, ethernet: bool, hardware: bool) -> Interface {
    Interface {
        index,
        up: true,
        hardware,
        ethernet,
        wireless: false,
        bps: 1_000_000_000,
    }
}

#[test]
fn interface_detection_keeps_physical_link_limits_separate_from_pacing_demand() {
    let rate = |link: Link| {
        butterpollo_media_policy::network_pacing::rate_bps(
            0,
            50_000,
            link.bps,
            link.wireless,
            65_000_000,
        )
    };
    // Wi-Fi has no known physical speed to cap by, so it paces from demand alone.
    let wifi = interface_link(&Interface {
        wireless: true,
        ..card(3, true, true)
    });
    assert_eq!((wifi.wireless, wifi.bps, wifi.interface), (true, 0, 3));
    assert_eq!(rate(wifi), 100_000_000);
    // A bridge, a VPN or a tunnel carries no physical speed.
    for (ethernet, hardware) in [(true, false), (false, false), (false, true)] {
        let link = interface_link(&card(4, ethernet, hardware));
        assert_eq!((link.bps, link.wireless), (0, false));
        assert_eq!(rate(link), 800_000_000);
    }
    let wired = Interface {
        bps: 100_000_000,
        ..card(2, true, true)
    };
    assert_eq!(
        interface_link(&wired),
        Link {
            bps: 100_000_000,
            wireless: false,
            interface: 2
        }
    );
    // An interface that is down is no link at all.
    let down = Interface { up: false, ..wired };
    assert_eq!(interface_link(&down), Link::default());
}

#[test]
fn a_virtual_interface_uses_only_an_unambiguous_physical_binding() {
    let lowers = |name: &str| -> Vec<String> {
        match name {
            "br0" => vec!["veth0".into()],
            "veth0" => vec!["eth0".into()],
            "team0" => vec!["eth0".into(), "eth1".into()],
            "loop0" => vec!["loop1".into()],
            "loop1" => vec!["loop0".into()],
            _ => Vec::new(),
        }
    };
    let entry = |name: &str| match name {
        "eth0" | "eth1" => Some(card(9, true, true)),
        "veth0" => Some(card(8, true, false)),
        "down0" => Some(Interface {
            up: false,
            ..card(7, true, true)
        }),
        _ => None,
    };
    let physical = interface_link(&card(9, true, true));
    assert_eq!(underlying_link("br0", lowers, entry), physical);
    // No binding, an unknown lower interface, two cards, or a cycle: no answer.
    assert_eq!(underlying_link("eth0", lowers, entry), Link::default());
    assert_eq!(
        underlying_link("br1", |_| vec!["gone".into()], entry),
        Link::default()
    );
    assert_eq!(underlying_link("team0", lowers, entry), Link::default());
    assert_eq!(underlying_link("loop0", lowers, entry), Link::default());
    assert_eq!(
        underlying_link("br2", |_| vec!["down0".into()], entry),
        Link::default()
    );
}

#[test]
fn an_uplink_is_a_physical_or_stacked_interface_with_a_default_gateway() {
    let interface = |hardware, hyper_v, gateways: &[Ipv4Addr]| LanInterface {
        address: Ipv4Addr::new(192, 0, 2, 10),
        hardware,
        hyper_v,
        metric: 25,
        gateways: gateways.to_vec(),
    };
    let router = [Ipv4Addr::new(192, 0, 2, 1)];
    assert!(interface(true, false, &[]).uplink());
    // A bridge over a card that carries the default route, and one that does not.
    assert!(interface(false, true, &router).uplink());
    assert!(!interface(false, true, &[]).uplink());
    // A TAP or ZeroTier interface, with or without a default gateway.
    assert!(!interface(false, false, &router).uplink());
}

/// Every address this PC lists is read with its metric and gateways; one with a default
/// route has its gateway among them.
#[test]
fn lan_interfaces_read_this_pcs_interfaces() -> Result<()> {
    let interfaces = lan_interfaces()?;
    let gateways = default_gateways()?;
    for interface in &interfaces {
        assert!(!interface.address.is_unspecified() && !interface.address.is_loopback());
        for gateway in &interface.gateways {
            assert!(gateways.contains(&IpAddr::V4(*gateway)), "{interface:?}");
        }
    }
    assert_eq!(
        lan_addresses()?,
        interfaces
            .iter()
            .map(|interface| interface.address.to_string())
            .collect::<Vec<_>>()
    );
    Ok(())
}

#[test]
fn loopback_has_no_mac_to_wake() -> Result<()> {
    for address in ["127.0.0.1", "::1", "::ffff:127.0.0.1"] {
        assert_eq!(
            local_mac(address.parse()?)?,
            "00:00:00:00:00:00",
            "{address}"
        );
    }
    Ok(())
}

#[test]
fn interface_change_notifications_arrive_and_end_with_the_registration() -> Result<()> {
    let arrived = Arc::new(AtomicUsize::new(0));
    let seen = arrived.clone();
    // The initial notification reports every interface this PC has, so at least one arrives
    // without an interface actually changing.
    let changes = InterfaceChanges::register_initial(
        move || {
            seen.fetch_add(1, Ordering::Relaxed);
        },
        || {},
    )
    .ok_or_else(|| anyhow::anyhow!("Linux refused the routing socket"))?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while arrived.load(Ordering::Relaxed) == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        arrived.load(Ordering::Relaxed) >= 1,
        "no interface reported"
    );
    // Dropping the registration ends its worker (the drop joins it) rather than waiting out
    // the two-second window the change opened.
    let dropped = Instant::now();
    drop(changes);
    assert!(dropped.elapsed() < Duration::from_secs(1));
    Ok(())
}

#[test]
fn a_registration_without_a_change_stays_quiet_and_ends_at_once() -> Result<()> {
    let calls = Arc::new(AtomicUsize::new(0));
    let (changed, quiet) = (calls.clone(), calls.clone());
    let changes = InterfaceChanges::register(
        move || {
            changed.fetch_add(1, Ordering::Relaxed);
        },
        move || {
            quiet.fetch_add(1, Ordering::Relaxed);
        },
    )
    .ok_or_else(|| anyhow::anyhow!("Linux refused the routing socket"))?;
    drop(changes);
    // Whatever another process changed meanwhile, `changed` and `quiet` come as a pair at
    // most.
    assert!(calls.load(Ordering::Relaxed) <= 2);
    Ok(())
}
