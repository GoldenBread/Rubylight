//! Tests for encoder stall backoff, cadence grid, phase lock, and wire-budget bitrate math.

use super::*;

#[test]
fn repeated_encoder_stalls_back_off_within_the_recovery_budget() {
    let limits: Vec<u64> = (0..6)
        .map(|n| encoder_stall_limit(n).as_millis() as u64)
        .collect();
    assert_eq!(limits, [250, 500, 1000, 2000, 2000, 2000]);
    assert_eq!(encoder_stall_limit(u32::MAX), Duration::from_millis(2000));
    // Four escalating attempts fit well inside the stream's recovery budget.
    assert!(limits[..4].iter().sum::<u64>() < 20_000);
}
#[test]
fn the_fec_and_audio_share_of_the_client_bitrate_is_not_a_warning() {
    let warnings = crate::TestWarnings::default();
    let client = Negotiated {
        configured_bitrate_kbps: 150_000,
        bitrate_kbps: 150_000,
        fps: 120,
        audio_channels: 2,
        ..Default::default()
    };
    for (text, warned) in [
        ("", false),
        (
            "max_bitrate=50000
",
            true,
        ),
    ] {
        let config = Config::parse(text).unwrap();
        let mut stream = client.clone();
        let uncapped = uncapped_bitrate_kbps(&stream, 0, &config);
        apply(&mut stream, 0, &config);
        // FEC and audio always take their share of the client's rate.
        assert!(stream.bitrate_kbps < client.configured_bitrate_kbps);
        report_bitrate(&warnings, uncapped, stream.bitrate_kbps, "max_bitrate");
        assert_eq!(!warnings.snapshot().is_empty(), warned, "{text}");
    }
}

#[test]
fn bitrate_clamps_are_reported_with_requested_and_applied_values() {
    let warnings = crate::TestWarnings::default();
    let config = Config::parse("max_bitrate=25000\n").unwrap();
    report_bitrate(
        &warnings,
        900_000,
        runtime_bitrate_kbps(&config, 900_000),
        "max_bitrate and the 500 Mbps runtime cap",
    );
    assert!(warnings.snapshot()[0].message.contains("900000 to 25000"));
    report_bitrate(
        &warnings,
        900_000,
        runtime_bitrate_kbps(&Config::default(), 900_000),
        "500 Mbps runtime cap",
    );
    assert!(warnings.snapshot()[0].message.contains("500000"));
    report_bitrate(&warnings, 20_000, 20_000, "unchanged");
    assert!(warnings.snapshot().is_empty());
}

