//! Simulated-link tests for capped links, knee holds, recovery, and probe request storms.

use super::*;

#[test]
fn a_capped_link_holds_its_learned_ceiling_without_a_sawtooth() {
    // 900 Mbps of PyroWave (about 650 on the wire) on a 500 Mbps path: the
    // stream cuts to what the path carries, learns that rate and stays there
    // between its retries instead of climbing back on every probe.
    let mut sim = Sim::pyrowave(q(Link::constant(500.), 50.), 900_000, 20.);
    sim.spread = 0.4;
    sim.run(300.);
    let collapses = sim
        .changes
        .iter()
        .filter(|(_, c)| c.kind == ChangeKind::StepDown && c.to_kbps * 100 < c.from_kbps * 90)
        .count();
    assert!(collapses <= 2, "{collapses} collapses: {:?}", sim.changes);
    assert!(
        sim.frame_loss(30., 300.) < 0.02,
        "loss {}",
        sim.frame_loss(30., 300.)
    );
    let wire = sim.mean_wire_bps(240., 300.) / 1e6;
    assert!((325. ..=500.).contains(&wire), "{wire:.0} Mbit/s");
    let r = sim.report(30., 300.);
    assert!(r.over_link <= 0.1 && r.p95_queue_ms < 40., "{r:?}");
    assert!(sim.lr.learned_bps() > 0);
}

#[test]
fn a_path_that_improves_is_found_again_by_the_retries() {
    // 200 Mbps until 60 s, then 350: the learned ceiling is not permanent.
    let link = Link::new(|t| if t < 60. { 200e6 } else { 350e6 });
    let mut sim = Sim::pyrowave_no_room(q(link, 50.), 350_000, 20.);
    sim.spread = 0.4;
    sim.run(180.);
    let back = sim.reaches(60., 200_000).unwrap_or(f64::MAX);
    assert!(back <= 150., "200 Mbps of the stream at {back:.1} s");
    assert!(
        sim.frame_loss(150., 180.) < 0.02,
        "loss {}",
        sim.frame_loss(150., 180.)
    );
}

#[test]
fn a_link_that_never_congests_never_learns_a_ceiling() {
    // A 1 GbE path and a PyroWave stream that fits: no change, no ceiling,
    // exactly as before.
    let mut sim = Sim::pyrowave(q(Link::constant(1000.), 50.), 800_000, 20.);
    sim.spread = 0.4;
    sim.run(60.);
    assert!(sim.changes.is_empty(), "{:?}", sim.changes.first());
    assert_eq!(sim.lr.learned_bps(), 0);
    assert!(sim.kbps_log.iter().all(|(_, k)| *k == 800_000));
}

#[test]
fn a_busy_pc_stops_probes_without_learning_a_network_ceiling() {
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    for _ in 0..3 {
        rig.run(PERIOD_MS, |lr, frame, now| lr.on_send_loss(frame, now));
        rig.clean(PERIOD_MS);
    }
    assert!(rig.cuts() >= 1 || rig.kinds().contains(&ChangeKind::Floor));
    assert!(rig.lr.host_limited(rig.now()));
    assert_eq!(rig.lr.snapshot(rig.now())["learned_kbps"].as_u64(), Some(0));
    let before = rig.changes.len();
    rig.clean(4_000);
    assert_eq!(rig.changes.len(), before, "probed while the host was busy");
    rig.clean(8_000);
    assert!(
        rig.changes[before..]
            .iter()
            .any(|(_, c)| c.kind == ChangeKind::Probe),
        "no probe after the host recovered: {:?}",
        &rig.changes[before..]
    );
}

#[test]
fn a_healthy_link_never_changes_anything() {
    type Make = fn() -> Link;
    let links: [(&str, Make, u32); 11] = [
        ("1 GbE 20", || Link::constant(1000.), 20_000),
        ("1 GbE 40", || Link::constant(1000.), 40_000),
        ("1 GbE 80", || Link::constant(1000.), 80_000),
        ("150 Mbit/s 80", || Link::constant(150.), 80_000),
        ("300 Mbit/s 40", || Link::constant(300.), 40_000),
        // Wi-Fi: 0.5 % and 1 % random loss, repaired by FEC.
        ("wifi 0.5 %", || lossy(Link::constant(1000.), 0.005), 40_000),
        ("wifi-hi 1 %", || lossy(Link::constant(1000.), 0.01), 80_000),
        // Burst loss: a Gilbert channel, mild and hard, repaired by FEC.
        ("gilbert mild", || gilbert(1000., 0.002, 0.2, 0.6), 40_000),
        ("gilbert hard", || gilbert(1000., 0.004, 0.15, 0.7), 80_000),
        ("gilbert 150", || gilbert(150., 0.002, 0.2, 0.6), 80_000),
        // A dip the stream never feels.
        (
            "dip at 20 Mbit/s",
            || Link::step(300., 30., 20., 40.),
            20_000,
        ),
    ];
    for (name, make, kbps) in links {
        for feedback in [2., 20., 80.] {
            let mut sim = Sim::new(make(), kbps, feedback);
            sim.run(75.);
            assert!(
                sim.changes.is_empty(),
                "{name} fb {feedback}: {:?}",
                sim.changes.first()
            );
            // A burst can take a frame whole; two in a thousand is the limit.
            let allowed = if name.starts_with("gilbert") {
                0.002
            } else {
                0.
            };
            assert!(sim.frame_loss(0., 75.) <= allowed, "{name} fb {feedback}");
            assert!(sim.kbps_log.iter().all(|(_, k)| *k == kbps), "{name}");
        }
    }
}

