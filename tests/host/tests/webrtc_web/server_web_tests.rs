//! WebRTC signalling server tests that pair/manage devices via `butterpollo-web` console APIs.

use butterpollo_session::webrtc::auth::proof_hex;
use butterpollo_stream::state::test_support::Fixture;
use butterpollo_web::web::update_clients;
use butterpollo_webrtc::WebRtcRuntime;
use butterpollo_webrtc::clients::MAX_PAIRING_REQUESTS;
use butterpollo_webrtc::server::{MAX_MESSAGE, REPLACED};
use butterpollo_webrtc::test_support::{Limits, Running, Ws, start_with};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

struct Server {
    f: Fixture,
    address: SocketAddr,
    _running: Running,
}

impl Server {
    fn start() -> Self {
        Self::start_with(Limits::default())
    }

    fn start_with(limits: Limits) -> Self {
        let f = Fixture::new();
        let running =
            start_with(f.host.clone(), [127, 0, 0, 1].into(), 0, 0, None, limits).expect("starts");
        let address = WebRtcRuntime::for_host(&f.host)
            .clients
            .status()
            .signalling
            .expect("listening");
        Self {
            f,
            address,
            _running: running,
        }
    }

    async fn connect(&self) -> (Ws, String) {
        let mut ws = Ws::connect(self.address).await;
        let nonce = ws.expect("auth-required").await["nonce"]
            .as_str()
            .unwrap()
            .to_owned();
        (ws, nonce)
    }

    /// Pair a device the way a TV does, approving through the console API.
    async fn pair(&self, name: &str) -> (String, String) {
        let (mut ws, _) = self.connect().await;
        ws.send(json!({"version":2,"type":"request-pairing","pin":"4321","clientName":name}))
            .await;
        let request = self.pending_request().await;
        update_clients(
            &self.f.host,
            "POST",
            "/api/pin",
            &json!({"pin":"4321","uniqueid":request}),
        )
        .unwrap();
        let paired = ws.expect("paired").await;
        (
            paired["clientId"].as_str().unwrap().to_owned(),
            paired["clientSecret"].as_str().unwrap().to_owned(),
        )
    }

