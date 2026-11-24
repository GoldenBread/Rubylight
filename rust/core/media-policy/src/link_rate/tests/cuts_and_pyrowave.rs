//! Unit tests for cut fences, floors, PyroWave bounds, and client takeover.

use super::*;

#[test]
fn reports_for_frames_sent_before_a_cut_took_effect_are_ignored() {
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    rig.lose_three();
    rig.clean(150);
    assert_eq!(rig.cuts(), 1);
    // Late reports for frames sent at the old rate (the queue is still
    // draining): plenty of them, and none counts.
    let last = rig.frame;
    let now = rig.now();
    for old in last - 40..last - 10 {
        unrecoverable(&mut rig.lr, old, now);
    }
    rig.clean(1_500);
    assert_eq!(rig.cuts(), 1, "{:?}", rig.changes);
    // Frames sent after the cut do count: a second cut follows.
    rig.lose_three();
    rig.clean(500);
    assert_eq!(rig.cuts(), 2, "{:?}", rig.changes);
}

#[test]
fn a_second_cut_right_after_the_first_may_go_below_half() {
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    rig.lose_three();
    rig.clean(1_000);
    let first = rig.kbps;
    // The first cut went down by at most half (40 -> 19 Mbit/s); the link
    // carries far less, so the next one inside the window is not held to
    // half of that again.
    assert!(first >= 40_000 * 40 / 100, "{first}");
    rig.lose_three();
    rig.clean(500);
    assert_eq!(rig.cuts(), 2);
    assert!(rig.kbps < first / 2, "{first} -> {}", rig.kbps);
    assert!(rig.kbps >= 1_000, "{}", rig.kbps);
}

#[test]
fn a_cut_after_a_quiet_window_is_held_to_half_again() {
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    rig.lose_three();
    rig.clean(1_000);
    let first = rig.kbps;
    // The previous cut is older than the evidence window: an isolated cut.
    rig.lr.last_cut = rig.lr.last_cut.map(|at| at - Duration::from_secs(3));
    rig.lose_three();
    rig.clean(500);
    assert_eq!(rig.cuts(), 2);
    assert!(rig.kbps >= first * 49 / 100, "{first} -> {}", rig.kbps);
}

#[test]
fn it_probes_back_up_in_small_steps_and_returns_to_full() {
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    rig.lose_three();
    rig.clean(60_000);
    let probes: Vec<_> = rig
        .changes
        .iter()
        .filter(|(_, c)| c.kind == ChangeKind::Probe)
        .collect();
    assert!(!probes.is_empty());
    let cut_at = rig.changes[0].0;
    assert!(probes[0].0 >= cut_at + 2_000, "first probe waits the hold");
    for (_, p) in &probes {
        // One step is about 12.5 % (5 % near the failing rate), never 25 %.
        assert!(p.to_kbps > p.from_kbps, "{p:?}");
        assert!(p.to_kbps * 100 <= p.from_kbps * 125, "{p:?}");
    }
    assert!(rig.kinds().contains(&ChangeKind::Full), "{:?}", rig.kinds());
    assert_eq!(rig.lr.state(), State::Full);
    assert_eq!(rig.kbps, 40_000, "Full goes back to the requested rate");
}

#[test]
fn a_failed_probe_returns_to_the_last_good_rate_and_waits_twice_as_long() {
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    rig.lose_three();
    let mut probed = None;
    for _ in 0..200 {
        rig.clean(100);
        if let Some((at, c)) = rig
            .changes
            .iter()
            .find(|(_, c)| c.kind == ChangeKind::Probe)
        {
            probed = Some((*at, *c));
            break;
        }
    }
    let (probe_at, probe) = probed.expect("a probe");
    rig.lose_three();
    rig.clean(400);
    let failed = rig
        .changes
        .iter()
        .find(|(_, c)| c.kind == ChangeKind::ProbeFailed)
        .copied()
        .expect("probe failed");
    assert_eq!(failed.1.to_kbps, probe.from_kbps, "back to the good rate");
    assert_eq!(rig.kbps, probe.from_kbps);
    assert_eq!(rig.cuts(), 1, "a failed probe is not another cut");
    // The wait doubled from 2 s to 4 s.
    rig.clean(30_000);
    let next = rig
        .changes
        .iter()
        .find(|(at, c)| *at > failed.0 && c.kind == ChangeKind::Probe)
        .expect("probes again");
    assert!(next.0 >= failed.0 + 4_000, "{} after {}", next.0, failed.0);
    assert!(probe_at < failed.0);
}

