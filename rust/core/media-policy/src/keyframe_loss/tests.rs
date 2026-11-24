//! Tests for keyframe-loss detection, ordinary loss windows, and host-provoked exclusions.

use super::*;

const FAST: u64 = 800_000_000;
const SAFE: u64 = 62_000_000;
const PERIOD: Duration = Duration::from_millis(16);

#[test]
fn remembered_loss_slows_the_first_frame_but_only_new_evidence_renews_it() {
    let now = Instant::now();
    let mut loss = KeyframeLoss::remembered("remembered from the previous session");
    assert_eq!(loss.bps(FAST, SAFE), SAFE);
    assert_eq!(loss.observed_at(), None);
    for frame in 1..=2 {
        assert_eq!(loss.unrecoverable(frame, 3, PERIOD, now), None);
    }
    assert!(loss.unrecoverable(3, 3, PERIOD, now).is_some());
    assert_eq!(loss.observed_at(), Some(now));
    assert_ne!(loss.reason(), Some("remembered from the previous session"));
    assert_eq!(loss.unrecoverable(4, 4, PERIOD, now + PERIOD), None);
    assert_eq!(loss.observed_at(), Some(now));
}

#[test]
fn a_keyframe_the_client_could_not_repair_slows_every_later_frame() {
    let now = Instant::now();
    let mut loss = KeyframeLoss::new(true);
    loss.sending(1, true);
    for frame in 2..=40 {
        loss.sending(frame, false);
    }
    // An isolated ordinary-frame loss is left to recovery frames.
    assert_eq!(loss.unrecoverable(7, 40, PERIOD, now), None);
    assert_eq!(loss.bps(FAST, SAFE), FAST);
    assert!(loss.unrecoverable(1, 40, PERIOD, now).is_some());
    assert_eq!(loss.bps(FAST, SAFE), SAFE);
    // It holds for the session and reports once.
    assert_eq!(loss.unrecoverable(1, 40, PERIOD, now), None);
    assert!(loss.latched());
    // Slower than the route only: a manual or Wi-Fi rate below it stays.
    assert_eq!(loss.bps(40_000_000, SAFE), 40_000_000);
}

#[test]
fn a_repeated_request_after_its_keyframe_was_sent_slows_the_stream() {
    let start = Instant::now();
    let mut loss = KeyframeLoss::new(true);
    // A keyframe is lost long after the start: the client asks for one.
    loss.sending_at(1, true, started_long_ago());
    assert_eq!(loss.requested(start), None);
    // A second request before any keyframe answered the first is the
    // same loss, not a lost keyframe.
    assert_eq!(loss.requested(start + Duration::from_millis(20)), None);
    loss.sending(3, true);
    loss.sending(4, false);
    assert!(loss.requested(start + Duration::from_millis(60)).is_some());
    assert_eq!(loss.bps(FAST, SAFE), SAFE);
}

#[test]
fn a_decoder_that_falls_behind_keeps_the_fast_rate() {
    // A 240 fps stream to a client whose decode queue overflows: every
    // keyframe arrives, 15 or more frames queue up behind it and the
    // client asks again, about 70 ms apart (e2e-av1-240-ddx-load: 24
    // requests in 10 s, the stream paced at twice the bitrate).
    let start = Instant::now();
    let mut loss = KeyframeLoss::new(true);
    let mut frame = 1;
    for request in 0..24u32 {
        loss.sending(frame, true);
        frame += 1;
        for _ in 0..DECODE_QUEUE + 1 {
            loss.sending(frame, false);
            frame += 1;
        }
        assert_eq!(
            loss.requested(start + Duration::from_millis(u64::from(request) * 70)),
            None,
            "request {request}"
        );
    }
    assert_eq!(loss.bps(FAST, SAFE), FAST);
    // A keyframe lost on the way is still asked for at the next frame.
    loss.sending(frame, true);
    loss.sending(frame + 1, false);
    assert!(
        loss.requested(start + Duration::from_millis(24 * 70))
            .is_some()
    );
    assert_eq!(loss.bps(FAST, SAFE), SAFE);
}

