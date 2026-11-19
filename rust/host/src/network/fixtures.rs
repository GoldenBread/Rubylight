//! Real loopback SOAP requests exercise permanent-only routers and ownership.
use super::*;
use axum::{Router, extract::State, http::HeaderMap, routing::post};
use std::sync::{Arc, Mutex};
#[derive(Clone)]
struct Row {
    port: u16,
    protocol: Protocol,
    owner: String,
    /// The address the mapping forwards to.
    client: String,
}
#[derive(Default)]
struct RouterState {
    rows: Vec<Row>,
    leases: Vec<u32>,
    deleted: Vec<u16>,
    /// Answer every table read with this UPnP error.
    list_fault: Option<u32>,
    add_fault: Option<u32>,
    /// Wait this long before answering a map request, for a test that looks at the record
    /// while the request is in flight.
    add_delay: Option<std::time::Duration>,
    /// Every SOAP request this router got.
    requests: usize,
    /// Answer nothing a UPnP client can read, as a router that went away or stopped its UPnP
    /// does: the client gets no answer at once rather than after its timeout.
    silent: bool,
    /// Answer no request for the router's own address, as a busy router may not.
    ignore_external: bool,
}
fn text<'a>(body: &'a str, tag: &str) -> &'a str {
    body.split_once(&format!("<{tag}>"))
        .unwrap()
        .1
        .split_once(&format!("</{tag}>"))
        .unwrap()
        .0
}
fn protocol(body: &str) -> Protocol {
    match text(body, "NewProtocol") {
        "UDP" => Protocol::UDP,
        _ => Protocol::TCP,
    }
}
fn response(action: &str, value: &str) -> String {
    format!(
        "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Body><u:{action}Response xmlns:u=\"urn:schemas-upnp-org:service:WANIPConnection:1\">{value}</u:{action}Response></s:Body></s:Envelope>"
    )
}
fn fault(code: u32) -> String {
    format!(
        "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Body><s:Fault><detail><UPnPError xmlns=\"urn:schemas-upnp-org:control-1-0\"><errorCode>{code}</errorCode><errorDescription>fixture</errorDescription></UPnPError></detail></s:Fault></s:Body></s:Envelope>"
    )
}
async fn soap(
    State(state): State<Arc<Mutex<RouterState>>>,
    headers: HeaderMap,
    body: String,
) -> String {
    let action = headers["soapaction"]
        .to_str()
        .unwrap()
        .split('#')
        .nth(1)
        .unwrap()
        .trim_end_matches('"');
    // A test can hold the map request here while it checks the lease record.
    let delay = if action == "AddPortMapping" {
        state.lock().unwrap().add_delay
    } else {
        None
    };
    if let Some(delay) = delay {
        tokio::time::sleep(delay).await;
    }
    let mut state = state.lock().unwrap();
    state.requests += 1;
    if state.silent {
        return String::new();
    }
    match action {
        "GetGenericPortMappingEntry" => {
            if let Some(code) = state.list_fault {
                return fault(code);
            }
            let index: usize = text(&body, "NewPortMappingIndex").parse().unwrap();
            let Some(row) = state.rows.get(index) else {
                return fault(713);
            };
            response(
                action,
                &format!(
                    "<NewRemoteHost></NewRemoteHost><NewExternalPort>{}</NewExternalPort><NewProtocol>{}</NewProtocol><NewInternalPort>{}</NewInternalPort><NewInternalClient>{}</NewInternalClient><NewEnabled>1</NewEnabled><NewPortMappingDescription>{}</NewPortMappingDescription><NewLeaseDuration>0</NewLeaseDuration>",
                    row.port, row.protocol, row.port, row.client, row.owner
                ),
            )
        }
        "AddPortMapping" => {
            if let Some(code) = state.add_fault {
                return fault(code);
            }
            let lease = text(&body, "NewLeaseDuration").parse().unwrap();
            state.leases.push(lease);
            if lease != 0 {
                return fault(725);
            }
            let port = text(&body, "NewExternalPort").parse().unwrap();
            let protocol = protocol(&body);
            let owner = text(&body, "NewPortMappingDescription").into();
            let client: String = text(&body, "NewInternalClient").into();
            // As routers do, a port forwarded to another address is not taken over.
            if state
                .rows
                .iter()
                .any(|row| (row.port, row.protocol) == (port, protocol) && row.client != client)
            {
                return fault(718);
            }
            state
                .rows
                .retain(|row| (row.port, row.protocol) != (port, protocol));
            state.rows.push(Row {
                port,
                protocol,
                owner,
                client,
            });
            response(action, "")
        }
        "DeletePortMapping" => {
            let port = text(&body, "NewExternalPort").parse().unwrap();
            let protocol = protocol(&body);
            state.deleted.push(port);
            state
                .rows
                .retain(|row| (row.port, row.protocol) != (port, protocol));
            response(action, "")
        }
        "GetExternalIPAddress" if state.ignore_external => String::new(),
        "GetExternalIPAddress" => response(
            action,
            "<NewExternalIPAddress>203.0.113.9</NewExternalIPAddress>",
        ),
        _ => panic!("unexpected SOAP action {action}"),
    }
}
async fn serve(
    state: Arc<Mutex<RouterState>>,
) -> (Gateway<upnp::Bounded>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = Router::new()
        .route("/control", post(soap))
        .with_state(state);
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let schema = [
        (
            "AddPortMapping",
            vec![
                "NewRemoteHost",
                "NewExternalPort",
                "NewProtocol",
                "NewInternalPort",
                "NewInternalClient",
                "NewEnabled",
                "NewPortMappingDescription",
                "NewLeaseDuration",
            ],
        ),
        (
            "DeletePortMapping",
            vec!["NewRemoteHost", "NewExternalPort", "NewProtocol"],
        ),
    ]
    .into_iter()
    .map(|(name, args)| (name.into(), args.into_iter().map(str::to_owned).collect()))
    .collect();
    let gateway = Gateway {
        addr,
        root_url: "/root.xml".into(),
        control_url: "/control".into(),
        control_schema_url: "/schema.xml".into(),
        control_schema: schema,
        provider: upnp::Bounded,
    };
    (gateway, server)
}
/// A guest lease record in a fresh temp folder, with the folder to remove after the test.
pub(super) fn guest_record() -> (Mutex<LeaseRecord>, std::path::PathBuf) {
    let directory = std::env::temp_dir().join(format!("rubylight-leases-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&directory).unwrap();
    (Mutex::new(LeaseRecord::load(&directory)), directory)
}
#[tokio::test]
async fn permanent_only_router_reclaims_old_own_ports_and_preserves_other_owners() {
    let owner = "Butterpollo Rust fixture-host";
    let state = Arc::new(Mutex::new(RouterState {
        rows: vec![
            Row {
                port: 48124,
                protocol: Protocol::TCP,
                owner: "Other application".into(),
                client: "192.0.2.10".into(),
            },
            Row {
                port: 48125,
                protocol: Protocol::TCP,
                owner: owner.into(),
                client: "192.0.2.10".into(),
            },
        ],
        ..Default::default()
    }));
    let (gateway, server) = serve(state.clone()).await;
    let local = "192.0.2.10".parse().unwrap();
    let wanted = [
        Mapping {
            protocol: Protocol::TCP,
            port: 48124,
        },
        Mapping {
            protocol: Protocol::TCP,
            port: 48126,
        },
    ];
    let mut reported = Vec::new();
    let (record, directory) = guest_record();
    let outcome = apply(
        &gateway,
        local,
        &wanted,
        owner,
        &mut reported,
        &[],
        &record,
        &wanted,
    )
    .await;
    // The other application's 48124 already forwards to this PC: no warning, guests reach it
    // under its own number, and the mapping stays the application's.
    assert!(reported.is_empty());
    assert_eq!(state.lock().unwrap().leases, [120, 0]);
    assert_eq!(outcome.forwarded, [wanted[0]]);
    // This host's own 48125, left by an earlier run and no longer wanted, goes at once.
    assert_eq!(outcome.held, [wanted[1]]);
    assert_eq!(state.lock().unwrap().deleted, [48125]);
    // Only the port this host holds is recorded.
    assert_eq!(
        record.lock().unwrap().leases(),
        [GuestLease::new(LeaseMethod::Upnp, wanted[1])]
    );
    // A restarted host adopts its own permanent mapping.
    let adopted = apply(
        &gateway,
        local,
        &[wanted[1]],
        owner,
        &mut reported,
        &[],
        &record,
        &wanted[1..],
    )
    .await;
    assert_eq!(adopted.held, outcome.held);
    // A third party replaced a mapping after it was created. Removing what is no longer wanted,
    // and teardown, must inspect current ownership, not trust the host's previous success.
    state
        .lock()
        .unwrap()
        .rows
        .iter_mut()
        .find(|r| r.port == 48126)
        .unwrap()
        .owner = "New owner".into();
    let last = apply(
        &gateway,
        local,
        &[],
        owner,
        &mut reported,
        &adopted.held,
        &record,
        &[],
    )
    .await;
    assert!(last.held.is_empty());
    // The table now shows another owner, so the host stops listing that lease.
    assert!(record.lock().unwrap().leases().is_empty());
    let _ = remove(&gateway, local, &adopted.held, owner).await;
    let state = state.lock().unwrap();
    assert_eq!(state.deleted, [48125]);
    assert_eq!(
        state.rows.iter().map(|r| r.port).collect::<Vec<_>>(),
        [48124, 48126]
    );
    drop(state);
    let _ = std::fs::remove_dir_all(directory);
    server.abort();
}
#[tokio::test]
async fn a_manual_forward_to_this_pc_counts_as_mapped_and_is_left_alone() {
    let owner = "Butterpollo Rust fixture-host";
    let local = "192.0.2.10".parse().unwrap();
    // A forward the user made by hand on the router, under another description.
    let state = Arc::new(Mutex::new(RouterState {
        rows: vec![Row {
            port: 48125,
            protocol: Protocol::TCP,
            owner: "Sunshine".into(),
            client: "192.0.2.10".into(),
        }],
        ..Default::default()
    }));
    let (gateway, server) = serve(state.clone()).await;
    let wanted = [Mapping {
        protocol: Protocol::TCP,
        port: 48125,
    }];
    let mut reported = Vec::new();
    let (record, directory) = guest_record();
    let outcome = apply(
        &gateway,
        local,
        &wanted,
        owner,
        &mut reported,
        &[],
        &record,
        &wanted,
    )
    .await;
    // The router already forwards the guest port to this PC: reachable at its own number, with
    // no lease, no conflict, and no mapping of this host's.
    assert!(outcome.held.is_empty());
    assert_eq!(outcome.forwarded, wanted);
    assert!(reported.is_empty());
    assert!(state.lock().unwrap().leases.is_empty());
    assert!(record.lock().unwrap().leases().is_empty());
    // The link ends: the forward was never this host's, so it stays on the router and stops
    // counting.
    let outcome = apply(
        &gateway,
        local,
        &[],
        owner,
        &mut reported,
        &outcome.held,
        &record,
        &[],
    )
    .await;
    assert!(outcome.held.is_empty());
    assert!(outcome.forwarded.is_empty());
    let state = state.lock().unwrap();
    assert!(state.deleted.is_empty());
    assert_eq!(state.rows.len(), 1);
    drop(state);
    let _ = std::fs::remove_dir_all(directory);
    server.abort();
}

#[tokio::test]
async fn a_refused_renewal_keeps_the_lease_this_host_holds() {
    let owner = "Butterpollo Rust fixture-host";
    let local = "192.0.2.10".parse().unwrap();
    let mapping = Mapping {
        protocol: Protocol::TCP,
        port: 48125,
    };
    // The table shows this host's mapping; the router then refuses the renewal request.
    let state = Arc::new(Mutex::new(RouterState {
        rows: vec![Row {
            port: 48125,
            protocol: Protocol::TCP,
            owner: owner.into(),
            client: "192.0.2.10".into(),
        }],
        add_fault: Some(718),
        ..Default::default()
    }));
    let (gateway, server) = serve(state.clone()).await;
    let (record, directory) = guest_record();
    let lease = GuestLease::new(LeaseMethod::Upnp, mapping);
    record.lock().unwrap().add_all([lease]);
    let outcome = apply(
        &gateway,
        local,
        &[mapping],
        owner,
        &mut Vec::new(),
        &[],
        &record,
        &[mapping],
    )
    .await;
    // The mapping is still this host's; a refused renewal is not a confirmation it is gone.
    assert_eq!(outcome.held, [mapping]);
    assert_eq!(record.lock().unwrap().leases(), [lease]);
    let _ = std::fs::remove_dir_all(directory);
    server.abort();
}

#[tokio::test]
async fn a_map_request_is_recorded_before_it_is_answered() {
    let owner = "Butterpollo Rust fixture-host";
    let local = "192.0.2.10".parse().unwrap();
    let state = Arc::new(Mutex::new(RouterState {
        add_delay: Some(std::time::Duration::from_secs(10)),
        ..Default::default()
    }));
    let (gateway, server) = serve(state.clone()).await;
    let (record, directory) = guest_record();
    let record = Arc::new(record);
    let wanted = [Mapping {
        protocol: Protocol::TCP,
        port: 48125,
    }];
    let round_record = record.clone();
    let round = tokio::spawn(async move {
        apply(
            &gateway,
            local,
            &wanted,
            owner,
            &mut Vec::new(),
            &[],
            &round_record,
            &wanted,
        )
        .await
    });
    // While the router has not answered the map request, the lease is already listed: a stop
    // in between still leaves it to be deleted at the next start.
    let mut listed = false;
    for _ in 0..200 {
        if !record.lock().unwrap().leases().is_empty() {
            listed = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        listed,
        "the lease is recorded before the map request is answered"
    );
    round.abort();
    let _ = round.await;
    let _ = std::fs::remove_dir_all(directory);
    server.abort();
}

#[tokio::test]
async fn a_start_removes_only_the_guest_ports_the_record_lists() {
    let owner = "Butterpollo Rust fixture-host";
    let local = "192.0.2.10".parse().unwrap();
    let row = |port, protocol, owner: &str| Row {
        port,
        protocol,
        owner: owner.into(),
        client: "192.0.2.10".into(),
    };
    let state = Arc::new(Mutex::new(RouterState {
        rows: vec![
            row(48125, Protocol::TCP, owner),
            // A streaming port and another application's media port stay.
            row(48124, Protocol::TCP, owner),
            row(8003, Protocol::UDP, owner),
            row(8004, Protocol::UDP, "Other application"),
            row(8002, Protocol::UDP, owner),
            // No longer a configured guest port, but the record lists it: it goes too.
            row(8006, Protocol::UDP, owner),
        ],
        ..Default::default()
    }));
    let (gateway, server) = serve(state.clone()).await;
    let (record, directory) = guest_record();
    let leases = [
        GuestLease::new(
            LeaseMethod::Upnp,
            Mapping {
                protocol: Protocol::TCP,
                port: 48125,
            },
        ),
        GuestLease::new(
            LeaseMethod::Upnp,
            Mapping {
                protocol: Protocol::UDP,
                port: 8002,
            },
        ),
        GuestLease::new(
            LeaseMethod::Upnp,
            Mapping {
                protocol: Protocol::UDP,
                port: 8006,
            },
        ),
    ];
    record.lock().unwrap().add_all(leases);
    remove_guest_leftovers(&gateway, local, owner, &record, &leases).await;
    let state = state.lock().unwrap();
    assert_eq!(state.deleted, [48125, 8002, 8006]);
    assert_eq!(
        state.rows.iter().map(|row| row.port).collect::<Vec<_>>(),
        [48124, 8003, 8004]
    );
    drop(state);
    // Every delete was answered: the record lists nothing more.
    assert!(record.lock().unwrap().leases().is_empty());
    let _ = std::fs::remove_dir_all(directory);
    server.abort();
}

#[tokio::test]
async fn a_cleanup_that_cannot_read_the_table_keeps_the_record() {
    let owner = "Butterpollo Rust fixture-host";
    let local = "192.0.2.10".parse().unwrap();
    let state = Arc::new(Mutex::new(RouterState {
        list_fault: Some(606),
        ..Default::default()
    }));
    let (gateway, server) = serve(state.clone()).await;
    let (record, directory) = guest_record();
    let lease = GuestLease::new(
        LeaseMethod::Upnp,
        Mapping {
            protocol: Protocol::TCP,
            port: 48125,
        },
    );
    record.lock().unwrap().add_all([lease]);
    // The router will not list its table, so it cannot confirm the lease is gone.
    remove_guest_leftovers(&gateway, local, owner, &record, &[lease]).await;
    assert_eq!(record.lock().unwrap().leases(), [lease]);
    assert!(state.lock().unwrap().deleted.is_empty());
    let _ = std::fs::remove_dir_all(directory);
    server.abort();
}

#[tokio::test]
async fn a_router_that_hides_its_table_still_gets_and_loses_the_mappings() {
    let owner = "Butterpollo Rust fixture-host";
    let state = Arc::new(Mutex::new(RouterState {
        list_fault: Some(606),
        ..Default::default()
    }));
    let (gateway, server) = serve(state.clone()).await;
    let local = "192.0.2.10".parse().unwrap();
    let wanted = [Mapping {
        protocol: Protocol::TCP,
        port: 48126,
    }];
    let (record, directory) = guest_record();
    let outcome = apply(
        &gateway,
        local,
        &wanted,
        owner,
        &mut Vec::new(),
        &[],
        &record,
        &wanted,
    )
    .await;
    assert_eq!(outcome.held, wanted);
    assert!(outcome.forwarded.is_empty());
    assert_eq!(
        record.lock().unwrap().leases(),
        [GuestLease::new(LeaseMethod::Upnp, wanted[0])]
    );
    assert_eq!(state.lock().unwrap().rows.len(), 1);
    // The next round still knows it: the table can't show it, yet it goes once unwanted.
    let again = apply(
        &gateway,
        local,
        &wanted,
        owner,
        &mut Vec::new(),
        &outcome.held,
        &record,
        &wanted,
    )
    .await;
    assert_eq!(again.held, wanted);
    let last = apply(
        &gateway,
        local,
        &[],
        owner,
        &mut Vec::new(),
        &again.held,
        &record,
        &[],
    )
    .await;
    assert!(last.held.is_empty());
    assert_eq!(state.lock().unwrap().deleted, [48126]);
    // The delete was answered, so the lease leaves the record.
    assert!(record.lock().unwrap().leases().is_empty());
    let _ = remove(&gateway, local, &outcome.held, owner).await;
    let _ = std::fs::remove_dir_all(directory);
    server.abort();
}

#[tokio::test]
async fn hidden_table_conflicts_remain_reported_until_the_mapping_succeeds() {
    let state = Arc::new(Mutex::new(RouterState {
        list_fault: Some(606),
        add_fault: Some(718),
        ..Default::default()
    }));
    let (gateway, server) = serve(state.clone()).await;
    let wanted = [Mapping {
        protocol: Protocol::TCP,
        port: 48126,
    }];
    let mut reported = Vec::new();
    let (record, directory) = guest_record();
    for _ in 0..2 {
        let outcome = apply(
            &gateway,
            "192.0.2.10".parse().unwrap(),
            &wanted,
            "fixture",
            &mut reported,
            &[],
            &record,
            &wanted,
        )
        .await;
        assert!(outcome.held.is_empty());
        assert_eq!(reported, wanted);
        // A refused map request leaves no lease in the record.
        assert!(record.lock().unwrap().leases().is_empty());
        state.lock().unwrap().list_fault = None;
    }
    state.lock().unwrap().add_fault = None;
    let mapped = apply(
        &gateway,
        "192.0.2.10".parse().unwrap(),
        &wanted,
        "fixture",
        &mut reported,
        &[],
        &record,
        &wanted,
    )
    .await;
    assert_eq!(mapped.held, wanted);
    assert_eq!(
        record.lock().unwrap().leases(),
        [GuestLease::new(LeaseMethod::Upnp, wanted[0])]
    );
    assert!(reported.is_empty());
    let _ = std::fs::remove_dir_all(directory);
    server.abort();
}

#[tokio::test]
async fn browser_guest_ports_are_forwarded_while_a_link_needs_them_and_removed_after() {
    let owner = "Butterpollo Rust fixture-host";
    let local = "192.0.2.10".parse().unwrap();
    let guest = |rows: &[Row]| {
        let mut ports: Vec<_> = rows
            .iter()
            .filter(|r| r.port == 48125 || (8002..=8005).contains(&r.port))
            .map(|r| (r.protocol, r.port))
            .collect();
        ports.sort_by_key(|&(_, port)| port);
        ports
    };
    let ports = Ports::from_base(48123);
    for (config, upnp, expected) in [
        ("upnp=true\n", true, vec![]),
        (
            "upnp=true\nwebrtc_server=true\n",
            true,
            vec![
                (Protocol::UDP, 8002),
                (Protocol::UDP, 8003),
                (Protocol::UDP, 8004),
                (Protocol::UDP, 8005),
                (Protocol::TCP, 48125),
            ],
        ),
        // Automatic port forwarding off: an internet link forwards the guest ports alone.
        (
            "webrtc_server=true\n",
            false,
            vec![
                (Protocol::UDP, 8002),
                (Protocol::UDP, 8003),
                (Protocol::UDP, 8004),
                (Protocol::UDP, 8005),
                (Protocol::TCP, 48125),
            ],
        ),
    ] {
        let state = Arc::new(Mutex::new(RouterState::default()));
        let (gateway, server) = serve(state.clone()).await;
        let (record, directory) = guest_record();
        let config = Config::parse(config).unwrap();
        // An internet link works.
        let (stream, guests) = wanted_now(&config, ports, upnp, true, true);
        let wanted: Vec<Mapping> = stream.iter().chain(&guests).copied().collect();
        let outcome = apply(
            &gateway,
            local,
            &wanted,
            owner,
            &mut Vec::new(),
            &[],
            &record,
            &guests,
        )
        .await;
        assert_eq!(outcome.held, wanted);
        assert_eq!(guest(&state.lock().unwrap().rows), expected);
        if !upnp {
            assert_eq!(state.lock().unwrap().rows.len(), expected.len());
        }
        // The link is gone and nobody watches: the guest ports go, the streaming ports stay.
        let (stream, guests) = wanted_now(&config, ports, upnp, false, false);
        assert!(guests.is_empty());
        let outcome = apply(
            &gateway,
            local,
            &stream,
            owner,
            &mut Vec::new(),
            &outcome.held,
            &record,
            &guests,
        )
        .await;
        assert_eq!(outcome.held, stream);
        assert!(guest(&state.lock().unwrap().rows).is_empty());
        assert_eq!(state.lock().unwrap().rows.len(), stream.len());
        // The guest deletes were answered, so their leases left the record.
        assert!(record.lock().unwrap().leases().is_empty());
        let _ = remove(&gateway, local, &outcome.held, owner).await;
        assert!(state.lock().unwrap().rows.is_empty());
        let _ = std::fs::remove_dir_all(directory);
        server.abort();
    }
}

/// This PC's network for the port mapping task's tests: physical Ethernet or Wi-Fi addresses
/// `lan`, which are all the addresses it has, and no default gateway, so nothing falls through
/// to a router.
fn on_lan(lan: &[&str]) {
    let lan: Vec<IpAddr> = lan.iter().map(|address| address.parse().unwrap()).collect();
    TEST_NET.with(|net| {
        *net.borrow_mut() = TestNet {
            gateways: Some(Vec::new()),
            addresses: Some(lan.clone()),
            lan: lan
                .into_iter()
                .map(|address| match address {
                    IpAddr::V4(address) => adapter(address, true, false, 25, &[]),
                    IpAddr::V6(_) => unreachable!(),
                })
                .collect(),
            ..Default::default()
        };
    });
}
/// A VPN connects: it adds an address and takes the default route.
fn vpn_up() {
    TEST_NET.with(|net| {
        let mut net = net.borrow_mut();
        net.addresses
            .get_or_insert_with(Vec::new)
            .push("10.8.0.2".parse().unwrap());
        net.gateways = Some(vec!["10.8.0.1".parse().unwrap()]);
    });
}
/// A UPnP search from `from` finds `gateway`.
fn router_on(from: &str, gateway: Gateway<upnp::Bounded>) {
    TEST_NET.with(|net| {
        net.borrow_mut()
            .routers
            .push((from.parse().unwrap(), gateway));
    });
}
/// The next round comes a renewal after the silent one, as the task schedules it.
fn a_renewal_later(held: &mut Held) {
    if let Some(since) = held.upnp_silent_since.as_mut() {
        *since = since.checked_sub(RENEWAL).unwrap();
    }
}
fn searches() -> Vec<IpAddr> {
    TEST_NET.with(|net| net.borrow().searches.clone())
}
/// The same router as `gateway`, as a new search would find it.
fn copy(gateway: &Gateway<upnp::Bounded>) -> Gateway<upnp::Bounded> {
    Gateway {
        addr: gateway.addr,
        root_url: gateway.root_url.clone(),
        control_url: gateway.control_url.clone(),
        control_schema_url: gateway.control_schema_url.clone(),
        control_schema: gateway.control_schema.clone(),
        provider: upnp::Bounded,
    }
}
const STREAM_PORT: Mapping = Mapping {
    protocol: Protocol::TCP,
    port: 48124,
};
/// One renewal round of the stream port, on a host that listens on every address.
async fn renew_round(held: &mut Held, record: &Mutex<LeaseRecord>) {
    let mut reported = Vec::new();
    renew(
        held,
        IpAddr::from([0, 0, 0, 0]),
        &[STREAM_PORT],
        &[],
        &[STREAM_PORT],
        "Butterpollo Rust fixture-host",
        b"key",
        &mut reported,
        record,
    )
    .await;
}
/// The UPnP router `held` keeps, by address, with the address its mappings are for and them.
fn kept(held: &Held) -> Option<(SocketAddr, IpAddr, Vec<Mapping>)> {
    held.upnp
        .as_ref()
        .map(|(gateway, local, applied)| (gateway.addr, *local, applied.clone()))
}
fn mapped_on(
    gateway: &Gateway<upnp::Bounded>,
    local: &str,
) -> Option<(SocketAddr, IpAddr, Vec<Mapping>)> {
    Some((gateway.addr, local.parse().unwrap(), vec![STREAM_PORT]))
}
/// The router's rows as (port, address it forwards to).
fn forwards(state: &Mutex<RouterState>) -> Vec<(u16, String)> {
    state
        .lock()
        .unwrap()
        .rows
        .iter()
        .map(|row| (row.port, row.client.clone()))
        .collect()
}
fn forwarded_to(address: &str) -> Vec<(u16, String)> {
    vec![(STREAM_PORT.port, address.to_owned())]
}
/// A UPnP router that holds the stream port for this PC at 192.0.2.10, as one round left it.
async fn mapped_router(
    state: &Arc<Mutex<RouterState>>,
    record: &Mutex<LeaseRecord>,
) -> (Held, Gateway<upnp::Bounded>, tokio::task::JoinHandle<()>) {
    let (gateway, server) = serve(state.clone()).await;
    let mut held = Held {
        upnp: Some((copy(&gateway), "192.0.2.10".parse().unwrap(), Vec::new())),
        ..Default::default()
    };
    renew_round(&mut held, record).await;
    assert_eq!(kept(&held), mapped_on(&gateway, "192.0.2.10"));
    (held, gateway, server)
}

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
    // The search from no address in particular comes last, as it did before.
    assert_eq!(
        searches(),
        [IpAddr::from([192, 0, 2, 10]), IpAddr::from([0, 0, 0, 0])]
    );
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

/// An Ethernet or Wi-Fi adapter of this PC: `hardware` a physical one, `hyper_v` Hyper-V's virtual
/// adapter, with its IPv4 metric and default gateways.
fn adapter(
    address: std::net::Ipv4Addr,
    hardware: bool,
    hyper_v: bool,
    metric: u32,
    gateways: &[&str],
) -> butterpollo_windows::net::LanInterface {
    butterpollo_windows::net::LanInterface {
        address,
        hardware,
        hyper_v,
        metric,
        gateways: gateways
            .iter()
            .map(|gateway| gateway.parse().unwrap())
            .collect(),
    }
}
/// This PC's adapters are `adapters`, and every address it has is theirs.
fn with_adapters(adapters: Vec<butterpollo_windows::net::LanInterface>) {
    TEST_NET.with(|net| {
        *net.borrow_mut() = TestNet {
            gateways: Some(Vec::new()),
            addresses: Some(
                adapters
                    .iter()
                    .map(|adapter| IpAddr::V4(adapter.address))
                    .collect(),
            ),
            lan: adapters,
            ..Default::default()
        };
    });
}
/// Ethernet at metric 25 and Wi-Fi at metric 35, both on the router at 192.0.2.1.
fn ethernet_and_wifi() -> [butterpollo_windows::net::LanInterface; 2] {
    [
        adapter([192, 0, 2, 10].into(), true, false, 25, &["192.0.2.1"]),
        adapter([192, 0, 2, 20].into(), true, false, 35, &["192.0.2.1"]),
    ]
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
        [IpAddr::from([192, 0, 2, 10]), IpAddr::from([192, 0, 2, 20])]
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
    assert_eq!(searches(), [IpAddr::from([192, 0, 2, 10])]);
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

/// Windows routes to `gateway` from `source`.
fn route_to(gateway: &str, source: &str) {
    TEST_NET.with(|net| {
        net.borrow_mut()
            .routes
            .push((gateway.parse().unwrap(), source.parse().unwrap()));
    });
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
