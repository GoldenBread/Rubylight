//! Tests for frame limiter recovery, display-rate following, warnings, and RTSS confirmation.
use super::*;

#[test]
fn a_journal_with_nvidia_entries_is_not_dropped_silently() {
    let directory = std::env::temp_dir().join(format!("limiter-nvidia-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        path(&directory),
        r#"{"version":1,"rtss":null,"nvidia":[{"id":277041154,"before":{"effective":0,"override_value":null},"applied":60},{"id":11041231,"before":{"effective":0,"override_value":null},"applied":134497095}]}"#,
    )
    .unwrap();
    // The entries survive loading and are written back with the journal.
    let journal = load(&directory).unwrap();
    assert_eq!(journal.nvidia.len(), 2);
    let message = take_stale_nvidia(&directory).expect("a warning");
    assert!(message.contains("Max Frame Rate"), "{message}");
    assert!(message.contains("Vertical sync"), "{message}");
    assert!(message.contains("NVIDIA Control Panel"), "{message}");
    // Warned once: the entries are gone, the rest of the journal stays.
    assert!(load(&directory).unwrap().nvidia.is_empty());
    assert!(take_stale_nvidia(&directory).is_none());
    std::fs::remove_dir_all(&directory).unwrap();
}

/// A game that sets the stream's display to 60 Hz under the automatic
/// 60 fps cap: the cap comes off, and returns with a fast mode. One stream,
/// RTSS and an untroubled cap only.
#[test]
fn the_automatic_cap_follows_one_streams_display() {
    let followed = || State {
        users: 1,
        active: "rtss".into(),
        rate: Some(Rate(60_000)),
        follow: Some(Follow {
            id: 1,
            root: PathBuf::new(),
            rate: Rate(60_000),
            follows_display: true,
            standing_down: false,
            queue_clamp: false,
            clamped_rate: None,
            changing: false,
            failed: false,
            shared: false,
        }),
        ..State::default()
    };
    let mut state = followed();
    assert_eq!(wanted_change(&state, Rate(1_000_000)), None);
    assert_eq!(wanted_change(&state, Rate(120_000)), None);
    assert_eq!(wanted_change(&state, Rate(60_000)), Some(true));
    assert_eq!(wanted_change(&state, Rate(59_000)), Some(true));
    state.follow.as_mut().unwrap().standing_down = true;
    assert_eq!(wanted_change(&state, Rate(60_000)), None);
    assert_eq!(wanted_change(&state, Rate(1_000_000)), Some(false));
    // Nothing while a change is under way or after one failed.
    state.follow.as_mut().unwrap().changing = true;
    assert_eq!(wanted_change(&state, Rate(1_000_000)), None);
    let mut state = followed();
    state.follow.as_mut().unwrap().failed = true;
    assert_eq!(wanted_change(&state, Rate(60_000)), None);
    // A second stream shares the cap, also once the first has left; a cap the user
    // asked for has no `follow`.
    let mut state = followed();
    state.users = 2;
    assert_eq!(wanted_change(&state, Rate(60_000)), None);
    let mut state = followed();
    state.follow.as_mut().unwrap().shared = true;
    assert_eq!(wanted_change(&state, Rate(60_000)), None);
    let mut state = followed();
    state.follow = None;
    assert_eq!(wanted_change(&state, Rate(60_000)), None);
    let mut state = followed();
    state.active = "none".into();
    assert_eq!(wanted_change(&state, Rate(60_000)), None);
}

/// A game's mode change reaches the cap within one live tick (250 ms) instead of the
/// next 5 s timing window: the display is read at each tick while a cap follows it.
#[test]
fn a_followed_display_is_read_every_live_tick() {
    let mut state = State {
        users: 1,
        active: "rtss".into(),
        follow: Some(Follow {
            id: 1,
            root: PathBuf::new(),
            rate: Rate(60_000),
            follows_display: true,
            standing_down: false,
            queue_clamp: false,
            clamped_rate: None,
            changing: false,
            failed: false,
            shared: false,
        }),
        ..State::default()
    };
    assert!(follows(&state));
    // Also while the cap is off, to see the display get fast again.
    state.follow.as_mut().unwrap().standing_down = true;
    assert!(follows(&state));
    state.follow.as_mut().unwrap().shared = true;
    assert!(!follows(&state));
    state.follow.as_mut().unwrap().shared = false;
    state.follow.as_mut().unwrap().failed = true;
    assert!(!follows(&state));
    state.follow.as_mut().unwrap().failed = false;
    state.users = 2;
    assert!(!follows(&state));
    state.users = 1;
    state.follow = None;
    assert!(!follows(&state));
    // A stream's loop: ticks every 250 ms, a 5 s window; the game changes the mode
    // at each offset. The wait until the display is read next, with and without
    // the follow.
    let tick = 250u64;
    let delay = |following: bool, change_at: u64| {
        (1..)
            .map(|n| n * tick)
            .find(|at| *at >= change_at && display_read_due(at % 5_000 == 0, following))
            .unwrap()
            - change_at
    };
    let (mut before, mut after) = (0, 0);
    for change_at in (0..10_000).step_by(10) {
        before = before.max(delay(false, change_at));
        after = after.max(delay(true, change_at));
    }
    assert!(before >= 4_990, "{before}");
    assert!(after <= tick, "{after}");
}

/// The stream's end, or the crash watcher, finds the profile with the cap or
/// with the limit off (the stream's display was too slow for the cap): both
/// are the host's, and the user's own values come back. A limit the user set
/// during the stream stays.
#[test]
fn a_stood_down_limit_is_restored_like_the_cap() {
    let values = |limit: u32, denominator: u32, sync: u32| {
        BTreeMap::from([
            ("Limit".to_string(), Some(limit)),
            ("LimitDenominator".to_string(), Some(denominator)),
            ("SyncLimiter".to_string(), Some(sync)),
        ])
    };
    for (name, during, after) in [
        ("cap", values(60, 1, 2), values(120, 1, 1)),
        ("off", values(0, 1, 2), values(120, 1, 1)),
        // 90 is the user's; the sync mode still holds the host's value.
        ("edited", values(90, 1, 2), values(90, 1, 1)),
    ] {
        let directory =
            std::env::temp_dir().join(format!("limiter-stood-down-{name}-{}", std::process::id()));
        let root = directory.join("rtss");
        std::fs::create_dir_all(root.join("Profiles")).unwrap();
        let profile = |values: &BTreeMap<String, Option<u32>>| {
            rtss::replace("[Framerate]\nCustom=keep\n", values).unwrap()
        };
        std::fs::write(root.join("Profiles/Global"), profile(&during)).unwrap();
        let journal = Journal {
            version: 1,
            rtss: Some(RtssChange {
                root: root.clone(),
                before: values(120, 1, 1),
                applied: values(60, 1, 2),
                disabled: false,
                stood_down: Some(no_limit()),
                clamped: None,
                previous_clamped: None,
            }),
            nvidia: Vec::new(),
        };
        butterpollo_session::state::write_json(&path(&directory), &journal).unwrap();
        recover(&directory).unwrap();
        let restored = rtss::properties(&rtss::read(&root).unwrap()).unwrap();
        assert_eq!(restored, after, "{name}");
        assert!(load(&directory).unwrap().rtss.is_none(), "{name}");
        std::fs::remove_dir_all(&directory).unwrap();
    }
    // A journal written before the limit could stand down reads as before.
    let old: RtssChange = serde_json::from_str(
        r#"{"root":"C:/rtss","before":{"Limit":120},"applied":{"Limit":60},"disabled":false}"#,
    )
    .unwrap();
    assert!(old.stood_down.is_none());
    assert!(!serde_json::to_string(&old).unwrap().contains("stood_down"));
}

/// A stream to a virtual display with `frame_limiter_provider = none` used
/// to warn "frame limiter has no available provider" at every launch,
/// although limiting was turned off on purpose.
#[test]
fn a_missing_provider_is_a_fault_only_when_a_cap_was_asked_for() {
    assert!(!missing_provider_is_a_fault(false, Provider::None, ""));
    assert!(!missing_provider_is_a_fault(true, Provider::None, ""));
    // The automatic virtual-display cap on a host without RTSS.
    assert!(!missing_provider_is_a_fault(false, Provider::Auto, ""));
    assert!(missing_provider_is_a_fault(true, Provider::Auto, ""));
    assert!(missing_provider_is_a_fault(false, Provider::Rtss, ""));
    assert!(missing_provider_is_a_fault(
        false,
        Provider::Auto,
        "RTSS did not confirm"
    ));
}

/// The automatic virtual-display cap is the automatic one only where RTSS can apply it;
/// `none` is off on purpose, not a missing provider.
#[test]
fn a_provider_is_usable_only_when_rtss_is_there_and_a_limiter_is_asked_for() {
    let missing = std::env::temp_dir().join(format!("limiter-no-rtss-{}", std::process::id()));
    let text = format!("rtss_install_path={}\n", missing.display());
    for provider in ["auto", "rtss"] {
        let config = Config::parse(&format!("{text}frame_limiter_provider={provider}")).unwrap();
        assert!(!provider_usable(&config), "{provider}");
    }
    let off = Config::parse(&format!("{text}frame_limiter_provider=none")).unwrap();
    assert!(!provider_usable(&off));
}

#[test]
fn limiter_reports_missing_and_failed_providers() {
    assert!(
        limiter_warning(true, true, "none", "", (None, None))
            .unwrap()
            .contains("not applied")
    );
    assert!(
        limiter_warning(true, true, "rtss", "recovery pending", (None, None))
            .unwrap()
            .contains("recovery pending")
    );
    assert!(limiter_warning(false, true, "none", "", (None, None)).is_none());
    // The automatic virtual-display cap on a host without RTSS is not a fault.
    assert!(limiter_warning(true, false, "none", "", (None, None)).is_none());
    assert!(
        limiter_warning(true, false, "none", "RTSS did not confirm", (None, None))
            .unwrap()
            .contains("(RTSS did not confirm)")
    );
    assert!(
        !limiter_warning(true, true, "none", "", (None, None))
            .unwrap()
            .contains("()")
    );
    assert!(limiter_warning(true, true, "rtss", "", (None, None)).is_none());
    assert!(
        limiter_warning(
            true,
            true,
            "rtss",
            "",
            (Some(Rate(116_000)), Some(Rate(60_000)))
        )
        .unwrap()
        .contains("60 fps")
    );
    assert!(limiter_warning(false, true, "rtss", "", (None, Some(Rate(60_000)))).is_some());
    assert!(
        limiter_warning(
            true,
            true,
            "rtss",
            "",
            (Some(Rate(60_000)), Some(Rate(60_000)))
        )
        .is_none()
    );
}

/// RL56's 3.4.0: "RTSS did not confirm restoration of LimitDenominator"
/// at every stream end and start, the limiter held for good.
#[test]
fn a_restoration_rtss_states_otherwise_or_cannot_report_is_confirmed() {
    let restored = |wanted: &_, reported: &_| confirmed(wanted, reported, false);
    let map = |values: &[(&str, Option<u32>)]| -> BTreeMap<String, Option<u32>> {
        values.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    };
    let all = |limit, denominator, sync| {
        map(&[
            ("Limit", limit),
            ("LimitDenominator", denominator),
            ("SyncLimiter", sync),
        ])
    };
    let wanted = all(Some(0), Some(1000), Some(0));
    // No limit: any denominator is no cap.
    assert_eq!(
        restored(&wanted, &all(Some(0), Some(1), Some(0))),
        Ok(vec![])
    );
    // The same cap stated another way, a zero denominator as 1.
    let wanted = all(Some(2997), Some(25), Some(1));
    assert_eq!(
        restored(&wanted, &all(Some(11988), Some(100), Some(1))),
        Ok(vec![])
    );
    assert_eq!(
        restored(
            &all(Some(60), Some(0), None),
            &all(Some(60), Some(1), Some(1))
        ),
        Ok(vec![])
    );
    // A key this RTSS doesn't report is named, not pending, but never
    // the limit itself.
    assert_eq!(
        restored(&wanted, &all(Some(2997), None, Some(1))),
        Ok(vec!["LimitDenominator".to_string()])
    );
    assert_eq!(
        restored(&wanted, &all(None, None, None)),
        Err("Limit".to_string())
    );
    // Another cap, or another sync mode, is a failed restoration.
    assert_eq!(
        restored(&wanted, &all(Some(2997), Some(1), Some(1))),
        Err("LimitDenominator".to_string())
    );
    assert_eq!(
        restored(
            &all(Some(0), Some(1), Some(1)),
            &all(Some(120), Some(1), Some(1))
        ),
        Err("Limit".to_string())
    );
    assert_eq!(
        restored(&wanted, &all(Some(2997), Some(25), Some(0))),
        Err("SyncLimiter".to_string())
    );
    // Only the denominator to restore: the user's later limit stays.
    assert_eq!(
        restored(
            &map(&[("LimitDenominator", Some(1))]),
            &all(Some(0), Some(1000), Some(1))
        ),
        Ok(vec![])
    );
    assert_eq!(
        restored(
            &map(&[("LimitDenominator", Some(1))]),
            &all(Some(90), Some(1000), Some(1))
        ),
        Err("LimitDenominator".to_string())
    );
}

/// The coordinator's review of the fix above: a stream's cap RTSS
/// doesn't report back is never "applied and verified".
#[test]
fn a_stream_cap_rtss_cannot_report_is_not_confirmed() {
    let all = |limit, denominator, sync| -> BTreeMap<String, Option<u32>> {
        [
            ("Limit", limit),
            ("LimitDenominator", denominator),
            ("SyncLimiter", sync),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect()
    };
    let applied = |wanted: &_, reported: &_| confirmed(wanted, reported, true);
    let fractional = all(Some(2997), Some(25), Some(1));
    // Nothing reported: nothing confirmed.
    assert_eq!(
        applied(&fractional, &all(None, None, None)),
        Err("Limit".to_string())
    );
    // 2997 without its denominator could be 2997 fps.
    assert_eq!(
        applied(&fractional, &all(Some(2997), None, Some(1))),
        Err("LimitDenominator".to_string())
    );
    // A whole cap needs no denominator; what isn't reported is named, and
    // the console says so.
    let whole = all(Some(60), Some(1), Some(1));
    assert_eq!(
        applied(&whole, &all(Some(60), None, None)),
        Ok(vec![
            "LimitDenominator".to_string(),
            "SyncLimiter".to_string()
        ])
    );
    assert_eq!(applied(&whole, &whole), Ok(vec![]));
    let message = unconfirmed_message(&["SyncLimiter".to_string()]);
    assert!(message.contains("SyncLimiter"), "{message}");
    let warning = limiter_warning(true, true, "rtss", &message, (None, None)).unwrap();
    assert!(warning.contains("not fully applied"), "{warning}");
}

#[test]
fn staged_clamp_transitions_preserve_ownership_of_previously_live_clamp() {
    let pair = |limit, denom| {
        BTreeMap::from([
            ("Limit".to_string(), Some(limit)),
            ("LimitDenominator".to_string(), Some(denom)),
        ])
    };
    let c1 = pair(873, 10);
    let c2 = pair(750, 10);
    let change = RtssChange {
        root: PathBuf::new(),
        before: pair(0, 1),
        applied: BTreeMap::from([
            ("Limit".into(), Some(120)),
            ("LimitDenominator".into(), Some(1)),
            ("SyncLimiter".into(), Some(1)),
        ]),
        disabled: false,
        stood_down: Some(no_limit()),
        clamped: Some(c2.clone()),
        previous_clamped: Some(c1.clone()),
    };
    // If `rtss::apply` failed or crashed while stepping C1 -> C2 or standing down, C1 is still ours.
    assert!(change.ours("Limit", &c1));
    assert!(change.ours("LimitDenominator", &c1));
    assert!(change.ours("Limit", &c2));
    assert!(change.ours("Limit", &no_limit()));
    assert!(!change.ours("Limit", &pair(144, 1)));
}
