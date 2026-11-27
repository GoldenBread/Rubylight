//! Tests for client link ceilings, wireless starting rates, batching, and startup bursts.

use super::*;

#[test]
fn host_link_speed_is_a_ceiling_not_a_measurement_of_the_client_path() {
    assert_eq!(rate_bps(0, 50_000, 0, false, 65_000_000), 800_000_000);
    assert_eq!(
        rate_bps(0, 50_000, 2_500_000_000, false, 65_000_000),
        800_000_000
    );
    assert_eq!(
        rate_bps(0, 50_000, 100_000_000, false, 65_000_000),
        80_000_000
    );
    for needed in [0, 65_000_000, 150_000_000] {
        assert_eq!(rate_bps(30_000, 50_000, 0, false, needed), 55_000_000);
        assert_eq!(rate_bps(800_000, 50_000, 0, false, needed), 800_000_000);
        assert_eq!(
            rate_bps(100_000, 100_000, 100_000_000, false, needed),
            80_000_000
        );
    }
    assert_eq!(rate_bps(0, 1, 0, true, 0), 1_000_000);
    assert_eq!(rate_bps(0, 1, 0, false, 0), 800_000_000);
    assert_eq!(rate_bps(0, u32::MAX, 0, true, u64::MAX), 800_000_000);
    assert!(rate_bps(i64::MAX, u32::MAX, u64::MAX, false, u64::MAX) > 0);
}

#[test]
fn the_clients_own_link_caps_conventional_pacing_at_four_fifths() {
    const M: u64 = 1_000_000;
    let rate = |host: u64, client: u64| {
        rate_bps_for_client(0, 50_000, host * M, false, 65 * M, client * M)
    };
    // No hint (stock Moonlight): unchanged.
    assert_eq!(rate(0, 0), 800 * M);
    assert_eq!(rate(1000, 0), 800 * M);
    // A 1 GbE and a 2.5 GbE client stay at 800 Mbps; 100 Mbps gets 80.
    assert_eq!(rate(1000, 1000), 800 * M);
    assert_eq!(rate(1000, 2500), 800 * M);
    assert_eq!(rate(0, 100), 80 * M);
    assert_eq!(rate(1000, 100), 80 * M);
    // The slower of the two NICs decides, whichever side it is.
    assert_eq!(rate(100, 1000), 80 * M);
    assert_eq!(rate(100, 2500), 80 * M);
    assert_eq!(rate(0, 2500), 800 * M);
    // The plain function is the same policy with no hint.
    assert_eq!(rate_bps(0, 50_000, 100 * M, false, 65 * M), rate(100, 0));
    // An explicit cap below the client's link stays; above it, the link wins.
    assert_eq!(
        rate_bps_for_client(30_000, 50_000, 0, false, 0, 100 * M),
        55 * M
    );
    assert_eq!(
        rate_bps_for_client(800_000, 50_000, 0, false, 0, 100 * M),
        80 * M
    );
}

#[test]
fn a_client_link_below_the_bitrate_does_not_pace_the_stream_under_it() {
    const M: u64 = 1_000_000;
    // 100 Mbps client, 150 Mbps stream: 4/5 of the port is 80, the floor
    // is 165.
    let rate = |client: u64| rate_bps_for_client(0, 150_000, 1000 * M, false, 180 * M, client * M);
    assert_eq!(rate(100), 165 * M);
    assert!(client_link_below_stream(150_000, 1000 * M, 100 * M));
    // A hint that fits the stream is capped at 4/5 as before.
    assert_eq!(rate(1000), 800 * M);
    assert_eq!(
        rate_bps_for_client(0, 50_000, 0, false, 65 * M, 100 * M),
        80 * M
    );
    assert!(!client_link_below_stream(50_000, 0, 100 * M));
    // No hint, or the host's own link limiting: no floor.
    assert!(!client_link_below_stream(150_000, 100 * M, 0));
    assert!(!client_link_below_stream(150_000, 100 * M, 1000 * M));
    assert_eq!(
        rate_bps_for_client(0, 150_000, 100 * M, false, 180 * M, 0),
        80 * M
    );
}

