//! Tests for mode matching, client display choices, virtual display mode
//! precedence, background identity overrides, retiming, and VRR renegotiation.
use super::*;

#[test]
fn a_retained_display_matches_only_the_exact_mode_it_was_made_for() {
    let made = Negotiated {
        width: 2560,
        height: 1440,
        fps: 120,
        hdr: true,
        ..Default::default()
    };
    assert_eq!(stream_mode(&made), stream_mode(&made.clone()));
    for other in [
        Negotiated {
            width: 1920,
            ..made.clone()
        },
        Negotiated {
            fps: 60,
            ..made.clone()
        },
        Negotiated {
            rate_millihz: 119_880,
            ..made.clone()
        },
        Negotiated {
            hdr: false,
            ..made.clone()
        },
        Negotiated {
            vrr_low_latency: true,
            ..made.clone()
        },
    ] {
        assert_ne!(stream_mode(&made), stream_mode(&other));
    }
}
#[test]
fn only_a_stream_takes_the_clients_display_choice() {
    use butterpollo_media_policy::display_policy::Arrangement;
    let f = crate::state::test_support::Fixture::new();
    let client = f.client(u32::MAX);
    let launch = |role: Role, value: Option<&str>| {
        let mut launch = f.launch(client.clone(), role);
        if let Some(value) = value {
            launch
                .options
                .insert(HostDisplay::PARAMETER.into(), value.into());
        }
        client_display(&launch)
    };
    assert_eq!(launch(Role::Stream, None), None);
    assert_eq!(
        launch(Role::Stream, Some("physical")),
        Some(HostDisplay::Physical)
    );
    assert_eq!(
        launch(Role::Stream, Some("exclusive")),
        Some(HostDisplay::Virtual(Arrangement::Exclusive))
    );
    assert_eq!(
        launch(Role::Stream, Some("extended_primary")),
        Some(HostDisplay::Virtual(Arrangement::Primary))
    );
    for unknown in ["", "default", "mirror"] {
        assert_eq!(launch(Role::Stream, Some(unknown)), None, "{unknown}");
    }
    assert_eq!(launch(Role::RemoteMonitor, Some("physical")), None);
    assert_eq!(launch(Role::InputOnly, Some("exclusive")), None);
}
#[test]
fn legacy_virtual_display_zero_leaves_the_host_settings_in_control() {
    use butterpollo_media_policy::display_policy::Arrangement;
    let f = crate::state::test_support::Fixture::new();
    let client = f.client(u32::MAX);
    let mut launch = f.launch(client, Role::Stream);
    assert_eq!(client_display(&launch), None);
    launch.options.insert("virtualDisplay".into(), "1".into());
    assert_eq!(client_display(&launch), None);
    // Apollo-compatible clients send 0 whenever their toggle is off; the
    // host's virtual display must still apply.
    launch.options.insert("virtualDisplay".into(), "0".into());
    assert_eq!(client_display(&launch), None);
    // An explicit hostDisplay=physical still selects the physical display.
    launch
        .options
        .insert(HostDisplay::PARAMETER.into(), "physical".into());
    let physical = client_display(&launch);
    assert_eq!(physical, Some(HostDisplay::Physical));
    assert!(!HostDisplay::reusable(
        Some(HostDisplay::Virtual(Arrangement::Exclusive)),
        physical
    ));
    launch
        .options
        .insert(HostDisplay::PARAMETER.into(), "extended".into());
    assert_eq!(
        client_display(&launch),
        Some(HostDisplay::Virtual(Arrangement::Extended))
    );
    for role in [Role::RemoteMonitor, Role::InputOnly] {
        launch.role = role;
        assert_eq!(client_display(&launch), None);
    }
}
#[test]
fn virtual_display_mode_keeps_device_app_host_precedence_and_legacy_global_inheritance() {
    let config = Config::parse("virtual_display_mode=shared").unwrap();
    let mut client: butterpollo_session::state::Client =
        serde_json::from_value(serde_json::json!({"name":"Device","uuid":"device","cert":""}))
            .unwrap();
    let app = serde_json::from_value(
        serde_json::json!({"name":"Game","virtual-display-mode":"per_client"}),
    )
    .unwrap();
    assert_eq!(
        virtual_display_mode(&Config::default(), &client, None, false),
        "disabled"
    );
    assert_eq!(
        virtual_display_mode(&Config::default(), &client, None, true),
        "per_client"
    );
    assert_eq!(virtual_display_mode(&config, &client, None, true), "shared");
    assert_eq!(
        virtual_display_mode(&config, &client, Some(&app), true),
        "per_client"
    );
    client
        .extra
        .insert("virtual_display_mode".into(), "disabled".into());
    assert_eq!(
        virtual_display_mode(&config, &client, Some(&app), true),
        "disabled"
    );
    for inherited in ["global", ""] {
        client
            .extra
            .insert("virtual_display_mode".into(), inherited.into());
        assert_eq!(
            virtual_display_mode(&config, &client, Some(&app), true),
            "per_client"
        );
        assert_eq!(virtual_display_mode(&config, &client, None, true), "shared");
    }
}