#[test]
fn runtime_bitrate_honours_the_host_ceiling() {
    assert_eq!(runtime_bitrate_kbps(&Config::default(), 80_000), 80_000);
    assert_eq!(runtime_bitrate_kbps(&Config::default(), 900_000), 500_000);
    assert_eq!(
        runtime_bitrate_kbps_for(&Config::default(), 3, 900_000),
        900_000
    );
    assert_eq!(
        runtime_bitrate_kbps_for(&Config::default(), 3, 2_500_000),
        2_000_000
    );
    let capped = Config::parse("max_bitrate=25000\n").unwrap();
    assert_eq!(runtime_bitrate_kbps(&capped, 80_000), 25_000);
    assert_eq!(runtime_bitrate_kbps(&capped, 10_000), 10_000);
    let pyrowave_capped = Config::parse("max_bitrate=750000\n").unwrap();
    assert_eq!(
        runtime_bitrate_kbps_for(&pyrowave_capped, 3, 900_000),
        750_000
    );
}
#[test]
fn pacing_defaults_to_arrival_and_keeps_the_grid_option() {
    assert_eq!(Pacing::from_config(&Config::default()), Pacing::Arrival);
    let grid = Config::parse("frame_pacing = grid\n").unwrap();
    assert_eq!(Pacing::from_config(&grid), Pacing::Grid);
}
#[test]
fn late_desktop_updates_keep_the_waiting_slot_and_static_resumes_do_not_burst() {
    let start = Instant::now();
    let period = Duration::from_millis(16);
    let mut cadence = Cadence::new(start, period, true);
    cadence.submitted(start);
    // No image at the next slot: checking it must not postpone a frame
    // arriving just afterward until another full refresh interval.
    assert_eq!(cadence.deadline(), start + period);
    let update = start + period + Duration::from_micros(250);
    assert!(cadence.deadline() <= update);
    cadence.submitted(update);
    assert_eq!(cadence.deadline(), start + period * 2);
    let resumed = start + Duration::from_millis(100);
    assert!(cadence.deadline() <= resumed);
    cadence.submitted(resumed);
    assert_eq!(cadence.deadline(), resumed + period);
}
#[test]
fn scheduler_overshoot_does_not_accumulate_or_allow_catch_up_submissions() {
    let start = Instant::now();
    let period = Duration::from_millis(8);
    let mut cadence = Cadence::new(start, period, true);
    for frame in 0..1000 {
        let submitted = start + period * frame + Duration::from_micros(300);
        cadence.submitted(submitted);
        assert_eq!(cadence.deadline(), start + period * (frame + 1));
    }
    let missed = cadence.deadline() + period;
    cadence.submitted(missed);
    assert_eq!(cadence.deadline(), missed + period);
    let mut unsmoothed = Cadence::new(start, period, false);
    unsmoothed.submitted(start + Duration::from_micros(300));
    assert_eq!(
        unsmoothed.deadline(),
        start + period + Duration::from_micros(300)
    );
}
#[test]
fn phase_locked_cadence_follows_the_client_period_and_falls_back_without_reports() {
    use crate::phase_sync::PhaseSync;
    use rubylight_protocol::phase_lock::{PhaseLock, Report};
    // A fake clock: every instant is the start plus an exact offset.
    let start = Instant::now();
    let nominal = Duration::from_nanos(8_333_333); // the host's 120.00 Hz
    let client_ns = 8_331_945u32; // a phone at 120.02 Hz
    let at_margin = Report {
        frames: 60,
        period_ns: client_ns,
        lead_ns: PhaseLock::DEFAULT_MARGIN_NS as i32,
        spread_ns: 0,
    }
    .encode();
    let mut sync = PhaseSync::default();
    let mut cadence = Cadence::new(start, nominal, true);
    let mut now = start;
    for frame in 0..1200u32 {
        // A report every 60 frames (about 500 ms) whose lead is already on target.
        if frame % 60 == 0 {
            assert!(sync.on_payload(now, &at_margin).is_some());
        }
        let due = cadence.deadline();
        cadence.submitted_after(now, sync.interval(now, nominal).unwrap());
        if frame > 0 {
            assert_eq!(
                cadence.deadline() - due,
                Duration::from_nanos(u64::from(client_ns))
            );
        }
        // Submitted a little after each slot; the overshoot must not accumulate.
        now = cadence.deadline() + Duration::from_micros(300);
    }
    // Ten seconds at the client's rate: 1.67 ms of drift that the host's own period
    // would have added to every frame's wait at the client.
    let locked_span = cadence.deadline() - start;
    assert_eq!(
        locked_span,
        Duration::from_nanos(u64::from(client_ns)) * 1200
    );
    assert_eq!(
        nominal * 1200 - locked_span,
        Duration::from_nanos(1_388 * 1200)
    );
    // Reports stop: after the timeout the lock is gone and the fixed period applies again.
    now = cadence.deadline() + PhaseSync::TIMEOUT;
    assert_eq!(sync.interval(now, nominal), None);
    cadence.submitted(now);
    for _ in 0..100 {
        let due = cadence.deadline();
        assert_eq!(sync.interval(due, nominal), None);
        cadence.submitted(due + Duration::from_micros(300));
        assert_eq!(cadence.deadline() - due, nominal);
    }
}
#[test]
fn without_reports_cadence_deadlines_match_the_fixed_period_exactly() {
    use crate::phase_sync::PhaseSync;
    // The cadence before phase lock, kept here as the reference.
    fn previous(due: Instant, period: Duration, smooth: bool, now: Instant) -> Instant {
        let anchored = due + period;
        if smooth && anchored > now {
            anchored
        } else {
            now + period
        }
    }
    let start = Instant::now();
    for smooth in [true, false] {
        for period in [
            Duration::from_nanos(8_333_333),
            Duration::from_nanos(16_666_667),
            Duration::from_millis(4),
        ] {
            let mut sync = PhaseSync::default();
            let mut cadence = Cadence::new(start, period, smooth);
            let mut reference = start;
            for frame in 0..1000u32 {
                // Uneven submissions: on time, late, and after a static pause.
                let late = Duration::from_micros(u64::from(frame % 7) * 400);
                let now = if frame % 97 == 0 {
                    cadence.deadline() + period * 5
                } else {
                    cadence.deadline() + late
                };
                // What the host does without a lock: no interval, the original call.
                assert_eq!(sync.interval(now, period), None);
                cadence.submitted(now);
                reference = previous(reference, period, smooth, now);
                assert_eq!(cadence.deadline(), reference, "frame {frame}");
            }
        }
    }
}
#[test]
fn without_reports_pacer_claims_match_an_untouched_pacer() {
    use crate::phase_sync::PhaseSync;
    let start = Instant::now();
    let period = Duration::from_nanos(8_333_333);
    let mut sync = PhaseSync::default();
    let mut paced = Pacer::new(start, period);
    let mut untouched = Pacer::new(start, period);
    for frame in 0..1000u32 {
        let now = start + period * frame + Duration::from_micros(u64::from(frame % 5) * 700);
        if let Some(interval) = sync.interval(now, period) {
            paced.set_period(now, interval);
        }
        assert_eq!(
            paced.allowed_at(now),
            untouched.allowed_at(now),
            "frame {frame}"
        );
        paced.claimed(now);
        untouched.claimed(now);
    }
}
#[test]
fn video_packets_always_use_the_device_request() {
    // #52: packetsize=1200 replaced the 1184 a remote IPv6 Moonlight asked for, so every
    // video packet was cut short in its receive buffer; a smaller size breaks the packets
    // Moonlight rebuilds from FEC at its own size.
    let warnings = crate::TestWarnings::default();
    for text in [
        "packetsize=1200
",
        "packetsize=512
",
        "packetsize=1401
",
    ] {
        let config = Config::parse(text).unwrap();
        for requested in [1184, 1024, 1392] {
            let mut stream = Negotiated {
                packet_size: requested,
                ..Default::default()
            };
            apply(&mut stream, 0, &config);
            assert_eq!(stream.packet_size, requested, "{text:?}");
        }
        report_packet_size(&warnings, &config);
        assert!(!warnings.snapshot().is_empty(), "{text:?}");
    }
    for text in [
        "",
        "packetsize=0
",
    ] {
        report_packet_size(&warnings, &Config::parse(text).unwrap());
        assert!(warnings.snapshot().is_empty(), "{text:?}");
    }
}
#[test]
fn configured_wire_budget_deducts_fec_and_audio_after_the_ceiling() {
    let mut stream = Negotiated {
        fps: 120,
        configured_bitrate_kbps: 20000,
        audio_channels: 6,
        ..Default::default()
    };
    apply(&mut stream, 59940, &Config::default());
    // The stream keeps its request's rate; see a_stream_runs_at_the_rate_its_request_asks_for.
    assert_eq!(stream.fps_millihz(), 120_000);
    assert_eq!(stream.bitrate_kbps, 14924);
    let config = Config::parse("max_bitrate=15000\n").unwrap();
    apply(&mut stream, 59940, &config);
    assert_eq!(stream.bitrate_kbps, 10924);
    let mut legacy = Negotiated {
        bitrate_kbps: 20000,
        ..Default::default()
    };
    apply(
        &mut legacy,
        0,
        &Config::parse("max_bitrate=25000\n").unwrap(),
    );
    assert_eq!(legacy.bitrate_kbps, 20000);
    let mut pyro = Negotiated {
        fps: 120,
        codec: 3,
        configured_bitrate_kbps: 800000,
        ..Default::default()
    };
    apply(&mut pyro, 120000, &Config::default());
    // Audio with its 50% parity and control, without generic 20% FEC, as
    // Vibeshine takes them off a PyroWave total: 192 * 3 / 2 + 500.
    assert_eq!(pyro.bitrate_kbps, 799212);
}