#[test]
fn spaced_recoveries_on_a_healthy_link_keep_the_fast_rate() {
    let start = Instant::now();
    let mut loss = KeyframeLoss::new(true);
    let mut frame = 1;
    // A keyframe a second for a minute, each one received: requests are
    // REPEAT or more apart.
    for second in 0..60 {
        loss.sending(frame, true);
        frame += 1;
        assert_eq!(
            loss.requested(start + Duration::from_millis(second * 1000 + 500)),
            None
        );
    }
    // Isolated reports of lost ordinary frames do not slow it either.
    for lost in 2..frame {
        if !loss.keyframes.contains(&Some(lost)) {
            assert_eq!(
                loss.unrecoverable(lost, lost, PERIOD, start + LOSS_WINDOW * lost),
                None
            );
        }
    }
    assert_eq!(loss.bps(FAST, SAFE), FAST);
}

#[test]
fn disabled_it_never_slows_the_stream() {
    let start = Instant::now();
    let mut loss = KeyframeLoss::new(false);
    loss.sending(1, true);
    assert_eq!(loss.unrecoverable(1, 1, PERIOD, start), None);
    loss.requested(start);
    loss.sending(2, true);
    assert_eq!(loss.requested(start + Duration::from_millis(10)), None);
    assert_eq!(loss.bps(FAST, SAFE), FAST);
}

/// The bitrate controller reads [`KeyframeLoss::network_loss`] and
/// [`KeyframeLoss::network_request`] whether the guard is on or off. A
/// keyframe the client asked for answers it; it is not the host's own change,
/// so loss and requests right after it stay evidence. A request from a
/// decode queue that overflowed behind that keyframe still is not.
#[test]
fn a_requested_keyframe_is_not_the_host_s_own_with_the_guard_on_or_off() {
    for enabled in [false, true] {
        let mut loss = KeyframeLoss::new(enabled);
        loss.sending_at(1, true, started_long_ago());
        let start = Instant::now();
        for frame in 2..=50 {
            loss.sending_at(frame, false, start - PERIOD * (51 - frame));
        }
        assert!(loss.network_request(start));
        loss.requested(start);
        loss.sending_at(51, true, start + PERIOD);
        loss.sending_at(52, false, start + PERIOD * 2);
        assert!(
            loss.network_loss(52, start + Duration::from_millis(100)),
            "guard {enabled}: a frame lost after the requested keyframe was set aside"
        );
        for frame in 53..53 + DECODE_QUEUE {
            loss.sending_at(frame, false, start + PERIOD * (frame - 50));
        }
        assert!(
            !loss.network_request(start + Duration::from_millis(300)),
            "guard {enabled}: a decode queue overflow counted as the network"
        );
        assert!(
            loss.network_request(start + Duration::from_millis(600)),
            "guard {enabled}: a request after the requested keyframe was set aside"
        );
    }
}

