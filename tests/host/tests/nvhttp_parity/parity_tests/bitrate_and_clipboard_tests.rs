//! NVHTTP encoder bitrate reporting, ABR capabilities, and text/image clipboard tests.
use super::*;

#[tokio::test]
async fn serverinfo_encoder_bitrate_tracks_only_own_video_sessions() {
    let f = Fixture::new();
    let client = f.client(1 << 25);
    let config = butterpollo_config::config::Config::parse("fec_percentage=20").unwrap();
    let mut negotiated = butterpollo_media_policy::rtsp::Negotiated {
        configured_bitrate_kbps: 50_000,
        audio_channels: 2,
        audio_quality: true,
        ..Default::default()
    };
    butterpollo_media_policy::stream_policy::apply(&mut negotiated, 60_000, &config);
    assert_eq!(negotiated.bitrate_kbps, 38_988);
    let mine =
        butterpollo_stream::state::Session::new(f.launch(client.clone(), Role::Stream), negotiated);
    f.host
        .sessions
        .lock()
        .unwrap()
        .active
        .insert(mine.launch.id.clone(), mine.clone());
    f.session(client.clone(), Role::InputOnly);
    let stopping = f.session(client.clone(), Role::Stream);
    stopping.stop();
    let mut other = client.clone();
    other.uuid = "other".into();
    let theirs = f.session(other, Role::Stream);
    let mut connection = f.connection(true);
    connection.peer = "192.0.2.1:12345".parse().unwrap();
    let info = || {
        serverinfo_response(
            &f.host,
            &connection,
            &config,
            Some(&client),
            true,
            String::new(),
            0,
            false,
        )
    };
    for current in [38_988, 24_000] {
        if current == 24_000 {
            let applied = document(request(&f, true, "GET", "/bitrate?bitrate=24000").await).await;
            assert_eq!(field(&applied, "bitrate"), "24000");
        }
        let root = document(info()).await;
        assert_eq!(
            field(&root, "RustHostEncoderBitrateKbps"),
            current.to_string()
        );
        assert_eq!(field(&root, "RustHostInitialEncoderBitrateKbps"), "38988");
        assert_eq!(mine.bitrate.load(Ordering::Acquire), current);
        assert_eq!(theirs.bitrate.load(Ordering::Acquire), 20_000);
    }
    let second = butterpollo_stream::state::Session::new(
        f.launch(client.clone(), Role::RemoteMonitor),
        butterpollo_media_policy::rtsp::Negotiated {
            bitrate_kbps: 18_000,
            ..Default::default()
        },
    );
    second.bitrate.store(30_000, Ordering::Release);
    f.host
        .sessions
        .lock()
        .unwrap()
        .active
        .insert(second.launch.id.clone(), second.clone());
    let root = document(info()).await;
    assert_eq!(field(&root, "RustHostEncoderBitrateKbps"), "24000");
    assert_eq!(field(&root, "RustHostInitialEncoderBitrateKbps"), "18000");
    mine.stop();
    second.stop();
    let root = document(info()).await;
    assert_eq!(field(&root, "RustHostEncoderBitrateKbps"), "0");
    assert_eq!(field(&root, "RustHostInitialEncoderBitrateKbps"), "0");
}

#[tokio::test]
async fn serverinfo_encoder_bitrate_requires_authenticated_view_or_launch_permission() {
    let f = Fixture::new();
    let client = f.client(0);
    f.session(client, Role::Stream);
    let config = f.host.config.read().unwrap().clone();
    for (tls, permission, expected) in [
        (true, 0, "0"),
        (true, 1 << 24, "0"),
        (true, 1 << 25, "20000"),
        (true, 1 << 26, "20000"),
        (false, VIEW, "0"),
    ] {
        f.host.paired.write().unwrap().clients[0].perm = permission;
        let connection = f.connection(tls);
        let client = authenticated(&f.host, &connection, 0).ok();
        let root = document(serverinfo_response(
            &f.host,
            &connection,
            &config,
            client.as_ref(),
            true,
            String::new(),
            0,
            false,
        ))
        .await;
        assert_eq!(field(&root, "RustHostEncoderBitrateKbps"), expected);
        assert_eq!(field(&root, "RustHostInitialEncoderBitrateKbps"), expected);
    }
}

