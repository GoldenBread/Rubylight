//! Tests for startup provisional latches, decode-queue filtering, and recovery FEC.

use super::*;

/// The start of a stream as the host log shows it (a 120 fps stream with
/// a remembered latch, twice): the client's first request comes before
/// the first keyframe, wire frame 1, is sent; it asks again 112 ms later,
/// the host answers with frame 11, and the client has shown nothing yet.
fn startup(loss: &mut KeyframeLoss, t0: Instant) {
    assert_eq!(loss.requested(t0 - Duration::from_millis(5)), None);
    loss.sending_at(1, true, t0);
    for frame in 2..=10 {
        loss.sending_at(frame, false, t0 + PERIOD * (frame - 1));
    }
    assert_eq!(loss.requested(t0 + Duration::from_millis(112)), None);
    loss.sending_at(11, true, t0 + Duration::from_millis(116));
    for frame in 12..=20 {
        loss.sending_at(
            frame,
            false,
            t0 + Duration::from_millis(116) + PERIOD * (frame - 11),
        );
    }
}

#[test]
fn the_clients_requests_while_the_first_keyframe_arrives_do_not_slow_the_stream() {
    let t0 = Instant::now();
    let mut loss = KeyframeLoss::new(true);
    startup(&mut loss, t0);
    // A third request and an ordinary-frame report in the same moment
    // are the same start, not evidence.
    assert_eq!(loss.requested(t0 + Duration::from_millis(300)), None);
    assert_eq!(
        loss.unrecoverable(12, 20, PERIOD, t0 + Duration::from_millis(300)),
        None
    );
    assert!(!loss.latched());
    assert_eq!(loss.bps(FAST, SAFE), FAST);
    // Past the window the first request of a new pair is not a pair.
    assert_eq!(loss.requested(t0 + Duration::from_millis(700)), None);
    assert!(!loss.latched());
}

#[test]
fn a_keyframe_lost_right_after_the_start_still_slows_the_stream() {
    let t0 = Instant::now();
    let mut loss = KeyframeLoss::new(true);
    startup(&mut loss, t0);
    // The client reports the keyframe it was sent incomplete: that
    // counts inside the startup window as well.
    assert!(
        loss.unrecoverable(11, 20, PERIOD, t0 + Duration::from_millis(200))
            .is_some()
    );
    assert_eq!(loss.bps(FAST, SAFE), SAFE);
    // Or, past the window, it asks, gets a keyframe and asks again.
    let mut loss = KeyframeLoss::new(true);
    startup(&mut loss, t0);
    let t = t0 + Duration::from_millis(600);
    assert_eq!(loss.requested(t), None);
    loss.sending_at(30, true, t + PERIOD);
    loss.sending_at(31, false, t + PERIOD * 2);
    assert!(loss.requested(t + Duration::from_millis(60)).is_some());
    assert_eq!(loss.bps(FAST, SAFE), SAFE);
}
/// The stream's first keyframe (wire frame 1) lost at the very start.
fn lost_first_keyframe(t0: Instant) -> (KeyframeLoss, Latched) {
    let mut loss = KeyframeLoss::new(true);
    loss.sending_at(1, true, t0);
    loss.sending_at(2, false, t0 + PERIOD);
    let latched = loss
        .unrecoverable(1, 2, PERIOD, t0 + Duration::from_millis(100))
        .unwrap();
    (loss, latched)
}

