//! Local `/play` portal, WAN rejection, browser pairing, play WebSocket, and internet invite tests.
use super::*;

/// The play page's build beside the console's and the guest page's.
fn install_play(f: &Fixture) {
    install_pages(f);
    let root = &f.host.assets;
    std::fs::create_dir_all(root.join("play/assets")).unwrap();
    std::fs::write(root.join("play/index.html"), "<p>play page</p>").unwrap();
    std::fs::write(root.join("play/assets/play-1.js"), "export {}").unwrap();
}

/// The headers the play page sends with its socket's upgrade.
fn play_upgrade() -> Vec<(&'static str, String)> {
    without("sec-websocket-protocol", Some("rubylight.play.v2"))
}

/// Peers of the local network: the LAN, Tailscale, a unique-local IPv6 address and this PC.
const LOCAL: [&str; 4] = [
    LAN,
    "100.101.102.103:50000",
    "[fd00::5]:50000",
    "127.0.0.1:50000",
];

#[tokio::test]
async fn the_play_page_is_its_own_build_for_the_local_network_and_the_guest_gate_stays() {
    let f = Fixture::new();
    install_play(&f);
    // Off with the WebRTC server.
    for path in ["/play/", "/play/signal"] {
        let off = guest(&f, LAN, "GET", path, &play_upgrade()).await;
        assert_eq!(off.status, StatusCode::NOT_FOUND, "{path}");
        assert!(!off.text().contains("play page"));
    }
    configure(&f, json!({"webrtc_server": true}));
    for peer in LOCAL {
        for path in [
            "/play",
            "/play/",
            "/play/index.html",
            "/play/library",
            "/play/../index.html",
            "/play/%2e%2e/index.html",
            "/play/..%5cindex.html",
            "/play/api/config",
            "/play/guest/",
        ] {
            let page = guest(&f, peer, "GET", path, &[]).await;
            assert_eq!(page.status, StatusCode::OK, "{peer} {path}");
            assert_eq!(page.text(), "<p>play page</p>", "{peer} {path}");
            assert_eq!(page.headers[header::CONTENT_SECURITY_POLICY], GUEST_POLICY);
            assert_eq!(page.headers["x-frame-options"], "DENY");
            assert_eq!(page.headers[header::REFERRER_POLICY], "no-referrer");
        }
        let script = guest(&f, peer, "GET", "/play/assets/play-1.js", &[]).await;
        assert_eq!(script.status, StatusCode::OK);
        for path in [
            "/play/assets/guest-1.js",
            "/play/assets/console-1.js",
            "/play/assets/../../index.html",
            "/play/assets/..%5c..%5cguest%5cassets%5cguest-1.js",
        ] {
            let gone = guest(&f, peer, "GET", path, &[]).await;
            assert_eq!(gone.status, StatusCode::NOT_FOUND, "{peer} {path}");
        }
        let posted = guest(&f, peer, "POST", "/play/", &[]).await;
        assert_eq!(posted.status, StatusCode::METHOD_NOT_ALLOWED);
        // The guest gate is as before: with no link, nothing else answers, the console least.
        for path in [
            "/",
            "/index.html",
            "/guest/",
            "/guest/signal",
            "/api/config",
            "/playground",
            "/stream",
        ] {
            let closed = guest(&f, peer, "GET", path, &[]).await;
            assert_eq!(closed.status, StatusCode::NOT_FOUND, "{peer} {path}");
            assert!(!closed.text().contains("page"), "{peer} {path}");
        }
    }
    // Once a link is live, the guest routes are the guest page's alone.
    open_guests(&f);
    assert_eq!(
        guest(&f, LAN, "GET", "/guest/", &[]).await.text(),
        "<p>guest page</p>"
    );
    assert_eq!(
        guest(&f, LAN, "GET", "/play/", &[]).await.text(),
        "<p>play page</p>"
    );
}

