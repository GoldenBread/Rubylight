//! Tests for faster-source rate control, host send loss, and test send-fault injectors.

use super::*;

/// Claims (ms, true present ms) for a source pacing on the given stamps, as
/// the host paces WGC: `arrival` stamps each frame when it was detected, as
/// `qpc_timestamps_at` does for the many WGC stamps still in the future.
fn simulate_stamped(period: f64, interval: f64, seconds: f64, arrival: bool) -> Vec<(f64, f64)> {
    let start = Instant::now();
    let at = |ms: f64| start + Duration::from_secs_f64(ms / 1000.);
    let ms = |instant: Instant| instant.duration_since(start).as_secs_f64() * 1000.;
    let mut pacer = Pacer::new(start, Duration::from_secs_f64(period / 1000.))
        .with_prediction(true)
        .with_source_phase(true)
        .with_spacing(0.75);
    let mut seed = 42u64;
    let mut jitter = || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 11) as f64 / (1u64 << 53) as f64
    };
    let present = source(interval, seconds, |_| 0.);
    // Detected 0.1-1.1 ms after presentation, as the polled WGC helper.
    let detected: Vec<f64> = present.iter().map(|p| p + 0.1 + jitter()).collect();
    let stamp = if arrival { &detected } else { &present };
    let mut claims = vec![];
    let mut newest: Option<usize> = None;
    let mut deadline: Option<f64> = None;
    let mut source_interval: Option<Duration> = None;
    let mut evaluate = |now: f64,
                        newest: &mut Option<usize>,
                        deadline: &mut Option<f64>,
                        pacer: &mut Pacer,
                        source_interval: Option<Duration>| {
        if let Some(i) = *newest {
            pacer.observe_source(at(stamp[i]));
            match pacer.decide(at(now), at(stamp[i]), source_interval) {
                Pace::Claim => {
                    pacer.claimed(at(now));
                    claims.push((now, present[i]));
                    *newest = None;
                    *deadline = None;
                }
                Pace::WaitUntil(t) => *deadline = Some(ms(t).max(now + 0.001)),
            }
        }
    };
    for i in 0..present.len() {
        while let Some(t) = deadline.filter(|t| *t < detected[i]) {
            evaluate(t, &mut newest, &mut deadline, &mut pacer, source_interval);
            if deadline == Some(t) {
                break;
            }
        }
        let recent = &stamp[i.saturating_sub(16)..=i];
        let mut intervals: Vec<f64> = recent.windows(2).map(|w| w[1] - w[0]).collect();
        intervals.sort_by(f64::total_cmp);
        source_interval = (intervals.len() >= 4)
            .then(|| Duration::from_secs_f64(intervals[intervals.len() / 2] / 1000.));
        newest = Some(i);
        evaluate(
            detected[i],
            &mut newest,
            &mut deadline,
            &mut pacer,
            source_interval,
        );
    }
    claims
}
#[test]
fn a_faster_source_is_claimed_at_the_stream_rate_not_above_it() {
    // A 165 Hz game, and the default 2x virtual display, at 120 and 60 fps.
    // The 1% refill margin used to set the claim rate: 121.2 and 60.6 a
    // second, as measured on the host.
    for (period, interval, limit) in [
        (PERIOD, 1000. / 165., 120.2),
        (PERIOD, 1000. / 240., 120.2),
        (1000. / 60., 1000. / 120., 60.1),
    ] {
        for arrival in [false, true] {
            let claims = simulate_stamped(period, interval, 30., arrival);
            let rate = steady_rate(&claims);
            assert!(
                rate <= limit,
                "{rate:.2} claims/s for a {:.0} Hz source (arrival stamps: {arrival})",
                1000. / interval
            );
        }
        if interval * 2. != period {
            continue;
        }
        // On the 2x display, every other source frame lands on the claim.
        let exact = steady_age_p99(&simulate_stamped(period, interval, 30., false));
        let arrival = steady_age_p99(&simulate_stamped(period, interval, 30., true));
        assert!(
            arrival <= exact + 0.2,
            "picture age p99 {arrival:.2} ms against {exact:.2} ms"
        );
    }
}