#[test]
fn a_lost_first_keyframe_is_provisional_and_lifts_after_ten_clean_seconds() {
    let t0 = Instant::now();
    let (mut loss, latched) = lost_first_keyframe(t0);
    assert_eq!(latched.kind, LatchKind::Startup);
    assert!(!latched.confirmed);
    assert_eq!(loss.kind(), Some(LatchKind::Startup));
    assert_eq!(loss.bps(FAST, SAFE), SAFE);
    // Nothing to remember: the next session starts at the route rate.
    assert!(loss.ever_latched());
    assert!(loss.sustained_at().is_none());
    // Still held just short of ten seconds.
    let (a, b) = (
        t0 + Duration::from_millis(9_900),
        t0 + Duration::from_millis(10_300),
    );
    let frame = flow(&mut loss, 3, t0 + PERIOD * 2, a);
    assert_eq!(loss.bps(FAST, SAFE), SAFE);
    assert!(loss.take_lifted().is_none());
    // Lifted ten seconds after the latch, once, with a note for the log.
    flow(&mut loss, frame, a, b);
    assert_eq!(loss.bps(FAST, SAFE), FAST);
    assert!(!loss.latched());
    assert!(loss.observed_at().is_none());
    assert!(loss.sustained_at().is_none());
    assert!(loss.ever_latched());
    let note = loss.take_lifted().unwrap();
    assert!(note.contains("first keyframe"), "{note}");
    assert!(loss.take_lifted().is_none());
}

#[test]
fn a_provisional_latch_waits_for_busy_video_and_quiet_reports() {
    let t0 = Instant::now();
    let (mut loss, _) = lost_first_keyframe(t0);
    // Ten seconds, but only a trickle of frames: not busy video.
    loss.sending_at(3, false, t0 + Duration::from_secs(11));
    assert!(loss.latched());
    // An isolated ordinary-frame loss within the window keeps it held.
    let late = t0 + Duration::from_secs(12);
    assert_eq!(loss.unrecoverable(500, 600, PERIOD, late), None);
    for frame in 4..=200 {
        loss.sending_at(frame, false, late + Duration::from_millis(u64::from(frame)));
    }
    assert!(loss.latched());
    // The loss restarts the count: ten seconds after it, not after the
    // latch, the latch lifts.
    loss.sending_at(201, false, late + Duration::from_millis(9_900));
    assert!(loss.latched());
    loss.sending_at(202, false, late + Duration::from_millis(10_100));
    assert!(!loss.latched());
}

#[test]
fn a_provisional_latch_is_clean_for_ten_seconds_after_the_last_counted_loss() {
    let t0 = Instant::now();
    let (mut loss, _) = lost_first_keyframe(t0);
    let mut frame = flow(&mut loss, 3, t0 + PERIOD * 2, t0 + Duration::from_secs(6));
    // A loss after the latch (frames sent after it) at 6 s.
    let lost_at = t0 + Duration::from_secs(6);
    loss.sending_at(frame, false, lost_at);
    assert_eq!(loss.unrecoverable(frame, frame, PERIOD, lost_at), None);
    frame = flow(
        &mut loss,
        frame + 1,
        lost_at,
        t0 + Duration::from_millis(10_500),
    );
    // Ten seconds after the latch, but only four and a half after the loss.
    assert!(loss.latched());
    frame = flow(
        &mut loss,
        frame,
        t0 + Duration::from_millis(10_500),
        t0 + Duration::from_millis(15_900),
    );
    assert!(loss.latched());
    flow(
        &mut loss,
        frame,
        t0 + Duration::from_millis(15_900),
        t0 + Duration::from_millis(16_300),
    );
    assert!(!loss.latched());
    assert!(loss.take_lifted().is_some());
}

