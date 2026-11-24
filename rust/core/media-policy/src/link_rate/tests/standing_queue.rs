//! Tests for lossless standing-queue detection, RTT floors, and competing flows.

use super::*;

// ---- standing queue (delay signal) ----

/// A link that carries `link_kbps` of the encoder's rate, as a queue: it grows
/// by the excess over the link every 100 ms and drains by the same below it.
/// The RTT is 20 ms plus the queue, smoothed over about 1 s like the ENet
/// round trip. Returns the largest queue, in ms.
fn queue_for(rig: &mut Rig, link_kbps: u32, ms: u64, queue: &mut f64) -> f64 {
    let mut largest = 0f64;
    let mut shown = *queue;
    for _ in 0..ms / 100 {
        *queue =
            (*queue + (f64::from(rig.kbps) / f64::from(link_kbps) - 1.) * 100.).clamp(0., 300.);
        largest = largest.max(*queue);
        shown += (*queue - shown) * 0.1;
        rig.rtt = Duration::from_millis(20 + shown as u64);
        rig.run(100, |_, _, _| {});
    }
    largest
}

fn queue_changes(rig: &Rig) -> Vec<(u64, Change)> {
    rig.changes
        .iter()
        .filter(|(_, c)| c.reason == "queue")
        .copied()
        .collect()
}

/// The 2026-10-24 r50-hevc-hi run held 36.8 Mbps into 50 Mbit/s with a 25 ms
/// queue and no loss, p50 48 ms: a probe step went over the link, the queue
/// grew without a lost packet, and probes stop while the RTT is up. No loss
/// evidence exists to cut on; the RTT staying over its floor does.
#[test]
fn a_lossless_standing_queue_after_a_probe_is_cut_and_the_rtt_returns_to_the_floor() {
    let mut rig = Rig::new(40_000);
    rig.rtt_jitter_ms = 3;
    rig.clean(3_000);
    rig.lose_three();
    let mut queue = 0.;
    let largest = queue_for(&mut rig, 30_000, 60_000, &mut queue);
    let cuts = queue_changes(&rig);
    assert!(!cuts.is_empty(), "{:?}", rig.changes);
    // 15 ms of queue and two seconds of it, then 12 %; never the 300 ms an
    // uncut probe would grow to.
    assert!(
        largest < 200.,
        "the queue reached {largest} ms: {:?}",
        rig.changes
    );
    assert!(cuts.len() <= 6, "{} queue cuts", cuts.len());
    let low = cuts.iter().map(|(_, c)| c.to_kbps).min().unwrap();
    assert!(low >= 30_000 * 60 / 100, "cut down to {low} kbps");
    assert!(rig.kbps <= 31_500, "{} kbps over a 30,000 link", rig.kbps);
}

/// Spikes and short excursions on the wifi arm, in the seconds after a probe,
/// are no queue: a dip under the margin starts the wait over, and 2 s over it
/// are needed.
#[test]
fn wifi_rtt_spikes_and_excursions_under_two_seconds_are_no_queue() {
    for parity in [7u16, 10, 13, 16] {
        let mut rig = Rig::new(14_988);
        rig.rtt_jitter_ms = 8;
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        rig.run(5_000, |lr, frame, now| {
            wifi_block(&mut seed, lr, frame, parity, 5, 0, now);
        });
        rig.lose_three();
        // 700 ms up to 60 ms, 700 ms back, while it probes back up.
        for i in 0..42 {
            rig.rtt = Duration::from_millis(if i % 2 == 0 { 60 } else { 20 });
            rig.run(700, |lr, frame, now| {
                wifi_block(&mut seed, lr, frame, parity, 5, 0, now);
            });
        }
        // 1.8 s up, five times, with 4 s of calm between.
        for _ in 0..5 {
            rig.rtt = Duration::from_millis(60);
            rig.run(1_800, |lr, frame, now| {
                wifi_block(&mut seed, lr, frame, parity, 5, 0, now);
            });
            rig.rtt = Duration::from_millis(20);
            rig.run(4_000, |lr, frame, now| {
                wifi_block(&mut seed, lr, frame, parity, 5, 0, now);
            });
        }
        assert!(
            queue_changes(&rig).is_empty(),
            "parity {parity}: {:?}",
            rig.changes
        );
    }
}

