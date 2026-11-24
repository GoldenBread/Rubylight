//! The guest port and the Stream page's guest panel: a guest reaches its page and its socket
//! and nothing of the console's, the guest port answers only while a link is live, and only a
//! signed-in console tab makes, lists or withdraws links.
use axum::{
    Extension, Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use butterpollo_session::config_ext::ConfigExt;
use butterpollo_session::state::Credentials;
use butterpollo_session::webrtc::guests::Permission;
use butterpollo_stream::{
    state::test_support::Fixture, tls::Connection, webrtc_bridge::MAX_GUESTS,
};
use butterpollo_web::web::{GUEST_POLICY, guest_router, router, token_catalog};
use butterpollo_webrtc::{WebRtcRuntime, test_support::Ws};
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto::Builder,
    service::TowerToHyperService,
};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::time::Duration;
use tower::ServiceExt;

#[path = "guests_helpers.rs"]
mod helpers;
use helpers::{LAN, Reply, TAILNET_HOST, WAN, connection, connection_to, with_webrtc};

/// A console tab signed in as "admin": its session cookie and CSRF token.
fn sign_in(f: &Fixture) -> (String, String) {
    f.host
        .save_credentials(&Credentials::new("admin".into(), "guests-test").unwrap())
        .unwrap();
    let (access, _, csrf, _) = f
        .host
        .new_web_session(
            "admin".into(),
            false,
            "test".into(),
            "192.168.4.20".into(),
            None,
        )
        .unwrap();
    (format!("__Host-apollo_session={access}"), csrf)
}

fn configure(f: &Fixture, settings: Value) {
    f.host
        .config
        .write()
        .unwrap()
        .update(settings.as_object().unwrap())
        .unwrap();
}

/// A request to the console's port.
async fn call(
    f: &Fixture,
    peer: &str,
    method: &str,
    path: &str,
    headers: &[(&str, String)],
    body: Option<Value>,
) -> Reply {
    send(router(with_webrtc(f)), peer, method, path, headers, body).await
}

/// A request to the guest port.
async fn guest(
    f: &Fixture,
    peer: &str,
    method: &str,
    path: &str,
    headers: &[(&str, String)],
) -> Reply {
    send(
        guest_router(with_webrtc(f)),
        peer,
        method,
        path,
        headers,
        None,
    )
    .await
}

/// A live link, so the guest port answers.
fn open_guests(f: &Fixture) -> String {
    let (invite, _) = WebRtcRuntime::for_host(&f.host)
        .guests
        .create(Permission::View, Duration::from_secs(300), None)
        .unwrap();
    invite.id
}

async fn send(
    router: Router,
    peer: &str,
    method: &str,
    path: &str,
    headers: &[(&str, String)],
    body: Option<Value>,
) -> Reply {
    send_on(router, connection(peer), method, path, headers, body).await
}

/// A request on the guest port from `peer` to this PC's `local` address.
async fn guest_on(f: &Fixture, peer: &str, local: &str, method: &str, path: &str) -> Reply {
    send_on(
        guest_router(with_webrtc(f)),
        connection_to(peer, local),
        method,
        path,
        &[],
        None,
    )
    .await
}

async fn send_on(
    router: Router,
    connection: Connection,
    method: &str,
    path: &str,
    headers: &[(&str, String)],
    body: Option<Value>,
) -> Reply {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, "test");
    for (name, value) in headers {
        request = request.header(*name, value);
    }
    let body = match body {
        Some(body) => {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(body.to_string())
        }
        None => Body::empty(),
    };
    let response = router
        .layer(Extension(connection))
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap()
        .to_vec();
    Reply {
        status,
        headers,
        body,
    }
}

/// What the console tab sends with a POST.
fn console(cookie: &str, csrf: &str) -> Vec<(&'static str, String)> {
    vec![
        ("cookie", cookie.to_owned()),
        ("x-csrf-token", csrf.to_owned()),
        ("origin", "https://test".into()),
    ]
}

async fn invite(f: &Fixture, cookie: &str, csrf: &str, body: Value) -> Reply {
    call(
        f,
        LAN,
        "POST",
        "/api/guests/invite",
        &console(cookie, csrf),
        Some(body),
    )
    .await
}

/// A console build and a guest build in the host's assets.
fn install_pages(f: &Fixture) {
    let root = &f.host.assets;
    std::fs::create_dir_all(root.join("guest/assets")).unwrap();
    std::fs::write(root.join("index.html"), "<p>console page</p>").unwrap();
    std::fs::write(root.join("guest/index.html"), "<p>guest page</p>").unwrap();
    std::fs::write(root.join("guest/assets/guest-1.js"), "export {}").unwrap();
}

/// The headers a guest page sends with its socket's upgrade.
fn guest_upgrade() -> Vec<(&'static str, String)> {
    vec![
        ("connection", "upgrade".into()),
        ("upgrade", "websocket".into()),
        ("sec-websocket-version", "13".into()),
        ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==".into()),
        ("origin", "https://test".into()),
        ("sec-websocket-protocol", "rubylight.guest.v1".into()),
    ]
}

fn without(name: &str, value: Option<&str>) -> Vec<(&'static str, String)> {
    let mut headers = guest_upgrade();
    let key = headers.iter().find(|(n, _)| *n == name).unwrap().0;
    headers.retain(|(n, _)| *n != name);
    if let Some(value) = value {
        headers.push((key, value.to_owned()));
    }
    headers
}

/// The guest port on a real socket, as `tls::serve` runs it (without TLS).
async fn serve(f: &Fixture) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local = listener.local_addr().unwrap();
    let router = guest_router(with_webrtc(f));
    tokio::spawn(async move {
        while let Ok((socket, peer)) = listener.accept().await {
            let service = TowerToHyperService::new(router.clone().layer(Extension(Connection {
                peer,
                local,
                certificate: None,
                tls: true,
                second_screen: false,
            })));
            tokio::spawn(async move {
                let _ = Builder::new(TokioExecutor::new())
                    .serve_connection_with_upgrades(TokioIo::new(socket), service)
                    .await;
            });
        }
    });
    local
}

/// The one message the host ends a refused guest's socket with, and the close after it.
async fn refused(ws: &mut Ws) -> String {
    let message = ws.next().await.expect("the host's reason");
    assert_eq!(message["type"], "error", "{message}");
    assert!(!message["message"].as_str().unwrap_or_default().is_empty());
    ws.closed().await;
    message["code"].as_str().unwrap().to_owned()
}

#[path = "guests_tests/guest_portal_tests.rs"]
mod guest_portal_tests;
#[path = "guests_tests/play_portal_tests.rs"]
mod play_portal_tests;
