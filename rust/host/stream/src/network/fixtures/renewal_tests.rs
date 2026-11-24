//! Loopback SOAP fixture tests for UPnP renewal silence, address changes, VPN tolerance, and trial rounds.

use super::super::*;
use super::*;
use std::sync::{Arc, Mutex};

/// A router that misses one renewal (busy, a lost packet) still holds the mappings: the host
/// keeps it and what it mapped, and looks for no other. Its next answer starts the count again.
#[tokio::test]
async fn a_upnp_router_that_misses_one_renewal_is_kept_with_its_mappings() {
    on_lan(&["192.0.2.10"]);
    let state = Arc::new(Mutex::new(RouterState::default()));
    let (record, directory) = guest_record();
    let (mut held, gateway, server) = mapped_router(&state, &record).await;
    for _ in 0..2 {
        state.lock().unwrap().silent = true;
        renew_round(&mut held, &record).await;
        assert_eq!(
            kept(&held),
            mapped_on(&gateway, "192.0.2.10"),
            "one silent round dropped the router"
        );
        a_renewal_later(&mut held);
        state.lock().unwrap().silent = false;
        renew_round(&mut held, &record).await;
        assert_eq!(kept(&held), mapped_on(&gateway, "192.0.2.10"));
    }
    assert_eq!(
        searches(),
        Vec::<IpAddr>::new(),
        "a single silent round started a search"
    );
    assert_eq!(forwards(&state), forwarded_to("192.0.2.10"));
    let _ = std::fs::remove_dir_all(directory);
    server.abort();
}
/// A router that stays silent past its mappings' lease is looked for again, and the router the
/// search finds maps the ports afresh for this PC's address.
#[tokio::test]
async fn a_upnp_router_that_stays_silent_is_looked_for_again() {
    on_lan(&["192.0.2.10"]);
    let state = Arc::new(Mutex::new(RouterState::default()));
    let (record, directory) = guest_record();
    let (mut held, gateway, server) = mapped_router(&state, &record).await;
    let other = Arc::new(Mutex::new(RouterState::default()));
    let (next, next_server) = serve(other.clone()).await;
    router_on("192.0.2.10", copy(&next));
    state.lock().unwrap().silent = true;
    renew_round(&mut held, &record).await;
    assert_eq!(searches(), Vec::<IpAddr>::new());
    assert_eq!(kept(&held), mapped_on(&gateway, "192.0.2.10"));
    a_renewal_later(&mut held);
    renew_round(&mut held, &record).await;
    assert_eq!(
        searches(),
        ["192.0.2.10".parse::<IpAddr>().unwrap()],
        "a router silent for two renewals is still waited on"
    );
    assert_eq!(kept(&held), mapped_on(&next, "192.0.2.10"));
    assert_eq!(forwards(&other), forwarded_to("192.0.2.10"));
    let _ = std::fs::remove_dir_all(directory);
    server.abort();
    next_server.abort();
}
/// This PC gets another address from the same router: the router is looked for again, the
/// mapping made for the old address goes, and the port is mapped for the new one.
#[tokio::test]
async fn a_new_address_on_the_same_router_is_mapped_afresh() {
    on_lan(&["192.0.2.10"]);
    let state = Arc::new(Mutex::new(RouterState::default()));
    let (record, directory) = guest_record();
    let (mut held, gateway, server) = mapped_router(&state, &record).await;
    on_lan(&["192.0.2.23"]);
    router_on("192.0.2.23", copy(&gateway));
    renew_round(&mut held, &record).await;
    assert_eq!(
        forwards(&state),
        forwarded_to("192.0.2.23"),
        "the port is not forwarded to this PC's new address"
    );
    assert_eq!(searches(), ["192.0.2.23".parse::<IpAddr>().unwrap()]);
    assert_eq!(kept(&held), mapped_on(&gateway, "192.0.2.23"));
    let _ = std::fs::remove_dir_all(directory);
    server.abort();
}
/// This PC moves to another router on another subnet: the new router maps the ports for the
/// address this PC has there, and the old router is not asked again.
#[tokio::test]
async fn another_router_on_another_subnet_maps_for_this_pcs_address_there() {
    on_lan(&["192.0.2.10"]);
    let state = Arc::new(Mutex::new(RouterState::default()));
    let (record, directory) = guest_record();
    let (mut held, _, server) = mapped_router(&state, &record).await;
    let asked = state.lock().unwrap().requests;
    on_lan(&["198.51.100.7"]);
    let other = Arc::new(Mutex::new(RouterState::default()));
    let (next, next_server) = serve(other.clone()).await;
    router_on("198.51.100.7", copy(&next));
    renew_round(&mut held, &record).await;
    assert_eq!(
        forwards(&other),
        forwarded_to("198.51.100.7"),
        "the new router got no mapping for this PC's address there"
    );
    assert_eq!(kept(&held), mapped_on(&next, "198.51.100.7"));
    assert_eq!(
        state.lock().unwrap().requests,
        asked,
        "the old router was asked"
    );
    let _ = std::fs::remove_dir_all(directory);
    server.abort();
    next_server.abort();
}
/// A VPN connects: another address and default gateway appear. The LAN router still answers, so
/// it keeps its mappings and no search starts.
#[tokio::test]
async fn a_vpn_connecting_leaves_an_answering_router_alone() {
    on_lan(&["192.0.2.10"]);
    let state = Arc::new(Mutex::new(RouterState::default()));
    let (record, directory) = guest_record();
    let (mut held, gateway, server) = mapped_router(&state, &record).await;
    vpn_up();
    for _ in 0..3 {
        renew_round(&mut held, &record).await;
        assert_eq!(kept(&held), mapped_on(&gateway, "192.0.2.10"));
    }
    assert_eq!(searches(), Vec::<IpAddr>::new());
    assert_eq!(forwards(&state), forwarded_to("192.0.2.10"));
    let _ = std::fs::remove_dir_all(directory);
    server.abort();
}
/// A host that listens on every address looks for the UPnP router from its LAN address: an
/// unbound search goes where Windows sends multicast, which with a VPN up can be the VPN.
#[tokio::test]
async fn the_upnp_search_goes_out_on_the_lan_not_a_vpn() {
    on_lan(&["192.0.2.10"]);
    vpn_up();
    let (vpn_gateway, vpn_server) = serve(Arc::default()).await;
    let (lan_gateway, lan_server) = serve(Arc::default()).await;
    router_on("0.0.0.0", copy(&vpn_gateway));
    router_on("10.8.0.2", copy(&vpn_gateway));
    router_on("192.0.2.10", copy(&lan_gateway));
    let (found, local) = upnp_gateway(IpAddr::from([0, 0, 0, 0])).await.unwrap();
    assert_eq!(
        (found.addr, local),
        (lan_gateway.addr, "192.0.2.10".parse().unwrap()),
        "the search took the VPN's router"
    );
    vpn_server.abort();
    lan_server.abort();
}
/// The LAN router goes silent while a VPN is up: the search that follows still goes out on the
/// LAN and maps the ports on the router found there, not on the VPN's.
#[tokio::test]
async fn a_search_after_the_lan_router_went_silent_does_not_choose_the_vpn() {
    on_lan(&["192.0.2.10"]);
    let state = Arc::new(Mutex::new(RouterState::default()));
    let (record, directory) = guest_record();
    let (mut held, _, server) = mapped_router(&state, &record).await;
    vpn_up();
    let vpn = Arc::new(Mutex::new(RouterState::default()));
    let (vpn_gateway, vpn_server) = serve(vpn.clone()).await;
    let lan = Arc::new(Mutex::new(RouterState::default()));
    let (lan_gateway, lan_server) = serve(lan.clone()).await;
    router_on("0.0.0.0", copy(&vpn_gateway));
    router_on("10.8.0.2", copy(&vpn_gateway));
    router_on("192.0.2.10", copy(&lan_gateway));
    state.lock().unwrap().silent = true;
    for _ in 0..2 {
        renew_round(&mut held, &record).await;
        a_renewal_later(&mut held);
    }
    assert_eq!(
        kept(&held),
        mapped_on(&lan_gateway, "192.0.2.10"),
        "the router found after the silence is not the LAN's"
    );
    assert_eq!(forwards(&lan), forwarded_to("192.0.2.10"));
    assert!(vpn.lock().unwrap().rows.is_empty());
    let _ = std::fs::remove_dir_all(directory);
    server.abort();
    vpn_server.abort();
    lan_server.abort();
}

