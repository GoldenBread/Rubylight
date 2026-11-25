//! Pacing mode, VRR display check, VrrSync, flip-latch guard, and fractional V-Sync tests.

use super::super::*;

#[test]
fn without_v_sync_tears_except_a_vrr_stream_on_wayland() {
    // (vrr, wayland, gamescope)
    assert!(prefers_immediate(false, false, false), "X11 without V-Sync tears");
    assert!(prefers_immediate(false, true, false), "Wayland without VRR tears");
    assert!(prefers_immediate(true, false, false), "X11 VRR takes IMMEDIATE");
    assert!(!prefers_immediate(true, true, false), "a VRR stream on a Wayland compositor keeps MAILBOX");
    assert!(prefers_immediate(true, true, true), "gamescope's VRR takes IMMEDIATE");
    assert_eq!(crate::sdl::present_mode_name(Some(1)), "Immediate (tears)");
    assert_eq!(crate::sdl::present_mode_name(Some(2)), "Mailbox (no tearing)");
    assert_eq!(crate::sdl::present_mode_name(Some(0)), "V-Sync (FIFO)");
    assert_eq!(crate::sdl::present_mode_name(None), "set by the renderer");
}

#[test]
fn the_default_config_shows_frames_at_host_times_with_vrr_and_waits_for_vsync_without() {
    // Real VRR: on screen it put 97% of frames on the game's cadence for 1.3 to 1.5 ms, and over
    // Wi-Fi halved the frames off it for 2.6 to 4.2 ms (docs/pyrowave-decode.md, "Real VRR").
    let settings = crate::config::Settings::default();
    assert!(settings.present_at_host_times);
    assert_eq!(pacing(&settings, true), (PacingMode::HostTimes, true));
    // No tearing support: VRR is not available and the display's refresh is awaited, as before.
    assert_eq!(pacing(&settings, false), (PacingMode::Vsync, false));
    let no_vrr = crate::config::Settings { vrr: false, ..settings.clone() };
    assert_eq!(pacing(&no_vrr, true), (PacingMode::Vsync, false));
    let no_vsync = crate::config::Settings { vrr: false, vsync: false, ..settings.clone() };
    assert_eq!(pacing(&no_vsync, true), (PacingMode::Immediate, false));
}

#[test]
fn pacing_preserves_vrr_without_host_time_delay() {
    let mut settings = crate::config::Settings { present_at_host_times: true, ..Default::default() };
    assert_eq!(pacing(&settings, true), (PacingMode::HostTimes, true));
    settings.present_at_host_times = false;
    assert_eq!(pacing(&settings, true), (PacingMode::Immediate, true));
    settings.present_at_host_times = true;
    settings.experimental_packed_444 = true;
    let expected = if cfg!(windows) {
        PacingMode::Immediate
    } else {
        PacingMode::HostTimes
    };
    assert_eq!(pacing(&settings, true), (expected, true));
    assert!(settings.present_at_host_times);
}

#[test]
fn only_a_display_that_scans_at_its_own_rate_turns_a_vrr_stream_to_v_sync() {
    use crate::stream::scanline::{Beam, RaceCapability, Reason, Verdict};
    let capability = RaceCapability { tearing: true, vrr: false, flip_inferred: false, scanline: true };
    let off = |reason| Verdict::Off(reason, capability);
    let fixed = off(Reason::NoVrr);
    let vsync = PacingMode::Vsync;
    assert_eq!(after_display_check(PacingMode::Immediate, &fixed, vsync), PacingMode::Vsync, "the laptop's 120 Hz panel");
    assert_eq!(after_display_check(PacingMode::HostTimes, &fixed, vsync), PacingMode::Vsync);
    assert_eq!(after_display_check(PacingMode::Vsync, &fixed, vsync), PacingMode::Vsync);
    // V-Sync turned off in the settings: what the stream would have without VRR.
    let immediate = PacingMode::Immediate;
    assert_eq!(after_display_check(PacingMode::Immediate, &fixed, immediate), PacingMode::Immediate);
    assert_eq!(after_display_check(PacingMode::HostTimes, &fixed, immediate), PacingMode::Immediate);
    // Still checking, VRR seen, and every reason that says nothing about the refresh keep the mode.
    let beam = Beam { rows: 1440, blank_rows: 200, total_rows: 1640, scan_ns: 4_100_000, refresh_hz: 243.9, lines_per_ms: 400.0,
        vblank_fraction: 0.7, scans: 50, idle_from: 2040, longest_idle_ns: 0, idle_scans: 0 };
    let mut kept = vec![Verdict::Checking, Verdict::On(beam, RaceCapability { vrr: true, flip_inferred: true, ..capability })];
    kept.extend([Reason::NoTearing, Reason::NoScanline, Reason::NoScan, Reason::NoRestart, Reason::Windowed, Reason::NoIdle, Reason::RowsDiffer, Reason::VrrNoScanline].map(off));
    for verdict in &kept {
        for mode in [PacingMode::Immediate, PacingMode::HostTimes, PacingMode::BeamRace, PacingMode::Vsync] {
            assert_eq!(after_display_check(mode, verdict, PacingMode::Vsync), mode, "{verdict:?}");
        }
    }
}

