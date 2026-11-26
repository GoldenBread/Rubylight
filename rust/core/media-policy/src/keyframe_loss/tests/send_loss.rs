//! Tests for host send-loss latching, refusal tracking, and guard-off behavior.

use super::*;

#[test]
fn frames_lost_on_send_slow_the_stream_and_are_not_counted_twice() {
    // Send-buffer pressure (error 10055 at 800 Mbps on a Hyper-V route):
    // the socket refuses the rest of ordinary frames. Three within two
    // seconds slow the stream, as three the client reports would.
    let now = Instant::now();
    let mut loss = running(100);
    assert_eq!(loss.abandoned(40, PERIOD, now), None);
    assert_eq!(loss.abandoned(41, PERIOD, now), None);
    let latched = loss.abandoned(42, PERIOD, now).unwrap();
    assert_eq!(
        latched.reason,
        "three recent video frames were lost on send within two seconds"
    );
    // Only the host saw it: provisional, and still slow.
    assert_eq!(latched.kind, LatchKind::Send);
    assert_eq!(loss.bps(FAST, SAFE), SAFE);
    // The client reporting the same frames adds nothing to count.
    let mut loss = running(100);
    for frame in [40, 41] {
        assert_eq!(loss.abandoned(frame, PERIOD, now), None);
    }
    for frame in [40, 41] {
        assert_eq!(loss.unrecoverable(frame, 100, PERIOD, now), None);
    }
    assert!(loss.take_ignored().is_some());
    assert!(!loss.latched());
    // Frames the host sent whole and the client lost count with them.
    assert!(loss.unrecoverable(60, 100, PERIOD, now).is_some());
}

#[test]
fn a_keyframe_lost_on_send_slows_the_stream_at_once() {
    let now = Instant::now();
    let mut loss = running(100);
    loss.sending_at(101, true, now);
    let latched = loss.abandoned(101, PERIOD, now).unwrap();
    assert_eq!(latched.reason, "a keyframe was lost on send");
    assert!(latched.detail.contains("101"), "{latched:?}");
    assert_eq!(latched.kind, LatchKind::Send);
    assert_eq!(loss.bps(FAST, SAFE), SAFE);
    assert!(loss.sustained_at().is_none());
    // Nothing lost on send is counted once the guard is off.
    let mut off = KeyframeLoss::new(false);
    off.sending_at(1, true, now);
    assert_eq!(off.abandoned(1, PERIOD, now), None);
    assert!(!off.latched());
}

/// A stream long past its start whose socket refused frames 101-103 at
/// `t0` (#59: `UDP send failed code=10035` while a game started).
fn send_latched(t0: Instant) -> KeyframeLoss {
    let mut loss = running(100);
    for frame in 101..=103 {
        loss.sending_at(frame, false, t0);
    }
    assert_eq!(loss.abandoned(101, PERIOD, t0), None);
    assert_eq!(loss.abandoned(102, PERIOD, t0), None);
    let latched = loss.abandoned(103, PERIOD, t0).unwrap();
    assert_eq!(latched.kind, LatchKind::Send);
    assert!(!latched.confirmed);
    loss
}

#[test]
fn loss_only_the_host_saw_is_provisional_and_lifts() {
    let t0 = Instant::now();
    let mut loss = send_latched(t0);
    assert_eq!(loss.kind(), Some(LatchKind::Send));
    assert_eq!(loss.bps(FAST, SAFE), SAFE);
    assert!(loss.sustained_at().is_none());
    assert!(loss.ever_latched());
    let frame = flow(&mut loss, 104, t0, t0 + Duration::from_millis(9_900));
    assert!(loss.latched());
    assert!(loss.take_lifted().is_none());
    flow(
        &mut loss,
        frame,
        t0 + Duration::from_millis(9_900),
        t0 + Duration::from_millis(10_300),
    );
    assert!(!loss.latched());
    assert_eq!(loss.bps(FAST, SAFE), FAST);
    assert!(loss.sustained_at().is_none());
    let note = loss.take_lifted().unwrap();
    assert!(note.contains("host's socket"), "{note}");
}

