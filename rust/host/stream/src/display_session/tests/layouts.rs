//! Tests for virtual display layout precedence, browser-on-this-PC overrides,
//! user-set vs learned layouts, legacy migrations, and departure learning guards.
use super::*;

#[test]
fn explicit_virtual_layouts_take_precedence_over_the_legacy_primary_flag() {
    use butterpollo_media_policy::display_policy::Arrangement;
    let config = Config::default();
    let mut client: butterpollo_session::state::Client =
        serde_json::from_value(serde_json::json!({"name":"Device","uuid":"device","cert":""}))
            .unwrap();
    let mut app: butterpollo_session::state::App =
        serde_json::from_value(serde_json::json!({"name":"Game","virtual-display-primary":true}))
            .unwrap();
    assert_eq!(
        virtual_display_layout(&config, &client, None, None),
        "exclusive"
    );
    assert_eq!(
        virtual_display_layout(&config, &client, Some(&app), None),
        "extended_primary"
    );
    app.extra
        .insert("virtual-display-layout".into(), "exclusive".into());
    assert_eq!(
        virtual_display_layout(&config, &client, Some(&app), None),
        "exclusive"
    );
    client
        .extra
        .insert("virtual_display_layout".into(), "extended".into());
    assert_eq!(
        virtual_display_layout(&config, &client, Some(&app), None),
        "extended"
    );
    client
        .extra
        .insert("virtual_display_layout".into(), "exclusive".into());
    assert_eq!(
        virtual_display_layout(&config, &client, Some(&app), None),
        "exclusive"
    );
    assert_eq!(
        virtual_display_layout(
            &config,
            &client,
            Some(&app),
            Some(HostDisplay::Virtual(Arrangement::Primary))
        ),
        "extended_primary"
    );
    for inherited in ["", "global"] {
        client
            .extra
            .insert("virtual_display_layout".into(), inherited.into());
        app.extra
            .insert("virtual-display-layout".into(), inherited.into());
        assert_eq!(
            virtual_display_layout(&config, &client, Some(&app), None),
            "extended_primary"
        );
        app.extra
            .insert("virtual-display-primary".into(), false.into());
        assert_eq!(
            virtual_display_layout(&config, &client, Some(&app), None),
            "exclusive"
        );
        app.extra
            .insert("virtual-display-primary".into(), true.into());
    }
}
/// One stream to the device (the `planned` layout, `owner` as the stream sets it
/// when Windows took the layout) that ends with the stream's display in the
/// `ended` state; `during` runs while it is connected. Returns the device's
/// layout settings afterwards.
fn stream_then_disconnect(
    fixture: &crate::state::test_support::Fixture,
    client: &butterpollo_session::state::Client,
    planned: &str,
    owner: bool,
    ended: Vec<(String, bool)>,
    during: impl FnOnce(),
) -> (Option<serde_json::Value>, Option<serde_json::Value>) {
    let config = Config::parse("dd_config_revert_delay=0").unwrap();
    *fixture.host.config.write().unwrap() = config.clone();
    let mut prepared = test_prepared(&fixture.host);
    prepared.output = r"\\.\DISPLAY9".into();
    prepared.virtual_layout = Some(planned.into());
    prepared.inherited_layout = Some(planned.into());
    prepared.layout_owner = owner.then(|| client.uuid.clone());
    prepared.layout_snapshot = Some(ended);
    let limiter =
        limiter::Lease::acquire(&fixture.host.directory, &config, &prepared.framegen).unwrap();
    let stream = Ready::prepare((prepared, limiter)).unwrap();
    during();
    drop(stream);
    let paired = fixture.host.paired.read().unwrap();
    let extra = &paired.clients[0].extra;
    (
        extra.get("virtual_display_layout").cloned(),
        extra.get(LEARNED_OVER).cloned(),
    )
}
/// The stream's display is on beside the PC's, and no longer the primary.
fn windows_moved_the_primary() -> Vec<(String, bool)> {
    vec![
        (r"\\.\DISPLAY9".into(), false),
        (r"\\.\DISPLAY1".into(), true),
    ]
}
#[test]
fn a_browser_on_this_pc_teaches_its_device_no_layout() {
    // The plan and the owner come as the display setup makes them, and the stream ends with
    // Windows' primary moved, which the device learns as "extended" from any real
    // extended_primary plan.
    let stream = |config: &str, this_pc: bool| {
        let fixture = crate::state::test_support::Fixture::new();
        let client = fixture.client(u32::MAX);
        let config = Config::parse(config).unwrap();
        *fixture.host.config.write().unwrap() = config.clone();
        let mut launch = fixture.launch(client.clone(), Role::Stream);
        launch.transport = butterpollo_session::session::Transport::WebRtc;
        if this_pc {
            launch.options.insert(BROWSER_ON_THIS_PC.into(), "1".into());
        }
        let configured = virtual_display_layout(&config, &client, None, None).to_owned();
        let planned = launch_display_layout(&config, &launch, None, None).to_owned();
        let monitors_kept = browser_keeps_monitors(&launch, &configured, true);
        let mut prepared = test_prepared(&fixture.host);
        prepared.output = r"\\.\DISPLAY9".into();
        prepared.virtual_layout = Some(planned.clone());
        prepared.inherited_layout = Some(configured);
        prepared.layout_owner = layout_owner(&launch, true, monitors_kept, None, None);
        prepared.layout_snapshot = Some(windows_moved_the_primary());
        let limiter =
            limiter::Lease::acquire(&fixture.host.directory, &config, &prepared.framegen).unwrap();
        drop(Ready::prepare((prepared, limiter)).unwrap());
        let paired = fixture.host.paired.read().unwrap();
        let extra = &paired.clients[0].extra;
        (
            planned,
            extra.get("virtual_display_layout").cloned(),
            extra.get(LEARNED_OVER).cloned(),
        )
    };
    assert_eq!(
        stream("dd_config_revert_delay=0", true),
        ("extended_primary".to_owned(), None, None)
    );
    // The same plan set by the user, from the same browser, and from elsewhere: Windows
    // moving the primary back at the end is nobody's choice either.
    for this_pc in [true, false] {
        assert_eq!(
            stream(
                "dd_config_revert_delay=0\nvirtual_display_layout=extended_primary",
                this_pc
            ),
            ("extended_primary".to_owned(), None, None)
        );
    }
}
#[test]
fn the_log_says_why_a_browser_on_this_pc_kept_its_monitors() {
    let reason = layout_reason(true, "ensure_only_display").unwrap();
    assert!(reason.contains("browser runs on this PC"), "{reason}");
    assert!(reason.contains("`ensure_only_display`"), "{reason}");
    assert_eq!(layout_reason(false, "exclusive"), None);
}
fn set_device_layout(fixture: &crate::state::test_support::Fixture, layout: &str) {
    let mut paired = fixture.host.paired.write().unwrap();
    paired.clients[0]
        .extra
        .insert("virtual_display_layout".into(), layout.into());
    paired.clients[0]
        .extra
        .insert(SET_BY_USER.into(), serde_json::Value::Bool(true));
    paired.clients[0].extra.remove(LEARNED_OVER);
}
#[test]
fn a_device_layout_the_user_set_survives_every_stream() {
    // Issue #40: the layout set under Devices was replaced by "extended" when
    // a later stream ended with Windows' primary elsewhere.
    for layout in ["exclusive", "extended_primary", "extended_isolated"] {
        let fixture = crate::state::test_support::Fixture::new();
        let client = fixture.client(u32::MAX);
        set_device_layout(&fixture, layout);
        for _stream in 0..3 {
            let (kept, over) = stream_then_disconnect(
                &fixture,
                &client,
                "extended_primary",
                true,
                windows_moved_the_primary(),
                || {},
            );
            assert_eq!(kept, Some(serde_json::json!(layout)));
            assert_eq!(over, None);
        }
    }
}
#[test]
fn a_device_layout_the_user_sets_during_a_stream_is_not_replaced_at_disconnect() {
    let fixture = crate::state::test_support::Fixture::new();
    let client = fixture.client(u32::MAX);
    let (kept, over) = stream_then_disconnect(
        &fixture,
        &client,
        "extended_primary",
        true,
        windows_moved_the_primary(),
        || set_device_layout(&fixture, "exclusive"),
    );
    assert_eq!(kept, Some(serde_json::json!("exclusive")));
    assert_eq!(over, None);
}
#[test]
fn a_device_layout_the_user_set_survives_a_reconnect() {
    let fixture = crate::state::test_support::Fixture::new();
    let client = fixture.client(u32::MAX);
    set_device_layout(&fixture, "extended_primary");
    let config = Config::parse("dd_config_revert_delay=0").unwrap();
    *fixture.host.config.write().unwrap() = config.clone();
    let mut prepared = test_prepared(&fixture.host);
    prepared.output = r"\\.\DISPLAY9".into();
    prepared.virtual_layout = Some("extended_primary".into());
    prepared.layout_owner = Some(client.uuid.clone());
    prepared.layout_snapshot = Some(windows_moved_the_primary());
    let limiter =
        limiter::Lease::acquire(&fixture.host.directory, &config, &prepared.framegen).unwrap();
    let first = Ready::prepare((prepared, limiter)).unwrap();
    let second = first
        .display
        .clone()
        .resume(
            &fixture.host.directory,
            &config,
            Arc::new(Warnings::default()),
        )
        .unwrap();
    drop(first);
    drop(second);
    let paired = fixture.host.paired.read().unwrap();
    let extra = &paired.clients[0].extra;
    assert_eq!(
        extra.get("virtual_display_layout"),
        Some(&serde_json::json!("extended_primary"))
    );
    assert_eq!(extra.get(LEARNED_OVER), None);
}
#[test]
fn a_layout_windows_refused_is_not_learned() {
    // The stream sets no layout owner when Windows did not take the layout.
    let fixture = crate::state::test_support::Fixture::new();
    let client = fixture.client(u32::MAX);
    let (kept, over) = stream_then_disconnect(
        &fixture,
        &client,
        "extended_primary",
        false,
        windows_moved_the_primary(),
        || {},
    );
    assert_eq!((kept, over), (None, None));
}
#[test]
fn a_learned_device_layout_is_replaced_but_a_chosen_one_is_not_learned_over() {
    let fixture = crate::state::test_support::Fixture::new();
    let client = fixture.client(u32::MAX);
    // No device layout: the "Show only on 2" the user left the stream in is learned.
    let only = || vec![(r"\\.\DISPLAY9".to_string(), true)];
    let (kept, over) = stream_then_disconnect(&fixture, &client, "extended", true, only(), || {});
    assert_eq!(kept, Some(serde_json::json!("exclusive")));
    assert_eq!(over, Some(serde_json::json!("extended")));
    // "global" is no layout of its own.
    set_device_layout(&fixture, "global");
    let (kept, _) = stream_then_disconnect(&fixture, &client, "extended", true, only(), || {});
    assert_eq!(kept, Some(serde_json::json!("exclusive")));
    // A learned layout follows what the user leaves the next stream in.
    let primary = vec![
        (r"\\.\DISPLAY9".to_string(), true),
        (r"\\.\DISPLAY1".to_string(), false),
    ];
    let (kept, over) = stream_then_disconnect(&fixture, &client, "extended", true, primary, || {});
    assert_eq!(kept, Some(serde_json::json!("extended_primary")));
    assert_eq!(over, Some(serde_json::json!("extended")));
}
#[test]
fn a_layout_saved_before_the_user_marker_existed_stays_learnable() {
    // Learning shipped before the marker: such a layout has neither marker
    // and must not be taken for the user's.
    let fixture = crate::state::test_support::Fixture::new();
    let client = fixture.client(u32::MAX);
    fixture.host.paired.write().unwrap().clients[0]
        .extra
        .insert("virtual_display_layout".into(), "extended".into());
    let (kept, over) = stream_then_disconnect(
        &fixture,
        &client,
        "extended_primary",
        true,
        vec![(r"\\.\DISPLAY9".into(), true)],
        || {},
    );
    assert_eq!(kept, Some(serde_json::json!("exclusive")));
    assert_eq!(over, Some(serde_json::json!("extended_primary")));
}
#[test]
fn a_learned_device_layout_yields_when_the_host_layout_changes() {
    // Issue #40: "exclusive" was learned over the host's "extended"; switching
    // the host to "extended_primary" must take effect.
    let mut client: butterpollo_session::state::Client =
        serde_json::from_value(serde_json::json!({"name":"Device","uuid":"device","cert":""}))
            .unwrap();
    client
        .extra
        .insert("virtual_display_layout".into(), "exclusive".into());
    client.extra.insert(LEARNED_OVER.into(), "extended".into());
    let extended = Config::parse("virtual_display_layout=extended").unwrap();
    let primary = Config::parse("virtual_display_layout=extended_primary").unwrap();
    assert_eq!(
        virtual_display_layout(&extended, &client, None, None),
        "exclusive"
    );
    assert_eq!(
        virtual_display_layout(&primary, &client, None, None),
        "extended_primary"
    );

    // A layout the user set for the device has no marker and always applies.
    client.extra.remove(LEARNED_OVER);
    assert_eq!(
        virtual_display_layout(&primary, &client, None, None),
        "exclusive"
    );
}
#[test]
fn a_legacy_extended_layout_stays_on_an_exclusive_host_and_still_applies() {
    // A device layout from 2.4.1-2.5.9 (a stream learned it) or from before 3.0.0 (the user
    // picked Extend in Devices) looks the same: neither marker exists. On an exclusive host
    // it must stay as it is, or the upgrade would turn the user's monitors off.
    let mut clients = vec![legacy_extended()];
    let exclusive = Config::parse("virtual_display_layout=exclusive").unwrap();
    assert_eq!(
        migrate_legacy_extended_layouts(&mut clients, "exclusive"),
        ["Device".to_owned()]
    );
    assert_eq!(
        clients[0].extra["virtual_display_layout"],
        serde_json::json!("extended")
    );
    assert_eq!(
        clients[0].extra[LEARNED_OVER],
        serde_json::json!("exclusive")
    );
    assert_eq!(
        virtual_display_layout(&exclusive, &clients[0], None, None),
        "extended"
    );
    // It follows the host from then on: an extended_primary host wins over the kept layout.
    let primary = Config::parse("virtual_display_layout=extended_primary").unwrap();
    assert_eq!(
        virtual_display_layout(&primary, &clients[0], None, None),
        "extended_primary"
    );
}