/// Two seconds of a 31 Mbps (a 40 Mbps client's) 1440p60 stream through
/// a 100 Mbps egress with a drop-tail queue of `queue` bytes, behind the
/// host's 1 Gbps link, as in `network_pacing`'s model. A Moonlight client
/// without reference invalidation shows nothing until a keyframe arrives
/// whole, and asks for one after each loss once a later frame arrives.
/// Returns the frames it showed and whether pacing slowed.
fn shown_frames(
    enabled: bool,
    fec_reports: bool,
    egress_bps: f64,
    queue: usize,
    fast: u64,
) -> (u32, bool) {
    use crate::network_pacing::{
        Pacer, batch_payload_limit, overhead, rate_bps, video_wire_rate_bps,
    };
    let packetizer = crate::packet::VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
        packet_size: 1392,
        fec_percent: 20,
        min_fec: 2,
        key: Some([0; 16]),
    };
    let (kbps, period) = (31_000, Duration::from_secs_f64(1. / 60.));
    let needed = video_wire_rate_bps(&packetizer, kbps, period, false);
    let safe = rate_bps(0, kbps, 0, true, needed);
    let datagram = packetizer.packet_size + 16 + 32;
    let wire = (datagram + overhead(false)) as f64 * 8.;
    let (wire_time, host_time) = (
        Duration::from_secs_f64(wire / egress_bps),
        Duration::from_secs_f64(wire / 1e9),
    );
    let capacity = Duration::from_secs_f64(queue as f64 * 8. / egress_bps);
    let start = Instant::now();
    let mut loss = KeyframeLoss::new(enabled);
    let mut pacer = Pacer::new(start);
    let (mut now, mut drained) = (start, start);
    let (mut waiting, mut shown) = (true, 0);
    for frame in 1..=120u32 {
        now = now.max(start + period * (frame - 1));
        // A keyframe is five ordinary frames here; AMF's are larger at low motion.
        let keyframe = waiting;
        let payload =
            (f64::from(kbps) * 125. * period.as_secs_f64()) as usize * if keyframe { 5 } else { 1 };
        loss.sending_at(frame, keyframe, now);
        let bps = loss.bps(fast, safe);
        let batch = (batch_payload_limit(bps, datagram, false, 64 * 1024) / datagram).clamp(1, 64);
        let mut whole = true;
        for (data, fec) in packetizer.block_layout(payload) {
            let (mut remaining, mut lost) = (data + fec, 0);
            while remaining > 0 {
                now = now.max(pacer.due());
                let count = remaining.min(batch);
                for _ in 0..count {
                    now += host_time;
                    let done = drained.max(now) + wire_time;
                    if done.saturating_duration_since(now) <= capacity {
                        drained = done;
                    } else {
                        lost += 1;
                    }
                }
                pacer.sent(now, count * datagram, count, false, bps);
                remaining -= count;
            }
            whole &= lost <= fec;
        }
        if !whole {
            if fec_reports {
                loss.unrecoverable(frame, frame, period, now);
            }
            // The request follows the next frame, a period later.
            loss.requested(now + period);
            waiting = true;
        } else if keyframe || !waiting {
            waiting = false;
            shown += 1;
        }
    }
    (shown, loss.latched())
}

#[test]
fn a_slow_port_that_drops_every_fast_keyframe_gets_a_picture() {
    for queue in [16 * 1024, 64 * 1024] {
        for fec_reports in [true, false] {
            // The released policy: every keyframe is lost, nothing is shown.
            assert_eq!(
                shown_frames(false, fec_reports, 100e6, queue, FAST),
                (0, false),
                "{queue} B queue"
            );
            // Slowed after the first lost keyframe, or the repeated
            // request without reports, the third keyframe arrives. The
            // requests of the first half second are the client's start
            // and do not count, so without reports it takes that long.
            let (shown, latched) = shown_frames(true, fec_reports, 100e6, queue, FAST);
            let least = if fec_reports { 117 } else { 80 };
            assert!(
                latched && shown >= least,
                "{queue} B queue, reports {fec_reports}: {shown}"
            );
        }
    }
}

#[test]
fn a_fast_path_keeps_the_fast_rate() {
    for queue in [16 * 1024, 64 * 1024] {
        assert_eq!(shown_frames(true, true, 1e9, queue, FAST), (120, false));
    }
}

#[test]
fn a_100_mbps_client_that_says_so_gets_its_first_keyframe_whole_without_a_latch() {
    // The same slow port as above, but the client reports its link
    // (Nightjar, wired): the host paces at 4/5 of it from the first
    // packet, so the keyframe fits the port's queue and nothing is lost.
    use crate::network_pacing::rate_bps_for_client;
    let hinted = rate_bps_for_client(0, 31_000, 1_000_000_000, false, 0, 100_000_000);
    assert_eq!(hinted, 80_000_000);
    for queue in [16 * 1024, 64 * 1024] {
        for fec_reports in [true, false] {
            assert_eq!(
                shown_frames(true, fec_reports, 100e6, queue, hinted),
                (120, false),
                "{queue} B queue, reports {fec_reports}"
            );
            // Without the hint, the same stream loses its keyframe.
            let (shown, latched) = shown_frames(true, fec_reports, 100e6, queue, FAST);
            assert!(latched && shown < 120, "{queue} B queue: {shown}");
        }
    }
}

