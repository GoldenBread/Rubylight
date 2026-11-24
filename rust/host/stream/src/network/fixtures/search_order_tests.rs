//! Loopback SOAP fixture tests for LAN/VPN/Hyper-V adapter search ordering and parallel gateway discovery.

use super::super::*;
use super::*;
use std::sync::Arc;

/// A VPN adapter that identifies as Ethernet (TAP, ZeroTier) is listed among the LAN addresses,
/// after the physical one: the search for the router does not go out on it while a physical
/// adapter has an address, even when the LAN finds no router.
#[tokio::test]
async fn the_search_skips_a_vpn_adapter_that_calls_itself_ethernet() {
    on_lan(&["192.0.2.10"]);
    TEST_NET.with(|net| {
        net.borrow_mut().lan.push(adapter(
            [10, 8, 0, 2].into(),
            false,
            false,
            1,
            &["10.8.0.1"],
        ));
    });
    let (vpn_gateway, vpn_server) = serve(Arc::default()).await;
    router_on("10.8.0.2", copy(&vpn_gateway));
    assert!(
        upnp_gateway(IpAddr::from([0, 0, 0, 0])).await.is_err(),
        "the search took the VPN adapter's router"
    );
    // Windows sends multicast from the LAN address: no search from no address in particular.
    assert_eq!(searches(), [IpAddr::from([192, 0, 2, 10])]);
    vpn_server.abort();
}
/// Without an uplink (a virtual machine's adapters), every Ethernet and Wi-Fi address is searched
/// from.
#[test]
fn without_an_uplink_every_lan_address_is_searched_from() {
    let virtual_only = [
        adapter([172, 20, 0, 5].into(), false, false, 25, &[]),
        adapter([10, 8, 0, 2].into(), false, false, 35, &[]),
    ];
    let order: Vec<_> = search_order(&virtual_only)
        .into_iter()
        .map(|interface| interface.address)
        .collect();
    assert_eq!(
        order,
        [
            std::net::Ipv4Addr::new(172, 20, 0, 5),
            std::net::Ipv4Addr::new(10, 8, 0, 2)
        ]
    );
}
/// Ethernet and Wi-Fi on the same router: the search goes out on the one Windows prefers, the
/// lower metric, whatever order the adapters are listed in, so the mappings forward to the
/// Ethernet address. When that one finds no router, the next one is searched from.
#[tokio::test]
async fn the_lower_metric_adapter_is_searched_from_first() {
    let (gateway, server) = serve(Arc::default()).await;
    for reversed in [false, true] {
        let mut adapters = ethernet_and_wifi().to_vec();
        if reversed {
            adapters.reverse();
        }
        with_adapters(adapters);
        router_on("192.0.2.10", copy(&gateway));
        router_on("192.0.2.20", copy(&gateway));
        let (_, local) = upnp_gateway(IpAddr::from([0, 0, 0, 0])).await.unwrap();
        assert_eq!(
            local,
            IpAddr::from([192, 0, 2, 10]),
            "mapped for the Wi-Fi address (listed {})",
            if reversed {
                "Wi-Fi first"
            } else {
                "Ethernet first"
            }
        );
    }
    with_adapters(ethernet_and_wifi().to_vec());
    router_on("192.0.2.20", copy(&gateway));
    let (_, local) = upnp_gateway(IpAddr::from([0, 0, 0, 0])).await.unwrap();
    assert_eq!(local, IpAddr::from([192, 0, 2, 20]));
    assert_eq!(
        searches(),
        [
            IpAddr::from([192, 0, 2, 10]),
            IpAddr::from([192, 0, 2, 20]),
            IpAddr::from([0, 0, 0, 0])
        ]
    );
    server.abort();
}
/// Ethernet on a Hyper-V external switch: this PC's address is on Hyper-V's virtual adapter, which
/// has the router as its default gateway. With Wi-Fi also connected, the virtual adapter's lower
/// metric wins, as it does for Windows' own traffic.
#[tokio::test]
async fn an_external_hyper_v_switch_counts_as_an_uplink() {
    with_adapters(vec![
        adapter([192, 0, 2, 20].into(), true, false, 35, &["192.0.2.1"]),
        adapter([192, 0, 2, 10].into(), false, true, 15, &["192.0.2.1"]),
    ]);
    let (gateway, server) = serve(Arc::default()).await;
    router_on("192.0.2.10", copy(&gateway));
    router_on("192.0.2.20", copy(&gateway));
    let (_, local) = upnp_gateway(IpAddr::from([0, 0, 0, 0])).await.unwrap();
    assert_eq!(
        local,
        IpAddr::from([192, 0, 2, 10]),
        "the external switch's address was passed over"
    );
    server.abort();
}
/// Hyper-V's Default Switch (and WSL's adapter) has no default gateway: this PC is the
/// gateway of the machines behind it. Its address is never searched from while an uplink exists,
/// though its metric is lower.
#[tokio::test]
async fn the_hyper_v_default_switch_is_never_searched_from() {
    with_adapters(vec![
        adapter([172, 20, 0, 1].into(), false, true, 5, &[]),
        adapter([192, 0, 2, 20].into(), true, false, 35, &["192.0.2.1"]),
    ]);
    let (gateway, server) = serve(Arc::default()).await;
    router_on("172.20.0.1", copy(&gateway));
    assert!(upnp_gateway(IpAddr::from([0, 0, 0, 0])).await.is_err());
    assert_eq!(
        searches(),
        [IpAddr::from([192, 0, 2, 20]), IpAddr::from([0, 0, 0, 0])]
    );
    server.abort();
}
/// A TAP adapter with a default gateway and the lowest metric, as a VPN sets it: still not
/// searched from, nor asked for PCP, while the LAN is there.
#[tokio::test]
async fn a_tap_adapter_with_a_gateway_is_not_chosen_over_the_lan() {
    let tap = adapter([10, 8, 0, 2].into(), false, false, 1, &["10.8.0.1"]);
    let lan = adapter([192, 0, 2, 10].into(), true, false, 25, &["192.0.2.1"]);
    with_adapters(vec![tap.clone(), lan.clone()]);
    let (gateway, server) = serve(Arc::default()).await;
    router_on("10.8.0.2", copy(&gateway));
    router_on("192.0.2.10", copy(&gateway));
    let (_, local) = upnp_gateway(IpAddr::from([0, 0, 0, 0])).await.unwrap();
    assert_eq!(local, IpAddr::from([192, 0, 2, 10]));
    assert!(!searches().contains(&IpAddr::from([10, 8, 0, 2])));
    let gateways = ["10.8.0.1".parse().unwrap(), "192.0.2.1".parse().unwrap()];
    assert_eq!(
        pcp_route(&[tap, lan], &gateways, IpAddr::from([0, 0, 0, 0])),
        Some(([192, 0, 2, 1].into(), [192, 0, 2, 10].into()))
    );
    server.abort();
}
/// A host bound to one address asks PCP and NAT-PMP at the default gateway of that address'
/// adapter, not at whichever default gateway is listed first.
#[test]
fn pcp_with_a_bound_address_asks_that_adapters_gateway() {
    let wifi = adapter([198, 51, 100, 7].into(), true, false, 35, &["198.51.100.1"]);
    let [ethernet, _] = ethernet_and_wifi();
    let gateways = [
        "192.0.2.1".parse().unwrap(),
        "198.51.100.1".parse().unwrap(),
    ];
    assert_eq!(
        pcp_route(
            &[ethernet.clone(), wifi.clone()],
            &gateways,
            IpAddr::from([198, 51, 100, 7])
        ),
        Some(([198, 51, 100, 1].into(), [198, 51, 100, 7].into())),
        "the bound address asks another adapter's router"
    );
    // Listening on every address: the lower metric adapter's router.
    assert_eq!(
        pcp_route(&[wifi, ethernet], &gateways, IpAddr::from([0, 0, 0, 0])),
        Some(([192, 0, 2, 1].into(), [192, 0, 2, 10].into()))
    );
}
/// Ethernet and Wi-Fi with the same metric (Windows gives gigabit Ethernet and fast Wi-Fi both 25)
/// on the same router: the address Windows itself routes to the router from wins, whatever order
/// the adapters are listed in, not the lower address.
#[tokio::test]
async fn equal_metrics_go_by_the_address_windows_routes_from() {
    let (gateway, server) = serve(Arc::default()).await;
    for reversed in [false, true] {
        let mut adapters = vec![
            adapter([192, 0, 2, 10].into(), true, false, 25, &["192.0.2.1"]),
            adapter([192, 0, 2, 20].into(), true, false, 25, &["192.0.2.1"]),
        ];
        if reversed {
            adapters.reverse();
        }
        with_adapters(adapters);
        route_to("192.0.2.1", "192.0.2.20");
        router_on("192.0.2.10", copy(&gateway));
        router_on("192.0.2.20", copy(&gateway));
        let (_, local) = upnp_gateway(IpAddr::from([0, 0, 0, 0])).await.unwrap();
        assert_eq!(
            local,
            IpAddr::from([192, 0, 2, 20]),
            "mapped for an address Windows does not route from"
        );
    }
    server.abort();
}
/// The router is reached through an adapter of no kind the list knows as an uplink (a network
/// bridge, a NIC team, a VLAN adapter) while a physical adapter has an address (a direct link to
/// a NAS): after the listed addresses find nothing, the search from no address in particular,
/// as before, finds it, for the address Windows routes to it from.
#[tokio::test]
async fn a_router_behind_an_unlisted_kind_of_adapter_is_found_by_the_unbound_search() {
    with_adapters(vec![
        adapter([198, 51, 100, 5].into(), true, false, 5, &[]),
        adapter([192, 0, 2, 10].into(), false, false, 25, &["192.0.2.1"]),
    ]);
    let (gateway, server) = serve(Arc::default()).await;
    router_on("0.0.0.0", copy(&gateway));
    let (found, local) = upnp_gateway(IpAddr::from([0, 0, 0, 0]))
        .await
        .expect("the router behind the bridge was never found");
    assert_eq!(found.addr, gateway.addr);
    assert_eq!(
        local,
        local_for(&gateway, IpAddr::from([0, 0, 0, 0])).unwrap()
    );
    assert_eq!(
        searches(),
        [IpAddr::from([198, 51, 100, 5]), IpAddr::from([0, 0, 0, 0])]
    );
    server.abort();
}
/// A round without a UPnP router takes one search's time: the uplinks' searches run at once, and
/// the search from no address in particular is not made again from where Windows sends
/// multicast, an address already searched from.
#[tokio::test]
async fn the_searches_of_a_round_run_at_once_and_none_twice() {
    with_adapters(vec![adapter([192, 0, 2, 10].into(), true, false, 25, &[])]);
    TEST_NET.with(|net| net.borrow_mut().multicast_from = Some([192, 0, 2, 10].into()));
    assert!(upnp_gateway(IpAddr::from([0, 0, 0, 0])).await.is_err());
    assert_eq!(
        searches(),
        [IpAddr::from([192, 0, 2, 10])],
        "one uplink, two searches"
    );
    with_adapters(ethernet_and_wifi().to_vec());
    assert!(upnp_gateway(IpAddr::from([0, 0, 0, 0])).await.is_err());
    assert_eq!(searches().len(), 3);
    assert_eq!(
        TEST_NET.with(|net| net.borrow().overlap),
        3,
        "the searches ran one after another"
    );
}
/// The first address in search order wins even when its search answers after a later one's; and
/// a later address that found a router is used only once the earlier one has answered that it
/// found none.
#[tokio::test]
async fn the_first_address_in_order_wins_whenever_its_search_answers() {
    let (gateway, server) = serve(Arc::default()).await;
    let (other, other_server) = serve(Arc::default()).await;
    let slow_first = |routers: &[(&str, &Gateway<upnp::Bounded>)]| {
        with_adapters(ethernet_and_wifi().to_vec());
        TEST_NET.with(|net| {
            let mut net = net.borrow_mut();
            net.multicast_from = Some([192, 0, 2, 10].into());
            net.delays = vec![([192, 0, 2, 10].into(), 5)];
        });
        for (from, router) in routers {
            router_on(from, copy(router));
        }
    };
    slow_first(&[("192.0.2.10", &gateway), ("192.0.2.20", &other)]);
    let (found, local) = upnp_gateway(IpAddr::from([0, 0, 0, 0])).await.unwrap();
    let finished = TEST_NET.with(|net| net.borrow().finished.clone());
    assert_eq!(finished.first(), Some(&IpAddr::from([192, 0, 2, 20])));
    assert_eq!(
        (found.addr, local),
        (gateway.addr, IpAddr::from([192, 0, 2, 10])),
        "the search that answered first won, not the first in order"
    );
    slow_first(&[("192.0.2.20", &other)]);
    let (found, local) = upnp_gateway(IpAddr::from([0, 0, 0, 0])).await.unwrap();
    assert_eq!(
        (found.addr, local),
        (other.addr, IpAddr::from([192, 0, 2, 20]))
    );
    assert!(
        TEST_NET.with(|net| net
            .borrow()
            .finished
            .contains(&IpAddr::from([192, 0, 2, 10]))),
        "the later address was used before the first had answered"
    );
    server.abort();
    other_server.abort();
}