#[test]
fn a_legacy_extended_layout_yields_when_the_host_asks_for_the_primary() {
    // The host asks for the primary: the learned value is the likely source (issue #33), so
    // the device follows the host layout again.
    for host in ["extended_primary", "extended_primary_isolated"] {
        let mut clients = vec![legacy_extended()];
        assert_eq!(
            migrate_legacy_extended_layouts(&mut clients, host),
            ["Device".to_owned()],
            "{host}"
        );
        assert!(!clients[0].extra.contains_key("virtual_display_layout"));
        let config = Config::parse(&format!("virtual_display_layout={host}")).unwrap();
        assert_eq!(
            virtual_display_layout(&config, &clients[0], None, None),
            host
        );
    }
}

#[test]
fn an_app_that_asks_for_the_primary_gets_it_while_a_legacy_layout_stays() {
    let exclusive = Config::parse("virtual_display_layout=exclusive").unwrap();
    let primary: butterpollo_session::state::App = serde_json::from_value(
        serde_json::json!({"name":"Game","virtual-display-layout":"extended_primary"}),
    )
    .unwrap();
    let plain: butterpollo_session::state::App =
        serde_json::from_value(serde_json::json!({"name":"Desktop"})).unwrap();
    // Only an app asks for the primary: the device keeps its layout, learned over the host's,
    // so that app gets the main screen while the host and the other apps keep "extended".
    let mut clients = vec![legacy_extended()];
    assert_eq!(
        migrate_legacy_extended_layouts(&mut clients, "exclusive"),
        ["Device".to_owned()]
    );
    assert_eq!(
        clients[0].extra["virtual_display_layout"],
        serde_json::json!("extended")
    );
    assert_eq!(
        clients[0].extra[LEARNED_OVER],
        serde_json::json!("exclusive")
    );
    assert_eq!(
        virtual_display_layout(&exclusive, &clients[0], None, None),
        "extended"
    );
    assert_eq!(
        virtual_display_layout(&exclusive, &clients[0], Some(&plain), None),
        "extended"
    );
    assert_eq!(
        virtual_display_layout(&exclusive, &clients[0], Some(&primary), None),
        "extended_primary"
    );
}

