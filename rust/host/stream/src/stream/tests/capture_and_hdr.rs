//! Unit tests for browser monitor layout, capture reopen/sharing keys, encoder recovery progress, and HDR fallback.

use super::super::*;

#[test]
fn a_browser_on_this_pc_keeps_the_monitors_it_shows_the_stream_on() {
    use crate::display_session::{BROWSER_ON_THIS_PC, launch_display_layout};
    use butterpollo_session::session::Transport;
    let f = crate::state::test_support::Fixture::new();
    let config = butterpollo_config::config::Config::default();
    let client = f.client(u32::MAX);
    let mut launch = f.launch(client.clone(), Role::Stream);
    // The default for everyone else: the stream takes over the displays.
    assert_eq!(
        launch_display_layout(&config, &launch, None, None),
        "exclusive"
    );
    launch.transport = Transport::WebRtc;
    assert_eq!(
        launch_display_layout(&config, &launch, None, None),
        "exclusive"
    );
    launch.options.insert(BROWSER_ON_THIS_PC.into(), "1".into());
    assert_eq!(
        launch_display_layout(&config, &launch, None, None),
        "extended_primary"
    );
    // A layout that leaves the monitors on is the user's and stays.
    let extended =
        butterpollo_config::config::Config::parse("virtual_display_layout=extended\n").unwrap();
    assert_eq!(
        launch_display_layout(&extended, &launch, None, None),
        "extended"
    );
    // A Moonlight client cannot ask for this with a launch parameter.
    launch.transport = Transport::default();
    assert_ne!(launch.transport, Transport::WebRtc);
    assert_eq!(
        launch_display_layout(&config, &launch, None, None),
        "exclusive"
    );
}
#[test]
fn a_browser_on_this_pc_keeps_its_monitors_however_the_setting_is_spelled() {
    use crate::display_session::{
        BROWSER_ON_THIS_PC, browser_keeps_monitors, launch_display_layout,
    };
    use butterpollo_session::session::Transport;
    let f = crate::state::test_support::Fixture::new();
    let mut launch = f.launch(f.client(u32::MAX), Role::Stream);
    launch.transport = Transport::WebRtc;
    launch.options.insert(BROWSER_ON_THIS_PC.into(), "1".into());
    let layout = |launch: &Launch, value: &str| {
        let config =
            butterpollo_config::config::Config::parse(&format!("virtual_display_layout={value}\n"))
                .unwrap();
        launch_display_layout(&config, launch, None, None).to_owned()
    };
    // All of these turn the other monitors off when the display setup reads them, and an unknown
    // virtual display layout falls back to that.
    for value in [
        "exclusive",
        "Exclusive",
        "EXCLUSIVE",
        "ensure_only_display",
        "Ensure-Only-Display",
        "no_such_layout",
    ] {
        assert_eq!(layout(&launch, value), "extended_primary", "{value}");
    }
    // These two arrange nothing, and no layout that leaves the monitors on is replaced.
    for value in [
        "disabled",
        "verify_only",
        "extended",
        "extended_isolated",
        "extended_primary_isolated",
    ] {
        assert_eq!(layout(&launch, value), value);
    }
    // A physical display takes `dd_configuration_option`; an unknown value arranges nothing.
    for value in ["ensure_only_display", "Ensure-Only-Display", "exclusive"] {
        assert!(browser_keeps_monitors(&launch, value, false), "{value}");
    }
    for value in [
        "disabled",
        "verify_only",
        "ensure_active",
        "ensure_primary",
        "no_such_option",
    ] {
        assert!(!browser_keeps_monitors(&launch, value, false), "{value}");
    }
    // Only a browser on this PC.
    launch.transport = Transport::default();
    assert!(!browser_keeps_monitors(
        &launch,
        "ensure_only_display",
        false
    ));
    launch.transport = Transport::WebRtc;
    launch.options.remove(BROWSER_ON_THIS_PC);
    assert!(!browser_keeps_monitors(
        &launch,
        "ensure_only_display",
        false
    ));
}
#[test]
fn capture_waits_for_its_display_and_shows_the_primary_only_after_the_timeout() {
    let start = Instant::now();
    let mut missing = None;
    assert_eq!(
        reopen_on(true, false, &mut missing, start),
        ReopenOn::Stream
    );
    // Windows switched the virtual display off: capture waits for the
    // heartbeat to bring it back instead of opening the physical desktop.
    assert_eq!(reopen_on(false, false, &mut missing, start), ReopenOn::Wait);
    let almost = start + DISPLAY_RETURN_WAIT - Duration::from_millis(1);
    assert_eq!(
        reopen_on(false, false, &mut missing, almost),
        ReopenOn::Wait
    );
    assert_eq!(
        reopen_on(false, false, &mut missing, start + DISPLAY_RETURN_WAIT),
        ReopenOn::Primary
    );
    // It came back under another name: the wait starts over next time.
    let back = start + Duration::from_secs(20);
    assert_eq!(reopen_on(true, false, &mut missing, back), ReopenOn::Stream);
    assert_eq!(missing, None);
    assert_eq!(reopen_on(false, false, &mut missing, back), ReopenOn::Wait);
}
#[test]
fn a_game_window_target_reopens_on_itself_never_the_primary_display() {
    // A Nucleus Co-op player's window: closed, on a missing display or
    // malformed, the reopen reports it instead of showing the PC's screen.
    let start = Instant::now();
    for target in [
        r"window:12@\\.\NO_SUCH_DISPLAY",
        r"window:0@\\.\DISPLAY1",
        "window:garbage",
    ] {
        let present = butterpollo_win_graphics::capture::display_present(target);
        let mut missing = None;
        for background in [false, true] {
            for elapsed in [Duration::ZERO, DISPLAY_RETURN_WAIT, Duration::from_secs(61)] {
                assert_eq!(
                    reopen_on(present, background, &mut missing, start + elapsed),
                    ReopenOn::Stream,
                    "{target}"
                );
            }
        }
        assert_eq!(missing, None, "{target}");
    }
}
#[test]
fn a_capture_reopens_on_its_own_backend_unless_the_fullscreen_fallback_moved_it() {
    // An ordinary reopen such as the secure desktop's tries the stream's own
    // backend again: Rubylight's display push and AMD Direct Capture, which
    // fell back while they were unavailable, get their next try.
    assert_eq!(reopen_backend("wgc", false), "wgc");
    assert_eq!(reopen_backend("idd-push", false), "idd-push");
    assert_eq!(reopen_backend("amd", false), "amd");
    // The fullscreen fallback's Desktop Duplication shows the game a frozen
    // WGC picture could not, so it keeps it for the session.
    assert_eq!(reopen_backend("wgc", true), "ddx");
    assert_eq!(reopen_backend("idd-push", true), "ddx");
}
#[test]
fn a_secure_desktops_duplication_returns_to_wgc_but_the_fullscreen_fallback_keeps_it() {
    // A UAC prompt or the lock screen left a WGC stream on Desktop Duplication;
    // with the desktop visible again it reopens on WGC.
    assert!(returns_to_wgc(true, false, "ddx", "wgc", false));
    // The fullscreen fallback keeps the source that shows the game.
    assert!(!returns_to_wgc(true, false, "ddx", "wgc", true));
    // A stream that asked for Desktop Duplication itself stays there, and a
    // desktop already usable or still unusable is no reason to reopen.
    assert!(!returns_to_wgc(true, false, "ddx", "ddx", false));
    assert!(!returns_to_wgc(true, true, "ddx", "wgc", false));
    assert!(!returns_to_wgc(false, true, "ddx", "wgc", false));
    assert!(!returns_to_wgc(false, false, "ddx", "wgc", false));
    assert!(!returns_to_wgc(true, false, "wgc", "wgc", false));
}
#[test]
fn the_fullscreen_probe_asks_the_shell_twice_a_second_at_most() {
    // A still picture polls capture continuously, so the shell's answer must not
    // run on every idle poll; the first probe goes through at once.
    let start = Instant::now();
    let mut next = start;
    assert!(fullscreen_probe_due(start, &mut next));
    for step in 1..25 {
        assert!(
            !fullscreen_probe_due(start + Duration::from_millis(step * 20), &mut next),
            "{step}"
        );
    }
    // Half a second after it ran, the answer may be asked again, whether it
    // found a fullscreen app or not.
    let second = start + FULLSCREEN_PROBE;
    assert!(fullscreen_probe_due(second, &mut next));
    assert!(!fullscreen_probe_due(
        second + Duration::from_millis(499),
        &mut next
    ));
    assert!(fullscreen_probe_due(second + FULLSCREEN_PROBE, &mut next));
}
/// #56: a Nucleus Co-op game replaced its splash window, or resized its
/// window, as the stream's first capture opened, and the stream ended; the
/// client's reconnect seconds later worked.
#[test]
fn a_game_windows_first_capture_is_tried_again_on_its_current_window() {
    let splash = (r"window:69518@\\.\DISPLAY1".to_string(), 0);
    let game = (r"window:11995426@\\.\DISPLAY1".to_string(), 0);
    // The splash closes as it opens; then the game's window opens, but is
    // resized as its capture starts; the next try works.
    let targets = std::cell::RefCell::new(vec![splash.clone(), game.clone(), game.clone()]);
    let mut tried = Vec::new();
    let mut waited = Vec::new();
    let (opened, target) = open_first(
        || targets.borrow_mut().remove(0),
        |target| {
            tried.push(target.clone());
            match tried.len() {
                1 => anyhow::bail!("Could not capture the given window. (0x80070057)"),
                2 => anyhow::bail!("Access is denied. (0x80070005)"),
                _ => Ok("capture"),
            }
        },
        |failed| {
            waited.push(failed.clone());
            true
        },
    )
    .unwrap();
    assert_eq!((opened, &target), ("capture", &game));
    assert_eq!(tried, [splash.clone(), game.clone(), game.clone()]);
    // Each wait is told the target that failed.
    assert_eq!(waited, [splash.clone(), game.clone()]);
    // Until the tries run out: then the last error is the stream's.
    let error = open_first(
        || splash.clone(),
        |_| -> Result<()> { anyhow::bail!("The handle is invalid. (0x80070006)") },
        {
            let mut left = 3;
            move |_| {
                left -= 1;
                left > 0
            }
        },
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "The handle is invalid. (0x80070006)");
    // A display's first failure is the stream's at once.
    let mut opens = 0;
    let error = open_first(
        || (r"\\.\DISPLAY1".to_string(), 0),
        |_| -> Result<()> {
            opens += 1;
            anyhow::bail!("no display")
        },
        |_| panic!("a display is not tried again"),
    )
    .unwrap_err();
    assert_eq!((opens, error.to_string()), (1, "no display".to_string()));
}
/// A try of a game window's capture starts only while it, with a WGC
/// helper's whole start, ends before the stream stops waiting: one that ran
/// past it ended the stream with a timeout instead of the capture's error.
#[test]
fn a_game_windows_capture_is_tried_again_only_within_the_streams_wait() {
    let started = Instant::now();
    let stop = AtomicBool::new(false);
    let at = |millis: u64| started + Duration::from_millis(millis);
    let helper = butterpollo_win_graphics::capture::WGC_START_TIMEOUT;
    let last = CAPTURE_START_WAIT - RECOVERY_RETRY - helper - WINDOW_OPEN_MARGIN;
    assert_eq!(last, Duration::from_millis(5850));
    let same = SAME_WINDOW_WAIT - RECOVERY_RETRY - helper - WINDOW_OPEN_MARGIN;
    assert_eq!(same, Duration::from_millis(3850));
    // A try that waited out its whole helper start on a window the game replaced meanwhile
    // gets another, on the new window (#59).
    let whole_try = helper + Duration::from_millis(500);
    assert!(whole_try + RECOVERY_RETRY < last);
    assert!(window_open_again(started, &stop, started + whole_try, true));
    // On a window the game still shows it fails with its own error, as a hung game's does:
    // that stream ends before its client gives up, and the next player's capture goes ahead.
    assert!(!window_open_again(started, &stop, started + helper, false));
    assert!(window_open_again(started, &stop, at(0), false));
    assert!(window_open_again(started, &stop, started + same, false));
    assert!(!window_open_again(
        started,
        &stop,
        started + same + Duration::from_millis(1),
        false
    ));
    assert!(window_open_again(started, &stop, at(0), true));
    assert!(window_open_again(started, &stop, started + last, true));
    assert!(!window_open_again(
        started,
        &stop,
        started + last + Duration::from_millis(1),
        true
    ));
    // Every try that starts, helper start and all, ends within the wait.
    assert!(last + RECOVERY_RETRY + helper < CAPTURE_START_WAIT);
    // A stopped stream's capture is not tried again.
    stop.store(true, Ordering::Release);
    assert!(!window_open_again(started, &stop, at(0), true));
    // The capture thread's own wait: once between tries, never past the wait.
    stop.store(false, Ordering::Release);
    let before = Instant::now();
    assert!(wait_to_open_again(started, &stop, false));
    assert!(before.elapsed() >= RECOVERY_RETRY);
    let late = Instant::now() - last - Duration::from_millis(10);
    let before = Instant::now();
    assert!(!wait_to_open_again(late, &stop, true));
    assert!(before.elapsed() < RECOVERY_RETRY);
    stop.store(true, Ordering::Release);
    assert!(!wait_to_open_again(Instant::now(), &stop, true));
}
#[test]
fn background_capture_never_recovers_onto_the_primary_display() {
    let start = Instant::now();
    let mut missing = None;
    for elapsed in [Duration::ZERO, DISPLAY_RETURN_WAIT, Duration::from_secs(60)] {
        assert_eq!(
            reopen_on(false, true, &mut missing, start + elapsed),
            ReopenOn::Wait
        );
    }
    assert_eq!(
        reopen_on(true, true, &mut missing, start + Duration::from_secs(61)),
        ReopenOn::Stream
    );
    assert_eq!(missing, None);

    let plain = Config::default();
    let mut background = plain.clone();
    butterpollo_session::background::apply(&mut background);
    for capture in ["auto", "wgc", "ddx"] {
        assert_ne!(
            CaptureKey::new(capture, "display", false, &plain, "", false),
            CaptureKey::new(capture, "display", false, &background, "", false)
        );
    }
}
/// An explicit Capture method is the user's and must never be moved to
/// Desktop Duplication by the fullscreen fallback of an Automatic stream
/// sharing its capture. The two therefore never share one.
#[test]
fn an_explicit_capture_method_never_shares_an_automatic_streams_capture() {
    let key = |setting: &str| {
        CaptureKey::new(
            "wgc",
            "display",
            false,
            &Config::parse(setting).unwrap(),
            "",
            false,
        )
    };
    assert_ne!(key("capture = auto"), key("capture = wgc"));
    assert_eq!(key("capture = auto"), key("capture = "));
    assert_eq!(key("capture = wgc"), key("capture = WGC"));
}
#[test]
fn accepting_input_without_output_cannot_extend_encoder_recovery() {
    let start = Instant::now();
    let mut failing = Some(start);
    for seconds in 0..5 {
        encoder_progress(&mut failing, false, start + Duration::from_secs(seconds)).unwrap();
        assert_eq!(failing, Some(start));
    }
    assert!(encoder_progress(&mut failing, false, start + ENCODER_RECOVERY).is_err());
}
#[test]
fn completed_output_ends_recovery_and_allows_a_later_independent_failure() {
    let start = Instant::now();
    let mut failing = Some(start);
    encoder_progress(&mut failing, true, start + Duration::from_secs(4)).unwrap();
    assert_eq!(failing, None);
    encoder_progress(&mut failing, false, start + ENCODER_RECOVERY).unwrap();
    let later = start + ENCODER_RECOVERY + Duration::from_secs(1);
    failing = Some(later);
    encoder_progress(&mut failing, false, later + Duration::from_secs(1)).unwrap();
    assert!(encoder_progress(&mut failing, false, later + ENCODER_RECOVERY).is_err());
}
#[test]
fn wgc_interval_is_unrestricted_by_default_and_preserves_overrides() {
    let default = Config::default();
    assert!(!capture_config(&default).boolean("wgc_high_rate_capture", true));
    for (setting, expected) in [("true", true), ("false", false)] {
        let config = Config::parse(&format!("wgc_high_rate_capture = {setting}")).unwrap();
        let original = config.values.clone();
        assert_eq!(
            capture_config(&config).boolean("wgc_high_rate_capture", !expected),
            expected
        );
        assert_eq!(config.values, original);
    }
    assert!(!default.values.contains_key("wgc_high_rate_capture"));
}
/// A stream's shared video socket never refuses fragmentation: the flag would
/// govern every stream on the process-wide socket, and a stock client behind a
/// 1280-byte tunnel would lose its video. The PyroWave probe keeps the flag on
/// its own socket for the jumbo path check.
#[test]
fn streams_never_set_dont_fragment_on_the_shared_video_socket() {
    assert!(!super::super::video_refuses_fragmentation());
}