#[test]
fn the_vrr_sync_switch_sets_the_sync_interval_of_tearing_modes_only() {
    assert_eq!(VrrSync::parse(None), VrrSync::Off);
    assert_eq!(VrrSync::parse(Some("off")), VrrSync::Off);
    assert_eq!(VrrSync::parse(Some(" present1 ")), VrrSync::Present1);
    assert_eq!(VrrSync::parse(Some("scan")), VrrSync::Scan);
    assert_eq!(VrrSync::parse(Some("vsync")), VrrSync::Off, "an unknown value is said and left off");
    assert_eq!(VrrSync::default(), VrrSync::Off, "presents on arrival unless measured otherwise");
    for sync in [VrrSync::Off, VrrSync::Present1, VrrSync::Scan] {
        assert_eq!(sync_interval(PacingMode::Vsync, sync), 1);
        assert_eq!(sync_interval(PacingMode::BeamRace, sync), 0, "slices need tearing");
    }
    assert_eq!(sync_interval(PacingMode::Immediate, VrrSync::Off), 0);
    assert_eq!(sync_interval(PacingMode::HostTimes, VrrSync::Off), i32::from(cfg!(target_os = "macos")), "Real VRR tears, except on macOS");
    assert_eq!(sync_interval(PacingMode::Immediate, VrrSync::Present1), 1);
    assert_eq!(sync_interval(PacingMode::HostTimes, VrrSync::Present1), 1);
    assert_eq!(sync_interval(PacingMode::Immediate, VrrSync::Scan), 0, "waits on this thread, then tears");
}

#[test]
fn tear_free_vrr_waits_out_the_scan_on_windows_unless_the_switch_says_otherwise() {
    assert_eq!(VrrSync::choose(None, true, true), VrrSync::Scan);
    assert_eq!(VrrSync::choose(Some(" "), true, true), VrrSync::Scan, "an empty switch is no switch");
    assert_eq!(VrrSync::choose(None, false, true), VrrSync::Off, "the setting turned off");
    assert_eq!(VrrSync::choose(None, true, false), VrrSync::Off, "no scan line to read off Windows");
    assert_eq!(VrrSync::choose(Some("off"), true, true), VrrSync::Off, "the switch shows every frame at once");
    assert_eq!(VrrSync::choose(Some("blank"), true, true), VrrSync::Blank);
    assert_eq!(VrrSync::choose(Some("scan"), false, false), VrrSync::Scan, "the switch is taken as given");
    assert_eq!(VrrSync::choose(Some("present1"), false, true), VrrSync::Present1);
}

#[test]
fn the_blank_vrr_sync_parses_labels_and_leaves_the_interval_to_each_present() {
    assert_eq!(VrrSync::parse(Some("blank")), VrrSync::Blank);
    assert_eq!(VrrSync::Blank.label(), "blank");
    for sync in [VrrSync::Off, VrrSync::Present1, VrrSync::Scan, VrrSync::Blank] {
        assert_eq!(VrrSync::parse(Some(sync.label())), sync, "the label parses back");
    }
    assert_eq!(sync_interval(PacingMode::Immediate, VrrSync::Blank), 0, "the choice is made per present");
    assert_eq!(sync_interval(PacingMode::Vsync, VrrSync::Blank), 1);
}

