//! Tests for remembered layout/resolution/scale persistence, pause-time layout
//! holding, disconnect reaping, and native paused display resume.
use super::*;

#[test]
fn only_an_opted_in_exclusive_layout_is_held_while_paused() {
    let on = Config::parse("dd_hold_layout_while_paused=true").unwrap();
    // Issue #56: off by default, so a paused stream leaves Win+P alone.
    assert!(!holds_layout_while_paused(&Config::default(), true));
    assert!(holds_layout_while_paused(&on, true));
    // Extended layouts never switched a monitor off; there is nothing to hold.
    assert!(!holds_layout_while_paused(&on, false));
    // Held or not, a paused stream with no layout to guard still renews its lease.
    let fixture = crate::state::test_support::Fixture::new();
    let mut paused = test_prepared(&fixture.host);
    paused.machine.hold_layout_while_paused = true;
    paused.feed(false).unwrap();
}

#[test]
fn layout_preferences_are_saved_at_disconnect_and_not_relearned_while_paused() {
    let fixture = crate::state::test_support::Fixture::new();
    let client = fixture.client(u32::MAX);
    let config = Config::parse("dd_config_revert_delay=0").unwrap();
    *fixture.host.config.write().unwrap() = config.clone();
    let mut prepared = test_prepared(&fixture.host);
    prepared.output = r"\\.\DISPLAY9".into();
    prepared.layout_owner = Some(client.uuid.clone());
    prepared.layout_snapshot = Some(vec![(prepared.output.clone(), true)]);
    let limiter =
        limiter::Lease::acquire(&fixture.host.directory, &config, &prepared.framegen).unwrap();
    let first = Ready::prepare((prepared, limiter)).unwrap();
    let display = first.display.clone();
    let second = display
        .clone()
        .resume(
            &fixture.host.directory,
            &config,
            Arc::new(Warnings::default()),
        )
        .unwrap();
    let chosen = || {
        fixture.host.paired.read().unwrap().clients[0]
            .extra
            .get("virtual_display_layout")
            .cloned()
    };
    drop(first);
    assert_eq!(chosen(), None);
    drop(second);
    assert_eq!(chosen(), Some(serde_json::json!("exclusive")));
    assert_eq!(
        fixture.host.paired.read().unwrap().clients[0]
            .extra
            .get(LEARNED_OVER),
        Some(&serde_json::json!("extended")),
        "the learned layout records the layout it replaced"
    );
    // A later local layout differs, but the paused reaper must not save it.
    display._state.lock().unwrap().layout_snapshot = Some(vec![
        (r"\\.\DISPLAY9".into(), true),
        (r"\\.\DISPLAY1".into(), false),
    ]);
    fixture
        .host
        .app_display
        .lock()
        .unwrap()
        .insert(client.uuid, (display, None));
    fixture.host.reap_paused_display();
    assert!(fixture.host.app_display.lock().unwrap().is_empty());
    assert_eq!(chosen(), Some(serde_json::json!("exclusive")));
}

#[test]
fn a_resolution_chosen_during_the_stream_is_kept_for_the_device_at_disconnect() {
    let fixture = crate::state::test_support::Fixture::new();
    let client = fixture.client(u32::MAX);
    let config = Config::parse("dd_config_revert_delay=0").unwrap();
    *fixture.host.config.write().unwrap() = config.clone();
    let kept = || {
        let paired = fixture.host.paired.read().unwrap();
        let extra = &paired.clients[0].extra;
        assert_eq!(
            extra.get("display_mode"),
            None,
            "the refresh is never pinned"
        );
        extra.get(REMEMBERED_RESOLUTION).cloned()
    };
    let uhd = (3840, 2160);
    let qhd = (2560, 1440);
    for (owner, shown, expected) in [
        // Not opted in.
        (false, qhd, None),
        // Untouched: a refresh Windows lowered (VRR's 1000 Hz) is not a choice.
        (true, uhd, None),
        // Issue #36: 1440p picked in Windows under a 4K stream.
        (true, qhd, Some(serde_json::json!("2560x1440"))),
    ] {
        let mut prepared = test_prepared(&fixture.host);
        prepared.recovery_dimensions = uhd;
        prepared.resolution_owner = owner.then(|| client.uuid.clone());
        prepared.size_snapshot = Some((uhd, shown));
        let limiter =
            limiter::Lease::acquire(&fixture.host.directory, &config, &prepared.framegen).unwrap();
        let stream = Ready::prepare((prepared, limiter)).unwrap();
        let display = stream.display.clone();
        drop(stream);
        assert_eq!(kept(), expected);
        let dimensions = display._state.lock().unwrap().recovery_dimensions;
        assert_eq!(dimensions, if expected.is_some() { qhd } else { uhd });
    }
}