#[test]
fn explain_bitrate_identifies_wire_budget_and_ceiling_sources() {
    let mut configured = Negotiated {
        fps: 144,
        codec: 2,
        configured_bitrate_kbps: 80_000,
        audio_channels: 2,
        audio_quality: true,
        ..Default::default()
    };
    let default_cfg = Config::default();
    apply(&mut configured, 144_000, &default_cfg);
    assert_eq!(configured.bitrate_kbps, 62_988);
    let explained = explain_bitrate(&configured, &default_cfg, 0);
    assert_eq!(explained.requested_kbps, 80_000);
    assert_eq!(explained.fec_reserved_kbps, 16_000);
    assert_eq!(explained.audio_reserved_kbps, 512);
    assert_eq!(explained.overhead_reserved_kbps, 500);
    assert_eq!(explained.encoder_kbps, 62_988);
    assert_eq!(explained.link_ceiling_kbps, 62_988);
    assert_eq!(explained.ceiling_source, "wire_budget_after_fec_audio");

    let capped_cfg = Config::parse("max_bitrate=15000\n").unwrap();
    let mut capped = Negotiated {
        fps: 120,
        codec: 2,
        configured_bitrate_kbps: 20_000,
        audio_channels: 6,
        ..Default::default()
    };
    apply(&mut capped, 120_000, &capped_cfg);
    let explained = explain_bitrate(&capped, &capped_cfg, 15_000);
    assert_eq!(explained.ceiling_source, "host_max_bitrate");
    assert_eq!(explained.host_max_bitrate_kbps, 15_000);

    let legacy = Negotiated {
        bitrate_kbps: 20_000,
        ..Default::default()
    };
    let explained = explain_bitrate(&legacy, &default_cfg, 0);
    assert_eq!(explained.link_ceiling_kbps, 20_000);
    assert_eq!(explained.ceiling_source, "client_request");

    let mut codec_limited = Negotiated {
        codec: 2,
        configured_bitrate_kbps: 800_000,
        ..Default::default()
    };
    apply(&mut codec_limited, 120_000, &default_cfg);
    let explained = explain_bitrate(&codec_limited, &default_cfg, 0);
    assert_eq!(explained.link_ceiling_kbps, 500_000);
    assert_eq!(explained.ceiling_source, "codec_limit");
}
#[test]
fn a_stream_runs_at_the_rate_its_request_asks_for() {
    // Artemis's Warp launches at 60 and asks for 120 or 240; Moonlight for
    // Android launches at 120 and asks for 119 on a 120 Hz panel.
    for (launch, request, expected) in [
        (60_000, 60, 60_000),
        (60_000, 120, 120_000),
        (60_000, 240, 240_000),
        (120_000, 119, 119_000),
        // A launch without a rate leaves the request's.
        (0, 120, 120_000),
    ] {
        let mut stream = artemis_announce(request);
        apply(&mut stream, launch, &Config::default());
        assert_eq!(
            stream.fps_millihz(),
            expected,
            "launch {launch}, request {request}"
        );
    }
    // "Use the launch frame rate" keeps the previous behaviour on request.
    let limited = Config::parse("limit_framerate=true\n").unwrap();
    for request in [120, 240] {
        let mut stream = artemis_announce(request);
        apply(&mut stream, 60_000, &limited);
        assert_eq!(stream.fps_millihz(), 60_000, "request {request}");
        assert_eq!(stream.bitrate_kbps, 31_308);
    }
    let mut stream = artemis_announce(120);
    apply(&mut stream, 0, &limited);
    assert_eq!(stream.fps_millihz(), 120_000);
    // Nightjar #119: when launch carries an exact fractional rate (59.94 / 119.88 Hz)
    // and RTSP SDP only carried the rounded integer `maxFPS`, preserve the fractional rate.
    for (launch, request) in [(59_940, 60), (119_880, 120), (23_976, 24)] {
        let mut fractional = artemis_announce(request);
        apply(&mut fractional, launch, &Config::default());
        assert_eq!(fractional.fps_millihz(), launch);
    }
    // An explicit fractional SDP rate is not overwritten by a different launch rate.
    let mut explicit = Negotiated::from_sdp(b"a=x-nv-video[0].maxFPS:59940\n").unwrap();
    apply(&mut explicit, 60_000, &Config::default());
    assert_eq!(explicit.fps_millihz(), 59_940);
}

