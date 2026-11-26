//! Tests for unknown-link wireless burst pacing step-down and idle hold behavior.

use super::*;

#[test]
fn wireless_unknown_link_steps_down_when_headroom_clears_while_wired_stays_put() {
    let cap = network_pacing::wireless_unknown_link_ceiling_bps(FAST, 40_000, DEMAND);
    assert_eq!(cap, 160_000_000);

    // 1) Sustained queue delay steps elevated wireless unknown-link pacing back down
    // toward SEED, re-arming the 2s hold after each step.
    let mut queued_stream = Stream::fresh(true);
    queued_stream.pacing.set_wireless_unknown_link(true);
    assert_eq!(queued_stream.pacing.rate(cap, DEMAND, 40_000), SEED);
    for _ in 0..24 {
        queued_stream.tick_step(Some(&PING), 30, 2_500_000, Duration::from_millis(2));
    }
    assert_eq!(queued_stream.pacing.rate(cap, DEMAND, 40_000), cap);
    // Under 2s hold does not jump immediately; from 2s on it steps down every 2s.
    let mut queue_steps = Vec::new();
    let mut step_ticks = Vec::new();
    for tick in 0..28 {
        if let Some(step) =
            queued_stream.tick_step(Some(&PING), 30, 2_500_000, Duration::from_millis(30))
        {
            queue_steps.push(step);
            step_ticks.push(tick);
        }
    }
    assert_eq!(queue_steps.len(), 6, "{queue_steps:?}");
    assert_eq!(step_ticks, vec![4, 8, 12, 16, 20, 24]);
    assert_eq!(queue_steps[0].reason, PacingReason::QueueDelay);
    assert!(queue_steps[0].reason_changed);
    assert!(!queue_steps[1].reason_changed);
    assert!(queue_steps[0].new_bps < queue_steps[0].old_bps);
    assert_eq!(queued_stream.pacing.rate(cap, DEMAND, 40_000), SEED);

    // 1b) A brief RTT excursion (< 2s) does not step down; a 2.5s stall followed by
    // ENet's 7/8 SRTT decay takes only one 12.5% step instead of collapsing to SEED.
    let mut brief_spike = Stream::fresh(true);
    brief_spike.pacing.set_wireless_unknown_link(true);
    brief_spike.pacing.rate(cap, DEMAND, 40_000);
    for _ in 0..24 {
        brief_spike.tick_step(Some(&PING), 30, 2_500_000, Duration::from_millis(2));
    }
    for _ in 0..3 {
        assert_eq!(
            brief_spike.tick_step(Some(&PING), 30, 2_500_000, Duration::from_millis(30)),
            None
        );
    }
    for _ in 0..4 {
        assert_eq!(
            brief_spike.tick_step(Some(&PING), 30, 2_500_000, Duration::from_millis(2)),
            None
        );
    }
    assert_eq!(brief_spike.pacing.rate(cap, DEMAND, 40_000), cap);

    let mut stall_steps = Vec::new();
    for _ in 0..5 {
        if let Some(step) =
            brief_spike.tick_step(Some(&PING), 30, 2_500_000, Duration::from_millis(60))
        {
            stall_steps.push(step);
        }
    }
    let mut srtt_ms = 60u64;
    for _ in 0..16 {
        srtt_ms = (srtt_ms * 7 + 2) / 8;
        assert_eq!(
            brief_spike.tick_step(Some(&PING), 30, 2_500_000, Duration::from_millis(srtt_ms)),
            None,
            "decay tail at {srtt_ms} ms must not keep stepping down"
        );
    }
    assert_eq!(stall_steps.len(), 1, "{stall_steps:?}");
    assert_eq!(stall_steps[0].reason, PacingReason::QueueDelay);
    assert_eq!(brief_spike.pacing.rate(cap, DEMAND, 40_000), cap - cap / 8);

    // 2) Quiet/low-bitrate traffic (!busy) with fresh pings and normal RTT holds the current
    // pacing rate without stepping down or probing higher.
    let mut quiet_stream = Stream::fresh(true);
    quiet_stream.pacing.set_wireless_unknown_link(true);
    quiet_stream.pacing.rate(cap, DEMAND, 40_000);
    for _ in 0..24 {
        quiet_stream.tick_step(Some(&PING), 30, 2_500_000, Duration::from_millis(2));
    }
    assert_eq!(quiet_stream.pacing.rate(cap, DEMAND, 40_000), cap);
    for _ in 0..20 {
        assert_eq!(
            quiet_stream.tick_step(Some(&PING), 1, 1_000, Duration::from_millis(2)),
            None
        );
    }
    assert_eq!(quiet_stream.pacing.rate(cap, DEMAND, 40_000), cap);

    // Mid-climb quiet ticks neither step down nor probe upward.
    let mut mid_quiet = Stream::fresh(true);
    mid_quiet.pacing.set_wireless_unknown_link(true);
    mid_quiet.pacing.rate(cap, DEMAND, 40_000);
    for _ in 0..4 {
        mid_quiet.tick_step(Some(&PING), 30, 2_500_000, Duration::from_millis(2));
    }
    let mid_rate = mid_quiet.pacing.rate(cap, DEMAND, 40_000);
    assert!(mid_rate > SEED && mid_rate < cap);
    for _ in 0..10 {
        assert_eq!(
            mid_quiet.tick_step(Some(&PING), 1, 1_000, Duration::from_millis(2)),
            None
        );
    }
    assert_eq!(mid_quiet.pacing.rate(cap, DEMAND, 40_000), mid_rate);

    // 2b) HeadroomCleared (stale control pings or zero RTT) steps back down every 2s toward SEED.
    let mut stale_ping = Stream::fresh(true);
    stale_ping.pacing.set_wireless_unknown_link(true);
    stale_ping.pacing.rate(cap, DEMAND, 40_000);
    for _ in 0..24 {
        stale_ping.tick_step(Some(&PING), 30, 2_500_000, Duration::from_millis(2));
    }
    assert_eq!(stale_ping.pacing.rate(cap, DEMAND, 40_000), cap);
    let mut stale_steps = Vec::new();
    let mut stale_ticks = Vec::new();
    for tick in 0..30 {
        if let Some(step) = stale_ping.tick_step(None, 30, 2_500_000, Duration::from_millis(2)) {
            stale_steps.push(step);
            stale_ticks.push(tick);
        }
    }
    assert_eq!(stale_steps.len(), 6, "{stale_steps:?}");
    assert_eq!(stale_ticks, vec![7, 11, 15, 19, 23, 27]);
    assert_eq!(stale_steps[0].reason, PacingReason::HeadroomCleared);
    assert!(stale_steps[0].reason_changed);
    assert!(!stale_steps[1].reason_changed);
    assert_eq!(stale_ping.pacing.rate(cap, DEMAND, 40_000), SEED);

    let mut zero_rtt = Stream::fresh(true);
    zero_rtt.pacing.set_wireless_unknown_link(true);
    zero_rtt.pacing.rate(cap, DEMAND, 40_000);
    for _ in 0..24 {
        zero_rtt.tick_step(Some(&PING), 30, 2_500_000, Duration::from_millis(2));
    }
    let mut zero_rtt_steps = Vec::new();
    for _ in 0..6 {
        if let Some(step) = zero_rtt.tick_step(Some(&PING), 30, 2_500_000, Duration::ZERO) {
            zero_rtt_steps.push(step);
        }
    }
    assert_eq!(zero_rtt_steps.len(), 1, "{zero_rtt_steps:?}");
    assert_eq!(zero_rtt_steps[0].reason, PacingReason::HeadroomCleared);
    assert_eq!(zero_rtt.pacing.rate(cap, DEMAND, 40_000), cap - cap / 8);

    // 3) Minor send loss steps back down immediately without halving.
    let mut minor_loss = Stream::fresh(true);
    minor_loss.pacing.set_wireless_unknown_link(true);
    minor_loss.pacing.rate(cap, DEMAND, 40_000);
    for _ in 0..24 {
        minor_loss.tick_step(Some(&PING), 30, 2_500_000, Duration::from_millis(2));
    }
    minor_loss.traffic.send_losses += 1;
    let loss_step = minor_loss
        .tick_step(Some(&PING), 30, 2_500_000, Duration::from_millis(2))
        .expect("minor loss step down");
    assert_eq!(loss_step.reason, PacingReason::MinorLoss);
    assert!(loss_step.reason_changed);
    assert_eq!(loss_step.new_bps, cap - cap / 8);

    // 4) Wired / known-link stream stays completely unchanged under idle, queue, or send losses.
    let mut wired = Stream::new(true);
    assert!(!wired.pacing.wireless_unknown_link);
    for _ in 0..20 {
        wired.traffic.send_losses += 1;
        assert_eq!(
            wired.tick_step(Some(&PING), 1, 1_000, Duration::from_millis(50)),
            None
        );
    }
    assert_eq!(wired.pacing.rate(FAST, DEMAND, 40_000), FAST);
}

