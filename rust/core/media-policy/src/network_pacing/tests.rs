//! Tests for wire-budget calculations, recovery caps, and PyroWave pacing.

use super::*;

#[test]
fn one_frame_waits_about_its_own_period_for_a_full_socket() {
    // 120 fps: one picture period, not the flat 20 ms cap.
    assert_eq!(
        frame_send_wait(Duration::from_micros(8_333)),
        Duration::from_micros(8_333)
    );
    // A slow stream holds at the cap.
    assert_eq!(frame_send_wait(Duration::from_millis(60)), SEND_FRAME_WAIT);
}

#[test]
fn two_frames_is_the_recovery_cap_floor() {
    // 64 FPS, a period f64 holds exactly: 857,421.875 B rounds up.
    let period = Duration::from_micros(15_625);
    assert_eq!(two_frames(219_500, period), 857_422);
    // A pace too slow for two frames still gets them.
    assert_eq!(
        recovery_frame_cap(1, 219_500, period, 20, 1392, false),
        857_422
    );
    assert_eq!(fec_whole_frame_limit(857_421, 219_500, period), None);
    assert_eq!(
        fec_whole_frame_limit(857_422, 219_500, period),
        Some(857_422)
    );
}
#[test]
fn unmeasured_and_wireless_pyrowave_routes_do_not_burst_at_the_host_link() {
    let demand = 200_000_000;
    let client = ClientPath {
        link_bps: 10_000_000_000,
        pace_bps: 0,
    };
    assert_eq!(
        pyrowave_route_rate_bps(
            0,
            180_000,
            10_000_000_000,
            false,
            demand,
            ClientPath::default()
        ),
        250_000_000
    );
    assert_eq!(
        pyrowave_route_rate_bps(0, 180_000, 10_000_000_000, true, demand, client),
        250_000_000
    );
    for cap in [0, 50_000, 900_000] {
        for host in [0, 100_000_000, 10_000_000_000] {
            let measured = ClientPath {
                pace_bps: 80_000_000,
                ..client
            };
            let rate = pyrowave_route_rate_bps(cap, 180_000, host, false, demand, measured);
            assert!(rate <= measured.pace_bps);
            if cap > 0 {
                assert!(rate <= cap as u64 * 1000);
            }
            let warnings = crate::TestWarnings::default();
            report_rate(&warnings, rate, demand, cap);
            assert!(
                warnings
                    .snapshot()
                    .iter()
                    .any(|w| w.code == "network_pacing")
            );
        }
    }
}

#[test]
fn an_unhinted_800_mbps_stream_stays_inside_a_one_gbe_client_ceiling() {
    // An Xbox Series S on 1 GbE behind a 2.5 GbE host reports no link or
    // pace hint. Wire demand already includes critical parity and headers,
    // so the 1.25x headroom must not burst above 1 GbE.
    assert_eq!(
        pyrowave_route_rate_bps(
            0,
            800_000,
            2_500_000_000,
            false,
            798_732_000,
            ClientPath::default()
        ),
        950_000_000
    );
    // A stream that cannot fit under 1 GbE keeps its headroom.
    assert_eq!(
        pyrowave_route_rate_bps(
            0,
            800_000,
            2_500_000_000,
            false,
            1_200_000_000,
            ClientPath::default()
        ),
        1_200_000_000 * 5 / 4
    );
}

#[test]
fn pyrowave_batches_fit_wire_time_and_segment_limits_for_both_ip_families() {
    for ipv6 in [false, true] {
        for rate in [1, 10_000_000, 100_000_000, 1_000_000_000, 10_000_000_000] {
            for packet in [272, 1072, 1440] {
                let payload = pyrowave_batch_limit(rate, packet, ipv6);
                let count = payload / packet;
                assert!(count >= 1);
                assert!(count <= 64);
                assert!(payload <= 64 * 1024);
                if count > 1 {
                    assert!(count as u64 * (packet + overhead(ipv6)) as u64 * 8000 <= rate);
                }
            }
        }
    }
}

#[test]
fn recovery_cap_matches_slow_route_and_relaxes_on_fast_routes() {
    let period = Duration::from_secs_f64(1. / 119.88);
    let average = 54_000. * 125. * period.as_secs_f64();
    let cap = |pace| recovery_frame_cap(pace, 54_000, period, 10, 1392, false);
    let slow = cap(70_770_000) as f64 / average;
    assert!((2.1..2.3).contains(&slow), "slow route: {slow} frames");
    assert_eq!(cap(800_000_000), (8. * average).floor() as u64);
    let wireless = cap(108_000_000) as f64 / average;
    assert!((3.2..3.4).contains(&wireless));
    assert!(cap(800_000_000) * 8 < 800_000_000 / 100);
}

