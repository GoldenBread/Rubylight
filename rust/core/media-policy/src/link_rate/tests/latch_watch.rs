//! Unit tests for pacing-latch fences, watch windows, unlatching, and snapshots.

use super::*;

#[test]
fn a_latch_that_fixes_the_loss_gets_one_fence_before_a_cut() {
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    // Two unrecoverable frames, then the loss guard latches and slows the
    // pacing: the reports in flight from before it must not cut.
    let mut left = 2;
    rig.run(PERIOD_MS * 2 + 1, |lr, frame, now| {
        if left > 0 {
            left -= 1;
            unrecoverable(lr, frame, now);
        }
    });
    rig.lr.on_latch(rig.now(), true, true);
    let late = rig.frame;
    let now = rig.now();
    unrecoverable(&mut rig.lr, late, now);
    rig.clean(2_000);
    assert!(rig.changes.is_empty(), "{:?}", rig.changes);
    // Loss that continues after the fence is the network and cuts.
    rig.lose_three();
    rig.clean(500);
    assert_eq!(rig.cuts(), 1);
}

/// The 2026-10-24 `r50-hevc-hi` arm: 75 Mbps requested (58,988 kbps encoder) into
/// 50 Mbit/s. The start-up keyframes are lost, the loss guard latches at 0.8 s
/// from the start, and the client, waiting for a keyframe, reports only the
/// keyframes it asked for, about one every 1.2 to 2 s. Three frames took until
/// 4.9 s and the stream lost its first 5.8 s; once the latch has had its
/// fence, one is enough, and the cut comes with the first report after it.
#[test]
fn a_latch_that_did_not_help_cuts_on_the_first_unrecoverable_frame_after_it() {
    let mut rig = Rig::new(58_988);
    rig.clean(780);
    rig.lr.on_latch(rig.now(), true, true);
    let base = rig.base;
    // Keyframes at 0.9 s (inside the fence), 2.1 s and 4.1 s, each reported
    // about 100 ms after it was sent.
    let mut reports = vec![1_000u64, 2_200, 4_200];
    rig.run(3_000, |lr, frame, now| {
        let at = now.duration_since(base).as_millis() as u64;
        if reports.first().is_some_and(|r| at >= *r) {
            reports.remove(0);
            lost_keyframe(lr, frame, now);
        }
    });
    assert_eq!(rig.cuts(), 1, "{:?}", rig.changes);
    let (at, change) = &rig.changes[0];
    assert!(*at < 2_500, "cut at {at} ms");
    assert_eq!(change.reason, "unrecoverable_frames");
}

/// The same run, 2026-10-24: after the latch's fence the only sign of loss for
/// 1.5 s was the client's keyframe request, from a client that reports FEC
/// status (so requests are otherwise not evidence). No keyframe had got through
/// since the latch, so the latch had failed; the cut had no measured share and
/// the 30 % default left the stream above the link.
#[test]
fn a_keyframe_request_with_no_keyframe_through_since_the_latch_cuts_deep() {
    let mut rig = Rig::new(58_988);
    rig.run(300, |lr, frame, now| repaired(lr, frame, 1, now));
    rig.lr.on_latch(rig.now(), true, true);
    // A request inside the fence is nothing.
    rig.clean(300);
    rig.lr.on_keyframe_request(true, rig.now());
    rig.clean(1_000);
    assert!(rig.changes.is_empty(), "{:?}", rig.changes);
    rig.lr.on_keyframe_request(true, rig.now());
    rig.clean(300);
    assert_eq!(rig.cuts(), 1, "{:?}", rig.changes);
    let (_, cut) = rig.changes[0];
    assert_eq!(cut.reason, "keyframe_requests");
    assert!(cut.to_kbps <= 58_988 / 2, "{cut:?}");

    // The same request, a keyframe that was lost since the latch: cut.
    let mut rig = Rig::new(58_988);
    rig.run(300, |lr, frame, now| repaired(lr, frame, 1, now));
    rig.lr.on_latch(rig.now(), true, true);
    rig.clean(600);
    let (frame, now) = (rig.frame, rig.now());
    rig.lr.on_keyframe_sent(frame, now);
    rig.clean(700);
    rig.lr.on_status(&status(frame, 60, 0), rig.now());
    rig.lr.on_unrecoverable(frame, true, rig.now());
    rig.clean(300);
    assert_eq!(rig.cuts(), 1, "{:?}", rig.changes);
}