#[test]
fn a_client_that_reports_wifi_gets_wifi_pacing_from_the_first_frame() {
    const M: u64 = 1_000_000;
    const GBPS: u64 = 1_000_000_000;
    // Wired 1 GbE host, HEVC 4K60 at 63 Mbps, client link unknown (Wi-Fi).
    let (kbps, needed) = (63_000, 75 * M);
    let start = |host_wireless, client_wireless| {
        rate_bps_for_client(
            0,
            kbps,
            GBPS,
            route_is_wireless(host_wireless, client_wireless),
            needed,
            0,
        )
    };
    assert_eq!(start(false, false), 800 * M);
    assert_eq!(start(false, true), 126 * M);
    assert_eq!(start(false, true), rate_bps(0, kbps, GBPS, true, needed));
    // A wireless host route still decides alone; the hint adds, never removes.
    assert_eq!(start(true, false), 126 * M);
    assert_eq!(start(true, true), 126 * M);
    // The same clamp as the host's own Wi-Fi route: demand above 2x wins.
    assert_eq!(
        rate_bps_for_client(0, 63_000, GBPS, route_is_wireless(false, true), 200 * M, 0),
        250 * M
    );
    // PyroWave without a client pace or wired link hint starts at demand,
    // not at the host link; the wireless flag also drops a wired link hint.
    let wired_hint = ClientPath {
        link_bps: GBPS,
        pace_bps: 0,
    };
    let pyrowave = |wireless| {
        pyrowave_route_rate_bps(
            0,
            300_000,
            2_500 * M,
            route_is_wireless(false, wireless),
            100 * M,
            wired_hint,
        )
    };
    assert_eq!(pyrowave(false), 950 * M);
    // Demand now includes parity and headers already, so the wireless route
    // starts 25% above it instead of above bitrate times 1.1.
    assert_eq!(pyrowave(true), 125 * M);
}

#[test]
fn only_wireless_starting_rates_follow_bitrate_without_capping_pyrowave_demand() {
    for kbps in [9_300, 19_000, 60_000, 149_000] {
        for link in [0, 1_000_000_000, 2_500_000_000] {
            assert_eq!(
                rate_bps(0, kbps, link, true, u64::from(kbps) * 1300),
                u64::from(kbps) * 2000
            );
            assert_eq!(
                rate_bps(0, kbps, link, false, u64::from(kbps) * 1300),
                800_000_000
            );
        }
    }
    for kbps in [200_000, 800_000, 2_000_000] {
        let demand = u64::from(kbps) * 1400;
        assert_eq!(
            pyrowave_rate_bps(0, kbps, 0, demand, ClientPath::default()),
            demand * 5 / 4
        );
        assert_eq!(
            pyrowave_rate_bps(0, kbps, 1_000_000_000, demand, ClientPath::default()),
            (demand * 5 / 4).min(950_000_000)
        );
    }
}