#[test]
fn planned_latch_follows_the_display_period_headroom_and_the_floor_gap() {
    assert!(!planned_latch(None, 4167), "no flip before: nothing to latch against");
    assert!(planned_latch(Some(4000), 4167), "inside one display period");
    assert!(!planned_latch(Some(4500), 4167), "past one period plus 225 us and below the floor gap");
    assert!(!planned_latch(Some(19_999), 4167));
    assert!(planned_latch(Some(20_000), 4167), "the floor gap latches after a long gap");
}

// Upstream cases of `D3D11PresentPolicy::latch`, tests/vrr/tst_dxgipresent.cpp (commit 19758e5e6).
#[test]
fn latch_matches_the_upstream_protection_cases() {
    use std::cell::Cell;
    let fresh_blank = FlipObservation { predecessor_known: true, statistics_valid: true, pending: false, raster_valid: true, in_vertical_blank: true, raster_query_start_us: 1000, raster_query_end_us: 1050 };
    // Runs `latch` on one observation and clock, checking the query count the upstream test checks.
    let protect = |observation: FlipObservation, now_us: u64, window_us: u64| -> bool {
        let queries = Cell::new(0);
        let latched = latch(false, window_us, || { queries.set(queries.get() + 1); observation }, || now_us);
        assert_eq!(queries.get(), usize::from(window_us != 0), "enabled protection makes one observation");
        latched
    };
    assert!(!protect(fresh_blank, 1100, 8333), "a fresh blank with a known retired predecessor may retain adaptive presentation");
    let mut observation = fresh_blank;
    observation.predecessor_known = false;
    assert!(protect(observation, 1100, 8333), "startup or a missing predecessor ID must not permit an unsafe tearing flip");
    observation = fresh_blank;
    observation.pending = true;
    assert!(protect(observation, 1100, 8333), "a pending predecessor must remain protected even when the raster reports blank");
    observation.in_vertical_blank = false;
    assert!(protect(observation, 1100, 8333), "pending work during active scanout must remain protected");
    observation.pending = false;
    assert!(protect(observation, 1100, 8333), "an accounted-for predecessor can still be actively scanning out");
    observation = fresh_blank;
    observation.statistics_valid = false;
    assert!(protect(observation, 1100, 8333), "invalid or disjoint statistics must not become proof of a safe blank");
    observation = fresh_blank;
    observation.in_vertical_blank = false;
    assert!(protect(observation, 1100, 8333), "active scanout must stay protected before a subsequent query failure");
    observation.raster_valid = false;
    observation.in_vertical_blank = true;
    assert!(protect(observation, 1100, 8333), "a raster failure after active scanout must not reuse a blank flag");
    for _ in 0..8 {
        assert!(protect(observation, 1100, 8333), "repeated raster failures must never disable flip protection");
    }
    observation.statistics_valid = false;
    assert!(protect(observation, 1100, 8333), "missing both timing sources must select synchronized fallback");
    observation = fresh_blank;
    observation.raster_query_start_us = 0;
    assert!(protect(observation, 1100, 8333), "a blank without an observation timestamp must remain protected");
    observation = fresh_blank;
    observation.raster_query_end_us = 999;
    assert!(protect(observation, 1100, 8333), "a reversed query clock must not authorize tearing");
    assert!(protect(fresh_blank, 1049, 8333), "a future raster observation must not authorize tearing");
    assert!(!protect(fresh_blank, 1250, 8333), "the 250 us total-age limit must include its exact boundary");
    assert!(protect(fresh_blank, 1251, 8333), "an observation older than the 250 us cap must remain protected");
    observation = fresh_blank;
    observation.raster_query_end_us = 1251;
    assert!(protect(observation, 1251, 8333), "a slow raster query must not produce usable phase evidence");
    observation.raster_query_end_us = 1160;
    assert!(protect(observation, 1260, 8333), "query duration and preemption after the query must share one total-age budget");
    assert!(protect(fresh_blank, 2000, 8333), "a stale blank after preemption must not authorize tearing");
    assert!(!protect(fresh_blank, 1100, 1600), "the period-relative freshness limit must include its exact boundary");
    assert!(protect(fresh_blank, 1101, 1600), "a short display period must tighten freshness below the 250 us cap");
    assert!(protect(fresh_blank, 1050, 15), "a nonzero protection window with no precision budget must fail closed");
    assert!(!protect(FlipObservation::default(), 1100, 0), "zero must remain an explicit opt-out of native flip protection");
}