#[test]
fn pacer_set_period_banks_accrued_credit_across_phase_lock_steps() {
    let start = Instant::now();
    let old = Duration::from_micros(16_667);
    let slower = Duration::from_micros(25_000);
    let mut pacer = Pacer::new(start, old);
    pacer.claimed(start);
    let step = start + old;
    // Exactly one old period has elapsed; stepping to a slower period must bank
    // the full frame of credit earned at `old` instead of dividing elapsed time
    // by `slower` into a -0.333 phantom deficit that delays the next frame.
    pacer.set_period(step, slower);
    pacer.claimed(step);
    let next = step + slower;
    assert_eq!(pacer.allowed_at(next), next);
    // Stepping back to a faster period must bank at `slower` instead of creating
    // a +0.50 phantom surplus that admits an early frame before `next + old`.
    pacer.set_period(next, old);
    pacer.claimed(next);
    assert!(pacer.allowed_at(next + old / 2) > next + old / 2);
    assert_eq!(pacer.allowed_at(next + old), next + old);
}
#[test]
fn a_warp_stream_request_keeps_the_clients_bitrate() {
    let config = Config::default();
    let mut plain = artemis_announce(60);
    apply(&mut plain, 60_000, &config);
    assert_eq!(plain.bitrate_kbps, 31_308);
    for warp in [2, 4] {
        let mut warped = artemis_announce(60 * warp);
        let uncapped = uncapped_bitrate_kbps(&warped, 60_000, &config);
        apply(&mut warped, 60_000, &config);
        assert_eq!(warped.bitrate_kbps, plain.bitrate_kbps, "warp {warp}");
        assert_eq!(uncapped, plain.bitrate_kbps, "warp {warp}");
    }
    // A host ceiling still applies to the client's bitrate.
    let mut capped = artemis_announce(240);
    apply(
        &mut capped,
        60_000,
        &Config::parse("max_bitrate=30000\n").unwrap(),
    );
    assert_eq!(capped.bitrate_kbps, 23_308);
}
#[derive(Debug)]
struct ClientPort {
    unrecoverable_frames: usize,
    lost_packets: usize,
    /// The queue left behind the last frame: what a control ping waits behind.
    standing_queue: Duration,
}
/// A deterministic model, not a measurement: a 1 Gbps host feeding a client's
/// 100 Mbps Ethernet port (the Onn 4K Pro's is 10/100) through a tail-drop
/// queue of `queue_bytes`. Ten seconds of 60 fps frames at the encoder's
/// per-frame budget, an 8x keyframe every five seconds, 20% FEC and
/// encryption, packetized and paced as the host sends them.
fn client_port(encoder_kbps: u32, pacing_kbps: i64, queue_bytes: usize) -> ClientPort {
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
    let period = Duration::from_secs_f64(1. / 60.);
    let needed = video_wire_rate_bps(&packetizer, encoder_kbps, period, false);
    let bps = rate_bps(pacing_kbps, encoder_kbps, 1_000_000_000, false, needed);
    let datagram = packetizer.packet_size + 16 + 32;
    let per_batch = (batch_payload_limit(bps, datagram, false, 64 * 1024) / datagram).max(1);
    let wire = |bps: f64| Duration::from_secs_f64((datagram + overhead(false)) as f64 * 8. / bps);
    let (host_time, port_time) = (wire(1e9), wire(100e6));
    let capacity = Duration::from_secs_f64(queue_bytes as f64 * 8. / 100e6);
    let start = Instant::now();
    let mut pacer = Pacer::new(start);
    let (mut now, mut drained_at) = (start, start);
    let mut result = ClientPort {
        unrecoverable_frames: 0,
        lost_packets: 0,
        standing_queue: Duration::ZERO,
    };
    for frame in 0..600u32 {
        now = now.max(start + period * frame);
        let payload = (f64::from(encoder_kbps) * 125. * period.as_secs_f64()).ceil() as usize
            * if frame % 300 == 0 { 8 } else { 1 };
        let mut failed = false;
        for (data, fec) in packetizer.block_layout(payload) {
            let (mut lost_data, mut parity) = (0, 0);
            let mut sent = 0;
            while sent < data + fec {
                now = now.max(pacer.due());
                let count = (data + fec - sent).min(per_batch);
                for index in sent..sent + count {
                    now += host_time;
                    let done = drained_at.max(now) + port_time;
                    if done.saturating_duration_since(now) <= capacity {
                        drained_at = done;
                        parity += usize::from(index >= data);
                    } else {
                        result.lost_packets += 1;
                        lost_data += usize::from(index < data);
                    }
                }
                pacer.sent(now, count * datagram, count, false, bps);
                sent += count;
            }
            failed |= lost_data > parity;
        }
        result.unrecoverable_frames += usize::from(failed);
    }
    result.standing_queue = drained_at.saturating_duration_since(now);
    result
}
#[test]
fn warp_requests_at_the_clients_bitrate_fit_a_100_mbps_client_port() {
    // 1 MiB holds about 84 ms at 100 Mbps, near the 78-80 ms round trip an
    // Onn client on Ethernet reported; 16 KiB is a shallow switch queue.
    let deep = 1024 * 1024;
    for warp in [1, 2, 4] {
        let mut stream = artemis_announce(60 * warp);
        apply(&mut stream, 60_000, &Config::default());
        for queue in [deep, 16 * 1024] {
            let capped = client_port(stream.bitrate_kbps, 80_000, queue);
            assert_eq!(capped.lost_packets, 0, "warp {warp}, {queue} B: {capped:?}");
            assert_eq!(capped.unrecoverable_frames, 0);
            assert!(capped.standing_queue < Duration::from_millis(15));
        }
        // The default 800 Mbps bursts need the deep queue, but fit it.
        let default = client_port(stream.bitrate_kbps, 0, deep);
        assert_eq!(default.unrecoverable_frames, 0, "warp {warp}: {default:?}");
    }
    // Negative control, the previous policy: Warp 2 multiplied the 40 Mbps
    // budget by four. About 165 Mbps cannot fit the port: the queue stays
    // full, as an 80 ms round trip on a LAN shows, and nearly every frame dies.
    let previous = 40_000 * 4 * 80 / 100 - 192 - 500;
    let overloaded = client_port(previous, 0, deep);
    assert!(overloaded.unrecoverable_frames > 540, "{overloaded:?}");
    assert!(overloaded.standing_queue > Duration::from_millis(75));
}