/// The 2026-10-24 `r50-hevc-hi` arm (50 Mbit/s, 50 ms queue, 59 Mbps asked): a
/// probe that failed at the link's edge was followed by climbs back to the
/// same edge, a freeze at each, 6 to 9 of them in a run.
#[test]
fn probes_stop_under_the_rate_that_failed_for_the_quiet_hold() {
    for (feedback, spread) in [(20., 0.05), (20., 1.), (80., 0.05), (80., 1.)] {
        let mut sim = Sim::new(q(Link::constant(50.), 50.), 60_000, feedback);
        sim.spread = spread;
        sim.run(60.);
        let what = format!("fb {feedback}, sp {spread}");
        let (at, failed) = sim
            .changes
            .iter()
            .find(|(_, c)| c.kind == ChangeKind::ProbeFailed)
            .map(|(t, c)| (*t, c.from_kbps))
            .unwrap_or_else(|| panic!("{what}: no probe failed: {:?}", sim.changes));
        // For the quiet hold nothing goes back to the rate that failed.
        let top = sim
            .kbps_log
            .iter()
            .filter(|(t, _)| *t > at + 1. && *t < at + 19.)
            .map(|(_, k)| *k)
            .max()
            .unwrap();
        assert!(top < failed, "{what}: {top} kbps again after {failed}");
        let failures: Vec<_> = sim
            .changes
            .iter()
            .filter(|(t, c)| {
                *t > 2. && matches!(c.kind, ChangeKind::ProbeFailed | ChangeKind::StepDown)
            })
            .map(|(t, c)| (*t, c.kind, c.reason))
            .collect();
        // A probe past the knee that builds a lossless queue is cut on its
        // RTT (twice at most): a failure without the lost frames of one.
        let loud = failures.iter().filter(|(_, _, r)| *r != "queue").count();
        assert!(loud <= 3, "{what}: {failures:?}");
        assert!(failures.len() <= 5, "{what}: {failures:?}");
        assert!(
            sim.frame_loss(3., 60.) < 0.02,
            "{what}: {}",
            sim.frame_loss(3., 60.)
        );
    }
}

#[test]
fn a_link_that_improves_clears_the_knee_and_the_rate_climbs_back() {
    // 50 Mbit/s for 15 s, then 150: the probe that failed at 50 keeps the
    // stream under it for the quiet hold, and one small step past it, then the
    // usual climb, finds the room. Without the knee cleared on the step that
    // passes it, the near-knee steps (a twentieth every two seconds) go on to
    // the ceiling and the stream gets there 15 s later.
    for feedback in [20., 80.] {
        let link = Link::new(|t| if t < 15. { 50e6 } else { 150e6 });
        let mut sim = Sim::new(q(link, 50.), 60_000, feedback);
        sim.run(75.);
        let what = format!("fb {feedback}");
        let back = sim.reaches(15., 54_000).unwrap_or(f64::MAX);
        assert!(back <= 40., "{what}: 90 % at {back:.1} s");
        assert!(
            sim.frame_loss(40., 75.) < 0.005,
            "{what}: {}",
            sim.frame_loss(40., 75.)
        );
    }
}

/// The 2026-10-24 `r50-hevc-hi@4` run: a probe to 41.6 Mbps on a 50 Mbit/s link
/// started a storm of recovery keyframes (38 in 3.5 s, a freeze of 3.7 s). The
/// client sent FEC status, so its keyframe requests were not evidence (E4), and
/// a keyframe lost whole is not reported as an unrecoverable frame: the probe
/// was judged failed only when ordinary frames were reported 4 s in.
#[test]
fn a_storm_of_keyframe_requests_fails_the_probe_that_is_pending() {
    let mut rig = Rig::new(59_000);
    rig.clean(2_000);
    rig.lose_three();
    rig.clean(500);
    rig.until_probe();
    rig.until_probe();
    let start = rig.ms;
    let before = rig.changes.len();
    let mut n = 0u32;
    // A request every 7 frames (115 ms), as in the log, and no FEC report.
    rig.run(1_500, |lr, _, now| {
        n += 1;
        if n.is_multiple_of(7) {
            lr.on_keyframe_request(true, now);
        }
    });
    let (at, change) = rig.changes[before..]
        .iter()
        .find(|(_, c)| c.kind == ChangeKind::ProbeFailed)
        .unwrap_or_else(|| panic!("no failed probe: {:?}", rig.kinds()));
    assert!(*at - start < 700, "failed after {} ms", at - start);
    assert_eq!(change.reason, "keyframe_requests");
}