/// A start-up latch (the first keyframe was lost, pacing slowed) is followed by
/// a keyframe that arrives whole. A request after that is the decoder, a
/// resize or one burst, not proof that the latch failed: the stream must not be
/// halved on it, whether or not the client reports FEC status.
#[test]
fn a_keyframe_request_after_a_whole_keyframe_since_the_latch_is_not_a_cut() {
    for fec in [true, false] {
        let mut rig = Rig::new(58_988);
        if fec {
            rig.run(300, |lr, frame, now| repaired(lr, frame, 1, now));
        } else {
            rig.clean(300);
        }
        rig.lr.on_latch(rig.now(), true, true);
        rig.clean(600);
        // A keyframe after the fence, nothing reported lost for it.
        let (frame, now) = (rig.frame, rig.now());
        rig.lr.on_keyframe_sent(frame, now);
        rig.clean(1_000);
        rig.lr.on_keyframe_request(true, rig.now());
        rig.clean(1_000);
        assert!(rig.changes.is_empty(), "fec {fec}: {:?}", rig.changes);
    }
    // One ordinary frame lost after a whole keyframe is no cut either, but
    // with no keyframe through since the latch it is.
    for keyframe in [true, false] {
        let mut rig = Rig::new(58_988);
        rig.clean(300);
        rig.lr.on_latch(rig.now(), true, true);
        rig.clean(600);
        let (frame, now) = (rig.frame, rig.now());
        if keyframe {
            rig.lr.on_keyframe_sent(frame, now);
        }
        rig.clean(1_000);
        let (frame, now) = (rig.frame, rig.now());
        unrecoverable(&mut rig.lr, frame, now);
        rig.clean(1_000);
        assert_eq!(rig.changes.is_empty(), keyframe, "{:?}", rig.changes);
    }
}

/// 2026-10-24 review: the keyframe that answers a latch is sent about 20 ms
/// after it, inside the fence, and arrives whole. One lost frame or one request
/// 1.5 s later must not halve the rate (main needed three events).
#[test]
fn a_keyframe_sent_inside_the_fence_that_arrived_whole_makes_one_event_no_cut() {
    for fec in [true, false] {
        for request in [false, true] {
            let mut rig = Rig::new(58_988);
            if fec {
                rig.run(300, |lr, frame, now| repaired(lr, frame, 1, now));
            } else {
                rig.clean(300);
            }
            rig.lr.on_latch(rig.now(), true, true);
            rig.clean(20);
            let (frame, now) = (rig.frame, rig.now());
            rig.lr.on_keyframe_sent(frame, now);
            rig.clean(1_500);
            let (frame, now) = (rig.frame, rig.now());
            if request {
                rig.lr.on_keyframe_request(true, now);
            } else {
                unrecoverable(&mut rig.lr, frame, now);
            }
            rig.clean(1_000);
            assert!(
                rig.changes.is_empty(),
                "fec {fec} request {request}: {:?}",
                rig.changes
            );
        }
    }
    // The same keyframe reported lost inside the fence did not arrive: the
    // next event is a cut.
    let mut rig = Rig::new(58_988);
    rig.clean(300);
    rig.lr.on_latch(rig.now(), true, true);
    rig.clean(20);
    let (frame, now) = (rig.frame, rig.now());
    rig.lr.on_keyframe_sent(frame, now);
    rig.clean(100);
    let now = rig.now();
    lost_keyframe(&mut rig.lr, frame, now);
    rig.clean(1_400);
    let (frame, now) = (rig.frame, rig.now());
    unrecoverable(&mut rig.lr, frame, now);
    rig.clean(1_000);
    assert_eq!(rig.cuts(), 1, "{:?}", rig.changes);
}