    async fn pending_request(&self) -> String {
        for _ in 0..200 {
            let pending =
                update_clients(&self.f.host, "GET", "/api/clients/pending", &Value::Null).unwrap();
            if let Some(request) = pending["requests"].as_array().and_then(|r| r.first()) {
                assert_eq!(request["webrtc"], true);
                return request["uniqueid"].as_str().unwrap().to_owned();
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("no pairing request reached the console");
    }

    fn api(&self, path: &str, data: Value) -> Value {
        let method = if path.ends_with("list") {
            "GET"
        } else {
            "POST"
        };
        update_clients(&self.f.host, method, path, &data).unwrap()
    }
}

fn proof(secret: &str, nonce: &str) -> String {
    proof_hex(secret, nonce).unwrap()
}

async fn sign_in(server: &Server, client_id: &str, secret: &str) -> Ws {
    let (mut ws, nonce) = server.connect().await;
    ws.send(json!({"version":2,"type":"authenticate","clientId":client_id,"proof":proof(secret, &nonce)}))
        .await;
    ws.expect("authenticated").await;
    ws.expect("gateway-status").await;
    ws.expect("capabilities").await;
    ws
}

#[tokio::test]
async fn a_tv_pairs_through_the_console_pin_page_and_signs_in_with_its_secret() {
    let server = Server::start();
    let (mut ws, _) = server.connect().await;
    ws.send(
        json!({"version":2,"type":"request-pairing","pin":"4321","clientName":"Living room TV"}),
    )
    .await;
    let request = server.pending_request().await;
    // A wrong PIN is refused, once; the request is spent.
    update_clients(
        &server.f.host,
        "POST",
        "/api/pin",
        &json!({"pin":"1111","uniqueid":request}),
    )
    .unwrap();
    let refused = ws.expect("error").await;
    assert_eq!(refused["code"], "incorrect-pin");
    assert_eq!(refused["requestType"], "request-pairing");
    assert!(server.f.host.paired.read().unwrap().clients.is_empty());
    ws.send(
        json!({"version":2,"type":"request-pairing","pin":"4321","clientName":"Living room TV"}),
    )
    .await;
    let request = server.pending_request().await;
    update_clients(
        &server.f.host,
        "POST",
        "/api/pin",
        &json!({"pin":"4321","uniqueid":request,"name":"Den TV"}),
    )
    .unwrap();
    let paired = ws.expect("paired").await;
    ws.expect("gateway-status").await;
    ws.expect("capabilities").await;
    let client_id = paired["clientId"].as_str().unwrap();
    let secret = paired["clientSecret"].as_str().unwrap();
    assert_eq!(client_id.len(), 32);
    assert_eq!(secret.len(), 64);
    {
        let state = server.f.host.paired.read().unwrap();
        let client = &state.clients[0];
        assert_eq!(client.name, "Den TV");
        assert!(client.cert.is_empty());
        assert_eq!(client.webrtc(), Some((client_id, secret)));
    }
    // The credentials never reach the console.
    let list = server.api("/api/clients/list", Value::Null);
    assert_eq!(list["clients"][0]["webrtc"], json!({"clientId": client_id}));
    assert!(!list.to_string().contains(secret));
    // The same device signs in on a new connection.
    let mut again = sign_in(&server, client_id, secret).await;
    again.send(json!({"version":2,"type":"get-apps"})).await;
    let apps = again.expect("apps").await;
    assert!(!apps["apps"].as_array().unwrap().is_empty(), "{apps}");
    assert!(
        apps["apps"][0]["id"]
            .as_str()
            .unwrap()
            .parse::<u32>()
            .is_ok()
    );
}

#[tokio::test]
async fn a_wrong_proof_fails_but_the_connection_can_still_ask_to_pair() {
    let server = Server::start();
    let (client_id, secret) = server.pair("TV").await;
    let (mut ws, nonce) = server.connect().await;
    ws.send(json!({"version":2,"type":"authenticate","clientId":client_id,"proof":proof(&"0".repeat(64), &nonce)}))
        .await;
    assert_eq!(ws.expect("error").await["code"], "authentication-failed");
    // The nonce is spent: even the right proof fails now.
    ws.send(json!({"version":2,"type":"authenticate","clientId":client_id,"proof":proof(&secret, &nonce)}))
        .await;
    assert_eq!(ws.expect("error").await["code"], "authentication-failed");
    // An unknown device gets the same answer as a wrong secret.
    let (mut unknown, nonce) = server.connect().await;
    unknown
        .send(json!({"version":2,"type":"authenticate","clientId":"ab".repeat(16),"proof":proof(&secret, &nonce)}))
        .await;
    assert_eq!(
        unknown.expect("error").await["code"],
        "authentication-failed"
    );
    // Nothing but pairing works before signing in.
    unknown.send(json!({"version":2,"type":"get-apps"})).await;
    assert_eq!(unknown.expect("error").await["code"], "not-authenticated");
    unknown
        .send(json!({"version":2,"type":"request-pairing","pin":"1234","clientName":"Other"}))
        .await;
    server.pending_request().await;
}

#[tokio::test]
async fn disabling_or_unpairing_drops_a_live_connection_and_blocks_the_next() {
    let server = Server::start();
    let (client_id, secret) = server.pair("TV").await;
    let uuid = server.f.host.paired.read().unwrap().clients[0].uuid.clone();
    let mut live = sign_in(&server, &client_id, &secret).await;
    server.api("/api/clients/update", json!({"uuid":uuid,"enabled":false}));
    assert_eq!(live.expect("error").await["code"], "client-disabled");
    live.closed().await;
    // A disabled device is told so when it signs in again.
    let (mut ws, nonce) = server.connect().await;
    ws.send(json!({"version":2,"type":"authenticate","clientId":client_id,"proof":proof(&secret, &nonce)}))
        .await;
    assert_eq!(ws.expect("error").await["code"], "client-disabled");
    // Enabled again, it signs in; unpaired, it is dropped and refused.
    server.api("/api/clients/update", json!({"uuid":uuid,"enabled":true}));
    let mut live = sign_in(&server, &client_id, &secret).await;
    server.api("/api/clients/unpair", json!({"uuid":uuid}));
    assert_eq!(live.expect("error").await["code"], "authentication-failed");
    live.closed().await;
    let (mut ws, nonce) = server.connect().await;
    ws.send(json!({"version":2,"type":"authenticate","clientId":client_id,"proof":proof(&secret, &nonce)}))
        .await;
    assert_eq!(ws.expect("error").await["code"], "authentication-failed");
}

#[tokio::test]
async fn the_console_cannot_overwrite_a_devices_credentials() {
    let server = Server::start();
    let (client_id, secret) = server.pair("TV").await;
    let uuid = server.f.host.paired.read().unwrap().clients[0].uuid.clone();
    server.api(
        "/api/clients/update",
        json!({"uuid":uuid,"webrtc":{"clientId":"ab".repeat(16),"secret":"cd".repeat(32)}}),
    );
    server.api("/api/clients/update", json!({"uuid":uuid,"webrtc":null}));
    assert_eq!(
        server.f.host.paired.read().unwrap().clients[0].webrtc(),
        Some((client_id.as_str(), secret.as_str()))
    );
}

#[tokio::test]
async fn a_fifth_unauthenticated_connection_from_one_peer_pushes_out_its_oldest() {
    let server = Server::start();
    let mut sockets = Vec::new();
    for _ in 0..4 {
        sockets.push(server.connect().await.0);
    }
    let mut fifth = server.connect().await.0;
    sockets[0].closed().await;
    // The others stay open: a probe connection may idle.
    fifth.send(json!({"version":2,"type":"get-apps"})).await;
    assert_eq!(fifth.expect("error").await["code"], "not-authenticated");
    sockets[1]
        .send(json!({"version":2,"type":"get-apps"}))
        .await;
    assert_eq!(
        sockets[1].expect("error").await["code"],
        "not-authenticated"
    );
}

fn short(handshake: u64, unauthenticated: u64, pairing: u64) -> Limits {
    Limits {
        handshake: Duration::from_millis(handshake),
        unauthenticated: Duration::from_millis(unauthenticated),
        pairing: Duration::from_millis(pairing),
    }
}

#[tokio::test]
async fn a_tcp_connection_that_sends_nothing_is_closed() {
    let server = Server::start_with(short(300, 60_000, 60_000));
    let mut silent = TcpStream::connect(server.address).await.unwrap();
    let mut byte = [0u8; 1];
    let read = tokio::time::timeout(Duration::from_secs(5), silent.read(&mut byte))
        .await
        .expect("the server closed the idle connection");
    assert!(matches!(read, Ok(0) | Err(_)));
    // A peer that starts a request and stalls is closed too.
    let mut slow = TcpStream::connect(server.address).await.unwrap();
    slow.write_all(b"GET / HTTP/1.1\r\nHost: te").await.unwrap();
    let read = tokio::time::timeout(Duration::from_secs(5), slow.read(&mut byte))
        .await
        .expect("the server closed the stalled request");
    assert!(matches!(read, Ok(0) | Err(_)));
}

#[tokio::test]
async fn an_unauthenticated_websocket_that_stays_silent_is_closed() {
    let server = Server::start_with(short(10_000, 400, 60_000));
    let (mut ws, _) = server.connect().await;
    ws.closed().await;
}

#[tokio::test]
async fn a_paired_connection_outlives_the_handshake_and_idle_limits() {
    // Both limits are far shorter than the wait below.
    let server = Server::start_with(short(300, 300, 60_000));
    let (client_id, secret) = server.pair("TV").await;
    let mut ws = sign_in(&server, &client_id, &secret).await;
    tokio::time::sleep(Duration::from_millis(900)).await;
    ws.send(json!({"version":2,"type":"get-apps"})).await;
    ws.expect("apps").await;
}

#[tokio::test]
async fn a_pairing_request_keeps_its_socket_until_it_expires_and_then_says_so() {
    let server = Server::start_with(short(10_000, 200, 700));
    let (mut ws, _) = server.connect().await;
    ws.send(json!({"version":2,"type":"request-pairing","pin":"4321","clientName":"TV"}))
        .await;
    server.pending_request().await;
    // Past the idle limit, the request holds the socket open.
    let error = ws.expect("error").await;
    assert_eq!(error["code"], "pairing-expired");
    assert_eq!(error["requestType"], "request-pairing");
    assert!(server.f.host.pins.lock().unwrap().is_empty());
    ws.closed().await;
}

#[tokio::test]
async fn pairing_turned_off_refuses_a_request_without_a_console_entry_or_notice() {
    let server = Server::start();
    server
        .f
        .host
        .config
        .write()
        .unwrap()
        .values
        .insert("enable_pairing".into(), "false".into());
    let (mut ws, _) = server.connect().await;
    ws.send(json!({"version":2,"type":"request-pairing","pin":"4321","clientName":"TV"}))
        .await;
    let error = ws.expect("error").await;
    assert_eq!(error["code"], "pairing-failed");
    assert!(server.f.host.pins.lock().unwrap().is_empty());
    // No notification was raised.
    let clients = WebRtcRuntime::for_host(&server.f.host).clients;
    assert_eq!(clients.notices(), 0);
    assert!(server.f.host.paired.read().unwrap().clients.is_empty());
    // The connection is still usable, and pairing works again once it is on.
    server
        .f
        .host
        .config
        .write()
        .unwrap()
        .values
        .insert("enable_pairing".into(), "true".into());
    ws.send(json!({"version":2,"type":"request-pairing","pin":"4321","clientName":"TV"}))
        .await;
    server.pending_request().await;
}

#[tokio::test]
async fn asking_again_does_not_raise_another_notice() {
    let server = Server::start();
    // No throttle window, so only the connection's own `again` check can
    // hold back the second notice.
    let clients = WebRtcRuntime::for_host(&server.f.host).clients;
    clients.set_notify_every(Duration::ZERO);
    let (mut ws, _) = server.connect().await;
    ws.send(json!({"version":2,"type":"request-pairing","pin":"4321","clientName":"TV"}))
        .await;
    server.pending_request().await;
    assert_eq!(clients.notices(), 1);
    ws.send(json!({"version":2,"type":"request-pairing","pin":"8765","clientName":"TV"}))
        .await;
    ws.send(json!({"version":2,"type":"get-apps"})).await;
    assert_eq!(ws.expect("error").await["code"], "not-authenticated");
    assert_eq!(server.f.host.pins.lock().unwrap().len(), 1);
    assert_eq!(clients.notices(), 1);
}

#[tokio::test]
async fn a_sixth_pairing_request_in_a_minute_is_refused_with_no_console_entry() {
    let server = Server::start();
    let (mut ws, _) = server.connect().await;
    for n in 0..MAX_PAIRING_REQUESTS {
        ws.send(json!({"version":2,"type":"request-pairing","pin":format!("{:04}", 1000 + n),"clientName":"TV"}))
            .await;
    }
    ws.send(json!({"version":2,"type":"request-pairing","pin":"9999","clientName":"TV"}))
        .await;
    let refused = ws.expect("error").await;
    assert_eq!(refused["code"], "pairing-failed");
    assert_eq!(refused["requestType"], "request-pairing");
    assert!(
        refused["message"]
            .as_str()
            .unwrap()
            .contains("wait a minute")
    );
    // Each request replaced the one before: the fifth waits.
    let request = server.pending_request().await;
    // A new socket of the same peer is refused too; the request already waiting still works.
    let (mut again, _) = server.connect().await;
    again
        .send(json!({"version":2,"type":"request-pairing","pin":"4321","clientName":"TV"}))
        .await;
    assert_eq!(again.expect("error").await["code"], "pairing-failed");
    let pins = server
        .f
        .host
        .pins
        .lock()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(pins, std::slice::from_ref(&request));
    update_clients(
        &server.f.host,
        "POST",
        "/api/pin",
        &json!({"pin":"1004","uniqueid":request}),
    )
    .unwrap();
    ws.expect("paired").await;
}

#[tokio::test]
async fn a_message_over_256_kib_closes_the_connection() {
    let server = Server::start();
    let (mut ws, _) = server.connect().await;
    // Just under the limit is read, and answered as the bad message it is.
    ws.send_text(vec![b' '; MAX_MESSAGE - 16]).await;
    assert_eq!(ws.expect("error").await["code"], "invalid-json");
    ws.send_text(vec![b' '; MAX_MESSAGE + 1]).await;
    ws.closed().await;
}

#[tokio::test]
async fn the_first_device_of_either_kind_gets_everything_and_a_later_tv_view_only() {
    let server = Server::start();
    let (first, _) = server.pair("First TV").await;
    let (second, _) = server.pair("Second TV").await;
    let perm = |id: &str| {
        server
            .f
            .host
            .paired
            .read()
            .unwrap()
            .webrtc_client(id)
            .unwrap()
            .perm
    };
    assert_eq!(
        perm(&first),
        butterpollo_session::state::FIRST_DEVICE_PERMISSION
    );
    assert_eq!(
        perm(&second),
        butterpollo_session::state::LATER_DEVICE_PERMISSION
    );
    // Behind a GameStream phone, a TV is a later device too.
    let phone = Server::start();
    phone
        .f
        .client(butterpollo_session::state::FIRST_DEVICE_PERMISSION);
    let (tv, secret) = phone.pair("TV").await;
    assert_eq!(
        phone
            .f
            .host
            .paired
            .read()
            .unwrap()
            .webrtc_client(&tv)
            .unwrap()
            .perm,
        butterpollo_session::state::LATER_DEVICE_PERMISSION
    );
    // Viewing the library is part of view-only.
    let mut ws = sign_in(&phone, &tv, &secret).await;
    ws.send(json!({"version":2,"type":"get-apps"})).await;
    ws.expect("apps").await;
}

#[tokio::test]
async fn a_second_connection_from_the_same_device_replaces_the_first() {
    let server = Server::start();
    let (client_id, secret) = server.pair("TV").await;
    let mut first = sign_in(&server, &client_id, &secret).await;
    let mut second = sign_in(&server, &client_id, &secret).await;
    first.closed().await;
    assert_eq!(first.close_code, Some(REPLACED));
    second.send(json!({"version":2,"type":"get-apps"})).await;
    second.expect("apps").await;
    let clients = WebRtcRuntime::for_host(&server.f.host).clients;
    assert_eq!(clients.connections(), 1);
}

#[tokio::test]
async fn a_server_without_a_stream_pipeline_refuses_session_requests_and_bad_messages_get_errors() {
    let server = Server::start();
    let (client_id, secret) = server.pair("TV").await;
    let mut ws = sign_in(&server, &client_id, &secret).await;
    ws.send(json!({"version":2,"type":"start-session","appId":"0","video":{"width":1920,"height":1080,"fps":60,"codec":"hevc","bitrateKbps":20000,"hdr":false},"audio":{"channels":2}}))
        .await;
    let error = ws.expect("error").await;
    assert_eq!(error["code"], "internal-error");
    assert_eq!(error["requestType"], "start-session");
    ws.send(json!({"version":2,"type":"stop-host-session"}))
        .await;
    let error = ws.expect("error").await;
    assert_eq!(error["code"], "internal-error");
    assert_eq!(error["requestType"], "stop-host-session");
    // Nothing is running, so stopping reports idle.
    ws.send(json!({"version":2,"type":"stop-session"})).await;
    assert_eq!(ws.expect("session-status").await["state"], "idle");
    ws.send(json!({"version":2,"type":"no-such-message"})).await;
    ws.expect("error").await;
    ws.send(json!({"version":2,"type":"get-app-artwork","appId":"999999"}))
        .await;
    let artwork = ws.expect("app-artwork").await;
    assert_eq!(artwork["available"], false);
    // The connection still works.
    ws.send(json!({"version":2,"type":"get-apps"})).await;
    ws.expect("apps").await;
}

#[tokio::test]
async fn an_unanswered_pairing_request_leaves_the_console_when_the_tv_goes() {
    let server = Server::start();
    let (mut ws, _) = server.connect().await;
    ws.send(json!({"version":2,"type":"request-pairing","pin":"4321","clientName":"TV"}))
        .await;
    server.pending_request().await;
    drop(ws);
    for _ in 0..200 {
        if server.f.host.pins.lock().unwrap().is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the request stayed in the console");
}