#[test]
fn loss_from_before_the_latch_does_not_confirm_it() {
    let t0 = Instant::now();
    let late = t0 + Duration::from_secs(1);
    // K1 and its answer K2 are both out when K1 is reported lost.
    let mut loss = KeyframeLoss::new(true);
    loss.sending_at(1, true, t0);
    assert_eq!(loss.requested(t0 + Duration::from_millis(100)), None);
    loss.sending_at(2, false, t0 + PERIOD);
    loss.sending_at(3, true, t0 + Duration::from_millis(120));
    loss.sending_at(4, false, t0 + Duration::from_millis(140));
    let latched = loss.unrecoverable(1, 4, PERIOD, late).unwrap();
    assert_eq!(latched.kind, LatchKind::Startup);
    // K2, sent before the latch, reported lost: the same bad start.
    assert_eq!(loss.unrecoverable(3, 4, PERIOD, late), None);
    assert_eq!(loss.abandoned(3, PERIOD, late), None);
    for frame in [2, 4] {
        assert_eq!(loss.unrecoverable(frame, 4, PERIOD, late), None);
    }
    assert_eq!(loss.kind(), Some(LatchKind::Startup));
    assert!(loss.sustained_at().is_none());
    // A keyframe sent after the latch and lost is more.
    loss.sending_at(5, true, late);
    assert_eq!(
        loss.unrecoverable(5, 5, PERIOD, late + Duration::from_millis(50))
            .unwrap()
            .kind,
        LatchKind::Sustained
    );
    // A repeated-request latch followed by the old first keyframe's report.
    let mut loss = KeyframeLoss::new(true);
    startup(&mut loss, t0);
    let t = t0 + Duration::from_millis(600);
    assert_eq!(loss.requested(t), None);
    loss.sending_at(30, true, t + PERIOD);
    loss.sending_at(31, false, t + PERIOD * 2);
    let latched = loss.requested(t + Duration::from_millis(60)).unwrap();
    assert_eq!(latched.kind, LatchKind::Startup);
    let after = t + Duration::from_millis(100);
    assert_eq!(loss.unrecoverable(1, 31, PERIOD, after), None);
    assert_eq!(loss.unrecoverable(11, 31, PERIOD, after), None);
    assert_eq!(loss.kind(), Some(LatchKind::Startup));
    // Ordinary frames sent before the latch do not count either.
    for frame in [12, 13, 14] {
        assert_eq!(loss.unrecoverable(frame, 31, PERIOD, after), None);
    }
    assert_eq!(loss.kind(), Some(LatchKind::Startup));
}

#[test]
fn only_the_first_keyframe_and_the_clients_quick_request_are_the_start() {
    let t0 = Instant::now();
    // An encoder rebuild two and a half seconds in.
    let mut loss = KeyframeLoss::new(true);
    loss.sending_at(1, true, t0);
    loss.host_keyframe();
    loss.sending_at(150, true, t0 + Duration::from_millis(2_500));
    let latched = loss
        .unrecoverable(150, 151, PERIOD, t0 + Duration::from_millis(2_600))
        .unwrap();
    assert_eq!(latched.kind, LatchKind::Sustained);
    // A keyframe nobody asked for, even soon after the first.
    let mut loss = KeyframeLoss::new(true);
    loss.sending_at(1, true, t0);
    loss.sending_at(40, true, t0 + Duration::from_millis(700));
    let latched = loss
        .unrecoverable(40, 41, PERIOD, t0 + Duration::from_millis(800))
        .unwrap();
    assert_eq!(latched.kind, LatchKind::Sustained);
    // A keyframe the client asked for, too late to be the first's answer.
    let mut loss = KeyframeLoss::new(true);
    loss.sending_at(1, true, t0);
    assert_eq!(loss.requested(t0 + Duration::from_millis(1_800)), None);
    loss.sending_at(100, true, t0 + Duration::from_millis(1_900));
    let latched = loss
        .unrecoverable(100, 101, PERIOD, t0 + Duration::from_millis(2_000))
        .unwrap();
    assert_eq!(latched.kind, LatchKind::Sustained);
    // The quick one is.
    let mut loss = KeyframeLoss::new(true);
    loss.sending_at(1, true, t0);
    assert_eq!(loss.requested(t0 + Duration::from_millis(300)), None);
    loss.sending_at(20, true, t0 + Duration::from_millis(400));
    let latched = loss
        .unrecoverable(20, 21, PERIOD, t0 + Duration::from_millis(500))
        .unwrap();
    assert_eq!(latched.kind, LatchKind::Startup);
}