#[test]
fn background_display_ignores_shared_and_app_identity_overrides() {
    let fixture = crate::state::test_support::Fixture::new();
    let mut config =
        Config::parse("virtual_display_mode=shared\nvirtual_display_layout=exclusive").unwrap();
    let mut launch = fixture.launch(fixture.client(u32::MAX), Role::Stream);
    launch.options.insert(
        "uniqueid".into(),
        "00000000-0000-0000-0000-000000000001".into(),
    );
    let app: butterpollo_session::state::App = serde_json::from_value(serde_json::json!({
        "name":"Game", "uuid":"app-display", "use-app-identity":true,
        "virtual-display-mode":"shared", "virtual-display-layout":"exclusive"
    }))
    .unwrap();
    assert_eq!(
        virtual_display_identity(&config, &launch, Some(&app)),
        "app-display"
    );
    butterpollo_session::background::apply(&mut config);
    for mode in ["shared", "disabled", "per_client"] {
        launch
            .client
            .extra
            .insert("virtual_display_mode".into(), mode.into());
        assert_eq!(
            virtual_display_mode(&config, &launch.client, Some(&app), true),
            "per_client"
        );
        assert_eq!(
            virtual_display_identity(&config, &launch, Some(&app)),
            launch.client.uuid
        );
        assert_eq!(
            virtual_display_layout(
                &config,
                &launch.client,
                Some(&app),
                Some(HostDisplay::Physical)
            ),
            butterpollo_session::background::LAYOUT
        );
    }
}

#[test]
fn a_faster_stream_request_keeps_only_a_display_that_can_follow_it() {
    // Artemis's Warp launches at 60 and asks for 120. Without a virtual
    // display of its own to raise (a physical display here), the
    // launch's preparation goes back to be prepared again at 120; one
    // made for another size never serves the stream.
    let fixture = crate::state::test_support::Fixture::new();
    let config = Config::default();
    let launched = Negotiated {
        width: 2560,
        height: 1440,
        fps: 60,
        ..Default::default()
    };
    let policy = Policy::resolve(&config, Rate(60_000), false, "none", false, false).unwrap();
    let prepared = Prepared {
        display: None,
        output: String::new(),
        framegen: policy.clone(),
        _arrangement: None,
        _activation: None,
        _profile: None,
        _retained: None,
        _vulkan: None,
        _golden: None,
        mode: stream_mode(&launched),
        client_display: None,
        virtual_layout: None,
        layout_owner: None,
        inherited_layout: None,
        resolution_owner: None,
        scale_baseline: None,
        follow: None,
        identity: None,
        joined: false,
        layout_snapshot: None,
        size_snapshot: None,
        scale_snapshot: None,
        scales_set: std::sync::Mutex::new(Vec::new()),
        revision: 0,
        machine: DisplaySessionMachine::bare(std::time::Instant::now()),
        mode_report: ModeReport::new(Arc::new(Warnings::default()), (None, None, None), 60_000),
        recovery_scale: 0,
        recovery_dimensions: (2560, 1440),
        recovery_profile: None,
        virtual_mode: false,
        profile_choice: None,
        host: Arc::downgrade(&fixture.host),
    };
    let limiter = limiter::Lease::acquire(&fixture.host.directory, &config, &policy).unwrap();
    let mut preparation = Ready::prepare((prepared, limiter)).unwrap();
    let display = preparation.display.clone();
    let launch = fixture.launch(fixture.client(u32::MAX), Role::Stream);
    for request in [
        Negotiated {
            fps: 120,
            ..launched.clone()
        },
        Negotiated {
            width: 1920,
            fps: 120,
            ..launched.clone()
        },
    ] {
        assert!(!display.matches(&request));
        preparation = retime(preparation, &fixture.host, &launch, &request, &config)
            .unwrap()
            .err()
            .expect("prepared again");
        assert_eq!(display.active.load(Ordering::Acquire), 1);
        assert_eq!(display.framegen.lock().unwrap().rate, Rate(60_000));
    }
    // The launch's own rate keeps the preparation as it is.
    assert!(display.matches(&launched));
    drop(preparation);
    assert_eq!(display.active.load(Ordering::Acquire), 0);
}

#[test]
fn vrr_negotiated_after_launch_requires_new_display_preparation() {
    let mut launch = Negotiated {
        fps: 116,
        ..Default::default()
    };
    let negotiated =
        Negotiated::from_sdp(b"a=x-nv-video[0].maxFPS:116\na=x-ss-video[0].vrrLowLatency:1\n")
            .unwrap();
    assert_eq!(negotiated.fps_millihz(), 116_000);
    assert_ne!(stream_mode(&launch), stream_mode(&negotiated));
    launch.vrr_low_latency = true;
    assert_eq!(stream_mode(&launch), stream_mode(&negotiated));
}

#[test]
fn retime_keeps_fractional_launch_rate_when_sdp_only_carries_rounded_integer_fps() {
    for (launch_rate, sdp_fps) in [(59_940, 60), (119_880, 120), (23_976, 24)] {
        let rounded = Negotiated {
            fps: sdp_fps,
            rate_millihz: 0,
            ..Default::default()
        };
        assert!(super::lifecycle::keeps_fractional_launch_rate(
            launch_rate,
            &rounded
        ));
    }
    // A genuine rate change (e.g. Warp 60 -> 120) or explicit fractional SDP rate still retimes.
    let warp = Negotiated {
        fps: 120,
        rate_millihz: 120_000,
        ..Default::default()
    };
    assert!(!super::lifecycle::keeps_fractional_launch_rate(
        59_940, &warp
    ));
    let explicit_different = Negotiated {
        fps: 60,
        rate_millihz: 59_940,
        ..Default::default()
    };
    assert!(!super::lifecycle::keeps_fractional_launch_rate(
        60_000,
        &explicit_different
    ));
}