#[test]
fn test_send_outage_follows_its_schedule_and_rejects_bad_values() {
    let outage = SendOutage::parse(" 1500 : 80 ").unwrap();
    let start = Instant::now();
    let at = |ms: u64| start + Duration::from_millis(ms);
    // Nothing during the first interval, so the stream starts cleanly.
    assert!(!outage.active(start, at(0)));
    assert!(!outage.active(start, at(1499)));
    assert!(outage.active(start, at(1500)));
    assert!(outage.active(start, at(1579)));
    assert!(!outage.active(start, at(1580)));
    assert!(outage.active(start, at(3020)));
    assert!(!outage.active(start, at(2999)));
    assert!(!outage.in_air());
    let air = SendOutage::parse("1500:80:air").unwrap();
    assert!(air.in_air());
    assert!(air.active(start, at(1500)));
    assert!(!air.active(start, at(1580)));
    for bad in [
        "",
        "80",
        "80:80",
        "10:0",
        "x:5",
        "1500:-1",
        "1500:80:1",
        "1500:80:air:1",
        "80:80:air",
        "3000:20:stall:1",
        "20:20:stall",
    ] {
        assert_eq!(SendOutage::parse(bad), None, "{bad}");
    }
    assert!(outage.stall().is_none() && air.stall().is_none());
}

#[test]
fn send_jitter_stalls_on_a_seeded_schedule() {
    let stalls = |mut jitter: SendJitter| {
        // Every 0.1 ms for a minute: (first held instant, end) of each stall.
        let mut stalls: Vec<(Duration, Duration)> = Vec::new();
        for step in 0..600_000 {
            let at = Duration::from_micros(step * 100);
            if let Some(end) = jitter.stalled_until(at) {
                assert!(at < end);
                if stalls.last().is_none_or(|&(_, last)| last != end) {
                    stalls.push((at, end));
                }
            }
        }
        stalls
    };
    let ms = Duration::from_millis;
    let schedule = stalls(SendJitter::parse(" 700 : 30 ").unwrap());
    assert_eq!(
        schedule,
        stalls(SendJitter::parse("700:30:1").unwrap()),
        "seed 1 by default"
    );
    assert_ne!(schedule, stalls(SendJitter::parse("700:30:2").unwrap()));
    assert!(
        (60..=120).contains(&schedule.len()),
        "{} stalls",
        schedule.len()
    );
    // Starts 0.5 to 1.5 periods apart, the first one into the stream; each
    // lasts a quarter of the longest to the longest.
    let (step, mut last) = (Duration::from_micros(100), Duration::ZERO);
    for &(from, until) in &schedule {
        let gap = from - last;
        assert!(
            gap + step >= ms(350) && gap <= ms(1050) + step,
            "gap {gap:?}"
        );
        let length = until - from;
        assert!(
            length + step > ms(7) + ms(1) / 2 && length <= ms(30),
            "length {length:?}"
        );
        last = from;
    }
    // Stalls as long as the period run into the next now and then, but the link is
    // free again within a second; longer ones would outrun the gaps and hold it for good.
    let mut tight = SendJitter::parse("40:40:5").unwrap();
    let mut held = 0u32;
    for i in 0..600_000u32 {
        held = if tight.stalled_until(i * step).is_some() {
            held + 1
        } else {
            0
        };
        assert!(held < 10_000, "held for a second at {:?}", i * step);
    }
    for bad in [
        "",
        "700",
        "0:30",
        "700:0",
        "4:30",
        "700:701",
        "x:30",
        "700:-1",
        "700:30:x",
        "700:30:-1",
        "700:30:1:2",
    ] {
        assert_eq!(SendJitter::parse(bad), None, "{bad}");
    }
}