#[test]
fn more_evidence_confirms_a_provisional_latch_and_it_stays() {
    let t0 = Instant::now();
    let (mut loss, _) = lost_first_keyframe(t0);
    // More reports about the same keyframe are not new evidence.
    // (Past the host-change window, so ordinary reports count.)
    let at = t0 + Duration::from_secs(1);
    assert_eq!(loss.unrecoverable(1, 2, PERIOD, at), None);
    assert_eq!(loss.abandoned(1, PERIOD, at), None);
    assert_eq!(loss.kind(), Some(LatchKind::Startup));
    // Three distinct unrecoverable frames are.
    for frame in 3..=4 {
        loss.sending_at(frame, false, at);
        assert_eq!(loss.unrecoverable(frame, 4, PERIOD, at), None);
    }
    loss.sending_at(5, false, at);
    let confirmed = loss.unrecoverable(5, 5, PERIOD, at).unwrap();
    assert!(confirmed.confirmed);
    assert_eq!(confirmed.kind, LatchKind::Sustained);
    assert_eq!(loss.kind(), Some(LatchKind::Sustained));
    assert_eq!(loss.sustained_at(), Some(at));
    // It no longer lifts.
    flow(&mut loss, 5, at, at + Duration::from_secs(30));
    assert!(loss.latched());
    assert!(loss.take_lifted().is_none());
}

#[test]
fn a_second_latch_in_the_session_is_sustained() {
    let t0 = Instant::now();
    let (mut loss, _) = lost_first_keyframe(t0);
    let frame = flow(&mut loss, 3, t0 + PERIOD * 2, t0 + Duration::from_secs(11));
    assert!(!loss.latched());
    // Whatever it is lost on, the second latch is sustained.
    let again = t0 + Duration::from_millis(11_000);
    loss.sending_at(frame, true, again);
    let latched = loss
        .unrecoverable(frame, frame, PERIOD, again + Duration::from_millis(100))
        .unwrap();
    assert_eq!(latched.kind, LatchKind::Sustained);
    assert!(!latched.confirmed);
    assert!(loss.sustained_at().is_some());
    let later = again + Duration::from_secs(1);
    flow(&mut loss, frame + 1, later, later + Duration::from_secs(30));
    assert!(loss.latched());
}

#[test]
fn only_the_start_of_the_stream_is_provisional() {
    let t0 = Instant::now();
    // A keyframe lost long after the first.
    let mut loss = KeyframeLoss::new(true);
    loss.sending_at(1, true, t0);
    loss.sending_at(500, true, t0 + STARTUP_SPAN + Duration::from_secs(1));
    let latched = loss
        .unrecoverable(500, 501, PERIOD, t0 + STARTUP_SPAN + Duration::from_secs(2))
        .unwrap();
    assert_eq!(latched.kind, LatchKind::Sustained);
    assert!(loss.sustained_at().is_some());
    // Evidence about ordinary frames came first.
    let mut loss = KeyframeLoss::new(true);
    loss.sending_at(1, true, t0);
    loss.sending_at(2, false, t0 + PERIOD);
    loss.sending_at(30, false, t0 + Duration::from_secs(1));
    let seen = t0 + Duration::from_millis(1_100);
    assert_eq!(loss.unrecoverable(20, 30, PERIOD, seen), None);
    let latched = loss.unrecoverable(1, 30, PERIOD, seen).unwrap();
    assert_eq!(latched.kind, LatchKind::Sustained);
    // A keyframe lost on send at the start is provisional too, as the
    // host's own loss.
    let mut loss = KeyframeLoss::new(true);
    loss.sending_at(1, true, t0);
    let latched = loss.abandoned(1, PERIOD, t0).unwrap();
    assert_eq!(latched.reason, "a keyframe was lost on send");
    assert_eq!(latched.kind, LatchKind::Send);
    // Three frames lost on send are never a start-up burst, but they are
    // still only the host's.
    let mut loss = running(100);
    loss.abandoned(40, PERIOD, t0);
    loss.abandoned(41, PERIOD, t0);
    assert_eq!(
        loss.abandoned(42, PERIOD, t0).unwrap().kind,
        LatchKind::Send
    );
}