#[test]
fn shared_capture_separates_explicit_wgc_intervals_without_splitting_ddx() {
    let key = |kind, setting: &str| {
        CaptureKey::new(
            kind,
            "display",
            false,
            &capture_config(&Config::parse(setting).unwrap()),
            "",
            false,
        )
    };
    for kind in ["wgc", "auto"] {
        assert_eq!(key(kind, ""), key(kind, "wgc_high_rate_capture=false"));
        assert_ne!(key(kind, ""), key(kind, "wgc_high_rate_capture=true"));
        assert_ne!(key(kind, ""), key(kind, "wgc_drain_to_newest=true"));
        assert_ne!(key(kind, ""), key(kind, "wgc_helper_streaming_scope=true"));
    }
    for kind in ["ddx", "dxgi"] {
        assert_eq!(key(kind, ""), key(kind, "wgc_high_rate_capture=true"));
        assert_eq!(key(kind, ""), key(kind, "wgc_high_rate_capture=false"));
        assert_eq!(key(kind, ""), key(kind, "wgc_drain_to_newest=true"));
        assert_eq!(key(kind, ""), key(kind, "wgc_helper_streaming_scope=true"));
    }
    // Constant rate publishes between frames, so a stream with it never
    // shares a capture opened for variable-rate WGC. Streams of the same
    // method still share, and the capture publishes for the fastest of them
    // (`Latest::repeat_period`).
    let config = Config::default();
    assert_ne!(
        CaptureKey::new("wgc", "display", false, &config, "", false),
        CaptureKey::new("wgc", "display", false, &config, "", true)
    );
}
#[test]
fn shared_capture_respects_compute_overrides_and_independent_frame_phases() {
    let config = Config::default();
    let key = |config: &Config, phase: &str| {
        CaptureKey::new("wgc", "display", false, config, phase, false)
    };
    let default = key(&config, "first");
    assert_eq!(default, key(&config, "second"));
    let mut helper = config.clone();
    helper
        .values
        .insert("wgc_user_helper".into(), "true".into());
    assert_ne!(default, key(&helper, "first"));
    // The fence ring is the default: only turning it off changes the capture's identity.
    let mut explicit = config.clone();
    explicit
        .values
        .insert("idd_push_fence_ring".into(), "true".into());
    assert_eq!(default, key(&explicit, "first"));
    let mut keyed = config.clone();
    keyed
        .values
        .insert("idd_push_fence_ring".into(), "false".into());
    assert_ne!(default, key(&keyed, "first"));
    let mut changed = config.clone();
    changed
        .values
        .insert("wgc_compute_copy".into(), "false".into());
    assert_ne!(default, key(&changed, "first"));
    changed
        .values
        .insert("wgc_compute_copy".into(), "true".into());
    assert_eq!(default, key(&changed, "first"));
    changed
        .values
        .insert("gpu_compute_conversion".into(), "false".into());
    assert_ne!(default, key(&changed, "first"));
    changed
        .values
        .insert("wgc_slot_aligned_publish".into(), "true".into());
    assert_ne!(key(&changed, "first"), key(&changed, "second"));
}
#[test]
fn gamestream_hdr_falls_back_to_sdr_only_when_display_and_source_stay_sdr() {
    use butterpollo_session::session::Transport;
    assert!(super::super::should_fallback_hdr_to_sdr(
        Transport::GameStream,
        true,
        true,
        Some(false),
        false,
    ));
    // WebRTC handles missing HDR via require_hdr_source.
    assert!(!super::super::should_fallback_hdr_to_sdr(
        Transport::WebRtc,
        true,
        true,
        Some(false),
        false,
    ));
    // Physical HDR display whose Desktop Duplication returned BGRA8 keeps HDR.
    assert!(!super::super::should_fallback_hdr_to_sdr(
        Transport::GameStream,
        true,
        true,
        Some(true),
        false,
    ));
    // Intentional RTX HDR converting BGRA8 SDR -> HDR keeps HDR.
    assert!(!super::super::should_fallback_hdr_to_sdr(
        Transport::GameStream,
        true,
        true,
        Some(false),
        true,
    ));
    // Unreadable display color space avoids false fallback.
    assert!(!super::super::should_fallback_hdr_to_sdr(
        Transport::GameStream,
        true,
        true,
        None,
        false,
    ));
}