#[test]
fn sends_during_a_stall_go_out_in_order_right_after_it() {
    let jitter = SendJitter::parse("700:30:3").unwrap();
    // The first stall, to the 0.1 ms.
    let step = Duration::from_micros(100);
    let mut probe = jitter.clone();
    let (from, until) = (0u32..)
        .map(|i| i * step)
        .find_map(|at| probe.stalled_until(at).map(|end| (at, end)))
        .unwrap();
    assert!(
        until - from > Duration::from_millis(7),
        "{from:?}..{until:?}"
    );
    let mut line = JitterLine::new(jitter);
    let mut sent = Vec::new();
    let mut send = |line: &mut JitterLine<u32>, at: Duration, item: u32| {
        sent.extend(line.release(at).map(|item| (item, at)));
        if line.holds(at) {
            line.hold(item);
        } else {
            sent.push((item, at));
        }
    };
    // Before the stall a send goes straight out; during it, every send waits
    // and none is released early.
    send(&mut line, from - step, 0);
    assert_eq!(line.release_at(), None);
    for (i, at) in [from, from + step, until - step].into_iter().enumerate() {
        send(&mut line, at, i as u32 + 1);
        assert_eq!(line.release_at(), Some(until));
    }
    assert_eq!(line.release(until - step).count(), 0);
    // At its end they go out first, the next send right behind them.
    send(&mut line, until, 4);
    assert_eq!(line.release_at(), None);
    // A send asked about before a release still waits behind what is held.
    let mut early = JitterLine::new(SendJitter::parse("700:30:3").unwrap());
    assert!(early.holds(from));
    early.hold(1);
    assert!(early.holds(until), "behind the held send");
    early.hold(2);
    assert_eq!(early.release(until).collect::<Vec<_>>(), [1, 2]);
    assert_eq!(
        sent,
        [
            (0, from - step),
            (1, until),
            (2, until),
            (3, until),
            (4, until)
        ]
    );
    // Back-to-back stalls hold their sends through both.
    let tight = SendJitter::parse("40:40:5").unwrap();
    let mut probe = tight.clone();
    let (at, first, second) = (0u32..)
        .map(|i| i * step)
        .find_map(|at| {
            let first = probe.stalled_until(at)?;
            let second = probe.clone().stalled_until(first)?;
            Some((at, first, second))
        })
        .unwrap();
    let mut tight = JitterLine::new(tight);
    assert!(tight.holds(at));
    tight.hold(1);
    assert_eq!(tight.release(first).count(), 0, "the second stall holds it");
    assert!(tight.release_at() >= Some(second));
}

#[test]
fn an_adapter_stall_starts_at_a_large_frame_and_fills_the_socket_buffer() {
    let outage = SendOutage::parse("3000:20:stall").unwrap();
    let start = Instant::now();
    let at = |ms: u64| start + Duration::from_millis(ms);
    // No schedule: the outage itself refuses nothing.
    assert!(!outage.active(start, at(3000)));
    assert!(!outage.in_air());
    let mut stall = outage.stall().unwrap();
    // Nothing before a frame starts it, and an ordinary frame does not.
    assert_eq!(stall.full_until(64 * 1024, at(0)), None);
    assert!(!stall.frame(VIDEO_SOCKET_BUFFER, at(0)));
    // A 1.46 MB frame (#59 frame 1001) does.
    assert!(stall.frame(1_462_208, at(100)));
    // Its first 1 MiB fits the socket buffer; the next batch meets a full
    // socket until the stall ends.
    for i in 0..16 {
        assert_eq!(stall.full_until(64 * 1024, at(101)), None, "batch {i}");
    }
    assert_eq!(stall.full_until(64 * 1024, at(105)), Some(at(120)));
    assert_eq!(stall.full_until(64 * 1024, at(119)), Some(at(120)));
    assert_eq!(stall.full_until(64 * 1024, at(120)), None);
    // At most once every three seconds.
    assert!(!stall.frame(1_462_208, at(3099)));
    assert!(stall.frame(1_462_208, at(3100)));
    assert_eq!(
        stall.full_until(VIDEO_SOCKET_BUFFER + 1, at(3100)),
        Some(at(3120))
    );
}