#[test]
fn more_send_loss_extends_a_send_latch_and_never_confirms_it() {
    let t0 = Instant::now();
    let mut loss = send_latched(t0);
    let mut frame = flow(&mut loss, 104, t0, t0 + Duration::from_secs(5));
    // Three more frames, and a keyframe, refused five seconds in.
    let again = t0 + Duration::from_secs(5);
    for _ in 0..3 {
        loss.sending_at(frame, false, again);
        assert_eq!(loss.abandoned(frame, PERIOD, again), None);
        frame += 1;
    }
    loss.sending_at(frame, true, again);
    assert_eq!(loss.abandoned(frame, PERIOD, again), None);
    assert_eq!(loss.kind(), Some(LatchKind::Send));
    assert!(loss.sustained_at().is_none());
    // Clean ten seconds counts from the last of them.
    frame = flow(
        &mut loss,
        frame + 1,
        again,
        t0 + Duration::from_millis(14_900),
    );
    assert!(loss.latched());
    flow(
        &mut loss,
        frame,
        t0 + Duration::from_millis(14_900),
        t0 + Duration::from_millis(15_300),
    );
    assert!(!loss.latched());
    assert!(loss.sustained_at().is_none());
}

#[test]
fn a_client_report_confirms_a_send_latch_for_memory() {
    let t0 = Instant::now();
    // An ordinary frame the client lost right after: it takes the oldest
    // of the three slots the host's losses filled, and confirms.
    let mut loss = send_latched(t0);
    loss.sending_at(104, false, t0);
    let at = t0 + Duration::from_millis(100);
    let confirmed = loss.unrecoverable(104, 104, PERIOD, at).unwrap();
    assert!(confirmed.confirmed);
    assert_eq!(confirmed.from, Some(LatchKind::Send));
    assert_eq!(confirmed.kind, LatchKind::Sustained);
    assert_eq!(loss.sustained_at(), Some(at));
    // Or a keyframe it could not rebuild.
    let mut loss = send_latched(t0);
    loss.sending_at(104, true, t0);
    let confirmed = loss.unrecoverable(104, 104, PERIOD, at).unwrap();
    assert!(confirmed.confirmed);
    assert_eq!(confirmed.from, Some(LatchKind::Send));
    assert_eq!(loss.sustained_at(), Some(at));
    flow(&mut loss, 105, at, at + Duration::from_secs(30));
    assert!(loss.latched());
    // A report of a frame the host refused is not the client's loss.
    let mut loss = send_latched(t0);
    assert_eq!(loss.unrecoverable(103, 103, PERIOD, at), None);
    assert!(loss.sustained_at().is_none());
}

#[test]
fn a_send_latch_after_a_lift_holds_for_the_stream_but_is_not_remembered() {
    let t0 = Instant::now();
    let mut loss = send_latched(t0);
    let mut frame = flow(&mut loss, 104, t0, t0 + Duration::from_millis(10_300));
    assert!(!loss.latched());
    let again = t0 + Duration::from_secs(20);
    for _ in 0..2 {
        loss.sending_at(frame, false, again);
        assert_eq!(loss.abandoned(frame, PERIOD, again), None);
        frame += 1;
    }
    loss.sending_at(frame, false, again);
    let latched = loss.abandoned(frame, PERIOD, again).unwrap();
    assert_eq!(latched.kind, LatchKind::Sustained);
    assert!(!latched.confirmed);
    assert!(loss.sustained_at().is_none());
    frame = flow(&mut loss, frame + 1, again, again + Duration::from_secs(30));
    assert!(loss.latched());
    assert!(loss.sustained_at().is_none());
    // The client's report is what makes it worth remembering.
    let late = again + Duration::from_secs(30);
    loss.sending_at(frame, true, late);
    let confirmed = loss.unrecoverable(frame, frame, PERIOD, late).unwrap();
    assert!(confirmed.confirmed);
    assert_eq!(confirmed.from, Some(LatchKind::Sustained));
    assert_eq!(loss.sustained_at(), Some(late));
    assert_eq!(loss.unrecoverable(frame, frame, PERIOD, late), None);
}