#[test]
fn it_does_not_probe_while_the_rtt_is_elevated() {
    let mut rig = Rig::new(40_000);
    rig.clean(3_000);
    rig.lose_three();
    rig.clean(500);
    rig.rtt = Duration::from_millis(300);
    rig.clean(15_000);
    assert!(
        rig.kinds().iter().all(|k| *k == ChangeKind::StepDown),
        "{:?}",
        rig.kinds()
    );
    rig.rtt = Duration::from_millis(20);
    rig.clean(5_000);
    assert!(rig.kinds().contains(&ChangeKind::Probe));
}

#[test]
fn it_never_goes_below_the_floor_and_says_so() {
    let mut rig = Rig::new(4_000);
    rig.clean(1_000);
    let mut lowest = rig.kbps;
    for _ in 0..12 {
        rig.lose_three();
        rig.clean(1_200);
        assert!(rig.kbps >= 2_000, "{}", rig.kbps);
        lowest = lowest.min(rig.kbps);
    }
    // The wire allowance for 2 Mbit/s is whole packets per frame.
    assert!((2_000..=2_500).contains(&lowest), "{lowest}");
    assert!(
        rig.kinds().contains(&ChangeKind::Floor),
        "{:?}",
        rig.kinds()
    );
    // The floor line says what the link delivered, not 0 (2026-10-24 logs).
    let floor = rig
        .changes
        .iter()
        .find(|(_, c)| c.kind == ChangeKind::Floor);
    let (_, floor) = floor.expect("a floor change");
    assert!(floor.delivered_kbps > 0, "{floor:?}");
    assert!(floor.evidence.loss_share > 0., "{floor:?}");
    // And the probe after it, once the losses stop.
    rig.clean(60_000);
    let probe = rig
        .changes
        .iter()
        .find(|(_, c)| c.kind == ChangeKind::Probe);
    let (_, probe) = probe.expect("a probe change");
    assert!(probe.delivered_kbps > 0, "{probe:?}");
}
/// 2026-10-29: PyroWave at 200 Mbps to a phone on Wi-Fi lost frames for the
/// whole stream and its bitrate never moved; the controller was off for it.
#[test]
fn pyrowave_loss_is_cut_like_any_codecs_and_a_healthy_pyrowave_link_is_left_alone() {
    let lr = LinkRate::new(true, &pyrowave(400_000), 0, 20);
    assert_eq!(lr.state(), State::Full);
    let floor = lr.floor_kbps;
    let mut rig = Rig::with(lr, 400_000);
    rig.run(60_000, |lr, frame, now| {
        if frame % 7 == 0 {
            repaired(lr, frame, 2, now);
        }
    });
    assert!(rig.changes.is_empty(), "{:?}", rig.kinds());
    assert_eq!(rig.kbps, 400_000);
    rig.lose_three();
    rig.clean(500);
    assert_eq!(rig.cuts(), 1, "{:?}", rig.kinds());
    let (_, cut) = rig.changes[0];
    assert_eq!(cut.reason, "unrecoverable_frames");
    assert!(cut.to_kbps < 400_000 * 85 / 100, "{cut:?}");
    assert!(cut.to_kbps >= floor, "{cut:?}");
    assert_eq!(rig.kbps, cut.to_kbps);
    assert_eq!(rig.lr.state(), State::Limited);
}

/// Review 2026-10-29: while a PyroWave picture moves only its coarse block
/// has parity, and a client reports every detail block that lost a packet.
/// Counted as lost frames, 0.1 % random loss (one picture in six at 200 Mbps)
/// cut a healthy link to the floor. A lost detail block only blurs part of one
/// picture; a lost coarse block, or any block of a legacy stream, loses it.
#[test]
fn only_a_lost_coarse_block_loses_a_pyrowave_frame() {
    let detail = |block| Status {
        block,
        blocks: 3,
        ..status(1, 60, 16)
    };
    let records = LinkRate::new(true, &pyrowave(200_000), 0, 20);
    assert!(records.frame_lost(&detail(0)));
    assert!(!records.frame_lost(&detail(1)));
    assert!(!records.frame_lost(&detail(2)));
    assert!(!records.frame_lost(&status(1, 80, 2)), "rebuilt");
    // Review 2026-10-29: without coarse parity (critical FEC 0, or coarse
    // records and parity over 255 packets) the picture goes in quarters and
    // block 0 is a quarter of the detail.
    let quarter = Status {
        block: 0,
        blocks: 4,
        parity: 0,
        ..status(1, 60, 16)
    };
    assert!(!records.frame_lost(&quarter));
    let legacy = LinkRate::new(
        true,
        &Negotiated {
            pyrowave_records: false,
            ..pyrowave(200_000)
        },
        0,
        20,
    );
    assert!(legacy.frame_lost(&detail(1)));
    let h26x = LinkRate::new(true, &stream(40_000), 0, 20);
    assert!(h26x.frame_lost(&detail(1)));
}