#[tokio::test]
async fn bitrate_caps_and_alias_update_only_the_authenticated_devices_sessions() {
    let f = Fixture::new();
    let client = f.client(1 << 25);
    let mine = f.session(client.clone(), Role::Stream);
    let second = f.session(client.clone(), Role::RemoteMonitor);
    let mut other = client;
    other.uuid = "other".into();
    let theirs = f.session(other, Role::Stream);
    let original = theirs.bitrate.load(Ordering::Acquire);
    for (ceiling, query, expected) in [
        ("60000", "bitrate=80000", 60000),
        ("0", "bitrate_kbps=900000", 500000),
        ("0", "bitrate=12345", 12345),
    ] {
        f.host
            .config
            .write()
            .unwrap()
            .values
            .insert("max_bitrate".into(), ceiling.into());
        let root = document(request(&f, true, "GET", &format!("/bitrate?{query}")).await).await;
        assert_eq!(root.attributes["status_code"], "200");
        assert_eq!(field(&root, "bitrate"), expected.to_string());
        assert_eq!(field(&root, "updated"), "2");
        assert_eq!(mine.bitrate.load(Ordering::Acquire), expected);
        assert_eq!(second.bitrate.load(Ordering::Acquire), expected);
        assert_eq!(theirs.bitrate.load(Ordering::Acquire), original);
    }
    for query in [
        "",
        "bitrate=0",
        "bitrate=-1",
        "bitrate=4294967296",
        "bitrate=no",
    ] {
        let root = document(request(&f, true, "GET", &format!("/bitrate?{query}")).await).await;
        assert_eq!(root.attributes["status_code"], "400");
        assert_eq!(field(&root, "bitrate"), "0");
        assert_eq!(mine.bitrate.load(Ordering::Acquire), 12345);
    }
    f.host.sessions.lock().unwrap().active.clear();
    assert_eq!(
        document(request(&f, true, "GET", "/bitrate?bitrate=1").await)
            .await
            .attributes["status_code"],
        "404"
    );
    f.host.paired.write().unwrap().clients[0].perm = 1 << 24;
    assert_eq!(
        document(request(&f, true, "GET", "/bitrate?bitrate=1").await)
            .await
            .attributes["status_code"],
        "403"
    );
}

#[tokio::test]
async fn abr_capabilities_are_client_driven_and_require_view_or_launch_permission() {
    let f = Fixture::new();
    f.client(0);
    for perm in [0, 1 << 24, 1 << 25, 1 << 26] {
        f.host.paired.write().unwrap().clients[0].perm = perm;
        let response = request(&f, true, "GET", "/api/abr/capabilities").await;
        if perm & VIEW == 0 {
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        } else {
            assert_eq!(response.status(), StatusCode::OK);
            let body = to_bytes(response.into_body(), 1024).await.unwrap();
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
                json!({"supported":false,"version":1,"features":["runtime_bitrate","host_loss_adaptation"],"host_adaptive_bitrate":false,"client_bitrate_takes_ownership":true})
            );
        }
    }
}