#[test]
fn latch_skips_the_observation_when_planned_or_disabled() {
    use std::cell::Cell;
    let fresh_blank = FlipObservation { predecessor_known: true, statistics_valid: true, pending: false, raster_valid: true, in_vertical_blank: true, raster_query_start_us: 1000, raster_query_end_us: 1050 };
    let queries = Cell::new(0);
    let clocks = Cell::new(0);
    let planned = latch(true, 8333, || { queries.set(queries.get() + 1); FlipObservation::default() }, || { clocks.set(clocks.get() + 1); 1100 });
    assert!(planned && queries.get() == 0 && clocks.get() == 0, "a planned latch bypasses the observation and the clock");
    let disabled = latch(false, 0, || { queries.set(queries.get() + 1); FlipObservation::default() }, || { clocks.set(clocks.get() + 1); 1100 });
    assert!(!disabled && queries.get() == 0 && clocks.get() == 0, "a disabled guard bypasses the observation and the clock");
    assert!(!latch(false, 8333, || fresh_blank, || 1100), "a fresh blank with valid statistics does not latch");
}

#[test]
fn a_vrr_streams_pacing_follows_the_display_check_once_and_sets_the_interval_once() {
    use crate::stream::scanline::{RaceCapability, Reason, Verdict};
    let capability = RaceCapability { tearing: true, vrr: false, flip_inferred: false, scanline: true };
    let fixed = Verdict::Off(Reason::NoVrr, capability);
    // V-Sync off: no pacing to change, but the present mode follows the display.
    let mut vrr = VrrPacing::new(false, PacingMode::Immediate, VrrSync::Off, PacingMode::Immediate).with_present_check(true);
    let mut mode = PacingMode::Immediate;
    assert!(vrr.display_vrr(), "VRR until the check says otherwise");
    assert_eq!(vrr.step(&mut mode, Some(&Verdict::Checking), 0), None);
    assert!(vrr.display_vrr());
    assert_eq!(vrr.step(&mut mode, Some(&fixed), 0), None, "no new sync interval");
    assert_eq!((mode, vrr.fell_back, vrr.display_vrr()), (PacingMode::Immediate, false, false));
    assert_eq!(vrr.step(&mut mode, Some(&Verdict::Off(Reason::VrrNoScanline, RaceCapability { vrr: true, ..capability })), 0), None);
    assert!(vrr.display_vrr(), "the display refreshes on present again");
    // Without either check, a verdict means nothing to it.
    let mut unchecked = VrrPacing::new(false, PacingMode::Immediate, VrrSync::Off, PacingMode::Immediate);
    unchecked.step(&mut mode, Some(&fixed), 0);
    assert!(unchecked.display_vrr());
    // Real VRR off: frames shown when decoded.
    let settings = crate::config::Settings { present_at_host_times: false, ..Default::default() };
    let (start, _) = pacing(&settings, true);
    assert_eq!(start, PacingMode::Immediate);
    let mut vrr = VrrPacing::new(true, without_vrr(&settings), VrrSync::Off, start);
    let mut mode = start;
    assert_eq!(vrr.step(&mut mode, None, 0), None, "no probe yet: the renderer keeps interval 0");
    assert_eq!(vrr.step(&mut mode, Some(&Verdict::Checking), 0), None);
    assert_eq!(vrr.step(&mut mode, Some(&fixed), 0), Some(1), "V-Sync waits for the refresh");
    assert_eq!((mode, vrr.fell_back), (PacingMode::Vsync, true));
    assert_eq!(vrr.step(&mut mode, Some(&fixed), 0), None, "given once");
    // V-Sync off in the settings: immediate stays, with nothing for the renderer where Real VRR tears.
    let no_vsync = crate::config::Settings { vsync: false, present_at_host_times: true, ..Default::default() };
    let (start, _) = pacing(&no_vsync, true);
    assert_eq!(start, PacingMode::HostTimes);
    let mut vrr = VrrPacing::new(true, without_vrr(&no_vsync), VrrSync::Off, start);
    let mut mode = start;
    assert_eq!(vrr.step(&mut mode, Some(&fixed), 0), interval_change(PacingMode::HostTimes, PacingMode::Immediate));
    assert_eq!((mode, vrr.fell_back), (PacingMode::Immediate, true));
    // NIGHTJAR_VRR_CHECK=0: the verdict is not applied; present1 still sets interval 1.
    let mut vrr = VrrPacing::new(false, PacingMode::Vsync, VrrSync::Present1, PacingMode::Immediate);
    let mut mode = PacingMode::Immediate;
    assert_eq!(vrr.step(&mut mode, Some(&fixed), 0), Some(1));
    assert_eq!((mode, vrr.fell_back), (PacingMode::Immediate, false));
    // A stream that starts on V-Sync has interval 1 already.
    let mut vrr = VrrPacing::new(false, PacingMode::Vsync, VrrSync::Off, PacingMode::Vsync);
    let mut mode = PacingMode::Vsync;
    assert_eq!(vrr.step(&mut mode, None, 0), None);
}

