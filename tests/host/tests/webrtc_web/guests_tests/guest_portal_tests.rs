//! Console guest panel, guest port gate, guest page build, and guest WebSocket tests.
use super::*;

#[tokio::test]
async fn only_a_signed_in_console_tab_reaches_the_guest_panel() {
    let f = Fixture::new();
    let (cookie, csrf) = sign_in(&f);
    let body = json!({"permission": "controller", "seconds": 3600});

    let anonymous = call(&f, LAN, "GET", "/api/guests", &[], None).await;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);
    let anonymous = call(
        &f,
        LAN,
        "POST",
        "/api/guests/invite",
        &[],
        Some(body.clone()),
    )
    .await;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);
    // An API token is automation: the panel is not in its catalogue.
    assert!(
        !token_catalog()
            .iter()
            .any(|scope| scope.path.starts_with("/api/guests"))
    );
    let bearer = call(
        &f,
        LAN,
        "GET",
        "/api/guests",
        &[("authorization", "Bearer 0123456789abcdef".into())],
        None,
    )
    .await;
    assert_eq!(bearer.status, StatusCode::UNAUTHORIZED);

    // Signed in, a POST needs the session's CSRF token and the console's own origin.
    let mut headers = console(&cookie, &csrf);
    headers.retain(|(name, _)| *name != "x-csrf-token");
    let forged = call(
        &f,
        LAN,
        "POST",
        "/api/guests/invite",
        &headers,
        Some(body.clone()),
    )
    .await;
    assert_eq!(forged.status, StatusCode::BAD_REQUEST);
    assert_eq!(forged.error(), "CSRF token required");
    let mut headers = console(&cookie, &csrf);
    headers.retain(|(name, _)| *name != "origin");
    headers.push(("origin", "https://evil.example".into()));
    let cross = call(
        &f,
        LAN,
        "POST",
        "/api/guests/invite",
        &headers,
        Some(body.clone()),
    )
    .await;
    assert_eq!(cross.status, StatusCode::FORBIDDEN);
    assert!(
        WebRtcRuntime::for_host(&f.host)
            .guests
            .status()
            .invites
            .is_empty()
    );

    let made = invite(&f, &cookie, &csrf, body).await;
    assert_eq!(made.status, StatusCode::OK, "{}", made.text());
    let token = made.json()["token"].as_str().unwrap().to_owned();
    assert_eq!(token.len(), 32);
    let listed = call(&f, LAN, "GET", "/api/guests", &[("cookie", cookie)], None).await;
    assert_eq!(listed.status, StatusCode::OK);
    assert_eq!(listed.json()["invites"][0]["permission"], "controller");
    assert_eq!(listed.json()["streaming"], false);
    assert!(
        !listed.text().contains(&token),
        "the list never has a token"
    );
}