/// After the host's socket refused frames, the client asks for a
/// keyframe, the socket refuses recovery keyframe 104 too, and the client
/// asks again 55 ms later. It never got 104: the host's loss.
fn asked_again_for_a_refused_keyframe(loss: &mut KeyframeLoss, t: Instant) -> Option<Latched> {
    assert_eq!(loss.requested(t), None);
    let sent = t + Duration::from_millis(5);
    loss.sending_at(104, true, sent);
    loss.abandoned(104, PERIOD, sent);
    for frame in 105..=107 {
        loss.sending_at(frame, false, sent + PERIOD);
    }
    loss.requested(t + Duration::from_millis(55))
}

#[test]
fn asking_again_for_a_keyframe_the_host_refused_does_not_confirm_a_send_latch() {
    let t0 = Instant::now();
    let mut loss = send_latched(t0);
    let t = t0 + Duration::from_millis(20);
    assert_eq!(asked_again_for_a_refused_keyframe(&mut loss, t), None);
    assert_eq!(loss.kind(), Some(LatchKind::Send));
    assert!(loss.sustained_at().is_none());
    // It lifts 10 s after the last of it, like any send latch.
    let frame = flow(&mut loss, 108, t, t + Duration::from_millis(10_300));
    assert!(!loss.latched(), "{frame}");
    assert!(loss.sustained_at().is_none());
}

#[test]
fn a_refused_keyframe_stays_the_host_s_loss_past_the_abandoned_ring() {
    // A 240 fps stream on a flapping adapter: after refused recovery
    // keyframe 104, nine more frames are given up, more than the eight
    // the abandoned ring holds, and the client asks again within
    // DECODE_QUEUE frames of 104.
    let t0 = Instant::now();
    let mut loss = send_latched(t0);
    let t = t0 + Duration::from_millis(20);
    assert_eq!(loss.requested(t), None);
    let sent = t + Duration::from_millis(5);
    loss.sending_at(104, true, sent);
    loss.abandoned(104, PERIOD, sent);
    for frame in 105..=113 {
        loss.sending_at(frame, false, sent);
        loss.abandoned(frame, PERIOD, sent);
    }
    assert!(!loss.is_abandoned(104));
    assert_eq!(loss.requested(t + Duration::from_millis(60)), None);
    assert_eq!(loss.kind(), Some(LatchKind::Send));
    assert!(loss.sustained_at().is_none());
    // A newer keyframe that goes out whole is the one asked for again.
    let mut loss = send_latched(t0);
    assert_eq!(loss.requested(t), None);
    loss.sending_at(104, true, sent);
    loss.abandoned(104, PERIOD, sent);
    assert_eq!(loss.requested(t + Duration::from_millis(60)), None);
    loss.sending_at(105, true, t + Duration::from_millis(65));
    let confirmed = loss.requested(t + Duration::from_millis(110)).unwrap();
    assert!(confirmed.confirmed);
    assert!(loss.sustained_at().is_some());
}

#[test]
fn asking_again_for_a_keyframe_the_host_refused_does_not_renew_a_remembered_route() {
    let t = Instant::now();
    let mut loss = KeyframeLoss::remembered("remembered from the previous session");
    loss.sending_at(1, true, started_long_ago());
    for frame in 2..=103 {
        loss.sending(frame, false);
    }
    assert_eq!(asked_again_for_a_refused_keyframe(&mut loss, t), None);
    assert!(loss.latched());
    assert!(loss.sustained_at().is_none());
    // A keyframe that did reach the client still renews it.
    let mut loss = KeyframeLoss::remembered("remembered from the previous session");
    loss.sending_at(1, true, started_long_ago());
    for frame in 2..=103 {
        loss.sending(frame, false);
    }
    assert_eq!(loss.requested(t), None);
    loss.sending_at(104, true, t + Duration::from_millis(5));
    let renewed = loss.requested(t + Duration::from_millis(55)).unwrap();
    assert_eq!(renewed.kind, LatchKind::Sustained);
    assert!(loss.sustained_at().is_some());
}