/// A route that got slower for good (RTT 20 to 50 ms, no queue, whatever the
/// rate): with no probe in the last 8 s it is no queue at all. Coinciding with
/// a probe it costs one step back: the cut changes nothing, so the second look
/// calls it no queue and re-learns the floor, and probes climb back. A slow
/// growth is tracked by the 30 s floor and costs nothing.
#[test]
fn a_route_change_costs_at_most_one_step_and_a_slow_rtt_growth_nothing() {
    // Long after any probe.
    let mut rig = Rig::new(40_000);
    rig.clean(3_000);
    rig.lose_three();
    rig.clean(60_000);
    assert_eq!(rig.lr.state(), State::Full);
    let before = rig.changes.len();
    rig.rtt = Duration::from_millis(50);
    rig.clean(30_000);
    assert!(rig.changes[before..].is_empty(), "{:?}", rig.changes);

    // Right after a probe.
    let mut rig = Rig::new(40_000);
    rig.clean(3_000);
    rig.lose_three();
    rig.until_probe();
    rig.rtt = Duration::from_millis(50);
    rig.clean(30_000);
    let cuts = queue_changes(&rig);
    assert_eq!(cuts.len(), 1, "{:?}", rig.changes);
    assert_eq!(cuts[0].1.kind, ChangeKind::ProbeFailed);
    // `minimum_rtt` is not re-learned, so `rtt_elevated` holds probes for as
    // long as the route is slower (as it did before the delay signal): the
    // cost of the route change is the one step, kept.
    let (low, probes) = (rig.kbps, rig.probes());
    rig.clean(120_000);
    assert_eq!(queue_changes(&rig).len(), 1, "{:?}", rig.changes);
    assert_eq!((rig.kbps, rig.probes()), (low, probes), "{:?}", rig.changes);

    // 0.3 ms a second for a minute and a half, probing all the while.
    let mut rig = Rig::new(40_000);
    rig.clean(3_000);
    rig.lose_three();
    for second in 0..90u64 {
        rig.rtt = Duration::from_millis(20 + second * 3 / 10);
        rig.clean(1_000);
    }
    assert!(queue_changes(&rig).is_empty(), "{:?}", rig.changes);
}

/// Another flow keeps the queue growing whatever the stream sends: three
/// cuts (about a third of the rate) are the most it takes, probes stay held
/// while the RTT is up, and the queue floor is re-learned so it is not cut
/// for the same queue again.
#[test]
fn a_competing_flow_that_keeps_the_queue_rising_gets_at_most_three_cuts_and_no_probes() {
    let mut rig = Rig::new(40_000);
    rig.rtt_jitter_ms = 3;
    rig.clean(3_000);
    rig.lose_three();
    rig.until_probe();
    // 10 ms more a second for 90 s, up to 300 ms, regardless of the rate.
    for second in 0..90u64 {
        rig.rtt = Duration::from_millis(20 + (second * 10).min(280));
        rig.clean(1_000);
    }
    let cuts = queue_changes(&rig);
    assert!(
        (1..=3).contains(&cuts.len()),
        "{} cuts: {:?}",
        cuts.len(),
        rig.changes
    );
    let last_cut = rig
        .changes
        .iter()
        .rposition(|(_, c)| c.reason == "queue")
        .unwrap();
    assert!(
        rig.changes[last_cut + 1..]
            .iter()
            .all(|(_, c)| c.kind != ChangeKind::Probe),
        "{:?}",
        rig.changes
    );
    // Each cut is 12 % at most, and there are no more than two after the first
    // (which only takes the probe back).
    assert!(
        u64::from(cuts[cuts.len() - 1].1.to_kbps) * 10_000
            >= u64::from(cuts[0].1.to_kbps) * 88 * 88 * 97 / 100,
        "{:?}",
        rig.changes
    );
}

