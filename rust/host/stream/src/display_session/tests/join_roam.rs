//! Tests for stream joining, roaming ownership, dual-screen isolation, resume,
//! HDR fallback preservation, and joined stream retiming.
use super::*;

#[test]
fn a_stream_being_prepared_stops_the_watch_before_anything_else() {
    use crate::state::test_support::Fixture;
    let f = Fixture::new();
    let launch = f.launch(f.client(u32::MAX), Role::Stream);
    let mut config = Config::default();
    // Refused at the first look at the settings, so nothing here touches a display.
    config
        .values
        .insert("output_name".into(), "window:nonsense".into());
    WAKE_WATCH_HOLDS.with(|holds| holds.set(0));
    assert!(prepare_stream(&f.host, &launch, &Negotiated::default(), &config).is_err());
    assert_eq!(WAKE_WATCH_HOLDS.with(Cell::get), 1);
}

#[test]
fn stream_here_too_shares_the_display_the_game_is_on() {
    assert_eq!(
        joined_display(&join_ask(), joined_game(Some("a"))).as_deref(),
        Some("a")
    );
    // Another app, nothing running, a background game, a remote monitor.
    let refused = |ask: JoinAsk| assert_eq!(joined_display(&ask, joined_game(Some("a"))), None);
    refused(JoinAsk {
        launched: Some(8),
        ..join_ask()
    });
    assert_eq!(joined_display(&join_ask(), None), None);
    assert_eq!(
        joined_display(
            &join_ask(),
            Some(JoinedGame {
                background: true,
                ..joined_game(Some("a")).unwrap()
            })
        ),
        None
    );
    refused(JoinAsk {
        role: Role::RemoteMonitor,
        ..join_ask()
    });
    // A press: the game's display goes with the previous owner.
    refused(JoinAsk {
        claim_live: true,
        ..join_ask()
    });
    // The device streams a physical display, or its settings pick its display.
    refused(JoinAsk {
        virtual_mode: false,
        ..join_ask()
    });
    refused(JoinAsk {
        display_mode: "shared",
        ..join_ask()
    });
    refused(JoinAsk {
        identity: "an app's",
        ..join_ask()
    });
}
/// `b`'s `/resume` of game 7, under per-client virtual displays.
fn join_ask<'a>() -> JoinAsk<'a> {
    JoinAsk {
        client: "b",
        role: Role::Stream,
        resumes: true,
        launched: Some(7),
        virtual_mode: true,
        display_mode: "per_client",
        identity: "b",
        claim_live: false,
    }
}
fn joined_game(display: Option<&str>) -> Option<JoinedGame<'_>> {
    Some(JoinedGame {
        id: 7,
        display,
        background: false,
        desktop: false,
    })
}

#[test]
fn a_devices_second_screen_never_joins_the_game_it_resumes() {
    // Dual screen mode on the AYN Thor: the second screen resumed the game and was put on
    // the first screen's display, a mirror of it.
    let second = butterpollo_session::state::second_screen_uuid("b");
    let ask = JoinAsk {
        client: &second,
        identity: &second,
        ..join_ask()
    };
    assert_eq!(joined_display(&ask, joined_game(Some("b"))), None);
    assert_eq!(joined_display(&ask, joined_game(Some(&second))), None);
    // The device's first screen still joins as before.
    assert_eq!(
        joined_display(&join_ask(), joined_game(Some("a"))).as_deref(),
        Some("a")
    );
}

#[test]
fn a_second_screen_on_physical_displays_shows_another_monitor() {
    let monitors = [
        (r"\\.\DISPLAY1", false),
        (r"\\.\DISPLAY7", true),
        (r"\\.\DISPLAY2", false),
    ];
    // Not the first screen's monitor, and never a virtual display.
    assert_eq!(
        pick_second_monitor(&monitors, Some(r"\\.\DISPLAY1")),
        Some(r"\\.\DISPLAY2")
    );
    assert_eq!(
        pick_second_monitor(&monitors, Some(r"\\.\display2")),
        Some(r"\\.\DISPLAY1")
    );
    // One monitor: none, both screens show it.
    assert_eq!(
        pick_second_monitor(&[(r"\\.\DISPLAY1", false)], Some(r"\\.\DISPLAY1")),
        None
    );
    assert_eq!(
        pick_second_monitor(
            &[(r"\\.\DISPLAY1", false), (r"\\.\DISPLAY7", true)],
            Some(r"\\.\DISPLAY1")
        ),
        None
    );
    assert!(is_second_screen(
        &butterpollo_session::state::second_screen_uuid("b")
    ));
    assert!(!is_second_screen("b"));
}