#[test]
fn frames_abandoned_on_send_are_remembered_with_the_guard_on_or_off() {
    let now = Instant::now();
    for enabled in [true, false] {
        let mut loss = KeyframeLoss::new(enabled);
        assert!(!loss.is_abandoned(9));
        loss.abandoned(9, PERIOD, now);
        assert!(loss.is_abandoned(9), "guard {enabled}");
        assert!(!loss.is_abandoned(10), "guard {enabled}");
    }
}

#[test]
fn the_recovery_keyframe_after_a_loss_on_send_opens_no_window() {
    // The host answers frames its socket refused with a keyframe. That
    // keyframe is recovery, not a change of the stream: the losses around
    // it still count.
    let start = Instant::now();
    let mut loss = running(100);
    loss.loss_keyframe();
    loss.sending_at(101, true, start);
    for frame in 102..=140 {
        loss.sending_at(frame, false, start);
    }
    for frame in [110, 120] {
        assert_eq!(loss.unrecoverable(frame, 140, PERIOD, start), None);
    }
    assert!(loss.unrecoverable(130, 140, PERIOD, start).is_some());
    assert!(loss.take_ignored().is_none());
}

#[test]
fn a_request_just_before_the_hosts_keyframe_does_not_make_it_the_one_asked_for() {
    // The client asks a moment before the host recreates the encoder; the
    // keyframe that follows is the host's, whatever was pending.
    let start = Instant::now();
    let mut loss = running(100);
    assert_eq!(loss.requested(start), None);
    loss.host_keyframe();
    loss.sending_at(101, true, start + Duration::from_millis(10));
    loss.sending_at(102, false, start + Duration::from_millis(20));
    assert_eq!(loss.requested(start + Duration::from_millis(60)), None);
    assert!(!loss.latched());
    assert!(loss.take_ignored().is_some());
    // The same request answered by an ordinary keyframe is still the one
    // a repeat asks again for.
    let mut loss = running(100);
    assert_eq!(loss.requested(start), None);
    loss.sending_at(101, true, start + Duration::from_millis(10));
    loss.sending_at(102, false, start + Duration::from_millis(20));
    assert!(loss.requested(start + Duration::from_millis(60)).is_some());
}

#[test]
fn a_request_a_codec_cannot_answer_leaves_nothing_pending() {
    // An intra-only codec makes no keyframe for a request; the next
    // keyframe (the host's own) must not be taken as its answer.
    let start = Instant::now();
    let mut loss = running(100);
    assert_eq!(loss.requested(start), None);
    loss.unanswerable();
    loss.sending_at(101, true, start + Duration::from_millis(10));
    // Taken as an answer it would arm the repeat rule; as the encoder's
    // own it opens the window, so the repeat is not evidence.
    loss.sending_at(102, false, start + Duration::from_millis(20));
    assert_eq!(loss.requested(start + Duration::from_millis(60)), None);
    assert!(!loss.latched());
}

#[test]
fn about_one_loss_a_second_with_recovery_keyframes_still_slows_the_stream() {
    // A route that loses a frame about every 900 ms: each loss is answered with a
    // reference-invalidation confirm keyframe. If those opened windows
    // the next loss would always land inside one and nothing would count.
    let start = Instant::now();
    let mut loss = running(100);
    let mut latched = None;
    for (i, frame) in [110_u32, 190, 270].into_iter().enumerate() {
        let at = start + Duration::from_millis(900 * i as u64);
        loss.loss_keyframe();
        loss.sending_at(frame + 5, true, at);
        latched = loss.unrecoverable(frame, frame + 6, PERIOD, at);
    }
    assert!(latched.is_some());
    assert_eq!(loss.bps(FAST, SAFE), SAFE);
}

#[test]
fn nothing_is_ignored_or_logged_while_the_guard_is_off() {
    let start = Instant::now();
    let mut loss = KeyframeLoss::new(false);
    loss.sending_at(1, true, start);
    loss.sending_at(2, false, start);
    assert_eq!(loss.requested(start), None);
    assert_eq!(loss.unrecoverable(2, 3, PERIOD, start), None);
    assert!(loss.take_ignored().is_none());
}