#[test]
fn only_recent_keyframes_match_reports() {
    let now = Instant::now();
    let mut loss = KeyframeLoss::new(true);
    for frame in 1..=9 {
        loss.sending(frame * 100, true);
    }
    // The first keyframe left the eight-entry history.
    assert_eq!(loss.unrecoverable(100, 900, PERIOD, now), None);
    assert!(loss.unrecoverable(200, 900, PERIOD, now).is_some());
}

#[test]
fn repeated_ordinary_losses_slow_before_any_keyframe_is_lost() {
    let now = Instant::now();
    let mut loss = KeyframeLoss::new(true);
    loss.sending_at(1, true, started_long_ago());
    for frame in 2..=20 {
        loss.sending(frame, false);
    }
    assert_eq!(loss.unrecoverable(5, 20, PERIOD, now), None);
    assert_eq!(
        loss.unrecoverable(12, 20, PERIOD, now + Duration::from_millis(500)),
        None
    );
    assert_eq!(loss.bps(FAST, SAFE), FAST);
    // Reports may arrive out of frame order.
    assert!(
        loss.unrecoverable(8, 20, PERIOD, now + Duration::from_secs(1))
            .is_some()
    );
    assert_eq!(loss.bps(FAST, SAFE), SAFE);
    assert_eq!(
        loss.unrecoverable(20, 20, PERIOD, now + Duration::from_secs(1)),
        None
    );
}

#[test]
fn interleaved_block_reports_count_each_frame_once() {
    let now = Instant::now();
    let mut loss = KeyframeLoss::new(true);
    for frame in [1, 2, 1, 2, 1, 2, 1, 2] {
        assert_eq!(loss.unrecoverable(frame, 3, PERIOD, now), None);
    }
    assert_eq!(loss.bps(FAST, SAFE), FAST);
    assert!(loss.unrecoverable(3, 3, PERIOD, now).is_some());
}

#[test]
fn loss_window_expires_individual_entries() {
    let start = Instant::now();
    let mut loss = KeyframeLoss::new(true);
    assert_eq!(loss.unrecoverable(1, 1, PERIOD, start), None);
    assert_eq!(
        loss.unrecoverable(2, 2, PERIOD, start + Duration::from_secs(1)),
        None
    );
    // Exactly two seconds expires frame 1, but retains frame 2.
    assert_eq!(loss.unrecoverable(3, 3, PERIOD, start + LOSS_WINDOW), None);
    assert_eq!(loss.bps(FAST, SAFE), FAST);
    assert!(
        loss.unrecoverable(4, 4, PERIOD, start + LOSS_WINDOW + PERIOD)
            .is_some()
    );
}

#[test]
fn ordinary_loss_fence_rejects_stale_and_future_frames_and_handles_wrap() {
    let now = Instant::now();
    for fps in [30, 60, 120, 240] {
        let period = Duration::from_secs_f64(1. / f64::from(fps));
        let mut loss = KeyframeLoss::new(true);
        for frame in [1, 2, 3, 1001] {
            assert_eq!(loss.unrecoverable(frame, 1000, period, now), None);
        }
        assert_eq!(loss.unrecoverable(998, 1000, period, now), None);
        assert_eq!(loss.unrecoverable(999, 1000, period, now), None);
        assert!(loss.unrecoverable(1000, 1000, period, now).is_some());
    }
    let mut loss = KeyframeLoss::new(true);
    assert_eq!(loss.unrecoverable(u32::MAX, 1, PERIOD, now), None);
    assert_eq!(loss.unrecoverable(0, 1, PERIOD, now), None);
    assert!(loss.unrecoverable(1, 1, PERIOD, now).is_some());
}