#[test]
fn a_v_sync_fall_back_is_rechecked_and_returns_once_the_display_refreshes_on_present() {
    use crate::stream::scanline::{Beam, RaceCapability, Reason, Verdict};
    let capability = RaceCapability { tearing: true, vrr: false, flip_inferred: false, scanline: true };
    let fixed = Verdict::Off(Reason::NoVrr, capability);
    let beam = Beam { rows: 2160, blank_rows: 90, total_rows: 2250, scan_ns: 8_310_000, refresh_hz: 120.3, lines_per_ms: 270.8,
        vblank_fraction: 0.45, scans: 80, idle_from: 2521, longest_idle_ns: 17_000_000, idle_scans: 30 };
    let on = Verdict::On(beam, RaceCapability { vrr: true, flip_inferred: true, ..capability });
    let idles = Verdict::Off(Reason::NoIdle, RaceCapability { vrr: true, ..capability });
    let busy = Verdict::Off(Reason::NoIdle, capability);
    let second = 1_000_000;
    let mut vrr = VrrPacing::new(true, PacingMode::Vsync, VrrSync::Off, PacingMode::Immediate);
    let mut mode = PacingMode::Immediate;
    // nightjar#72's resume: no VRR, V-Sync.
    assert!(vrr.probe_tears(mode));
    assert_eq!(vrr.step(&mut mode, Some(&fixed), 0), Some(1));
    assert!(vrr.probe_tears(mode), "rechecked with the tearing it had");
    assert!(!VrrPacing::new(true, PacingMode::Vsync, VrrSync::Off, PacingMode::Vsync).probe_tears(PacingMode::Vsync), "V-Sync from the settings");
    assert!(!vrr.recheck_due(0));
    assert_eq!(vrr.step(&mut mode, Some(&fixed), 0), None);
    assert!(!vrr.recheck_due(RECHECK_FIRST_US - 1), "nothing until the first recheck");
    assert!(vrr.recheck_due(RECHECK_FIRST_US));
    assert!(!vrr.recheck_due(RECHECK_FIRST_US), "asked once");
    // The probe measures again; the old verdict until its next present plans nothing.
    assert_eq!(vrr.step(&mut mode, Some(&fixed), RECHECK_FIRST_US), None);
    assert_eq!(vrr.step(&mut mode, Some(&Verdict::Checking), RECHECK_FIRST_US + second), None);
    assert!(!vrr.recheck_due(100 * RECHECK_MAX_US), "none while one runs");
    // At 120 fps on the 120 Hz display the presents never let it idle, and the calibration
    // saw no idle either: nothing says it refreshes on present, V-Sync stays and waits twice as long.
    let ended = RECHECK_FIRST_US + 2 * second;
    assert_eq!(vrr.step(&mut mode, Some(&busy), ended), None);
    assert_eq!(mode, PacingMode::Vsync);
    assert!(!vrr.recheck_due(ended + 2 * RECHECK_FIRST_US - 1));
    assert!(vrr.recheck_due(ended + 2 * RECHECK_FIRST_US));
    // Now the presents restart the scan: Immediate again, interval 0.
    assert_eq!(vrr.step(&mut mode, Some(&Verdict::Checking), 0), None);
    assert_eq!(vrr.step(&mut mode, Some(&on), 0), Some(0));
    assert_eq!((mode, vrr.fell_back, vrr.recovered), (PacingMode::Immediate, false, true));
    assert!(vrr.probe_tears(mode));
    assert_eq!(vrr.step(&mut mode, Some(&on), 0), None);
    assert!(!vrr.recheck_due(u64::MAX), "nothing to recheck on Immediate");
    // No VRR once more after returning: V-Sync for good, no flapping.
    assert_eq!(vrr.step(&mut mode, Some(&fixed), 0), Some(1));
    for at in [0, RECHECK_MAX_US, u64::MAX] {
        assert_eq!(vrr.step(&mut mode, Some(&fixed), at), None);
        assert!(!vrr.recheck_due(at));
    }
    assert_eq!(vrr.step(&mut mode, Some(&on), 0), Some(0), "the probe's own later verdict still counts");
    // A recheck with no idle to test, on a display the calibration saw idle, is no restart seen:
    // V-Sync stays; the waits never grow past the longest, and a passed check returns.
    let mut vrr = VrrPacing::new(true, PacingMode::Vsync, VrrSync::Off, PacingMode::HostTimes);
    let mut mode = PacingMode::HostTimes;
    vrr.step(&mut mode, Some(&fixed), 0);
    let mut at = 0;
    for _ in 0..10 {
        vrr.step(&mut mode, Some(&fixed), at);
        at += RECHECK_MAX_US;
        assert!(vrr.recheck_due(at));
    }
    assert_eq!(vrr.step(&mut mode, Some(&idles), at), None);
    assert_eq!(mode, PacingMode::Vsync);
    assert_eq!(vrr.step(&mut mode, Some(&on), at), interval_change(PacingMode::Vsync, PacingMode::HostTimes));
    assert_eq!(mode, PacingMode::HostTimes, "the pacing it had before");
}

