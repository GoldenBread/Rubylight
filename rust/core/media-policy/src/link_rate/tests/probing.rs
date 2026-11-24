//! Unit tests for probe step confirmation, knee backoff, frame wrap, and rebuild.

use super::*;

// ---- probing, confirmation and the knee ----
#[test]
fn a_probe_that_fails_after_a_chain_of_confirmed_steps_goes_back_one_step() {
    // 40 Mbit/s cut once, then four steps confirmed in a row, then the RTT
    // rises and three frames are lost: back to the step before the last one,
    // not to where the climb began.
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    rig.lose_three();
    rig.clean(200);
    let cut = rig.last(ChangeKind::StepDown).unwrap().to_kbps;
    let cut_total = rig.lr.good;
    while rig.probes() < 4 {
        rig.until_probe();
    }
    rig.rtt = Duration::from_millis(300);
    // Every outstanding step is confirmed; no new one is issued.
    for _ in 0..400 {
        rig.clean(10);
        if rig.lr.pending.iter().all(Option::is_none) {
            break;
        }
    }
    assert!(rig.lr.pending.iter().all(Option::is_none));
    let c = rig.lr.confirmed.expect("a recent confirmation");
    assert!(c.before > cut_total, "a chain, not one step");
    let expect = rig.lr.kbps_for(c.before, 20, PERIOD);
    assert!(expect > cut, "{expect} against the first cut {cut}");
    rig.lose_three();
    rig.clean(200);
    let failed = rig.last(ChangeKind::ProbeFailed).expect("a failed probe");
    assert_eq!(failed.to_kbps, expect);
    assert_eq!(rig.kbps, expect);
    assert_eq!(rig.cuts(), 1);
}

#[test]
fn a_step_sent_just_before_the_picture_goes_static_is_not_confirmed_by_the_stillness() {
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    rig.lose_three();
    let probe = rig.until_probe();
    let good = rig.lr.good;
    rig.busy = false;
    rig.clean(6_000);
    assert_eq!(rig.lr.good, good, "a static picture confirmed a step");
    assert!(rig.lr.bad.is_some());
    // Motion returns and the link cannot carry the step.
    rig.busy = true;
    rig.lose_three();
    rig.clean(200);
    let failed = rig.last(ChangeKind::ProbeFailed).expect("probe failed");
    assert_eq!(failed.to_kbps, probe.from_kbps);
    assert_eq!(rig.cuts(), 1, "not another cut");
}

#[test]
fn a_quiet_stream_never_probes() {
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    rig.lose_three();
    rig.busy = false;
    rig.clean(40_000);
    assert_eq!(rig.probes(), 0, "{:?}", rig.kinds());
    assert_eq!(rig.lr.state(), State::Limited);
    rig.busy = true;
    rig.clean(5_000);
    assert!(rig.probes() > 0);
}

#[test]
fn less_fec_never_raises_the_bitrate_without_a_probe() {
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    rig.lose_three();
    rig.clean(200);
    let now = rig.now();
    let rtt = rig.rtt;
    let kbps = rig.kbps;
    let mut at = |fec| rig.lr.tick(now, rtt, fec, PERIOD, kbps, true).unwrap();
    let with_20 = at(20);
    let with_10 = at(10);
    assert!(with_10 <= with_20, "{with_10} after {with_20}");
    assert!(at(0) <= with_20);
    // More FEC leaves less for the video, as before.
    assert!(at(30) < with_20);
}

#[test]
fn bad_is_forgotten_after_a_minute_at_it() {
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    rig.lose_three();
    rig.clean(500);
    // Sit at the bad rate: no probes, so only time passes.
    rig.lr.bad = Some(rig.lr.current);
    rig.lr.next_probe = Some(rig.now() + Duration::from_secs(1_000));
    rig.clean(59_000);
    assert!(rig.lr.bad.is_some(), "forgotten too early");
    rig.clean(2_000);
    assert!(rig.lr.bad.is_none(), "still remembered after a minute");
}

#[test]
fn consecutive_failed_probes_double_the_wait_up_to_the_maximum() {
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    rig.lose_three();
    let mut holds = Vec::new();
    for _ in 0..5 {
        rig.until_probe();
        rig.lose_three();
        rig.clean(200);
        holds.push(rig.lr.hold);
    }
    let secs = Duration::from_secs;
    assert_eq!(holds, [secs(4), secs(8), secs(8), secs(8), secs(8)]);
    assert_eq!(rig.lr.hold, MAX_HOLD);
}

