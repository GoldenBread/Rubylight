//! Unit tests for loss evidence triggers, reordering baselines, and keyframe/invalidation requests.

use super::*;

// ---- unit tests ----

#[test]
fn a_healthy_link_gets_no_target_and_no_change_in_a_minute() {
    let mut rig = Rig::new(40_000);
    // Recovered loss well under the threshold, including a packet or two per
    // frame, is what a good Wi-Fi link looks like.
    rig.run(60_000, |lr, frame, now| {
        if frame % 7 == 0 {
            repaired(lr, frame, 2, now);
        }
    });
    assert!(rig.changes.is_empty());
    assert_eq!(rig.kbps, 40_000);
    assert_eq!(rig.lr.state(), State::Full);
    assert_eq!(
        rig.lr.tick(
            rig.now(),
            rig.rtt,
            20,
            Duration::from_micros(16_667),
            40_000,
            true
        ),
        None
    );
}

#[test]
fn three_unrecoverable_frames_cut_once_to_between_half_and_85_percent() {
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    rig.lose_three();
    rig.clean(500);
    assert_eq!(rig.cuts(), 1);
    let (_, cut) = rig.changes[0];
    assert_eq!(cut.reason, "unrecoverable_frames");
    assert_eq!(cut.from_kbps, 40_000);
    assert!(cut.to_kbps < 40_000 * 85 / 100, "{cut:?}");
    assert!(cut.to_kbps >= 40_000 * 40 / 100, "{cut:?}");
    assert_eq!(rig.kbps, cut.to_kbps);
    assert_eq!(rig.lr.state(), State::Limited);
    assert_eq!(cut.evidence.unrecoverable_frames, 3);
}

#[test]
fn a_report_before_the_frame_is_recorded_counts_nothing_and_one_after_does() {
    // The host records a frame once it has sent it (see the module notes).
    let now = Instant::now();
    let mut lr = LinkRate::new(true, &stream(40_000), 0, 20);
    let early = status(7, 5, 0);
    lr.on_status(&early, now);
    assert_eq!(lr.events.missing, 0);
    lr.on_sent(7, PACKETS, WIRE_BYTES, now);
    lr.on_status(&early, now);
    assert!(lr.events.missing > 0);
    assert_eq!(lr.sent(7).unwrap().missing, lr.events.missing as u32);
}

#[test]
fn two_unrecoverable_frames_or_three_spread_over_time_do_not_cut() {
    let mut rig = Rig::new(40_000);
    let mut left = 2;
    rig.run(1_000, |lr, frame, now| {
        if left > 0 {
            left -= 1;
            unrecoverable(lr, frame, now);
        }
    });
    rig.clean(5_000);
    // Three, but one every 2.5 s.
    for _ in 0..3 {
        let mut once = true;
        rig.run(2_500, |lr, frame, now| {
            if std::mem::take(&mut once) {
                unrecoverable(lr, frame, now);
            }
        });
    }
    assert!(rig.changes.is_empty(), "{:?}", rig.changes);
}

#[test]
fn the_same_frame_reported_in_several_blocks_counts_once() {
    let mut rig = Rig::new(40_000);
    rig.run(500, |lr, frame, now| {
        for _ in 0..5 {
            unrecoverable(lr, frame.min(1), now);
        }
    });
    assert!(rig.changes.is_empty());
}

#[test]
fn recovered_loss_never_cuts_however_many_packets_it_lacked() {
    // A block is reported the moment `data` packets are in, so the parity still
    // in flight reads as missing; FEC rebuilt every one of these frames.
    for lost_parity in [4, 8, 16] {
        let mut rig = Rig::new(40_000);
        rig.run(20_000, |lr, frame, now| {
            repaired(lr, frame, lost_parity, now)
        });
        assert!(rig.changes.is_empty(), "{lost_parity}: {:?}", rig.changes);
        assert_eq!(rig.kbps, 40_000);
    }
}

