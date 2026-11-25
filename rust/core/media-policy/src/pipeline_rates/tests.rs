//! Unit tests for pipeline frame-rate accounting and source-rate warnings.
use super::*;

#[test]
fn unknown_stages_are_null_in_the_console_json() {
    let report = PipelineRatesReport {
        short_window: false,
        window_ms: 1000,
        source_mhz: 59_940,
        capture_mhz: 60_000,
        submit_mhz: UNKNOWN,
        output_mhz: 60_000,
        repeat_mhz: 0,
    };
    assert_eq!(
        serde_json::to_value(PipelineRates::from(&report)).unwrap(),
        serde_json::json!({
            "window_ms": 1000,
            "source_mhz": 59940,
            "capture_mhz": 60000,
            "submit_mhz": null,
            "output_mhz": 60000,
            "repeat_mhz": 0,
        })
    );
}

fn counts(source: u64, capture: u64, submit: u64, output: u64, repeat: u64) -> Counts {
    Counts {
        source,
        capture,
        submit,
        output,
        repeat,
    }
}

#[test]
fn counters_count_submits_repeats_and_captures() {
    let counters = Counters::default();
    counters.captured(2, 3);
    counters.submitted(false);
    counters.submitted(true);
    assert_eq!(counters.counts(7), counts(2, 3, 2, 7, 1));
}

#[test]
fn a_slot_counts_only_what_changed_and_a_new_slot_starts_over() {
    let mut follow = Follow::default();
    // Captures before the stream looked are not its own.
    assert_eq!(follow.advance(1, 40, 90), (0, 0));
    assert_eq!(follow.advance(1, 41, 92), (1, 2));
    assert_eq!(follow.advance(1, 41, 92), (0, 0));
    // A recreated capture's slot counts from zero again.
    assert_eq!(follow.advance(2, 5, 5), (0, 0));
    assert_eq!(follow.advance(2, 6, 7), (1, 2));
}

#[test]
fn rates_are_differences_over_the_window() {
    let rates = report(
        &counts(0, 10, 10, 10, 0),
        &counts(120, 250, 240, 240, 120),
        Duration::from_millis(2000),
        Duration::from_secs(1),
    );
    assert_eq!(
        rates,
        PipelineRatesReport {
            short_window: false,
            window_ms: 2000,
            source_mhz: 60_000,
            capture_mhz: 120_000,
            submit_mhz: 115_000,
            output_mhz: 115_000,
            repeat_mhz: 60_000,
        }
    );
    let short = report(
        &Counts::default(),
        &counts(0, 0, 0, 30, 0),
        Duration::from_millis(500),
        Duration::from_secs(1),
    );
    assert!(short.short_window);
    assert_eq!(short.output_mhz, 60_000);
}

#[test]
fn the_sampler_holds_its_rates_until_the_next_window_is_up() {
    let start = Instant::now();
    let second = Duration::from_secs(1);
    let mut sampler = Sampler::default();
    assert_eq!(sampler.sample(start, Counts::default(), second), None);
    assert_eq!(
        sampler.sample(start + second / 2, counts(0, 0, 0, 30, 0), second),
        None
    );
    let first = sampler
        .sample(start + second, counts(0, 0, 0, 60, 0), second)
        .unwrap();
    assert_eq!(first.output_mhz, 60_000);
    // A second reader soon after sees the same rates, not a short window.
    assert_eq!(
        sampler.sample(start + second + second / 10, counts(0, 0, 0, 66, 0), second),
        Some(first)
    );
    let next = sampler
        .sample(start + second * 2, counts(0, 0, 0, 90, 0), second)
        .unwrap();
    assert_eq!(next.output_mhz, 30_000);
}