#[test]
fn disabling_hdr_updates_effective_config_info_and_hdr_control_payload() {
    let f = crate::state::test_support::Fixture::new();
    let mut negotiated = butterpollo_media_policy::rtsp::Negotiated {
        codec: 2,
        hdr: true,
        hdr_format: 1,
        ..Default::default()
    };
    negotiated.csc_mode = 2; // BT.709 limited SDR
    let client = f.client(u32::MAX);
    let session = Session::new(f.launch(client, Role::Stream), negotiated);

    assert!(session.hdr_active());
    assert_eq!(session.info()["hdr"], true);
    let initial_meta =
        session.set_display_hdr_metadata(butterpollo_media_policy::hdr::Metadata::default());
    assert_eq!(initial_meta.wire(session.hdr_active())[0], 1);
    let revision_before = session
        .hdr_revision
        .load(std::sync::atomic::Ordering::Acquire);

    assert!(session.disable_hdr());
    assert!(!session.disable_hdr());
    assert!(!session.hdr_active());
    assert_eq!(session.info()["hdr"], false);
    assert!(
        session
            .hdr_revision
            .load(std::sync::atomic::Ordering::Acquire)
            > revision_before
    );

    let effective = session.effective_config();
    assert!(!effective.hdr);
    assert!(effective.sdr_10bit);
    assert!(effective.ten_bit());
    assert_eq!(effective.hdr_format, 0);
    assert_eq!(effective.color_matrix(), 1);
    assert_eq!(initial_meta.wire(session.hdr_active())[0], 0);
}

#[test]
fn latest_needs_release_clears_when_generation_released() -> anyhow::Result<()> {
    let latest = super::super::capture::Latest::<u8>::new();
    latest.publish_captured(Arc::new(1), std::time::Instant::now())?;
    let consumer = latest.subscribe(std::sync::Weak::new())?;
    assert!(!latest.needs_release(&consumer));
    latest.begin_recovery()?;
    assert!(latest.needs_release(&consumer));
    assert!(!latest.consumers_released());
    latest.release_generation(&consumer);
    assert!(!latest.needs_release(&consumer));
    assert!(latest.consumers_released());
    Ok(())
}
