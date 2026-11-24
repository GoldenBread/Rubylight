//! Tests for arrival pacing, VRR cadence, repeats, recovery, and frame prediction.

use super::*;

#[test]
fn a_stream_above_250_fps_is_noticed_within_its_quarter_period() {
    // A quarter period is under `PREDICTED_NOTICE` there: the shorter bound wins.
    let start = Instant::now();
    for (fps, notice) in [(60, 1000), (250, 1000), (300, 833), (4000, 62)] {
        let mut pacer = Pacer::new(start, Duration::from_secs(1) / fps);
        assert_eq!(pacer.notice().as_micros(), notice, "{fps} fps");
        pacer.notices[0] = Duration::from_millis(50);
        assert_eq!(pacer.notice(), Duration::from_secs(1) / fps / 4);
    }
}
#[test]
fn a_double_rate_source_is_claimed_on_arrival_at_the_stream_rate() {
    // Any detection delay: a fresh frame presented just before the claim
    // is allowed must not lose to the older frame it replaces.
    for detect in [0.1, 0.3, 0.5, 0.8, 1.0] {
        let claims = simulate(PERIOD, &source(1000. / 240., 2., |_| 0.), |_| detect);
        let steady = &claims[10..];
        for (claim, presented) in steady {
            assert!(
                claim - presented <= detect + 1e-6,
                "detected after {detect} ms, aged {}",
                claim - presented
            );
        }
        for pair in steady.windows(2) {
            assert!((pair[1].0 - pair[0].0 - PERIOD).abs() < 0.01);
        }
        assert!((claims.len() as f64 - 240.).abs() <= 2.);
    }
}
#[test]
fn a_vrr_stream_claims_on_arrival_but_never_faster_than_the_stream_rate() {
    // VRR: no predictive waits, as the host configures it.
    let vrr = |pacer: Pacer| {
        pacer
            .with_prediction(false)
            .with_source_phase(false)
            .with_spacing(0.5)
    };
    // A game uncapped on the 1000 Hz virtual display: the encoder still
    // gets the stream rate, each frame claimed within a source interval.
    let claims = simulate_with(PERIOD, &source(1., 2., |_| 0.), |_| 0.2, vrr);
    assert!(
        (claims.len() as f64 - 240.).abs() <= 3.,
        "{} claims",
        claims.len()
    );
    assert!(
        claims
            .iter()
            .all(|(claim, presented)| claim - presented <= 1.2 + 1e-6)
    );
    // A game below the stream rate is claimed the moment each frame arrives.
    let frames = source(1000. / 48., 2., |_| 0.);
    let claims = simulate_with(PERIOD, &frames, |_| 0.2, vrr);
    assert_eq!(claims.len(), frames.len());
    assert!(
        claims
            .iter()
            .all(|(claim, presented)| (claim - presented - 0.2).abs() < 1e-6)
    );
}
/// The host's VRR loop: a new frame is claimed through the pacer; an
/// unchanged picture is sent again `repeat` ms after the last encode. Every
/// encode counts toward the stream rate. Returns the encodes, the game frames
/// claimed and the longest wait of a claimed game frame, in milliseconds.
fn vrr_with_repeats(period: f64, frames: &[f64], repeat: f64) -> (usize, usize, f64) {
    let start = Instant::now();
    let at = |ms: f64| start + Duration::from_secs_f64(ms / 1000.);
    let mut pacer = Pacer::new(start, Duration::from_secs_f64(period / 1000.))
        .with_prediction(false)
        .with_source_phase(false)
        .with_spacing(0.5);
    let (mut encodes, mut claimed, mut longest) = (0, 0, 0f64);
    let (mut next, mut newest, mut encoded_at) = (0, None, 0.);
    for step in 0..=((frames.last().unwrap() + period) * 20.) as usize {
        let now = step as f64 / 20.;
        while next < frames.len() && frames[next] <= now {
            newest = Some(frames[next]);
            next += 1;
        }
        let encode = match newest {
            Some(presented) => pacer.decide(at(now), at(presented), None) == Pace::Claim,
            None => now >= encoded_at + repeat,
        };
        if encode {
            if let Some(presented) = newest.take() {
                claimed += 1;
                longest = longest.max(now - presented);
            }
            pacer.claimed(at(now));
            encodes += 1;
            encoded_at = now;
        }
    }
    (encodes, claimed, longest)
}
#[test]
fn repeats_count_toward_the_stream_rate() {
    // A 100 fps VRR stream and a game at about 90 fps whose frames come
    // unevenly, 9 to 13 ms apart.
    let gaps = [9., 13., 10.5, 12.];
    let frames: Vec<f64> = (0..180)
        .scan(0., |t, i| {
            *t += gaps[i % 4];
            Some(*t)
        })
        .collect();
    let seconds = frames.last().unwrap() / 1000.;
    // The default minimum of 20 fps: no repeat while the game runs, every
    // frame claimed the moment it arrives.
    let (encodes, claimed, longest) = vrr_with_repeats(10., &frames, 50.);
    assert_eq!((encodes, claimed), (frames.len(), frames.len()));
    assert!(longest < 0.1, "{longest} ms");
    // A minimum at the stream rate, PyroWave's old default, resends after
    // every gap over 10 ms. Uncounted, those repeats took this stream to
    // 158 fps. Counted, it stays at the stream rate; the game frames that
    // follow a repeat wait for room instead, up to a period, and a few are
    // replaced by the next before they go out.
    let (encodes, claimed, longest) = vrr_with_repeats(10., &frames, 10.);
    let fps = encodes as f64 / seconds;
    assert!(fps <= 102., "{fps} fps");
    assert!(claimed * 100 >= frames.len() * 95, "{claimed} claimed");
    assert!(longest <= 10., "{longest} ms");
}
/// The host's arrival-paced loop with recovery requests (a keyframe or a
/// reference invalidation) from a client losing packets, at `requests` ms.
/// A request served with a new picture skips the pacer, as the host does.
/// `held`: an unchanged picture waits for the next new one per
/// [`reencode_at`] and spends credit per [`counts_toward_rate`]; otherwise it
/// is encoded at once and counted, as in rc.28. Returns the encodes, each
/// game frame's wait from arrival to claim in milliseconds, and the last
/// encode's time.
fn arrival_with_recovery(
    period: f64,
    frames: &[f64],
    requests: &[f64],
    configure: impl Fn(Pacer) -> Pacer,
    held: bool,
) -> (usize, Vec<f64>, f64) {
    let start = Instant::now();
    let at = |ms: f64| start + Duration::from_secs_f64(ms / 1000.);
    let mut pacer = configure(Pacer::new(start, Duration::from_secs_f64(period / 1000.)));
    let span = Duration::from_secs_f64(period / 1000.);
    let (mut encodes, mut waits) = (0, vec![]);
    let (mut next, mut request, mut newest, mut shown) = (0, 0, None, 0.);
    let (mut recovery, mut encoded_at) = (false, 0.);
    let end = frames.last().unwrap().max(*requests.last().unwrap_or(&0.)) + period;
    for step in 0..=(end * 20.) as usize {
        let now = step as f64 / 20.;
        while next < frames.len() && frames[next] <= now {
            newest = Some(frames[next]);
            next += 1;
        }
        while request < requests.len() && requests[request] <= now {
            recovery = true;
            request += 1;
        }
        // The capture worker's median of recent intervals.
        let mut intervals: Vec<f64> = frames[next.saturating_sub(16)..next]
            .windows(2)
            .map(|w| w[1] - w[0])
            .collect();
        intervals.sort_by(f64::total_cmp);
        let interval = (intervals.len() >= 4)
            .then(|| Duration::from_secs_f64(intervals[intervals.len() / 2] / 1000.));
        if let Some(presented) = newest {
            if recovery || pacer.decide(at(now), at(presented), interval) == Pace::Claim {
                pacer.claimed(at(now));
                (encodes, encoded_at) = (encodes + 1, now);
                waits.push(now - presented);
                (newest, shown, recovery) = (None, presented, false);
            }
        } else if recovery
            && (!held || at(now) >= reencode_at(true, at(shown), span, interval, at(end + 1e3)))
        {
            if !held || counts_toward_rate(false, true) {
                pacer.claimed(at(now));
            }
            (encodes, encoded_at) = (encodes + 1, now);
            recovery = false;
        }
    }
    (encodes, waits, encoded_at)
}
#[test]
fn recovery_requests_do_not_hold_back_a_moving_picture() {
    // A client on a lossy link asks for recovery four times a second, at
    // every phase of the frame period. Sources: a game held at the stream
    // rate by an RTSS limit, frame times a little uneven; and the release
    // check's motion probe, 60 fps on a 144 Hz display (14 and 21 ms gaps)
    // streamed at 60 fps.
    let at_rate = source(PERIOD, 10., |i| [0.3, -0.2, 0.1, -0.3][i % 4]);
    let on_144: Vec<f64> = (0..600)
        .map(|i| (i * 144 / 60) as f64 * 1000. / 144.)
        .collect();
    let requests: Vec<f64> = (0..40).map(|i| 100. + i as f64 * 251.3).collect();
    let vrr = |pacer: Pacer| pacer.with_prediction(false).with_spacing(0.5);
    let standard = |pacer: Pacer| pacer.with_prediction(true);
    let percentile = |waits: &[f64], p: f64| {
        let mut sorted = waits.to_vec();
        sorted.sort_by(f64::total_cmp);
        sorted[((sorted.len() - 1) as f64 * p) as usize]
    };
    let mean = |waits: &[f64]| waits.iter().sum::<f64>() / waits.len() as f64;
    let cases = [
        (
            "120 fps game, vrr",
            PERIOD,
            &at_rate,
            &vrr as &dyn Fn(Pacer) -> Pacer,
        ),
        ("120 fps game", PERIOD, &at_rate, &standard),
        ("60 fps probe on 144 Hz", 1000. / 60., &on_144, &standard),
    ];
    for (name, period, frames, configure) in cases {
        let (before, before_waits, _) =
            arrival_with_recovery(period, frames, &requests, configure, false);
        let (after, after_waits, _) =
            arrival_with_recovery(period, frames, &requests, configure, true);
        eprintln!(
            "{name}: encodes {before} -> {after}, game frames sent {} -> {} of {}; game frame wait mean {:.2} -> {:.2} ms, p95 {:.2} -> {:.2} ms, p99 {:.2} -> {:.2} ms, max {:.2} -> {:.2} ms",
            before_waits.len(),
            after_waits.len(),
            frames.len(),
            mean(&before_waits),
            mean(&after_waits),
            percentile(&before_waits, 0.95),
            percentile(&after_waits, 0.95),
            percentile(&before_waits, 0.99),
            percentile(&after_waits, 0.99),
            percentile(&before_waits, 1.),
            percentile(&after_waits, 1.),
        );
        // A request between frames re-encoded the unchanged picture, and its
        // credit held the following game frames back: at four requests a
        // second the deficit is never repaid, so frames wait milliseconds
        // and some are replaced by the next before they go out.
        assert!(before >= before_waits.len() + 8, "{name}: {before} encodes");
        assert!(before_waits.len() < frames.len() - 5, "{name}");
        assert!(mean(&before_waits) > 1., "{name}");
        // Held: every request rides the next game frame, nothing extra is
        // sent, and no game frame waits.
        assert_eq!(after, frames.len(), "{name}");
        assert_eq!(after_waits.len(), frames.len(), "{name}");
        assert!(percentile(&after_waits, 1.) < 0.1, "{name}");
    }
    // A still screen still gets its keyframe at once.
    let still = [0.];
    let requests = [500.];
    let (encodes, _, encoded_at) = arrival_with_recovery(PERIOD, &still, &requests, vrr, true);
    assert_eq!((encodes, encoded_at), (2, 500.));
}
#[test]
fn a_source_at_the_stream_rate_is_never_skipped_despite_jitter() {
    let frames = source(PERIOD, 2., |i| if i % 3 == 0 { 0.3 } else { -0.2 });
    let claims = simulate(PERIOD, &frames, |_| 0.4);
    assert_eq!(claims.len(), frames.len());
    assert!(
        claims
            .iter()
            .all(|(claim, presented)| claim - presented <= 0.4 + 1e-6)
    );
}
#[test]
fn slower_sources_are_claimed_on_arrival() {
    for interval in [1000. / 60., 1000. / 90., 11.] {
        let frames = source(interval, 2., |_| 0.);
        let claims = simulate(PERIOD, &frames, |_| 0.5);
        assert_eq!(claims.len(), frames.len());
        assert!(
            claims
                .iter()
                .all(|(claim, presented)| claim - presented <= 0.5 + 1e-6)
        );
    }
}
#[test]
fn an_unaligned_faster_source_stays_fresh_and_within_the_stream_rate() {
    for interval in [6., 1000. / 144., 1000. / 165., 1.] {
        let claims = simulate(PERIOD, &source(interval, 4., |_| 0.), |_| 0.3);
        let rate = claims.len() as f64 / 4.;
        assert!(rate <= 121.5, "{interval} ms source claimed at {rate} fps");
        assert!(
            rate >= 100.,
            "{interval} ms source claimed at only {rate} fps"
        );
        for pair in claims.windows(2) {
            assert!(pair[1].0 - pair[0].0 >= PERIOD * 0.75 - 1e-6);
        }
        let mean_age = claims.iter().map(|(c, p)| c - p).sum::<f64>() / claims.len() as f64;
        assert!(
            mean_age <= PERIOD / 4.,
            "{interval} ms source aged {mean_age} ms"
        );
    }
}
#[test]
fn a_late_detection_still_claims_the_fresh_double_rate_frame() {
    // A polled capture sometimes notices a frame well after it was presented.
    let detect = |i: usize| if i.is_multiple_of(7) { 1.8 } else { 0.3 };
    let claims = simulate(PERIOD, &source(1000. / 240., 2., |_| 0.), detect);
    for (claim, presented) in &claims[10..] {
        assert!(
            claim - presented <= 1.8 + 1e-6,
            "aged {}",
            claim - presented
        );
    }
    assert!((claims.len() as f64 - 240.).abs() <= 2.);
}
#[test]
fn a_frame_slightly_before_a_drained_credit_slot_is_claimed_at_once() {
    // Extra desktop updates can spend the credit; a later claim then sits
    // exactly on the credit slot. Timers here wake half a millisecond late,
    // so waiting for that slot would make every following frame late too.
    let start = Instant::now();
    let period = Duration::from_secs_f64(PERIOD / 1000.);
    let mut pacer = Pacer::new(start, period);
    for claim in 0..3 {
        pacer.claimed(start + period.mul_f64(0.75) * claim);
    }
    let last = start + period.mul_f64(1.5);
    let arrived = last + period - Duration::from_micros(500);
    assert_eq!(pacer.decide(arrived, arrived, Some(period)), Pace::Claim);
    // The burst guard still holds.
    let early = last + period / 2;
    assert_ne!(pacer.decide(early, early, Some(period)), Pace::Claim);
}
#[test]
fn a_startup_credit_deficit_does_not_delay_a_steady_source_for_long() {
    // A burst at startup leaves the credit short. A source at exactly the
    // stream rate must soon be claimed on arrival again, not one timer
    // wake later for the rest of the session.
    let start = Instant::now();
    let period = Duration::from_secs_f64(PERIOD / 1000.);
    let mut pacer = Pacer::new(start, period);
    for claim in 0..6 {
        pacer.claimed(start + Duration::from_micros(10) * claim);
    }
    let mut delayed_late = 0;
    for frame in 1..=240u32 {
        let arrived = start + period * frame;
        if pacer.decide(arrived, arrived, Some(period)) == Pace::Claim {
            pacer.claimed(arrived);
        } else {
            let at = pacer.allowed_at(arrived);
            pacer.claimed(at);
            delayed_late += u32::from(frame > 120);
        }
    }
    assert_eq!(delayed_late, 0);
}
#[test]
fn a_prediction_floor_leaves_a_1000_hz_source_unchanged_and_waits_for_a_slower_one() {
    // VRR: 16 ms stream period, claims half a period apart. After a claim at
    // 0, a frame arrives at 3 ms; the next claim is allowed at 8 ms.
    let start = Instant::now();
    let period = Duration::from_millis(16);
    let floor = Duration::from_millis(2);
    let vrr = || {
        let mut pacer = Pacer::new(start, period)
            .with_spacing(0.5)
            .with_prediction_floor(floor);
        pacer.claimed(start);
        pacer
    };
    let arrived = start + Duration::from_millis(3);
    let wait =
        |pacer: &mut Pacer, interval: Duration| pacer.decide(arrived, arrived, Some(interval));
    // A 240 Hz game (4.17 ms) and a source of exactly the floor are expected
    // before the slot is over: wait for them, with the detection window.
    assert_eq!(
        wait(&mut vrr(), Duration::from_millis(4)),
        Pace::WaitUntil(start + Duration::from_millis(11))
    );
    assert_eq!(
        wait(&mut vrr(), floor),
        Pace::WaitUntil(start + Duration::from_millis(9))
    );
    // A 1000 Hz source is below the floor: the claim waits only for its slot,
    // exactly as with prediction off, however the floor is set.
    let one_khz = Duration::from_micros(1500);
    assert_eq!(
        wait(&mut vrr(), one_khz),
        Pace::WaitUntil(start + Duration::from_millis(8))
    );
    let mut off = Pacer::new(start, period)
        .with_spacing(0.5)
        .with_prediction(false);
    off.claimed(start);
    assert_eq!(wait(&mut off, one_khz), wait(&mut vrr(), one_khz));
    // Without the floor the same source would have held the claim.
    let mut unfloored = Pacer::new(start, period).with_spacing(0.5);
    unfloored.claimed(start);
    assert_eq!(
        wait(&mut unfloored, one_khz),
        Pace::WaitUntil(start + Duration::from_micros(8500))
    );
}
#[test]
fn prediction_override_keeps_the_earliest_slot_and_does_not_bypass_rate_limits() {
    let start = Instant::now();
    let period = Duration::from_millis(8);
    let mut predicted = Pacer::new(start, period);
    let mut immediate = Pacer::new(start, period).with_prediction(false);
    predicted.claimed(start);
    immediate.claimed(start);
    let arrived = start + Duration::from_millis(3);
    let source = Some(Duration::from_millis(4));
    // The newer source frame is expected at 7 ms. Normal pacing allows
    // its detection window within the 2.5 ms prediction budget; the diagnostic
    // mode only waits for the 6 ms minimum spacing.
    assert_eq!(
        predicted.decide(arrived, arrived, source),
        Pace::WaitUntil(start + Duration::from_micros(8500))
    );
    assert_eq!(
        immediate.decide(arrived, arrived, source),
        Pace::WaitUntil(start + Duration::from_millis(6))
    );
    assert_eq!(predicted.allowed_at(arrived), immediate.allowed_at(arrived));
    // Disabling anticipation must not turn a fast capture into an uncapped
    // stream. Exercise both spacing and the longer-term credit budget.
    let mut claims = Vec::new();
    for tick in 1..=20_000 {
        let at = start + Duration::from_micros(tick * 100);
        if immediate.decide(at, at, source) == Pace::Claim {
            immediate.claimed(at);
            claims.push(at);
        }
    }
    assert!(claims.len() >= 250);
    assert!(claims.len() <= 254);
    assert!(
        claims
            .windows(2)
            .all(|pair| pair[1] - pair[0] >= period.mul_f64(0.75))
    );
}
#[test]
fn a_missed_prediction_does_not_stack_detection_after_the_wait_budget() {
    for fps in [59.94, 90., 120., 144., 240.] {
        let start = Instant::now();
        let period = Duration::from_secs_f64(1. / fps);
        let interval = period / 2;
        let presented = start + interval;
        let detected = presented + Duration::from_micros(200);
        let mut pacer = Pacer::new(start, period);
        pacer.claimed(start);
        let allowed = pacer.allowed_at(detected);
        let Pace::WaitUntil(deadline) = pacer.decide(detected, presented, Some(interval)) else {
            panic!("the minimum spacing still applies");
        };
        assert!(deadline > allowed);
        // The next frame is due half a millisecond before the budget ends; it
        // gets the notice window, not another quarter period.
        assert!(
            deadline <= allowed + period / 4 + Duration::from_micros(500) + Pacer::PREDICTED_NOTICE
        );
        assert_eq!(
            pacer.decide(deadline, presented, Some(interval)),
            Pace::Claim
        );
    }
}
#[test]
fn a_frame_due_at_the_end_of_the_wait_budget_is_still_noticed() {
    // A game at twice the stream rate on a VRR stream: claims half a period
    // apart. The next game frame is due 0.2 ms before the prediction budget
    // ends and the capture notices it 0.3 ms after its present. Cut off at the
    // budget, the claim took the frame before it, half a period old.
    for fps in [59.94, 90., 120., 144.] {
        let start = Instant::now();
        let period = Duration::from_secs_f64(1. / fps);
        let interval = period / 2;
        let mut pacer = Pacer::new(start, period)
            .with_spacing(0.5)
            .with_prediction_floor(Duration::from_millis(2));
        pacer.claimed(start);
        let allowed = start + period.mul_f64(0.5);
        let budget = allowed + period / 4 + Duration::from_micros(500);
        let next = budget - Duration::from_micros(200);
        let presented = next - interval;
        let noticed = |at: Instant| at + Duration::from_micros(300);
        assert_eq!(pacer.allowed_at(noticed(presented)), allowed);
        let Pace::WaitUntil(deadline) = pacer.decide(noticed(presented), presented, Some(interval))
        else {
            panic!("the claim is not allowed yet");
        };
        assert!(
            deadline > noticed(next),
            "{fps} fps: gave up before the frame was noticed"
        );
        assert!(deadline <= budget + Pacer::PREDICTED_NOTICE);
        assert_eq!(
            pacer.decide(noticed(next), next, Some(interval)),
            Pace::Claim
        );
    }
}
#[test]
fn a_picture_later_than_the_median_is_waited_for_within_the_late_interval() {
    // The median predicts the next picture 0.2 ms before the budget ends; it
    // comes an eighth of a period later, as a game's uneven frames do.
    for fps in [59.94, 90., 120., 144.] {
        let start = Instant::now();
        let period = Duration::from_secs_f64(1. / fps);
        let interval = period / 2;
        let mut pacer = Pacer::new(start, period)
            .with_spacing(0.5)
            .with_prediction_floor(Duration::from_millis(2));
        pacer.claimed(start);
        let allowed = start + period.mul_f64(0.5);
        let budget = allowed + period / 4 + Duration::from_micros(500);
        let next = budget - Duration::from_micros(200);
        let presented = next - interval;
        let noticed = |at: Instant| at + Duration::from_micros(300);
        let arrived = noticed(next + period / 8);
        let mut deadline = |late: Option<Duration>| {
            pacer.observe_picture(Some(presented), late);
            match pacer.decide(noticed(presented), presented, Some(interval)) {
                Pace::WaitUntil(deadline) => deadline,
                Pace::Claim => panic!("the claim is not allowed yet"),
            }
        };
        assert!(deadline(None) < arrived, "{fps} fps");
        let waited = deadline(Some(interval + period / 8));
        assert!(
            arrived < waited,
            "{fps} fps: gave up before the late picture"
        );
        assert!(waited <= next + period / 8 + Pacer::PREDICTED_NOTICE);
        // A late interval far past the median extends the wait by at most a
        // quarter period.
        assert!(deadline(Some(interval * 3)) <= next + period / 4 + Pacer::PREDICTED_NOTICE);
    }
}
#[test]
fn a_game_at_twice_a_vrr_stream_rate_is_never_claimed_a_game_frame_old() {
    // 3.0.0-rc.2: a 120 Hz game on a 60 fps VRR stream, its frames a little
    // uneven and noticed 0.15 to 0.45 ms after their present. Claims that gave
    // up just before the next frame took one 7-9 ms old, and each set the next
    // budget a period later, so the miss repeated (3.5 % of claims here, 6-9 %
    // on the RX 7900 XT): the age p95 doubled.
    let vrr = |pacer: Pacer| {
        pacer
            .with_prediction_floor(Duration::from_millis(2))
            .with_source_phase(true)
            .with_spacing(0.5)
    };
    let period = 1000. / 60.;
    for seed in 0..6 {
        let frames = source(1000. / 120., 25., |i| 1. + noise(i, seed) * 0.5);
        let claims = simulate_with(period, &frames, |i| 0.15 + 0.3 * noise(i, seed + 99), vrr);
        let steady = &claims[300..];
        let oldest = steady.iter().map(|(c, p)| c - p).fold(0., f64::max);
        assert!(
            oldest < 4.,
            "seed {seed}: a claim took a frame {oldest} ms old"
        );
        let seconds = (steady.last().unwrap().0 - steady[0].0) / 1000.;
        assert!((steady.len() - 1) as f64 / seconds <= 60.2);
        assert!(
            steady
                .windows(2)
                .all(|pair| pair[1].0 - pair[0].0 >= period / 2. - 1e-6)
        );
    }
}
#[test]
fn a_second_composition_of_each_game_frame_does_not_age_vrr_claims() {
    // 3.0.0-rc.2 on the RX 7900 XT: WGC on the 240 Hz VRR display delivers
    // each frame of a 120 Hz game with a second composition 0.1 or 1 ms later,
    // in some sessions a third 3-4 ms later, all noticed about 0.1 ms after
    // their present. The capture intervals' median read 7 or 3 ms, so a claim
    // waited for a frame that was not coming, then took a second composition
    // 7-8 ms old just before the next frame: 6-9 % of claims, and the age p95
    // went from 3.7 to 7.7 ms. With the capture median, this simulation takes
    // 10 % of claims late (52-57 % with the third composition).
    let vrr = |pacer: Pacer| {
        pacer
            .with_prediction_floor(Duration::from_millis(2))
            .with_source_phase(true)
            .with_spacing(0.5)
    };
    let period = 1000. / 60.;
    for third in [false, true] {
        for seed in 0..6 {
            let mut captures = vec![];
            for frame in 0..3000 {
                let start = frame as f64 * 1000. / 120. + noise(frame, seed) * 1.2;
                captures.push(start);
                captures.push(
                    start
                        + if noise(frame, seed + 7) < 0.35 {
                            0.1
                        } else {
                            1.
                        },
                );
                if third && noise(frame, seed + 13) < 0.25 {
                    captures.push(start + 3. + noise(frame, seed + 17));
                }
            }
            let claims = simulate_captures(
                period,
                &captures,
                |i| 0.08 + 0.12 * noise(i, seed + 99),
                vrr,
            );
            let steady = &claims[300..];
            let mut ages: Vec<f64> = steady.iter().map(|(c, p)| c - p).collect();
            ages.sort_by(f64::total_cmp);
            let late = ages.iter().filter(|age| **age > 4.).count();
            let p99 = ages[ages.len() * 99 / 100];
            assert!(
                late * 200 < ages.len() && p99 < 4.,
                "third={third} seed {seed}: {late} of {} claims late, p99 {p99} ms",
                ages.len()
            );
            let seconds = (steady.last().unwrap().0 - steady[0].0) / 1000.;
            assert!((steady.len() - 1) as f64 / seconds <= 60.2);
            assert!(
                steady
                    .windows(2)
                    .all(|pair| pair[1].0 - pair[0].0 >= period / 2. - 1e-6)
            );
        }
    }
}