#[test]
fn reports_follow_the_requested_interval_and_stop() {
    let start = Instant::now();
    let reporter = Reporter::default();
    assert_eq!(reporter.take(start, Counts::default), None);
    let on = PipelineRatesRequest {
        enabled: true,
        interval_ms: 500,
    };
    assert_eq!(
        reporter.on_request(start, &on.encode(), counts(0, 0, 0, 100, 0)),
        Some(true)
    );
    let half = Duration::from_millis(500);
    assert_eq!(
        reporter.take(start + half / 2, || unreachable!("not due")),
        None
    );
    let encoded = reporter
        .take(start + half, || counts(60, 60, 60, 160, 0))
        .unwrap();
    let report = PipelineRatesReport::decode(&encoded).unwrap();
    assert_eq!((report.window_ms, report.output_mhz), (500, 120_000));
    assert!(!report.short_window);
    assert_eq!(
        reporter.take(start + half + half / 2, || unreachable!("not due")),
        None
    );
    // A malformed request changes nothing.
    assert_eq!(
        reporter.on_request(start, &[1, 1, 0, 0], Counts::default()),
        None
    );
    assert!(
        reporter
            .take(start + half * 2, || counts(60, 60, 60, 220, 0))
            .is_some()
    );
    let off = PipelineRatesRequest {
        enabled: false,
        interval_ms: 500,
    };
    assert_eq!(
        reporter.on_request(start, &off.encode(), Counts::default()),
        Some(false)
    );
    assert_eq!(reporter.take(start + half * 10, Counts::default), None);
}

#[test]
fn the_gate_is_set_while_reports_are_on_and_clears_when_they_stop() {
    let start = Instant::now();
    let reporter = Reporter::default();
    assert!(!reporter.queued());
    assert_eq!(reporter.take(start, Counts::default), None);
    assert!(!reporter.queued(), "a quiet pass reads the gate clear");
    let on = PipelineRatesRequest {
        enabled: true,
        interval_ms: 500,
    };
    reporter.on_request(start, &on.encode(), counts(0, 0, 0, 100, 0));
    assert!(reporter.queued(), "reports are on");
    assert_eq!(
        reporter.take(start + Duration::from_millis(100), || {
            unreachable!("not due")
        }),
        None
    );
    assert!(
        reporter.queued(),
        "the next report comes due with time alone"
    );
    assert!(
        reporter
            .take(start + Duration::from_millis(500), || {
                counts(60, 60, 60, 160, 0)
            })
            .is_some()
    );
    assert!(reporter.queued(), "the report after keeps the gate set");
    let off = PipelineRatesRequest {
        enabled: false,
        interval_ms: 500,
    };
    reporter.on_request(start, &off.encode(), Counts::default());
    assert_eq!(
        reporter.take(start + Duration::from_secs(10), Counts::default),
        None
    );
    assert!(!reporter.queued(), "reports off: the gate clears");
}

#[test]
fn millihertz_counts_frames_per_window() {
    assert_eq!(millihertz(60, 1000), 60_000);
    assert_eq!(millihertz(599, 10_000), 59_900);
    assert_eq!(millihertz(0, 1000), 0);
    assert_eq!(millihertz(1, 0), UNKNOWN);
    // A count that would overflow stays below the unknown marker.
    assert_eq!(millihertz(u64::MAX, 1), UNKNOWN - 1);
}

const TARGET: u32 = 120_000;
/// A display whose refresh the host cannot read: the stream's rate is the
/// only limit.
const NO_DISPLAY: Option<u32> = None;

fn below(source_fps: u32, limit_fps: u32) -> Option<SourceRateWarning> {
    Some(SourceRateWarning::Below {
        source_mhz: source_fps * 1000,
        limit_mhz: limit_fps * 1000,
    })
}

fn at(
    watch: &mut SourceRate,
    start: Instant,
    n: u64,
    target: u32,
    source: u32,
) -> Option<SourceRateWarning> {
    at_display(watch, start, n, target, NO_DISPLAY, source)
}