/// A Wi-Fi stall of 2.5 to 4 s that starts just after a probe step has the
/// RTT up for longer than the hold: it costs one 12 % cut (the probe goes
/// back to the last good rate) and the knee hold keeps probes under the rate
/// that failed for `KNEE_QUIET`. The bound and the price: at most one cut a stall, the
/// rate lower for 20 s. Not a loop: the RTT is back by the next look.
#[test]
fn a_wifi_stall_of_a_few_seconds_after_a_probe_costs_one_cut_and_the_knee_hold() {
    for stall_ms in [2_500u64, 3_000, 4_000] {
        let mut rig = Rig::new(40_000);
        rig.rtt_jitter_ms = 8;
        rig.clean(3_000);
        rig.lose_three();
        let probe = rig.until_probe();
        rig.clean(1_000);
        let before = rig.changes.len();
        rig.rtt = Duration::from_millis(80);
        rig.clean(stall_ms);
        rig.rtt = Duration::from_millis(20);
        rig.clean(1_000);
        let cuts: Vec<_> = rig.changes[before..]
            .iter()
            .filter(|(_, c)| c.reason == "queue")
            .copied()
            .collect();
        assert_eq!(cuts.len(), 1, "stall {stall_ms}: {:?}", rig.changes);
        assert_eq!(cuts[0].1.kind, ChangeKind::ProbeFailed);
        assert!(cuts[0].1.to_kbps >= probe.to_kbps * 88 / 100 - 1);
        // The knee hold: for KNEE_QUIET no probe reaches the rate that failed (it may climb to just under it).
        let (after, probes) = (rig.changes.len(), rig.probes());
        rig.clean(KNEE_QUIET.as_millis() as u64 - 2_000);
        let knee = cuts[0].1.from_kbps;
        for (at, c) in &rig.changes[after..] {
            assert!(
                c.kind != ChangeKind::Probe || u64::from(c.to_kbps) < u64::from(knee),
                "stall {stall_ms}: probed to {} at {at} (knee {knee})",
                c.to_kbps
            );
        }
        // And it climbs again afterwards.
        rig.clean(60_000);
        assert!(rig.probes() > probes, "stall {stall_ms}");
        assert_eq!(queue_changes(&rig).len(), 1, "{:?}", rig.changes);
    }
}

/// A deep buffer and a link that cannot carry the rate: the first cut is on
/// loss (the queue is full); after that every probe past the link builds a
/// queue that is cut on its RTT, and no packet is lost for it.
#[test]
fn a_deep_buffer_link_settles_with_a_short_queue() {
    let mut sim = Sim::new(q(Link::constant(50.), 400.), 60_000, 20.);
    sim.run(90.);
    let queue_cuts = sim
        .changes
        .iter()
        .filter(|(_, c)| c.reason == "queue")
        .count();
    assert!(
        queue_cuts >= 1,
        "{:?}",
        sim.changes
            .iter()
            .map(|(t, c)| (*t, c.reason))
            .collect::<Vec<_>>()
    );
    // Settled: sample the queue in the last 20 s.
    let mut late: Vec<f64> = sim
        .queue_log
        .iter()
        .filter(|(t, _)| *t >= 70.)
        .map(|(_, ms)| *ms)
        .collect();
    late.sort_by(f64::total_cmp);
    let worst = late[late.len() * 9 / 10];
    assert!(worst < 120., "queue {worst:.0} ms");
}

/// The 5 s timings line logs these two to read a delay-signal run back: the
/// smoothed RTT and the lowest RTT of the last 30 s the signal compares with.
#[test]
fn the_snapshot_carries_the_rtt_and_its_30_second_floor() {
    let mut rig = Rig::new(40_000);
    rig.clean(5_000);
    rig.rtt = Duration::from_millis(45);
    rig.clean(1_500);
    let snapshot = rig.lr.snapshot(rig.now());
    assert_eq!(snapshot["rtt_floor_ms"], 20);
    assert_eq!(snapshot["rtt_ms"], 45);
}