#[test]
fn a_plain_second_stream_and_desktop_keep_displays_of_their_own() {
    let plain = JoinAsk {
        resumes: false,
        ..join_ask()
    };
    assert_eq!(joined_display(&plain, joined_game(Some("a"))), None);
    let desktop = || {
        Some(JoinedGame {
            desktop: true,
            ..joined_game(Some("a")).unwrap()
        })
    };
    assert_eq!(joined_display(&plain, desktop()), None);
    // Not when it asks either, and not the device whose display that is.
    assert_eq!(joined_display(&join_ask(), desktop()), None);
    let returning = JoinAsk {
        client: "a",
        identity: "a",
        ..join_ask()
    };
    assert_eq!(joined_display(&returning, desktop()), None);
}

#[test]
fn a_game_no_stream_showed_gives_a_join_no_display_to_make() {
    // Started from the console (its owner is ""), or with its owner on a physical
    // monitor: an empty display under the owner's id would show nothing.
    assert_eq!(joined_display(&join_ask(), joined_game(None)), None);
    assert_eq!(game_display_after(None, false, true, false, None), None);
    // A stream beside it stays beside it, and does not become where the game is.
    assert_eq!(
        game_display_after(None, false, false, false, Some("b")),
        None
    );
}

#[test]
fn a_press_moves_the_owner_and_a_join_still_finds_the_display_the_game_is_on() {
    // `b` joined `a`'s display and pressed: `b` owns the game, on the display under
    // `a`'s identity. Nothing asks for the owner here, only for the display.
    let after_press = game_display_after(Some("a"), true, true, true, Some("a"));
    assert_eq!(after_press.as_deref(), Some("a"));
    let third = JoinAsk {
        client: "c",
        identity: "c",
        ..join_ask()
    };
    assert_eq!(
        joined_display(&third, joined_game(after_press.as_deref())).as_deref(),
        Some("a")
    );
    // `a` coming back, however it asks, finds it too instead of a mode conflict.
    let returning = JoinAsk {
        client: "a",
        identity: "a",
        resumes: false,
        ..join_ask()
    };
    assert_eq!(
        joined_display(&returning, joined_game(after_press.as_deref())).as_deref(),
        Some("a")
    );
    // A press from a device with a display of its own: the game follows it there.
    assert_eq!(
        game_display_after(Some("a"), true, true, false, Some("b")).as_deref(),
        Some("b")
    );
}

#[test]
fn the_games_display_follows_its_owner_and_outlives_only_while_it_is_live() {
    // A launch, on a virtual display or a physical one.
    assert_eq!(
        game_display_after(None, false, true, false, Some("a")).as_deref(),
        Some("a")
    );
    assert_eq!(game_display_after(Some("a"), true, true, false, None), None);
    // A second stream on a display of its own changes nothing.
    assert_eq!(
        game_display_after(Some("a"), true, false, false, Some("b")).as_deref(),
        Some("a")
    );
    // Unless the game's display is gone: what is left is where Windows moved it.
    assert_eq!(
        game_display_after(Some("a"), false, false, false, Some("b")).as_deref(),
        Some("b")
    );
    // An owner that joined streams the display the game is on, and moves nothing.
    assert_eq!(
        game_display_after(Some("a"), true, true, true, Some("b")).as_deref(),
        Some("a")
    );
}

#[test]
fn a_joined_stream_leaves_the_displays_hdr_scale_and_mode_alone() {
    assert_eq!(
        display_changes(false, true, true),
        DisplayChanges {
            hdr_profile: true,
            scale: true,
            mode: true
        }
    );
    assert_eq!(
        display_changes(true, true, true),
        DisplayChanges {
            hdr_profile: false,
            scale: false,
            mode: false
        }
    );
    // An SDR stream of a physical display, as ever.
    assert_eq!(
        display_changes(false, false, false),
        DisplayChanges {
            hdr_profile: false,
            scale: false,
            mode: true
        }
    );
}

#[test]
fn only_a_joined_stream_with_other_hdr_than_its_display_is_logged() {
    assert_eq!(
        joined_hdr_mismatch(true, Some(true), Some(false)),
        Some(true)
    );
    assert_eq!(
        joined_hdr_mismatch(true, Some(false), Some(true)),
        Some(false)
    );
    assert_eq!(joined_hdr_mismatch(true, Some(true), Some(true)), None);
    assert_eq!(joined_hdr_mismatch(true, None, Some(true)), None);
    assert_eq!(joined_hdr_mismatch(true, Some(true), None), None);
    assert_eq!(joined_hdr_mismatch(false, Some(true), Some(false)), None);
}

#[test]
fn only_an_active_stream_of_its_own_brings_the_game_back() {
    assert!(brings_back_game(false, true));
    // A remote monitor's display is someone else's to look at.
    assert!(!brings_back_game(true, true));
    // A paused stream holding its layout has nobody to show the game to.
    assert!(!brings_back_game(false, false));
}