/// The interval `VrrPacing::step` reports when the pacing changes: none where both pacings
/// present alike, as Real VRR and V-Sync do on macOS.
fn interval_change(from: PacingMode, to: PacingMode) -> Option<i32> {
    let (before, after) = (sync_interval(from, VrrSync::Off), sync_interval(to, VrrSync::Off));
    (before != after).then_some(after)
}

#[test]
fn a_linux_vrr_stream_on_a_display_without_vrr_waits_for_the_refresh() {
    use crate::stream::drm_vrr::{verdict, Reading};
    // nightjar#80 and the Fedora 8600G report: Linux has no scan line, so the check said "no
    // scanline" and the stream stayed immediate on a 144 Hz display without VRR. SDL's GPU
    // renderer takes vsync 0 as MAILBOX first, which at the panel's own rate replaces and
    // repeats pictures ("replaced before they were shown" at the stream's end), and HostTimes
    // was worse still (display wait p95 31-65 ms against 19 ms with V-Sync). DRM's
    // VRR_ENABLED 0 now turns both to V-Sync.
    let off = Reading::Vrr { enabled: false, output: "DP-1".into(), capable: Some(1) };
    let on = Reading::Vrr { enabled: true, output: "DP-1".into(), capable: Some(1) };
    let settings = crate::config::Settings::default();
    for start in [PacingMode::Immediate, PacingMode::HostTimes] {
        let mut vrr = VrrPacing::new(true, without_vrr(&settings), VrrSync::Off, start);
        let mut mode = start;
        assert_eq!(vrr.step(&mut mode, Some(&verdict(true, None)), 0), None, "still reading: the pacing stays");
        assert_eq!(vrr.step(&mut mode, Some(&verdict(true, Some(&off))), 0), interval_change(start, PacingMode::Vsync), "V-Sync waits for the refresh");
        assert_eq!((mode, vrr.fell_back), (PacingMode::Vsync, true));
        // A recheck that reads VRR on (the user turned it on) returns to the pacing it had.
        let tears = vrr.probe_tears(mode);
        assert!(tears, "rechecked with the pacing it had");
        assert_eq!(vrr.step(&mut mode, Some(&verdict(tears, Some(&on))), 0), interval_change(PacingMode::Vsync, start));
        assert_eq!(mode, start);
    }
    // Where VRR runs (KDE adaptive sync, GNOME VRR, gamescope) nothing changes.
    let mut vrr = VrrPacing::new(true, without_vrr(&settings), VrrSync::Off, PacingMode::Immediate);
    let mut mode = PacingMode::Immediate;
    assert_eq!(vrr.step(&mut mode, Some(&verdict(true, Some(&on))), 0), None);
    assert_eq!((mode, vrr.fell_back), (PacingMode::Immediate, false));
    // Nor where the output cannot be told: today's behaviour.
    assert_eq!(vrr.step(&mut mode, Some(&verdict(true, Some(&Reading::Unknown("two outputs".into())))), 0), None);
    assert_eq!(mode, PacingMode::Immediate);
    // V-Sync off in the settings: immediate, as without VRR.
    let no_vsync = crate::config::Settings { vsync: false, ..Default::default() };
    let mut vrr = VrrPacing::new(true, without_vrr(&no_vsync), VrrSync::Off, PacingMode::HostTimes);
    let mut mode = PacingMode::HostTimes;
    vrr.step(&mut mode, Some(&verdict(true, Some(&off))), 0);
    assert_eq!(mode, PacingMode::Immediate);
}

