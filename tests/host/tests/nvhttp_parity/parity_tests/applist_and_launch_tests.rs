//! NVHTTP applist, co-op listing, unpair, second-screen, launch gate, and encoder probe tests.
use super::*;

#[tokio::test]
async fn applist_permissions_placeholder_and_hdr_xml_match_moonlight() {
    let f = Fixture::new();
    f.client(1 << 25);
    let denied = document(request(&f, true, "GET", "/applist").await).await;
    let app = denied.get_child("App").unwrap();
    assert_eq!(field(app, "ID"), "114514");
    assert_eq!(field(app, "IsHdrSupported"), "0");
    assert_eq!(
        field(app, "AppTitle"),
        "Permission denied - enable \"List applications\" for this device in the host's Web UI"
    );
    assert_eq!(denied.children.len(), 1);
    f.host.paired.write().unwrap().clients[0].perm = 1 << 24;
    let title = "Game <&> \"日本語\"";
    f.host.apps.write().unwrap()[0].name = title.into();
    let allowed = document(request(&f, true, "GET", "/applist").await).await;
    assert_eq!(allowed.children.len(), f.host.apps.read().unwrap().len());
    let app = allowed.get_child("App").unwrap();
    assert_eq!(field(app, "AppTitle"), title);
    assert_eq!(
        field(app, "ID"),
        f.host.apps.read().unwrap()[0].id().to_string()
    );
    assert_eq!(field(app, "IsHdrSupported"), "1");
    assert!(!field(app, "UUID").is_empty());
    f.host.paired.write().unwrap().clients[0].enabled = false;
    let disabled = document(request(&f, true, "GET", "/applist").await).await;
    assert_eq!(disabled.attributes["status_code"], "401");
    assert!(disabled.children.is_empty());
}

/// With a Nucleus Co-op handler for an app's program the list gains its
/// co-op entry after the user's apps; the user's entries keep their order,
/// index and ids.
#[tokio::test]
async fn applist_lists_co_op_entries_after_the_apps() {
    let f = Fixture::new();
    f.client(1 << 24);
    let before = document(request(&f, true, "GET", "/applist").await).await;
    f.host.nucleus.use_pc(std::sync::Arc::new(
        butterpollo_stream::nucleus::fake::Pc::installed(),
    ));
    // Without a handler for any app's program the list does not change.
    let installed = document(request(&f, true, "GET", "/applist").await).await;
    assert_eq!(installed, before);
    let app = |name: &str, cmd: String| App {
        name: name.into(),
        cmd,
        working_dir: String::new(),
        prep: Vec::new(),
        computed_id: None,
        aliases: Vec::new(),
        extra: Default::default(),
    };
    let portal = app(
        "Portal 2",
        format!("\"{}\"", butterpollo_stream::nucleus::fake::EXE),
    );
    f.host.apps.write().unwrap().push(portal);
    let apps = f.host.apps.read().unwrap().clone();
    let listed = document(request(&f, true, "GET", "/applist").await).await;
    assert_eq!(listed.children.len(), apps.len() + 1);
    let entries: Vec<_> = listed
        .children
        .iter()
        .filter_map(xmltree::XMLNode::as_element)
        .collect();
    for (index, app) in apps.iter().enumerate() {
        assert_eq!(field(entries[index], "ID"), app.id().to_string());
        assert_eq!(field(entries[index], "IDX"), index.to_string());
    }
    let coop = entries[apps.len()];
    assert_eq!(field(coop, "AppTitle"), "Portal 2 (Co-op)");
    assert_eq!(
        field(coop, "ID"),
        app("Portal 2 (Co-op)", String::new()).id().to_string()
    );
    assert_eq!(field(coop, "IDX"), apps.len().to_string());
    assert_eq!(field(coop, "UUID"), "");
}