#[test]
fn a_second_screens_chosen_resolution_is_kept_for_that_screen_only() {
    let fixture = crate::state::test_support::Fixture::new();
    let client = fixture.client(u32::MAX);
    let config = Config::parse("dd_config_revert_delay=0").unwrap();
    *fixture.host.config.write().unwrap() = config.clone();
    let second = client.clone().second_screen();
    let mut prepared = test_prepared(&fixture.host);
    prepared.recovery_dimensions = (3840, 2160);
    prepared.resolution_owner = Some(second.uuid.clone());
    prepared.size_snapshot = Some(((3840, 2160), (2560, 1440)));
    let limiter =
        limiter::Lease::acquire(&fixture.host.directory, &config, &prepared.framegen).unwrap();
    drop(Ready::prepare((prepared, limiter)).unwrap());
    let device = fixture.host.paired.read().unwrap().clients[0].clone();
    assert_eq!(device.extra.get(REMEMBERED_RESOLUTION), None);
    // The second screen reads it back as its own, the first screen does not.
    assert_eq!(
        device
            .clone()
            .second_screen()
            .extra
            .get(REMEMBERED_RESOLUTION),
        Some(&serde_json::json!("2560x1440"))
    );
}

#[test]
fn a_scale_chosen_during_the_stream_is_kept_for_the_device_at_disconnect() {
    let fixture = crate::state::test_support::Fixture::new();
    let client = fixture.client(u32::MAX);
    let config = Config::parse("dd_config_revert_delay=0").unwrap();
    *fixture.host.config.write().unwrap() = config.clone();
    let kept = || {
        let paired = fixture.host.paired.read().unwrap();
        let extra = &paired.clients[0].extra;
        assert_eq!(
            extra.get("display_mode"),
            None,
            "the refresh is never pinned"
        );
        extra.get(REMEMBERED_SCALE).cloned()
    };
    for (owner, baseline, shown, expected) in [
        // Not opted in: nothing is stored.
        (false, Some(100), 200, None),
        // Untouched.
        (true, Some(100), 100, None),
        // No baseline: a configured scale, or one Windows did not let us read.
        (true, None, 200, None),
        // Issue #36: 200% picked in Windows.
        (true, Some(100), 200, Some(serde_json::json!(200))),
    ] {
        let mut prepared = test_prepared(&fixture.host);
        prepared.resolution_owner = owner.then(|| client.uuid.clone());
        prepared.scale_baseline = baseline;
        prepared.scale_snapshot = Some(shown);
        let limiter =
            limiter::Lease::acquire(&fixture.host.directory, &config, &prepared.framegen).unwrap();
        let stream = Ready::prepare((prepared, limiter)).unwrap();
        let display = stream.display.clone();
        drop(stream);
        assert_eq!(kept(), expected);
        let recovery = display._state.lock().unwrap().recovery_scale;
        assert_eq!(recovery, if expected.is_some() { 200 } else { 0 });
    }
}

