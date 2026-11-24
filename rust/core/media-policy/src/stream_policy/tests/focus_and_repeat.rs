//! Tests for focus-window labeling, background repaint filtering, and repeat grace.

use super::*;

#[test]
fn another_window_animating_on_the_display_does_not_pace_the_focused_game() {
    // Hardware: a 60 Hz game streamed at 60 fps under VRR on a 240 Hz display
    // while another window presented 245/s on it. Every composition counted
    // as a picture, claims ran at 60.1/s against the game's 60.0 and drifted
    // through its frames: repeats and skips, and up to a frame of extra age
    // the host could not see. With dirty regions, only game frames start a
    // picture and a still game picture waits for the next one.
    let mut unlabelled_repeats = 0;
    for other in [1000. / 245., 1000. / 240., 1000. / 144.] {
        for seed in 0..4 {
            let (compositions, frames) = mixed_desktop(25., other, seed * 10);
            for (labelled, duplicate_other) in [(false, false), (true, false), (true, true)] {
                let claims = simulate_focus(
                    1000. / 60.,
                    &focus_labels(&compositions, labelled, duplicate_other),
                    |i| 0.08 + 0.12 * noise(i, seed + 99),
                    vrr_desktop,
                );
                let steady = &claims[300..];
                let (repeats, skipped, behind) = game_frames_shown(steady, &frames);
                if !labelled {
                    unlabelled_repeats += repeats;
                    continue;
                }
                let shown = |at: f64| frames.partition_point(|f| *f <= at + 1e-9);
                let mut ages: Vec<f64> = steady
                    .iter()
                    .map(|(claim, presented)| claim - frames[shown(*presented).max(1) - 1])
                    .collect();
                ages.sort_by(f64::total_cmp);
                let rate =
                    (steady.len() - 1) as f64 * 1000. / (steady.last().unwrap().0 - steady[0].0);
                let case =
                    format!("other {other:.2} ms seed {seed} duplicate other {duplicate_other}");
                assert_eq!((repeats, skipped, behind), (0, 0, 0), "{case}");
                assert!(rate < 60.05, "{case}: {rate:.3}/s");
                assert!(
                    ages[ages.len() * 95 / 100] < 1.,
                    "{case}: game age p95 {:.2} ms",
                    ages[ages.len() * 95 / 100]
                );
            }
        }
    }
    // Without dirty regions the pacer behaves as before, which repeats frames.
    assert!(unlabelled_repeats > 0);
}
#[test]
fn another_window_still_streams_at_the_stream_rate_once_the_game_is_still() {
    // A focus window that stops changing (a paused game, a menu) must not
    // hold the rest of the display to the static repeat.
    let (compositions, frames) = mixed_desktop_at(1000. / 60., 2., 6., 1000. / 240., 0);
    let claims = simulate_focus(
        1000. / 60.,
        &focus_labels(&compositions, true, false),
        |i| 0.08 + 0.12 * noise(i, 99),
        vrr_desktop,
    );
    let still = frames.last().unwrap() + 200.;
    let after: Vec<f64> = claims.iter().map(|c| c.0).filter(|c| *c > still).collect();
    let rate = (after.len() - 1) as f64 * 1000. / (after.last().unwrap() - after[0]);
    assert!((59.5..60.5).contains(&rate), "{rate:.2}/s");
    // A 30 Hz game is still a game picture every 33 ms, not 60 claims.
    let (compositions, frames) = mixed_desktop_at(1000. / 30., 25., 25., 1000. / 240., 0);
    let claims = simulate_focus(
        1000. / 60.,
        &focus_labels(&compositions, true, false),
        |i| 0.08 + 0.12 * noise(i, 99),
        vrr_desktop,
    );
    let steady = &claims[150..];
    assert_eq!(game_frames_shown(steady, &frames), (0, 0, 0));
}
#[test]
fn a_pointer_moving_beside_the_focused_game_does_not_cost_it_a_frame() {
    // Hardware: the same desktop with the pointer circling outside the game
    // at 125 Hz. WGC draws the cursor and reports no dirty region for a
    // composition that only moved it, so it came unlabelled, within the
    // picture gap of another window's update, and ended the extension: the
    // claim no longer waited for the predicted game frame and took the
    // previous one just before the next arrived, a game frame late.
    for seed in 0..4 {
        let (compositions, frames) = mixed_desktop(25., 1000. / 240., seed * 10);
        let mut labelled = focus_labels(&compositions, true, false);
        let mut at = 1.7;
        let mut index = 0;
        while at < 25_000. {
            // A move within 0.05 ms of another update is composed with it.
            let next = compositions.partition_point(|c| c.0 < at);
            let near = |i: usize| compositions.get(i).is_some_and(|c| (c.0 - at).abs() < 0.05);
            if !near(next) && !(next > 0 && near(next - 1)) {
                labelled.push((at, None));
            }
            at += 8. + 0.4 * (noise(index, seed + 3) - 0.5);
            index += 1;
        }
        labelled.sort_by(|a, b| a.0.total_cmp(&b.0));
        let claims = simulate_focus(
            1000. / 60.,
            &labelled,
            |i| 0.08 + 0.12 * noise(i, seed + 99),
            vrr_desktop,
        );
        let steady = &claims[300..];
        let shown = |at: f64| frames.partition_point(|f| *f <= at + 1e-9);
        let mut ages: Vec<f64> = steady
            .iter()
            .map(|(claim, presented)| claim - frames[shown(*presented).max(1) - 1])
            .collect();
        ages.sort_by(f64::total_cmp);
        let case = format!("seed {seed}");
        assert_eq!(game_frames_shown(steady, &frames), (0, 0, 0), "{case}");
        assert!(
            ages[ages.len() * 95 / 100] < 1.,
            "{case}: game age p95 {:.2} ms",
            ages[ages.len() * 95 / 100]
        );
    }
}
#[test]
fn a_game_alone_on_the_display_is_claimed_as_without_focus_labels() {
    // Holding a later composition of a claimed frame by its timing measured
    // no better (rust/PERFORMANCE.md, October 26). The labels hold only
    // another window's updates: a game's own compositions, its duplicates
    // included, are claimed exactly as without them.
    for seed in 0..4 {
        let (compositions, _) = mixed_desktop(25., 1e9, seed * 10);
        let detect = |i: usize| 0.08 + 0.12 * noise(i, seed + 99);
        let labelled = simulate_focus(
            1000. / 60.,
            &focus_labels(&compositions, true, false),
            detect,
            vrr_desktop,
        );
        let unlabelled = simulate_focus(
            1000. / 60.,
            &focus_labels(&compositions, false, false),
            detect,
            vrr_desktop,
        );
        assert_eq!(labelled, unlabelled, "seed {seed}");
    }
}
/// A window that is not the foreground window animating at 60 Hz (a video,
/// or the check's motion strip) behind a foreground window that repaints in
/// bursts of `burst` repaints `gap` ms apart about every `every` ms (a
/// terminal printing, a chat app): the compositions, each with whether it
/// changed the foreground window, and the strip's frames. A repaint within
/// half a millisecond of a strip frame is composed with it; each present
/// may come with a second composition up to a millisecond later.
fn strip_behind_focus(
    seconds: f64,
    every: f64,
    burst: usize,
    gap: f64,
    seed: u64,
) -> (Vec<(f64, bool)>, Vec<f64>) {
    let strip = 1000. / 60.;
    let frames: Vec<f64> = (0..(seconds * 1000. / strip) as usize)
        .map(|frame| frame as f64 * strip + 1.3 + 0.5 * noise(frame, seed))
        .collect();
    let mut repaints = vec![];
    let mut start = 100. + every * noise(0, seed + 5);
    let mut index = 0;
    while start < seconds * 1000. {
        for k in 0..burst {
            repaints.push(start + k as f64 * gap + 0.4 * noise(index, seed + 6));
            index += 1;
        }
        start += every * (0.8 + 0.4 * noise(index, seed + 7));
    }
    let near = |list: &[f64], at: f64| {
        let next = list.partition_point(|t| *t < at);
        [next.wrapping_sub(1), next]
            .iter()
            .any(|i| list.get(*i).is_some_and(|t| (t - at).abs() < 0.5))
    };
    let mut compositions = vec![];
    for (i, &frame) in frames.iter().enumerate() {
        let focus = near(&repaints, frame);
        compositions.push((frame, focus));
        if noise(i, seed + 8) < 0.5 {
            compositions.push((frame + 0.1 + 0.9 * noise(i, seed + 9), focus));
        }
    }
    for (i, &repaint) in repaints.iter().enumerate() {
        if !near(&frames, repaint) {
            compositions.push((repaint, true));
            if noise(i, seed + 10) < 0.5 {
                compositions.push((repaint + 0.1 + 0.9 * noise(i, seed + 11), true));
            }
        }
    }
    compositions.sort_by(|a, b| a.0.total_cmp(&b.0));
    (compositions, frames)
}
/// Strip frames shown by `claims` (claim, presented): frames never shown,
/// gaps longer than `long` ms between claims showing a newer strip frame,
/// and the longest such gap.
fn strip_shown(claims: &[(f64, f64)], frames: &[f64], long: f64) -> (usize, usize, f64) {
    let shown = |at: f64| frames.partition_point(|f| *f <= at + 1e-9);
    let (mut skipped, mut longs, mut longest) = (0, 0, 0f64);
    let mut last: Option<(usize, f64)> = None;
    for &(claim, presented) in claims {
        let count = shown(presented);
        match last {
            Some((previous, at)) if count > previous => {
                skipped += count - previous - 1;
                let gap = claim - at;
                longs += usize::from(gap > long);
                longest = longest.max(gap);
                last = Some((count, claim));
            }
            None => last = Some((count, claim)),
            _ => {}
        }
    }
    (skipped, longs, longest)
}
/// A desktop's name and how the host configures the pacer for it.
type Desktop = (&'static str, fn(Pacer) -> Pacer);
fn fixed_desktop(pacer: Pacer) -> Pacer {
    pacer.with_source_phase(true).with_spacing(0.75)
}
#[test]
fn a_window_repainting_in_bursts_in_front_does_not_hold_the_picture_behind_it() {
    // Hardware (rust/PERFORMANCE.md, November 20): the check's motion strip
    // at 60 Hz behind a foreground window that repainted in bursts (agents
    // printing in a terminal app) streamed 56.5 fps with a 50 ms hold about
    // every half second. The window counted as animating from its second
    // repaint, so each strip frame waited, as another window's update of
    // its picture, for the static repeat (here strip frames up to 66 ms
    // apart, and 290 to 1450 frames left out in 76 s). A window repainting
    // in bursts shorter than 500 ms now paces the strip as it did without
    // focus labels.
    let desktops: [Desktop; 2] = [("fixed", fixed_desktop), ("vrr", vrr_desktop)];
    for (desktop, configure) in desktops {
        for (burst, gap) in [(2, 12.), (3, 16.), (5, 16.), (8, 16.), (12, 30.)] {
            let (mut labelled, mut unlabelled) = ((0, 0), (0, 0));
            for seed in 0..4 {
                let (compositions, frames) = strip_behind_focus(20., 520., burst, gap, seed * 10);
                let shown = |focus_labels: bool| {
                    let labels: Vec<(f64, Option<bool>)> = compositions
                        .iter()
                        .map(|&(t, focus)| (t, focus_labels.then_some(focus)))
                        .collect();
                    let claims = simulate_focus(
                        1000. / 60.,
                        &labels,
                        |i| 0.08 + 0.12 * noise(i, seed + 99),
                        configure,
                    );
                    let steady: Vec<(f64, f64)> =
                        claims.into_iter().filter(|c| c.0 > 1000.).collect();
                    strip_shown(&steady, &frames, 25.)
                };
                let (with, without) = (shown(true), shown(false));
                assert!(
                    with.2 < 40.,
                    "{desktop} burst {burst}x{gap} ms seed {seed}: a strip frame {:.1} ms after the one before",
                    with.2
                );
                labelled = (labelled.0 + with.0, labelled.1 + with.1);
                unlabelled = (unlabelled.0 + without.0, unlabelled.1 + without.1);
            }
            // Frames left out and gaps over 25 ms, over the four seeds: a
            // focus repaint is a picture of its own either way (a labelled
            // one also right after a strip frame), and at the stream rate it
            // takes a strip frame's claim now and then.
            let close = |with: usize, without: usize| with <= without + without / 10 + 4;
            assert!(
                close(labelled.0, unlabelled.0) && close(labelled.1, unlabelled.1),
                "{desktop} burst {burst}x{gap} ms: {labelled:?} against {unlabelled:?} without labels"
            );
        }
    }
}
#[test]
fn a_fresh_capture_can_replace_the_frame_before_the_prediction_fallback() {
    for fps in [59.94, 90., 120., 144., 240.] {
        let start = Instant::now();
        let period = Duration::from_secs_f64(1. / fps);
        let interval = period / 2;
        let presented = start + interval;
        let detected = presented + Duration::from_micros(200);
        let mut pacer = Pacer::new(start, period);
        pacer.claimed(start);
        let Pace::WaitUntil(deadline) = pacer.decide(detected, presented, Some(interval)) else {
            panic!("the minimum spacing still applies");
        };
        let fresh = presented + interval;
        let arrived = fresh + Duration::from_micros(200);
        assert!(arrived < deadline);
        assert_eq!(pacer.decide(arrived, fresh, Some(interval)), Pace::Claim);
        pacer.claimed(arrived);
        assert!(pacer.allowed_at(arrived) >= arrived + period.mul_f64(0.75));
    }
}
#[test]
fn explicitly_enabling_prediction_preserves_default_decisions() {
    let start = Instant::now();
    let period = Duration::from_millis(8);
    let mut default = Pacer::new(start, period);
    let mut explicit = Pacer::new(start, period).with_prediction(true);
    for tick in 0..2000 {
        let at = start + Duration::from_micros(tick * 700);
        let presented = at - Duration::from_micros((tick % 7) * 100);
        let interval = Some(Duration::from_micros(3500 + (tick % 3) * 500));
        let decision = default.decide(at, presented, interval);
        assert_eq!(decision, explicit.decide(at, presented, interval));
        if decision == Pace::Claim {
            default.claimed(at);
            explicit.claimed(at);
        }
    }
}
#[test]
fn repeat_grace_preserves_credit_for_the_imminent_fresh_frame() {
    let start = Instant::now();
    let period = Duration::from_millis(16);
    let first_claim = start + Duration::from_micros(400);
    let mut guarded = Pacer::new(start, period).with_source_phase(true);
    let mut repeated = Pacer::new(start, period);
    guarded.claimed(first_claim);
    repeated.claimed(first_claim);
    let due = first_claim + period;
    let fresh_presented = start + period;
    let fresh_arrived = fresh_presented + Duration::from_micros(600);
    let deadline = guarded.repeat_deadline(due, start, Some(period));
    assert!(due < fresh_arrived && fresh_arrived < deadline);
    assert_eq!(guarded.last_claim, Some(first_claim));
    assert_eq!(guarded.credit_at, first_claim);
    assert_eq!(guarded.credit, repeated.credit);
    // Without grace, the unchanged frame wins just before capture arrives.
    repeated.claimed(due);
    assert!(matches!(
        repeated.decide(fresh_arrived, fresh_presented, Some(period)),
        Pace::WaitUntil(_)
    ));
    assert_eq!(
        guarded.decide(fresh_arrived, fresh_presented, Some(period)),
        Pace::Claim
    );
}
#[test]
fn repeat_grace_is_fixed_and_does_not_starve_a_static_desktop() {
    let start = Instant::now();
    let period = Duration::from_millis(16);
    let pacer = Pacer::new(start, period).with_source_phase(true);
    let due = start + period;
    let deadline = pacer.repeat_deadline(due, start, Some(period));
    assert_eq!(deadline, due + period / 4);
    for _ in 0..100 {
        assert_eq!(pacer.repeat_deadline(due, start, Some(period)), deadline);
    }
    // No fresh capture arrived: later static repeats cannot keep predicting
    // a new frame relative to the polling time or the last repeated encode.
    for repeat in 1..=20 {
        let next_due = deadline + period * repeat;
        assert_eq!(
            pacer.repeat_deadline(next_due, start, Some(period)),
            next_due
        );
    }
    // A prediction near the edge cannot extend beyond the explicit cap.
    let early_due = start + period - Duration::from_millis(3);
    assert_eq!(
        pacer.repeat_deadline(early_due, start, Some(period)),
        early_due + period / 4 + Duration::from_micros(500)
    );
    let too_early = start + period / 2;
    assert_eq!(
        pacer.repeat_deadline(too_early, start, Some(period)),
        too_early
    );
}
#[test]
fn repeat_grace_handles_fractional_rates_without_delaying_slow_sources() {
    let start = Instant::now();
    for (stream_hz, source_hz) in [(60., 59.94), (59.94, 60.), (120., 119.998)] {
        let period = Duration::from_secs_f64(1. / stream_hz);
        let interval = Duration::from_secs_f64(1. / source_hz);
        let pacer = Pacer::new(start, period).with_source_phase(true);
        let due = start + period;
        let deadline = pacer.repeat_deadline(due, start, Some(interval));
        assert!(deadline > due);
        assert!(deadline <= due + period / 4 + Duration::from_micros(500));
        for unavailable_or_slow in [None, Some(period * 2), Some(period * 4)] {
            assert_eq!(pacer.repeat_deadline(due, start, unavailable_or_slow), due);
        }
    }
}
#[test]
fn repeat_grace_protects_ddx_and_vrr_without_predicting_fresh_claims() {
    let start = Instant::now();
    let period = Duration::from_millis(16);
    let due = start + period;
    for pacer in [
        Pacer::new(start, period),
        Pacer::new(start, period).with_source_phase(false),
        Pacer::new(start, period)
            .with_source_phase(true)
            .with_prediction(false),
    ] {
        assert_eq!(
            pacer.repeat_deadline(due, start, Some(period)),
            due + period / 4
        );
    }
}
#[test]
fn repeat_grace_protects_slow_sources_only_when_their_next_frame_is_due() {
    let start = Instant::now();
    let period = Duration::from_millis(8);
    let pacer = Pacer::new(start, period);
    for multiple in [2, 3, 5, 12] {
        let interval = period * multiple;
        for repeat in 1..multiple {
            let due = start + period * repeat;
            assert_eq!(pacer.repeat_deadline(due, start, Some(interval)), due);
        }
        let due = start + interval;
        assert_eq!(
            pacer.repeat_deadline(due, start, Some(interval)),
            due + period / 4
        );
        let late = due + period;
        assert_eq!(pacer.repeat_deadline(late, start, Some(interval)), late);
    }
}