#[test]
fn wireless_unknown_link_ignores_enet_blip_decay_idle_spikes_and_ordinary_jitter() {
    let cap = network_pacing::wireless_unknown_link_ceiling_bps(FAST, 40_000, DEMAND);
    let warmed = || {
        let mut stream = Stream::fresh(true);
        stream.pacing.set_wireless_unknown_link(true);
        stream.pacing.rate(cap, DEMAND, 40_000);
        for _ in 0..24 {
            stream.tick_step(Some(&PING), 30, 2_500_000, Duration::from_millis(2));
        }
        assert_eq!(stream.pacing.rate(cap, DEMAND, 40_000), cap);
        stream
    };

    // Single 294 ms Wi-Fi blip decaying through ENet's round_up_srtt EWMA must not step down.
    let mut blip = warmed();
    let mut srtt_ms = 2u64 + (294u64 - 2).div_ceil(8); // 39 ms
    for _ in 0..24 {
        assert_eq!(
            blip.tick_step(Some(&PING), 30, 2_500_000, Duration::from_millis(srtt_ms)),
            None,
            "single blip decay at {srtt_ms} ms must not step pacing down"
        );
        srtt_ms = srtt_ms.saturating_sub(srtt_ms.saturating_sub(2).div_ceil(8));
    }
    assert_eq!(blip.pacing.rate(cap, DEMAND, 40_000), cap);

    // Non-floor post-spike decay toward ordinary 14 ms Wi-Fi jitter at 1 s ping cadence
    // (dropping only 1-2 ms per second) must not step down while excess keeps falling.
    let mut jitter_decay = warmed();
    for &ms in &[23u64, 23, 21, 21, 20, 20, 19, 19, 18, 18, 17, 16] {
        assert_eq!(
            jitter_decay.tick_step(Some(&PING), 30, 2_500_000, Duration::from_millis(ms)),
            None,
            "non-floor decay at {ms} ms must not step pacing down"
        );
    }
    assert_eq!(jitter_decay.pacing.rate(cap, DEMAND, 40_000), cap);

    // Ordinary Wi-Fi jitter (<= 15 ms over floor) pauses probes without stepping down.
    let mut jitter = warmed();
    for _ in 0..20 {
        assert_eq!(
            jitter.tick_step(Some(&PING), 30, 2_500_000, Duration::from_millis(14)),
            None
        );
    }
    assert_eq!(jitter.pacing.rate(cap, DEMAND, 40_000), cap);

    // Standing still / quiet traffic (!busy) with high RTT never triggers QueueDelay and
    // does not pre-charge the 2 s hold before active video resumes.
    let mut quiet_spike = warmed();
    for _ in 0..20 {
        assert_eq!(
            quiet_spike.tick_step(Some(&PING), 30, 250_000, Duration::from_millis(50)),
            None
        );
    }
    assert_eq!(quiet_spike.pacing.rate(cap, DEMAND, 40_000), cap);
    for tick in 0..4 {
        assert_eq!(
            quiet_spike.tick_step(Some(&PING), 30, 2_500_000, Duration::from_millis(50)),
            None,
            "tick {tick} after quiet scene must wait full 2s busy hold"
        );
    }
    let step = quiet_spike
        .tick_step(Some(&PING), 30, 2_500_000, Duration::from_millis(50))
        .expect("sustained busy queue after 2s must step down");
    assert_eq!(step.reason, PacingReason::QueueDelay);
    assert_eq!(step.new_bps, cap - cap / 8);
}