#[test]
fn a_scale_windows_refused_is_a_warning_and_is_not_kept() {
    let fixture = crate::state::test_support::Fixture::new();
    let client = fixture.client(u32::MAX);
    let config = Config::parse("dd_config_revert_delay=0").unwrap();
    *fixture.host.config.write().unwrap() = config.clone();
    fixture.host.paired.write().unwrap().clients[0]
        .extra
        .insert(REMEMBERED_SCALE.into(), serde_json::json!(150));
    // Windows refuses the kept 150% and the display stays at 100%. The refusal does
    // not fail the stream, and the baseline is what Windows shows, not the refused 150.
    let (in_force, baseline) = set_up_scale(
        "out",
        0,
        false,
        Some(150),
        (2560, 1440),
        true,
        |_, _, _, _| anyhow::bail!("requested DPI scale is outside this display's range"),
        |_| Ok(100),
    )
    .unwrap();
    assert_eq!(in_force, 0, "recovery does not retry a refused scale");
    assert_eq!(baseline, Some(100));
    let mut prepared = test_prepared(&fixture.host);
    prepared.resolution_owner = Some(client.uuid.clone());
    prepared.scale_baseline = baseline;
    prepared.scale_snapshot = Some(100);
    let limiter =
        limiter::Lease::acquire(&fixture.host.directory, &config, &prepared.framegen).unwrap();
    drop(Ready::prepare((prepared, limiter)).unwrap());
    assert_eq!(
        fixture.host.paired.read().unwrap().clients[0]
            .extra
            .get(REMEMBERED_SCALE),
        Some(&serde_json::json!(150)),
        "the 100% Windows shows is not saved over the kept scale"
    );
}

#[test]
fn the_scale_baseline_is_read_after_the_kept_scale_is_applied() {
    let log = std::cell::RefCell::new(Vec::new());
    let (in_force, baseline) = set_up_scale(
        "out",
        0,
        false,
        Some(200),
        (2560, 1440),
        true,
        |_, percent, _, _| {
            log.borrow_mut().push(format!("set {percent}"));
            Ok(())
        },
        |_| {
            log.borrow_mut().push("read".to_owned());
            Ok(200)
        },
    )
    .unwrap();
    assert_eq!(*log.borrow(), ["set 200", "read"]);
    assert_eq!((in_force, baseline), (200, Some(200)));
    // Nothing is read when a scale is configured or the device keeps none.
    let (_, baseline) = set_up_scale(
        "out",
        150,
        false,
        Some(200),
        (2560, 1440),
        false,
        |_, _, _, _| Ok(()),
        |_| panic!("the scale is not read"),
    )
    .unwrap();
    assert_eq!(baseline, None);
}

#[test]
fn resuming_puts_the_scale_back_that_a_pause_may_have_changed() {
    let fixture = crate::state::test_support::Fixture::new();
    let config = Config::parse("dd_config_revert_delay=0").unwrap();
    *fixture.host.config.write().unwrap() = config.clone();
    for (baseline, recovery_scale, expected) in [
        // A device keeping the scale: the baseline, so a change at the PC while paused
        // is not learned at the next disconnect.
        (Some(200), 0, vec![200]),
        // A configured scale.
        (None, 150, vec![150]),
        // Nothing to put back.
        (None, 0, vec![]),
    ] {
        let mut prepared = test_prepared(&fixture.host);
        prepared.scale_baseline = baseline;
        prepared.recovery_scale = recovery_scale;
        let limiter =
            limiter::Lease::acquire(&fixture.host.directory, &config, &prepared.framegen).unwrap();
        let display = Ready::prepare((prepared, limiter)).unwrap().display.clone();
        let resumed = display
            .clone()
            .resume(
                &fixture.host.directory,
                &config,
                Arc::new(Warnings::default()),
            )
            .unwrap();
        assert_eq!(
            *display._state.lock().unwrap().scales_set.lock().unwrap(),
            expected
        );
        drop(resumed);
    }
}

