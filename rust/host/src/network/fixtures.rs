//! Real loopback SOAP requests exercise permanent-only routers and ownership.
use super::*;
use axum::{Router, extract::State, http::HeaderMap, routing::post};
use std::sync::{Arc, Mutex};
#[derive(Clone)]
struct Row {
    port: u16,
    protocol: Protocol,
    owner: String,
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
                    "<NewRemoteHost></NewRemoteHost><NewExternalPort>{}</NewExternalPort><NewProtocol>{}</NewProtocol><NewInternalPort>{}</NewInternalPort><NewInternalClient>192.0.2.10</NewInternalClient><NewEnabled>1</NewEnabled><NewPortMappingDescription>{}</NewPortMappingDescription><NewLeaseDuration>0</NewLeaseDuration>",
                    row.port, row.protocol, row.port, row.owner
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
            state
                .rows
                .retain(|row| (row.port, row.protocol) != (port, protocol));
            state.rows.push(Row {
                port,
                protocol,
                owner,
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
            },
            Row {
                port: 48125,
                protocol: Protocol::TCP,
                owner: owner.into(),
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
