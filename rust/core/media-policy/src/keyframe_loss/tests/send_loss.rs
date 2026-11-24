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