/// A window of a stream at `stream` on a display refreshing at `display`
/// (`None` when unreadable).
fn at_display(
    watch: &mut SourceRate,
    start: Instant,
    n: u64,
    stream: u32,
    display: Option<u32>,
    source: u32,
) -> Option<SourceRateWarning> {
    watch.window(
        start + Duration::from_secs(5 * n),
        stream,
        display,
        Some(source),
    )
}

#[test]
fn a_brief_dip_below_the_stream_rate_is_not_a_warning() {
    let mut watch = SourceRate::default();
    let t = Instant::now();
    assert_eq!(at(&mut watch, t, 0, TARGET, 90_000), None);
    assert_eq!(at(&mut watch, t, 1, TARGET, 88_000), None);
    // One healthy window starts the streak over.
    assert_eq!(at(&mut watch, t, 2, TARGET, 120_000), None);
    assert_eq!(at(&mut watch, t, 3, TARGET, 90_000), None);
    assert_eq!(at(&mut watch, t, 4, TARGET, 90_000), None);
    assert_eq!(at(&mut watch, t, 5, TARGET, 90_000), below(90, 120));
    // A source at exactly 97% of the stream is not below it.
    let mut watch = SourceRate::default();
    for n in 0..10 {
        assert_eq!(at(&mut watch, t, n, TARGET, 116_400), None);
    }
}

#[test]
fn issue_124s_116_of_120_fps_warns_but_a_healthy_capped_119_does_not() {
    // The game in issue #124 rendered 116 of a 120 fps stream (96.7%): the
    // warning must fire after three windows, not stay silent as it did at
    // a 95% threshold.
    let mut slow = SourceRate::default();
    let t = Instant::now();
    assert_eq!(at(&mut slow, t, 0, TARGET, 116_000), None);
    assert_eq!(at(&mut slow, t, 1, TARGET, 116_000), None);
    assert_eq!(at(&mut slow, t, 2, TARGET, 116_000), below(115, 120));
    // A game capped at 119.x of 120 fps (99.5%) is healthy: no warning,
    // however long it runs.
    let mut capped = SourceRate::default();
    for n in 0..20 {
        assert_eq!(at(&mut capped, t, n, TARGET, 119_400), None);
    }
    // 116.5 of 120 (97.1%, the bundle's median window) is above the
    // threshold on its own; the sustained dips are what warn there.
    let mut median = SourceRate::default();
    for n in 0..20 {
        assert_eq!(at(&mut median, t, n, TARGET, 116_500), None);
    }
}

#[test]
fn a_slow_display_is_the_limit_and_its_source_does_not_warn() {
    let t = Instant::now();
    // A 60 Hz monitor streamed at 120 fps presents 60 pictures a second at
    // best: a desktop or game at 60 is not the limit, however long it runs.
    let mut watch = SourceRate::default();
    for n in 0..20 {
        assert_eq!(
            at_display(&mut watch, t, n, TARGET, Some(60_000), 59_940),
            None,
            "{n}"
        );
    }
    // Neither is one at the same rate on a display Windows reports below
    // the stream's, such as a 99 Hz panel.
    for n in 0..20 {
        assert_eq!(
            at_display(&mut watch, t, n, TARGET, Some(99_000), 99_000),
            None,
            "{n}"
        );
    }
    // The same 60 fps source is the limit of a 120 Hz display's stream.
    let mut watch = SourceRate::default();
    assert_eq!(
        at_display(&mut watch, t, 0, TARGET, Some(120_000), 60_000),
        None
    );
    assert_eq!(
        at_display(&mut watch, t, 1, TARGET, Some(120_000), 60_000),
        None
    );
    assert_eq!(
        at_display(&mut watch, t, 2, TARGET, Some(120_000), 60_000),
        below(60, 120)
    );
    // The 1000 Hz display of a VRR stream leaves the stream's own rate as
    // the limit: the issue #124 game warns there too.
    let mut watch = SourceRate::default();
    assert_eq!(
        at_display(&mut watch, t, 0, TARGET, Some(1_000_000), 116_000),
        None
    );
    assert_eq!(
        at_display(&mut watch, t, 1, TARGET, Some(1_000_000), 116_000),
        None
    );
    assert_eq!(
        at_display(&mut watch, t, 2, TARGET, Some(1_000_000), 116_000),
        below(115, 120)
    );
}