#[test]
fn send_loss_is_unrecoverable_only_beyond_a_blocks_parity() {
    // Two blocks of 10 data and 2 parity packets, sent in batches of 8.
    let mut loss = SendLoss::new(vec![(10, 2), (10, 2)]);
    assert!(!loss.sent(8, 0));
    // 2 refused within the first block: FEC still repairs it.
    assert!(!loss.sent(4, 2));
    // Packets 12-19 belong to the second block only.
    assert!(!loss.sent(8, 2));
    assert!(loss.sent(4, 1));
    assert!(loss.unrecoverable());

    // A whole batch refused across both blocks loses the frame.
    let mut loss = SendLoss::new(vec![(10, 2), (10, 2)]);
    assert!(loss.sent(24, 24));

    // A batch that spans the boundary is charged to both blocks.
    let mut loss = SendLoss::new(vec![(10, 2), (10, 2)]);
    assert!(!loss.sent(10, 0));
    assert!(!loss.sent(4, 2));
    assert!(loss.sent(4, 1));

    // Without parity any refused packet loses the frame.
    let mut loss = SendLoss::new(vec![(3, 0)]);
    assert!(!loss.sent(2, 0));
    assert!(loss.sent(1, 1));

    // Batches beyond the layout, as with a stale layout, change nothing.
    let mut loss = SendLoss::new(vec![(2, 1)]);
    assert!(!loss.sent(3, 0));
    assert!(!loss.sent(5, 5));
}

#[test]
fn send_loss_keyframes_back_off_only_when_part_of_a_frame_got_through() {
    let start = Instant::now();
    let period = Duration::from_micros(8333);
    let at = |ms: u64| start + Duration::from_millis(ms);
    let mut recovery = SendLossRecovery::default();
    // A radio outage: every frame refused from its first packet. Each next
    // frame is a keyframe, so the first one through after it recovers.
    for ms in [0, 8, 16, 25, 33, 41, 50, 58, 66, 75, 83] {
        assert!(recovery.lost(at(ms), period, false), "{ms}");
    }
    // Congestion: the forced keyframe got partly in and was lost anyway.
    assert!(!recovery.lost(at(91), period, true));
    assert!(!recovery.lost(at(122), period, true));
    // After the hold another loss starts one again.
    assert!(recovery.lost(at(123), period, true));
    // A keyframe sent whole ends the hold; a P-frame does not.
    recovery.delivered(false);
    assert!(!recovery.lost(at(130), period, true));
    recovery.delivered(true);
    assert!(recovery.lost(at(131), period, true));
    // At 30 fps the hold is four periods.
    let mut slow = SendLossRecovery::default();
    let period = Duration::from_micros(33_333);
    assert!(slow.lost(at(0), period, true));
    assert!(!slow.lost(at(130), period, true));
    assert!(slow.lost(at(134), period, true));
}

#[test]
fn send_loss_knows_whether_any_packet_reached_the_socket() {
    let mut loss = SendLoss::new(vec![(10, 2)]);
    assert!(loss.sent(8, 8));
    assert!(!loss.reached_socket());
    let mut loss = SendLoss::new(vec![(10, 2)]);
    assert!(!loss.sent(8, 0));
    assert!(loss.sent(4, 4));
    assert!(loss.reached_socket());
}

#[test]
fn streamed_send_loss_matches_the_whole_frame_layout() {
    // Blocks are added as they are built, a batch never spans an unbuilt one.
    let layout = vec![(30, 6), (30, 6), (20, 4), (8, 2)];
    let mut seed = 99u64;
    let mut random = move |range: usize| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) as usize % range
    };
    for _ in 0..500 {
        let mut whole = SendLoss::new(layout.clone());
        let mut streamed = SendLoss::streamed();
        assert!(!streamed.unrecoverable() && !streamed.reached_socket());
        for &(data, parity) in &layout {
            streamed.push_block(data, parity);
            let mut left = data + parity;
            while left > 0 {
                let batch = 1 + random(left.min(10));
                let dropped = if random(4) == 0 { random(batch + 1) } else { 0 };
                assert_eq!(
                    whole.sent(batch, dropped),
                    streamed.sent(batch, dropped),
                    "{layout:?}"
                );
                assert_eq!(whole.reached_socket(), streamed.reached_socket());
                left -= batch;
            }
        }
        assert_eq!(whole.unrecoverable(), streamed.unrecoverable());
    }
    // The first block lost beyond its parity ends the frame before the rest
    // of it is built.
    let mut streamed = SendLoss::streamed();
    streamed.push_block(4, 1);
    assert!(streamed.sent(5, 2));
}