#[test]
fn a_resume_leaves_its_own_kept_display_for_the_one_the_game_is_on() {
    let fixture = crate::state::test_support::Fixture::new();
    let host = &fixture.host;
    let mut config = host.config.read().unwrap().clone();
    config
        .values
        .insert("virtual_display_mode".into(), "per_client".into());
    let mut app = host.apps.read().unwrap()[0].clone();
    app.cmd = "game.exe".into();
    let mut running =
        crate::process::RunningApp::with_environment(&app, Default::default()).unwrap();
    running.owner = "a".into();
    running.display = Some("a".into());
    *host.current_app.lock().unwrap() = Some(running);
    let client = fixture.client(u32::MAX);
    let plain = fixture.launch(client.clone(), Role::Stream);
    let mut resume = fixture.launch(client.clone(), Role::Stream);
    resume.options.insert(RESUMES_GAME.into(), "1".into());
    let keep = |identity: &str, joined: bool| {
        host.app_display.lock().unwrap().insert(
            client.uuid.clone(),
            (retained_on(host, &config, identity, joined), None),
        );
    };
    let take = |launch: &Launch, serves: bool, live: bool| {
        take_retained_display(host, launch, &config, Some(&app), |_| serves, |_| live)
    };
    // It tapped the tile, saw an empty desktop and left. A launch comes back to that.
    keep(&client.uuid, false);
    assert!(take(&plain, true, true).is_ok());
    // Stream here too in the same mode must not: it is handed back to be dropped, and
    // the setup that follows joins the game's display.
    assert!(matches!(take(&resume, true, true), Err(Some(_))));
    assert!(host.app_display.lock().unwrap().is_empty());
    // With the game's display gone it has nothing better.
    keep(&client.uuid, false);
    assert!(take(&resume, true, false).is_ok());
    // A display in another mode goes as ever.
    assert!(matches!(take(&resume, false, false), Err(Some(_))));
    // Already on the game's display, joined earlier.
    keep("a", true);
    assert!(take(&resume, true, true).is_ok());
    host.app_display.lock().unwrap().clear();
}

#[test]
fn a_layout_windows_refused_after_idle_is_tried_once_more_unless_locked() {
    let run = |failures: usize, refused: bool, secure: bool| {
        let (mut attempts, mut waited) = (0, false);
        let result = retry_refused(
            || {
                attempts += 1;
                if attempts <= failures {
                    anyhow::bail!("access denied")
                }
                Ok(attempts)
            },
            |_| refused,
            || secure,
            || waited = true,
        );
        (result.is_ok(), attempts, waited)
    };
    // A resume, as a launch, gets its second try after the wait.
    assert_eq!(run(1, true, false), (true, 2, true));
    // Only one: a second refusal fails it.
    assert_eq!(run(2, true, false), (false, 2, true));
    // The lock screen has its own way; another error is no refusal.
    assert_eq!(run(1, true, true), (false, 1, false));
    assert_eq!(run(1, false, false), (false, 1, false));
    assert_eq!(run(0, true, false), (true, 1, false));
}

#[test]
fn a_resume_takes_the_frame_limiter_and_hdr_profile_changed_while_paused() {
    let fixture = crate::state::test_support::Fixture::new();
    let host = &fixture.host;
    let kept = retained_on(host, &Config::default(), "a", false);
    assert!(!kept.framegen.lock().unwrap().enabled);
    let launch = fixture.launch(fixture.client(u32::MAX), Role::Stream);
    let changed = Config::parse("frame_limiter_enable=true").unwrap();
    kept.follow_settings(None, &launch, &Negotiated::default(), &changed)
        .unwrap();
    assert!(
        kept.framegen.lock().unwrap().enabled,
        "the limiter's policy"
    );
    assert!(kept._state.lock().unwrap().framegen.enabled);
    // The device's profile: chosen, changed or taken off while paused; only for an HDR
    // stream on a display of its own.
    assert_eq!(resumed_profile(true, None, None), None);
    assert_eq!(resumed_profile(true, Some("tv"), Some("tv")), None);
    assert_eq!(
        resumed_profile(true, None, Some("tv")),
        Some(Some("tv".into()))
    );
    assert_eq!(
        resumed_profile(true, Some("tv"), Some("oled")),
        Some(Some("oled".into()))
    );
    assert_eq!(resumed_profile(true, Some("tv"), None), Some(None));
    assert_eq!(resumed_profile(false, None, Some("tv")), None);
}