/// With a VPN up, PCP and NAT-PMP ask the LAN router, not the VPN's gateway that Windows may list
/// first among the default routes, also from an adapter that calls itself Ethernet.
#[test]
fn pcp_asks_the_lan_router_not_a_vpn_gateway() {
    let lan = adapter([192, 0, 2, 10].into(), true, false, 25, &["192.0.2.1"]);
    let tap = adapter([10, 8, 0, 2].into(), false, false, 1, &["10.8.0.1"]);
    let gateways = ["10.8.0.1".parse().unwrap(), "192.0.2.1".parse().unwrap()];
    let any = IpAddr::from([0, 0, 0, 0]);
    let lan_route = Some(([192, 0, 2, 1].into(), [192, 0, 2, 10].into()));
    assert_eq!(
        pcp_route(&[tap.clone(), lan.clone()], &gateways, any),
        lan_route
    );
    assert_eq!(pcp_route(&[lan, tap], &gateways, any), lan_route);
}

/// Two silent rounds seconds apart, as when a guest link asks for one in between, are not the
/// lease's two minutes: the router and its mappings stay.
#[tokio::test]
async fn silent_rounds_closer_than_a_renewal_keep_the_router() {
    on_lan(&["192.0.2.10"]);
    let state = Arc::new(Mutex::new(RouterState::default()));
    let (record, directory) = guest_record();
    let (mut held, gateway, server) = mapped_router(&state, &record).await;
    state.lock().unwrap().silent = true;
    for _ in 0..3 {
        renew_round(&mut held, &record).await;
    }
    assert_eq!(
        kept(&held),
        mapped_on(&gateway, "192.0.2.10"),
        "rounds seconds apart gave the router up"
    );
    assert_eq!(searches(), Vec::<IpAddr>::new());
    let _ = std::fs::remove_dir_all(directory);
    server.abort();
}
/// A router given up for silence that still answers the search is not taken back round after
/// round: each round tries it once (its address, its table, the ports), PCP and NAT-PMP get the
/// rounds it answers none of, and once it answers it maps the ports again.
#[tokio::test]
async fn a_router_that_answers_only_the_search_is_tried_once_a_round() {
    on_lan(&["192.0.2.10"]);
    let state = Arc::new(Mutex::new(RouterState::default()));
    let (record, directory) = guest_record();
    let (mut held, gateway, server) = mapped_router(&state, &record).await;
    state.lock().unwrap().silent = true;
    renew_round(&mut held, &record).await;
    a_renewal_later(&mut held);
    router_on("192.0.2.10", copy(&gateway));
    renew_round(&mut held, &record).await;
    assert_eq!(searches().len(), 1);
    assert!(kept(&held).is_none(), "the silent router was taken back");
    for _ in 0..2 {
        let asked = state.lock().unwrap().requests;
        router_on("192.0.2.10", copy(&gateway));
        renew_round(&mut held, &record).await;
        assert!(kept(&held).is_none());
        // Its address, the first entry of its table, and the one port.
        assert_eq!(state.lock().unwrap().requests, asked + 3);
    }
    state.lock().unwrap().silent = false;
    router_on("192.0.2.10", copy(&gateway));
    renew_round(&mut held, &record).await;
    assert_eq!(kept(&held), mapped_on(&gateway, "192.0.2.10"));
    assert_eq!(forwards(&state), forwarded_to("192.0.2.10"));
    let _ = std::fs::remove_dir_all(directory);
    server.abort();
}
/// After an address change on a router that hides its table, nothing is deleted on a guess: the
/// port may be another device's on this network.
#[tokio::test]
async fn an_old_address_on_a_hidden_table_deletes_nothing() {
    on_lan(&["192.0.2.10"]);
    let state = Arc::new(Mutex::new(RouterState::default()));
    let (record, directory) = guest_record();
    let (mut held, gateway, server) = mapped_router(&state, &record).await;
    state.lock().unwrap().list_fault = Some(606);
    on_lan(&["192.0.2.23"]);
    router_on("192.0.2.23", copy(&gateway));
    renew_round(&mut held, &record).await;
    assert_eq!(state.lock().unwrap().deleted, Vec::<u16>::new());
    assert_eq!(forwards(&state), forwarded_to("192.0.2.10"));
    assert_eq!(
        kept(&held).map(|(addr, local, _)| (addr, local)),
        Some((gateway.addr, "192.0.2.23".parse().unwrap()))
    );
    let _ = std::fs::remove_dir_all(directory);
    server.abort();
}
/// A router given up for silence, busy enough to miss the first request of a round but answering
/// the port requests, is taken back in that round.
#[tokio::test]
async fn a_router_given_up_that_answers_its_port_requests_is_taken_back() {
    on_lan(&["192.0.2.10"]);
    let state = Arc::new(Mutex::new(RouterState::default()));
    let (record, directory) = guest_record();
    let (mut held, gateway, server) = mapped_router(&state, &record).await;
    state.lock().unwrap().silent = true;
    renew_round(&mut held, &record).await;
    a_renewal_later(&mut held);
    router_on("192.0.2.10", copy(&gateway));
    renew_round(&mut held, &record).await;
    assert!(kept(&held).is_none());
    {
        let mut state = state.lock().unwrap();
        state.silent = false;
        state.ignore_external = true;
    }
    router_on("192.0.2.10", copy(&gateway));
    renew_round(&mut held, &record).await;
    assert_eq!(
        kept(&held),
        mapped_on(&gateway, "192.0.2.10"),
        "a router answering its port requests was not taken back"
    );
    assert_eq!(forwards(&state), forwarded_to("192.0.2.10"));
    let _ = std::fs::remove_dir_all(directory);
    server.abort();
}
/// A router given up for silence that holds its requests open gets one port request on trial,
/// not one per port: the round stops at its first timeout.
#[tokio::test]
async fn a_router_on_trial_stops_at_its_first_timeout() {
    on_lan(&["192.0.2.10"]);
    let state = Arc::new(Mutex::new(RouterState {
        silent: true,
        add_delay: Some(std::time::Duration::from_secs(30)),
        ..Default::default()
    }));
    let (gateway, server) = serve(state.clone()).await;
    let (record, directory) = guest_record();
    let mut held = Held {
        upnp_silent: Some((gateway.addr, gateway.control_url.clone())),
        ..Default::default()
    };
    router_on("192.0.2.10", copy(&gateway));
    let wanted = [
        STREAM_PORT,
        Mapping {
            protocol: Protocol::UDP,
            port: 47998,
        },
    ];
    renew(
        &mut held,
        IpAddr::from([0, 0, 0, 0]),
        &wanted,
        &[],
        &wanted,
        "Butterpollo Rust fixture-host",
        b"key",
        &mut Vec::new(),
        &record,
    )
    .await;
    assert!(kept(&held).is_none());
    assert_eq!(
        state.lock().unwrap().adds_started,
        1,
        "the trial went on after a timeout"
    );
    let _ = std::fs::remove_dir_all(directory);
    server.abort();
}