#[test]
fn an_occasional_failed_frame_is_not_a_cut() {
    // One frame in sixty fails beyond repair: under three in two seconds, and
    // under 5 % of the packets sent.
    let mut rig = Rig::new(40_000);
    let mut n = 0;
    rig.run(20_000, |lr, frame, now| {
        n += 1;
        if n % 60 == 0 {
            unrecoverable(lr, frame, now);
        }
    });
    assert!(rig.changes.is_empty(), "{:?}", rig.changes);
}

/// A frame that lost `missing` data packets that FEC rebuilt.
fn rebuilt_data(lr: &mut LinkRate, frame: u32, missing: u16, now: Instant) {
    lr.on_status(&status(frame, 80 - missing, 0), now);
}

#[test]
fn rebuilt_data_loss_cuts_only_while_the_rtt_is_elevated() {
    // (rtt ms, data packets missing per 100 sent, cut)
    for (rtt, missing, cut) in [
        (20, 10, false),
        (100, 10, true),
        // 4 of 80 data packets is 5 % of the block's 96, not 4 % of the 100 sent.
        (100, 4, true),
        (100, 1, false),
        (100, 0, false),
    ] {
        let mut rig = Rig::new(40_000);
        rig.clean(3_000);
        rig.rtt = Duration::from_millis(rtt);
        rig.run(5_000, |lr, frame, now| {
            rebuilt_data(lr, frame, missing, now)
        });
        assert_eq!(
            rig.cuts() > 0,
            cut,
            "rtt {rtt}, {missing}: {:?}",
            rig.changes
        );
        if cut {
            assert_eq!(rig.changes[0].1.reason, "fec_loss_share");
        }
    }
}

#[test]
fn a_queue_filling_on_a_link_the_stream_exceeds_cuts_before_fec_gives_out() {
    // 50 Mbit/s with a 50 ms drop-tail queue and about 7 % more on the wire
    // than the link carries, packets spread over the frame period: FEC rebuilds
    // every frame (under 1 % are lost), the RTT climbs by the whole queue, and
    // the stream sits there. Only the rebuilt blocks, with the RTT, show it.
    let mut sim = Sim::new(q(Link::constant(50.), 50.), 38_800, 20.);
    sim.spread = 1.;
    sim.run(30.);
    let first = sim.changes.first().expect("a cut");
    assert_eq!(first.1.kind, ChangeKind::StepDown);
    assert_eq!(first.1.reason, "fec_loss_share");
    assert!(first.0 < 5., "cut at {} s", first.0);
    let wire = sim.mean_wire_bps(10., 30.) / 1e6;
    assert!(
        (50. * 0.65..=50. * 1.02).contains(&wire),
        "{wire:.1} Mbit/s"
    );
    assert!(
        sim.frame_loss(3., 30.) < 0.02,
        "{}",
        sim.frame_loss(3., 30.)
    );
}

#[test]
fn an_encoder_at_one_and_a_fifth_of_the_link_cuts_within_two_seconds_and_settles_under_it() {
    let mut sim = Sim::new(q(Link::constant(50.), 50.), 60_000, 20.);
    sim.run(30.);
    let first = sim.changes.first().expect("a cut");
    assert_eq!(first.1.kind, ChangeKind::StepDown);
    assert!(first.0 < 2., "cut at {} s", first.0);
    let wire = sim.mean_wire_bps(10., 30.) / 1e6;
    assert!(
        (50. * 0.65..=50. * 1.08).contains(&wire),
        "{wire:.1} Mbit/s"
    );
    assert!(
        sim.frame_loss(3., 30.) < 0.02,
        "{}",
        sim.frame_loss(3., 30.)
    );
}
#[test]
fn the_wifi_arm_with_reordering_and_random_loss_never_cuts() {
    let mut rig = Rig::new(14_988);
    // Far more than the 3 +/- 2 ms each way of the arm.
    rig.rtt_jitter_ms = 8;
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let (mut blocks, mut rebuilt) = (0u32, 0u32);
    rig.run(45_000, |lr, frame, now| {
        blocks += 1;
        rebuilt += u32::from(wifi_block(&mut seed, lr, frame, 7, 5, 0, now));
    });
    assert!(rig.changes.is_empty(), "{:?}", rig.changes);
    assert_eq!(rig.kbps, 14_988);
    // The model is the log: nearly every block is reported, and the data
    // packets the client had not yet were far over the 5 % that cut. Counting
    // them as loss (or the whole tail, 18 %) would cut this stream.
    assert!(rebuilt * 100 > blocks * 95, "{rebuilt} of {blocks}");
    let share = rig.lr.snapshot(rig.now())["rebuilt_data_missing_percent"]
        .as_f64()
        .unwrap();
    assert!(share > 5., "{share}");
}

