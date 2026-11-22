//! Unit tests for the web console router, CSRF protections, and static asset confinement.
use super::*;

#[test]
fn cover_requests_name_their_app_and_short_paths_do_not_panic() {
    assert_eq!(super::cover_id("/api/apps/42/cover"), Some("42"));
    assert_eq!(super::cover_id("/api/covers/7"), Some("7"));
    assert_eq!(super::cover_id("/api/apps/cover"), None);
    assert_eq!(super::cover_id("/api/apps//cover"), None);
    assert_eq!(super::cover_id("/api/covers/"), None);
    assert_eq!(super::icon_id("/api/apps/abc-1/icon"), Some("abc-1"));
    assert_eq!(super::icon_id("/api/apps/icon"), None);
    assert_eq!(super::icon_id("/api/apps//icon"), None);
    assert_eq!(super::icon_id("/api/apps/a/b/icon"), None);
}

#[test]
fn sign_ins_are_limited_per_address_but_never_on_this_pc() {
    use std::net::IpAddr;
    let fresh = || super::SignIns {
        attempts: Default::default(),
    };
    let now = Instant::now();
    let lan: IpAddr = "192.168.1.20".parse().unwrap();
    let mut sign_ins = fresh();
    for _ in 0..10 {
        assert!(sign_ins.attempt(lan, now));
    }
    assert!(!sign_ins.attempt(lan, now), "the 11th attempt in a minute");
    assert!(sign_ins.attempt("192.168.1.21".parse().unwrap(), now));
    assert!(
        sign_ins.attempt(lan, now + Duration::from_secs(61)),
        "a minute later"
    );
    sign_ins.succeeded(lan);
    assert!(!sign_ins.attempts.contains_key(&lan));
    for local in ["127.0.0.1", "::1", "::ffff:127.0.0.1"] {
        let local: IpAddr = local.parse().unwrap();
        assert!((0..50).all(|_| sign_ins.attempt(local, now)), "{local}");
    }
    // One LAN host can take any address in its /64.
    let mut sign_ins = fresh();
    for host in 0..10 {
        assert!(sign_ins.attempt(format!("fe80::{host:x}").parse().unwrap(), now));
    }
    assert!(!sign_ins.attempt("fe80::abcd".parse().unwrap(), now));
    assert!(sign_ins.attempt("fd00:1:2:3::1".parse().unwrap(), now));
    // A full table forgets the oldest address instead of refusing new ones.
    let mut sign_ins = fresh();
    for host in 0..super::SignIns::TRACKED as u32 {
        let address = IpAddr::from(std::net::Ipv4Addr::from(0x0a00_0000 + host));
        assert!(sign_ins.attempt(address, now + Duration::from_millis(u64::from(host))));
    }
    let newcomer: IpAddr = "172.16.0.1".parse().unwrap();
    assert!(sign_ins.attempt(newcomer, now + Duration::from_secs(2)));
    assert_eq!(sign_ins.attempts.len(), super::SignIns::TRACKED);
    assert!(
        !sign_ins
            .attempts
            .contains_key(&"10.0.0.0".parse::<IpAddr>().unwrap())
    );
}

#[tokio::test]
async fn a_new_console_password_revokes_the_api_tokens() {
    use axum::{Extension, body::Body, http::Request};
    use tower::ServiceExt;
    let f = butterpollo_stream::state::test_support::Fixture::new();
    f.host.aliases.lock().unwrap()["root"]["api_tokens"] = json!([{"hash": "leaked"}]);
    let request = Request::builder()
        .method("POST")
        .uri("/api/password")
        .header("Content-Type", "application/json")
        .body(Body::from(
            json!({"newUsername": "user", "newPassword": "new-pass", "confirmNewPassword": "new-pass"})
                .to_string(),
        ))
        .unwrap();
    let response = router(f.host.clone())
        .layer(Extension(f.connection(true)))
        .oneshot(request)
        .await
        .unwrap();
    crate::test_support::value(response, StatusCode::OK).await;
    assert!(f.host.credentials.read().unwrap().is_some());
    assert_eq!(
        f.host.aliases.lock().unwrap()["root"]["api_tokens"],
        json!([])
    );
}

