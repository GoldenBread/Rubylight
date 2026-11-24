//! Unit tests for port mappings, guest lease persistence/expiry, and router cleanup confirmation.

use super::super::*;

#[test]
fn lan_defaults_and_explicit_binding_keep_legacy_semantics() {
    assert_eq!(
        bind_address(&Config::default(), None).unwrap(),
        "0.0.0.0".parse::<IpAddr>().unwrap()
    );
    let config = Config::parse("address_family=both\nbind_address=192.0.2.10\n").unwrap();
    assert_eq!(
        bind_address(&config, None).unwrap(),
        "192.0.2.10".parse::<IpAddr>().unwrap()
    );
    assert_eq!(
        bind_address(&config, Some("127.0.0.1".parse().unwrap())).unwrap(),
        "127.0.0.1".parse::<IpAddr>().unwrap()
    );
    // An interface name instead of an address: this PC only, not down.
    let named = Config::parse("bind_address=Ethernet\n").unwrap();
    assert_eq!(
        bind_address(&named, None).unwrap(),
        "127.0.0.1".parse::<IpAddr>().unwrap()
    );
    // TCP 48118, 48123, 48144; UDP 48132, 48133, 48134 and the microphone's 48135.
    assert_eq!(
        mappings(&Config::default(), Ports::from_base(48123)).len(),
        7
    );
    let no_mic = Config::parse("stream_mic=false\n").unwrap();
    assert_eq!(mappings(&no_mic, Ports::from_base(48123)).len(), 6);
}
#[test]
fn browser_guest_ports_are_mapped_only_with_the_webrtc_server_on() {
    let ports = Ports::from_base(48123);
    let off = Config::parse("webrtc_media_port=9001\n").unwrap();
    assert!(guest_mappings(&off, ports).is_empty());
    let on = Config::parse("webrtc_server=true\nwebrtc_media_port=9001\n").unwrap();
    let mut expected = vec![Mapping {
        protocol: Protocol::TCP,
        port: 48125,
    }];
    expected.extend((9002..=9005).map(|port| Mapping {
        protocol: Protocol::UDP,
        port,
    }));
    assert_eq!(guest_mappings(&on, ports), expected);
    // The streaming ports never include them: they follow the guests' links.
    assert!(
        !mappings(&on, ports)
            .iter()
            .any(|m| m.port == 48125 || (9000..9010).contains(&m.port))
    );
}
#[test]
fn a_moved_media_port_never_forwards_the_tv_protocol_or_the_console() {
    let ports = Ports::from_base(47989);
    let near_discovery = Config::parse("webrtc_server=true\nwebrtc_media_port=7999\n").unwrap();
    let forwarded: Vec<u16> = guest_mappings(&near_discovery, ports)
        .iter()
        .map(|m| m.port)
        .collect();
    assert_eq!(forwarded, [47991, 8002, 8003]);
    let near_console = Config::parse("webrtc_server=true\nwebrtc_media_port=47988\n").unwrap();
    let forwarded: Vec<u16> = guest_mappings(&near_console, ports)
        .iter()
        .map(|m| m.port)
        .collect();
    assert!(!forwarded.contains(&47990), "{forwarded:?}");
}
#[test]
fn guest_ports_are_wanted_only_while_a_link_needs_them() {
    let ports = Ports::from_base(48123);
    // The console reachable from the internet, so its port is among the streaming ports.
    let config =
        Config::parse("webrtc_server=true\nwebrtc_media_port=9001\norigin_web_ui_allowed=wan\n")
            .unwrap();
    let guest = guest_mappings(&config, ports);
    // Nothing at all with `upnp` off and no internet link, even with a link open.
    assert_eq!(
        wanted_now(&config, ports, false, true, false),
        (vec![], vec![])
    );
    // A link made for the internet maps only the guest ports, whatever `upnp` says.
    let (stream, wanted) = wanted_now(&config, ports, false, true, true);
    assert!(stream.is_empty());
    assert_eq!(wanted, guest);
    // With `upnp` on, the guest ports only while a link works or a guest watches.
    let (stream, wanted) = wanted_now(&config, ports, true, false, false);
    assert_eq!(stream, mappings(&config, ports));
    assert!(wanted.is_empty());
    assert_eq!(wanted_now(&config, ports, true, true, false).1, guest);
    // Never the console's port, the WebRTC signalling port or the owner's own media ports
    // (UDP 8000 and 8001 by default, 9001 here) for a guest link.
    for config in [
        config.clone(),
        Config::parse("webrtc_server=true\n").unwrap(),
    ] {
        let (_, wanted) = wanted_now(&config, ports, false, true, true);
        assert!(!wanted.is_empty());
        assert!(
            !wanted.iter().any(|m| m.port == ports.web
                || m.port == config.webrtc_port()
                || m.port == config.webrtc_media_port()
                || m.port == 8000
                || m.port == 8001),
            "{wanted:?}"
        );
    }
}
#[test]
fn a_mapping_left_by_another_host_on_this_pc_is_not_a_conflict() {
    let local: IpAddr = "192.168.0.20".parse().unwrap();
    let entry = |remote: &str, client: &str, internal_port, enabled| igd_next::PortMappingEntry {
        remote_host: remote.into(),
        external_port: 47989,
        protocol: Protocol::TCP,
        internal_port,
        internal_client: client.into(),
        enabled,
        port_mapping_description: "Sunshine".into(),
        lease_duration: 0,
    };
    // A mapping from any address, enabled, forwards the port to this PC under its own number.
    for remote in ["", "0.0.0.0", "*"] {
        assert!(forwards_here(
            &entry(remote, "192.168.0.20", 47989, true),
            local
        ));
    }
    // Another address, another number, a disabled mapping, or one only for one remote host:
    // guests elsewhere reach nothing through it, so the port is not mapped.
    assert!(!forwards_here(
        &entry("", "192.168.0.30", 47989, true),
        local
    ));
    assert!(!forwards_here(
        &entry("", "192.168.0.20", 47990, true),
        local
    ));
    assert!(!forwards_here(
        &entry("", "192.168.0.20", 47989, false),
        local
    ));
    assert!(!forwards_here(
        &entry("203.0.113.7", "192.168.0.20", 47989, true),
        local
    ));
    assert!(!owned(
        &entry("", "192.168.0.20", 47989, true),
        local,
        "Butterpollo Rust id"
    ));
}
#[tokio::test]
#[ignore = "listens on every interface; Windows Firewall asks again for every new test binary"]
async fn dual_stack_listener_accepts_ipv4_and_ipv6_loopback() {
    let listener = tcp("[::]:0".parse().unwrap()).unwrap();
    let port = listener.local_addr().unwrap().port();
    for address in [format!("127.0.0.1:{port}"), format!("[::1]:{port}")] {
        let client = tokio::net::TcpStream::connect(address).await.unwrap();
        let (_, peer) = listener.accept().await.unwrap();
        assert!(peer.ip().to_canonical().is_loopback());
        drop(client);
    }
}