/// An app that hides its co-op entry (`coop-entry: false`) lists none, and
/// the entry's id launches nothing: it is not found, as any unknown id.
#[tokio::test]
async fn a_hidden_co_op_entry_is_not_listed_and_its_id_cannot_be_launched() {
    let f = Fixture::new();
    let client = f.client(u32::MAX);
    f.host.nucleus.use_pc(std::sync::Arc::new(
        butterpollo_stream::nucleus::fake::Pc::installed(),
    ));
    let mut portal = App {
        name: "Portal 2".into(),
        cmd: format!("\"{}\"", butterpollo_stream::nucleus::fake::EXE),
        working_dir: String::new(),
        prep: Vec::new(),
        computed_id: None,
        aliases: Vec::new(),
        extra: Default::default(),
    };
    f.host.apps.write().unwrap().push(portal.clone());
    let apps = f.host.apps.read().unwrap().clone();
    let id = f.host.nucleus.entries(&apps, "")[0].id();
    let resolve = || {
        resolve_launch_app(
            &f.host,
            &Args::new(),
            &client,
            id,
            None,
            remote::Owner::None,
            false,
        )
    };
    let titles = |list: &xmltree::Element| -> Vec<String> {
        list.children
            .iter()
            .filter_map(xmltree::XMLNode::as_element)
            .map(|entry| field(entry, "AppTitle"))
            .collect()
    };
    // Shown, its id launches the co-op entry.
    assert_eq!(resolve().unwrap().3.map(|coop| coop.id()), Some(id));
    let shown = titles(&document(request(&f, true, "GET", "/applist").await).await);
    assert!(shown.contains(&"Portal 2 (Co-op)".to_string()), "{shown:?}");
    portal
        .extra
        .insert(butterpollo_stream::nucleus::ENTRY_KEY.into(), false.into());
    *f.host.apps.write().unwrap().last_mut().unwrap() = portal;
    let hidden = titles(&document(request(&f, true, "GET", "/applist").await).await);
    assert_eq!(hidden.len(), shown.len() - 1);
    assert!(
        !hidden.contains(&"Portal 2 (Co-op)".to_string()),
        "{hidden:?}"
    );
    assert_eq!(
        resolve().unwrap_err().to_string(),
        APPLICATION_NOT_FOUND,
        "a hidden entry's id launches nothing"
    );
}

#[tokio::test]
async fn unpair_get_and_post_require_tls_and_stop_only_the_paired_device() {
    for method in ["GET", "POST"] {
        let f = Fixture::new();
        let client = f.client(u32::MAX);
        let mine = f.session(client.clone(), Role::InputOnly);
        let mut other = client;
        other.uuid = "another-device".into();
        let theirs = f.session(other, Role::RemoteMonitor);
        let plain = document(request(&f, false, method, "/unpair").await).await;
        assert_eq!(field(&plain, "unpaired"), "0");
        assert_eq!(f.host.paired.read().unwrap().clients.len(), 1);
        assert!(!mine.stopping());
        let secure = document(request(&f, true, method, "/unpair").await).await;
        assert_eq!(field(&secure, "unpaired"), "1");
        assert!(mine.stopping());
        assert!(!theirs.stopping());
        assert!(
            butterpollo_session::state::PairedState::load(&f.host.paired_path)
                .unwrap()
                .clients
                .is_empty()
        );
        assert_eq!(
            field(
                &document(request(&f, true, method, "/unpair").await).await,
                "unpaired"
            ),
            "0"
        );
    }
}

#[tokio::test]
async fn unpairing_from_either_screen_ends_both_screens_and_the_pairing() {
    for query in ["", "?rlSecondScreen=1"] {
        let f = Fixture::new();
        let client = f.client(u32::MAX);
        let first = f.session(client.clone(), Role::Stream);
        let second = f.session(client.clone().second_screen(), Role::Stream);
        let mut other = client;
        other.uuid = "another-device".into();
        let theirs = f.session(other, Role::Stream);
        let root = document(request(&f, true, "GET", &format!("/unpair{query}")).await).await;
        assert_eq!(field(&root, "unpaired"), "1", "{query}");
        assert!(first.stopping(), "{query}");
        assert!(second.stopping(), "{query}");
        assert!(!theirs.stopping(), "{query}");
        assert!(f.host.paired.read().unwrap().clients.is_empty(), "{query}");
    }
}