#[test]
fn recovery_cap_accounts_for_parity_headers_and_two_frame_bounds() {
    for bitrate in [1, 2_000, 54_000, 200_000, u32::MAX] {
        for fps in [24., 59.94, 119.88, 240.] {
            let period = Duration::from_secs_f64(1. / fps);
            let average = f64::from(bitrate) * 125. * period.as_secs_f64();
            let min = (2. * average).ceil() as u64;
            let max = (8. * average).floor() as u64;
            for pace in [0, 70_000_000, 800_000_000, u64::MAX] {
                let plain = recovery_frame_cap(pace, bitrate, period, 0, 1392, false);
                let parity = recovery_frame_cap(pace, bitrate, period, 20, 1392, false);
                let ipv6 = recovery_frame_cap(pace, bitrate, period, 20, 1392, true);
                let smaller = recovery_frame_cap(pace, bitrate, period, 20, 512, true);
                assert!((min..=max).contains(&plain));
                assert!((min..=plain).contains(&parity));
                assert!((min..=parity).contains(&ipv6));
                assert!((min..=ipv6).contains(&smaller));
            }
        }
    }
    for packet in [0, 16, usize::MAX] {
        assert_eq!(
            recovery_frame_cap(u64::MAX, 0, Duration::ZERO, 100, packet, true),
            0
        );
    }
}
#[test]
fn the_fec_whole_cap_keeps_recovery_frames_within_full_parity_above_two_frames() {
    let period = Duration::from_secs_f64(1. / 60.);
    let packetizer = crate::packet::VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
        packet_size: 1392,
        fec_percent: 20,
        min_fec: 2,
        key: Some([0; 16]),
    };
    let whole = packetizer.fec_whole_payload_limit();
    // 200 Mbps at 60 fps on an 800 Mbps route: about 2.4 MB, no parity.
    let cap = recovery_frame_cap(800_000_000, 200_000, period, 20, 1392, false);
    assert!(packetizer.fec_limited(cap as usize));
    let held = fec_whole_frame_cap(cap, whole, 200_000, period);
    assert_eq!(held, whole as u64);
    assert!(!packetizer.fec_limited(held as usize));
    // A cap already within full parity is left as it is.
    let slow = recovery_frame_cap(100_000_000, 200_000, period, 20, 1392, false);
    assert_eq!(fec_whole_frame_cap(slow, whole, 200_000, period), slow);
    // Two nominal frames already past full parity (1.67 MB at 400 Mbps): no
    // cap at the floor keeps it, so the cap stays and the frame stays sharp.
    let fast = recovery_frame_cap(800_000_000, 400_000, period, 20, 1392, false);
    assert!((400_000. * 125. * period.as_secs_f64() * 2.).ceil() as u64 > whole as u64);
    assert_eq!(fec_whole_frame_cap(fast, whole, 400_000, period), fast);
    // The re-encode bound for scene cuts is the same: full parity where the
    // cap keeps it, none where a keyframe at the cap would lose it too.
    assert_eq!(
        fec_whole_frame_limit(whole, 200_000, period),
        Some(whole as u64)
    );
    assert_eq!(fec_whole_frame_limit(whole, 400_000, period), None);
}

#[test]
fn without_fec_the_cap_is_the_four_block_packet_limit() {
    let period = Duration::from_secs_f64(1. / 60.);
    let packetizer = crate::packet::VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
        packet_size: 1392,
        fec_percent: 0,
        min_fec: 2,
        key: Some([0; 16]),
    };
    // 255 data shards in each of four blocks: about 1.40 MB.
    let whole = packetizer.fec_whole_payload_limit();
    assert_eq!(whole, 4 * 255 * 1376 - 8);
    // The automatic cap, about 3.1 MB without parity, puts over 2,000
    // packets of a recovery frame at risk; held, it has 1,020.
    let cap = recovery_frame_cap(800_000_000, 200_000, period, 0, 1392, false);
    let held = fec_whole_frame_cap(cap, whole, 200_000, period);
    let packets = |payload: u64| {
        packetizer
            .block_layout(payload as usize)
            .iter()
            .map(|(data, parity)| data + parity)
            .sum::<usize>()
    };
    assert!(packets(cap) > 2000, "{}", packets(cap));
    assert_eq!(packets(held), 4 * 255);
}

