//! Real loopback SOAP requests exercise permanent-only routers and ownership.

mod lease_and_table_tests;
mod renewal_tests;
mod search_order_tests;

use super::*;
use axum::{Router, extract::State, http::HeaderMap, routing::post};
use std::sync::{Arc, Mutex};
#[derive(Clone)]
pub(super) struct Row {
    pub(super) port: u16,
    pub(super) protocol: Protocol,
    pub(super) owner: String,
    /// The address the mapping forwards to.
    pub(super) client: String,
}
#[derive(Default)]
pub(super) struct RouterState {
    pub(super) rows: Vec<Row>,
    pub(super) leases: Vec<u32>,
    pub(super) deleted: Vec<u16>,
    /// Answer every table read with this UPnP error.
    pub(super) list_fault: Option<u32>,
    pub(super) add_fault: Option<u32>,
    /// Wait this long before answering a map request, for a test that looks at the record
    /// while the request is in flight.
    pub(super) add_delay: Option<std::time::Duration>,
    /// Every SOAP request this router got.
    pub(super) requests: usize,
    /// Answer nothing a UPnP client can read, as a router that went away or stopped its UPnP
    /// does: the client gets no answer at once rather than after its timeout.
    pub(super) silent: bool,
    /// Answer no request for the router's own address, as a busy router may not.
    pub(super) ignore_external: bool,
    /// The map requests that reached the router, answered or not.
    pub(super) adds_started: usize,
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
        let mut state = state.lock().unwrap();
        state.adds_started += 1;
        state.add_delay
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
pub(super) async fn serve(
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
/// [`apply_round`] on a router that is not on trial.
#[allow(clippy::too_many_arguments)]
pub(super) async fn apply<P: Provider>(
    gateway: &Gateway<P>,
    local: IpAddr,
    wanted: &[Mapping],
    description: &str,
    reported: &mut Vec<Mapping>,
    previous: &[Mapping],
    record: &std::sync::Mutex<LeaseRecord>,
    guest: &[Mapping],
) -> Applied {
    apply_round(
        gateway,
        local,
        wanted,
        description,
        reported,
        previous,
        record,
        guest,
        false,
    )
    .await
}
/// A guest lease record in a fresh temp folder, with the folder to remove after the test.
pub(super) fn guest_record() -> (Mutex<LeaseRecord>, std::path::PathBuf) {
    let directory = std::env::temp_dir().join(format!("rubylight-leases-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&directory).unwrap();
    (Mutex::new(LeaseRecord::load(&directory)), directory)
}
/// This PC's network for the port mapping task's tests: physical Ethernet or Wi-Fi addresses
/// `lan`, which are all the addresses it has, and no default gateway, so nothing falls through
/// to a router.
pub(super) fn on_lan(lan: &[&str]) {
    let lan: Vec<IpAddr> = lan.iter().map(|address| address.parse().unwrap()).collect();
    TEST_NET.with(|net| {
        *net.borrow_mut() = TestNet {
            gateways: Some(Vec::new()),
            addresses: Some(lan.clone()),
            // Windows sends multicast from the LAN.
            multicast_from: lan.first().copied(),
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
pub(super) fn vpn_up() {
    TEST_NET.with(|net| {
        let mut net = net.borrow_mut();
        net.addresses
            .get_or_insert_with(Vec::new)
            .push("10.8.0.2".parse().unwrap());
        net.gateways = Some(vec!["10.8.0.1".parse().unwrap()]);
        net.multicast_from = Some([10, 8, 0, 2].into());
    });
}
/// A UPnP search from `from` finds `gateway`.
pub(super) fn router_on(from: &str, gateway: Gateway<upnp::Bounded>) {
    TEST_NET.with(|net| {
        net.borrow_mut()
            .routers
            .push((from.parse().unwrap(), gateway));
    });
}
/// The next round comes a renewal after the silent one, as the task schedules it.
pub(super) fn a_renewal_later(held: &mut Held) {
    if let Some(since) = held.upnp_silent_since.as_mut() {
        *since = since.checked_sub(RENEWAL).unwrap();
    }
}
pub(super) fn searches() -> Vec<IpAddr> {
    TEST_NET.with(|net| net.borrow().searches.clone())
}
/// The same router as `gateway`, as a new search would find it.
pub(super) fn copy(gateway: &Gateway<upnp::Bounded>) -> Gateway<upnp::Bounded> {
    Gateway {
        addr: gateway.addr,
        root_url: gateway.root_url.clone(),
        control_url: gateway.control_url.clone(),
        control_schema_url: gateway.control_schema_url.clone(),
        control_schema: gateway.control_schema.clone(),
        provider: upnp::Bounded,
    }
}
pub(super) const STREAM_PORT: Mapping = Mapping {
    protocol: Protocol::TCP,
    port: 48124,
};
/// One renewal round of the stream port, on a host that listens on every address.
pub(super) async fn renew_round(held: &mut Held, record: &Mutex<LeaseRecord>) {
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
pub(super) fn kept(held: &Held) -> Option<(SocketAddr, IpAddr, Vec<Mapping>)> {
    held.upnp
        .as_ref()
        .map(|(gateway, local, applied)| (gateway.addr, *local, applied.clone()))
}
pub(super) fn mapped_on(
    gateway: &Gateway<upnp::Bounded>,
    local: &str,
) -> Option<(SocketAddr, IpAddr, Vec<Mapping>)> {
    Some((gateway.addr, local.parse().unwrap(), vec![STREAM_PORT]))
}
/// The router's rows as (port, address it forwards to).
pub(super) fn forwards(state: &Mutex<RouterState>) -> Vec<(u16, String)> {
    state
        .lock()
        .unwrap()
        .rows
        .iter()
        .map(|row| (row.port, row.client.clone()))
        .collect()
}
pub(super) fn forwarded_to(address: &str) -> Vec<(u16, String)> {
    vec![(STREAM_PORT.port, address.to_owned())]
}
/// A UPnP router that holds the stream port for this PC at 192.0.2.10, as one round left it.
pub(super) async fn mapped_router(
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
/// An Ethernet or Wi-Fi adapter of this PC: `hardware` a physical one, `hyper_v` Hyper-V's virtual
/// adapter, with its IPv4 metric and default gateways.
pub(super) fn adapter(
    address: std::net::Ipv4Addr,
    hardware: bool,
    hyper_v: bool,
    metric: u32,
    gateways: &[&str],
) -> butterpollo_win_io::net::LanInterface {
    butterpollo_win_io::net::LanInterface {
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
pub(super) fn with_adapters(adapters: Vec<butterpollo_win_io::net::LanInterface>) {
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
pub(super) fn ethernet_and_wifi() -> [butterpollo_win_io::net::LanInterface; 2] {
    [
        adapter([192, 0, 2, 10].into(), true, false, 25, &["192.0.2.1"]),
        adapter([192, 0, 2, 20].into(), true, false, 35, &["192.0.2.1"]),
    ]
}
/// Windows routes to `gateway` from `source`.
pub(super) fn route_to(gateway: &str, source: &str) {
    TEST_NET.with(|net| {
        net.borrow_mut()
            .routes
            .push((gateway.parse().unwrap(), source.parse().unwrap()));
    });
}
