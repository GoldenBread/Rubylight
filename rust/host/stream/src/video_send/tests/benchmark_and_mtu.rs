//! Unit tests for route MTU warnings, socket accounting, and the manual sender handoff benchmark.

use super::super::*;
use super::*;

#[test]
fn a_shrunken_route_mtu_only_warns_when_a_jumbo_packet_does_not_fit() {
    // 8192 + 64 fits a 9000-byte path, not a 1500- or 1280-byte one.
    assert!(mtu_carries(9000, 8192));
    assert!(!mtu_carries(8192, 8192));
    assert!(!mtu_carries(1500, 8192));
    let warning = mtu_warning(1280, 8192);
    assert!(warning.contains("1280"), "{warning}");
    assert!(warning.contains("8192"), "{warning}");
    assert!(warning.contains("64 bytes of headers"), "{warning}");
    // Only this session's warning list changes; a stock packet size fits.
    let f = crate::state::test_support::Fixture::new();
    let client = f.client(u32::MAX);
    let s = f.session(client, butterpollo_session::session::Role::Stream);
    check_route_mtu(&s, 8192, Some(1280));
    let warnings = s.launch.warnings.snapshot();
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].code, "network_mtu");
    assert!(warnings[0].message.contains("1280"));
    s.launch.warnings.clear("network_mtu");
    check_route_mtu(&s, 1400, Some(1280));
    assert!(s.launch.warnings.snapshot().is_empty());
}
/// A manual CPU-only measurement of the production queue and worker handoff.
/// Run with `cargo test --release -p butterpollo --bin butterpollo
/// sender_handoff_benchmark -- --ignored --nocapture --test-threads=1`.
/// This deliberately uses `spawn`, not `new`: no capture priority, timer,
/// socket, display, installed host or streaming scope is created. It measures
/// encrypted packetization plus handoff, not network or end-to-end latency.
#[test]
#[ignore = "manual release-mode CPU-only sender handoff benchmark"]
fn sender_handoff_benchmark() -> Result<()> {
    anyhow::ensure!(
        !cfg!(debug_assertions),
        "run sender_handoff_benchmark with --release"
    );
    const WARMUP: usize = 32;
    const SECONDS: usize = 3;
    const BITRATE: usize = 50_000_000;

    struct Sample {
        index: usize,
        started: Instant,
        work_started: Instant,
        completed: Instant,
        packets: usize,
        bytes: usize,
        digest: u64,
    }
    fn work(packetizer: &mut VideoPacketizer, encoded: Encoded) -> Result<Sample> {
        let work_started = Instant::now();
        let started = encoded.presentation.unwrap();
        let index = u64::from_le_bytes(encoded.bytes[..8].try_into().unwrap()) as usize;
        anyhow::ensure!(packetizer.frame == index as u32 + 1, "frame order changed");
        let (packets, bytes, digest) = {
            let packets = packetizer.encode_recovery(
                &encoded.bytes,
                encoded.idr,
                encoded.after_invalidation,
                index as u32 * 1000,
                0,
            )?;
            let bytes = packets.iter().map(Vec::len).sum();
            // Consume the complete result and retain a reproducible digest,
            // including headers, encrypted payload and parity. Identical
            // inline/threaded runs must produce exactly the same packets.
            let digest = packets.iter().flatten().fold(0u64, |sum, byte| {
                sum.rotate_left(5).wrapping_add(u64::from(*byte))
            });
            (packets.len(), bytes, std::hint::black_box(digest))
        };
        Ok(Sample {
            index,
            started,
            work_started,
            completed: Instant::now(),
            packets,
            bytes,
            digest,
        })
    }
    fn packetizer() -> VideoPacketizer {
        VideoPacketizer {
            sequence: 0,
            iv_counter: 0,
            frame: 1,
            packet_size: 1392,
            fec_percent: 20,
            min_fec: 2,
            key: Some([17; 16]),
        }
    }
    fn distribution(values: &[f64]) -> serde_json::Value {
        let mut sorted = values.to_vec();
        sorted.sort_by(f64::total_cmp);
        let percentile = |percent: usize| sorted[(sorted.len() * percent).div_ceil(100) - 1];
        serde_json::json!({
            "mean": values.iter().sum::<f64>() / values.len() as f64,
            "p50": percentile(50),
            "p95": percentile(95),
            "p99": percentile(99),
            "max": sorted[sorted.len() - 1],
        })
    }
    fn run(fps: usize, threaded: bool, round: usize) -> Result<serde_json::Value> {
        let period = Duration::from_secs_f64(1. / fps as f64);
        let (finished, results) = mpsc::channel();
        let sender = if threaded {
            Some(Sender::spawn("handoff-benchmark".into(), 1, move |slot| {
                let mut packetizer = packetizer();
                slot.run(|frame| {
                    anyhow::ensure!(frame.skipped == 0, "benchmark unexpectedly skipped a frame");
                    let sample = work(&mut packetizer, frame.encoded)?;
                    finished.send(sample)?;
                    Ok(packetizer.frame)
                })
            })?)
        } else {
            None
        };
        let mut inline_packetizer = packetizer();
        let mut due = Instant::now();
        let mut measured = Vec::with_capacity(fps * SECONDS);
        let mut late_frames = 0usize;
        let mut total_bytes = 0usize;
        let mut total_packets = 0usize;
        let mut digest = 0u64;
        for index in 0..WARMUP + fps * SECONDS {
            let mut encoded = Encoded {
                framing: None,
                bytes: vec![index as u8; (BITRATE / 8 / fps).max(8)],
                unit: butterpollo_win_graphics::encoder::Unit::Whole,
                idr: index == 0,
                after_invalidation: false,
                frame_cap_bytes: None,
                reencoded: false,
                frame_cap_target: None,
                latency: None,
                presentation: None,
            };
            encoded.bytes[..8].copy_from_slice(&(index as u64).to_le_bytes());
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                thread::sleep(wait);
            }
            let started = Instant::now();
            if started.saturating_duration_since(due) >= period {
                if index >= WARMUP {
                    late_frames += 1;
                }
                due = started;
            }
            due += period;
            encoded.presentation = Some(started);
            // Both paths own the same single-output Vec as the real caller.
            let mut output = vec![encoded];
            let sample = if let Some(sender) = &sender {
                sender.submit(output, peer(), Duration::ZERO, &mut Claims::default())?;
                results.recv_timeout(WAIT)?
            } else {
                work(&mut inline_packetizer, output.pop().unwrap())?
            };
            anyhow::ensure!(sample.index == index, "completion order changed");
            total_packets += sample.packets;
            total_bytes += sample.bytes;
            digest = digest.rotate_left(7).wrapping_add(sample.digest);
            if index >= WARMUP {
                measured.push(sample);
            }
        }
        if let Some(sender) = &sender {
            anyhow::ensure!(
                sender.next_wire_frame()? == (WARMUP + fps * SECONDS + 1) as u64,
                "worker did not finish every frame"
            );
        }
        let micros = |duration: Duration| duration.as_secs_f64() * 1_000_000.;
        let completed: Vec<_> = measured
            .iter()
            .map(|s| micros(s.completed.duration_since(s.started)))
            .collect();
        let waiting: Vec<_> = measured
            .iter()
            .map(|s| micros(s.work_started.duration_since(s.started)))
            .collect();
        let working: Vec<_> = measured
            .iter()
            .map(|s| micros(s.completed.duration_since(s.work_started)))
            .collect();
        let elapsed = measured.last().unwrap().started - measured[0].started;
        Ok(serde_json::json!({
            "mode": if threaded { "sender" } else { "inline" },
            "round": round,
            "fps_requested": fps,
            "fps_observed": (measured.len() - 1) as f64 / elapsed.as_secs_f64(),
            "warmup_frames": WARMUP,
            "sample_frames": measured.len(),
            "late_by_full_period": late_frames,
            "packets_including_warmup": total_packets,
            "bytes_including_warmup": total_bytes,
            "digest_including_warmup": digest,
            "submit_to_complete_us": distribution(&completed),
            "submit_to_work_us": distribution(&waiting),
            "work_us": distribution(&working),
            "raw_submit_to_complete_us": completed,
            "raw_submit_to_work_us": waiting,
            "raw_work_us": working,
        }))
    }

    let mut runs = Vec::new();
    for fps in [60, 120, 240] {
        let first = runs.len();
        // Alternate ordering to reduce a systematic warm-cache/clock bias.
        for (round, order) in [[false, true], [true, false]].into_iter().enumerate() {
            for threaded in order {
                runs.push(run(fps, threaded, round + 1)?);
            }
        }
        for run in &runs[first + 1..] {
            for field in [
                "packets_including_warmup",
                "bytes_including_warmup",
                "digest_including_warmup",
            ] {
                anyhow::ensure!(
                    run[field] == runs[first][field],
                    "{field} changed at {fps} FPS"
                );
            }
        }
    }
    println!(
        "SENDER_HANDOFF_BENCHMARK={}",
        serde_json::json!({
            "version": 1,
            "release": !cfg!(debug_assertions),
            "workload": "50 Mbps payload, 1392-byte packets, 20% FEC, encrypted; full packet digest",
            "scope": "Sender::spawn/submit/Slot::run at normal thread priority; CPU only; completion timestamp precedes result notification",
            "runs": runs,
        })
    );
    Ok(())
}
#[test]
fn only_what_reached_the_socket_is_counted_for_the_link_rate_controller() {
    use butterpollo_media_policy::network_pacing::overhead;
    // Ten offered, three refused: seven datagrams and the bytes the socket took.
    assert_eq!(
        reached_socket(10, 3, 7000, false),
        (7, (7000 + 7 * overhead(false)) as u64)
    );
    assert_eq!(
        reached_socket(10, 0, 10_000, true),
        (10, (10_000 + 10 * overhead(true)) as u64)
    );
    assert_eq!(reached_socket(4, 4, 0, false), (0, 0));
}