#[test]
fn conventional_wire_budget_includes_framing_encryption_and_fec_rounding() {
    for ipv6 in [false, true] {
        for key in [None, Some([0; 16])] {
            let mut packetizer = crate::packet::VideoPacketizer {
                sequence: 0,
                iv_counter: 0,
                frame: 1,
                packet_size: 1392,
                fec_percent: 20,
                min_fec: 2,
                key,
            };
            let period = Duration::from_secs_f64(1. / 60.);
            let payload = (63_000. * 125. * period.as_secs_f64()).ceil() as usize;
            let needed = video_wire_rate_bps(&packetizer, 63_000, period, ipv6);
            let packets = packetizer.encode(&vec![0; payload], false, 0, 0).unwrap();
            let wire: usize = packets.iter().map(|p| p.len() + overhead(ipv6)).sum();
            assert_eq!(
                needed,
                (wire as f64 * 8. / period.as_secs_f64()).ceil() as u64
            );
            assert!(needed > 78_000_000);
            let warnings = crate::TestWarnings::default();
            report_rate(&warnings, 78_000_000, needed, 78_000);
            assert_eq!(warnings.snapshot()[0].code, "network_pacing");
        }
    }
}

#[test]
fn legacy_pyrowave_wire_budget_counts_container_headers_at_shard_boundaries() {
    for (fps, boundary_kbps, boundary_packets) in [(125, 912, 2), (50, 4768, 14)] {
        let period = Duration::from_secs_f64(1. / f64::from(fps));
        for kbps in boundary_kbps - 1..=boundary_kbps + 1 {
            for fec in [0, 20] {
                // Eight-byte codec packets maximize the length container's
                // overhead. Use the real container and transport encoder.
                let image_budget = crate::pyrowave::budget(kbps, period, 1392, false, fec);
                if image_budget < 16 {
                    // Only the low-rate parity case emits no frame;
                    // neither unprotected shard-boundary case is skipped.
                    assert_eq!((fps, fec, image_budget), (125, 20, 0));
                    continue;
                }
                let raw = vec![0; image_budget & !7];
                let records: Vec<_> = (0..raw.len()).step_by(8).map(|at| (at, 8)).collect();
                let frame = crate::pyrowave::container(&raw, &records).unwrap();
                let needed = pyrowave_wire_rate_bps(kbps, 1392, false, period);
                for ipv6 in [false, true] {
                    for key in [None, Some([0; 16])] {
                        let mut packetizer = crate::packet::VideoPacketizer {
                            sequence: 0,
                            iv_counter: 0,
                            frame: 1,
                            packet_size: 1392,
                            fec_percent: 0,
                            min_fec: 2,
                            key,
                        };
                        let packets = packetizer
                            .encode_pyrowave(
                                &frame,
                                0,
                                0,
                                crate::packet::PyrowaveFec {
                                    records: false,
                                    critical_percentage: fec as usize,
                                    detail_percentage: 0,
                                    wire_budget: 0,
                                    ipv6,
                                },
                            )
                            .unwrap();
                        let wire: usize = packets.iter().map(|p| p.len() + overhead(ipv6)).sum();
                        let actual = (wire as f64 * 8. / period.as_secs_f64()).ceil() as u64;
                        assert!(needed >= actual, "{fps}/{kbps}/{fec}: {needed} < {actual}");
                        if kbps == boundary_kbps && fec == 0 {
                            assert_eq!(packets.len(), boundary_packets);
                            if ipv6 && key.is_some() {
                                assert_eq!(needed, actual);
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn only_a_pacing_shortfall_warns_and_a_faster_link_clears_it() {
    let warnings = crate::TestWarnings::default();
    report_rate(
        &warnings,
        rate_bps(0, 100_000, 100_000_000, false, 120_000_000),
        120_000_000,
        0,
    );
    assert!(warnings.snapshot()[0].message.contains("80.0 Mbps"));
    report_rate(
        &warnings,
        rate_bps(0, 100_000, 1_000_000_000, false, 120_000_000),
        120_000_000,
        0,
    );
    assert!(warnings.snapshot().is_empty());
    report_rate(
        &warnings,
        rate_bps(50_000, 100_000, 0, false, 120_000_000),
        100_000_000,
        50_000,
    );
    assert_eq!(warnings.snapshot()[0].code, "network_pacing_floor");
    report_rate(
        &warnings,
        rate_bps(200_000, 100_000, 0, false, 120_000_000),
        100_000_000,
        200_000,
    );
    assert!(warnings.snapshot().is_empty());
}

#[test]
fn sender_selection_includes_fec_and_minimum_parity_in_frame_send_time() {
    // Dynamic automatic pacing reserves the sender independently of this
    // near-budget check; a healthy wired stream still starts fast.
    assert!(!paced(
        rate_bps(0, 80_000, 0, false, 100_000_000),
        100_000_000
    ));
    assert!(!paced(
        rate_bps(0, 80_000, 2_500_000_000, false, 100_000_000),
        100_000_000
    ));
    assert!(paced(
        rate_bps(160_000, 80_000, 0, false, 100_000_000),
        100_000_000
    ));
    assert!(paced(
        rate_bps(0, 50_000, 100_000_000, false, 65_000_000),
        65_000_000
    ));
    assert!(!paced(
        rate_bps(800_000, 80_000, 0, false, 100_000_000),
        100_000_000
    ));
    // Parity dominates a low-bitrate, high-refresh stream's wire demand.
    assert!(paced(rate_bps(0, 2_000, 0, true, 15_000_000), 15_000_000));
    assert!(paced(400_000_000, 100_000_000));
    assert!(!paced(400_000_001, 100_000_000));
    assert!(!paced(1, 0));
}

#[test]
fn large_frames_need_the_sender_even_when_average_traffic_fits_inline() {
    let period = Duration::from_secs_f64(1. / 90.);
    for bitrate in [80_000, 100_000] {
        for ipv6 in [false, true] {
            for key in [None, Some([7; 16])] {
                let mut packetizer = crate::packet::VideoPacketizer {
                    sequence: 0,
                    iv_counter: 0,
                    frame: 1,
                    packet_size: 1392,
                    fec_percent: 20,
                    min_fec: 2,
                    key,
                };
                let average = video_wire_rate_bps(&packetizer, bitrate, period, ipv6);
                let bps = rate_bps(0, bitrate, 1_000_000_000, false, average);
                assert!(!paced(bps, average));
                let payload = recovery_frame_cap(bps, bitrate, period, 20, 1392, ipv6);
                let needed = video_frame_wire_rate_bps(&packetizer, payload as usize, period, ipv6);
                assert!(paced(bps, needed));
                let packets = packetizer
                    .encode(&vec![0; payload as usize], true, 0, 0)
                    .unwrap();
                let bytes: usize = packets.iter().map(|p| p.len() + overhead(ipv6)).sum();
                assert_eq!(
                    needed,
                    (bytes as f64 * 8. / period.as_secs_f64()).ceil() as u64
                );
                let wire_time = Duration::from_secs_f64(bytes as f64 * 8. / bps as f64);
                assert!(wire_time > period);
                assert!(!paced(
                    bps,
                    video_frame_wire_rate_bps(&packetizer, 0, period, ipv6)
                ));
            }
        }
    }
}

#[test]
fn pyrowave_demand_cannot_override_an_explicit_cap_or_link_limit() {
    let demand = 512_000 * 60 * 8;
    assert_eq!(
        pyrowave_rate_bps(30_000, 30_000, 100_000_000, demand, ClientPath::default()),
        30_000_000
    );
    assert_eq!(
        pyrowave_rate_bps(100_000, 100_000, 100_000_000, demand, ClientPath::default()),
        95_000_000
    );
    assert_eq!(
        pyrowave_rate_bps(0, 30_000, 1_000_000_000, demand, ClientPath::default()),
        demand * 5 / 4
    );
    assert_eq!(
        pyrowave_rate_bps(0, 30_000, 0, demand, ClientPath::default()),
        demand * 5 / 4
    );
}

#[test]
fn pyrowave_paces_a_slower_client_port_at_its_own_rate() {
    const G: u64 = 1_000_000_000;
    let demand = 300_000_000;
    let link = |bps| ClientPath {
        link_bps: bps,
        ..Default::default()
    };
    // Issue #31: a 2.5 GbE client on a 10 GbE host, and a 1 GbE client on
    // a 2.5 GbE host, as the client reports its link.
    assert_eq!(
        pyrowave_rate_bps(0, 270_000, 10 * G, demand, link(5 * G / 2)),
        2_375_000_000
    );
    assert_eq!(
        pyrowave_rate_bps(0, 270_000, 5 * G / 2, demand, link(G)),
        950_000_000
    );
    // A client as fast as the host keeps the link rate: no added send time.
    assert_eq!(
        pyrowave_rate_bps(0, 270_000, 10 * G, demand, link(10 * G)),
        9_500_000_000
    );
    // Calibration remains a hard ceiling, even below the stream's demand.
    let pace = |bps| ClientPath {
        pace_bps: bps,
        link_bps: 10 * G,
    };
    assert_eq!(
        pyrowave_rate_bps(0, 270_000, 10 * G, demand, pace(2 * G)),
        2 * G
    );
    assert_eq!(
        pyrowave_rate_bps(0, 270_000, 10 * G, demand, pace(100_000_000)),
        100_000_000
    );
    assert_eq!(
        pyrowave_rate_bps(0, 270_000, G, demand, pace(2 * G)),
        950_000_000
    );
    // An overloaded stream must not overrun a slower client link.
    assert_eq!(
        pyrowave_rate_bps(0, 270_000, 10 * G, demand, link(100_000_000)),
        95_000_000
    );
    // Explicit caps and client evidence also apply without a host speed.
    assert_eq!(
        pyrowave_rate_bps(100_000, 100_000, 10 * G, demand, link(G)),
        100_000_000
    );
    assert_eq!(
        pyrowave_rate_bps(0, 270_000, 0, demand, link(G)),
        950_000_000
    );
}

mod link_and_burst;