#[test]
fn test_send_loss_is_seeded_per_datagram_and_rejects_bad_values() {
    let mut loss = SendDrop::parse(" 1 : 42 ").unwrap();
    assert!(!loss.in_air());
    let mut again = SendDrop::parse("1:42").unwrap();
    let mut other_seed = SendDrop::parse("1:43").unwrap();
    let first: Vec<bool> = (0..200_000).map(|_| loss.drops()).collect();
    let second: Vec<bool> = (0..200_000).map(|_| again.drops()).collect();
    let third: Vec<bool> = (0..200_000).map(|_| other_seed.drops()).collect();
    assert_eq!(first, second);
    assert_ne!(first, third);
    // About 1% of 200,000.
    let lost = first.iter().filter(|lost| **lost).count();
    assert!((1_700..2_300).contains(&lost), "{lost} lost");
    let mut half = SendDrop::parse("0.5:7:air").unwrap();
    assert!(half.in_air());
    let lost = (0..200_000).filter(|_| half.drops()).count();
    assert!((800..1_200).contains(&lost), "{lost} lost");
    let mut all = SendDrop::parse("100:1").unwrap();
    assert!((0..1000).all(|_| all.drops()));
    for bad in [
        "",
        "1",
        "0:1",
        "-1:1",
        "101:1",
        "x:1",
        "1:x",
        "1:1:1",
        "1:1:air:1",
    ] {
        assert_eq!(SendDrop::parse(bad), None, "{bad}");
    }
}
#[test]
fn a_faster_source_repays_a_startup_deficit_instead_of_lagging_every_claim() {
    // The 2x virtual display at 60 fps. A slow start left the credit short;
    // refilling at exactly the stream rate, every later claim then waited the
    // same 2-3 ms for credit beside a frame already in hand (October 15,
    // idd-push at 60 fps). A bounded grace now repays it.
    let period = 1000. / 60.;
    for burst in [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0] {
        let claims = simulate_with(
            period,
            &source(period / 2., 30., |i| 0.05 * ((i * 7919) % 5) as f64),
            |_| 0.1,
            |mut pacer| {
                let now = Instant::now();
                for ms in [burst, burst + 3., burst + 6.] {
                    pacer.claimed(now + Duration::from_secs_f64(ms / 1000.));
                }
                pacer
            },
        );
        let late: Vec<f64> = claims[claims.len() / 3..]
            .iter()
            .map(|(claim, presented)| claim - presented)
            .collect();
        let mean = late.iter().sum::<f64>() / late.len() as f64;
        assert!(
            mean <= 0.4,
            "claims lag {mean:.2} ms after a burst at {burst} ms"
        );
        let rate = steady_rate(&claims);
        assert!(rate <= 60.1, "{rate:.2} claims/s");
    }
}
#[test]
fn only_wgc_captures_wait_for_the_learned_source_phase() {
    let config = |text: &str| Config::parse(text).unwrap();
    assert!(source_phase(&config(""), "wgc"));
    // Rubylight's display driver pushes every composition: waiting for the
    // next one under GPU load held claims up to 1.25 periods.
    assert!(!source_phase(&config(""), "idd-push"));
    assert!(!source_phase(&config(""), "ddx"));
    assert!(source_phase(
        &config("frame_pacing_source_phase=true"),
        "idd-push"
    ));
    assert!(!source_phase(
        &config("frame_pacing_source_phase=false"),
        "wgc"
    ));
    // Capture `auto` is planned as WGC; the backend that opens decides.
    let start = Instant::now();
    let period = Duration::from_micros(8_333);
    let mut pacer = Pacer::new(start, period).with_source_phase(true);
    pacer.set_source_phase(false);
    assert!(pacer.source_phase.is_none());
    pacer.set_source_phase(true);
    assert!(pacer.source_phase.is_some());
    for n in 0..48 {
        pacer.observe_source(start + period / 2 * n);
    }
    let learned = pacer.source_phase.as_ref().map(|phase| phase.center);
    // The same backend again keeps what was learned.
    pacer.set_source_phase(true);
    assert_eq!(
        pacer.source_phase.as_ref().map(|phase| phase.center),
        learned
    );
}

