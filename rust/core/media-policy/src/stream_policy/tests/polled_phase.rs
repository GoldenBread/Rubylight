//! Tests for polled capture source phase, missed refreshes, and late frame notices.

use super::*;

/// Claims of a fixed-rate stream from `captures` (ms), each published on the
/// capture worker's 0.5 ms poll: (claim, presented, published).
fn simulate_polled(period: f64, captures: &[f64]) -> Vec<(f64, f64, f64)> {
    simulate_noticed(period, captures, |_| 0.08)
}
/// As `simulate_polled`, each capture reaching the poll `notice(index)` ms
/// after its present.
fn simulate_noticed(
    period: f64,
    captures: &[f64],
    notice: impl Fn(usize) -> f64,
) -> Vec<(f64, f64, f64)> {
    let start = Instant::now();
    let ns = |ms: f64| (ms * 1e6).round() as u64;
    let at = |ms: f64| start + Duration::from_nanos(ns(ms));
    let ms = |instant: Instant| instant.duration_since(start).as_secs_f64() * 1000.;
    let mut pacer = Pacer::new(start, Duration::from_secs_f64(period / 1000.))
        .with_source_phase(true)
        .with_spacing(0.75);
    let mut cadence = crate::capture_policy::Freshness::default();
    let mut claims = vec![];
    let mut newest: Option<(f64, f64)> = None;
    let mut deadline: Option<f64> = None;
    let mut interval: Option<Duration> = None;
    let mut published = 0f64;
    let mut evaluate = |now: f64,
                        newest: &mut Option<(f64, f64)>,
                        deadline: &mut Option<f64>,
                        pacer: &mut Pacer,
                        interval: Option<Duration>| {
        if let Some((presented, publish)) = *newest {
            match pacer.decide(at(now), at(presented), interval) {
                Pace::Claim => {
                    pacer.claimed(at(now));
                    claims.push((now, presented, publish));
                    *newest = None;
                    *deadline = None;
                }
                Pace::WaitUntil(t) => *deadline = Some(ms(t).max(now + 0.001)),
            }
        }
    };
    for (index, &presented) in captures.iter().enumerate() {
        published = published.max(((presented + notice(index)) / 0.5).ceil() * 0.5);
        while let Some(t) = deadline.filter(|t| *t < published) {
            evaluate(t, &mut newest, &mut deadline, &mut pacer, interval);
            if deadline == Some(t) {
                break;
            }
        }
        cadence.observe_focus(ns(presented), ns(published - presented), None);
        pacer.observe_extension(cadence.extends_picture());
        interval = cadence.picture_interval().map(Duration::from_nanos);
        pacer.observe_source_rate(cadence.mean_interval().map(Duration::from_nanos));
        pacer.observe_picture(
            cadence
                .picture_start()
                .map(|nanos| start + Duration::from_nanos(nanos)),
            cadence.late_picture_interval().map(Duration::from_nanos),
        );
        newest = Some((presented, published));
        pacer.observe_source(at(presented));
        evaluate(published, &mut newest, &mut deadline, &mut pacer, interval);
    }
    claims
}
/// Claims per second after the first tenth, and the mean and 95th
/// percentile age of the game frame each claim showed when claimed.
fn polled_rate_and_age(claims: &[(f64, f64, f64)], frames: &[f64]) -> (f64, f64, f64) {
    let steady = &claims[claims.len() / 10..];
    let rate = (steady.len() - 1) as f64 * 1000. / (steady.last().unwrap().0 - steady[0].0);
    let shown = |at: f64| frames.partition_point(|f| *f <= at + 1e-9);
    let mut ages: Vec<f64> = steady
        .iter()
        .map(|c| c.0 - frames[shown(c.1).max(1) - 1])
        .collect();
    ages.sort_by(f64::total_cmp);
    let mean = ages.iter().sum::<f64>() / ages.len() as f64;
    (rate, mean, ages[ages.len() * 95 / 100])
}
#[test]
fn another_window_beside_a_double_rate_game_does_not_take_fixed_rate_claims() {
    // A 120 Hz game at 60 fps has two updates per period, so its phase never
    // reached the support of one: another window's updates (10-30 a second,
    // no focus labels) were claimed at the spacing limit, the game frame 4 ms
    // later waited a whole frame, and claims alternated 12.5 and 20.8 ms
    // apart with up to 39 % of them showing the other window's update.
    let period = 1000. / 60.;
    for game in [1000. / 120.02, 1000. / 119.98] {
        for other in [1000. / 10., 1000. / 30.] {
            for seed in 0..3 {
                let (compositions, frames) = mixed_desktop_at(game, 30., 30., other, seed * 10);
                let captures: Vec<f64> = compositions.iter().map(|c| c.0).collect();
                let claims = simulate_polled(period, &captures);
                let (rate, _, age) = polled_rate_and_age(&claims, &frames);
                let steady = &claims[claims.len() / 10..];
                let extras = steady
                    .iter()
                    .filter(|c| !compositions[compositions.partition_point(|x| x.0 < c.1)].1)
                    .count();
                let uneven = steady
                    .windows(2)
                    .filter(|w| !(14.5..19.).contains(&(w[1].0 - w[0].0)))
                    .count();
                let case = format!("game {game:.3} ms, other {other:.1} ms, seed {seed}");
                assert!(rate < 60.05, "{case}: {rate:.3} claims/s");
                assert!(age < 1., "{case}: p95 age {age:.2} ms");
                assert!(
                    extras * 100 <= steady.len() * 3,
                    "{case}: {extras} of {} extras",
                    steady.len()
                );
                assert!(uneven * 100 <= steady.len(), "{case}: {uneven} uneven gaps");
            }
        }
    }
}
/// A game drawing one frame every `per` refreshes of a `hz` display, each
/// with a second composition, one frame in `late_every` a refresh late
/// (0: none): the captures and the game's frames (ms).
fn missed_refreshes(hz: f64, per: usize, late_every: usize, seed: u64) -> (Vec<f64>, Vec<f64>) {
    let refresh = 1000. / hz;
    let mut compositions = vec![];
    let mut frames = vec![];
    for frame in 0..3600 {
        let missed = late_every > 0 && (frame + seed as usize) % late_every == 3;
        let slot = (frame * per + usize::from(missed)) as f64;
        let composed = slot * refresh + 2.5 + 0.2 * noise(frame, seed);
        frames.push(composed);
        compositions.push(composed);
        // A second composition of the same frame.
        compositions.push(composed + 0.5 + 1.5 * noise(frame, seed + 1));
    }
    compositions.sort_by(f64::total_cmp);
    frames.sort_by(f64::total_cmp);
    (compositions, frames)
}
/// The share of claims after the first tenth made more than 0.5 ms after
/// their capture was published, in percent.
fn polled_held(claims: &[(f64, f64, f64)]) -> f64 {
    let steady = &claims[claims.len() / 10..];
    steady.iter().filter(|c| c.0 - c.2 > 0.5).count() as f64 * 100. / steady.len() as f64
}
#[test]
fn a_game_frame_that_missed_a_refresh_does_not_take_a_fixed_rate_claim_early() {
    // A 120 Hz game on a 240 or 360 Hz display: a frame that misses its
    // refresh is presented on the next one, 4.2 or 2.8 ms late, off the
    // game's phase. Claimed at the spacing limit, it spent credit the game's
    // next frames then waited for: 60.1 claims a second, 25-34 % of frames
    // held 0.5-2 ms after publishing, p95 age 3-10 ms. Waiting at the phase
    // bound until the late frame could be noticed matters too: cut off right
    // at it, a claim took the frame before, 12.5 ms old. And a late frame
    // that had arrived was held to the phase deadline for an update that
    // could not come before it: 11-22 % of frames held, p95 age 1.4-2.8 ms.
    let period = 1000. / 60.;
    for (hz, per) in [(240., 2), (360., 3)] {
        for late_every in [7, 13, 29] {
            for seed in 0..3 {
                let (compositions, frames) = missed_refreshes(hz, per, late_every, seed);
                let claims = simulate_polled(period, &compositions);
                let (rate, mean, p95) = polled_rate_and_age(&claims, &frames);
                let held = polled_held(&claims);
                let case =
                    format!("{hz} Hz, a missed refresh every {late_every} frames, seed {seed}");
                assert!(rate < 60.07, "{case}: {rate:.3} claims/s");
                assert!(mean < 0.5, "{case}: mean age {mean:.2} ms");
                assert!(p95 < 1., "{case}: p95 age {p95:.2} ms");
                assert!(held < 1., "{case}: {held:.1} % held");
            }
        }
    }
}
#[test]
fn a_missed_refresh_on_a_120_hz_display_costs_one_stale_claim() {
    // A 60 or 120 Hz game on a 120 Hz display: a frame that misses its
    // refresh comes half a period late, past the phase bound, so the claim
    // at the bound shows the frame before. Waiting past the bound for a
    // 60 Hz game only made that claim older (p95 age 22.2 ms against 21.2),
    // and spacing the next claim from it held the game's next frame on its
    // cadence too: two held claims a miss instead of one.
    let period = 1000. / 60.;
    for (game, per) in [(60, 2), (120, 1)] {
        for late_every in [7, 13] {
            for seed in 0..3 {
                let (compositions, frames) = missed_refreshes(120., per, late_every, seed);
                let claims = simulate_polled(period, &compositions);
                let (rate, _, p95) = polled_rate_and_age(&claims, &frames);
                let held = polled_held(&claims);
                let case = format!(
                    "{game} Hz game, a missed refresh every {late_every} frames, seed {seed}"
                );
                assert!(rate < 60.02, "{case}: {rate:.3} claims/s");
                assert!(p95 < period * 1.25 + 0.6, "{case}: p95 age {p95:.2} ms");
                let misses = 100. / late_every as f64;
                assert!(held < misses + 1., "{case}: {held:.1} % held");
            }
        }
    }
}
#[test]
fn a_double_rate_game_that_moves_a_refresh_is_followed_within_three_of_its_frames() {
    // A 120 Hz game on a 240 Hz display whose frames move one refresh for
    // good is half its cycle off the learned phase. Kept for three stream
    // periods, the old phase held the claim after the move to the phase
    // bound, showing a frame 5.4 ms old; three of the game's frames drop it.
    let period = 1000. / 60.;
    for (hz, per) in [(240., 2), (360., 3)] {
        let refresh = 1000. / hz;
        for seed in 0..3 {
            let mut compositions = vec![];
            let mut frames = vec![];
            for frame in 0..3600 {
                let slot = (frame * per + usize::from(frame >= 1800)) as f64;
                let composed = slot * refresh + 2.5 + 0.2 * noise(frame, seed);
                frames.push(composed);
                compositions.push(composed);
                compositions.push(composed + 0.5 + 1.5 * noise(frame, seed + 1));
            }
            compositions.sort_by(f64::total_cmp);
            frames.sort_by(f64::total_cmp);
            let moved = frames[1800];
            let claims = simulate_polled(period, &compositions);
            let shown = |at: f64| frames.partition_point(|f| *f <= at + 1e-9);
            for claim in claims.iter().filter(|c| c.0 >= moved && c.0 < moved + 300.) {
                let age = claim.0 - frames[shown(claim.1) - 1];
                let case = format!(
                    "{hz} Hz, seed {seed}, claim {:.2} ms after the move",
                    claim.0 - moved
                );
                assert!(age < 1., "{case}: age {age:.2} ms");
                assert!(
                    claim.0 - claim.2 <= 0.5,
                    "{case}: held {:.2} ms",
                    claim.0 - claim.2
                );
            }
        }
    }
}
#[test]
fn another_window_on_the_other_refreshes_does_not_become_a_double_rate_games_phase() {
    // A 120 Hz game on a 240 Hz display with its second compositions, and
    // another window updating 30 to 70 times a second on the refreshes in
    // between. At 50 to 70 a second they filled 80 % of the quarter-period
    // cycles together with the game's frames, the phase was learned at four
    // updates a period, and up to 14 % of claims showed the other window's
    // update between two game frames.
    let period = 1000. / 60.;
    let refresh = 1000. / 240.;
    for other_every in [8usize, 4, 3] {
        for seed in 0..3u64 {
            let mut compositions = vec![];
            let mut frames = vec![];
            for slot in 0..30 * 240usize {
                let at = slot as f64 * refresh + 2.5;
                if slot % 2 == 0 {
                    let frame = slot / 2;
                    let composed = at + 0.8 * noise(frame, seed * 10);
                    frames.push(composed);
                    compositions.push((composed, true));
                    // Its second composition, a tenth of them 2 to 3 ms later.
                    let late = noise(frame, seed * 10 + 1);
                    let second = if late > 0.9 {
                        2. + 10. * (late - 0.9)
                    } else {
                        0.8 + 0.5 * late
                    };
                    compositions.push((composed + second, true));
                } else if (slot / 2 + seed as usize * 10).is_multiple_of(other_every)
                    || noise(slot, seed * 10 + 2) * other_every as f64 > other_every as f64 - 1.
                {
                    compositions.push((at + 0.2 * noise(slot, seed * 10 + 3), false));
                }
            }
            compositions.sort_by(|a, b| a.0.total_cmp(&b.0));
            let captures: Vec<f64> = compositions.iter().map(|c| c.0).collect();
            let claims = simulate_polled(period, &captures);
            let (rate, _, age) = polled_rate_and_age(&claims, &frames);
            let steady = &claims[claims.len() / 10..];
            let extras = steady
                .iter()
                .filter(|c| !compositions[compositions.partition_point(|x| x.0 < c.1)].1)
                .count();
            let uneven = steady
                .windows(2)
                .filter(|w| !(14.5..19.).contains(&(w[1].0 - w[0].0)))
                .count();
            let case = format!("other window on every {other_every}th refresh, seed {seed}");
            assert!(rate < 60.08, "{case}: {rate:.3} claims/s");
            assert!(age < 1., "{case}: p95 age {age:.2} ms");
            assert!(
                extras * 100 <= steady.len(),
                "{case}: {extras} of {} extras",
                steady.len()
            );
            assert!(uneven * 100 <= steady.len(), "{case}: {uneven} uneven gaps");
        }
    }
}
/// A game's frames at `times` (ms), each with a second composition 0.1-1 ms
/// later: the captures and the frames.
fn with_second_compositions(times: &[f64], seed: u64) -> (Vec<f64>, Vec<f64>) {
    let mut captures: Vec<f64> = times
        .iter()
        .enumerate()
        .flat_map(|(frame, &at)| [at, at + 0.1 + 0.9 * noise(frame, seed + 1)])
        .collect();
    captures.sort_by(f64::total_cmp);
    let mut frames = times.to_vec();
    frames.sort_by(f64::total_cmp);
    (captures, frames)
}
/// The oldest game frame a claim after the first tenth showed, in ms, and how
/// many of those claims showed the same frame as the claim before.
fn polled_oldest_and_repeats(claims: &[(f64, f64, f64)], frames: &[f64]) -> (f64, usize) {
    let steady = &claims[claims.len() / 10..];
    let shown = |at: f64| frames.partition_point(|f| *f <= at + 1e-9);
    let oldest = steady
        .iter()
        .map(|c| c.0 - frames[shown(c.1).max(1) - 1])
        .fold(0., f64::max);
    let repeats = steady
        .windows(2)
        .filter(|w| shown(w[0].1) == shown(w[1].1))
        .count();
    (oldest, repeats)
}
#[test]
fn an_in_phase_frame_noticed_late_is_claimed_once_allowed() {
    // A game at two or four times the stream rate whose captures reach the
    // claim more than about two phase radii after their present: an in-phase
    // capture waited for the game's next frame, noticed as late, and so on
    // to the phase bound. A 240 Hz game at 120 fps noticed 0.9-1.5 ms late
    // took 88-105 claims a second, p95 age 4.3-5.8 ms.
    for (period, game, notice) in [
        (1000. / 60., 1000. / 120., 2.0..2.5),
        (1000. / 60., 1000. / 240., 2.0..2.5),
        (1000. / 120., 1000. / 240., 1.0..1.5),
    ] {
        for seed in 0..3 {
            let times: Vec<f64> = (0..(20_000. / game) as usize)
                .map(|frame| frame as f64 * game + 2.5 + 0.3 * noise(frame, seed))
                .collect();
            let (captures, frames) = with_second_compositions(&times, seed);
            let claims = simulate_noticed(period, &captures, |index| {
                notice.start + (notice.end - notice.start) * noise(index, seed + 7)
            });
            let (rate, _, p95) = polled_rate_and_age(&claims, &frames);
            let case = format!(
                "{:.0} fps, a {:.0} Hz game noticed {notice:?} ms late, seed {seed}",
                1000. / period,
                1000. / game
            );
            assert!(
                (rate - 1000. / period).abs() < 0.2,
                "{case}: {rate:.2} claims/s"
            );
            assert!(p95 < notice.end + 1., "{case}: p95 age {p95:.2} ms");
        }
    }
}
#[test]
fn a_jittery_double_rate_game_noticed_late_is_waited_for_as_long_as_its_notice() {
    // A 120 Hz game at 60 fps whose frames come up to 1 ms off their cadence
    // and are noticed 0.5-2.5 ms late. A wait for its next frame that ended
    // a fixed millisecond after the picture interval's prediction took the
    // frame before, 10 ms old, for one claim in twenty.
    let period = 1000. / 60.;
    let game = 1000. / 120.;
    for seed in 0..3 {
        let times: Vec<f64> = (0..2400)
            .map(|frame| frame as f64 * game + 2.5 + 2. * (noise(frame, seed + 5) - 0.5))
            .collect();
        let (captures, frames) = with_second_compositions(&times, seed);
        let claims = simulate_noticed(period, &captures, |index| {
            0.5 + 2. * noise(index, seed + 99)
        });
        let (rate, _, p95) = polled_rate_and_age(&claims, &frames);
        let case = format!("seed {seed}");
        assert!((rate - 60.).abs() < 0.1, "{case}: {rate:.2} claims/s");
        assert!(p95 < 3.3, "{case}: p95 age {p95:.2} ms");
    }
}
#[test]
fn a_jittery_double_rate_game_frame_just_early_of_its_phase_does_not_wait_for_itself() {
    // A 120 Hz game at 60 fps whose frames come up to 1 ms off their cadence,
    // beside second compositions that pull the learned center late. A frame
    // 1.2-1.8 ms early, just outside the radius, waited for the update it
    // was to the phase bound: 3-4 gaps of 21.8 ms a run, frames 6.6 ms old.
    let period = 1000. / 60.;
    let game = 1000. / 120.;
    for seed in 0..2 {
        let times: Vec<f64> = (0..3600)
            .map(|frame| frame as f64 * game + 2.5 + 2. * (noise(frame, seed + 5) - 0.5))
            .collect();
        let (captures, frames) = with_second_compositions(&times, seed);
        let claims = simulate_polled(period, &captures);
        let steady = &claims[claims.len() / 10..];
        let shown = |at: f64| frames.partition_point(|f| *f <= at + 1e-9);
        let age = steady
            .iter()
            .map(|c| c.0 - frames[shown(c.1).max(1) - 1])
            .fold(0., f64::max);
        let gap = steady
            .windows(2)
            .map(|w| w[1].0 - w[0].0)
            .fold(0., f64::max);
        let case = format!("seed {seed}");
        assert!(age < 3., "{case}: max age {age:.2} ms");
        // Jitter alone spaces claims up to 19 ms; the wait made 21-22 ms.
        assert!(gap < 20., "{case}: longest gap {gap:.2} ms");
    }
}
#[test]
fn a_double_rate_game_that_stalls_or_halves_is_not_held_for_frames_that_do_not_come() {
    // A 120 Hz game at 60 fps whose frames stop for a 30 ms hitch or a
    // 200 ms pause, or fall to 60 Hz every other 2 s. Held to the phase
    // bound for the frame the phase predicted, the claim before a hitch or
    // pause showed a frame 14 ms old, against 9.6 ms where the picture
    // interval's wait ends; at 60 Hz the phase still waited for the game's
    // other frame of the period, and claims showed frames up to 16.9 ms old,
    // some twice.
    let period = 1000. / 60.;
    let game = 1000. / 120.;
    for seed in 0..3 {
        let hitches: Vec<f64> = (0..3600)
            .map(|frame| frame as f64 * game + 2.5 + (frame / 60) as f64 * (30. - game))
            .collect();
        let pauses: Vec<f64> = (0..3000)
            .map(|frame| frame as f64 * game + 2.5 + (frame / 240) as f64 * 200.)
            .collect();
        let mut halves = vec![];
        let mut at = 2.5;
        while at < 30_000. {
            halves.push(at);
            at += if ((at / 2000.) as usize).is_multiple_of(2) {
                game
            } else {
                2. * game
            };
        }
        for (name, times, oldest_limit) in [
            ("a 30 ms hitch every 60 frames", hitches, 10.2),
            ("a 200 ms pause every 240 frames", pauses, 10.2),
            ("120 and 60 Hz in turns of 2 s", halves, 1.5),
        ] {
            let times: Vec<f64> = times
                .iter()
                .enumerate()
                .map(|(frame, at)| at + 0.3 * noise(frame, seed))
                .collect();
            let (captures, frames) = with_second_compositions(&times, seed);
            let claims = simulate_polled(period, &captures);
            let (oldest, repeats) = polled_oldest_and_repeats(&claims, &frames);
            let case = format!("{name}, seed {seed}");
            assert!(oldest < oldest_limit, "{case}: a claim {oldest:.2} ms old");
            assert_eq!(repeats, 0, "{case}: claims showing a frame twice");
        }
    }
}