#[test]
fn a_picked_or_already_marked_legacy_layout_is_left_alone() {
    let mut picked = legacy_extended();
    picked
        .extra
        .insert(SET_BY_USER.into(), serde_json::json!(true));
    let mut marked = legacy_extended();
    marked
        .extra
        .insert(LEARNED_OVER.into(), serde_json::json!("exclusive"));
    let mut clients = vec![picked, marked];
    assert!(migrate_legacy_extended_layouts(&mut clients, "extended_primary").is_empty());
    assert_eq!(
        clients[0].extra["virtual_display_layout"],
        serde_json::json!("extended")
    );
    assert_eq!(
        clients[1].extra[LEARNED_OVER],
        serde_json::json!("exclusive")
    );
}

#[test]
fn the_legacy_layout_notice_says_which_branch_happened() {
    let devices = ["Steam Deck".to_owned(), "Phone".to_owned()];
    // Kept: the host does not ask for the primary, so the device keeps Extend as learned.
    let kept = legacy_layout_notice(&devices, "exclusive");
    assert!(kept.starts_with("Steam Deck, Phone keep Extend"), "{kept}");
    assert!(kept.contains("marked as learned"), "{kept}");
    assert!(kept.contains("under Devices"), "{kept}");
    // Removed: the host asks for the primary, so the device follows the host layout.
    assert_eq!(
        legacy_layout_notice(&devices, "extended_primary"),
        "Steam Deck, Phone follow the host's display layout again."
    );
}