#[test]
fn disabled_guard_and_unknown_period_ignore_repeated_ordinary_losses() {
    let now = Instant::now();
    for (enabled, period) in [(false, PERIOD), (true, Duration::ZERO)] {
        let mut loss = KeyframeLoss::new(enabled);
        for frame in 1..20 {
            assert_eq!(loss.unrecoverable(frame, frame, period, now), None);
        }
        assert_eq!(loss.bps(FAST, SAFE), FAST);
    }
}

/// A stream that has been running a while: frame 1 was the first keyframe
/// and ordinary frames follow it up to `last`.
/// Seconds into a stream, well past its first keyframe's startup window.
fn started_long_ago() -> Instant {
    Instant::now() - Duration::from_secs(10)
}

fn running(last: u32) -> KeyframeLoss {
    let mut loss = KeyframeLoss::new(true);
    loss.sending_at(1, true, started_long_ago());
    for frame in 2..=last {
        loss.sending(frame, false);
    }
    loss
}

#[test]
fn a_keyframe_the_host_sent_on_its_own_is_not_the_one_a_request_asks_again_for() {
    // Bitrate change, encoder or capture restart, HDR metadata: the
    // encoder emits a keyframe nobody asked for. The client resets its
    // decoder and asks, twice in quick succession, as Moonlight does when
    // a request races the keyframe already on its way. Nothing was lost.
    let start = Instant::now();
    let mut loss = running(300);
    loss.sending_at(301, true, start);
    assert_eq!(loss.requested(start + Duration::from_millis(40)), None);
    loss.sending_at(302, true, start + Duration::from_millis(45));
    loss.sending_at(303, false, start + Duration::from_millis(50));
    assert_eq!(loss.requested(start + Duration::from_millis(120)), None);
    assert!(!loss.latched());
    assert_eq!(loss.bps(FAST, SAFE), FAST);
    assert!(loss.take_ignored().is_some());
}

#[test]
fn requests_the_host_provoked_do_not_count_even_without_its_keyframe_being_asked_for() {
    // A request is answered, then the host changes the stream again with
    // a keyframe of its own (Alt+Tab recreates the capture and encoder)
    // and the client asks once more. The old rule armed on every keyframe.
    let start = Instant::now();
    let mut loss = running(50);
    assert_eq!(loss.requested(start), None);
    loss.sending_at(51, true, start + Duration::from_millis(5));
    loss.sending_at(60, true, start + Duration::from_millis(200));
    assert_eq!(loss.requested(start + Duration::from_millis(300)), None);
    assert_eq!(loss.bps(FAST, SAFE), FAST);
}

#[test]
fn ordinary_frames_after_a_host_keyframe_are_not_counted_for_a_second() {
    let start = Instant::now();
    let mut loss = running(100);
    loss.sending_at(101, true, start);
    for frame in 102..=140 {
        loss.sending_at(frame, false, start);
    }
    for (frame, ms) in [(110, 100), (120, 200), (130, 300), (131, 400)] {
        assert_eq!(
            loss.unrecoverable(frame, 140, PERIOD, start + Duration::from_millis(ms)),
            None
        );
    }
    assert!(!loss.latched());
    // Losses that persist after the window are counted.
    let after = start + HOST_CHANGE_WINDOW;
    for frame in [132, 133] {
        assert_eq!(loss.unrecoverable(frame, 140, PERIOD, after), None);
    }
    assert!(loss.unrecoverable(134, 140, PERIOD, after).is_some());
    // Frames sent before the host's change were not affected by it.
    let mut loss = running(100);
    loss.sending_at(101, true, start);
    for frame in [97, 98] {
        assert_eq!(loss.unrecoverable(frame, 101, PERIOD, start), None);
    }
    assert!(loss.unrecoverable(99, 101, PERIOD, start).is_some());
}