/// 2026-10-24, `r50-hevc-hi` (59 Mbps into 50 Mbit/s): the keyframe answering
/// the latch was sent inside the fence and never reported (its packets did not
/// get through), so it counted as arrived, and the client's requests 100 ms and
/// 1.3 s after the latch were each "one event". The stream lost its first 5 s.
/// Two events in the watch, 100 ms or more apart, are proof the latch failed
/// whether or not they fall inside the fence; the next admitted event cuts deep.
#[test]
fn two_events_in_the_latch_watch_even_inside_the_fence_cut_deep() {
    for fec in [true, false] {
        let mut rig = Rig::new(58_988);
        if fec {
            rig.run(300, |lr, frame, now| repaired(lr, frame, 1, now));
        } else {
            rig.clean(300);
        }
        let latched = rig.ms;
        rig.lr.on_latch(rig.now(), true, true);
        rig.clean(20);
        let (frame, now) = (rig.frame, rig.now());
        rig.lr.on_keyframe_sent(frame, now);
        // The client asks again 100 ms after the latch (inside the fence), and
        // 1.2 s later.
        rig.clean(80);
        rig.lr.on_keyframe_request(true, rig.now());
        rig.clean(1_100);
        assert!(rig.changes.is_empty(), "fec {fec}: {:?}", rig.changes);
        rig.lr.on_keyframe_request(true, rig.now());
        rig.clean(300);
        assert_eq!(rig.cuts(), 1, "fec {fec}: {:?}", rig.changes);
        let (at, cut) = rig.changes[0];
        assert!(
            at - latched < 1_500,
            "cut {} ms after the latch",
            at - latched
        );
        assert_eq!(cut.reason, "keyframe_requests");
        assert!(cut.to_kbps <= 58_988 / 2, "{cut:?}");
    }
    // The same with lost frames: one inside the fence, one after it.
    let mut rig = Rig::new(58_988);
    rig.clean(300);
    rig.lr.on_latch(rig.now(), true, true);
    rig.clean(20);
    let (frame, now) = (rig.frame, rig.now());
    rig.lr.on_keyframe_sent(frame, now);
    rig.clean(80);
    let (frame, now) = (rig.frame, rig.now());
    unrecoverable(&mut rig.lr, frame, now);
    rig.clean(1_100);
    assert!(rig.changes.is_empty(), "{:?}", rig.changes);
    let (frame, now) = (rig.frame, rig.now());
    unrecoverable(&mut rig.lr, frame, now);
    rig.clean(300);
    assert_eq!(rig.cuts(), 1, "{:?}", rig.changes);
    assert!(rig.changes[0].1.to_kbps <= 58_988 / 2, "{:?}", rig.changes);
}

/// One burst is one event: several frames reported lost inside 100 ms of each
/// other, in the fence or after it, are not two.
#[test]
fn a_burst_inside_100_ms_is_one_event_in_the_latch_watch() {
    let mut rig = Rig::new(58_988);
    rig.run(300, |lr, frame, now| repaired(lr, frame, 1, now));
    rig.lr.on_latch(rig.now(), true, true);
    rig.clean(20);
    let (frame, now) = (rig.frame, rig.now());
    rig.lr.on_keyframe_sent(frame, now);
    rig.clean(1_000);
    // Two lost frames and a request within 60 ms; the fence is over, so they
    // are admitted, but they are one burst (and fewer than three frames).
    let mut left = 2;
    rig.run(PERIOD_MS * 2 + 1, |lr, frame, now| {
        if left > 0 {
            left -= 1;
            unrecoverable(lr, frame, now);
        }
    });
    rig.lr.on_keyframe_request(true, rig.now());
    rig.clean(2_000);
    assert!(rig.changes.is_empty(), "{:?}", rig.changes);
    // A burst of three frames inside the fence is one event too (the fence
    // admits none of them).
    let mut rig = Rig::new(58_988);
    rig.clean(300);
    rig.lr.on_latch(rig.now(), true, true);
    rig.clean(20);
    let (frame, now) = (rig.frame, rig.now());
    rig.lr.on_keyframe_sent(frame, now);
    let mut left = 3;
    rig.run(PERIOD_MS * 3, |lr, frame, now| {
        if left > 0 {
            left -= 1;
            unrecoverable(lr, frame, now);
        }
    });
    rig.clean(1_000);
    assert!(rig.changes.is_empty(), "{:?}", rig.changes);
}

/// The r50 shape end to end: a keyframe sent right after the latch that is never
/// reported lost (nothing of it came through), then frames lost from there on,
/// reported at the rate a client waiting for a keyframe reports (one in 15). The
/// cut must come within 1.5 s of the first loss.
#[test]
fn a_standing_overload_after_a_latch_cuts_within_a_second_and_a_half() {
    for fec in [true, false] {
        let mut rig = Rig::new(58_988);
        if fec {
            rig.run(300, |lr, frame, now| repaired(lr, frame, 1, now));
        } else {
            rig.clean(300);
        }
        let latched = rig.ms;
        rig.lr.on_latch(rig.now(), true, true);
        rig.clean(20);
        let (frame, now) = (rig.frame, rig.now());
        rig.lr.on_keyframe_sent(frame, now);
        rig.clean(80);
        // The client asks again every 1.2 s; one frame in 15 is reported lost.
        let mut n = 0u32;
        rig.run(4_000, |lr, frame, now| {
            n += 1;
            if n.is_multiple_of(70) {
                lr.on_keyframe_request(true, now);
            }
            if n.is_multiple_of(15) {
                unrecoverable(lr, frame, now);
            }
        });
        let first = rig.changes.first().expect("a cut");
        assert!(
            first.0 - latched < 1_600,
            "fec {fec}: cut {} ms after the latch: {:?}",
            first.0 - latched,
            rig.changes
        );
        assert!(first.1.to_kbps <= 58_988 / 2, "{:?}", first.1);
    }
}