#[test]
fn a_repeated_request_at_the_start_is_provisional() {
    let t0 = Instant::now();
    let mut loss = KeyframeLoss::new(true);
    startup(&mut loss, t0);
    // Past the window the client asks, is sent a keyframe, asks again.
    let t = t0 + Duration::from_millis(600);
    assert_eq!(loss.requested(t), None);
    loss.sending_at(30, true, t + PERIOD);
    loss.sending_at(31, false, t + PERIOD * 2);
    let latched = loss.requested(t + Duration::from_millis(60)).unwrap();
    assert_eq!(latched.kind, LatchKind::Startup);
    assert_eq!(loss.bps(FAST, SAFE), SAFE);
    assert!(loss.sustained_at().is_none());
    // The same chain deep into the stream is not.
    let mut loss = running(100);
    let t = Instant::now();
    assert_eq!(loss.requested(t), None);
    loss.sending_at(101, true, t + PERIOD);
    loss.sending_at(102, false, t + PERIOD * 2);
    let latched = loss.requested(t + Duration::from_millis(60)).unwrap();
    assert_eq!(latched.kind, LatchKind::Sustained);
}

#[test]
fn an_inherited_latch_stays_sustained_when_the_first_keyframe_is_lost_again() {
    let t0 = Instant::now();
    let mut loss = KeyframeLoss::remembered("remembered from the previous session");
    assert_eq!(loss.kind(), Some(LatchKind::Sustained));
    loss.sending_at(1, true, t0);
    loss.sending_at(2, false, t0 + PERIOD);
    let latched = loss
        .unrecoverable(1, 2, PERIOD, t0 + Duration::from_millis(100))
        .unwrap();
    assert_eq!(latched.kind, LatchKind::Sustained);
    assert!(loss.sustained_at().is_some());
    flow(&mut loss, 3, t0 + PERIOD * 2, t0 + Duration::from_secs(30));
    assert!(loss.latched());
}

#[test]
fn the_latch_names_the_frames_behind_it() {
    let now = Instant::now();
    let mut loss = running(30);
    for frame in [20, 22] {
        assert_eq!(loss.unrecoverable(frame, 30, PERIOD, now), None);
    }
    let latched = loss.unrecoverable(25, 30, PERIOD, now).unwrap();
    assert_eq!(
        latched.detail,
        "unrecoverable frames [20, 22, 25]; last frame sent 30"
    );
}

#[test]
fn the_bitrate_controller_shares_the_classification_even_with_the_guard_off() {
    let t0 = Instant::now();
    for enabled in [true, false] {
        let mut loss = KeyframeLoss::new(enabled);
        loss.sending_at(1, true, t0);
        for frame in 2..=10 {
            loss.sending_at(frame, false, t0);
        }
        // The first keyframe's startup window: ordinary frames after it
        // and requests are the client reacting, not the network.
        assert!(!loss.network_loss(5, t0 + Duration::from_millis(10)));
        assert!(!loss.network_request(t0 + Duration::from_millis(10)));
        // A lost keyframe always counts.
        assert!(loss.network_loss(1, t0 + Duration::from_millis(10)));
        // Past the window an ordinary frame does.
        let later = t0 + STARTUP_WINDOW + Duration::from_millis(1);
        assert!(loss.network_loss(5, later));
        assert!(loss.network_request(later));
        // A frame the host abandoned on send is counted there.
        loss.abandoned(7, PERIOD, later);
        assert!(!loss.network_loss(7, later), "guard {enabled}");
    }
}