/// The review of 2026-10-24: the same arm with the RTT up from 20 to 60 ms for
/// 1.5 s at 20 s (a Wi-Fi roam, a competing flow) gave a cut 14,988 -> 8,705
/// and a rate still 14,027 kbps 21 s later: while the RTT is up the reordering
/// share counted as loss. Only what the flat RTT did not already show is a
/// queue; a real one, with loss on top, still cuts.
#[test]
fn a_short_rtt_excursion_on_the_wifi_arm_is_no_cut_but_a_real_queue_is() {
    // Data packets behind a queue on top of the arm's, per block, while the
    // RTT is up.
    // At 20, 30 and 40 % FEC the data a reordering path has not delivered at
    // report time is up to the parity, 0.18 to 0.31 of the data (review
    // 2026-10-24: a cap of 0.2 cut 14,988 -> 10,156 at 10, 13 and 16 parity).
    for (parity, extra) in [
        (7u16, 0u16),
        (7, 3),
        (10, 0),
        (10, 3),
        (13, 0),
        (13, 3),
        (16, 0),
        (16, 3),
    ] {
        let mut rig = Rig::new(14_988);
        rig.rtt_jitter_ms = 8;
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        rig.run(20_000, |lr, frame, now| {
            wifi_block(&mut seed, lr, frame, parity, 5, 0, now);
        });
        assert!(rig.changes.is_empty(), "{:?}", rig.changes);
        rig.rtt = Duration::from_millis(60);
        rig.run(1_500, |lr, frame, now| {
            wifi_block(&mut seed, lr, frame, parity, 5, extra, now);
        });
        rig.rtt = Duration::from_millis(20);
        rig.run(20_000, |lr, frame, now| {
            wifi_block(&mut seed, lr, frame, parity, 5, 0, now);
        });
        assert_eq!(
            rig.cuts() > 0,
            extra > 0,
            "parity {parity} extra {extra}: {:?}",
            rig.changes
        );
        if extra == 0 {
            assert_eq!(rig.kbps, 14_988);
        }
    }
}

/// Re-review 2026-10-24: the reordering baseline had no time decay. After 20 s
/// with 2 of 80 data packets missing, 10 s with 10 missing and five clean
/// minutes, a queue with 8 missing a frame never cut (without the 10 s spell
/// it cut in 1.4 s). It also had no cap: a flat-RTT share of loss is not
/// reordering.
#[test]
fn an_old_spell_of_flat_rtt_loss_does_not_hide_a_queue_later() {
    // (missing per frame in the spell, seconds, seconds of silence, queue missing)
    for (spell, spell_s, quiet_s, queue, cut) in [(0, 0, 0, 8, true), (10, 10, 300, 8, true)] {
        let mut rig = Rig::new(40_000);
        rig.run(20_000, |lr, frame, now| rebuilt_data(lr, frame, 2, now));
        if spell > 0 {
            rig.run(spell_s * 1_000, |lr, frame, now| {
                rebuilt_data(lr, frame, spell, now)
            });
        }
        rig.clean(quiet_s * 1_000);
        assert!(rig.changes.is_empty(), "{:?}", rig.changes);
        rig.rtt = Duration::from_millis(60);
        rig.run(20_000, |lr, frame, now| rebuilt_data(lr, frame, queue, now));
        assert_eq!(
            rig.cuts() > 0,
            cut,
            "spell {spell} for {spell_s} s, quiet {quiet_s} s, queue {queue}: {:?}",
            rig.changes
        );
    }
}