/// Review 2026-10-24: a latch that fixed the loss, then residual loss of one
/// unrecoverable frame (or one request) every 2 s inside the 4.5 s watch, is
/// not a failed latch: events more than 1.5 s apart are not a pair.
#[test]
fn residual_loss_of_one_event_every_two_seconds_after_a_latch_is_not_a_cut() {
    for request in [false, true] {
        let mut rig = Rig::new(58_988);
        rig.run(300, |lr, frame, now| repaired(lr, frame, 1, now));
        rig.lr.on_latch(rig.now(), true, true);
        rig.clean(20);
        let (frame, now) = (rig.frame, rig.now());
        rig.lr.on_keyframe_sent(frame, now);
        rig.clean(980);
        for _ in 0..3 {
            let (frame, now) = (rig.frame, rig.now());
            if request {
                rig.lr.on_keyframe_request(true, now);
            } else {
                unrecoverable(&mut rig.lr, frame, now);
            }
            rig.clean(2_000);
        }
        assert!(
            rig.changes.is_empty(),
            "request {request}: {:?}",
            rig.changes
        );
    }
}

#[test]
fn a_stray_unrecoverable_frame_long_after_a_latch_is_not_a_cut() {
    let mut rig = Rig::new(40_000);
    rig.clean(1_000);
    rig.lr.on_latch(rig.now(), true, true);
    // The fence and the watch both over: one frame is no longer a cut.
    rig.clean(500 + LATCH_WATCH.as_millis() as u64 + 500);
    let mut left = 1;
    rig.run(PERIOD_MS * 2, |lr, frame, now| {
        if left > 0 {
            left -= 1;
            unrecoverable(lr, frame, now);
        }
    });
    rig.clean(2_000);
    assert!(rig.changes.is_empty(), "{:?}", rig.changes);
}

#[test]
fn a_latch_that_is_not_new_or_was_not_fast_starts_no_watch() {
    let mut rig = Rig::new(40_000);
    rig.clean(1_000);
    rig.lr.on_latch(rig.now(), false, true);
    rig.lr.on_latch(rig.now(), true, false);
    rig.clean(1_000);
    let mut left = 1;
    rig.run(PERIOD_MS * 2, |lr, frame, now| {
        if left > 0 {
            left -= 1;
            unrecoverable(lr, frame, now);
        }
    });
    rig.clean(2_000);
    assert!(rig.changes.is_empty(), "{:?}", rig.changes);
}

#[test]
fn a_lifted_latch_fences_only_when_the_pace_gets_faster() {
    let mut rig = Rig::new(40_000);
    rig.clean(2_000);
    // Back at a rate no faster than the Wi-Fi rate: nothing changed.
    rig.lr.on_unlatch(rig.now(), false);
    assert!(rig.lr.fence_at().is_none());
    // Back at the faster rate: one fence, so reports from before it decide
    // nothing.
    rig.lr.on_unlatch(rig.now(), true);
    assert!(rig.lr.fence_at().is_some());
    let late = rig.frame;
    let now = rig.now();
    unrecoverable(&mut rig.lr, late, now);
    rig.clean(2_000);
    assert!(rig.changes.is_empty(), "{:?}", rig.changes);
}

#[test]
fn the_snapshot_reports_state_and_counters() {
    let mut rig = Rig::new(40_000);
    let before = rig.lr.snapshot(rig.now());
    assert_eq!(before["state"], "full");
    assert_eq!(before["ceiling_kbps"], 40_000);
    rig.clean(2_000);
    rig.lose_three();
    rig.clean(500);
    let after = rig.lr.snapshot(rig.now());
    assert_eq!(after["state"], "limited");
    assert_eq!(after["reason"], "unrecoverable_frames");
    assert_eq!(after["steps_down"], 1);
    assert_eq!(after["current_kbps"], rig.kbps);
}