#[tokio::test]
async fn requests_of_a_second_screen_reach_its_own_stream() {
    let f = Fixture::new();
    let client = f.client(1 << 25);
    let first = f.session(client.clone(), Role::Stream);
    let second = f.session(client.clone().second_screen(), Role::Stream);
    let original = second.bitrate.load(Ordering::Acquire);
    let root = document(request(&f, true, "GET", "/bitrate?bitrate=12345").await).await;
    assert_eq!(field(&root, "updated"), "1");
    assert_eq!(first.bitrate.load(Ordering::Acquire), 12345);
    assert_eq!(second.bitrate.load(Ordering::Acquire), original);
    let root =
        document(request(&f, true, "GET", "/bitrate?bitrate=23456&rlSecondScreen=1").await).await;
    assert_eq!(field(&root, "updated"), "1");
    assert_eq!(first.bitrate.load(Ordering::Acquire), 12345);
    assert_eq!(second.bitrate.load(Ordering::Acquire), 23456);
    // The device's permissions decide for both screens.
    f.host.paired.write().unwrap().clients[0].perm = 1 << 24;
    let refused =
        document(request(&f, true, "GET", "/bitrate?bitrate=1&rlSecondScreen=1").await).await;
    assert_eq!(refused.attributes["status_code"], "403");
    f.host.paired.write().unwrap().clients[0].enabled = false;
    f.host.paired.write().unwrap().clients[0].perm = 1 << 25;
    let refused =
        document(request(&f, true, "GET", "/bitrate?bitrate=1&rlSecondScreen=1").await).await;
    assert_eq!(refused.attributes["status_code"], "403");
    assert_eq!(second.bitrate.load(Ordering::Acquire), 23456);
}

#[tokio::test]
async fn a_second_screen_launch_is_its_own_device_and_needs_the_devices_permission() {
    let f = Fixture::new();
    f.client(1 << 26);
    let mut connection = f.connection(true);
    connection.second_screen = true;
    let screen = validate_launch_client(&f.host, &connection, None, false).unwrap();
    assert_eq!(
        (screen.uuid.as_str(), screen.name.as_str()),
        ("parity-device#screen2", "Parity device (second screen)")
    );
    f.host.paired.write().unwrap().clients[0].perm = 1 << 24;
    assert!(validate_launch_client(&f.host, &connection, None, false).is_err());
}