#[test]
fn a_flat_rtt_share_over_what_reordering_produces_is_not_the_baseline() {
    // Data 32, parity 16: 14 data packets missing is 0.44, over the 0.35 that
    // reordering can leave, and still rebuilt.
    fn block(lr: &mut LinkRate, frame: u32, missing: u16, now: Instant) {
        lr.on_status(
            &Status {
                frame,
                highest_received: 0,
                next_contiguous: 0,
                missing_before_highest: 0,
                data: 32,
                parity: 16,
                received_data: 32 - missing,
                received_parity: 16,
                percentage: 50,
                block: 0,
                blocks: 1,
            },
            now,
        );
    }
    let mut rig = Rig::new(40_000);
    rig.run(30_000, |lr, frame, now| block(lr, frame, 14, now));
    assert!(rig.changes.is_empty(), "{:?}", rig.changes);
    rig.rtt = Duration::from_millis(60);
    rig.run(10_000, |lr, frame, now| block(lr, frame, 14, now));
    assert!(rig.cuts() > 0, "{:?}", rig.changes);
}

/// Re-review 2026-10-24: the excess counted every rise of its running sum and
/// never gave back a fall, so noise around the baseline built up: data missing
/// alternating 0 and 16 of 80 against a learned 0.1 cut 40,000 -> 28,542 ->
/// 19,349 in 1.8 s.
#[test]
fn noise_around_the_reordering_baseline_never_adds_up() {
    let mut rig = Rig::new(40_000);
    rig.run(10_000, |lr, frame, now| rebuilt_data(lr, frame, 8, now));
    rig.rtt = Duration::from_millis(60);
    let mut n = 0u32;
    rig.run(20_000, |lr, frame, now| {
        n += 1;
        rebuilt_data(lr, frame, if n.is_multiple_of(2) { 0 } else { 16 }, now)
    });
    assert!(rig.changes.is_empty(), "{:?}", rig.changes);
}

#[test]
fn a_five_percent_burst_in_one_window_does_not_cut() {
    let mut rig = Rig::new(40_000);
    rig.clean(1_000);
    // One window of loss, then clean traffic: the second window resets it.
    rig.run(450, |lr, frame, now| repaired(lr, frame, 8, now));
    rig.clean(5_000);
    assert!(rig.changes.is_empty(), "{:?}", rig.changes);
}

#[test]
fn a_quiet_stream_cannot_cut_on_a_single_lost_packet() {
    let mut rig = Rig::new(40_000);
    // Under LOSS_MIN_PACKETS sent per window.
    for _ in 0..4 {
        let now = rig.now();
        rig.lr.on_sent(rig.frame + 1, 10, 11_000, now);
        rig.frame += 1;
        repaired(&mut rig.lr, rig.frame, 6, now);
        rig.clean(500);
    }
    assert!(rig.changes.is_empty());
}

#[test]
fn three_frames_lost_on_send_cut() {
    let mut rig = Rig::new(40_000);
    rig.clean(1_000);
    let mut left = 3;
    rig.run(200, |lr, frame, now| {
        if left > 0 {
            left -= 1;
            lr.on_send_loss(frame, now);
        }
    });
    rig.clean(300);
    assert_eq!(rig.cuts(), 1);
    assert_eq!(rig.changes[0].1.reason, "send_losses");
}

#[test]
fn network_keyframe_requests_cut_only_without_fec_status() {
    // A client that sends no FEC status: three network requests cut.
    let mut rig = Rig::new(40_000);
    rig.clean(1_000);
    for _ in 0..3 {
        rig.lr.on_keyframe_request(true, rig.now());
        rig.clean(100);
    }
    rig.clean(400);
    assert_eq!(rig.cuts(), 1);
    let (_, cut) = rig.changes[0];
    assert_eq!(cut.reason, "keyframe_requests");
    // No share is known: 70 % of the wire rate, less the audio reserve.
    assert!(
        cut.to_kbps < 40_000 * 75 / 100 && cut.to_kbps > 40_000 / 3,
        "{cut:?}"
    );

    // The decoder asked, not the network.
    let mut rig = Rig::new(40_000);
    rig.clean(1_000);
    for _ in 0..6 {
        rig.lr.on_keyframe_request(false, rig.now());
        rig.clean(100);
    }
    assert!(rig.changes.is_empty());

    // A client that reports FEC status is judged by it, not by its requests.
    let mut rig = Rig::new(40_000);
    rig.run(300, |lr, frame, now| repaired(lr, frame, 1, now));
    for _ in 0..6 {
        rig.lr.on_keyframe_request(true, rig.now());
        rig.clean(100);
    }
    rig.clean(1_000);
    assert!(rig.changes.is_empty());
}