#[test]
fn a_resume_at_another_rate_keeps_the_display_the_game_is_on() {
    let fixture = crate::state::test_support::Fixture::new();
    let host = &fixture.host;
    let config = host.config.read().unwrap().clone();
    let client = fixture.client(u32::MAX);
    let launch = fixture.launch(client.clone(), Role::Stream);
    let kept = retained_on(host, &config, &client.uuid, false);
    host.app_display
        .lock()
        .unwrap()
        .insert(client.uuid.clone(), (kept.clone(), None));
    let launched = Negotiated::default();
    let faster = Negotiated {
        fps: 240,
        rate_millihz: 240_000,
        ..launched.clone()
    };
    assert!(!kept.matches(&faster));
    // The launch's own filter: the same display comes back, for the stream to retime.
    let Ok(resumed) = take_retained_display(
        host,
        &launch,
        &config,
        None,
        |lease| lease.resumes(&faster),
        |_| false,
    ) else {
        panic!("the kept display was not handed back");
    };
    assert!(
        Arc::ptr_eq(&resumed, &kept),
        "the kept display, not a new one"
    );
    assert_eq!(host.app_display.lock().unwrap().len(), 1);
    for other in [
        Negotiated {
            width: launched.width + 2,
            ..faster.clone()
        },
        Negotiated {
            hdr: !launched.hdr,
            ..faster.clone()
        },
        Negotiated {
            vrr_low_latency: !launched.vrr_low_latency,
            ..faster.clone()
        },
    ] {
        assert!(!kept.resumes(&other), "{other:?}");
    }
    host.app_display.lock().unwrap().clear();
}

#[test]
fn a_joined_stream_asks_for_the_display_as_it_is_whatever_its_own_size() {
    assert_eq!(
        display_ask(
            Some((2560, 1440, 120_000)),
            (1280, 720),
            Rate(60_000),
            Some(true)
        ),
        ((2560, 1440), Rate(120_000), None)
    );
    assert_eq!(
        display_ask(None, (1280, 720), Rate(60_000), Some(true)),
        ((1280, 720), Rate(60_000), Some(true))
    );
}

#[test]
fn a_joined_stream_hears_of_a_slower_or_other_hdr_display_and_nothing_fails() {
    let warnings = Arc::new(Warnings::default());
    let report = ModeReport::new(
        warnings.clone(),
        joined_report(Rate(120_000), Some(true)),
        120_000,
    )
    .shared();
    // Another size is what the encoder is for; 60 Hz and SDR are not what it asked.
    report.publish(Ok((2560, 1440, 60_000, false)));
    let found = codes(&warnings);
    assert!(
        found.iter().any(|code| code == "display_refresh"),
        "{found:?}"
    );
    assert!(found.iter().any(|code| code == "display_hdr"), "{found:?}");
    assert!(
        !found.iter().any(|code| code == "display_resolution"),
        "{found:?}"
    );
    report.publish(Ok((2560, 1440, 240_000, true)));
    assert_eq!(codes(&warnings), Vec::<String>::new());
}

#[test]
fn hdr_fallback_preserves_display_hdr_warning_across_mode_reports() {
    let warnings = Arc::new(Warnings::default());
    let report = ModeReport::new(
        warnings.clone(),
        joined_report(Rate(144_000), Some(true)),
        144_000,
    );
    report.publish(Ok((2560, 1440, 144_000, false)));
    let fallback = "Requested display HDR state was not applied; capture source HDR is off, so this stream fell back to SDR. Check Windows HDR and the display policy, or reconnect with HDR disabled.";
    warnings.set("display_hdr", fallback);
    report.hdr_fell_back();

    // Subsequent mode reports (e.g. primary/follow updates) must not overwrite or clear it.
    report.publish(Ok((2560, 1440, 144_000, true)));
    let snap = warnings.snapshot();
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].code, "display_hdr");
    assert_eq!(snap[0].message, fallback);

    // Attaching a fresh stream's warnings resets the fallback latch.
    let resumed_warnings = Arc::new(Warnings::default());
    report.attach(resumed_warnings.clone());
    assert!(resumed_warnings.snapshot().is_empty());
}

#[test]
fn a_joined_stream_at_another_rate_keeps_its_place_and_leaves_the_display() {
    let fixture = crate::state::test_support::Fixture::new();
    let config = Config::default();
    let mut prepared = test_prepared(&fixture.host);
    prepared.joined = true;
    let policy = prepared.framegen.clone();
    let limiter = limiter::Lease::acquire(&fixture.host.directory, &config, &policy).unwrap();
    let preparation = Ready::prepare((prepared, limiter)).unwrap();
    let display = preparation.display.clone();
    let launch = fixture.launch(fixture.client(u32::MAX), Role::Stream);
    let request = Negotiated {
        fps: 120,
        ..Default::default()
    };
    // Nothing to raise and nothing to prepare again: it stays on the game's display.
    let preparation = retime(preparation, &fixture.host, &launch, &request, &config)
        .unwrap()
        .ok()
        .expect("kept");
    assert_eq!(display.active.load(Ordering::Acquire), 1);
    drop(preparation);
}