/// Review 2026-10-29: the floor was 2 Mbps for every codec, far under the
/// rate at which PyroWave keeps a usable picture (187 Mbps at 1080p60).
#[test]
fn pyrowave_stops_at_its_test_scene_detail_level_and_says_so() {
    let minimum = crate::pyrowave::minimum_kbps(1920, 1080, 60_000);
    let lr = LinkRate::new(true, &pyrowave(400_000), 0, 20);
    assert_eq!(lr.floor_kbps, minimum);
    // The session's bitrate is the negotiated one; the encoder gets it scaled
    // by configured over negotiated, so the floor is scaled back.
    let scaled = LinkRate::new(
        true,
        &Negotiated {
            configured_bitrate_kbps: 400_000,
            ..pyrowave(397_000)
        },
        0,
        20,
    );
    assert_eq!(
        scaled.floor_kbps,
        (u64::from(minimum) * 397_000 / 400_000) as u32
    );
    let mut rig = Rig::with(lr, 400_000);
    for _ in 0..12 {
        rig.lose_three();
        rig.clean(600);
    }
    let floor = rig
        .changes
        .iter()
        .find(|(_, c)| c.kind == ChangeKind::Floor)
        .map(|(_, c)| c.to_kbps);
    assert_eq!(floor, Some(minimum), "{:?}", rig.changes);
    assert!(
        rig.changes.iter().all(|(_, c)| c.to_kbps >= minimum),
        "{:?}",
        rig.changes
    );
    // H.26x keeps its 2 Mbps floor.
    assert_eq!(
        LinkRate::new(true, &stream(40_000), 0, 20).floor_kbps,
        2_000
    );
}

/// Review 2026-10-29: the controller held every codec to 500 Mbps, but
/// PyroWave streams go to 2 Gbps: a 1.2 Gbps stream was cut to 425 Mbps and
/// probed only up to 500, then jumped back to 1.2 Gbps at the end.
#[test]
fn a_pyrowave_stream_above_500_mbps_keeps_its_ceiling() {
    let lr = LinkRate::new(true, &pyrowave(1_200_000), 0, 20);
    assert_eq!(lr.ceiling_kbps(), 1_200_000);
    assert_eq!(
        LinkRate::new(true, &pyrowave(3_000_000), 0, 20).ceiling_kbps(),
        2_000_000
    );
    assert_eq!(
        LinkRate::new(true, &stream(1_200_000), 0, 20).ceiling_kbps(),
        500_000
    );
    let mut rig = Rig::with(lr, 1_200_000);
    rig.clean(1_000);
    rig.lose_three();
    rig.clean(500);
    let (_, cut) = rig.changes[0];
    assert!(cut.to_kbps > 500_000, "{cut:?}");
    for _ in 0..40 {
        rig.clean(1_000);
    }
    assert_eq!(rig.kbps, 1_200_000, "{:?}", rig.kinds());
}

#[test]
fn it_is_off_when_disabled_and_after_the_client_sets_the_bitrate() {
    let disabled = LinkRate::new(false, &stream(40_000), 0, 20);
    assert_eq!(disabled.state(), State::Off);
    let mut rig = Rig::with(disabled, 40_000);
    rig.clean(1_000);
    rig.lose_three();
    rig.clean(2_000);
    assert!(rig.changes.is_empty());
    assert_eq!(rig.kbps, 40_000);

    let mut rig = Rig::new(40_000);
    rig.clean(1_000);
    rig.lr.client_takeover();
    rig.lose_three();
    rig.clean(2_000);
    assert!(rig.changes.is_empty());
    assert_eq!(rig.lr.state(), State::Off);
    assert_eq!(rig.lr.snapshot(rig.now())["reason"], "client_controlled");
}

#[test]
fn max_bitrate_lowers_the_ceiling() {
    let lr = LinkRate::new(true, &stream(80_000), 30_000, 20);
    assert_eq!(lr.ceiling_kbps(), 30_000);
    let mut rig = Rig::with(lr, 30_000);
    rig.clean(1_000);
    rig.lose_three();
    rig.clean(500);
    assert!(rig.kbps < 30_000);
}