/// 2026-10-29 sweep (r-def, AV1 to a Retroid, 07:55:55 UTC): the client lost
/// one frame and asked to invalidate 2685-2695 eleven times before the repair
/// came. Each request counted, ten of them cut 21 to 14.5 Mbps for 26 s on a
/// link that carried the stream. One loss is one incident; three separate
/// losses still cut.
#[test]
fn repeated_invalidations_for_one_loss_are_one_request() {
    let mut rig = Rig::new(21_388);
    rig.clean(1_000);
    for last in 2685..=2695 {
        rig.lr.on_invalidation(2685, last, true, rig.now());
        rig.clean(8);
    }
    rig.clean(2_000);
    assert!(rig.changes.is_empty(), "{:?}", rig.kinds());
    // A client still asking long after it began has not got its repair.
    let mut rig = Rig::new(21_388);
    rig.clean(1_000);
    for _ in 0..3 {
        rig.lr.on_invalidation(2685, 2695, true, rig.now());
        rig.clean(600);
    }
    rig.clean(400);
    assert_eq!(rig.cuts(), 1, "{:?}", rig.kinds());
    // Three losses, each asked for a few times.
    let mut rig = Rig::new(21_388);
    rig.clean(1_000);
    for first in [3000, 3060, 3120] {
        for last in first..first + 4 {
            rig.lr.on_invalidation(first, last, true, rig.now());
            rig.clean(8);
        }
        rig.clean(300);
    }
    rig.clean(400);
    assert_eq!(rig.cuts(), 1, "{:?}", rig.kinds());
    assert_eq!(rig.changes[0].1.reason, "keyframe_requests");
    // A range the host cannot read counts every time.
    let mut rig = Rig::new(21_388);
    rig.clean(1_000);
    for _ in 0..3 {
        rig.lr.on_invalidation(0, 0, true, rig.now());
        rig.clean(100);
    }
    rig.clean(400);
    assert_eq!(rig.cuts(), 1, "{:?}", rig.kinds());
}

/// rc.5's `KeyframeLoss` calls a request from a client that announced FEC
/// status (Moonlight, Artemis, Nightjar) its decoder asking again. The
/// controller now agrees from the first frame, before any status arrived.
#[test]
fn a_client_that_announced_fec_status_is_judged_by_it_from_the_start() {
    let announced = Negotiated {
        moonlight_features: crate::rtsp::FEC_STATUS_FEATURE,
        ..stream(21_388)
    };
    let mut rig = Rig::with(LinkRate::new(true, &announced, 0, 20), 21_388);
    rig.clean(1_000);
    for first in [3000, 3060, 3120, 3180] {
        rig.lr.on_invalidation(first, first + 2, true, rig.now());
        rig.lr.on_keyframe_request(true, rig.now());
        rig.clean(300);
    }
    rig.clean(1_000);
    assert!(rig.changes.is_empty(), "{:?}", rig.kinds());
    // Its lost frames still cut.
    rig.lose_three();
    rig.clean(500);
    assert_eq!(rig.cuts(), 1, "{:?}", rig.kinds());
}

#[test]
fn a_rise_in_rtt_alone_is_no_evidence() {
    let mut rig = Rig::new(40_000);
    rig.clean(3_000);
    rig.rtt = Duration::from_millis(400);
    rig.clean(20_000);
    assert!(rig.changes.is_empty());
    assert_eq!(rig.kbps, 40_000);
}