#[test]
fn the_ignored_note_names_the_window_that_applied() {
    let start = Instant::now();
    let mut loss = KeyframeLoss::new(true);
    loss.sending_at(1, true, start);
    assert_eq!(loss.requested(start + Duration::from_millis(100)), None);
    let note = loss.take_ignored().unwrap();
    assert!(note.contains("500 ms"), "{note}");
    let mut loss = running(100);
    loss.host_keyframe();
    loss.sending_at(101, true, start);
    assert_eq!(loss.requested(start + Duration::from_millis(100)), None);
    let note = loss.take_ignored().unwrap();
    assert!(note.contains("1000 ms"), "{note}");
}

#[test]
fn route_abandoned_preserves_partial_frames_and_keyframe_slots_across_long_outage() {
    let start = Instant::now();
    let mut loss = running(100);
    // Partially-sent P-frames refused before the route drops stay in `abandoned`.
    assert_eq!(loss.abandoned(98, PERIOD, start), None);
    assert_eq!(loss.abandoned(99, PERIOD, start), None);
    // Keyframe 101 starts (batch 0 hits the socket) and is dropped due to route outage.
    loss.sending_at(101, true, start + Duration::from_millis(16));
    loss.route_abandoned(101, true);
    // More than 8 completely-unsent P-frames fail on packet 0 during the outage.
    for frame in 102..=121 {
        loss.sending_at(frame, false, start + Duration::from_millis(32));
        loss.route_abandoned(frame, false);
    }
    // Recovery keyframe starts before delayed 0x5502 reports arrive for earlier frames.
    loss.host_keyframe();
    loss.sending_at(122, true, start + Duration::from_millis(400));

    assert!(loss.is_abandoned(98));
    assert!(loss.is_abandoned(99));
    for frame in 101..=121 {
        assert!(loss.is_abandoned(frame), "frame {frame}");
        assert!(
            !loss.network_loss(frame, start + Duration::from_millis(450)),
            "frame {frame}"
        );
        assert_eq!(
            loss.unrecoverable(frame, 122, PERIOD, start + Duration::from_millis(450)),
            None,
            "frame {frame}"
        );
    }
    assert!(!loss.latched());
    assert!(!loss.ever_latched());
}

#[test]
fn route_abandoned_keyframe_and_pending_host_keyframe_suppress_network_request() {
    let start = Instant::now();
    let mut loss = running(100);
    assert!(loss.network_request(start));
    assert_eq!(loss.requested(start), None);

    loss.sending_at(101, true, start + Duration::from_millis(10));
    loss.route_abandoned(101, true);
    for frame in 102..=121 {
        loss.sending_at(frame, false, start + Duration::from_millis(20));
        loss.route_abandoned(frame, false);
    }
    // Past both REPEAT (500 ms) and DECODE_QUEUE (15), across multiple repeats.
    for offset in [700, 750, 800] {
        let now = start + Duration::from_millis(offset);
        assert!(!loss.network_request(now), "offset={offset}");
        assert_eq!(loss.requested(now), None, "offset={offset}");
    }
    // Once the route recovers on a P-frame and queues a host keyframe, requests arriving
    // before the replacement keyframe begins sending remain suppressed.
    loss.host_keyframe();
    let before_replacement = start + Duration::from_millis(820);
    assert!(!loss.network_request(before_replacement));
    assert_eq!(loss.requested(before_replacement), None);

    loss.sending_at(122, true, start + Duration::from_millis(830));
    assert!(!loss.network_request(start + Duration::from_millis(850)));
    assert!(!loss.latched());
    assert!(!loss.ever_latched());
}

#[test]
fn pending_host_keyframe_prevents_repeat_request_latch_even_after_clean_keyframe() {
    let start = Instant::now();
    let mut loss = running(100);
    assert!(loss.network_request(start));
    assert_eq!(loss.requested(start), None);

    loss.sending_at(101, true, start + Duration::from_millis(10));
    loss.host_keyframe();
    let repeat_at = start + Duration::from_millis(60);
    assert!(!loss.network_request(repeat_at));
    assert_eq!(loss.requested(repeat_at), None);
    assert!(!loss.latched());
    assert!(!loss.ever_latched());
    assert!(loss.take_ignored().is_some());
}