#[test]
fn wireless_unknown_link_recovers_after_lasting_route_rtt_shift() {
    let cap = network_pacing::wireless_unknown_link_ceiling_bps(FAST, 40_000, DEMAND);
    let mut stream = Stream::fresh(true);
    stream.pacing.set_wireless_unknown_link(true);
    stream.pacing.rate(cap, DEMAND, 40_000);
    for _ in 0..24 {
        stream.tick_step(Some(&PING), 30, 2_500_000, Duration::from_millis(2));
    }
    assert_eq!(stream.pacing.rate(cap, DEMAND, 40_000), cap);

    // Route latency shifts permanently from 2 ms to 25 ms with no loss.
    for _ in 0..28 {
        stream.tick_step(Some(&PING), 30, 2_500_000, Duration::from_millis(25));
    }
    assert_eq!(stream.pacing.rate(cap, DEMAND, 40_000), SEED);

    // Once the 30 s rolling floor forgets the old 2 ms baseline, recovery probing resumes
    // and climbs back to the wireless ceiling instead of locking at SEED forever.
    let mut recovered = false;
    for _ in 0..160 {
        if let Some(step) = stream.tick_step(Some(&PING), 30, 2_500_000, Duration::from_millis(25))
            && step.reason == PacingReason::RecoveryProbe
        {
            recovered = true;
        }
    }
    assert!(
        recovered,
        "stream must resume probing after 30 s floor adapts"
    );
    assert_eq!(stream.pacing.rate(cap, DEMAND, 40_000), cap);
}