#[test]
fn the_legacy_host_layout_reads_the_host_setting_or_the_default() {
    assert_eq!(legacy_host_layout(&Config::default()), "exclusive");
    assert_eq!(
        legacy_host_layout(&Config::parse("virtual_display_layout=extended_primary").unwrap()),
        "extended_primary"
    );
}

fn legacy_extended() -> butterpollo_session::state::Client {
    let mut client: butterpollo_session::state::Client =
        serde_json::from_value(serde_json::json!({"name":"Device","uuid":"device","cert":""}))
            .unwrap();
    client
        .extra
        .insert("virtual_display_layout".into(), "extended".into());
    client
}

/// What a device keeps from a stream that planned `extended` and ends with only its own
/// display on, as `set_up` has it.
fn learned_alone(set_up: impl FnOnce(&mut Prepared, &Launch)) -> Option<serde_json::Value> {
    let fixture = crate::state::test_support::Fixture::new();
    let client = fixture.client(u32::MAX);
    let config = Config::parse("dd_config_revert_delay=0").unwrap();
    *fixture.host.config.write().unwrap() = config.clone();
    let launch = fixture.launch(client, Role::Stream);
    let mut prepared = test_prepared(&fixture.host);
    prepared.output = r"\\.\DISPLAY9".into();
    prepared.layout_snapshot = Some(vec![(prepared.output.clone(), true)]);
    set_up(&mut prepared, &launch);
    let limiter =
        limiter::Lease::acquire(&fixture.host.directory, &config, &prepared.framegen).unwrap();
    drop(Ready::prepare((prepared, limiter)).unwrap());
    fixture.host.paired.read().unwrap().clients[0]
        .extra
        .get("virtual_display_layout")
        .cloned()
}
fn extended_under(in_force: Arrangement, prepared: &mut Prepared, launch: &Launch) {
    prepared.layout_owner = layout_owner(
        launch,
        true,
        false,
        Some(Arrangement::Extended),
        Some(in_force),
    );
}

