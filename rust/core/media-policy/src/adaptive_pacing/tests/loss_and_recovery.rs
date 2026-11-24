//! Tests for adaptive pacing loss classification, frame fences, and recovery backoff.

use super::*;

#[test]
fn one_failed_frame_is_not_counted_as_four_when_it_has_four_blocks() {
    let mut stream = Stream::new(true);
    for _ in 0..4 {
        stream.report(30, 30, Block::Unrecoverable);
    }
    assert_eq!(stream.healthy(), None);
    // A delayed block for that same frame in a later control interval cannot
    // make one actual failed frame look like a new failure.
    stream.report(30, 60, Block::Unrecoverable);
    stream.report(60, 60, Block::Unrecoverable);
    assert_eq!(stream.healthy(), None);
    assert_eq!(stream.failures(), Some((FAST, FAST / 2)));
}

#[test]
fn recovered_blocks_do_not_hide_failed_blocks_of_the_same_frames() {
    let mut stream = Stream::new(true);
    for frame in [29, 30] {
        stream.report(frame, 30, Block::Recovered { lost: 1, data: 100 });
        stream.report(frame, 30, Block::Unrecoverable);
    }
    assert_eq!(stream.healthy(), Some((FAST, FAST / 2)));
}

#[test]
fn later_failed_blocks_still_count_after_the_recovered_blocks_were_sampled() {
    let mut stream = Stream::new(true);
    for frame in [29, 30] {
        stream.report(frame, 30, Block::Recovered { lost: 1, data: 100 });
    }
    assert_eq!(stream.healthy(), None);
    // Distinct later blocks establish that both frames actually failed, even
    // though their initial recovered-block losses belonged to the last sample.
    for frame in [29, 30] {
        stream.report(frame, 60, Block::Unrecoverable);
    }
    assert_eq!(stream.healthy(), Some((FAST, FAST / 2)));
}

#[test]
fn distinct_out_of_order_failures_count_once_each() {
    let mut stream = Stream::new(true);
    stream.report(30, 30, Block::Unrecoverable);
    stream.report(29, 30, Block::Unrecoverable);
    stream.report(30, 30, Block::Unrecoverable);
    assert_eq!(stream.healthy(), Some((FAST, FAST / 2)));
}

#[test]
fn future_stale_and_pre_cut_reports_cannot_reduce_the_replacement_rate() {
    let mut stream = Stream::new(true);
    stream.last = 300;
    for frame in [301, 302, 1, 2] {
        stream.report(frame, 300, Block::Unrecoverable);
    }
    assert_eq!(stream.healthy(), None);
    assert_eq!(stream.failures(), Some((FAST, FAST / 2)));
    let fenced = stream.last;
    for frame in [fenced.wrapping_sub(1), fenced] {
        stream.report(frame, fenced, Block::Unrecoverable);
    }
    assert_eq!(stream.healthy(), None);
    assert_eq!(stream.failures(), Some((400_000_000, 200_000_000)));
}

#[test]
fn frame_fences_and_distinct_failures_work_across_u32_wrap() {
    let mut stream = Stream::new(true);
    stream.last = u32::MAX - 30;
    assert_eq!(stream.failures(), Some((FAST, FAST / 2)));
    assert_eq!(stream.last, u32::MAX);
    for frame in [u32::MAX - 1, u32::MAX] {
        stream.report(frame, 29, Block::Unrecoverable);
    }
    stream.report(0, 29, Block::Unrecoverable);
    stream.report(1, 29, Block::Unrecoverable);
    assert_eq!(stream.healthy(), Some((400_000_000, 200_000_000)));
}

#[test]
fn sparse_repaired_packets_do_not_ratchet_the_rate_down() {
    let mut stream = Stream::new(true);
    for _ in 0..60 {
        let end = stream.last + 30;
        for frame in [end - 1, end] {
            stream.report(frame, end, Block::Recovered { lost: 1, data: 100 });
        }
        assert_eq!(stream.healthy(), None);
    }
    assert_eq!(stream.pacing.rate(FAST, DEMAND, 40_000), FAST);
    let end = stream.last + 30;
    for frame in [end - 1, end] {
        stream.report(
            frame,
            end,
            Block::Recovered {
                lost: 20,
                data: 100,
            },
        );
    }
    assert_eq!(stream.healthy(), Some((FAST, FAST / 2)));
}

#[test]
fn host_send_failures_alone_do_not_guess_a_slower_downstream_link() {
    let mut stream = Stream::new(true);
    for _ in 0..20 {
        stream.traffic.send_losses += 100;
        assert_eq!(stream.healthy(), None);
    }
}