#[test]
fn wireless_unknown_link_bitrate_changes_keep_rtt_floor_and_queue_watch() {
    let mut stream = Stream::fresh(true);
    stream.pacing.set_wireless_unknown_link(true);
    let cap_40 = network_pacing::wireless_unknown_link_ceiling_bps(FAST, 40_000, DEMAND);
    assert_eq!(stream.pacing.rate(FAST, DEMAND, 40_000), SEED);
    for _ in 0..24 {
        stream.tick_step(Some(&PING), 30, 2_500_000, Duration::from_millis(2));
    }
    assert_eq!(stream.pacing.rate(FAST, DEMAND, 40_000), cap_40);

    // Begin a standing queue at 45 ms (1.0 s elapsed on ticks 0..2).
    for _ in 0..3 {
        assert_eq!(
            stream.tick_step(Some(&PING), 30, 2_500_000, Duration::from_millis(45)),
            None
        );
    }

    // Auto-bitrate lowers encoder_kbps on the SAME host route (FAST unchanged).
    // preview_rate must agree with rate, and rtt_floor / queue_watch must survive.
    let cap_34 = network_pacing::wireless_unknown_link_ceiling_bps(FAST, 34_000, DEMAND);
    assert_eq!(stream.pacing.preview_rate(FAST, DEMAND, 34_000), cap_34);
    assert_eq!(stream.pacing.rate(FAST, DEMAND, 34_000), cap_34);
    assert_eq!(stream.pacing.rtt_floor(), Some(Duration::from_millis(2)));

    // Next sample re-establishes the post-change traffic window; the following tick
    // completes the 2 s queue hold from before the bitrate change and steps down.
    let mut stepped = None;
    for _ in 0..2 {
        stream.now += Duration::from_millis(500);
        stream.traffic.frames += 30;
        stream.traffic.bytes += 2_200_000;
        stream.traffic.packets += 2_000;
        stream.last = stream.last.wrapping_add(30);
        stream.pacing.on_ping(stream.now, &PING);
        if let Some(step) = stream.pacing.tick_step(
            stream.now,
            stream.traffic,
            Some(stream.last),
            stream.period,
            34_000,
            Duration::from_millis(45),
        ) {
            stepped = Some(step);
            break;
        }
    }
    let step = stepped.expect("bitrate change on same route must not blind QueueDelay");
    assert_eq!(step.reason, PacingReason::QueueDelay);
    assert_eq!(step.old_bps, cap_34);
    assert_eq!(step.new_bps, cap_34 - cap_34 / 8);
}

#[test]
fn wireless_unknown_link_preserves_hold_across_queue_and_stale_ping_transitions() {
    let cap = network_pacing::wireless_unknown_link_ceiling_bps(FAST, 40_000, DEMAND);
    let warmed = || {
        let mut stream = Stream::fresh(true);
        stream.pacing.set_wireless_unknown_link(true);
        stream.pacing.rate(FAST, DEMAND, 40_000);
        for _ in 0..24 {
            stream.tick_step(Some(&PING), 30, 2_500_000, Duration::from_millis(2));
        }
        assert_eq!(stream.pacing.rate(FAST, DEMAND, 40_000), cap);
        stream
    };

    // 1) QueueDelay interrupted at 2.0 s by channel-0 ping staleness steps down
    // immediately as HeadroomCleared instead of restarting a fresh 2 s hold.
    let mut queue_to_stale = warmed();
    assert_eq!(
        queue_to_stale.tick_step(Some(&PING), 30, 2_500_000, Duration::from_millis(60)),
        None
    );
    for _ in 0..3 {
        assert_eq!(
            queue_to_stale.tick_step(None, 30, 2_500_000, Duration::from_millis(60)),
            None
        );
    }
    let step = queue_to_stale
        .tick_step(None, 30, 2_500_000, Duration::from_millis(60))
        .expect("stale ping at 2.0 s must carry QueueDelay hold");
    assert_eq!(step.reason, PacingReason::HeadroomCleared);
    assert_eq!(step.new_bps, cap - cap / 8);

    // 2) HeadroomCleared followed directly by a delayed high-RTT ping carries its
    // elapsed hold into QueueDelay so recurring ~2 s stalls still step down.
    let mut stale_to_queue = warmed();
    // 4 ticks without pings bring last_ping to 2.0 s (starts HeadroomCleared on tick 3),
    // then 3 more ticks reach 1.5 s of HeadroomCleared hold before a delayed ping arrives.
    for _ in 0..7 {
        assert_eq!(
            stale_to_queue.tick_step(None, 30, 2_500_000, Duration::from_millis(2)),
            None
        );
    }
    let step = stale_to_queue
        .tick_step(Some(&PING), 30, 2_500_000, Duration::from_millis(120))
        .expect("resumed high-RTT ping must carry HeadroomCleared hold into QueueDelay");
    assert_eq!(step.reason, PacingReason::QueueDelay);
    assert_eq!(step.new_bps, cap - cap / 8);
}
