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
    let (applied, forwarded) = apply(&gateway, local, &wanted, owner, &mut reported, &[])
        .await
        .unwrap();
    // The other application's 48124 already forwards to this PC: no warning, guests reach it
    // under its own number, and the mapping stays the application's.
    assert!(reported.is_empty());
    assert_eq!(state.lock().unwrap().leases, [120, 0]);
    assert_eq!(forwarded, [wanted[0]]);
    // This host's own 48125, left by an earlier run and no longer wanted, goes at once.
    assert_eq!(applied, [wanted[1]]);
    assert_eq!(state.lock().unwrap().deleted, [48125]);
    // A restarted host adopts its own permanent mapping.
    let (adopted, _) = apply(&gateway, local, &[wanted[1]], owner, &mut reported, &[])
        .await
        .unwrap();
    assert_eq!(adopted, applied);
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
    let (unwanted, _) = apply(&gateway, local, &[], owner, &mut reported, &adopted)
        .await
        .unwrap();
    assert!(unwanted.is_empty());
    remove(&gateway, local, &adopted, owner).await.unwrap();
    let state = state.lock().unwrap();
    assert_eq!(state.deleted, [48125]);
    assert_eq!(
        state.rows.iter().map(|r| r.port).collect::<Vec<_>>(),
        [48124, 48126]
    );
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
    let (applied, forwarded) = apply(&gateway, local, &wanted, owner, &mut reported, &[])
        .await
        .unwrap();
    // The router already forwards the guest port to this PC: reachable at its own number, with
    // no lease, no conflict, and no mapping of this host's.
    assert!(applied.is_empty());
    assert_eq!(forwarded, wanted);
    assert!(reported.is_empty());
    assert!(state.lock().unwrap().leases.is_empty());
    // The link ends: the forward was never this host's, so it stays on the router and stops
    // counting.
    let (applied, forwarded) = apply(&gateway, local, &[], owner, &mut reported, &applied)
        .await
        .unwrap();
    assert!(applied.is_empty());
    assert!(forwarded.is_empty());
    let state = state.lock().unwrap();
    assert!(state.deleted.is_empty());
    assert_eq!(state.rows.len(), 1);
    drop(state);
    server.abort();
}

#[tokio::test]
async fn a_start_removes_only_this_hosts_guest_ports_a_stop_left_forwarded() {
    let owner = "Butterpollo Rust fixture-host";
    let local = "192.0.2.10".parse().unwrap();
    let row = |port, owner: &str| Row {
        port,
        protocol: if port == 48125 {
            Protocol::TCP
        } else {
            Protocol::UDP
        },
        owner: owner.into(),
    };
    let state = Arc::new(Mutex::new(RouterState {
        rows: vec![
            row(48125, owner),
            // A streaming port and another application's media port stay.
            row(48124, owner),
            row(8003, "Other application"),
            row(8002, owner),
            row(8004, owner),
        ],
        ..Default::default()
    }));
    let (gateway, server) = serve(state.clone()).await;
    let config = Config::parse("webrtc_server=true\n").unwrap();
    let guest = guest_ports(&config, Ports::from_base(48123));
    remove_guest_leftovers(&gateway, local, &guest, owner).await;
    let state = state.lock().unwrap();
    assert_eq!(state.deleted, [48125, 8002, 8004]);
    assert_eq!(
        state.rows.iter().map(|row| row.port).collect::<Vec<_>>(),
        [48124, 8003]
    );
    drop(state);
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
    let (applied, forwarded) = apply(&gateway, local, &wanted, owner, &mut Vec::new(), &[])
        .await
        .unwrap();
    assert_eq!(applied, wanted);
    assert!(forwarded.is_empty());
    assert_eq!(state.lock().unwrap().rows.len(), 1);
    // The next round still knows it: the table can't show it, yet it goes once unwanted.
    let (again, _) = apply(&gateway, local, &wanted, owner, &mut Vec::new(), &applied)
        .await
        .unwrap();
    assert_eq!(again, wanted);
    let (unwanted, _) = apply(&gateway, local, &[], owner, &mut Vec::new(), &again)
        .await
        .unwrap();
    assert!(unwanted.is_empty());
    assert_eq!(state.lock().unwrap().deleted, [48126]);
    remove(&gateway, local, &applied, owner).await.unwrap();
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
    for _ in 0..2 {
        let (applied, _) = apply(
            &gateway,
            "192.0.2.10".parse().unwrap(),
            &wanted,
            "fixture",
            &mut reported,
            &[],
        )
        .await
        .unwrap();
        assert!(applied.is_empty());
        assert_eq!(reported, wanted);
        state.lock().unwrap().list_fault = None;
    }
    state.lock().unwrap().add_fault = None;
    let (mapped, _) = apply(
        &gateway,
        "192.0.2.10".parse().unwrap(),
        &wanted,
        "fixture",
        &mut reported,
        &[],
    )
    .await
    .unwrap();
    assert_eq!(mapped, wanted);
    assert!(reported.is_empty());
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
        let config = Config::parse(config).unwrap();
        // An internet link works.
        let (stream, guests) = wanted_now(&config, ports, upnp, true, true);
        let wanted: Vec<Mapping> = stream.iter().chain(&guests).copied().collect();
        let (applied, _) = apply(&gateway, local, &wanted, owner, &mut Vec::new(), &[])
            .await
            .unwrap();
        assert_eq!(applied, wanted);
        assert_eq!(guest(&state.lock().unwrap().rows), expected);
        if !upnp {
            assert_eq!(state.lock().unwrap().rows.len(), expected.len());
        }
        // The link is gone and nobody watches: the guest ports go, the streaming ports stay.
        let (stream, guests) = wanted_now(&config, ports, upnp, false, false);
        assert!(guests.is_empty());
        let (applied, _) = apply(&gateway, local, &stream, owner, &mut Vec::new(), &applied)
            .await
            .unwrap();
        assert_eq!(applied, stream);
        assert!(guest(&state.lock().unwrap().rows).is_empty());
        assert_eq!(state.lock().unwrap().rows.len(), stream.len());
        remove(&gateway, local, &applied, owner).await.unwrap();
        assert!(state.lock().unwrap().rows.is_empty());
        server.abort();
    }
}