#[tokio::test]
async fn an_internet_peer_gets_no_play_page_or_socket_whatever_its_headers_say() {
    let f = Fixture::new();
    configure(&f, json!({"webrtc_server": true}));
    install_play(&f);
    // A live link opens the guest port to the internet, never /play.
    open_guests(&f);
    let spoofed: [Vec<(&str, String)>; 5] = [
        vec![],
        vec![("x-forwarded-for", "192.168.4.20".into())],
        vec![("x-real-ip", "127.0.0.1".into())],
        vec![("forwarded", "for=192.168.4.20;proto=https".into())],
        vec![
            ("x-forwarded-for", "127.0.0.1, 192.168.4.20".into()),
            ("x-client-ip", "192.168.4.20".into()),
            ("true-client-ip", "192.168.4.20".into()),
        ],
    ];
    // A router forwarding TCP 47991 hands the internet peer's address to the LAN socket as it is.
    // One that source-NATs its forwards hands over its own, the default gateway; a neighbour
    // behind the ISP's carrier-grade NAT arrives from 100.64/10 on the LAN address.
    let gateway = format!("{}:50000", butterpollo_stream::network::TEST_GATEWAY);
    let peers = [
        (WAN, "192.168.4.69:47991"),
        ("[2001:db8::7]:50000", "192.168.4.69:47991"),
        ("8.8.8.8:443", "192.168.4.69:47991"),
        (gateway.as_str(), "192.168.4.69:47991"),
        ("100.101.102.103:50000", "192.168.4.69:47991"),
        ("100.64.0.9:50000", "[::ffff:192.168.4.69]:47991"),
    ];
    // The guest port's answer to a path it does not serve: what the internet gets for /play too.
    let unserved = guest(&f, WAN, "GET", "/nothing", &[]).await;
    assert_eq!(unserved.status, StatusCode::NOT_FOUND);
    for (peer, local) in peers {
        for headers in &spoofed {
            let mut upgrade = play_upgrade();
            upgrade.extend(headers.iter().cloned());
            for (method, path, headers) in [
                ("GET", "/play", headers),
                ("GET", "/play/", headers),
                ("GET", "/play/index.html", headers),
                ("GET", "/play/assets/play-1.js", headers),
                ("POST", "/play/signal", headers),
                ("GET", "/play/signal", &upgrade),
            ] {
                let refused = send_on(
                    guest_router(with_webrtc(&f)),
                    connection_to(peer, local),
                    method,
                    path,
                    headers,
                    None,
                )
                .await;
                assert_eq!(
                    (refused.status, refused.text()),
                    (unserved.status, unserved.text()),
                    "{peer} on {local} {method} {path} {headers:?}"
                );
            }
        }
    }
    // Real Tailscale traffic arrives on the PC's Tailscale address.
    let tailnet = guest_on(&f, "100.101.102.103:50000", TAILNET_HOST, "GET", "/play/").await;
    assert_eq!(tailnet.text(), "<p>play page</p>");
    // A LAN peer is not refused for a header naming the internet either.
    let page = guest(
        &f,
        LAN,
        "GET",
        "/play/",
        &[("x-forwarded-for", "203.0.113.7".into())],
    )
    .await;
    assert_eq!(page.text(), "<p>play page</p>");
    // The guest page still answers the internet with its link.
    assert_eq!(
        guest(&f, WAN, "GET", "/guest/", &[]).await.text(),
        "<p>guest page</p>"
    );
}

#[tokio::test]
async fn with_no_link_open_the_internet_gets_the_closed_guest_port_for_play_too() {
    let f = Fixture::new();
    configure(&f, json!({"webrtc_server": true}));
    install_play(&f);
    let closed = guest(&f, WAN, "GET", "/", &[]).await;
    assert_eq!(closed.status, StatusCode::NOT_FOUND);
    for (peer, local) in [
        (WAN, "192.168.4.69:47991"),
        ("100.101.102.103:50000", "192.168.4.69:47991"),
        ("192.168.4.1:50000", "192.168.4.69:47991"),
    ] {
        for path in ["/play", "/play/", "/play/signal"] {
            let refused = guest_on(&f, peer, local, "GET", path).await;
            assert_eq!(
                (refused.status, refused.text()),
                (closed.status, closed.text()),
                "{peer} {path}"
            );
        }
    }
    assert_eq!(
        guest(&f, LAN, "GET", "/play/", &[]).await.text(),
        "<p>play page</p>"
    );
}

