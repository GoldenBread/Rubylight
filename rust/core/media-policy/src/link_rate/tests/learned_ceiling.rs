//! Tests for learned ceilings, retry backoff, host-only refusals, and PyroWave FPS floors.

use super::*;

// ---- the learned ceiling (review 2026-11-17) ----

/// A stream held at a learned ceiling after two failures at one edge.
fn at_a_learned_ceiling() -> Rig {
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    rig.lose_three();
    rig.clean(500);
    let now = rig.now();
    let lr = &mut rig.lr;
    lr.learned = Some(lr.current);
    lr.failed_at = Some(lr.current * 110 / 100);
    lr.retry_wait = RETRY_FIRST;
    lr.next_retry = Some(now + RETRY_FIRST);
    lr.bad = None;
    lr.forget_knee();
    lr.next_probe = None;
    rig
}

/// Review 2026-11-17 H1: at the learned ceiling the probe limit was the
/// current rate, and a rounding gap let every 0.5 s tick jump a full step past
/// it. With no retry due it holds; a due retry goes one twentieth up, alone.
#[test]
fn at_the_learned_ceiling_probes_hold_until_a_retry_is_due_then_go_a_twentieth() {
    let mut rig = at_a_learned_ceiling();
    let held = rig.kbps;
    let probes = rig.probes();
    rig.clean(9_000);
    assert_eq!(
        rig.probes(),
        probes,
        "probed before the retry: {:?}",
        rig.kinds()
    );
    assert_eq!(rig.kbps, held);
    let retry = rig.until_probe();
    assert_eq!(retry.from_kbps, held);
    assert!(
        u64::from(retry.to_kbps) * 100 <= u64::from(held) * 106,
        "{retry:?}"
    );
    assert!(rig.lr.pending.iter().flatten().all(|step| step.retry));
    // Alone: no second step while it is unconfirmed.
    rig.clean(1_000);
    assert_eq!(rig.probes(), probes + 1, "{:?}", rig.kinds());
}

/// A retry that confirms near the rate that failed raises the ceiling by its
/// step, and the next retry follows; loss soon after is the retries failing:
/// back under them, and the next retry waits twice as long.
#[test]
fn a_cut_soon_after_a_retry_confirms_is_the_retry_failing() {
    let mut rig = at_a_learned_ceiling();
    let held = rig.lr.current;
    rig.until_probe();
    let step = rig.lr.current;
    for _ in 0..50 {
        if rig.lr.learned != Some(held) {
            break;
        }
        rig.clean(100);
    }
    assert_eq!(rig.lr.learned, Some(step), "the retry confirmed");
    // Its window is still open whether or not the next retry went yet.
    assert!(rig.lr.recent(rig.now()).is_some_and(|c| c.retry));
    let cuts = rig.cuts();
    rig.lose_three();
    rig.clean(300);
    let failed = rig.last(ChangeKind::ProbeFailed).expect("a failed retry");
    assert_eq!(rig.cuts(), cuts, "a failed retry, not a cut: {failed:?}");
    assert!(rig.lr.current <= step, "{failed:?}");
    assert!(rig.lr.learned.is_some_and(|learned| learned <= step));
    assert_eq!(rig.lr.retry_wait, RETRY_FIRST * 2);
}

/// Low, review 2026-11-17: frames lost to datagrams too large for the path
/// cut the rate (E3) but are not the PC's send path being the limit, and one
/// or two refused frames are not either.
#[test]
fn only_refused_frames_make_the_host_the_limit() {
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    rig.run(PERIOD_MS * 3, |lr, frame, now| {
        lr.on_send_too_large(frame, now)
    });
    assert!(!rig.lr.host_limited(rig.now()));
    let mut left = 2;
    rig.run(PERIOD_MS * 3, |lr, frame, now| {
        if left > 0 {
            left -= 1;
            lr.on_send_loss(frame, now);
        }
    });
    assert!(!rig.lr.host_limited(rig.now()), "two refused frames");
    rig.run(PERIOD_MS, |lr, frame, now| lr.on_send_loss(frame, now));
    assert!(rig.lr.host_limited(rig.now()));
}

/// The field log: the host's socket refused frames in the same seconds as
/// hundreds of unrecoverable frames and a 200 ms round trip. That is the
/// path's limit, and a repeat of it teaches the ceiling as client loss does.
#[test]
fn refusals_that_come_with_client_loss_are_the_paths_limit() {
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    let mut left = 3;
    rig.run(200, |lr, frame, now| {
        if left > 0 {
            left -= 1;
            lr.on_send_loss(frame, now);
            unrecoverable(lr, frame, now);
        }
    });
    rig.clean(300);
    assert_eq!(rig.cuts(), 1, "{:?}", rig.changes);
    assert!(rig.lr.failed_at.is_some(), "the edge was noted");
}