#[test]
fn a_kept_scale_is_applied_unless_a_scale_is_configured() {
    let mut extra = std::collections::BTreeMap::new();
    extra.insert(REMEMBERED_SCALE.to_owned(), serde_json::json!(200));
    let applied = |configured: i64, remembered: Option<u32>| {
        let mut calls = Vec::new();
        let in_force = apply_virtual_scale(
            r"\\.\DISPLAY9",
            configured,
            false,
            remembered,
            (2560, 1440),
            |output, percent, width, height| {
                calls.push((output.to_owned(), percent, width, height));
                Ok(())
            },
        )
        .unwrap();
        (calls, in_force)
    };
    let call = |percent| vec![(r"\\.\DISPLAY9".to_owned(), percent, 2560, 1440)];
    // The kept scale is applied at the display's mode, which is the kept resolution.
    assert_eq!(applied(0, kept_scale(true, &extra)), (call(200), 200));
    // A configured scale, a percentage or recommended, wins and nothing is learned over it.
    assert_eq!(applied(150, kept_scale(true, &extra)), (call(150), 150));
    assert_eq!(applied(-1, kept_scale(true, &extra)), (call(-1), -1));
    // Nothing kept: the unconfigured scale 0 leaves Windows' own.
    assert_eq!(
        applied(0, kept_scale(true, &Default::default())),
        (call(0), 0)
    );
    // The option off: a scale left over is not applied.
    assert_eq!(kept_scale(false, &extra), None);
    assert_eq!(applied(0, kept_scale(false, &extra)), (call(0), 0));
    // Anything but a positive percentage is not a scale.
    for bad in [
        serde_json::json!("200"),
        serde_json::json!(0),
        serde_json::json!(-5),
    ] {
        extra.insert(REMEMBERED_SCALE.to_owned(), bad);
        assert_eq!(kept_scale(true, &extra), None);
    }
}

#[test]
fn disconnect_reaping_waits_for_active_owners_and_releases_exclusive_layouts() {
    for (settings, exclusive, released) in [
        ("dd_config_revert_delay=0", false, true),
        (
            "dd_config_revert_on_disconnect=false\ndd_paused_virtual_display_timeout_secs=0\ndd_config_revert_delay=0",
            false,
            false,
        ),
        (
            "dd_config_revert_on_disconnect=false\ndd_paused_virtual_display_timeout_secs=0\ndd_config_revert_delay=0",
            true,
            true,
        ),
    ] {
        let fixture = crate::state::test_support::Fixture::new();
        let config = Config::parse(settings).unwrap();
        *fixture.host.config.write().unwrap() = config.clone();
        let prepared = test_prepared(&fixture.host);
        let limiter =
            limiter::Lease::acquire(&fixture.host.directory, &config, &prepared.framegen).unwrap();
        let mut stream = Ready::prepare((prepared, limiter)).unwrap();
        // Supply the arrangement result without touching Windows displays.
        Arc::get_mut(&mut stream.display).unwrap().exclusive_layout = exclusive;
        fixture
            .host
            .app_display
            .lock()
            .unwrap()
            .insert("client".into(), (stream.display.clone(), None));
        fixture.host.reap_paused_display();
        assert_eq!(fixture.host.app_display.lock().unwrap().len(), 1);
        drop(stream);
        fixture.host.reap_paused_display();
        assert_eq!(
            fixture.host.app_display.lock().unwrap().is_empty(),
            released,
            "{settings}, exclusive={exclusive}"
        );
    }
}