#[test]
fn a_request_behind_a_full_decode_queue_is_not_network_evidence() {
    let t0 = Instant::now();
    let mut loss = running(1);
    let first = t0 + Duration::from_secs(5);
    assert_eq!(loss.requested(first), None);
    loss.sending_at(40, true, first);
    for frame in 41..41 + DECODE_QUEUE {
        loss.sending_at(frame, false, first);
    }
    let soon = first + Duration::from_millis(250);
    assert!(!loss.network_request(soon), "overflow behind the keyframe");
    // A second later it is a fresh loss again.
    assert!(loss.network_request(first + REPEAT + Duration::from_millis(1)));
    // Right behind the keyframe it is the keyframe that was lost.
    let mut loss = running(1);
    assert_eq!(loss.requested(first), None);
    loss.sending_at(40, true, first);
    loss.sending_at(41, false, first);
    assert!(loss.network_request(soon));
}

/// Forty seconds of the October 28 scene-cut run: 200 Mbps HEVC at 60 fps
/// on an 800 Mbps route with 0.3 % random loss and no bottleneck, a scene
/// cut every two seconds. AMF does not hold the cut, an ordinary frame, to
/// the frame cap, so it leaves without parity and is lost; the recovery
/// keyframe answering it is as large as the cap allows. A frame is lost
/// when one of its blocks loses more packets than it has parity. Returns
/// the recovery keyframes lost and the guard.
fn lost_cut_recoveries(fec_whole: bool) -> (usize, KeyframeLoss) {
    use crate::network_pacing::{fec_whole_frame_cap, recovery_frame_cap};
    let packetizer = crate::packet::VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
        packet_size: 1392,
        fec_percent: 20,
        min_fec: 2,
        key: Some([0; 16]),
    };
    let (kbps, period) = (200_000, Duration::from_secs_f64(1. / 60.));
    let mut cap = recovery_frame_cap(FAST, kbps, period, 20, 1392, false);
    if fec_whole {
        cap = fec_whole_frame_cap(cap, packetizer.fec_whole_payload_limit(), kbps, period);
    }
    let blocks = packetizer.block_layout(cap as usize);
    let mut seed = 0x2545_f491_4f6c_dd1d_u64;
    let mut lost_packet = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % 1000 < 3
    };
    let start = started_long_ago();
    let mut loss = KeyframeLoss::new(true);
    loss.sending_at(1, true, start);
    let mut lost = 0;
    for frame in 2..=2400u32 {
        let at = start + period * frame;
        if frame % 120 != 0 {
            loss.sending_at(frame, false, at);
            continue;
        }
        // The cut, reported two frames later; then the recovery keyframe.
        loss.sending_at(frame, false, at);
        loss.unrecoverable(frame, frame + 2, period, at + period * 2);
        loss.loss_keyframe();
        loss.sending_at(frame + 3, true, at + period * 3);
        let whole = blocks
            .iter()
            .all(|&(data, parity)| (0..data + parity).filter(|_| lost_packet()).count() <= parity);
        if !whole {
            lost += 1;
            loss.unrecoverable(frame + 3, frame + 5, period, at + period * 5);
        }
    }
    (lost, loss)
}

#[test]
fn recovery_keyframes_within_full_fec_survive_random_loss_without_a_latch() {
    // The automatic cap, about 2.6 MB, leaves no parity in four blocks:
    // the first lost recovery keyframe latches the slow pace.
    let (lost, loss) = lost_cut_recoveries(false);
    assert!(lost >= 15, "{lost} of 20 recovery keyframes lost");
    assert!(loss.latched());
    // Held to full parity (about 1.17 MB) every one arrives, and the cut
    // losses alone, one every two seconds, slow nothing.
    let (lost, loss) = lost_cut_recoveries(true);
    assert_eq!(lost, 0);
    assert!(!loss.latched());
    assert_eq!(loss.bps(FAST, SAFE), FAST);
}