#[test]
fn pacing_keeps_the_non_vrr_fallback() {
    let mut settings = crate::config::Settings {
        experimental_packed_444: true,
        ..Default::default()
    };
    assert_eq!(pacing(&settings, false), (PacingMode::Vsync, false));
    settings.vrr = false;
    assert_eq!(pacing(&settings, true), (PacingMode::Vsync, false));
    settings.vsync = false;
    assert_eq!(pacing(&settings, true), (PacingMode::Immediate, false));
}

#[test]
fn fractional_v_sync_holds_v_sync_pictures_only_and_follows_the_rates() {
    let mut vrr = VrrPacing::new(false, PacingMode::Vsync, VrrSync::Off, PacingMode::Vsync).with_refreshes(2);
    assert_eq!(vrr.held(PacingMode::Vsync), 2, "60 fps on 120 Hz");
    assert_eq!(vrr.held(PacingMode::Immediate), 1, "VRR shows pictures as they come");
    assert_eq!(vrr.held(PacingMode::HostTimes), 1);
    vrr.set_refreshes(3);
    assert_eq!(vrr.held(PacingMode::Vsync), 3, "the stream went to 40 fps");
    vrr.set_refreshes(9);
    assert_eq!(vrr.held(PacingMode::Vsync), 4, "at most 4");
    vrr.set_refreshes(0);
    assert_eq!(vrr.held(PacingMode::Vsync), 1);
    // The renderer keeps sync interval 1: the stream waits for the turn itself.
    let mut mode = PacingMode::Vsync;
    let mut vrr = VrrPacing::new(false, PacingMode::Vsync, VrrSync::Off, PacingMode::Vsync).with_refreshes(2);
    assert_eq!(vrr.step(&mut mode, None, 0), None);
}

#[test]
fn a_display_whose_presents_start_no_scan_turns_tear_free_vrr_to_sync_interval_1() {
    // Tear-free VRR (`VrrSync::Scan`) presents with tearing after its own wait for the beam.
    for mode in [PacingMode::Immediate, PacingMode::HostTimes] {
        let mut vrr = VrrPacing::new(true, PacingMode::Vsync, VrrSync::Scan, mode);
        let mut first = mode;
        assert_eq!(vrr.step(&mut first, None, 0), None, "it starts at sync interval 0");
        assert!(vrr.distrust_scan());
        assert_eq!((vrr.sync, vrr.distrusted), (VrrSync::Present1, true));
        let mut now = mode;
        // macOS presents Real VRR with sync interval 1 already.
        let want = (sync_interval(mode, VrrSync::Scan) != 1).then_some(1);
        assert_eq!(vrr.step(&mut now, None, 0), want, "the next step gives the renderer sync interval 1");
        assert!(!vrr.probe_tears(mode), "the display check takes its presents as synchronised");
        assert_eq!(now, mode, "the pacing stays VRR's");
        assert!(!vrr.distrust_scan(), "once");
    }
    // A sync the switch chose stays, and so do Fastest (off) and the other measurement syncs.
    let mut forced = VrrPacing::new(true, PacingMode::Vsync, VrrSync::Scan, PacingMode::Immediate).with_wait(crate::config::VrrWait::Automatic, true);
    assert!(!forced.distrust_scan());
    assert_eq!(forced.sync, VrrSync::Scan);
    for sync in [VrrSync::Off, VrrSync::Blank, VrrSync::Present1] {
        let mut vrr = VrrPacing::new(true, PacingMode::Vsync, sync, PacingMode::Immediate);
        assert!(!vrr.distrust_scan(), "{sync:?}");
        assert_eq!((vrr.sync, vrr.distrusted), (sync, false));
    }
}