#[tokio::test]
async fn a_play_pairing_request_may_not_carry_its_pin_in_the_name_and_shows_its_address() {
    let f = Fixture::new();
    configure(&f, json!({"webrtc_server": true}));
    let address = serve(&f).await;
    let (mut ws, _) = play_socket(address, "").await;
    // More refusals than the five requests a minute a device may make: a name to fix uses none.
    for (name, message) in [
        ("Living room TV - PIN 4821", "PIN"),
        ("4821", "PIN"),
        ("TV 4-8-2-1", "PIN"),
        ("TV \u{ff14}\u{ff18}\u{ff12}\u{ff11}", "0-9"),
        ("Xbox, PIN ④⑧②①", "0-9"),
        ("TV \u{0664}\u{0668}\u{0662}\u{0661}", "0-9"),
        ("TV \u{1d7d2}\u{1d7d6}\u{1d7d0}\u{1d7cf}", "0-9"),
    ] {
        ws.send(
            json!({"version": 2, "type": "request-pairing", "pin": "4821", "clientName": name}),
        )
        .await;
        let refused = ws.expect("error").await;
        assert_eq!(refused["code"], "invalid-message", "{name}");
        assert!(
            refused["message"].as_str().unwrap().contains(message),
            "{refused}"
        );
    }
    let pending =
        butterpollo_web::web::update_clients(&f.host, "GET", "/api/clients/pending", &Value::Null)
            .unwrap();
    assert_eq!(pending["requests"], json!([]));
    // A name with other digits pairs; the console lists it with the address it came from.
    ws.send(json!({"version": 2, "type": "request-pairing", "pin": "4821", "clientName": "Living room TV 2024"}))
        .await;
    let mut request = Value::Null;
    for _ in 0..200 {
        let pending = butterpollo_web::web::update_clients(
            &f.host,
            "GET",
            "/api/clients/pending",
            &Value::Null,
        )
        .unwrap();
        if let Some(first) = pending["requests"].as_array().and_then(|r| r.first()) {
            request = first.clone();
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(request["name"], "Living room TV 2024", "{request}");
    assert_eq!(request["address"], "127.0.0.1", "{request}");
}

#[test]
fn a_pairing_notice_shows_the_address_apart_from_a_name_that_could_hide_or_fake_it() {
    let lan = "192.168.4.20".parse().unwrap();
    assert_eq!(
        butterpollo_stream::state::pairing_notice("Living room TV 2024", lan),
        (
            "Pairing request from 192.168.4.20".into(),
            "Living room TV 2024 wants to pair. Enter the PIN it shows in the Rubylight console."
                .into()
        )
    );
    // 128 math bold letters are 256 UTF-16 units, more than Windows keeps of a notice's text.
    let long: String = std::iter::repeat_n('\u{1d400}', 128).collect();
    butterpollo_crypto::pairing::validate_device_name(&long).unwrap();
    let (title, text) = butterpollo_stream::state::pairing_notice(&long, lan);
    assert_eq!(title, "Pairing request from 192.168.4.20");
    assert!(text.encode_utf16().count() < 256, "{text}");
    assert!(text.starts_with(&format!("{}…", "\u{1d400}".repeat(40))));
    // A name that names an address does not change the one in the title.
    let (title, text) = butterpollo_stream::state::pairing_notice(
        "Living room TV (from 192.168.4.20)",
        "::1".parse().unwrap(),
    );
    assert_eq!(title, "Pairing request from this PC (a tunnel or proxy)");
    assert!(!title.contains("192.168"), "{title}");
    assert!(text.starts_with("Living room TV (from 192.168.4.20) wants"));
    // The longest address still fits Windows' 63-unit title.
    let (title, _) = butterpollo_stream::state::pairing_notice(
        "TV",
        "fd12:3456:789a:bcde:f012:3456:789a:bcde".parse().unwrap(),
    );
    assert!(title.encode_utf16().count() <= 63, "{title}");
    // A request from the internet asks the owner to expect it; the local network's does not.
    for address in ["203.0.113.7", "2001:db8::7", "127.77.3.4"] {
        let (_, text) =
            butterpollo_stream::state::pairing_notice("Phone", address.parse().unwrap());
        assert_eq!(
            text,
            "Phone wants to pair from the internet. Enter its PIN in the Rubylight console only if you expect it.",
            "{address}"
        );
    }
    for address in ["192.168.4.20", "100.101.102.103", "fd12::7", "::1"] {
        let (_, text) =
            butterpollo_stream::state::pairing_notice("Phone", address.parse().unwrap());
        assert!(!text.contains("internet"), "{address}: {text}");
    }
}

#[tokio::test]
async fn the_play_socket_needs_its_page_origin_and_subprotocol() {
    let f = Fixture::new();
    configure(&f, json!({"webrtc_server": true}));
    let with = |name: &'static str, value: Option<&str>| {
        let mut headers = play_upgrade();
        headers.retain(|(n, _)| *n != name);
        if let Some(value) = value {
            headers.push((name, value.to_owned()));
        }
        headers
    };
    for (headers, status) in [
        (with("origin", None), StatusCode::FORBIDDEN),
        (
            with("origin", Some("https://evil.example")),
            StatusCode::FORBIDDEN,
        ),
        (with("origin", Some("http://test")), StatusCode::FORBIDDEN),
        (
            with("sec-websocket-protocol", None),
            StatusCode::BAD_REQUEST,
        ),
        (
            with("sec-websocket-protocol", Some("rubylight.guest.v1")),
            StatusCode::BAD_REQUEST,
        ),
        (
            with(
                "sec-websocket-protocol",
                Some("rubylight.signal.v2, csrf.x"),
            ),
            StatusCode::BAD_REQUEST,
        ),
        (play_upgrade(), StatusCode::UPGRADE_REQUIRED),
    ] {
        let reply = guest(&f, LAN, "GET", "/play/signal", &headers).await;
        assert_eq!(reply.status, status, "{headers:?}: {}", reply.text());
    }
}

async fn play_socket(address: SocketAddr, extra: &str) -> (Ws, String) {
    let mut ws = Ws::connect_with(
        address,
        "/play/signal",
        &format!("Origin: https://test\r\nSec-WebSocket-Protocol: rubylight.play.v2\r\n{extra}"),
    )
    .await;
    assert!(
        ws.head
            .to_ascii_lowercase()
            .contains("\r\nsec-websocket-protocol: rubylight.play.v2\r\n"),
        "{}",
        ws.head
    );
    let nonce = ws.expect("auth-required").await["nonce"]
        .as_str()
        .unwrap()
        .to_owned();
    (ws, nonce)
}

fn proof(secret: &str, nonce: &str) -> String {
    butterpollo_session::webrtc::auth::proof_hex(secret, nonce).unwrap()
}

#[tokio::test]
async fn an_unpaired_play_socket_can_only_pair_and_unpairing_ends_its_access_at_once() {
    let f = Fixture::new();
    configure(&f, json!({"webrtc_server": true}));
    let address = serve(&f).await;
    let (cookie, _) = sign_in(&f);
    // Before pairing, even with a console sign-in in the request, nothing but pairing works.
    let (mut ws, nonce) = play_socket(address, &format!("Cookie: {cookie}\r\n")).await;
    for message in [
        json!({"version": 2, "type": "get-apps"}),
        json!({"version": 2, "type": "get-app-artwork", "appId": "1"}),
        json!({"version": 2, "type": "start-session", "appId": "1", "video": {"width": 1920, "height": 1080, "fps": 60, "codec": "h264", "bitrateKbps": 20000, "hdr": false}, "audio": {"channels": 2}}),
        json!({"version": 2, "type": "stop-host-session"}),
        json!({"version": 2, "type": "answer", "sessionId": 1, "sdp": "v=0"}),
    ] {
        ws.send(message.clone()).await;
        let refused = ws.expect("error").await;
        assert_eq!(refused["code"], "not-authenticated", "{message}");
    }
    // A guest's join is no message here.
    ws.send(json!({"type": "join", "token": "0".repeat(32)}))
        .await;
    assert_ne!(ws.expect("error").await["code"], "not-authenticated");
    ws.send(json!({"version": 2, "type": "authenticate", "clientId": "ab".repeat(16), "proof": proof(&"cd".repeat(32), &nonce)}))
        .await;
    assert_eq!(ws.expect("error").await["code"], "authentication-failed");
    // The console confirms the PIN the page shows.
    ws.send(json!({"version": 2, "type": "request-pairing", "pin": "2468", "clientName": "Xbox browser"}))
        .await;
    let mut request = Value::Null;
    for _ in 0..200 {
        let pending = butterpollo_web::web::update_clients(
            &f.host,
            "GET",
            "/api/clients/pending",
            &Value::Null,
        )
        .unwrap();
        if let Some(first) = pending["requests"].as_array().and_then(|r| r.first()) {
            request = first.clone();
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(request["webrtc"], true, "{request}");
    assert_eq!(request["name"], "Xbox browser");
    butterpollo_web::web::update_clients(
        &f.host,
        "POST",
        "/api/pin",
        &json!({"pin": "2468", "uniqueid": request["uniqueid"]}),
    )
    .unwrap();
    let paired = ws.expect("paired").await;
    ws.expect("gateway-status").await;
    ws.expect("capabilities").await;
    let client_id = paired["clientId"].as_str().unwrap().to_owned();
    let secret = paired["clientSecret"].as_str().unwrap().to_owned();
    // Paired, it gets what a TV gets: the library (this test host has no stream pipeline).
    ws.send(json!({"version": 2, "type": "get-apps"})).await;
    ws.expect("apps").await;
    drop(ws);
    // It signs in again with its secret; unpaired in the console, it is dropped at once.
    let (mut ws, nonce) = play_socket(address, "").await;
    ws.send(json!({"version": 2, "type": "authenticate", "clientId": client_id, "proof": proof(&secret, &nonce)}))
        .await;
    ws.expect("authenticated").await;
    ws.expect("gateway-status").await;
    ws.expect("capabilities").await;
    let uuid = f.host.paired.read().unwrap().clients[0].uuid.clone();
    butterpollo_web::web::update_clients(
        &f.host,
        "POST",
        "/api/clients/unpair",
        &json!({"uuid": uuid}),
    )
    .unwrap();
    assert_eq!(ws.expect("error").await["code"], "authentication-failed");
    ws.closed().await;
    let (mut ws, nonce) = play_socket(address, "").await;
    ws.send(json!({"version": 2, "type": "authenticate", "clientId": client_id, "proof": proof(&secret, &nonce)}))
        .await;
    assert_eq!(ws.expect("error").await["code"], "authentication-failed");
    ws.send(json!({"version": 2, "type": "get-apps"})).await;
    assert_eq!(ws.expect("error").await["code"], "not-authenticated");
}

#[tokio::test]
async fn an_internet_invite_asks_for_the_guest_ports_and_says_what_the_router_did() {
    let f = Fixture::new();
    let (cookie, csrf) = sign_in(&f);
    let headers = console(&cookie, &csrf);
    // Signed out, nothing.
    let anonymous = call(&f, LAN, "GET", "/api/guests/internet", &[], None).await;
    assert_ne!(anonymous.status, StatusCode::OK);
    let before = WebRtcRuntime::for_host(&f.host).guests.internet_round();
    let asked = call(
        &f,
        LAN,
        "POST",
        "/api/guests/internet",
        &headers,
        Some(json!({})),
    )
    .await;
    assert_eq!(asked.status, StatusCode::OK, "{}", asked.text());
    assert_eq!(
        WebRtcRuntime::for_host(&f.host).guests.internet_round(),
        before + 1
    );
    assert!(WebRtcRuntime::for_host(&f.host).guests.internet_wanted());
    let reach = asked.json();
    for key in [
        "checking",
        "runs",
        "method",
        "error",
        "ports",
        "publicIpv4",
        "routerIpv4",
        "ipv6",
        "address",
        "notes",
        "internetPlay",
        "guestJoin",
    ] {
        assert!(reach.get(key).is_some(), "{key}: {reach}");
    }
    assert_eq!(reach["guestJoin"], "off");
    let read = call(&f, LAN, "GET", "/api/guests/internet", &headers, None).await;
    assert_eq!(read.status, StatusCode::OK, "{}", read.text());
    assert_eq!(
        WebRtcRuntime::for_host(&f.host).guests.internet_round(),
        before + 1
    );

    let made = invite(
        &f,
        &cookie,
        &csrf,
        json!({"permission": "view", "seconds": 300, "address": "203.0.113.5:47991", "internet": true}),
    )
    .await;
    assert_eq!(made.status, StatusCode::OK, "{}", made.text());
    assert_eq!(made.json()["invite"]["internet"], true);
    let local = invite(
        &f,
        &cookie,
        &csrf,
        json!({"permission": "view", "seconds": 300}),
    )
    .await;
    assert_eq!(local.json()["invite"]["internet"], false);
    let wrong = invite(
        &f,
        &cookie,
        &csrf,
        json!({"permission": "view", "seconds": 300, "internet": "yes"}),
    )
    .await;
    assert_eq!(wrong.status, StatusCode::BAD_REQUEST);
    let listed = call(&f, LAN, "GET", "/api/guests", &headers, None).await;
    let invites = listed.json()["invites"].as_array().unwrap().clone();
    assert_eq!(invites.iter().filter(|i| i["internet"] == true).count(), 1);
    // Each guest seat's media port, for the dialog to name.
    assert_eq!(
        listed.json()["media_ports"].as_array().unwrap().len(),
        usize::from(MAX_GUESTS)
    );
}