#[test]
fn automatic_pacing_leaves_room_for_parity_small_packets_and_high_refresh() {
    for kbps in [1, 2_000, 40_000, 200_000] {
        for fps in [30, 60, 120, 240] {
            for fec in [0, 20, 100] {
                for packet_size in [512, 1392] {
                    for ipv6 in [false, true] {
                        let packetizer = crate::packet::VideoPacketizer {
                            sequence: 0,
                            iv_counter: 0,
                            frame: 1,
                            packet_size,
                            fec_percent: fec,
                            min_fec: 4,
                            key: Some([0; 16]),
                        };
                        let period = Duration::from_secs_f64(1. / f64::from(fps));
                        let needed = video_wire_rate_bps(&packetizer, kbps, period, ipv6);
                        let bps = rate_bps(0, kbps, 1_000_000_000, true, needed);
                        assert!(
                            bps >= needed + needed / 4,
                            "{kbps} Kbps, {fps} fps, {fec}% FEC, {packet_size} bytes: {needed} > {bps}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn batches_fit_one_millisecond_on_the_wire_or_one_datagram() {
    for ipv6 in [false, true] {
        for datagram in [528, 1408, 1440] {
            for bps in [1, 1_000_000, 80_000_000, 800_000_000, u64::MAX] {
                for maximum in [16 * 1024, 32 * 1024, 64 * 1024] {
                    let limit = batch_payload_limit(bps, datagram, ipv6, maximum);
                    let count = limit / datagram;
                    assert!(count >= 1 && limit <= maximum);
                    if count > 1 {
                        assert!((count * (datagram + overhead(ipv6))) as u64 * 8000 <= bps);
                    }
                }
            }
        }
    }
    assert_eq!(batch_payload_limit(1, 1408, false, 1024), 1408);
}

/// Model a 1 Gbps host feeding a 100 Mbps egress queue of just 16 KiB.
/// It is an explicit test topology, not a claim about any client's NIC.
fn downstream_losses(previous_policy: bool, ipv6: bool, keyframes: bool) -> usize {
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
    let needed = video_wire_rate_bps(&packetizer, 40_000, period, ipv6);
    let bps = if previous_policy {
        800_000_000
    } else {
        rate_bps(80_000, 40_000, 1_000_000_000, false, needed)
    };
    let datagram = packetizer.packet_size + 16 + 32;
    let wire_packet = datagram + overhead(ipv6);
    let limit = if previous_policy {
        64 * 1024
    } else {
        batch_payload_limit(bps, datagram, ipv6, 64 * 1024)
    };
    let per_batch = (limit.min(65507) / datagram).clamp(1, 64);
    let start = Instant::now();
    let mut pacer = Pacer::new(start);
    let mut now = start;
    let mut drained_at = start;
    let mut lost = 0;
    let wire_time = Duration::from_secs_f64(wire_packet as f64 * 8. / 100_000_000.);
    let host_time = Duration::from_secs_f64(wire_packet as f64 * 8. / 1_000_000_000.);
    let capacity = Duration::from_secs_f64(16. * 1024. * 8. / 100_000_000.);
    for frame in 0..600 {
        now = now.max(start + period * frame);
        let payload = (40_000. * 125. * period.as_secs_f64()).ceil() as usize
            * if keyframes && frame % 300 == 0 { 8 } else { 1 };
        for (data, fec) in packetizer.block_layout(payload) {
            let mut remaining = data + fec;
            while remaining > 0 {
                now = now.max(pacer.due());
                let count = remaining.min(per_batch);
                for _ in 0..count {
                    now += host_time;
                    let done = drained_at.max(now) + wire_time;
                    if done.saturating_duration_since(now) <= capacity {
                        drained_at = done;
                    } else {
                        lost += 1;
                    }
                }
                pacer.sent(now, count * datagram, count, ipv6, bps);
                remaining -= count;
            }
        }
    }
    lost
}

#[test]
fn an_explicit_eighty_mbps_cap_can_protect_a_slower_downstream_queue() {
    for ipv6 in [false, true] {
        for keyframes in [false, true] {
            assert!(downstream_losses(true, ipv6, keyframes) > 10_000);
            assert_eq!(downstream_losses(false, ipv6, keyframes), 0);
        }
    }
}

/// A record frame of the encoder's full budget, as the host frames it.
fn budget_frame(width: u32, height: u32, budget: usize) -> Vec<u8> {
    let size = |i: usize| 8 + (i * 73 % 504) * 4;
    let (mut count, mut total) = (0, 8);
    while total + size(count) <= budget {
        total += size(count);
        count += 1;
    }
    let mut raw = Vec::with_capacity(total);
    raw.extend_from_slice(&(0x80000000u32 | (width - 1) | ((height - 1) << 14)).to_le_bytes());
    raw.extend_from_slice(&(count as u32).to_le_bytes());
    for i in 0..count {
        raw.extend_from_slice(&((size(i) as u32 / 4) << 16).to_le_bytes());
        raw.extend_from_slice(&((((i * 37) % count) as u32) << 8).to_le_bytes());
        raw.extend((0..size(i) - 8).map(|j| (i * 31 + j * 17) as u8));
    }
    raw
}

#[test]
fn pyrowave_pacing_leaves_headroom_for_a_budget_sized_frame() {
    use crate::{
        packet::{PyrowaveFec, VideoPacketizer},
        pyrowave,
    };
    // Moonlight's 1392-byte packets less the encryption header.
    let packet_size = 1360;
    let period = Duration::from_secs_f64(1. / 60.);
    for (width, height, kbps) in [
        (1920, 1080, 400_000),
        (2560, 1440, 708_000),
        (3840, 2160, 800_000),
    ] {
        let budget = pyrowave::budget(kbps, period, packet_size, true, 20);
        let framed = pyrowave::record_frame(
            &budget_frame(width, height, budget),
            pyrowave::aligned_payload(packet_size),
            pyrowave::max_frame_bytes(packet_size, true),
        )
        .unwrap();
        let mut packetizer = VideoPacketizer {
            sequence: 0,
            iv_counter: 0,
            frame: 1,
            packet_size,
            fec_percent: 0,
            min_fec: 2,
            key: None,
        };
        let fec = PyrowaveFec {
            records: true,
            critical_percentage: 20,
            detail_percentage: 0,
            wire_budget: 0,
            ipv6: false,
        };
        let (packets, fec_limited, _) = packetizer.pyrowave_blocks(&framed, 0, 0, fec).unwrap();
        assert!(!fec_limited);
        // Payload, RTP and NV headers, encryption, and Ethernet/IP/UDP.
        let wire = packets * (pyrowave::aligned_payload(packet_size) + 32 + 32 + overhead(false));
        // As the PyroWave sender computes its demand.
        let demand = (packets * (packet_size + 16 + 32 + overhead(false))) as u64 * 60 * 8;
        let bps = pyrowave_rate_bps(0, kbps, 0, demand, ClientPath::default());
        let paced = bps as f64 * period.as_secs_f64() / 8.;
        // An unhinted client is held to 1 GbE's 95% ceiling, so an 800 Mbps
        // stream is paced at 950 Mbps rather than bursting above 1 GbE.
        let capped = demand.saturating_mul(5) / 4 > 950_000_000;
        assert!(
            paced >= if capped { 1.15 } else { 1.2 } * wire as f64,
            "{width}x{height} at {kbps} Kbps: {packets} packets, {wire} wire bytes, {paced:.0} paced bytes per frame"
        );
    }
}

#[test]
fn socket_time_and_short_wake_delays_do_not_accumulate() {
    let start = Instant::now();
    let mut pacer = Pacer::new(start);
    for batch in 1..=20 {
        let woke = pacer.due() + Duration::from_micros(50);
        let completed = woke + Duration::from_micros(200);
        pacer.sent(completed, 934, 1, false, 8_000_000);
        assert_eq!(pacer.due(), start + Duration::from_millis(batch));
    }
    // Sends slower than the slack fall behind without building debt.
    for _ in 0..5 {
        let completed = pacer.due() + Duration::from_micros(700);
        pacer.sent(completed, 934, 1, false, 8_000_000);
        assert_eq!(
            pacer.due(),
            completed + Duration::from_millis(1) - SEND_SLACK
        );
    }
}

#[test]
fn socket_stalls_and_late_wakeups_cannot_create_catch_up_credit() {
    let start = Instant::now();
    let mut pacer = Pacer::new(start);
    // One millisecond of IPv4 traffic at 8 Mbps.
    let interval = Duration::from_millis(1);
    pacer.sent(start, 934, 1, false, 8_000_000);
    assert_eq!(pacer.due(), start + interval);
    // A 5 ms stall earns only the bounded slack, not a catch-up burst.
    let after_stall = start + Duration::from_millis(6);
    pacer.sent(after_stall, 934, 1, false, 8_000_000);
    assert_eq!(pacer.due(), after_stall + interval - SEND_SLACK);
    let on_time = pacer.due();
    pacer.sent(on_time, 934, 1, false, 8_000_000);
    assert_eq!(pacer.due(), on_time + interval);
    // Starting the next frame after a long idle period creates no debt.
    let next_frame = start + Duration::from_secs(1);
    pacer.sent(next_frame, 934, 1, false, 8_000_000);
    assert_eq!(pacer.due(), next_frame + interval - SEND_SLACK);
}

#[test]
fn each_ipv6_datagram_has_wire_overhead_and_empty_batches_have_no_debt() {
    let start = Instant::now();
    let mut pacer = Pacer::new(start);
    pacer.sent(start, 2 * 914, 2, true, 8_000_000);
    assert_eq!(pacer.due(), start + Duration::from_millis(2));
    let completed = start + Duration::from_millis(3);
    pacer.sent(completed, 0, 0, false, 8_000_000);
    assert_eq!(pacer.due(), completed);
}

/// `pacing_follow_client_link`: a wired 2.5 or 10 GbE path that both ends
/// report is paced at 4/5 of its slower link instead of 800 Mbps; off, or
/// without both links known, or on Wi-Fi, nothing changes.
#[test]
fn following_the_client_link_lifts_the_wired_ceiling_to_the_slower_link() {
    const M: u64 = 1_000_000;
    let rate = |host: u64, client: u64, wireless: bool, follow: bool| {
        rate_bps_following_client(0, 150_000, host * M, wireless, 180 * M, client * M, follow)
    };
    assert_eq!(rate(10_000, 2_500, false, true), 2_000 * M);
    assert_eq!(rate(2_500, 10_000, false, true), 2_000 * M);
    assert_eq!(rate(10_000, 10_000, false, true), 8_000 * M);
    // Off: the ceiling stays where it was.
    assert_eq!(rate(10_000, 2_500, false, false), 800 * M);
    assert_eq!(
        rate(10_000, 2_500, false, false),
        rate_bps_for_client(0, 150_000, 10_000 * M, false, 180 * M, 2_500 * M)
    );
    // A slower link keeps its 4/5 cap, never above what it was.
    assert_eq!(rate(1_000, 1_000, false, true), 800 * M);
    assert_eq!(
        rate(1_000, 100, false, true),
        rate(1_000, 100, false, false)
    );
    // Without the host's link or the client's report there is nothing to follow.
    assert_eq!(rate(0, 2_500, false, true), 800 * M);
    assert_eq!(rate(10_000, 0, false, true), 800 * M);
    // Wi-Fi keeps its own policy.
    assert_eq!(
        rate(10_000, 2_500, true, true),
        rate(10_000, 2_500, true, false)
    );
}

#[test]
fn pyrowave_defaults_to_following_confirmed_multi_gigabit_wired_links() {
    const M: u64 = 1_000_000;
    assert!(follow_client_link_default(true));
    assert!(!follow_client_link_default(false));
    // PyroWave (default true) unlocks >1 GbE pacing when both wired endpoints confirm >1 GbE.
    assert_eq!(
        rate_bps_following_client(
            0,
            400_000,
            10_000 * M,
            false,
            500 * M,
            2_500 * M,
            follow_client_link_default(true),
        ),
        2_000 * M
    );
    // Conventional codecs (default false) stay held to 800 Mbps unless opted in.
    assert_eq!(
        rate_bps_following_client(
            0,
            150_000,
            10_000 * M,
            false,
            180 * M,
            2_500 * M,
            follow_client_link_default(false),
        ),
        800 * M
    );
}

/// The skip-ab legs of 2026-11-10 (PyroWave 1.4 Gbps, paced at 1.75 Gbps):
/// a 1 MiB buffer held under 5 ms and the socket refused whole 46-datagram
/// batches (10035). The paced sender's buffer holds 10 ms at its pace.
#[test]
fn the_paced_send_buffer_holds_ten_milliseconds_at_the_pace() {
    const MIB: usize = 1 << 20;
    // H.26x rates keep the buffer every video socket starts with.
    for bps in [0, 1, 150_000_000, 800_000_000, 838_000_000] {
        assert_eq!(paced_send_buffer(bps), SEND_BUFFER_FLOOR, "{bps}");
    }
    assert_eq!(paced_send_buffer(840_000_000), 2 * MIB);
    // 1.4 Gbps PyroWave's 1.25x pace, and a 2.5 GbE route's 95% ceiling.
    assert_eq!(paced_send_buffer(1_750_000_000), 3 * MIB);
    assert_eq!(paced_send_buffer(2_375_000_000), 3 * MIB);
    for bps in [840_000_000, 1_750_000_000, 2_375_000_000, 6_000_000_000] {
        let held = paced_send_buffer(bps) as f64 * 8. / bps as f64;
        assert!(held >= SEND_BUFFER_SPAN.as_secs_f64(), "{bps}: {held} s");
    }
    // A 10 GbE pace stops at the ceiling.
    assert_eq!(paced_send_buffer(9_500_000_000), SEND_BUFFER_CEILING);
    assert_eq!(paced_send_buffer(u64::MAX), SEND_BUFFER_CEILING);
    for bps in (0..40).map(|i| i * 250_000_000) {
        assert_eq!(paced_send_buffer(bps) % MIB, 0, "{bps}");
    }
}

#[test]
fn startup_frames_cap_batch_burst_and_keep_steady_state_unchanged() {
    let datagram = 1408;
    let steady = batch_payload_limit(800_000_000, datagram, false, 64 * 1024);
    assert!(steady > STARTUP_BATCH_CAP_BYTES);

    // Frames 1..=STARTUP_BATCH_FRAMES cap segmented bursts at STARTUP_BATCH_CAP_BYTES.
    for frame in 1..=STARTUP_BATCH_FRAMES {
        let limit = startup_batch_payload_limit(frame, 800_000_000, datagram, false, 64 * 1024);
        assert!(limit <= STARTUP_BATCH_CAP_BYTES, "frame {frame}: {limit}");
        assert!(limit >= datagram);
    }

    // Frame 0 (unset/legacy) and frames past startup preserve batch_payload_limit verbatim.
    for frame in [0, STARTUP_BATCH_FRAMES + 1, 1000] {
        assert_eq!(
            startup_batch_payload_limit(frame, 800_000_000, datagram, false, 64 * 1024),
            steady
        );
    }

    // Smaller caller limits stay respected during startup as well.
    assert_eq!(
        startup_batch_payload_limit(1, 800_000_000, datagram, false, 4096),
        batch_payload_limit(800_000_000, datagram, false, 4096)
    );
}

#[test]
fn enobufs_wait_widens_startup_window_without_changing_steady_state() {
    for frame in 1..=STARTUP_ENOBUFS_FRAMES {
        assert_eq!(enobufs_frame_wait(frame), STARTUP_ENOBUFS_WAIT);
    }
    assert_eq!(enobufs_frame_wait(0), SEND_FRAME_WAIT);
    assert_eq!(
        enobufs_frame_wait(STARTUP_ENOBUFS_FRAMES + 1),
        SEND_FRAME_WAIT
    );
}

#[test]
fn wireless_unknown_link_ceiling_caps_without_exceeding_route() {
    // 62.988 Mbps wireless stream on a wired 1 GbE host (800 Mbps route base):
    // capped at 4x bitrate instead of ratcheting to 800 Mbps.
    let cap = wireless_unknown_link_ceiling_bps(800_000_000, 62_988, 81_506_310);
    assert_eq!(cap, 62_988 * 1000 * WIRELESS_UNKNOWN_LINK_MULTIPLIER);

    // A slower physical route still wins over the 4x headroom cap.
    assert_eq!(
        wireless_unknown_link_ceiling_bps(100_000_000, 62_988, 81_506_310),
        100_000_000
    );

    // Extreme FEC / wire demand keeps its 1.25x headroom inside base_bps.
    assert_eq!(
        wireless_unknown_link_ceiling_bps(800_000_000, 20_000, 120_000_000),
        150_000_000
    );
}