#[tokio::test]
async fn the_panel_makes_and_revokes_links_and_says_what_it_refuses() {
    let f = Fixture::new();
    let (cookie, csrf) = sign_in(&f);
    for (body, status) in [
        (
            json!({"permission": "admin", "seconds": 3600}),
            StatusCode::BAD_REQUEST,
        ),
        (json!({"seconds": 3600}), StatusCode::BAD_REQUEST),
        (json!({"permission": "view"}), StatusCode::BAD_REQUEST),
        (
            json!({"permission": "view", "seconds": 3600, "address": "https://home"}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"permission": "view", "seconds": 3600, "address": "home/x:1"}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"permission": "view", "seconds": 3600, "address": 7}),
            StatusCode::BAD_REQUEST,
        ),
        // Five minutes to seven days.
        (
            json!({"permission": "view", "seconds": 60}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"permission": "view", "seconds": 8 * 86_400}),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let refused = invite(&f, &cookie, &csrf, body.clone()).await;
        assert_eq!(refused.status, status, "{body}");
        assert!(!refused.error().is_empty(), "{body}");
    }
    let mut ids = Vec::new();
    let addresses = [
        json!(null),
        json!("home.example.org"),
        json!("203.0.113.9:47991"),
        json!("[2001:db8::1]:47991"),
    ];
    for n in 0..butterpollo_session::webrtc::guests::MAX_INVITES {
        let made = invite(
            &f,
            &cookie,
            &csrf,
            json!({"permission": "view", "seconds": 300, "address": addresses[n % 4]}),
        )
        .await;
        assert_eq!(made.status, StatusCode::OK, "{}", made.text());
        assert_eq!(made.json()["invite"]["permission"], "view");
        ids.push(made.json()["invite"]["id"].as_str().unwrap().to_owned());
    }
    let full = invite(
        &f,
        &cookie,
        &csrf,
        json!({"permission": "view", "seconds": 300}),
    )
    .await;
    assert_eq!(full.status, StatusCode::CONFLICT);
    assert!(
        full.error().contains("revoke one first"),
        "{}",
        full.error()
    );

    let (host, headers) = (&f, console(&cookie, &csrf));
    let post = move |path: &'static str, body: Value| {
        let headers = headers.clone();
        async move { call(host, LAN, "POST", path, &headers, Some(body)).await }
    };
    let revoked = post("/api/guests/revoke", json!({"id": ids[0]})).await;
    assert_eq!(revoked.status, StatusCode::OK, "{}", revoked.text());
    assert!(
        WebRtcRuntime::for_host(&f.host)
            .guests
            .status()
            .invites
            .iter()
            .all(|i| i.id != ids[0])
    );
    let again = post("/api/guests/revoke", json!({"id": ids[0]})).await;
    assert_eq!(again.status, StatusCode::NOT_FOUND);
    // The panel builds links on the guest port, never the console's.
    let listed = call(
        &f,
        LAN,
        "GET",
        "/api/guests",
        &console(&cookie, &csrf),
        None,
    )
    .await;
    assert_eq!(listed.json()["port"], 47991, "{}", listed.text());
    let lan = call(
        &f,
        LAN,
        "GET",
        "/api/guests/addresses",
        &console(&cookie, &csrf),
        None,
    )
    .await;
    assert!(lan.json()["addresses"].is_array(), "{}", lan.text());
    assert_eq!(
        post("/api/guests/revoke", json!({})).await.status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post("/api/guests/kick", json!({"id": 7})).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        post("/api/guests/kick", json!({"id": "7"})).await.status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post("/api/guests/nothing", json!({})).await.status,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn the_guest_port_answers_only_while_a_link_is_live() {
    let f = Fixture::new();
    configure(&f, json!({"webrtc_server": true}));
    install_pages(&f);
    for path in ["/guest/", "/guest/index.html", "/guest/signal", "/"] {
        let closed = guest(&f, WAN, "GET", path, &guest_upgrade()).await;
        assert_eq!(closed.status, StatusCode::NOT_FOUND, "{path}");
        assert!(!closed.text().contains("guest page"), "{path}");
    }
    let id = open_guests(&f);
    assert_eq!(
        guest(&f, WAN, "GET", "/guest/", &[]).await.text(),
        "<p>guest page</p>"
    );
    // The page's tab icon, and the one a browser asks for itself (404 in 3.4.0).
    for path in ["/guest/favicon.svg", "/favicon.ico"] {
        let icon = guest(&f, WAN, "GET", path, &[]).await;
        assert_eq!(icon.status, StatusCode::OK, "{path}");
        assert_eq!(
            icon.headers[header::CONTENT_TYPE],
            "image/svg+xml",
            "{path}"
        );
        assert!(icon.text().starts_with("<svg"), "{path}");
    }
    WebRtcRuntime::for_host(&f.host).guests.revoke(&id);
    for path in ["/guest/", "/guest/favicon.svg", "/favicon.ico"] {
        assert_eq!(
            guest(&f, WAN, "GET", path, &[]).await.status,
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
}

#[tokio::test]
async fn the_guest_page_is_its_own_build_and_never_the_consoles() {
    let f = Fixture::new();
    open_guests(&f);
    let missing = guest(&f, WAN, "GET", "/guest/", &[]).await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    install_pages(&f);
    for path in [
        "/guest",
        "/guest/",
        "/guest/index.html",
        "/guest/join",
        "/guest/../index.html",
        "/guest/%2e%2e/index.html",
        "/guest/api/config",
        "/guest/..%5cindex.html",
    ] {
        let page = guest(&f, WAN, "GET", path, &[]).await;
        assert_eq!(page.status, StatusCode::OK, "{path}");
        assert_eq!(page.text(), "<p>guest page</p>", "{path}");
        assert_eq!(page.headers[header::CONTENT_SECURITY_POLICY], GUEST_POLICY);
        assert_eq!(page.headers["x-frame-options"], "DENY");
        assert_eq!(page.headers["x-content-type-options"], "nosniff");
        assert_eq!(page.headers[header::REFERRER_POLICY], "no-referrer");
        assert_eq!(page.headers[header::CACHE_CONTROL], "no-store");
    }
    let script = guest(&f, WAN, "GET", "/guest/assets/guest-1.js", &[]).await;
    assert_eq!(script.status, StatusCode::OK);
    assert!(
        script.headers[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/javascript")
    );
    let gone = guest(&f, WAN, "GET", "/guest/assets/console-1.js", &[]).await;
    assert_eq!(gone.status, StatusCode::NOT_FOUND);
    let posted = guest(&f, LAN, "POST", "/guest/", &[]).await;
    assert_eq!(posted.status, StatusCode::METHOD_NOT_ALLOWED);
    // Nothing but the guest routes, even from this PC: no console page, file or API.
    for path in [
        "/",
        "/index.html",
        "/stream",
        "/api/config",
        "/api/guests",
        "/pin",
        "/serverinfo",
    ] {
        for peer in [WAN, LAN, "127.0.0.1:50000"] {
            let other = guest(&f, peer, "GET", path, &[]).await;
            assert_eq!(other.status, StatusCode::NOT_FOUND, "{peer} {path}");
            assert!(other.text().is_empty(), "{peer} {path}: {}", other.text());
        }
    }
    // And the console's port no longer serves the guest page or its socket.
    let console_page = call(&f, LAN, "GET", "/guest/", &[], None).await;
    assert_ne!(console_page.text(), "<p>guest page</p>");
    // The guest page loads its own files and talks to this host only.
    assert!(GUEST_POLICY.contains("connect-src 'self';"));
    assert!(GUEST_POLICY.contains("script-src 'self';"));
    assert!(!GUEST_POLICY.contains("http"));
}

#[tokio::test]
async fn an_internet_guest_reaches_the_guest_routes_and_no_console_route() {
    let f = Fixture::new();
    configure(&f, json!({"webrtc_server": true}));
    install_pages(&f);
    let (cookie, csrf) = sign_in(&f);
    // The console stays on the LAN (origin_web_ui_allowed = lan), even with a sign-in.
    for (method, path) in [
        ("GET", "/"),
        ("GET", "/stream"),
        ("GET", "/api/guests"),
        ("GET", "/api/config"),
        ("GET", "/api/auth/status"),
        ("POST", "/api/guests/invite"),
    ] {
        let refused = call(&f, WAN, method, path, &console(&cookie, &csrf), None).await;
        assert_eq!(refused.status, StatusCode::FORBIDDEN, "{method} {path}");
    }
    open_guests(&f);
    let page = guest(&f, WAN, "GET", "/guest/", &[]).await;
    assert_eq!(page.text(), "<p>guest page</p>");
    // On the console's port a guest's upgrade is a console request, refused from the internet.
    let console_socket = call(&f, WAN, "GET", "/guest/signal", &guest_upgrade(), None).await;
    assert_eq!(console_socket.status, StatusCode::FORBIDDEN);
    // A oneshot request cannot be upgraded: one that passes every check ends in 426.
    let socket = guest(&f, WAN, "GET", "/guest/signal", &guest_upgrade()).await;
    assert_eq!(
        socket.status,
        StatusCode::UPGRADE_REQUIRED,
        "{}",
        socket.text()
    );
}

#[tokio::test]
async fn the_guest_socket_needs_its_page_origin_its_subprotocol_and_the_server_on() {
    let f = Fixture::new();
    open_guests(&f);
    let host = &f;
    let socket = move |headers: Vec<(&'static str, String)>| async move {
        guest(host, WAN, "GET", "/guest/signal", &headers).await
    };
    let off = socket(guest_upgrade()).await;
    assert_eq!(off.status, StatusCode::CONFLICT);
    configure(&f, json!({"webrtc_server": true}));
    for (headers, status) in [
        (without("origin", None), StatusCode::FORBIDDEN),
        (
            without("origin", Some("https://evil.example")),
            StatusCode::FORBIDDEN,
        ),
        (
            without("origin", Some("http://test")),
            StatusCode::FORBIDDEN,
        ),
        (
            without("sec-websocket-protocol", None),
            StatusCode::BAD_REQUEST,
        ),
        // The console's own socket protocol opens nothing here.
        (
            without(
                "sec-websocket-protocol",
                Some("rubylight.signal.v2, csrf.x"),
            ),
            StatusCode::BAD_REQUEST,
        ),
        (guest_upgrade(), StatusCode::UPGRADE_REQUIRED),
    ] {
        let reply = socket(headers.clone()).await;
        assert_eq!(reply.status, status, "{headers:?}: {}", reply.text());
    }
}

async fn guest_socket(address: SocketAddr, extra: &str) -> Ws {
    let ws = Ws::connect_with(
        address,
        "/guest/signal",
        &format!("Origin: https://test\r\nSec-WebSocket-Protocol: rubylight.guest.v1\r\n{extra}"),
    )
    .await;
    assert!(
        ws.head
            .to_ascii_lowercase()
            .contains("\r\nsec-websocket-protocol: rubylight.guest.v1\r\n"),
        "{}",
        ws.head
    );
    ws
}

#[tokio::test]
async fn the_guest_socket_speaks_only_its_own_messages_even_with_a_console_sign_in() {
    let f = Fixture::new();
    configure(&f, json!({"webrtc_server": true}));
    open_guests(&f);
    let address = serve(&f).await;
    let (cookie, csrf) = sign_in(&f);
    // A right link while the owner is not streaming: the page is told to wait.
    let made = invite(
        &f,
        &cookie,
        &csrf,
        json!({"permission": "view", "seconds": 300}),
    )
    .await;
    let token = made.json()["token"].as_str().unwrap().to_owned();
    let mut ws = guest_socket(address, "").await;
    ws.send(json!({"type": "join", "token": token, "name": "Sam"}))
        .await;
    assert_eq!(refused(&mut ws).await, "not-streaming");
    // Withdrawn, the same link opens nothing.
    let id = made.json()["invite"]["id"].as_str().unwrap().to_owned();
    let revoked = call(
        &f,
        LAN,
        "POST",
        "/api/guests/revoke",
        &console(&cookie, &csrf),
        Some(json!({"id": id})),
    )
    .await;
    assert_eq!(revoked.status, StatusCode::OK);
    let mut ws = guest_socket(address, "").await;
    ws.send(json!({"type": "join", "token": token})).await;
    assert_eq!(refused(&mut ws).await, "invalid-link");

    // The console's messages, from a tab that is signed in to it, get nothing.
    let signed_in = format!("Cookie: {cookie}\r\n");
    for message in [
        json!({"version": 2, "type": "get-apps"}),
        json!({"version": 2, "type": "start-session", "appId": "1"}),
        json!({"type": "answer", "sdp": "v=0"}),
        json!({"type": "join"}),
    ] {
        let mut ws = guest_socket(address, &signed_in).await;
        ws.send(message.clone()).await;
        assert_eq!(refused(&mut ws).await, "invalid-message", "{message}");
    }
    // Each counted as a wrong token, and with the withdrawn link's that is five from this
    // address: its next socket waits out the limit before anything is looked at.
    let mut ws = guest_socket(address, "").await;
    assert_eq!(refused(&mut ws).await, "rate-limited");
}