#[test]
fn failures_at_the_same_rate_double_the_knee_wait_up_to_its_maximum() {
    // The link carries 27 Mbit/s of the 40 requested: every step over it fails.
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    rig.lose_three();
    let limit = 27_000;
    let mut knees = Vec::new();
    while rig.ms < 600_000 {
        rig.clean(100);
        if rig.lr.pending.iter().flatten().any(|_| rig.kbps > limit) {
            rig.lose_three();
            rig.clean(200);
            knees.push(rig.lr.knee);
        }
    }
    assert!(knees.len() >= 6, "{knees:?}");
    assert!(knees.windows(2).all(|w| w[1] >= w[0]), "{knees:?}");
    assert_eq!(knees.last(), Some(&KNEE_MAX_HOLD), "{knees:?}");
    assert!(knees.iter().all(|k| *k <= KNEE_MAX_HOLD));
}

#[test]
fn a_cut_from_within_ten_percent_of_the_bad_rate_counts_as_a_knee_failure() {
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    rig.lose_three();
    rig.clean(500);
    // The bad rate sits 5 % above where the stream is.
    rig.lr.bad = Some(rig.lr.current * 105 / 100);
    rig.lr.hold = HOLD;
    rig.lr.next_probe = Some(rig.now() + Duration::from_secs(1_000));
    rig.clean(1_000);
    rig.lose_three();
    rig.clean(200);
    assert_eq!(rig.cuts(), 2);
    assert_eq!(rig.lr.knee, HOLD * 2);
    assert!(rig.lr.knee_until.is_some());
    assert_eq!(rig.lr.hold, HOLD * 2, "the wait is not reset to the first");
    // A cut from far above the bad rate is a new situation.
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    rig.lose_three();
    rig.clean(500);
    rig.lr.bad = Some(rig.lr.current * 2);
    rig.lr.next_probe = Some(rig.now() + Duration::from_secs(1_000));
    rig.clean(1_000);
    rig.lose_three();
    rig.clean(200);
    assert_eq!(rig.lr.knee, HOLD);
    assert!(rig.lr.knee_until.is_none());
    assert_eq!(rig.lr.hold, HOLD);
}

#[test]
fn probes_stay_under_a_known_knee_until_its_wait_is_over() {
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    rig.lose_three();
    rig.clean(500);
    let before = rig.changes.len();
    let bad = rig.lr.current * 100 / 80;
    rig.lr.bad = Some(bad);
    rig.lr.knee_until = Some(rig.now() + Duration::from_secs(20));
    rig.lr.next_probe = Some(rig.now());
    let cap = rig.lr.kbps_for(bad * KNEE_CAP_PERCENT / 100, 20, PERIOD);
    rig.clean(15_000);
    assert!(rig.probes() > 0);
    for (_, c) in &rig.changes[before..] {
        assert!(c.to_kbps <= cap, "{c:?} over the cap {cap}");
    }
    // After the wait the probes go on past it.
    rig.clean(30_000);
    assert!(rig.changes[before..].iter().any(|(_, c)| c.to_kbps > cap));
}

#[test]
fn a_confirmation_below_the_bad_rate_keeps_the_knee_wait() {
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    rig.lose_three();
    rig.clean(500);
    rig.lr.bad = Some(rig.lr.current * 2);
    rig.lr.knee = HOLD * 4;
    rig.lr.knee_until = Some(rig.now() + Duration::from_secs(1));
    let good = rig.lr.good;
    rig.until_probe();
    rig.clean(3_000);
    assert!(rig.lr.good > good, "the step was confirmed");
    assert!(rig.lr.bad.is_some());
    assert_eq!(rig.lr.knee, HOLD * 4);
}

#[test]
fn a_wrap_of_the_frame_counter_changes_nothing() {
    let run = |start: u32| {
        let mut rig = Rig::new(40_000);
        rig.frame = start;
        rig.clean(2_000);
        rig.lose_three();
        rig.clean(6_000);
        rig.lose_three();
        rig.clean(30_000);
        rig.changes
    };
    let plain = run(0);
    assert!(plain.len() > 3);
    for start in [u32::MAX - 100, u32::MAX - 1_500, u32::MAX - 20] {
        assert_eq!(run(start), plain, "start {start}");
    }
}

#[test]
fn a_rebuilt_controller_keeps_the_client_in_charge() {
    let stream = stream(40_000);
    let mut lr = LinkRate::new(true, &stream, 0, 20);
    lr.client_takeover();
    let mut rebuilt = lr.rebuild(true, &stream, 0, 20);
    let now = Instant::now();
    assert_eq!(rebuilt.state(), State::Off);
    assert_eq!(rebuilt.snapshot(now)["reason"], "client_controlled");
    let rtt = Duration::from_millis(20);
    assert_eq!(rebuilt.tick(now, rtt, 20, PERIOD, 40_000, true), None);
    let fresh = LinkRate::new(true, &stream, 0, 20).rebuild(true, &stream, 0, 20);
    assert_eq!(fresh.state(), State::Full);
}