#[test]
fn reductions_follow_live_wire_demand_and_never_override_a_lower_ceiling() {
    let mut stream = Stream::new(true);
    for _ in 0..10 {
        stream.failures();
    }
    assert_eq!(stream.pacing.rate(FAST, DEMAND, 40_000), 62_500_000);
    // Increased FEC or changed FPS/packet size can increase wire demand while
    // encoder bitrate stays fixed. The next frame must immediately fit it.
    assert_eq!(stream.pacing.rate(FAST, 80_000_000, 40_000), 100_000_000);
    assert_eq!(stream.pacing.rate(FAST, 40_000_000, 40_000), 100_000_000);
    // A changed wire budget fences the old report epoch before accepting
    // fresh losses at the replacement rate.
    assert_eq!(stream.failures(), None);
    assert_eq!(stream.failures(), Some((100_000_000, 50_000_000)));
    assert_eq!(
        stream.pacing.rate(80_000_000, 120_000_000, 40_000),
        80_000_000
    );
    assert_eq!(stream.failures(), None);
    assert_eq!(stream.pacing.rate(80_000_000, u64::MAX, 40_000), 80_000_000);
}

#[test]
fn recovery_requires_fresh_valid_pings_busy_traffic_and_stable_rtt() {
    for condition in 0..6 {
        let mut stream = Stream::new(true);
        assert_eq!(stream.failures(), Some((FAST, FAST / 2)));
        for _ in 0..24 {
            let result = match condition {
                0 => stream.step(None, 30, 2_500_000, Duration::from_millis(2)),
                1 => stream.step(Some(&[3; 8]), 30, 2_500_000, Duration::from_millis(2)),
                2 => stream.step(Some(&PING), 1, 2_500_000, Duration::from_millis(2)),
                3 => stream.step(Some(&PING), 30, 1, Duration::from_millis(2)),
                4 => stream.step(Some(&PING), 30, 2_500_000, Duration::ZERO),
                _ => stream.step(Some(&PING), 30, 2_500_000, Duration::from_millis(50)),
            };
            assert_eq!(result, None, "unhealthy condition {condition}");
        }
        for _ in 0..16 {
            assert_eq!(stream.healthy(), None);
        }
        assert_eq!(stream.healthy(), Some((400_000_000, 420_000_000)));
    }
}

#[test]
fn a_late_control_sample_does_not_count_as_continuous_healthy_streaming() {
    let mut stream = Stream::new(true);
    assert_eq!(stream.failures(), Some((FAST, FAST / 2)));
    for _ in 0..14 {
        assert_eq!(stream.healthy(), None);
    }
    stream.now += Duration::from_secs(3);
    assert_eq!(stream.healthy(), None);
    for _ in 0..16 {
        assert_eq!(stream.healthy(), None);
    }
    assert_eq!(stream.healthy(), Some((400_000_000, 420_000_000)));
}

#[test]
fn a_probe_needs_two_continuous_healthy_seconds_before_another_increase() {
    let mut stream = Stream::new(true);
    stream.first_probe();
    assert_eq!(stream.healthy(), None);
    assert_eq!(stream.healthy(), None);
    // An idle sample breaks confirmation, even though wall-clock time since
    // starting the probe eventually exceeds two seconds.
    assert_eq!(
        stream.step(Some(&PING), 0, 0, Duration::from_millis(2)),
        None
    );
    for _ in 0..3 {
        assert_eq!(stream.healthy(), None);
    }
    assert!(stream.pacing.probe.is_some());
    for _ in 0..2 {
        assert_eq!(stream.healthy(), None);
    }
    assert!(stream.pacing.probe.is_none());
    for _ in 0..11 {
        assert_eq!(stream.healthy(), None);
    }
    assert_eq!(stream.healthy(), Some((420_000_000, 441_000_000)));
}

#[test]
fn even_sparse_new_loss_rolls_back_an_unconfirmed_probe() {
    let mut stream = Stream::new(true);
    stream.first_probe();
    let before_probe = stream.last;
    // Reports from the old pacing rate cannot invalidate the new probe.
    for frame in [before_probe - 1, before_probe] {
        stream.report(frame, before_probe, Block::Unrecoverable);
    }
    assert_eq!(stream.healthy(), None);
    let end = stream.last + 30;
    stream.report(end, end, Block::Recovered { lost: 1, data: 100 });
    assert_eq!(stream.healthy(), Some((420_000_000, 400_000_000)));
    assert_eq!(stream.pacing.snapshot()["recovery_hold_seconds"], 16);
}

#[test]
fn failed_probes_restore_the_working_rate_and_back_off_to_a_one_minute_hold() {
    let mut stream = Stream::new(true);
    stream.first_probe();
    for hold_seconds in [16, 32, 60, 60] {
        assert_eq!(stream.failures(), Some((420_000_000, 400_000_000)));
        assert_eq!(
            stream.pacing.snapshot()["recovery_hold_seconds"],
            hold_seconds
        );
        for _ in 0..hold_seconds * 2 {
            assert_eq!(stream.healthy(), None);
        }
        assert_eq!(stream.healthy(), Some((400_000_000, 420_000_000)));
    }
}