/// Claims (ms, detected ms) for Rubylight's display driver on a 240 Hz
/// virtual display whose compositor alternates at random between one and two
/// refreshes per picture, as measured on October 16: about 160 frames a
/// second with a median interval of either 4.2 or 8.3 ms.
fn simulate_alternating(rate_known: bool) -> Vec<(f64, f64)> {
    let start = Instant::now();
    let at = |ms: f64| start + Duration::from_secs_f64(ms / 1000.);
    let ms = |instant: Instant| instant.duration_since(start).as_secs_f64() * 1000.;
    let mut pacer = Pacer::new(start, Duration::from_secs_f64(PERIOD / 1000.))
        .with_prediction(true)
        .with_source_phase(false)
        .with_spacing(0.75);
    let mut seed = 7u64;
    let mut coin = || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        // Slightly more double refreshes, so the median sits on 8.3 ms.
        (seed >> 33) % 100 < 55
    };
    let refresh = 1000. / 240.;
    let mut present = vec![0.];
    while present[present.len() - 1] < 30_000. {
        let steps = if coin() { 2. } else { 1. };
        present.push(present[present.len() - 1] + refresh * steps);
    }
    let detected: Vec<f64> = present.iter().map(|p| p + 0.15).collect();
    let mut claims = vec![];
    let mut newest: Option<usize> = None;
    let mut deadline: Option<f64> = None;
    let (mut median, mut mean) = (None, None);
    let mut evaluate = |now: f64,
                        newest: &mut Option<usize>,
                        deadline: &mut Option<f64>,
                        pacer: &mut Pacer,
                        median: Option<Duration>,
                        mean: Option<Duration>| {
        if let Some(i) = *newest {
            if rate_known {
                pacer.observe_source_rate(mean);
            }
            match pacer.decide(at(now), at(present[i]), median) {
                Pace::Claim => {
                    pacer.claimed(at(now));
                    claims.push((now, detected[i]));
                    *newest = None;
                    *deadline = None;
                }
                Pace::WaitUntil(t) => *deadline = Some(ms(t).max(now + 0.001)),
            }
        }
    };
    for i in 0..present.len() {
        while let Some(t) = deadline.filter(|t| *t < detected[i]) {
            evaluate(t, &mut newest, &mut deadline, &mut pacer, median, mean);
            if deadline == Some(t) {
                break;
            }
        }
        let recent = &present[i.saturating_sub(16)..=i];
        let mut intervals: Vec<f64> = recent.windows(2).map(|w| w[1] - w[0]).collect();
        if intervals.len() >= 4 {
            let average = intervals.iter().sum::<f64>() / intervals.len() as f64;
            mean = Some(Duration::from_secs_f64(average / 1000.));
            intervals.sort_by(f64::total_cmp);
            median = Some(Duration::from_secs_f64(
                intervals[intervals.len() / 2] / 1000.,
            ));
        }
        newest = Some(i);
        evaluate(
            detected[i],
            &mut newest,
            &mut deadline,
            &mut pacer,
            median,
            mean,
        );
    }
    claims
}
#[test]
fn a_source_alternating_between_two_cadences_is_claimed_at_the_stream_rate() {
    let wait = |claims: &[(f64, f64)]| {
        let steady = &claims[claims.len() / 10..];
        steady.iter().map(|(claim, seen)| claim - seen).sum::<f64>() / steady.len() as f64
    };
    let claims = simulate_alternating(true);
    let rate = steady_rate(&claims);
    assert!(rate <= 120.2, "{rate:.2} claims/s");
    // Judged by its median, the source looked no faster than the stream:
    // claims ran at 121 a second, as the host measured, and waited longer.
    let previous = simulate_alternating(false);
    assert!(steady_rate(&previous) > 120.5);
    assert!(wait(&claims) < wait(&previous));
}
