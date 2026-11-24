//! Tests for learned capture source phase, surplus gating, retention, and multiples.

use super::*;

#[test]
fn learned_phase_anticipates_the_main_update_instead_of_an_extra_composition() {
    let start = Instant::now();
    let period = Duration::from_millis(16);
    let mut pacer = Pacer::new(start, period)
        .with_prediction(false)
        .with_source_phase(true);
    for frame in 0..40 {
        pacer.observe_source(start + period * frame);
        if frame % 4 == 0 {
            pacer.observe_source(start + period * frame + period / 2);
        }
    }
    assert!(pacer.source_phase.as_ref().unwrap().has_surplus());
    let last = start + period * 39;
    pacer.claimed(last);
    let extra = last + period / 2;
    pacer.observe_source(extra);
    let Pace::WaitUntil(deadline) = pacer.decide(extra, extra, Some(period)) else {
        panic!("an extra composition must not consume this source frame's credit");
    };
    assert_eq!(deadline, last + period + period / 4);
    assert!(deadline - pacer.allowed_at(extra) < period);
    let fresh = last + period;
    pacer.observe_source(fresh);
    assert_eq!(pacer.decide(fresh, fresh, Some(period)), Pace::Claim);
    // The feature learns nothing from repeatedly inspecting one Arc.
    let count = pacer.source_phase.as_ref().unwrap().count;
    for _ in 0..100 {
        pacer.observe_source(fresh);
    }
    assert_eq!(pacer.source_phase.as_ref().unwrap().count, count);

    pacer.claimed(fresh);
    let extra = fresh + period - Duration::from_millis(2);
    pacer.observe_source(extra);
    let Pace::WaitUntil(deadline) = pacer.decide(extra, extra, None) else {
        panic!("a funded extra composition must still wait for the imminent update");
    };
    assert_eq!(deadline, fresh + period + period / 4);
    // Surplus compositions distort the interval median, and an already funded
    // slot must still give the imminent source update a chance to arrive.
    for interval in [None, Some(period / 2), Some(period * 2)] {
        assert_eq!(pacer.allowed_at(extra), extra);
        assert_eq!(
            pacer.decide(extra, extra, interval),
            Pace::WaitUntil(deadline)
        );
        assert_eq!(pacer.decide(deadline, extra, interval), Pace::Claim);
    }
}
#[test]
fn stale_in_phase_compositions_wait_for_late_updates_with_a_fixed_fallback() {
    let start = Instant::now();
    let period = Duration::from_millis(16);
    for arrives in [false, true] {
        let mut pacer = Pacer::new(start, period)
            .with_prediction(false)
            .with_source_phase(true)
            .with_spacing(0.5);
        for frame in 0..40 {
            pacer.observe_source(start + period * frame);
            if frame % 2 == 0 {
                pacer.observe_source(start + period * frame + Duration::from_micros(1500));
            }
        }
        let last = start + period * 39;
        pacer.claimed(last - period / 2);
        pacer.claimed(last);
        let extra = last + Duration::from_micros(1500);
        pacer.observe_source(extra);
        let phase = pacer.source_phase.as_ref().unwrap();
        assert!(phase.distance(phase.offset(extra), phase.center.unwrap()) <= phase.radius_ns());
        let now = last + Duration::from_millis(14);
        assert_eq!(pacer.allowed_at(now), now);
        let Pace::WaitUntil(deadline) = pacer.decide(now, extra, None) else {
            panic!("an old in-phase composition must not bypass the next update");
        };
        let late = last + period + period / 8;
        assert_eq!(deadline, last + period + period / 4);
        for step in 0..10 {
            assert_eq!(
                pacer.decide(now + (deadline - now) * step / 10, extra, None),
                Pace::WaitUntil(deadline)
            );
        }
        if arrives {
            pacer.observe_source(late);
            assert_eq!(pacer.decide(late, late, None), Pace::Claim);
            pacer.claimed(late);
        } else {
            assert_eq!(pacer.decide(deadline, extra, None), Pace::Claim);
            pacer.claimed(deadline);
        }
        // Stopping the extra compositions or the animation must not delay
        // newly arriving frames behind the previously learned phase.
        for frame in 1..=64 {
            let at = late + period * (frame * 2);
            pacer.observe_source(at);
            assert_eq!(pacer.decide(at, at, None), Pace::Claim);
            pacer.claimed(at);
        }
        assert!(!pacer.source_phase.as_ref().unwrap().has_surplus());
        let resumed = late + period * 256;
        pacer.observe_source(resumed);
        assert_eq!(pacer.decide(resumed, resumed, None), Pace::Claim);
        assert_eq!(pacer.source_phase.as_ref().unwrap().count, 1);
    }
}
#[test]
fn an_early_new_picture_cannot_wait_past_the_phase_delivery_budget() {
    let start = Instant::now();
    for fps in [60., 120.] {
        let period = Duration::from_secs_f64(1. / fps);
        let mut pacer = Pacer::new(start, period)
            .with_prediction(false)
            .with_source_phase(true)
            .with_spacing(0.5);
        for frame in 0..64 {
            pacer.observe_source(start + period * frame);
            if frame % 2 == 0 {
                pacer.observe_source(start + period * frame + period / 2);
            }
        }
        let last = start + period * 63;
        pacer.claimed(last - period / 2);
        pacer.claimed(last);
        // The compositor can deliver a new game picture before the phase
        // learned from earlier captures, then supply no replacement at it.
        let picture = last + period.mul_f64(0.8);
        pacer.observe_source(picture);
        let seen = picture + Duration::from_micros(150);
        let deadline = last + period + period / 4;
        assert!(pacer.source_phase.as_ref().unwrap().has_surplus());
        for step in 0..10 {
            let now = seen + (deadline - seen) * step / 10;
            assert_eq!(
                pacer.decide(now, picture, Some(period / 2)),
                Pace::WaitUntil(deadline),
                "{fps} fps, poll {step}"
            );
        }
        assert_eq!(
            pacer.decide(deadline, picture, Some(period / 2)),
            Pace::Claim
        );
        assert!(deadline - last < period.mul_f64(1.5));
    }
}
#[test]
fn vrr_source_phase_escapes_startup_alias_with_jittered_extra_compositions() {
    let period = 1000. / 60.;
    let mut lowest_without_phase = 1f64;
    for offset in 0..64 {
        // The startup burst spends credit before the regular source begins.
        // Every second game frame also produces a visually identical capture.
        let mut publications = vec![(0., 0), (period / 2. + 0.1, 0)];
        for frame in 0..600 {
            let at = period * (frame + 1) as f64
                + period * f64::from(offset) / 64.
                + f64::from(frame * 7 % 11) * 0.12;
            publications.push((at, frame + 1));
            if frame % 2 == 0 {
                publications.push((at + 2. + f64::from(frame * 5 % 7) * 0.1, frame + 1));
            }
        }
        let captures: Vec<f64> = publications.iter().map(|&(at, _)| at).collect();
        for phase in [false, true] {
            let claims = simulate_with(
                period,
                &captures,
                |_| 0.,
                |pacer| {
                    pacer
                        .with_prediction(false)
                        .with_source_phase(phase)
                        .with_spacing(0.5)
                },
            );
            let steady: Vec<_> = claims.iter().filter(|(at, _)| *at >= 3000.).collect();
            let mut frames: Vec<_> = steady
                .iter()
                .map(|&&(_, presented)| {
                    let index = publications
                        .binary_search_by(|(at, _)| at.total_cmp(&presented))
                        .unwrap();
                    publications[index].1
                })
                .collect();
            frames.dedup();
            let unique = frames.len() as f64 / steady.len() as f64;
            if phase {
                assert!(
                    steady.len() >= 410,
                    "offset {offset}: {} claims",
                    steady.len()
                );
                assert!(unique >= 0.9, "offset {offset}: {unique} unique share");
            } else {
                lowest_without_phase = lowest_without_phase.min(unique);
            }
        }
    }
    // The faster-source grace repays part of the deficit on its own (0.90
    // unique without the phase, against under 0.8 before it); the alias
    // still shows.
    assert!(
        lowest_without_phase < 0.95,
        "startup alias was not exercised"
    );
}
#[test]
fn phase_learning_tracks_fractional_rate_drift_and_resets_on_jumps_and_recovery() {
    let start = Instant::now();
    for (stream_hz, source_hz) in [(60., 59.94), (59.94, 60.)] {
        let period = Duration::from_secs_f64(1. / stream_hz);
        let mut pacer = Pacer::new(start, period).with_source_phase(true);
        for frame in 0..500 {
            let at = start + Duration::from_secs_f64(f64::from(frame) / source_hz);
            pacer.observe_source(at);
            if frame > 100 {
                let phase = pacer.source_phase.as_ref().unwrap();
                assert!(phase.center.is_some());
                assert!(!phase.has_surplus());
                assert!(
                    phase.distance(phase.offset(at), phase.center.unwrap()) <= phase.radius_ns()
                );
            }
        }
        let last = pacer.source_phase.as_ref().unwrap().last.unwrap();
        for frame in 1..=3 {
            pacer.observe_source(last + period * frame + period / 3);
        }
        assert!(pacer.source_phase.as_ref().unwrap().center.is_none());
        pacer.reset_source_phase();
        assert_eq!(pacer.source_phase.as_ref().unwrap().count, 0);
        assert!(pacer.source_phase.as_ref().unwrap().last.is_none());
    }
}
#[test]
fn learned_phase_survives_increased_surplus_without_repeating_game_frames() {
    let period = 1000. / 60.;
    for offset in 0..64 {
        let mut publications = vec![(0., 0), (period / 2. + 0.1, 0)];
        for frame in 0..600 {
            let at = period * (frame + 1) as f64
                + period * f64::from(offset) / 64.
                + f64::from(frame * 7 % 11) * 0.12;
            publications.push((at, frame + 1));
            // Once the phase is learned, surplus captures rise from 30 to 60 Hz.
            if frame >= 180 || frame % 2 == 0 {
                publications.push((at + 2. + f64::from(frame * 5 % 7) * 0.1, frame + 1));
            }
        }
        let captures: Vec<f64> = publications.iter().map(|&(at, _)| at).collect();
        let claims = simulate_with(
            period,
            &captures,
            |_| 0.,
            |pacer| {
                pacer
                    .with_prediction(false)
                    .with_source_phase(true)
                    .with_spacing(0.5)
            },
        );
        let steady: Vec<_> = claims.iter().filter(|(at, _)| *at >= 4000.).collect();
        let mut frames: Vec<_> = steady
            .iter()
            .map(|&&(_, presented)| {
                let index = publications
                    .binary_search_by(|(at, _)| at.total_cmp(&presented))
                    .unwrap();
                publications[index].1
            })
            .collect();
        frames.dedup();
        let unique = frames.len() as f64 / steady.len() as f64;
        assert!(
            steady.len() >= 350,
            "offset {offset}: {} claims",
            steady.len()
        );
        assert!(unique >= 0.95, "offset {offset}: {unique} unique share");
    }
}
#[test]
fn phase_retention_counts_stream_cycles_instead_of_capture_bursts() {
    let start = Instant::now();
    let period = Duration::from_millis(16);
    let mut phase = SourcePhase::new(period);
    for frame in 0..64 {
        phase.observe(start + period * frame);
        if frame % 2 == 0 {
            phase.observe(start + period * frame + period / 2);
        }
    }
    assert!(phase.center.is_some());
    let last = start + period * 63;
    // Several off-phase captures within one stream period are not a phase jump.
    for offset in 1..=8 {
        phase.observe(last + Duration::from_millis(offset + 3));
        assert!(phase.center.is_some());
    }
    for frame in 1..=3 {
        phase.observe(last + period * frame + period / 2);
    }
    assert!(phase.center.is_none());

    // Many in-phase samples in one cycle cannot support mostly empty cycles.
    phase = SourcePhase::new(period);
    for frame in 0..8 {
        for burst in 0..8 {
            phase.observe(start + period * (frame * 3) + Duration::from_micros(burst * 100));
        }
    }
    assert!(!phase.covers_cycles(0));
}
#[test]
fn phase_override_preserves_default_decisions_without_clear_capture_surplus() {
    let start = Instant::now();
    for prediction in [false, true] {
        for (stream_hz, observed_hz) in [
            (60., 60.),
            (60., 59.94),
            (59.94, 60.),
            (120., 119.998),
            (120., 121.),
            (60., 49.),
            (60., 30.),
        ] {
            let period = Duration::from_secs_f64(1. / stream_hz);
            let source_interval = Duration::from_secs_f64(1. / observed_hz);
            let mut normal = Pacer::new(start, period).with_prediction(prediction);
            let mut learned = Pacer::new(start, period)
                .with_prediction(prediction)
                .with_source_phase(true);
            for frame in 0..256 {
                let presented = start + source_interval * frame;
                learned.observe_source(presented);
                assert!(!learned.source_phase.as_ref().unwrap().has_surplus());
                normal.claimed(presented);
                learned.claimed(presented);
                // Exercise waiting as well as immediate claims. With a stable
                // learned phase these off-phase candidates used to change the
                // next deadline despite there being no surplus to correct.
                for fraction in [0.125, 0.25, 0.5, 0.75, 1.] {
                    let at = presented + period.mul_f64(fraction);
                    for interval in [None, Some(period), Some(source_interval)] {
                        assert_eq!(
                            learned.decide(at, at, interval),
                            normal.decide(at, at, interval),
                            "stream {stream_hz}, captures {observed_hz}, frame {frame}"
                        );
                    }
                }
            }
        }
    }
}
#[test]
fn phase_surplus_gate_excludes_slow_sources_with_extra_compositions() {
    let start = Instant::now();
    let period = Duration::from_secs_f64(1. / 60.);
    let mut phase = SourcePhase::new(period);
    for frame in 0..128 {
        // A 30 Hz source with 15 extra compositions per second has a
        // dominant phase but still supplies fewer than 60 updates/second.
        phase.observe(start + period * (frame * 2));
        if frame % 2 == 0 {
            phase.observe(start + period * (frame * 2) + period / 2);
        }
        assert!(!phase.has_surplus());
    }
    assert!(phase.center.is_some());
}
#[test]
fn phase_surplus_gate_turns_off_after_extra_compositions_stop() {
    let start = Instant::now();
    let period = Duration::from_millis(16);
    let mut phase = SourcePhase::new(period);
    for frame in 0..80 {
        phase.observe(start + period * frame);
        if frame % 4 == 0 {
            phase.observe(start + period * frame + period / 2);
        }
    }
    assert!(phase.has_surplus());
    assert!(phase.center.is_some());
    for frame in 80..144 {
        phase.observe(start + period * frame);
    }
    assert!(!phase.has_surplus());
    assert!(phase.center.is_some());
}
#[test]
fn phase_prediction_falls_back_for_irregular_faster_and_slower_sources() {
    let start = Instant::now();
    let period = Duration::from_millis(16);
    let mut phase = SourcePhase::new(period);
    for frame in 0..128u32 {
        phase.observe(start + period * frame + period.mul_f64(f64::from(frame * 17 % 64) / 64.));
    }
    assert!(phase.center.is_none());
    for prediction in [false, true] {
        for interval in [period / 2, period, period * 2] {
            let mut normal = Pacer::new(start, period).with_prediction(prediction);
            let mut learned = Pacer::new(start, period)
                .with_prediction(prediction)
                .with_source_phase(true);
            for frame in 0..128 {
                let at = start + interval * frame;
                learned.observe_source(at);
                assert_eq!(
                    learned.decide(at, at, Some(interval)),
                    normal.decide(at, at, Some(interval))
                );
                normal.claimed(at);
                learned.claimed(at);
            }
        }
        let mut normal = Pacer::new(start, period).with_prediction(prediction);
        let mut learned = Pacer::new(start, period)
            .with_prediction(prediction)
            .with_source_phase(true);
        for frame in 0..128u32 {
            let at = start + period * frame / 2 + period * (frame * 17 % 64) / 128;
            learned.observe_source(at);
            assert!(learned.source_phase.as_ref().unwrap().center.is_none());
            assert_eq!(learned.decide(at, at, None), normal.decide(at, at, None));
            normal.claimed(at);
            learned.claimed(at);
        }
    }
    phase.observe(start + period * 200);
    assert_eq!(phase.count, 1);
    assert!(phase.center.is_none());
}
#[test]
fn source_phase_learns_whole_multiples_of_the_stream_rate_from_the_captures() {
    let start = Instant::now();
    let period = Duration::from_secs_f64(1. / 60.);
    let learned = |hz: f64, other: bool| {
        let mut phase = SourcePhase::new(period);
        let interval = 1. / hz;
        for update in 0..96 {
            let at = start
                + Duration::from_secs_f64(update as f64 * interval + 0.0003 * noise(update, 1));
            // As a picture interval of a 240 Hz display's refresh allows.
            phase.limit_to(4, at);
            phase.observe(at);
            // Another window's update on the 240 Hz display's other refresh.
            if other && update % 2 == 0 {
                phase.observe(at + Duration::from_secs_f64(interval / 2.));
            }
        }
        (phase.multiple, phase.center.is_some())
    };
    assert_eq!(learned(120., false), (2, true));
    assert_eq!(learned(119.98, false), (2, true));
    assert_eq!(learned(180., false), (3, true));
    assert_eq!(learned(240., false), (4, true));
    assert_eq!(learned(60., false), (1, true));
    assert_eq!(learned(120., true), (2, true));
    assert_eq!(learned(90., false).0, 1);
}
#[test]
fn source_phase_keeps_its_multiple_through_a_brief_lower_picture_interval() {
    // Late second compositions split a 120 Hz game's pictures, and the
    // picture interval's median read a whole period now and then. Taken as
    // the limit right away, it dropped the learned multiple: on a replay of
    // hardware captures 3.7 % of claims waited for credit afterwards.
    let start = Instant::now();
    let period = Duration::from_secs_f64(1. / 60.);
    let mut phase = SourcePhase::new(period);
    let at = |update: usize| {
        start + Duration::from_secs_f64(update as f64 / 120. + 0.0003 * noise(update, 1))
    };
    for update in 0..96 {
        phase.limit_to(2, at(update));
        phase.observe(at(update));
    }
    assert_eq!((phase.multiple, phase.center.is_some()), (2, true));
    for update in 96..192 {
        // One picture interval in twelve reads a whole period.
        phase.limit_to(if update % 12 == 0 { 1 } else { 2 }, at(update));
        phase.observe(at(update));
        assert_eq!(
            (phase.multiple, phase.center.is_some()),
            (2, true),
            "update {update}"
        );
    }
    // A lower limit that lasts takes over once `LIMIT_HOLD` has passed since
    // the limit last read two, not before.
    let read = at(191);
    for update in 192..240 {
        phase.limit_to(1, at(update));
        phase.observe(at(update));
        let since = at(update).duration_since(read);
        if since + Duration::from_millis(5) < SourcePhase::LIMIT_HOLD {
            assert_eq!(phase.multiple, 2, "update {update}");
        } else if since > SourcePhase::LIMIT_HOLD + Duration::from_millis(10) {
            assert_eq!(
                (phase.multiple, phase.center.is_some()),
                (1, true),
                "update {update}"
            );
        }
    }
}
#[test]
fn a_source_phase_restart_keeps_its_multiple_limit_and_hold() {
    // A 120 Hz game at 60 fps whose frames move half a cycle for good: three
    // of its updates off the phase start learning again, for the same game.
    // Its multiple, picture limit and the limit's hold stay: reset to one, a
    // single picture interval reading a whole period right after the move
    // took over at once.
    let start = Instant::now();
    let period = Duration::from_secs_f64(1. / 60.);
    let mut phase = SourcePhase::new(period);
    let at = |update: usize, moved: f64| {
        start + Duration::from_secs_f64(update as f64 / 120. + moved + 0.0003 * noise(update, 1))
    };
    for update in 0..96 {
        phase.limit_to(2, at(update, 0.));
        phase.observe(at(update, 0.));
    }
    assert_eq!((phase.multiple, phase.center.is_some()), (2, true));
    let moved = 1. / 240.;
    let mut update = 96;
    while phase.center.is_some() {
        phase.limit_to(2, at(update, moved));
        phase.observe(at(update, moved));
        update += 1;
        assert!(update < 100, "the old phase kept past three updates");
    }
    assert_eq!((phase.multiple, phase.limit), (2, 2));
    phase.limit_to(1, at(update, moved));
    assert_eq!(phase.limit, 2, "a single lower reading inside the hold");
    // Learnt again at the moved phase.
    for update in update..update + 40 {
        phase.limit_to(2, at(update, moved));
        phase.observe(at(update, moved));
    }
    let center = phase.center.expect("the moved phase is learnt again");
    assert_eq!(phase.multiple, 2);
    let next = phase.offset(at(update + 40, moved));
    assert!(phase.distance(next, center) <= phase.radius_ns());
}