#[tokio::test]
async fn launch_and_resume_reject_disconnected_sessions_before_preparation() {
    let f = Fixture::new();
    f.client(1 << 26);
    let app_id = f.host.apps.read().unwrap()[0].id();
    let args = Args::from([
        ("appid".into(), app_id.to_string()),
        ("rikey".into(), "00000000000000000000000000000000".into()),
        ("rikeyid".into(), "1".into()),
    ]);
    let message = "The host Windows session is disconnected after RDP. Retry in a moment.";
    for (resume, reply) in [(false, "gamesession"), (true, "resume")] {
        SESSION_ERROR.set(Some(message));
        let response = start(f.host.clone(), f.connection(true), args.clone(), resume);
        SESSION_ERROR.set(None);
        let root = document(response).await;
        assert_eq!(root.attributes["status_code"], "503");
        assert_eq!(root.attributes["status_message"], message);
        assert_eq!(field(&root, reply), "0");
        assert!(root.get_child("sessionUrl0").is_none());
        assert!(f.host.sessions.lock().unwrap().pending.is_empty());
        assert!(f.host.current_app.lock().unwrap().is_none());
        assert!(f.host.app_display.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn the_session_gate_keeps_authorization_and_terminate_semantics() {
    let f = Fixture::new();
    f.client(1 << 24);
    SESSION_ERROR.set(Some("session gate must not run for these requests"));
    let unauthorized = start(f.host.clone(), f.connection(true), Args::new(), false);
    f.host.paired.write().unwrap().clients[0].perm = 1 << 26;
    let terminate = start(
        f.host.clone(),
        f.connection(true),
        Args::from([("appid".into(), "2147483504".into())]),
        false,
    );
    SESSION_ERROR.set(None);
    assert_eq!(
        document(unauthorized).await.attributes["status_code"],
        "401"
    );
    let terminate = document(terminate).await;
    assert_eq!(terminate.attributes["status_code"], "409");
    assert_eq!(
        terminate.attributes["status_message"],
        "No application is running"
    );
    assert!(f.host.sessions.lock().unwrap().pending.is_empty());
}

#[tokio::test]
async fn launch_and_resume_reject_permissions_and_report_protocol_failures() {
    let f = Fixture::new();
    f.client(1 << 24);
    for endpoint in ["launch", "resume"] {
        let root = document(request(&f, true, "GET", &format!("/{endpoint}")).await).await;
        assert_eq!(root.attributes["status_code"], "401");
    }
    f.host.paired.write().unwrap().clients[0].perm = 1 << 26;
    for (endpoint, reply) in [("launch", "gamesession"), ("resume", "resume")] {
        let root = document(request(&f, true, "GET", &format!("/{endpoint}")).await).await;
        assert_eq!(root.attributes["status_code"], "503");
        assert_eq!(field(&root, reply), "0");
        assert_eq!(root.attributes["status_message"], "missing stream key");
    }
    assert!(f.host.sessions.lock().unwrap().pending.is_empty());
}

#[tokio::test]
async fn an_unavailable_encoder_does_not_queue_a_launch_or_start_an_app() {
    let f = Fixture::new();
    f.client(1 << 26);
    f.host.codecs.store(0, Ordering::Release);
    let app_id = f.host.apps.read().unwrap()[0].id();
    let root = document(
        request(
            &f,
            true,
            "GET",
            &format!("/launch?appid={app_id}&rikey=00000000000000000000000000000000&rikeyid=1"),
        )
        .await,
    )
    .await;
    assert_eq!(root.attributes["status_code"], "503");
    let message = &root.attributes["status_message"];
    assert!(message.contains("No video encoder is ready"), "{message}");
    assert!(message.contains("AMD Radeon GPU"), "{message}");
    assert!(message.contains("NVIDIA GPU and its NVENC"), "{message}");
    assert!(message.contains("update the graphics driver"), "{message}");
    assert!(
        message.contains("For Intel graphics, stay on Rubylight 2.6.x"),
        "{message}"
    );
    assert!(message.contains("/releases/tag/v2.6.0"), "{message}");
    assert!(f.host.sessions.lock().unwrap().pending.is_empty());
    assert!(f.host.current_app.lock().unwrap().is_none());
    assert!(f.host.app_display.lock().unwrap().is_empty());
}

/// A launch refused for want of an encoder names the encoder check's cause, as the
/// host log does, and the device's `/serverinfo` keeps it.
#[tokio::test]
async fn an_unavailable_encoder_refuses_the_launch_with_the_checks_cause() {
    let f = Fixture::new();
    f.client(1 << 26);
    f.host.codecs.store(0, Ordering::Release);
    let cause = "NVENC is selected (encoder = nvenc), but this PC has no NVIDIA GPU";
    *f.host.encoder_probe_error.lock().unwrap() = Some(cause.into());
    let app_id = f.host.apps.read().unwrap()[0].id();
    let root = document(
        request(
            &f,
            true,
            "GET",
            &format!("/launch?appid={app_id}&rikey=00000000000000000000000000000000&rikeyid=1"),
        )
        .await,
    )
    .await;
    assert_eq!(root.attributes["status_code"], "503");
    let message = &root.attributes["status_message"];
    assert!(
        message.starts_with(&format!(
            "The encoder check failed: {cause}: No video encoder is ready"
        )),
        "{message}"
    );
    assert_eq!(
        f.host.last_failure("parity-device").as_deref(),
        Some(format!("The encoder check failed: {cause}").as_str())
    );
    assert!(f.host.sessions.lock().unwrap().pending.is_empty());
}

#[tokio::test]
async fn discovery_does_not_wait_for_a_stuck_encoder_probe() {
    let f = Fixture::new();
    f.client(1 << 24);
    // The startup probe found nothing; a retry (no signed-in user, a driver
    // failure) must not hold discovery back.
    f.host.codecs.store(0, Ordering::Release);
    f.host.probing_codecs.store(true, Ordering::Release);
    let response = tokio::time::timeout(
        Duration::from_millis(500),
        request(&f, true, "GET", "/applist"),
    )
    .await
    .unwrap();
    let root = document(response).await;
    assert_eq!(root.attributes["status_code"], "200");
    assert_eq!(field(root.get_child("App").unwrap(), "IsHdrSupported"), "0");
}

/// The startup probe publishes `codecs` after a moment, as on a host that
/// Windows or Wake-on-LAN has just started.
fn finish_startup_probe_later(f: &Fixture, codecs: u32) -> std::thread::JoinHandle<()> {
    f.host.codecs.store(0, Ordering::Release);
    f.host.probing_codecs.store(true, Ordering::Release);
    f.host.startup_codecs.send_replace(false);
    let h = f.host.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        h.codecs.store(codecs, Ordering::Release);
        h.probing_codecs.store(false, Ordering::Release);
        h.startup_codecs.send_replace(true);
    })
}

#[tokio::test]
async fn serverinfo_offers_sleep_fields_to_paired_https_clients_only() {
    let f = Fixture::new();
    for https in [false, true] {
        let root = document(request(&f, https, "GET", "/serverinfo").await).await;
        for name in ["PowerActionVersion", "SleepSupported", "SleepAllowed"] {
            assert!(root.get_child(name).is_none(), "{name} https={https}");
        }
    }
    let mut client = f.client(u32::MAX);
    let info = |client: &Client| {
        let config = f.host.config.read().unwrap().clone();
        serverinfo_response(
            &f.host,
            &f.connection(true),
            &config,
            Some(client),
            false,
            "00:00:00:00:00:00".into(),
            0,
            false,
        )
    };
    // Tests have no sleep backend.
    let root = document(info(&client)).await;
    assert_eq!(field(&root, "PowerActionVersion"), "1");
    assert_eq!(field(&root, "SleepSupported"), "0");
    assert_eq!(field(&root, "SleepAllowed"), "0");
    client.extra.insert("allow_sleep".into(), true.into());
    assert_eq!(field(&document(info(&client)).await, "SleepAllowed"), "1");
    // Only S1 to S3 count: Modern Standby alone hibernated a laptop.
    let supported = |states| {
        f.host.power.set_backend(std::sync::Arc::new(
            butterpollo_stream::power_action::FixedStates(states),
        ));
        let root = info(&client);
        async move { field(&document(root).await, "SleepSupported") }
    };
    let s3 = SleepStates {
        s3: true,
        hibernate: true,
        ..Default::default()
    };
    assert_eq!(supported(s3).await, "1");
    for hibernate in [true, false] {
        let laptop = SleepStates {
            modern_standby: true,
            hibernate,
            ..Default::default()
        };
        assert_eq!(supported(laptop).await, "0", "hibernate {hibernate}");
    }
}

#[tokio::test]
async fn discovery_during_the_startup_probe_reports_its_codecs() {
    let f = Fixture::new();
    let probe = finish_startup_probe_later(&f, 0x1 | 0x100 | 0x200);
    let root = document(request(&f, false, "GET", "/serverinfo").await).await;
    probe.join().unwrap();
    // Not 0, which a client caches as a host without HEVC or HDR.
    assert_eq!(field(&root, "ServerCodecModeSupport"), "769");
}

#[tokio::test]
async fn a_launch_during_the_startup_probe_waits_for_its_result() {
    let f = Fixture::new();
    f.client(1 << 26);
    // Stop the launch at a later step, before any display is prepared.
    f.host
        .config
        .write()
        .unwrap()
        .values
        .insert("lan_encryption_mode".into(), "2".into());
    let app_id = f.host.apps.read().unwrap()[0].id();
    let args = Args::from([
        ("appid".into(), app_id.to_string()),
        ("rikey".into(), "00000000000000000000000000000000".into()),
        ("rikeyid".into(), "1".into()),
    ]);
    let probe = finish_startup_probe_later(&f, 0x1 | 0x100);
    let launch = {
        let (h, connection) = (f.host.clone(), f.connection(true));
        tokio::task::spawn_blocking(move || start(h, connection, args, false))
            .await
            .unwrap()
    };
    probe.join().unwrap();
    let root = document(launch).await;
    let message = &root.attributes["status_message"];
    assert!(!message.contains("No video encoder is ready"), "{message}");
    assert!(
        message.contains("requires encrypted streams on the local network"),
        "{message}"
    );
    assert!(f.host.sessions.lock().unwrap().pending.is_empty());
}

/// A device through an internet play tunnel reaches the host from this PC, from its own loopback
/// address (127.77.x.y) or, on a host listening on one address, from that address under a
/// registration. Either way it gets the internet's encryption, not the local network's.
#[tokio::test]
async fn a_tunnelled_launch_needs_the_internets_encryption() {
    let f = Fixture::new();
    f.client(1 << 26);
    {
        let mut config = f.host.config.write().unwrap();
        config
            .values
            .insert("lan_encryption_mode".into(), "0".into());
        config
            .values
            .insert("wan_encryption_mode".into(), "2".into());
    }
    let app_id = f.host.apps.read().unwrap()[0].id();
    for (peer, register) in [("127.77.3.4:40000", false), ("127.0.0.1:40001", true)] {
        let peer: std::net::SocketAddr = peer.parse().unwrap();
        let _mark = register.then(|| rubylight_traverse::peers::register(peer, "phone"));
        let args = Args::from([
            ("appid".into(), app_id.to_string()),
            ("rikey".into(), "00000000000000000000000000000000".into()),
            ("rikeyid".into(), "1".into()),
        ]);
        let probe = finish_startup_probe_later(&f, 0x1 | 0x100);
        let launch = {
            let h = f.host.clone();
            let mut connection = f.connection(true);
            connection.peer = peer;
            tokio::task::spawn_blocking(move || start(h, connection, args, false))
                .await
                .unwrap()
        };
        probe.join().unwrap();
        let root = document(launch).await;
        let message = &root.attributes["status_message"];
        assert!(
            message.contains("requires encrypted streams on the internet"),
            "{peer}: {message}"
        );
        assert!(f.host.sessions.lock().unwrap().pending.is_empty());
    }
}

#[tokio::test]
async fn a_stuck_startup_probe_holds_requests_only_for_the_limit() {
    let f = Fixture::new();
    f.host.startup_codecs.send_replace(false);
    let limit = Duration::from_millis(100);
    let started = Instant::now();
    f.host.wait_for_startup_codecs(limit).await;
    f.host.wait_for_startup_codecs_blocking(limit);
    let waited = started.elapsed();
    assert!(
        waited >= 2 * limit && waited < Duration::from_secs(2),
        "{waited:?}"
    );
    // Once the startup probe has finished, nothing waits again.
    f.host.startup_codecs.send_replace(true);
    let started = Instant::now();
    f.host.wait_for_startup_codecs(Duration::from_secs(5)).await;
    f.host
        .wait_for_startup_codecs_blocking(Duration::from_secs(5));
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[tokio::test]
async fn launch_failures_preserve_the_actionable_cause() {
    let error = anyhow::anyhow!("executable was not found").context("application could not start");
    let root = document(launch_response(Err(error), 1, "gamesession", false, false)).await;
    assert_eq!(
        root.attributes["status_message"],
        "application could not start: executable was not found"
    );
}

#[tokio::test]
async fn virtual_display_advertisement_matches_configuration_and_codecs() {
    let f = Fixture::new();
    for (config, windows_11, codecs, capable, hdr) in [
        ("", false, 0x200, "false", "false"),
        (
            "virtual_display_mode=disabled",
            true,
            0x200,
            "false",
            "false",
        ),
        ("virtual_display_mode=shared", true, 1, "true", "false"),
        ("virtual_display_mode=shared", true, 0x200, "true", "true"),
    ] {
        let config = butterpollo_config::config::Config::parse(config).unwrap();
        f.host.codecs.store(codecs, Ordering::Release);
        let root = document(serverinfo_response(
            &f.host,
            &f.connection(true),
            &config,
            None,
            windows_11,
            String::new(),
            0,
            true,
        ))
        .await;
        assert_eq!(field(&root, "VirtualDisplayCapable"), capable);
        assert_eq!(field(&root, "VirtualDisplayHDRCapable"), hdr);
    }
}