#[test]
fn p_frame_only_route_outage_suppresses_requests_and_opens_host_change_window() {
    let start = Instant::now();
    let mut loss = running(100);
    assert!(loss.network_request(start));
    assert_eq!(loss.requested(start), None);

    // Route outage drops only P-frames (batch 0 of 101 reached socket before WSAENETUNREACH).
    loss.sending_at(101, false, start + Duration::from_millis(16));
    loss.route_abandoned(101, true);
    for frame in 102..=130 {
        loss.sending_at(frame, false, start + Duration::from_millis(32));
        loss.route_abandoned(frame, false);
    }

    // Requests arriving during the P-frame outage stay ignored without latching or advancing
    // repeat state.
    for offset in [100, 700] {
        let now = start + Duration::from_millis(offset);
        assert!(!loss.network_request(now), "offset={offset}");
        assert_eq!(loss.requested(now), None, "offset={offset}");
    }

    // Client request set `session.idr = true` without `host_keyframe()`: sending keyframe 131
    // clears the active route-outage flag and opens HOST_CHANGE_WINDOW so an in-flight repeat
    // does not latch KeyframeLoss or charge E4.
    loss.sending_at(131, true, start + Duration::from_millis(750));
    loss.sending_at(132, false, start + Duration::from_millis(766));
    let in_flight = start + Duration::from_millis(780);
    assert!(!loss.network_request(in_flight));
    assert_eq!(loss.requested(in_flight), None);
    assert!(!loss.latched());
    assert!(!loss.ever_latched());

    // If recovery keyframe 131 itself fails on route loss, the unresolved outage flag returns
    // and keeps requests suppressed even past HOST_CHANGE_WINDOW until keyframe 133 starts.
    loss.route_abandoned(131, false);
    let past_window = start + Duration::from_millis(1_900);
    assert!(!loss.network_request(past_window));
    assert_eq!(loss.requested(past_window), None);
    loss.sending_at(133, true, start + Duration::from_millis(1_920));
    assert!(!loss.network_request(start + Duration::from_millis(1_950)));
}

#[test]
fn route_outage_does_not_lift_a_provisional_latch_mid_outage_or_on_recovery() {
    let t0 = Instant::now();
    let mut loss = send_latched(t0);
    assert_eq!(loss.kind(), Some(LatchKind::Send));
    // Send 115 clean frames over the first 2 seconds (below LIFT_FRAMES = 120 and PROVISIONAL_LIFT = 10 s).
    for i in 1..=115u32 {
        loss.sending_at(
            200 + i,
            false,
            t0 + Duration::from_millis(16 * u64::from(i)),
        );
    }
    assert!(loss.latched());

    // Hotspot/Wi-Fi route drops for 15 seconds (600 frames >= LIFT_FRAMES and >= PROVISIONAL_LIFT).
    let outage_start = t0 + Duration::from_secs(2);
    for i in 0..600u32 {
        let frame = 400 + i;
        let at = outage_start + Duration::from_millis(25 * u64::from(i));
        loss.sending_at(frame, false, at);
        loss.route_abandoned(frame, false);
        assert!(loss.latched(), "lifted mid-outage at frame {frame}");
        assert_eq!(loss.kind(), Some(LatchKind::Send));
        assert!(loss.take_lifted().is_none());
    }

    // Post-outage recovery keyframe leaves while the provisional latch is still held and clean clock restarted.
    let recovery_at = outage_start + Duration::from_secs(15);
    loss.sending_at(1_000, true, recovery_at);
    assert!(loss.latched(), "lifted immediately on recovery keyframe");
    assert_eq!(loss.kind(), Some(LatchKind::Send));
    assert!(loss.take_lifted().is_none());

    // Wait less than PROVISIONAL_LIFT after the recovery keyframe -> still held, then past PROVISIONAL_LIFT clean -> lifts.
    for i in 1..=119u32 {
        loss.sending_at(
            1_000 + i,
            false,
            recovery_at + Duration::from_millis(16 * u64::from(i)),
        );
    }
    assert!(loss.latched());
    loss.sending_at(
        1_200,
        false,
        recovery_at + PROVISIONAL_LIFT - Duration::from_millis(1),
    );
    assert!(loss.latched());
    loss.sending_at(1_201, false, recovery_at + PROVISIONAL_LIFT);
    assert!(!loss.latched());
    assert!(loss.take_lifted().is_some());
}