#[test]
fn a_sustained_slow_source_warns_after_three_windows_and_rewords_slowly() {
    let mut watch = SourceRate::default();
    let t = Instant::now();
    assert_eq!(at(&mut watch, t, 0, TARGET, 90_000), None);
    assert_eq!(at(&mut watch, t, 1, TARGET, 90_000), None);
    assert_eq!(at(&mut watch, t, 2, TARGET, 90_000), below(90, 120));
    // The number follows the rate, but not before `REWORD`.
    assert_eq!(at(&mut watch, t, 3, TARGET, 60_000), None);
    assert_eq!(at(&mut watch, t, 4, TARGET, 62_000), None);
    assert_eq!(at(&mut watch, t, 13, TARGET, 60_000), None);
    assert_eq!(at(&mut watch, t, 14, TARGET, 60_000), below(60, 120));
    // Once shown, a rate in the same 5 fps step says nothing new.
    assert_eq!(at(&mut watch, t, 15, TARGET, 59_000), None);
    // A re-word waits for `REWORD` from the last one.
    assert_eq!(at(&mut watch, t, 16, TARGET, 31_000), None);
    assert_eq!(at(&mut watch, t, 26, TARGET, 31_000), below(30, 120));
}

#[test]
fn the_warning_clears_only_after_thirty_seconds_at_the_rate() {
    let mut watch = SourceRate::default();
    let t = Instant::now();
    assert_eq!(at(&mut watch, t, 0, TARGET, 60_000), None);
    assert_eq!(at(&mut watch, t, 1, TARGET, 60_000), None);
    assert_eq!(at(&mut watch, t, 2, TARGET, 60_000), below(60, 120));
    // Five healthy windows do not clear it.
    for n in 3..8 {
        assert_eq!(at(&mut watch, t, n, TARGET, 120_000), None, "{n}");
    }
    // The sixth does.
    assert_eq!(
        at(&mut watch, t, 8, TARGET, 119_000),
        Some(SourceRateWarning::Cleared)
    );
    // A dip after the clear starts a new streak, not a warning.
    assert_eq!(at(&mut watch, t, 9, TARGET, 60_000), None);
    assert_eq!(at(&mut watch, t, 10, TARGET, 60_000), None);
    assert_eq!(at(&mut watch, t, 11, TARGET, 60_000), below(60, 120));
    // The new warning takes the full six healthy windows again.
    for n in 12..17 {
        assert_eq!(at(&mut watch, t, n, TARGET, 120_000), None, "{n}");
    }
    assert_eq!(
        at(&mut watch, t, 17, TARGET, 120_000),
        Some(SourceRateWarning::Cleared)
    );
}

#[test]
fn a_still_picture_never_warns_and_clears_a_standing_one() {
    let mut watch = SourceRate::default();
    let t = Instant::now();
    // A paused picture: no new frames are expected, however long it lasts.
    for n in 0..20 {
        assert_eq!(at(&mut watch, t, n, TARGET, 0), None);
    }
    assert_eq!(at(&mut watch, t, 20, TARGET, 60_000), None);
    assert_eq!(at(&mut watch, t, 21, TARGET, 60_000), None);
    assert_eq!(at(&mut watch, t, 22, TARGET, 60_000), below(60, 120));
    // A still screen after it is what the user sees once the game stops
    // updating: it counts toward clearing the warning, over the same six
    // windows, so the console does not keep a warning on an idle screen.
    for n in 23..28 {
        assert_eq!(at(&mut watch, t, n, TARGET, 0), None, "{n}");
    }
    assert_eq!(
        at(&mut watch, t, 28, TARGET, 0),
        Some(SourceRateWarning::Cleared)
    );
    // A slow picture after the clear starts a new streak, not a warning.
    assert_eq!(at(&mut watch, t, 29, TARGET, 60_000), None);
}