#[test]
fn every_stream_sees_a_monitors_return_had_displays_put_back_once() {
    let (mut one, mut other) = (3, 3);
    assert!(!wake_layout_passed(&mut one, 3));
    assert!(wake_layout_passed(&mut one, 4));
    assert!(!wake_layout_passed(&mut one, 4));
    assert!(wake_layout_passed(&mut other, 4));
}
#[test]
fn a_stream_that_began_as_a_monitor_woke_learns_nothing_from_how_it_ends() {
    // Its display alone with the monitor away reads just like the user's "Show only on 2".
    let alone = || learned_alone(|p, l| extended_under(Arrangement::Extended, p, l));
    assert_eq!(alone(), Some(serde_json::json!("exclusive")));
    BEGAN_AS_A_MONITOR_WOKE.with(|began| began.set(true));
    let learned = alone();
    BEGAN_AS_A_MONITOR_WOKE.with(|began| began.set(false));
    assert_eq!(learned, None);
}

#[test]
fn a_stream_on_another_streams_layout_learns_nothing_from_how_that_ends() {
    // A owned exclusive and left: B's display is alone, the monitor still off.
    assert_eq!(
        learned_alone(|p, l| extended_under(Arrangement::Exclusive, p, l)),
        None
    );
    // A owned extended_primary: the same read would teach B that.
    assert_eq!(
        learned_alone(|p, l| {
            extended_under(Arrangement::Primary, p, l);
            p.layout_snapshot = Some(vec![(p.output.clone(), true), ("monitor".into(), false)]);
        }),
        None
    );
    // Issue 33: on its own layout, the user's "Show only on 2" is still learned.
    assert_eq!(
        learned_alone(|p, l| extended_under(Arrangement::Extended, p, l)),
        Some(serde_json::json!("exclusive"))
    );
}

#[test]
fn a_layout_read_while_a_departure_is_unsettled_is_not_learned() {
    // Another display left and this stream has not looked yet.
    assert_eq!(
        learned_alone(|p, l| {
            extended_under(Arrangement::Extended, p, l);
            p.machine.departures = display_arrangement::departures().wrapping_add(1);
        }),
        None
    );
    // It looked, and the repair is still to come.
    assert_eq!(
        learned_alone(|p, l| {
            extended_under(Arrangement::Extended, p, l);
            p.machine
                .layout_watch
                .display_left(std::time::Instant::now());
        }),
        None
    );
    // Once that ran, what the stream ends with is the user's again.
    assert_eq!(
        learned_alone(|p, l| {
            extended_under(Arrangement::Extended, p, l);
            p.machine
                .layout_watch
                .display_left(std::time::Instant::now());
            p.machine.layout_watch.departure_handled();
        }),
        Some(serde_json::json!("exclusive"))
    );
}