#[test]
fn whole_frames_under_vrr_picks_the_wait_unless_the_switch_set_one() {
    use crate::config::VrrWait;
    let scan = || VrrPacing::new(true, PacingMode::Vsync, VrrSync::Scan, PacingMode::Immediate);
    // Automatic: the beam's wait, which may still turn.
    let auto = scan().with_wait(VrrWait::Automatic, false);
    assert_eq!((auto.sync, auto.distrusted, auto.sync_forced), (VrrSync::Scan, false, false));
    // Nightjar's wait stays, whatever the display check finds.
    let mut kept = scan().with_wait(VrrWait::Scan, false);
    assert!(!kept.distrust_scan());
    assert_eq!(kept.sync, VrrSync::Scan);
    // Windows V-Sync: sync interval 1 from the first present.
    let mut vsync = scan().with_wait(VrrWait::VSync, false);
    assert_eq!((vsync.sync, vsync.distrusted), (VrrSync::Present1, true));
    let mut mode = PacingMode::Immediate;
    assert_eq!(vsync.step(&mut mode, None, 0), Some(1));
    // NIGHTJAR_VRR_SYNC wins over the setting.
    let set = scan().with_wait(VrrWait::VSync, true);
    assert_eq!((set.sync, set.distrusted, set.sync_forced), (VrrSync::Scan, false, true));
    // Fastest: nothing to wait for.
    let fastest = VrrPacing::new(true, PacingMode::Vsync, VrrSync::Off, PacingMode::Immediate).with_wait(VrrWait::VSync, false);
    assert_eq!((fastest.sync, fastest.distrusted), (VrrSync::Off, false));
}

/// A monitor a stream found not starting its scans is kept in the desktop config, so the next
/// start's Automatic wait turns to Windows V-Sync on it, as an earlier stream of the same run does.
#[test]
fn an_untrusted_monitor_starts_automatic_on_windows_v_sync_after_a_restart() {
    use crate::config::{ScanUntrusted, Store, VrrWait};
    let dir = std::env::temp_dir().join(format!("nightjar-scan-untrusted-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let display = r"\\?\DISPLAY#TEST-MONITOR-RESTART-111#5&1&0&UID111#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}";
    let store = Store::at(dir.clone()).unwrap();
    store.save_scan_untrusted(&ScanUntrusted { displays: vec![display.to_owned()] }).unwrap();
    // The next start reads it back before a stream starts.
    let kept = Store::at(dir.clone()).unwrap().scan_untrusted().displays;
    assert_eq!(kept, vec![display.to_owned()]);
    crate::stream::scan_sync::restore(&kept);
    assert!(crate::stream::scan_sync::untrusted(display), "the next start remembers it");
    // Its first probe reading of that display turns Automatic's wait to sync interval 1
    // (`render::distrust_scan_line`); a display not in the file keeps Nightjar's wait.
    let marked = crate::stream::scan_sync::untrusted(display);
    assert!(crate::stream::scan_sync::distrusts(0, marked, true, false).turn);
    assert!(!crate::stream::scan_sync::distrusts(0, false, true, false).turn);
    let mut vrr = VrrPacing::new(true, PacingMode::Vsync, VrrSync::Scan, PacingMode::Immediate).with_wait(VrrWait::Automatic, false);
    assert!(vrr.distrust_scan());
    assert_eq!((vrr.sync, vrr.distrusted), (VrrSync::Present1, true));
    std::fs::remove_dir_all(dir).ok();
}