/// A launch holds this lock from its check of the running game to its start of one: closing
/// the game from the console waits for it, as Terminate and the tray do.
#[test]
fn closing_the_game_from_the_console_waits_for_a_launch_in_progress() {
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let app = butterpollo_session::state::App::desktop();
    *f.host.current_app.lock().unwrap() = Some(
        butterpollo_stream::process::RunningApp::with_environment(&app, Default::default())
            .unwrap(),
    );
    let launching = f.host.launch_transition.lock().unwrap();
    let host = f.host.clone();
    let close = std::thread::spawn(move || {
        apps::handle(&host, "POST", "/api/apps/close", &json!({})).unwrap();
    });
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(
        f.host.current_app.lock().unwrap().is_some(),
        "closed during a launch"
    );
    drop(launching);
    close.join().unwrap();
    assert!(f.host.current_app.lock().unwrap().is_none());
}

#[tokio::test]
async fn basic_credentials_do_not_carry_a_cross_site_form() {
    use axum::{Extension, body::Body, http::Request};
    use tower::ServiceExt;
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let credentials = Credentials::new("user".into(), "password1").unwrap();
    f.host.save_credentials(&credentials).unwrap();
    *f.host.credentials.write().unwrap() = Some(credentials);
    let send = |headers: &[(&str, &str)]| {
        let mut builder = Request::builder()
            .method("POST")
            .uri("/api/apps/close")
            .header(header::AUTHORIZATION, "Basic dXNlcjpwYXNzd29yZDE=");
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        router(f.host.clone())
            .layer(Extension(f.connection(true)))
            .oneshot(builder.body(Body::from("{}")).unwrap())
    };
    for refused in [
        &[("Content-Type", "text/plain")][..],
        &[(
            "Content-Type",
            "application/x-www-form-urlencoded; charset=utf-8",
        )],
        &[("Content-Type", "Multipart/Form-Data; boundary=x")],
        &[
            ("Content-Type", "application/json"),
            ("Sec-Fetch-Site", "cross-site"),
        ],
    ] {
        let response = send(refused).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{refused:?}");
    }
    // A script, and the console's own page, still get through.
    for allowed in [
        &[("Content-Type", "application/json")][..],
        &[],
        &[
            ("Content-Type", "application/json"),
            ("Sec-Fetch-Site", "same-origin"),
        ],
    ] {
        let response = send(allowed).await.unwrap();
        assert_ne!(response.status(), StatusCode::FORBIDDEN, "{allowed:?}");
    }
    assert!(!basic_auth_cross_site(&HeaderMap::new()));
}

#[test]
fn web_app_files_stay_in_their_folder() {
    let base = std::env::temp_dir().join(format!("butterpollo-web-{}", uuid::Uuid::new_v4()));
    let root = base.join("web");
    std::fs::create_dir_all(root.join("assets")).unwrap();
    std::fs::write(root.join("index.html"), "<html></html>").unwrap();
    std::fs::write(root.join("assets/index-1.js"), "export {}").unwrap();
    std::fs::write(base.join("outside.txt"), "x").unwrap();
    let get = |path: &str| app_file(&root, &Method::GET, path, APP_POLICY);
    let header = |response: &Response, name: header::HeaderName| {
        response
            .headers()
            .get(name)
            .map(|v| v.to_str().unwrap().to_owned())
    };

    let script = get("/assets/index-1.js");
    assert_eq!(script.status(), StatusCode::OK);
    assert!(
        header(&script, header::CACHE_CONTROL)
            .unwrap()
            .contains("immutable")
    );
    assert!(header(&script, header::CONTENT_SECURITY_POLICY).is_none());

    let page = get("/devices");
    assert_eq!(page.status(), StatusCode::OK);
    assert!(
        header(&page, header::CONTENT_TYPE)
            .unwrap()
            .starts_with("text/html")
    );
    assert!(header(&page, header::CACHE_CONTROL).is_none());
    assert_eq!(
        header(&page, header::CONTENT_SECURITY_POLICY).as_deref(),
        Some(APP_POLICY)
    );

    for escape in [
        "/../outside.txt",
        "/assets/../../outside.txt",
        "/C:/Windows/win.ini",
    ] {
        let response = get(escape);
        assert_ne!(
            header(&response, header::CONTENT_TYPE).as_deref(),
            Some("application/octet-stream"),
            "{escape}"
        );
    }
    assert_eq!(get("/assets/index-0.js").status(), StatusCode::NOT_FOUND);
    assert_eq!(
        app_file(&root, &Method::POST, "/devices", APP_POLICY).status(),
        StatusCode::METHOD_NOT_ALLOWED
    );
    std::fs::remove_dir_all(&base).ok();
}