/// Review 2026-10-24: a probe step sent just before the picture went static
/// stays pending (its clock is held), and a client reporting FEC status then
/// had its requests counted against the step although nothing was being sent.
#[test]
fn keyframe_requests_while_the_picture_is_static_do_not_fail_a_pending_probe() {
    for busy in [true, false] {
        let mut rig = Rig::new(59_000);
        rig.clean(2_000);
        rig.run(10, |lr, frame, now| repaired(lr, frame, 2, now));
        rig.lose_three();
        rig.clean(500);
        rig.until_probe();
        assert!(rig.lr.pending.iter().any(Option::is_some));
        rig.busy = busy;
        let before = rig.changes.len();
        let mut n = 0u32;
        rig.run(1_500, |lr, _, now| {
            n += 1;
            if n.is_multiple_of(7) {
                lr.on_keyframe_request(true, now);
            }
        });
        let failed = rig.changes[before..]
            .iter()
            .any(|(_, c)| c.kind == ChangeKind::ProbeFailed);
        assert_eq!(failed, busy, "busy {busy}: {:?}", rig.kinds());
    }
    // A step confirmed a moment ago is the same.
    let mut rig = Rig::new(59_000);
    rig.clean(2_000);
    rig.lr.pending = [None; PENDING_CAP];
    rig.lr.confirmed = Some(Confirmed {
        before: rig.lr.good,
        rate: rig.lr.good,
        at: rig.now(),
        window: EVIDENCE_WINDOW,
        retry: false,
    });
    assert!(rig.lr.probe_unconfirmed(rig.now()));
    rig.lr.static_picture = true;
    assert!(!rig.lr.probe_unconfirmed(rig.now()));
}

/// Review 2026-10-29: repeated invalidations for one range count once, but
/// not while a probe step is unconfirmed, where three requests fail it as
/// three keyframe requests do.
#[test]
fn repeated_invalidations_fail_an_unconfirmed_probe() {
    let mut rig = Rig::new(59_000);
    rig.clean(2_000);
    rig.run(10, |lr, frame, now| repaired(lr, frame, 2, now));
    rig.lose_three();
    rig.clean(500);
    rig.until_probe();
    let before = rig.changes.len();
    let mut n = 0u32;
    rig.run(1_500, |lr, _, now| {
        n += 1;
        if n == 10 || n == 17 || n == 24 {
            lr.on_invalidation(5_000, 5_000 + u64::from(n), true, now);
        }
    });
    assert!(
        rig.changes[before..]
            .iter()
            .any(|(_, c)| c.kind == ChangeKind::ProbeFailed),
        "{:?}",
        rig.kinds()
    );
}

/// Review 2026-10-29: under a latch watch two requests 100 ms or more apart
/// fail the latch, and repeated invalidations of one range are two requests
/// there, not one incident.
#[test]
fn repeated_invalidations_under_a_latch_watch_fail_the_latch() {
    let mut rig = Rig::new(58_988);
    rig.run(300, |lr, frame, now| repaired(lr, frame, 1, now));
    rig.lr.on_latch(rig.now(), true, true);
    rig.clean(600);
    // A keyframe got through since the latch: one request alone is no cut.
    let (frame, now) = (rig.frame, rig.now());
    rig.lr.on_keyframe_sent(frame, now);
    rig.clean(1_000);
    let first = u64::from(rig.frame);
    rig.lr.on_invalidation(first, first + 2, true, rig.now());
    rig.clean(200);
    assert!(rig.changes.is_empty(), "{:?}", rig.changes);
    rig.lr.on_invalidation(first, first + 14, true, rig.now());
    rig.clean(300);
    assert_eq!(rig.cuts(), 1, "{:?}", rig.changes);
    assert_eq!(rig.changes[0].1.reason, "keyframe_requests");
}

#[test]
fn keyframe_requests_without_a_pending_probe_or_with_fewer_than_three_are_not_a_cut() {
    // A healthy link: a client that reports FEC status asks for a few keyframes.
    let mut rig = Rig::new(59_000);
    rig.clean(2_000);
    rig.run(10, |lr, frame, now| repaired(lr, frame, 2, now));
    let mut n = 0u32;
    rig.run(10_000, |lr, _, now| {
        n += 1;
        if n.is_multiple_of(7) {
            lr.on_keyframe_request(true, now);
        }
    });
    assert!(rig.changes.is_empty(), "{:?}", rig.kinds());
    // A probe pending, and only two requests inside the window.
    let mut rig = Rig::new(59_000);
    rig.clean(2_000);
    rig.lose_three();
    rig.clean(500);
    rig.until_probe();
    let before = rig.changes.len();
    let mut n = 0u32;
    rig.run(1_500, |lr, _, now| {
        n += 1;
        if n == 10 || n == 17 {
            lr.on_keyframe_request(true, now);
        }
    });
    assert!(
        rig.changes[before..]
            .iter()
            .all(|(_, c)| c.kind != ChangeKind::ProbeFailed && c.kind != ChangeKind::StepDown),
        "{:?}",
        rig.kinds()
    );
}