#[test]
fn a_host_keyframe_the_client_could_not_rebuild_is_still_a_lost_keyframe() {
    let start = Instant::now();
    let mut loss = running(100);
    loss.sending_at(101, true, start);
    let latched = loss
        .unrecoverable(101, 103, PERIOD, start + Duration::from_millis(30))
        .unwrap();
    assert_eq!(latched.reason, "a keyframe arrived incomplete");
    assert!(latched.detail.contains("101"), "{latched:?}");
    assert_eq!(loss.bps(FAST, SAFE), SAFE);
}

#[test]
fn a_keyframe_asked_again_for_after_the_window_still_slows_the_stream() {
    let start = Instant::now();
    let mut loss = running(100);
    loss.sending_at(101, true, start);
    let late = start + HOST_CHANGE_WINDOW + Duration::from_millis(300);
    assert_eq!(loss.requested(late), None);
    loss.sending_at(150, true, late + Duration::from_millis(20));
    loss.sending_at(151, false, late + Duration::from_millis(30));
    let latched = loss.requested(late + Duration::from_millis(100)).unwrap();
    assert_eq!(
        latched.reason,
        "the client asked again for the keyframe it was just sent"
    );
    assert!(latched.detail.contains("150"), "{latched:?}");
    assert_eq!(loss.bps(FAST, SAFE), SAFE);
}

/// The rc.4 run-1 chain (#59): the client asks, keyframe 2509 goes out,
/// 11 frames follow, and it asks again 128 ms after the first request.
/// It reported no loss at all.
fn asked_again(loss: &mut KeyframeLoss, t: Instant) -> Option<Latched> {
    assert_eq!(loss.requested(t), None);
    loss.sending_at(2509, true, t + Duration::from_millis(10));
    for frame in 2510..=2520 {
        loss.sending_at(frame, false, t + Duration::from_millis(10) + PERIOD);
    }
    loss.requested(t + Duration::from_millis(128))
}

#[test]
fn a_client_that_reports_its_losses_does_not_latch_on_a_repeated_request() {
    let t = Instant::now();
    let mut loss = running(2500);
    loss.set_reports_losses(true);
    assert_eq!(asked_again(&mut loss, t), None);
    let note = loss.take_ignored().unwrap();
    assert!(note.contains("128 ms"), "{note}");
    assert!(note.contains("reports its own losses"), "{note}");
    assert!(!loss.latched());
    assert_eq!(loss.bps(FAST, SAFE), FAST);
    // A client without FEC status latches on the same chain, as before.
    let mut loss = running(2500);
    let latched = asked_again(&mut loss, t).unwrap();
    assert_eq!(
        latched.reason,
        "the client asked again for the keyframe it was just sent"
    );
    assert_eq!(loss.bps(FAST, SAFE), SAFE);
}

#[test]
fn a_client_that_reports_its_losses_still_latches_on_an_incomplete_keyframe() {
    let t = Instant::now();
    let mut loss = running(2500);
    loss.set_reports_losses(true);
    assert_eq!(asked_again(&mut loss, t), None);
    let latched = loss
        .unrecoverable(2509, 2520, PERIOD, t + Duration::from_millis(130))
        .unwrap();
    assert_eq!(latched.reason, "a keyframe arrived incomplete");
    assert_eq!(loss.bps(FAST, SAFE), SAFE);
    // So do three unrecoverable frames.
    let mut loss = running(2500);
    loss.set_reports_losses(true);
    for frame in [2400, 2450] {
        assert_eq!(loss.unrecoverable(frame, 2500, PERIOD, t), None);
    }
    assert!(loss.unrecoverable(2490, 2500, PERIOD, t).is_some());
}
/// Ordinary frames numbered from `from`, one per period starting at
/// `start`, until `end`. Returns the next frame number.
fn flow(loss: &mut KeyframeLoss, from: u32, start: Instant, end: Instant) -> u32 {
    let mut frame = from;
    while start + PERIOD * (frame - from) < end {
        loss.sending_at(frame, false, start + PERIOD * (frame - from));
        frame += 1;
    }
    frame
}

mod send_loss;
mod startup;