/// Five new pictures a second is a menu, a spinner or a loading screen, not a game whose
/// frame cap the user should raise: below the static floor it never warns.
#[test]
fn a_five_fps_menu_is_not_a_slow_game() {
    let mut watch = SourceRate::default();
    let t = Instant::now();
    for n in 0..20 {
        assert_eq!(at(&mut watch, t, n, TARGET, 5_000), None, "{n}");
    }
    // Ten fps is moving content: three complete windows still warn.
    assert_eq!(at(&mut watch, t, 20, TARGET, 10_000), None);
    assert_eq!(at(&mut watch, t, 21, TARGET, 10_000), None);
    assert_eq!(at(&mut watch, t, 22, TARGET, 10_000), below(10, 120));
}

/// The limit the warning carries is the stream's exact rate: a 144 fps stream is 144, not
/// the 145 that rounding its 1000 Hz display's refresh produced.
#[test]
fn the_limit_is_the_streams_exact_rate() {
    let mut watch = SourceRate::default();
    let t = Instant::now();
    assert_eq!(at(&mut watch, t, 0, 144_000, 125_000), None);
    assert_eq!(at(&mut watch, t, 1, 144_000, 125_000), None);
    assert_eq!(at(&mut watch, t, 2, 144_000, 125_000), below(125, 144));
    let mut vrr = SourceRate::default();
    assert_eq!(
        at_display(&mut vrr, t, 0, 144_000, Some(1_000_000), 70_000),
        None
    );
    assert_eq!(
        at_display(&mut vrr, t, 1, 144_000, Some(1_000_000), 70_000),
        None
    );
    assert_eq!(
        at_display(&mut vrr, t, 2, 144_000, Some(1_000_000), 70_000),
        below(70, 144)
    );
}

/// `idd-push` points to the virtual display capture rather than blaming the game alone, and
/// exact stream rates like 144 and 59.94 show as 144 and 60 fps.
#[test]
fn wording_matches_the_capture_backend_and_exact_stream_rate() {
    let pushed = below_message(true, true, 70_000, 144_000);
    assert!(
        pushed.contains("70 fps") && pushed.contains("144 fps"),
        "{pushed}"
    );
    assert!(
        pushed.contains("virtual display") && pushed.contains("game's frame rate"),
        "{pushed}"
    );
    assert!(!pushed.contains("The game is rendering"), "{pushed}");
    assert!(!pushed.contains("Raise the game's own"), "{pushed}");

    let game = below_message(false, true, 50_000, 59_940);
    assert!(game.contains("50 fps") && game.contains("60 fps"), "{game}");
    assert!(game.contains("Raise the game's own"), "{game}");

    let desktop = below_message(false, false, 30_000, 144_000);
    assert!(
        desktop.contains("30 times a second") && desktop.contains("144 fps"),
        "{desktop}"
    );
}

