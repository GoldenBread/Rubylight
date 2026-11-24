//! Unit tests for internet play NAT diagnostics and guest reachability.

use super::*;

#[test]
fn only_public_ipv4_counts_as_external() {
    assert!(public_ipv4("203.0.113.5".parse().unwrap()));
    assert!(!public_ipv4("192.168.1.1".parse().unwrap()));
    assert!(!public_ipv4("100.72.0.1".parse().unwrap()));
    assert!(!public_ipv4("2001:db8::1".parse().unwrap()));
}

#[test]
fn notes_name_carrier_grade_and_double_nat() {
    let cgnat = MappingStatus {
        external_ipv4: Some("100.72.0.1".parse().unwrap()),
        ..Default::default()
    };
    let found = notes(&cgnat, &[], Nat::Cone);
    assert_eq!(found.len(), 1);
    assert!(found[0].contains("carrier-grade NAT"));

    let double = MappingStatus {
        external_ipv4: Some("203.0.113.5".parse().unwrap()),
        ..Default::default()
    };
    let found = notes(
        &double,
        &["198.51.100.7:40000".parse().unwrap()],
        Nat::Symmetric,
    );
    assert_eq!(found.len(), 2);
    assert!(found[0].contains("another router or NAT"));
    assert!(found[1].contains("symmetric NAT"));

    assert!(notes(&double, &["203.0.113.5:40000".parse().unwrap()], Nat::Cone).is_empty());
}

#[test]
fn a_private_router_address_names_what_is_in_front_of_it() {
    let router_note = |address: &str| {
        let mapping = MappingStatus {
            external_ipv4: Some(address.parse().unwrap()),
            ..Default::default()
        };
        notes(&mapping, &[], Nat::Cone)
    };
    for address in ["192.168.1.1", "172.20.0.1"] {
        let found = router_note(address);
        assert_eq!(found.len(), 1, "{address}: {found:?}");
        assert!(
            found[0].contains("another router or modem is in front of this one"),
            "{address}: {found:?}"
        );
        assert!(
            found[0].contains("forward the ports there too, or put it in bridge mode"),
            "{address}: {found:?}"
        );
        assert!(
            !found[0].contains("carrier-grade NAT"),
            "{address}: {found:?}"
        );
    }
    // 10/8 is used at home and by providers behind another NAT: the note names either.
    let found = router_note("10.1.2.3");
    assert!(found[0].contains("carrier-grade NAT"), "{found:?}");
    assert!(found[0].contains("another router or modem"), "{found:?}");
    // 100.64/10 is the provider's shared address, never another router.
    let found = router_note("100.72.0.1");
    assert!(found[0].contains("carrier-grade NAT"), "{found:?}");
    assert!(!found[0].contains("another router"), "{found:?}");

    // The guest link's note says the same, and stays off the address.
    let behind = MappingStatus {
        runs: true,
        method: Some("UPnP".into()),
        external_ipv4: Some("192.168.1.1".parse().unwrap()),
        guest_mapped: guest_ports(|port| port),
        ..Default::default()
    };
    let reach = guest_reach_of(&behind, &host(0, None, &[]));
    assert_eq!(reach.address, None);
    assert!(
        reach.notes[0].contains("forward the ports there too, or put it in bridge mode"),
        "{:?}",
        reach.notes
    );
}

fn guest_ports(external: impl Fn(u16) -> u16) -> Vec<PortState> {
    [
        ("TCP", 47991),
        ("UDP", 8002),
        ("UDP", 8003),
        ("UDP", 8004),
        ("UDP", 8005),
    ]
    .into_iter()
    .map(|(protocol, port)| PortState {
        protocol: protocol.into(),
        port,
        external: Some(external(port)),
        reason: None,
    })
    .collect()
}

fn host(requested: u64, public: Option<&str>, stun: &[&str]) -> GuestHost {
    GuestHost {
        requested,
        public_ipv4: public.map(|ip| ip.parse().unwrap()),
        guest_port: 47991,
        stun: stun.iter().map(|ip| ip.parse().unwrap()).collect(),
        internet_play: !stun.is_empty(),
        ipv6_listening: false,
        guest_join: "off".to_owned(),
    }
}