#[test]
fn a_retained_display_stops_enforcing_layout_after_the_last_preparation_drops() {
    let fixture = crate::state::test_support::Fixture::new();
    let config = Config::default();
    let prepared = test_prepared(&fixture.host);
    let limiter =
        limiter::Lease::acquire(&fixture.host.directory, &config, &prepared.framegen).unwrap();
    let first = Ready::prepare((prepared, limiter)).unwrap();
    let display = first.display.clone();
    let mut changed = Config::parse("virtual_display_layout=extended").unwrap();
    let mut client = fixture.client(u32::MAX);
    let mut app: butterpollo_session::state::App =
        serde_json::from_value(serde_json::json!({"name":"Game"})).unwrap();
    assert!(display.serves(
        None,
        virtual_display_layout(&changed, &client, Some(&app), None)
    ));
    for layout in ["exclusive", "extended_primary"] {
        for source in ["host", "device", "app"] {
            match source {
                "host" => {
                    changed
                        .values
                        .insert("virtual_display_layout".into(), layout.into());
                }
                "device" => {
                    client
                        .extra
                        .insert("virtual_display_layout".into(), layout.into());
                }
                _ => {
                    app.extra
                        .insert("virtual-display-layout".into(), layout.into());
                }
            }
            assert!(
                !display.serves(
                    None,
                    virtual_display_layout(&changed, &client, Some(&app), None)
                ),
                "{source}: {layout}"
            );
            changed
                .values
                .insert("virtual_display_layout".into(), "extended".into());
            client.extra.remove("virtual_display_layout");
            app.extra.remove("virtual-display-layout");
        }
    }
    assert_eq!(display.active.load(Ordering::Acquire), 1);
    let second = display
        .clone()
        .resume(
            &fixture.host.directory,
            &config,
            Arc::new(Warnings::default()),
        )
        .unwrap();
    assert_eq!(display.active.load(Ordering::Acquire), 2);
    drop(first);
    assert_eq!(display.active.load(Ordering::Acquire), 1);
    drop(second);
    assert_eq!(display.active.load(Ordering::Acquire), 0);
    let mut paused = display._state.lock().unwrap();
    paused.machine.recovery.start(std::time::Instant::now());
    paused.feed(false).unwrap();
    assert!(paused.machine.recovery.pending);
    drop(paused);
    drop(display);
}

#[test]
#[ignore = "creates a virtual display, disables it while paused, and resumes it"]
fn native_paused_game_leaves_a_disabled_display_off_until_resume() -> Result<()> {
    use butterpollo_win_display::display::{Snapshot, Topology, VirtualOptions, monitors};

    let before = Snapshot::capture()?;
    let result = (|| -> Result<()> {
        let fixture = crate::state::test_support::Fixture::new();
        let config = Config::default();
        let guard = Guard::new_virtual_options(
            "",
            true,
            &format!("paused-game-test-{}", std::process::id()),
            1280,
            720,
            Rate(60_000),
            None,
            None,
            None,
            &VirtualOptions::default(),
        )?;
        let target = monitors()?
            .into_iter()
            .find(|monitor| monitor.matches(&guard.output))
            .context("test display unavailable")?;
        let mut prepared = test_prepared(&fixture.host);
        prepared.output = guard.output.clone();
        prepared.display = Some(guard);
        let limiter =
            limiter::Lease::acquire(&fixture.host.directory, &config, &prepared.framegen)?;
        let stream = Ready::prepare((prepared, limiter))?;
        let display = stream.display.clone();
        drop(stream);
        anyhow::ensure!(display.active.load(Ordering::Acquire) == 0);

        // The same operation as a local layout that excludes the virtual display.
        let kept: Vec<_> = monitors()?
            .into_iter()
            .filter(|monitor| monitor.device_id != target.device_id)
            .map(|monitor| monitor.device_id)
            .collect();
        anyhow::ensure!(!kept.is_empty(), "test needs another active display");
        Topology::set_active(&kept)?;
        let physical = Snapshot::capture()?;
        // Cross the driver's 10 s watchdog and its 3 s polling ticks.
        let deadline = std::time::Instant::now() + Duration::from_secs(16);
        while std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(250));
            anyhow::ensure!(
                !monitors()?.iter().any(|m| m.device_id == target.device_id),
                "paused game reactivated the display the user switched off"
            );
        }
        anyhow::ensure!(
            Topology::query_all()?
                .monitors()
                .iter()
                .any(|m| m.device_id == target.device_id),
            "paused game's display lease expired"
        );
        anyhow::ensure!(
            serde_json::to_value(Snapshot::capture()?)? == serde_json::to_value(physical)?,
            "paused renewal changed the physical displays"
        );

        let resumed = display.clone().resume(
            &fixture.host.directory,
            &config,
            Arc::new(Warnings::default()),
        )?;
        let (output, generation) = display.capture_target();
        anyhow::ensure!(
            monitors()?
                .iter()
                .any(|m| m.device_id == target.device_id && m.matches(&output)),
            "resume returned before publishing the active capture target"
        );
        anyhow::ensure!(generation > 0, "resume did not publish the new generation");
        drop(resumed);
        drop(display);
        Ok(())
    })();
    before.restore()?;
    result
}