#[tokio::test]
async fn clipboard_requires_direction_view_text_and_an_active_session_for_this_device() {
    let f = Fixture::new();
    let client = f.client((1 << 16) | (1 << 17) | (1 << 25));
    let args = Args::from([("type".into(), "text".into())]);
    let c = f.connection(true);
    assert!(clipboard_client(&f.host, &c, 1 << 17, &args).is_err());
    let mut other = client.clone();
    other.uuid = "other".into();
    f.session(other, Role::Stream);
    assert!(clipboard_client(&f.host, &c, 1 << 17, &args).is_err());
    let mine = f.session(client, Role::InputOnly);
    assert!(clipboard_client(&f.host, &c, 1 << 17, &args).is_ok());
    assert!(clipboard_client(&f.host, &c, 1 << 16, &args).is_ok());
    assert!(clipboard_client(&f.host, &c, 1 << 17, &Args::new()).is_err());
    assert!(
        clipboard_client(
            &f.host,
            &c,
            1 << 17,
            &Args::from([("type".into(), "image".into())])
        )
        .is_err()
    );
    let invalid = clipboard_write(
        State(f.host.clone()),
        Extension(c.clone()),
        Query(args.clone()),
        Request::new(Body::from(vec![0xff])),
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    for perm in [1 << 17, 1 << 25] {
        f.host.paired.write().unwrap().clients[0].perm = perm;
        for method in ["GET", "POST"] {
            assert_eq!(
                request(&f, true, method, "/actions/clipboard?type=text")
                    .await
                    .status(),
                StatusCode::FORBIDDEN
            );
        }
    }
    f.host.paired.write().unwrap().clients[0].perm = (1 << 17) | (1 << 26);
    assert!(clipboard_client(&f.host, &c, 1 << 17, &args).is_ok());
    // With the image clipboard off, `type=png` is 403 as any other type is,
    // even for a device allowed both directions.
    f.host.paired.write().unwrap().clients[0].perm = (1 << 16) | (1 << 17) | (1 << 26);
    let png = Args::from([("type".into(), "png".into())]);
    assert_eq!(
        clipboard_client(&f.host, &c, 1 << 17, &png)
            .ok()
            .map(|(kind, _)| kind),
        Some(ClipboardType::Png)
    );
    f.host
        .config
        .write()
        .unwrap()
        .values
        .insert("clipboard_image".into(), "false".into());
    assert!(clipboard_client(&f.host, &c, 1 << 17, &png).is_err());
    for method in ["GET", "POST"] {
        assert_eq!(
            request(&f, true, method, "/actions/clipboard?type=png")
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    let canceled = f
        .host
        .sessions
        .lock()
        .unwrap()
        .request_stop(Some(&mine.launch.client.uuid));
    drop(canceled);
    assert!(clipboard_client(&f.host, &c, 1 << 17, &args).is_err());
}

async fn post_png(
    f: &Fixture,
    content_type: Option<&str>,
    length: Option<usize>,
    body: Vec<u8>,
) -> StatusCode {
    let mut request = Request::builder()
        .method("POST")
        .uri("/actions/clipboard?type=png");
    if let Some(content_type) = content_type {
        request = request.header(header::CONTENT_TYPE, content_type);
    }
    if let Some(length) = length {
        request = request.header(header::CONTENT_LENGTH, length);
    }
    router(f.host.clone(), true)
        .layer(Extension(f.connection(true)))
        .oneshot(request.body(Body::from(body)).unwrap())
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn clipboard_images_travel_as_png_within_the_limit() {
    use butterpollo_session::clipboard_image::{Image, decode_png, encode_png};
    const MIB: usize = 1024 * 1024;
    let _turn = clipboard::TURN.lock().await;
    let f = Fixture::new();
    let client = f.client((1 << 16) | (1 << 17) | (1 << 25));
    let client_uuid = client.uuid.clone();
    let _session = f.session(client, Role::InputOnly);
    let set = |key: &str, value: &str| {
        f.host
            .config
            .write()
            .unwrap()
            .values
            .insert(key.into(), value.into());
    };
    *clipboard::PNG.lock().unwrap() = None;
    let get = || request(&f, true, "GET", "/actions/clipboard?type=png");
    assert_eq!(get().await.status(), StatusCode::NOT_FOUND);

    let image = Image {
        width: 2,
        height: 1,
        rgba: vec![255, 0, 0, 255, 0, 0, 255, 128],
    };
    let png = encode_png(&image).unwrap();
    assert_eq!(
        post_png(&f, None, Some(png.len()), png.clone()).await,
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
    assert_eq!(
        post_png(&f, Some("image/png"), Some(png.len()), png.clone()).await,
        StatusCode::OK
    );
    let read = get().await;
    assert_eq!(read.status(), StatusCode::OK);
    assert_eq!(read.headers()[header::CONTENT_TYPE], "image/png");
    let body = to_bytes(read.into_body(), MIB).await.unwrap();
    assert_eq!(decode_png(&body).unwrap(), image);
    assert_eq!(
        post_png(&f, Some("Image/PNG; x=1"), None, b"GIF89a".to_vec()).await,
        StatusCode::BAD_REQUEST
    );
    // The 1 MiB limit of the other routes doesn't hold an image back...
    assert_eq!(
        post_png(&f, Some("image/png"), Some(2 * MIB), vec![0; 2 * MIB]).await,
        StatusCode::BAD_REQUEST
    );
    // ...but `clipboard_image_max_mb` does, by the Content-Length or, without
    // one, once the body passes it.
    set("clipboard_image_max_mb", "1");
    // Each refusal is logged with the size and the limit.
    let path = f.host.directory.join("clipboard-limit.log");
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .without_time()
        .with_ansi(false)
        .with_writer(std::sync::Mutex::new(std::fs::File::create(&path).unwrap()))
        .finish();
    let guard = butterpollo_stream::test_log::capture(subscriber);
    for length in [Some(MIB + 1), None] {
        assert_eq!(
            post_png(&f, Some("image/png"), length, vec![0; MIB + 1]).await,
            StatusCode::PAYLOAD_TOO_LARGE,
            "{length:?}"
        );
    }
    // A larger image on the PC's clipboard isn't sent either.
    *clipboard::PNG.lock().unwrap() = Some(vec![0; MIB + 1]);
    assert_eq!(get().await.status(), StatusCode::PAYLOAD_TOO_LARGE);
    *clipboard::PNG.lock().unwrap() = None;
    drop(guard);
    let log = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines.len(), 3, "{log}");
    assert!(
        lines[0].contains(&format!(
            "refused an image of {} bytes by its Content-Length, over the limit of {MIB} bytes",
            MIB + 1
        )),
        "{log}"
    );
    assert!(
        lines[1].contains(&format!(
            "refused an image whose body passed the limit of {MIB} bytes"
        )),
        "{log}"
    );
    assert!(
        lines[2].contains(&format!(
            "the image is {} bytes as PNG, over the limit of {MIB} bytes",
            MIB + 1
        )),
        "{log}"
    );
    assert!(!log.contains("pixels"), "{log}");
    // While one of its image requests runs, the device's next one is
    // refused, both ways; text doesn't wait for images.
    let running = IMAGE_TURNS.take(&client_uuid).await.unwrap();
    assert_eq!(get().await.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        post_png(&f, Some("image/png"), Some(png.len()), png.clone()).await,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        request(&f, true, "GET", "/actions/clipboard?type=text")
            .await
            .status(),
        StatusCode::OK
    );
    drop(running);
    assert_eq!(get().await.status(), StatusCode::NOT_FOUND);
}

#[test]
fn unpaired_size_refusals_are_logged_once_a_minute_per_address() {
    let log = RefusalLog::new();
    let now = Instant::now();
    let a: std::net::IpAddr = "192.168.1.20".parse().unwrap();
    let b: std::net::IpAddr = "192.168.1.21".parse().unwrap();
    assert_eq!(log.admit(a, now), Some(0));
    assert_eq!(log.admit(a, now + Duration::from_secs(1)), None);
    // As an IPv4-mapped IPv6 address it is the same device.
    let mapped = std::net::IpAddr::V6(std::net::Ipv4Addr::new(192, 168, 1, 20).to_ipv6_mapped());
    assert_eq!(log.admit(mapped, now + Duration::from_secs(59)), None);
    // Another address has its own minute.
    assert_eq!(log.admit(b, now + Duration::from_secs(2)), Some(0));
    // The next line says how many went unlogged.
    assert_eq!(log.admit(a, now + Duration::from_secs(60)), Some(2));
    assert_eq!(log.admit(a, now + Duration::from_secs(61)), None);
    // Addresses quiet for a minute are forgotten once another is logged.
    let c: std::net::IpAddr = "192.168.1.22".parse().unwrap();
    assert_eq!(log.admit(c, now + Duration::from_secs(120)), Some(0));
    assert_eq!(log.0.lock().unwrap().len(), 1);
}

#[test]
fn unpaired_size_refusals_from_rotating_addresses_cannot_fill_the_log() {
    let log = RefusalLog::new();
    let now = Instant::now();
    // A device's IPv6 privacy addresses rotate within its /64: one line a minute for all of them.
    let first: std::net::IpAddr = "2001:db8:1:2::1".parse().unwrap();
    assert_eq!(log.admit(first, now), Some(0));
    for n in 2..100u16 {
        let rotated = std::net::Ipv6Addr::new(0x2001, 0xdb8, 1, 2, n, n, n, n);
        assert_eq!(log.admit(rotated.into(), now), None);
    }
    // Many addresses in one minute: only the first are logged.
    let logged = (0..=255u8)
        .filter(|&n| {
            log.admit(std::net::Ipv4Addr::new(10, 0, 0, n).into(), now)
                .is_some()
        })
        .count();
    assert_eq!(logged, UNPAIRED_REFUSAL_LOG_PEERS - 1);
    assert_eq!(log.0.lock().unwrap().len(), UNPAIRED_REFUSAL_LOG_PEERS);
    // A minute on, they are logged again.
    let later = now + UNPAIRED_REFUSAL_LOG_INTERVAL;
    let ten: std::net::IpAddr = std::net::Ipv4Addr::new(10, 0, 0, 200).into();
    assert_eq!(log.admit(ten, later), Some(0));
}

#[tokio::test]
async fn an_unpaired_device_over_the_clipboard_limit_is_logged_once() {
    let f = Fixture::new();
    // Only this test uses the address, so the shared log starts empty for it.
    let unpaired = Connection {
        peer: "192.0.2.77:50000".parse().unwrap(),
        local: "127.0.0.1:47984".parse().unwrap(),
        certificate: None,
        tls: true,
        second_screen: false,
    };
    let path = f.host.directory.join("unpaired-limit.log");
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .without_time()
        .with_ansi(false)
        .with_writer(std::sync::Mutex::new(std::fs::File::create(&path).unwrap()))
        .finish();
    let guard = butterpollo_stream::test_log::capture(subscriber);
    for _ in 0..3 {
        let request = Request::builder()
            .method("POST")
            .uri("/actions/clipboard?type=text")
            .header(header::CONTENT_LENGTH, 2 * 1024 * 1024)
            .body(Body::empty())
            .unwrap();
        let status = router(f.host.clone(), true)
            .layer(Extension(unpaired.clone()))
            .oneshot(request)
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    }
    drop(guard);
    let log = std::fs::read_to_string(&path).unwrap();
    assert_eq!(log.lines().count(), 1, "{log}");
    assert!(
        log.contains("refused a copy of 2097152 bytes by its Content-Length from unpaired 192.0.2.77, over the limit of 1048576 bytes (0 more from it unlogged)"),
        "{log}"
    );
}

#[tokio::test]
async fn image_clipboard_requests_take_turns_one_per_device() {
    let turns = ImageTurns::new();
    let first = turns.take("a").await.expect("a's turn");
    // The device's second request is refused rather than queued...
    assert!(turns.take("a").await.is_none());
    // ...and another device's waits, without its copy or body, for the turn.
    let waiting = tokio::time::timeout(Duration::from_millis(50), turns.take("b")).await;
    assert!(waiting.is_err());
    // A request dropped while it waited leaves its device free to ask again.
    assert!(!turns.devices.lock().unwrap().contains("b"));
    drop(first);
    let b = turns.take("b").await.expect("b's turn once a's has ended");
    assert!(
        tokio::time::timeout(Duration::from_millis(50), turns.take("a"))
            .await
            .is_err()
    );
    drop(b);
    assert!(turns.take("a").await.is_some());
}