/// The 2026-11-17 field case: 4K120 PyroWave asking 800 Mbps (about 680 on
/// the wire) on a path good for 640 behind a 270 ms buffer, with and without
/// the host's socket refusing frames while that buffer is over 100 ms full.
/// The controller before held the request into a full buffer (no room: client
/// loss could not cut; p95 queue 270 ms, over the link every second) or, with
/// refusals, cut three times and stood over the link a quarter of the time
/// (p95 99 ms); the first learned-ceiling build ratcheted its ceiling into a
/// floor (over the link 87 % of the time, 270 ms) or failed ten probes. Now
/// it settles under the edge: a cut or two, failed retries further and
/// further apart, a short queue.
#[test]
fn a_deep_buffer_path_settles_under_its_edge_without_a_sawtooth() {
    for refuse in [None, Some(100.)] {
        let link = {
            let mut link = Link::constant(640.);
            link.queue_ms = 270.;
            link.delay = 0.003;
            link
        };
        let mut sim = Sim::pyrowave_4k120(link, 800_000, 1.);
        sim.refuse_above_ms = refuse;
        sim.run(300.);
        let r = sim.report(0., 300.);
        let what = format!("refusing {refuse:?}: {r:?}");
        assert!(r.cuts <= 2, "{what}");
        assert!(r.failed_probes <= 6, "{what}");
        assert!(r.over_link <= 0.10, "{what}");
        assert!(r.p95_queue_ms <= 60., "{what}");
        assert!(r.delivered_mbps >= 640. * 0.85, "{what}");
        assert!(r.frame_loss < 0.001, "{what}");
        // Each failure at the edge after the first cut comes later than the
        // one before.
        let failures: Vec<f64> = sim
            .changes
            .iter()
            .filter(|(t, c)| {
                *t > 10. && matches!(c.kind, ChangeKind::ProbeFailed | ChangeKind::StepDown)
            })
            .map(|(t, _)| *t)
            .collect();
        let gaps: Vec<f64> = failures.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(
            gaps.windows(2).all(|g| g[1] >= g[0] - 5.),
            "{what}: failures at {failures:?}"
        );
    }
}

/// H.26x into a 400 ms buffer (the `deep-r50-hevc-hi` arm's shape): the first
/// loss cut left the buffer full, and nothing cut again because no probe had
/// raised the rate (p95 queue 390 ms, a frame in a hundred lost). A queue
/// that stands on after a loss cut is the stream's own.
#[test]
fn a_queue_left_standing_after_a_loss_cut_is_cut_too() {
    let mut link = Link::constant(50.);
    link.queue_ms = 400.;
    let mut sim = Sim::new(link, 75_000, 20.);
    sim.run(120.);
    let r = sim.report(30., 120.);
    assert!(r.p95_queue_ms < 100., "{r:?}");
    assert!(r.over_link < 0.2, "{r:?}");
    assert!(r.frame_loss < 0.005, "{r:?}");
}

/// Review 2026-11-17: refusals while a queue stands in the path are the
/// path's, so they are not reported as the PC's limit.
#[test]
fn refusals_with_a_queue_in_the_path_are_not_called_the_pcs() {
    for (rtt, reason) in [(10, "send_losses"), (60, "send_losses_queued")] {
        let mut rig = Rig::new(40_000);
        rig.clean(2_000);
        rig.rtt = Duration::from_millis(rtt);
        let mut left = 3;
        rig.run(200, |lr, frame, now| {
            if left > 0 {
                left -= 1;
                lr.on_send_loss(frame, now);
            }
        });
        let change = &rig.changes.first().expect("refusals cut").1;
        assert_eq!(change.reason, reason, "rtt {rtt}: {change:?}");
    }
}

/// Review 2026-11-17: the edge of one dip stayed known after the stream was
/// back at its request, so a second, unrelated dip near that rate engaged
/// the learned ceiling. Each dip that passes recovers the same way.
#[test]
fn a_second_dip_recovers_like_the_first() {
    let link = Link::new(|t| {
        if (20. ..40.).contains(&t) || (120. ..140.).contains(&t) {
            10e6
        } else {
            80e6
        }
    });
    let mut sim = Sim::new(q(link, 50.), 40_000, 20.);
    sim.run(200.);
    let first = sim.reaches(40., 36_000).expect("back after the first dip") - 40.;
    let second = sim
        .reaches(140., 36_000)
        .expect("back after the second dip")
        - 140.;
    assert!(first <= 25., "first dip: {first:.1} s");
    assert!(
        second <= first + 5.,
        "second dip: {second:.1} s, first {first:.1} s"
    );
}

#[test]
fn the_pyrowave_floor_uses_whole_frames_per_second() {
    // A 59.94 or 119.88 stream is judged at the whole rate, as the client's own minimum is.
    let stream = |fps, rate_millihz| Negotiated {
        codec: 3,
        width: 1920,
        height: 1080,
        fps,
        rate_millihz,
        bitrate_kbps: 1000,
        configured_bitrate_kbps: 1000,
        ..Default::default()
    };
    assert_eq!(
        pyrowave_floor_kbps(&stream(60, 59_940)),
        crate::pyrowave::minimum_kbps(1920, 1080, 60_000)
    );
    assert_eq!(
        pyrowave_floor_kbps(&stream(120, 119_880)),
        crate::pyrowave::minimum_kbps(1920, 1080, 120_000)
    );
    assert_eq!(
        pyrowave_recommended_kbps(&stream(60, 59_940)),
        crate::pyrowave::recommended_kbps(1920, 1080, 60_000)
    );
    // A client that asked exactly a rate is not red or amber for the few Mbps the picture gives
    // up to sound and control: the published rate and the coded one are scaled alike.
    let floor = crate::pyrowave::minimum_kbps(1920, 1080, 60_000);
    let exact = Negotiated {
        codec: 3,
        bitrate_kbps: floor - 752,
        configured_bitrate_kbps: floor,
        ..Default::default()
    };
    assert_eq!(pyrowave_floor_kbps(&exact), floor - 752);
    let recommended = crate::pyrowave::recommended_kbps(1920, 1080, 60_000);
    let exact = Negotiated {
        codec: 3,
        bitrate_kbps: recommended - 1300,
        configured_bitrate_kbps: recommended,
        ..Default::default()
    };
    assert_eq!(pyrowave_recommended_kbps(&exact), recommended - 1300);
}