#[test]
fn idle_route_outage_does_not_lift_provisional_latch_on_recovery_keyframe() {
    let t0 = Instant::now();
    let mut loss = send_latched(t0);
    assert_eq!(loss.kind(), Some(LatchKind::Send));
    // Build up 119 clean frames (< PROVISIONAL_LIFT = 10 s), leaving frames_since_latch one shy of 120.
    for i in 1..=119u32 {
        loss.sending_at(
            200 + i,
            false,
            t0 + Duration::from_millis(16 * u64::from(i)),
        );
    }
    assert!(loss.latched());

    // One route-abandoned P-frame, then an idle >= 10s gap (e.g. static screen) with no frames encoded.
    let outage_at = t0 + Duration::from_secs(2);
    loss.sending_at(400, false, outage_at);
    loss.route_abandoned(400, false);
    assert!(loss.latched());

    let recovery_at = outage_at + Duration::from_secs(12);
    loss.sending_at(500, true, recovery_at);
    assert!(
        loss.latched(),
        "lifted immediately on recovery keyframe after idle outage"
    );
    assert_eq!(loss.kind(), Some(LatchKind::Send));
    assert!(loss.take_lifted().is_none());

    // Full PROVISIONAL_LIFT of clean traffic after recovery is still required before lifting.
    for i in 1..=119u32 {
        loss.sending_at(
            500 + i,
            false,
            recovery_at + Duration::from_millis(16 * u64::from(i)),
        );
    }
    assert!(loss.latched());
    loss.sending_at(
        700,
        false,
        recovery_at + PROVISIONAL_LIFT - Duration::from_millis(1),
    );
    assert!(loss.latched());
    loss.sending_at(701, false, recovery_at + PROVISIONAL_LIFT);
    assert!(!loss.latched());
    assert!(loss.take_lifted().is_some());
}

#[test]
fn gapped_route_outage_coalesces_skipped_frames_but_splits_across_sent_frames() {
    let t0 = Instant::now();
    let mut loss = running(29);
    // Partial P-frame 30 sends batch 0 before failing with route unavailable.
    loss.sending_at(30, false, t0);
    loss.route_abandoned(30, true);
    // Queued frames skipped in admission/delivery leave >4 gaps in wire numbers during one outage.
    for (i, frame) in [32u32, 34, 36, 38, 40, 45].into_iter().enumerate() {
        let at = t0 + Duration::from_millis(16 * (i as u64 + 1));
        loss.sending_at(frame, false, at);
        loss.route_abandoned(frame, false);
    }
    assert!(loss.is_route_abandoned(30));
    assert!(loss.is_route_abandoned(45));
    assert_eq!(
        loss.unrecoverable(30, 45, PERIOD, t0 + Duration::from_millis(200)),
        None
    );

    // A frame sent cleanly without route_abandoned splits the outage even if a later gap occurs.
    loss.sending_at(46, false, t0 + Duration::from_millis(220));
    loss.sending_at(48, false, t0 + Duration::from_millis(250));
    loss.route_abandoned(48, false);
    assert!(!loss.is_route_abandoned(46));
    assert!(loss.is_route_abandoned(30));
    assert!(loss.is_route_abandoned(48));

    // Coalescing across skipped wire numbers also handles u32::MAX -> 0 wrap-around.
    let mut wrapped = running(1);
    wrapped.sending_at(u32::MAX - 1, false, t0);
    wrapped.route_abandoned(u32::MAX - 1, true);
    wrapped.sending_at(u32::MAX, false, t0 + Duration::from_millis(16));
    wrapped.route_abandoned(u32::MAX, false);
    wrapped.sending_at(1, false, t0 + Duration::from_millis(32));
    wrapped.route_abandoned(1, false);
    assert!(wrapped.is_route_abandoned(u32::MAX - 1));
    assert!(wrapped.is_route_abandoned(u32::MAX));
    assert!(wrapped.is_route_abandoned(1));
    assert!(!wrapped.is_route_abandoned(2));
}