#[test]
fn a_guest_link_takes_the_public_address_only_when_every_guest_port_is_mapped() {
    let public: IpAddr = "203.0.113.5".parse().unwrap();
    let mapped = MappingStatus {
        runs: true,
        method: Some("UPnP".into()),
        external_ipv4: Some(public),
        guest_mapped: guest_ports(|port| port),
        guest_round: 3,
        ..Default::default()
    };
    let reach = guest_reach_of(&mapped, &host(3, Some("203.0.113.5"), &[]));
    assert_eq!(reach.address.as_deref(), Some("203.0.113.5:47991"));
    assert!(!reach.checking);
    assert_eq!(reach.ports.len(), 5);
    assert!(reach.notes.is_empty(), "{:?}", reach.notes);
    // A paired friend joins as a guest unless When another device joins the running game
    // is off; another value reads as off.
    let mut letting = host(3, Some("203.0.113.5"), &[]);
    letting.guest_join = "allow".to_owned();
    assert_eq!(guest_reach_of(&mapped, &letting).guest_join, "allow");
    letting.guest_join = "sometimes".to_owned();
    assert_eq!(guest_reach_of(&mapped, &letting).guest_join, "off");
    // STUN agreeing with the router changes nothing.
    let agreed = guest_reach_of(
        &mapped,
        &host(3, Some("203.0.113.5"), &["203.0.113.5:40000"]),
    );
    assert_eq!(agreed.address, reach.address);
    assert!(agreed.internet_play);
    // Until the task has asked the router for the latest request, the dialog waits.
    assert!(guest_reach_of(&mapped, &host(4, Some("203.0.113.5"), &[])).checking);
    // A task that does not run never answers: no waiting for it.
    let idle = MappingStatus {
        runs: false,
        ..Default::default()
    };
    assert!(!guest_reach_of(&idle, &host(4, None, &[])).checking);

    // One media port refused: no link at the public address.
    let mut partly = mapped.clone();
    let refused = partly.guest_mapped.pop().unwrap();
    partly.guest_failed.push(PortState {
        external: None,
        reason: Some("the router is out of mappings".into()),
        ..refused
    });
    let reach = guest_reach_of(&partly, &host(3, Some("203.0.113.5"), &[]));
    assert_eq!(reach.address, None);
    assert_eq!(reach.ports.len(), 5);

    // Other port numbers can't be used, and say so.
    let renumbered = MappingStatus {
        guest_mapped: guest_ports(|port| port + 10_000),
        ..mapped.clone()
    };
    let reach = guest_reach_of(&renumbered, &host(3, Some("203.0.113.5"), &[]));
    assert_eq!(reach.address, None);
    assert!(reach.notes[0].contains("other port numbers"));

    // A router that does not say its address: no guess from STUN.
    let silent = MappingStatus {
        external_ipv4: None,
        ..mapped
    };
    let reach = guest_reach_of(&silent, &host(3, Some("198.51.100.7"), &["198.51.100.7:1"]));
    assert_eq!(reach.address, None);
    assert!(reach.notes[0].contains("did not say its internet address"));
}

#[test]
fn a_guest_link_behind_carrier_grade_nat_falls_back_to_ipv6_or_nothing() {
    let cgnat = MappingStatus {
        runs: true,
        method: Some("UPnP".into()),
        external_ipv4: Some("100.72.0.1".parse().unwrap()),
        guest_mapped: guest_ports(|port| port),
        ..Default::default()
    };
    // With Play over the internet on, STUN sees the provider's public address: the
    // forwarded ports on the home router are still out of reach.
    let reach = guest_reach_of(
        &cgnat,
        &host(0, Some("198.51.100.7"), &["198.51.100.7:40000"]),
    );
    assert_eq!(reach.address, None);
    assert!(
        reach.notes[0].contains("carrier-grade NAT"),
        "{:?}",
        reach.notes
    );
    assert_eq!(reach.public_ipv4, Some("198.51.100.7".parse().unwrap()));

    let with_ipv6 = MappingStatus {
        guest_ipv6: Some("2001:db8::7".parse().unwrap()),
        ..cgnat
    };
    let reach = guest_reach_of(&with_ipv6, &host(0, None, &[]));
    assert_eq!(reach.address.as_deref(), Some("[2001:db8::7]:47991"));

    // Another NAT in front of a router with a public address: nothing at IPv4.
    let double = MappingStatus {
        runs: true,
        external_ipv4: Some("203.0.113.5".parse().unwrap()),
        guest_mapped: guest_ports(|port| port),
        ..Default::default()
    };
    let reach = guest_reach_of(
        &double,
        &host(0, Some("203.0.113.5"), &["198.51.100.7:40000"]),
    );
    assert_eq!(reach.address, None);
    assert!(reach.notes[0].contains("another router or NAT"));
    assert!(reach.internet_play);
}

#[test]
fn device_ids_ignore_surrounding_whitespace() {
    assert_eq!(device_id("cert\n"), device_id("cert"));
    assert_ne!(device_id("a"), device_id("b"));
    assert_eq!(parse_secret(&Value::String("00".repeat(32))), Some([0; 32]));
    assert_eq!(parse_secret(&Value::String("00".repeat(31))), None);
}