/// On `idd-push`, only blame the virtual display when `capture_fps` itself is
/// below the limit and matches `source_fps`, not when the driver is already
/// pushing full-rate or paired frames (`#146`).
#[test]
fn virtual_display_limited_distinguishes_true_low_delivery_from_paired_frames() {
    // True half-rate / 60 fps delivery on idd-push when display mode itself is low or unknown.
    assert!(virtual_display_limited(
        true,
        Some(72_000),
        Some(72_000),
        72_000,
        144_000
    ));
    assert!(virtual_display_limited(
        true,
        Some(81_000),
        Some(72_000),
        72_000,
        144_000
    ));
    assert!(virtual_display_limited(
        true,
        Some(60_000),
        Some(60_000),
        60_000,
        144_000
    ));
    assert!(virtual_display_limited(true, None, None, 72_000, 144_000));

    // Issue #146: on event-driven idd-push with a virtual display mode >= limit
    // (e.g. 1000 Hz VRR or 240/288 Hz), a 60 fps game is game-limited, not display-limited.
    assert!(!virtual_display_limited(
        true,
        Some(60_000),
        Some(1_000_000),
        60_000,
        144_000
    ));
    assert!(!virtual_display_limited(
        true,
        Some(72_000),
        Some(144_000),
        72_000,
        144_000
    ));

    // Full-rate or near-full-rate paired frames (< 2 ms apart): capture_fps is
    // 138-143 while source_fps reads 73-111, so do not blame the virtual display.
    assert!(!virtual_display_limited(
        true,
        Some(143_300),
        None,
        76_500,
        144_000
    ));
    assert!(!virtual_display_limited(
        true,
        Some(138_900),
        None,
        73_500,
        144_000
    ));
    assert!(!virtual_display_limited(
        true,
        Some(138_400),
        None,
        110_900,
        144_000
    ));

    // Non-idd-push or zero limit never uses the virtual-display wording.
    assert!(!virtual_display_limited(
        false,
        Some(72_000),
        None,
        72_000,
        144_000
    ));
    assert!(!virtual_display_limited(
        true,
        Some(72_000),
        None,
        72_000,
        0
    ));
}

#[test]
fn unjudgeable_windows_change_nothing_and_a_new_stream_rate_starts_over() {
    let mut watch = SourceRate::default();
    let t = Instant::now();
    for n in 0..10 {
        assert_eq!(
            watch.window(t + Duration::from_secs(5 * n), TARGET, NO_DISPLAY, None),
            None
        );
    }
    assert_eq!(watch.window(t, 0, NO_DISPLAY, Some(0)), None);
    assert_eq!(at(&mut watch, t, 0, TARGET, 60_000), None);
    assert_eq!(at(&mut watch, t, 1, TARGET, 60_000), None);
    assert_eq!(at(&mut watch, t, 2, TARGET, 60_000), below(60, 120));
    // The stream drops to 60: the old target no longer describes it.
    assert_eq!(
        at(&mut watch, t, 3, 60_000, 50_000),
        Some(SourceRateWarning::Cleared)
    );
    assert_eq!(at(&mut watch, t, 4, 60_000, 50_000), None);
    assert_eq!(at(&mut watch, t, 5, 60_000, 50_000), None);
    assert_eq!(at(&mut watch, t, 6, 60_000, 50_000), below(50, 60));
}

#[test]
fn a_display_that_changes_its_refresh_starts_the_streak_over() {
    let t = Instant::now();
    let mut watch = SourceRate::default();
    assert_eq!(
        at_display(&mut watch, t, 0, TARGET, Some(120_000), 100_000),
        None
    );
    assert_eq!(
        at_display(&mut watch, t, 1, TARGET, Some(120_000), 100_000),
        None
    );
    // The display drops to 100 Hz: 100 fps is its rate now, not a limit,
    // and the two windows of the old one count for nothing.
    assert_eq!(
        at_display(&mut watch, t, 2, TARGET, Some(100_000), 100_000),
        None
    );
    for n in 3..12 {
        assert_eq!(
            at_display(&mut watch, t, n, TARGET, Some(100_000), 100_000),
            None,
            "{n}"
        );
    }
    // A standing warning is cleared when the limit moves, the same as a
    // stream rate change.
    let mut watch = SourceRate::default();
    assert_eq!(
        at_display(&mut watch, t, 0, TARGET, Some(120_000), 60_000),
        None
    );
    assert_eq!(
        at_display(&mut watch, t, 1, TARGET, Some(120_000), 60_000),
        None
    );
    assert_eq!(
        at_display(&mut watch, t, 2, TARGET, Some(120_000), 60_000),
        below(60, 120)
    );
    assert_eq!(
        at_display(&mut watch, t, 3, TARGET, Some(60_000), 60_000),
        Some(SourceRateWarning::Cleared)
    );
}