#[test]
fn the_guest_lease_record_keeps_added_leases_until_a_confirmed_delete() {
    let (record, directory) = super::super::fixtures::guest_record();
    let mut record = record.into_inner().unwrap();
    assert!(record.leases().is_empty());
    // No record on disk: the next start deletes nothing.
    assert!(!directory.join(GUEST_LEASES).exists());

    let tcp = Mapping {
        protocol: Protocol::TCP,
        port: 47991,
    };
    let udp = Mapping {
        protocol: Protocol::UDP,
        port: 8002,
    };
    let upnp = GuestLease::new(LeaseMethod::Upnp, tcp);
    let pcp6 = GuestLease::new(LeaseMethod::PcpV6, udp);
    // Added before its map request: the file lists it at once.
    record.add_all([upnp, pcp6]);
    assert_eq!(LeaseRecord::load(&directory).leases(), [upnp, pcp6]);

    // A lease added twice is one lease.
    record.add_all([upnp]);
    assert_eq!(record.leases(), [upnp, pcp6]);

    // A delete for another method, or one the router did not answer for, drops nothing.
    record.remove_confirmed(|method| method == LeaseMethod::Upnp, &[udp]);
    assert_eq!(record.leases(), [upnp, pcp6]);
    record.remove_confirmed(|method| method == LeaseMethod::Upnp, &[]);
    assert_eq!(record.leases(), [upnp, pcp6]);

    // The confirmed delete of the UPnP lease drops only it.
    record.remove_confirmed(|method| method == LeaseMethod::Upnp, &[tcp]);
    assert_eq!(record.leases(), [pcp6]);

    // The last confirmed delete removes the file: nothing is left for a start to clean.
    record.remove_confirmed(|method| method == LeaseMethod::PcpV6, &[udp]);
    assert!(record.leases().is_empty());
    assert!(!directory.join(GUEST_LEASES).exists());

    // An unreadable record lists nothing rather than guessing at ports to delete.
    std::fs::write(directory.join(GUEST_LEASES), b"{ not json").unwrap();
    assert!(LeaseRecord::load(&directory).leases().is_empty());

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn an_expired_guest_lease_is_dropped_at_load() {
    let (record, directory) = super::super::fixtures::guest_record();
    let udp = Mapping {
        protocol: Protocol::UDP,
        port: 8002,
    };
    let tcp = Mapping {
        protocol: Protocol::TCP,
        port: 47991,
    };
    let mut expired = GuestLease::new(LeaseMethod::Pcp, udp);
    expired.expires = Some(now_seconds().saturating_sub(1));
    let upnp = GuestLease::new(LeaseMethod::Upnp, tcp);
    record.lock().unwrap().add_all([expired, upnp]);
    // A PCP lease whose lifetime ran out leaves the record at the next start, without asking
    // the router; a UPnP mapping has no lifetime and stays until its delete is confirmed.
    assert_eq!(LeaseRecord::load(&directory).leases(), [upnp]);
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn a_silent_router_does_not_extend_a_lease() {
    let (record, directory) = super::super::fixtures::guest_record();
    let mapping = Mapping {
        protocol: Protocol::UDP,
        port: 8002,
    };
    let lease = GuestLease::new(LeaseMethod::NatPmp, mapping);
    let expires = lease.expires;
    record.lock().unwrap().add_all([lease]);
    // Every round pre-adds the wanted ports before its request; a later expiry from that
    // pre-add must not replace the one the router granted, so a router that never answers
    // cannot keep the entry past the lifetime.
    let mut later = GuestLease::new(LeaseMethod::NatPmp, mapping);
    later.expires = Some(u64::MAX);
    record.lock().unwrap().add_all([later]);
    assert_eq!(record.lock().unwrap().leases()[0].expires, expires);
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn cleanup_takes_only_the_recorded_leases_from_each_method() {
    let tcp = Mapping {
        protocol: Protocol::TCP,
        port: 47991,
    };
    let udp = Mapping {
        protocol: Protocol::UDP,
        port: 8002,
    };
    let upnp = GuestLease::new(LeaseMethod::Upnp, tcp);
    let natpmp = GuestLease::new(LeaseMethod::NatPmp, udp);
    let leases = [upnp, natpmp];
    assert_eq!(
        recorded_leases(&leases, |method| method == LeaseMethod::Upnp),
        [upnp]
    );
    // A record with no PCP or NAT-PMP lease sends the router no delete for them.
    assert_eq!(
        recorded_leases(&leases, |method| matches!(
            method,
            LeaseMethod::Pcp | LeaseMethod::NatPmp
        )),
        [natpmp]
    );
    assert!(recorded_leases(&[], |_| true).is_empty());
    assert!(recorded_leases(&[upnp], |method| method == LeaseMethod::NatPmp).is_empty());
}

#[tokio::test]
async fn a_cleanup_the_router_does_not_answer_keeps_the_record() {
    let router = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let address = router.local_addr().unwrap();
    // The router receives the delete and never answers it.
    tokio::spawn(async move {
        let mut buffer = [0u8; 128];
        while router.recv_from(&mut buffer).await.is_ok() {}
    });
    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    socket.connect(address).await.unwrap();
    let mut mapper = portmap::PortMapper::new(socket, "127.0.0.1".parse().unwrap(), b"key");
    let (record, directory) = super::super::fixtures::guest_record();
    let lease = GuestLease::new(
        LeaseMethod::NatPmp,
        Mapping {
            protocol: Protocol::UDP,
            port: 8002,
        },
    );
    record.lock().unwrap().add_all([lease]);
    delete_recorded(&mut mapper, &record, &[lease]).await;
    // No answer: the mapping may still be on the router, so the lease stays listed.
    assert_eq!(record.lock().unwrap().leases(), [lease]);
    let _ = std::fs::remove_dir_all(directory);
}

#[tokio::test]
async fn a_cleanup_the_router_answers_drops_the_recorded_lease() {
    let router = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let address = router.local_addr().unwrap();
    // A NAT-PMP router: PCP gets version 0, mapping and delete requests get the internal port
    // asked back with no lifetime.
    tokio::spawn(async move {
        let mut buffer = [0u8; 128];
        loop {
            let Ok((length, from)) = router.recv_from(&mut buffer).await else {
                return;
            };
            let request = buffer[..length].to_vec();
            let reply = match request[0] {
                2 => vec![0, 0x81, 0, 1, 0, 0, 0, 0],
                _ => {
                    let mut reply = vec![0, 0x80 | request[1], 0, 0, 0, 0, 0, 1];
                    reply.extend_from_slice(&request[4..6]);
                    reply.extend_from_slice(&[0, 0]);
                    reply.extend_from_slice(&0u32.to_be_bytes());
                    reply
                }
            };
            router.send_to(&reply, from).await.unwrap();
        }
    });
    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    socket.connect(address).await.unwrap();
    let mut mapper = portmap::PortMapper::new(socket, "127.0.0.1".parse().unwrap(), b"key");
    let (record, directory) = super::super::fixtures::guest_record();
    let lease = GuestLease::new(
        LeaseMethod::NatPmp,
        Mapping {
            protocol: Protocol::UDP,
            port: 8002,
        },
    );
    record.lock().unwrap().add_all([lease]);
    delete_recorded(&mut mapper, &record, &[lease]).await;
    assert!(record.lock().unwrap().leases().is_empty());
    let _ = std::fs::remove_dir_all(directory);
}

#[tokio::test]
async fn a_release_keeps_leases_the_router_did_not_answer_for() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let router = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let address = router.local_addr().unwrap();
    let answers = std::sync::Arc::new(AtomicBool::new(true));
    let replying = answers.clone();
    tokio::spawn(async move {
        let mut buffer = [0u8; 128];
        loop {
            let Ok((length, from)) = router.recv_from(&mut buffer).await else {
                return;
            };
            if !replying.load(Ordering::Acquire) {
                continue;
            }
            let request = buffer[..length].to_vec();
            let reply = match request[0] {
                2 => vec![0, 0x81, 0, 1, 0, 0, 0, 0],
                // The router's own address, for a NAT-PMP address request.
                _ if request[1] == 0 => vec![0, 128, 0, 0, 0, 0, 0, 1, 203, 0, 113, 9],
                _ => {
                    let mut reply = vec![0, 0x80 | request[1], 0, 0, 0, 0, 0, 1];
                    reply.extend_from_slice(&request[4..6]);
                    reply.extend_from_slice(&request[4..6]);
                    reply.extend_from_slice(&3600u32.to_be_bytes());
                    reply
                }
            };
            router.send_to(&reply, from).await.unwrap();
        }
    });
    let mapping = Mapping {
        protocol: Protocol::UDP,
        port: 8002,
    };
    let lease = GuestLease::new(LeaseMethod::NatPmp, mapping);
    let (record, directory) = super::super::fixtures::guest_record();
    record.lock().unwrap().add_all([lease]);
    let mapper = {
        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        socket.connect(address).await.unwrap();
        let mut mapper = portmap::PortMapper::new(socket, "127.0.0.1".parse().unwrap(), b"key");
        assert_eq!(mapper.renew(&[mapping]).await.mapped.len(), 1);
        mapper
    };
    // The router stops answering: the release cannot confirm the delete, so the lease stays.
    answers.store(false, Ordering::Release);
    let mut held = Held {
        mapper: Some(mapper),
        ..Default::default()
    };
    held.release("fixture", &record).await;
    assert_eq!(record.lock().unwrap().leases(), [lease]);
    let _ = std::fs::remove_dir_all(directory);
}

#[tokio::test]
async fn a_release_the_router_answers_drops_the_recorded_lease() {
    let router = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let address = router.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buffer = [0u8; 128];
        loop {
            let Ok((length, from)) = router.recv_from(&mut buffer).await else {
                return;
            };
            let request = buffer[..length].to_vec();
            let reply = match request[0] {
                2 => vec![0, 0x81, 0, 1, 0, 0, 0, 0],
                // The router's own address, for a NAT-PMP address request.
                _ if request[1] == 0 => vec![0, 128, 0, 0, 0, 0, 0, 1, 203, 0, 113, 9],
                _ => {
                    let mut reply = vec![0, 0x80 | request[1], 0, 0, 0, 0, 0, 1];
                    reply.extend_from_slice(&request[4..6]);
                    reply.extend_from_slice(&request[4..6]);
                    reply.extend_from_slice(&3600u32.to_be_bytes());
                    reply
                }
            };
            router.send_to(&reply, from).await.unwrap();
        }
    });
    let mapping = Mapping {
        protocol: Protocol::UDP,
        port: 8002,
    };
    let lease = GuestLease::new(LeaseMethod::NatPmp, mapping);
    let (record, directory) = super::super::fixtures::guest_record();
    record.lock().unwrap().add_all([lease]);
    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    socket.connect(address).await.unwrap();
    let mut mapper = portmap::PortMapper::new(socket, "127.0.0.1".parse().unwrap(), b"key");
    assert_eq!(mapper.renew(&[mapping]).await.mapped.len(), 1);
    let mut held = Held {
        mapper: Some(mapper),
        ..Default::default()
    };
    held.release("fixture", &record).await;
    assert!(record.lock().unwrap().leases().is_empty());
    let _ = std::fs::remove_dir_all(directory);
}
