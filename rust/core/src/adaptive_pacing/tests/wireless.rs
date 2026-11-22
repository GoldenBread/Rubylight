//! Tests for unknown-link wireless burst pacing step-down and idle hold behavior.

use super::*;

#[test]
fn wireless_unknown_link_steps_down_when_headroom_clears_while_wired_stays_put() {
    let cap = network_pacing::wireless_unknown_link_ceiling_bps(FAST, 40_000, DEMAND);
    assert_eq!(cap, 160_000_000);

    // 1) Sustained queue delay steps elevated wireless unknown-link pacing back down
    // toward SEED, re-arming the 1s hold after each step so brief RTT spikes only take one step.
    let mut queued_stream = Stream::fresh(true);
    queued_stream.pacing.set_wireless_unknown_link(true);
    assert_eq!(queued_stream.pacing.rate(cap, DEMAND, 40_000), SEED);
    for _ in 0..24 {
        queued_stream.tick_step(Some(&PING), 30, 2_500_000, Duration::from_millis(2));
    }
    assert_eq!(queued_stream.pacing.rate(cap, DEMAND, 40_000), cap);
    // Two ticks (< 1s hold) do not jump immediately; from 1s on it steps down once per second.
    let mut queue_steps = Vec::new();
    let mut step_ticks = Vec::new();
    for tick in 0..16 {
        if let Some(step) =
            queued_stream.tick_step(Some(&PING), 30, 2_500_000, Duration::from_millis(30))
        {
            queue_steps.push(step);
            step_ticks.push(tick);
        }
    }
    assert_eq!(queue_steps.len(), 6, "{queue_steps:?}");
    assert_eq!(step_ticks, vec![2, 4, 6, 8, 10, 12]);
    assert_eq!(queue_steps[0].reason, PacingReason::QueueDelay);
    assert!(queue_steps[0].reason_changed);
    assert!(!queue_steps[1].reason_changed);
    assert!(queue_steps[0].new_bps < queue_steps[0].old_bps);
    assert_eq!(queued_stream.pacing.rate(cap, DEMAND, 40_000), SEED);

    // 1b) A brief RTT spike (~1.5s) takes only one 12.5% step rather than collapsing to SEED.
    let mut brief_spike = Stream::fresh(true);
    brief_spike.pacing.set_wireless_unknown_link(true);
    brief_spike.pacing.rate(cap, DEMAND, 40_000);
    for _ in 0..24 {
        brief_spike.tick_step(Some(&PING), 30, 2_500_000, Duration::from_millis(2));
    }
    let mut spike_steps = Vec::new();
    for _ in 0..4 {
        if let Some(step) =
            brief_spike.tick_step(Some(&PING), 30, 2_500_000, Duration::from_millis(30))
        {
            spike_steps.push(step);
        }
    }
    // RTT returns to normal; no further step-down occurs.
    for _ in 0..4 {
        assert_eq!(
            brief_spike.tick_step(Some(&PING), 30, 2_500_000, Duration::from_millis(2)),
            None
        );
    }
    assert_eq!(spike_steps.len(), 1, "{spike_steps:?}");
    assert_eq!(spike_steps[0].reason, PacingReason::QueueDelay);
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
