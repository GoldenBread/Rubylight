use super::*;
/// The decision as the capture worker makes it: the Capture method setting, the open
/// backend, whether the target is the display, the time without a picture, what is in
/// front of that display and whether Windows reports an exclusive-fullscreen app.
fn switches(
    capture: &str,
    backend: &str,
    display: bool,
    idle: Duration,
    in_front: InFront,
    exclusive_fullscreen: bool,
) -> bool {
    duplication_for_fullscreen(
        automatic_capture(capture),
        backend,
        display,
        idle,
        in_front,
        exclusive_fullscreen,
    )
}
#[test]
fn only_an_automatic_stream_moves_to_duplication_for_an_exclusive_fullscreen_app() {
    let stalled = FULLSCREEN_STALL;
    // The reported case: Automatic, WGC of the display, an exclusive-fullscreen emulator
    // covering it and no picture for 2 s.
    assert!(switches(
        "auto",
        "wgc",
        true,
        stalled,
        InFront::Covering,
        true
    ));
    assert!(switches(
        "",
        "wgc",
        true,
        stalled + Duration::from_secs(30),
        InFront::Covering,
        true
    ));
    // An explicit method is the user's choice, including for the one-reconnect
    // Windows Graphics Capture against Desktop Duplication comparison.
    for explicit in ["wgc", "wgcc", "ddx", "dxgi", "idd-push", "amd"] {
        assert!(
            !switches(explicit, "wgc", true, stalled, InFront::Covering, true),
            "{explicit}"
        );
    }
    for automatic in ["", "auto", " AUTO ", "Auto", "other", "WGCX"] {
        assert!(automatic_capture(automatic), "{automatic}");
    }
    for explicit in ["wgc", "wgcc", "ddx", "dxgi", "idd-push", "amd"] {
        assert!(!automatic_capture(explicit), "{explicit}");
    }
    // An unrecognised value is Automatic to the frame limiter, which opens the
    // normal WGC path: the fullscreen fallback follows it rather than treating
    // it as a method the user named.
    assert!(switches(
        "other",
        "wgc",
        true,
        stalled,
        InFront::Covering,
        true
    ));
    // A budget for a mode change, a busy GPU or a game leaving fullscreen.
    assert!(!switches(
        "auto",
        "wgc",
        true,
        stalled - Duration::from_millis(1),
        InFront::Covering,
        true
    ));
}
#[test]
fn a_static_desktop_a_paused_borderless_game_and_a_window_never_qualify() {
    let stalled = FULLSCREEN_STALL;
    // An idle desktop: Windows reports the shell window (Progman) as the foreground
    // window, and WGC sends nothing for a desktop that does not change.
    assert!(!switches(
        "auto",
        "wgc",
        true,
        stalled,
        InFront::Nothing,
        true
    ));
    // A paused or loading borderless game keeps its picture without presenting, and
    // Windows reports no exclusive-fullscreen app for a borderless window.
    assert!(!switches(
        "auto",
        "wgc",
        true,
        stalled,
        InFront::Covering,
        false
    ));
    // Another program's window beside the streamed display, while some other app is in
    // exclusive fullscreen.
    assert!(!switches(
        "auto",
        "wgc",
        true,
        stalled,
        InFront::Partial,
        true
    ));
    // One app's window is captured by WGC whatever runs, and only a display can be
    // duplicated; the display driver's push and Desktop Duplication read it already.
    assert!(!switches(
        "auto",
        "wgc",
        false,
        stalled,
        InFront::Covering,
        true
    ));
    for backend in ["ddx", "idd-push", "amd", "closed"] {
        assert!(!switches(
            "auto",
            backend,
            true,
            stalled,
            InFront::Covering,
            true
        ));
    }
}
#[test]
fn compositions_tell_a_focus_change_only_by_dirty_rectangles_inside_it() {
    // A game window at (100, 50)-(1380, 770) on the second display of a
    // desktop whose first display is 2560 wide.
    let display = [2560, 0, 2560 + 1920, 1080];
    let game = focus_on_display([2660, 50, 3940, 770], display);
    assert_eq!(game, Some([100, 50, 1380, 770]));
    // A window half off the display is clipped to it; one on another
    // display is no focus window here.
    assert_eq!(
        focus_on_display([4000, 900, 5000, 1300], display),
        Some([1440, 900, 1920, 1080])
    );
    assert_eq!(focus_on_display([0, 0, 2560, 1440], display), None);
    // Another window animating beside the game.
    assert_eq!(focus_changed(&[[1500, 0, 1900, 400]], game), Some(false));
    // A rectangle touching the game's edge does not overlap it.
    assert_eq!(focus_changed(&[[1380, 0, 1900, 400]], game), Some(false));
    // Any rectangle inside the game changes it.
    assert_eq!(
        focus_changed(&[[1500, 0, 1900, 400], [600, 300, 700, 400]], game),
        Some(true)
    );
    // No dirty rectangles (a forced repaint, or Windows before 11 24H2)
    // or no focus window on the display cannot tell.
    assert_eq!(focus_changed(&[], game), None);
    assert_eq!(focus_changed(&[[1500, 0, 1900, 400]], None), None);
    // A dropped composition's focus change survives into the published one.
    assert_eq!(merge_focus(Some(true), Some(false)), Some(true));
    assert_eq!(merge_focus(Some(false), Some(true)), Some(true));
    assert_eq!(merge_focus(None, Some(false)), Some(false));
    assert_eq!(merge_focus(Some(false), None), Some(false));
    assert_eq!(merge_focus(None, None), None);
}
#[test]
fn static_capture_sleeps_longer_only_without_a_pending_gpu_handoff() {
    let poll = Duration::from_micros(500);
    assert_eq!(
        notification_wait_interval(Duration::ZERO, false, poll),
        poll
    );
    assert_eq!(
        notification_wait_interval(Duration::from_millis(99), false, poll),
        poll
    );
    assert_eq!(
        notification_wait_interval(Duration::from_millis(100), false, poll),
        Duration::from_millis(10)
    );
    assert_eq!(
        notification_wait_interval(Duration::from_secs(1), true, poll),
        poll
    );
}
#[test]
fn another_window_extends_the_focus_picture_only_while_the_focus_changes() {
    let ms = 1_000_000;
    let mut cadence = Freshness::default();
    // A focus window changing every 150 ms (typing) does not animate:
    // another window's updates stay pictures of their own.
    for at in [100, 250] {
        cadence.observe_focus(at * ms, ms, Some(true));
        cadence.observe_focus((at + 4) * ms, ms, Some(false));
        assert!(!cadence.extends_picture());
        assert_eq!(cadence.picture_start(), Some((at + 4) * ms));
    }
    // Nor does one that repaints in short bursts (a terminal printing a
    // line): two repaints 12 ms apart.
    cadence.observe_focus(400 * ms, ms, Some(true));
    cadence.observe_focus(412 * ms, ms, Some(true));
    cadence.observe_focus(416 * ms, ms, Some(false));
    assert!(!cadence.extends_picture());
    assert_eq!(cadence.picture_start(), Some(416 * ms));
    // A game at 60 Hz animates once its frames have come for 500 ms.
    for frame in 0..32 {
        cadence.observe_focus((1000 + 16 * frame) * ms, ms, Some(true));
    }
    cadence.observe_focus(1500 * ms, ms, Some(false));
    assert!(!cadence.extends_picture());
    cadence.observe_focus(1512 * ms, ms, Some(true));
    let game = cadence.picture_start();
    assert_eq!(game, Some(1512 * ms));
    // The duplicate composition and another window's updates extend it.
    cadence.observe_focus(1513 * ms, ms, Some(true));
    assert!(!cadence.extends_picture());
    for at in [1517, 1521, 1525] {
        cadence.observe_focus(at * ms, ms, Some(false));
        assert!(cadence.extends_picture());
        assert_eq!(cadence.picture_start(), game);
    }
    // A focus change soon after another window's update is a new picture.
    cadence.observe_focus(1526 * ms, ms, Some(true));
    assert!(!cadence.extends_picture());
    assert_eq!(cadence.picture_start(), Some(1526 * ms));
    cadence.observe_focus(1566 * ms, ms, Some(false));
    assert!(cadence.extends_picture());
    // Once the focus window is still, every update is a picture again.
    cadence.observe_focus(1577 * ms, ms, Some(false));
    assert!(!cadence.extends_picture());
    assert_eq!(cadence.picture_start(), Some(1577 * ms));
    // Without dirty regions nothing changes.
    cadence.observe_focus(1581 * ms, ms, None);
    assert!(!cadence.extends_picture());
    assert_eq!(cadence.picture_start(), Some(1581 * ms));
    // A hitch longer than 50 ms starts the 500 ms over.
    cadence.observe_focus(1700 * ms, ms, Some(true));
    cadence.observe_focus(1716 * ms, ms, Some(true));
    cadence.observe_focus(1720 * ms, ms, Some(false));
    assert!(!cadence.extends_picture());
}
fn learned(interval: u64) -> Freshness {
    let mut cadence = Freshness::default();
    for frame in 0..=16 {
        cadence.observe(frame * interval, 500_000);
    }
    cadence
}
#[test]
fn constant_rate_fills_each_slot_once_and_never_holds_a_new_composition() {
    // 180 fps.
    let period = Duration::from_micros(5_555);
    let start = Instant::now();
    let mut rate = ConstantRate::default();
    // Nothing has been captured: no picture to publish again, no slot yet.
    assert_eq!(rate.due(period), None);
    assert_eq!(rate.decide(start, period, false), None);
    // A new composition goes out at once, before its slot.
    assert_eq!(rate.decide(start, period, true), Some(Publish::Fresh));
    rate.published(start);
    // Only from the slot on is the last picture published again.
    assert_eq!(rate.due(period), Some(start + period));
    assert_eq!(
        rate.decide(start + period / 2, period, false),
        Some(Publish::Wait(start + period))
    );
    assert_eq!(
        rate.decide(start + period, period, false),
        Some(Publish::Repeat)
    );
    // A repeat starts the next period where it happened: a publisher back
    // from a stall publishes one picture, not the slots it missed.
    let late = start + period * 3;
    rate.published(late);
    assert_eq!(
        rate.decide(late + period / 4, period, false),
        Some(Publish::Wait(late + period))
    );
    // The slot follows a new stream rate.
    assert_eq!(
        rate.decide(start + period * 4, period / 2, false),
        Some(Publish::Repeat)
    );
}
#[test]
fn late_wakeups_do_not_slow_repeats_below_the_configured_rate() {
    // 180 fps; every slot is noticed 0.3 ms late.
    let period = Duration::from_micros(5_555);
    let late = Duration::from_micros(300);
    let start = Instant::now();
    let mut rate = ConstantRate::default();
    rate.published(start);
    let mut last = start;
    for _ in 0..1000 {
        let due = rate.due(period).unwrap();
        let now = due + late;
        assert_eq!(rate.decide(now, period, false), Some(Publish::Repeat));
        rate.repeated(now, period);
        last = now;
    }
    let average = (last - start - late).as_secs_f64() / 1000.0;
    assert!((average / period.as_secs_f64() - 1.0).abs() < 0.001);
    assert_eq!(rate.due(period), Some(start + period * 1001));
}
#[test]
fn a_stall_gives_one_repeat_and_a_new_frame_reanchors() {
    let period = Duration::from_micros(5_555);
    let start = Instant::now();
    let mut rate = ConstantRate::default();
    rate.published(start);
    // Back from a stall of many periods: one picture, the grid restarts.
    let now = start + period * 10;
    assert_eq!(rate.decide(now, period, false), Some(Publish::Repeat));
    rate.repeated(now, period);
    assert_eq!(
        rate.decide(now + period / 2, period, false),
        Some(Publish::Wait(now + period))
    );
    // Exactly one period late is a stall, not a grid slot.
    let at = now + period * 2;
    rate.repeated(at, period);
    assert_eq!(rate.due(period), Some(at + period));
    // A new frame re-anchors at its arrival.
    let fresh = at + period / 3;
    rate.published(fresh);
    assert_eq!(rate.due(period), Some(fresh + period));
}
#[test]
fn grid_next_stays_on_the_grid_only_for_a_fill_less_than_a_period_late() {
    let period = Duration::from_millis(5);
    let due = Instant::now();
    assert_eq!(grid_next(due, due, period), due);
    assert_eq!(grid_next(due, due + period / 2, period), due);
    assert_eq!(grid_next(due, due + period, period), due + period);
    // Filled before it was due (nothing waited): counts from now.
    assert_eq!(grid_next(due + period, due, period), due);
    // The resend loop's 0.3 ms latency per slot keeps the average period.
    let mut at = due;
    for _ in 0..1000 {
        at = grid_next(
            at + period,
            at + period + Duration::from_micros(300),
            period,
        );
    }
    assert_eq!(at, due + period * 1000);
}
#[test]
fn a_constant_rate_repeat_is_a_picture_of_its_own() {
    let ms = 1_000_000;
    let mut cadence = Freshness::default();
    // A 90 fps game on a 180 fps stream: every second picture is a repeat.
    cadence.observe_focus(0, ms, Some(true));
    for at in 1..=4u64 {
        cadence.observe_repeat(at * 5_555_000, ms);
    }
    assert_eq!(cadence.picture_interval(), Some(5_555_000));
    assert!(!cadence.extends_picture());
    // Even beside an animating focus window a repeat is a publication of its
    // own, not that window extending the game's picture.
    cadence.observe_focus(30 * ms, ms, Some(true));
    cadence.observe_focus(35 * ms, ms, Some(true));
    cadence.observe_repeat(40 * ms, ms);
    assert_eq!(cadence.picture_start(), Some(40 * ms));
    assert!(!cadence.extends_picture());
}
#[test]
fn a_real_frame_right_after_a_repeat_is_a_picture_of_its_own() {
    let ms = 1_000_000;
    let mut cadence = Freshness::default();
    cadence.observe_focus(0, ms, None);
    cadence.observe_repeat(5 * ms, ms);
    // The game's next frame 1 ms after the repeat: captures less than 2 ms
    // apart are one picture otherwise, and the capture cannot tell either.
    cadence.observe_focus(6 * ms, ms, None);
    assert_eq!(cadence.picture_start(), Some(6 * ms));
    // Without a repeat the 2 ms rule still merges these.
    cadence.observe_focus(7 * ms, ms, None);
    assert_eq!(cadence.picture_start(), Some(6 * ms));
}
#[test]
fn cached_statistics_match_the_original_policy_across_cadence_changes_and_resets() {
    fn reference(cadence: &Freshness, age: u64, normal: u64) -> u64 {
        if cadence.count < 8 {
            return normal;
        }
        let mut intervals = cadence.intervals;
        intervals[..cadence.count].sort_unstable();
        let source = intervals[cadence.count / 2];
        if source < 3_000_000
            || intervals[0] < source.saturating_mul(7) / 8
            || intervals[cadence.count - 1] > source.saturating_mul(9) / 8
        {
            return normal;
        }
        let mut delays = cadence.delays;
        delays[..cadence.count].sort_unstable();
        let expected = source.saturating_add(delays[(cadence.count - 1) / 10]);
        let early = expected.saturating_sub(1_000_000);
        if age < early {
            normal.min(early - age)
        } else if age <= expected.saturating_add(500_000) {
            normal.min(100_000)
        } else {
            normal
        }
    }
    let mut cadence = Freshness::default();
    let mut captured = 1_000_000_000u64;
    for frame in 0..2000u64 {
        let period = [16_666_667, 8_333_333, 2_000_000, 20_000_000][(frame / 40 % 4) as usize];
        captured = match frame % 97 {
            0 => captured.saturating_sub(50_000_000),
            1 => captured,
            2 => captured + 600_000_000,
            _ => captured + period + (frame % 5) * 10_000,
        };
        cadence.observe(captured, (frame % 17) * 100_000);
        let mut intervals = cadence.intervals;
        intervals[..cadence.count].sort_unstable();
        assert_eq!(
            cadence.median_interval(),
            (cadence.count >= 4).then(|| intervals[cadence.count / 2])
        );
        for age in [0, period - 1_000_000, period, period + 2_000_000, u64::MAX] {
            for normal in [50_000, 1_000_000, 10_000_000] {
                assert_eq!(
                    cadence.poll_wait(age, normal),
                    reference(&cadence, age, normal)
                );
            }
        }
    }
}
#[test]
fn a_second_composition_of_a_picture_does_not_shorten_the_picture_interval() {
    // WGC on a 240 Hz VRR display: each frame of a 120 Hz game arrives
    // with another composition 0.1 or 1 ms later; in some sessions every
    // fourth frame has a third 3.5 ms later.
    for third in [false, true] {
        let mut cadence = Freshness::default();
        for frame in 0..200u64 {
            let jitter = (frame.wrapping_mul(2_654_435_761) >> 7) % 600_000;
            let mut start = 1_000_000_000 + frame * 8_333_333 + jitter;
            cadence.observe(start, 100_000);
            let second = start + [100_000, 1_000_000][(frame * 5 % 3).min(1) as usize];
            cadence.observe(second, 100_000);
            if third && frame % 4 == 1 {
                start += 3_500_000;
                cadence.observe(start, 100_000);
            }
            if frame >= 20 {
                let picture = cadence.picture_interval().unwrap();
                assert!(
                    (7_833_333..=8_833_333).contains(&picture),
                    "third={third} frame {frame}: {picture}"
                );
                assert_eq!(cadence.picture_start(), Some(start));
                assert!(cadence.median_interval().unwrap() < 7_500_000);
                // The mean of all captures halves; the pictures' own mean, with a
                // picture split by a third composition counted once, still agrees
                // with their median, so step mode can tell the cadence.
                let mean = cadence.picture_mean_interval().unwrap();
                assert!(
                    mean.abs_diff(picture) <= picture / 16,
                    "third={third} frame {frame}: {mean} {picture}"
                );
                assert!(cadence.mean_interval().unwrap() < 5_000_000);
            }
        }
    }
    // 160 fps on the 240 Hz display: pictures one and two refreshes apart
    // in turn. Their mean stays apart from the median, so step mode stays off.
    let mut alternating = Freshness::default();
    let mut at = 1_000_000_000u64;
    for picture in 0..64u64 {
        at += [4_166_667, 8_333_333][(picture % 2) as usize];
        alternating.observe(at, 100_000);
        alternating.observe(at + 100_000, 100_000);
    }
    let median = alternating.picture_interval().unwrap();
    let mean = alternating.picture_mean_interval().unwrap();
    assert!(mean.abs_diff(median) > median / 8, "{median} {mean}");
    // The 1000 Hz virtual display's own refresh is no picture cadence.
    let mut display = Freshness::default();
    for tick in 0..100 {
        display.observe(tick * 1_000_000, 100_000);
    }
    assert_eq!(display.picture_interval(), None);
    assert_eq!(display.median_interval(), Some(1_000_000));
    // A source without paired compositions has the same median either way.
    let mut cadence = Freshness::default();
    for frame in 0..100u64 {
        cadence.observe(frame * 16_666_667 + frame * 7_919 % 900_000, 100_000);
        assert_eq!(cadence.picture_interval(), cadence.median_interval());
    }
    // A restart starts the picture history over.
    cadence.observe(1, 100_000);
    assert_eq!(cadence.picture_interval(), None);
    assert_eq!(cadence.picture_start(), Some(1));
}
#[test]
fn high_refresh_captures_count_source_pictures_without_chaining_below_two_ms() {
    let ms = 1_000_000;
    // 1000 Hz virtual display (1 ms spacing): picture_interval stays None for
    // pacing, but source_fps counting must not collapse to 1 picture total.
    let mut hz1000 = Freshness::default();
    let mut counted = 0u64;
    for tick in 0..1000u64 {
        hz1000.observe(tick * ms, 100_000);
        if hz1000.new_source_picture() {
            counted += 1;
        }
    }
    assert_eq!(hz1000.picture_interval(), None);
    assert_eq!(counted, 500);

    // 576 Hz virtual display (1.736 ms spacing): every second capture crosses
    // the 2 ms merge window instead of chaining indefinitely.
    let mut hz576 = Freshness::default();
    let mut counted = 0u64;
    for tick in 0..576u64 {
        hz576.observe(tick * 1_736_111, 100_000);
        if hz576.new_source_picture() {
            counted += 1;
        }
    }
    assert_eq!(hz576.picture_interval(), None);
    assert_eq!(counted, 288);

    // WGC 60 Hz game with a second composition 0.5 ms later (and even a third
    // 1.2 ms after the first) still counts once per game frame.
    let mut wgc = Freshness::default();
    let mut counted = 0u64;
    for frame in 0..60u64 {
        let base = 1_000_000_000 + frame * 16_666_667;
        for offset in [0, 500_000, 1_200_000] {
            wgc.observe(base + offset, 100_000);
            if wgc.new_source_picture() {
                counted += 1;
            }
        }
    }
    assert_eq!(counted, 60);

    // Identical timestamps never double-count; repeats never count; a real
    // capture 1 ms after a repeat counts, and its duplicate 1 ms later does not.
    let mut edge = Freshness::default();
    edge.observe(10 * ms, 100_000);
    assert!(edge.new_source_picture());
    edge.observe(10 * ms, 100_000);
    assert!(!edge.new_source_picture());
    edge.observe_repeat(15 * ms, 100_000);
    assert!(!edge.new_source_picture());
    edge.observe(16 * ms, 100_000);
    assert!(edge.new_source_picture());
    edge.observe(17 * ms, 100_000);
    assert!(!edge.new_source_picture());

    // Background window updates beside an animating focus window do not count.
    let mut focused = Freshness::default();
    for frame in 0..40u64 {
        focused.observe_focus((1000 + 16 * frame) * ms, 100_000, Some(true));
        assert!(focused.new_source_picture());
    }
    let mut bg_counted = 0u64;
    for offset in [4, 8, 12] {
        focused.observe_focus((1000 + 16 * 39 + offset) * ms, 100_000, Some(false));
        if focused.new_source_picture() {
            bg_counted += 1;
        }
    }
    assert_eq!(bg_counted, 0);
}
#[test]
fn predictive_polling_is_bounded_and_static_or_irregular_sources_use_normal_waits() {
    let mut cadence = learned(8_333_333);
    assert_eq!(cadence.poll_wait(2_000_000, 1_000_000), 1_000_000);
    assert_eq!(cadence.poll_wait(7_500_000, 1_000_000), 333_333);
    assert_eq!(cadence.poll_wait(8_000_000, 1_000_000), 100_000);
    assert_eq!(cadence.poll_wait(10_000_000, 1_000_000), 1_000_000);
    assert_eq!(cadence.poll_wait(1_000_000_000, 1_000_000), 1_000_000);
    cadence.observe(16 * 8_333_333 + 20_000_000, 500_000);
    assert_eq!(cadence.poll_wait(8_000_000, 1_000_000), 1_000_000);
    assert_eq!(
        Freshness::default().poll_wait(8_000_000, 1_000_000),
        1_000_000
    );
    assert_eq!(
        learned(1_000_000).poll_wait(1_000_000, 1_000_000),
        1_000_000
    );
}
#[test]
fn publication_preserves_static_updates_and_a_waiting_encode_slot() {
    assert_eq!(
        publication_deadline(20_000_000, 3_000_000, 8_333_333, 0),
        Some(19_750_000)
    );
    assert_eq!(
        publication_deadline(20_000_000, 19_000_000, 8_333_333, 0),
        None
    );
    assert_eq!(
        publication_deadline(20_000_000, 21_000_000, 8_333_333, 0),
        None
    );
    assert_eq!(
        publication_deadline(20_000_000, 3_000_000, 12_001_000, 0),
        None
    );
    assert_eq!(publication_deadline(0, 3, 1, 0), None);
    let mut history = Intervals::default();
    for time in [0, 8_000_000, 16_000_000, 24_000_000] {
        assert_eq!(history.observe(time), 0);
    }
    assert_eq!(history.observe(32_000_000), 8_000_000);
    assert_eq!(history.observe(432_000_000), 8_000_000);
}
