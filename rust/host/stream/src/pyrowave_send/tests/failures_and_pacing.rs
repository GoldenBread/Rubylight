//! Unit tests for PyroWave block failure abandonment, detail FEC drop warnings, and 1080p budget pacing.

use super::super::*;
use butterpollo_win_graphics::timing::Timer;
use butterpollo_win_io::net::Batch;
use std::net::UdpSocket;

/// Review 2026-11-07: a block that fails after block 0 left the frame to
/// go out in part and count as sent. One the sender has not taken is
/// dropped with its blocks; one in hand is flagged, and the sender takes
/// no more of it.
#[test]
fn a_failed_block_abandons_a_frame_handed_over_block_by_block() {
    for taken in [false, true] {
        let fixture = crate::state::test_support::Fixture::new();
        let launch = fixture.launch(
            fixture.client(0),
            butterpollo_session::session::Role::Stream,
        );
        let session = Session::new(launch, Default::default());
        let sender = Sender::packetizer(
            session,
            fixture.host.config.read().unwrap().clone(),
            fixture.host.clone(),
            Instant::now(),
            None,
        )
        .unwrap();
        let slot = &sender.slot;
        let block = |tag: u8| SentBlock::from_packets(vec![vec![tag; 4]]);
        let mut step = 0;
        let blocks = std::iter::from_fn(|| {
            step += 1;
            match step {
                1 => Some(Some(block(1))),
                2 => {
                    if taken {
                        let mut ready = slot.ready.lock().unwrap();
                        assert!(ready.frame.take().is_some());
                        ready.sending = true;
                    }
                    Some(Some(block(2)))
                }
                3 => Some(None),
                _ => Some(Some(block(4))),
            }
        });
        let now = Instant::now();
        let handed = slot.hand_over(blocks, |block| Prepared {
            blocks: vec![block],
            peer: "127.0.0.1:12345".parse().unwrap(),
            claimed: now,
            processing: 0,
            latency: 0,
            age: 0,
            wire_frame: 1,
            demand: 0,
            kbps: 0,
            claim: None,
            polled: now,
            packetized: now,
            prepared: now,
            timestamp: 0,
            streamed: true,
        });
        assert!(!handed);
        assert_eq!(step, 3, "no block after the failed one");
        {
            let ready = slot.ready.lock().unwrap();
            assert!(ready.frame.is_none() && ready.blocks.is_empty() && !ready.open);
            assert_eq!(ready.abandoned, taken);
        }
        assert_eq!(transport::next_block(slot), None);
        // The blocks nobody sends go back to the pool: both untaken, or
        // the one queued behind the frame in hand. Pooled buffers are
        // refilled whole, so their order does not matter.
        if taken {
            assert_eq!(slot.pool.take(), block(2).packets);
        } else {
            let mut pooled = [slot.pool.take(), slot.pool.take()];
            pooled.sort();
            assert_eq!(pooled, [block(1).packets, block(2).packets]);
        }
        assert!(slot.pool.take().is_empty());
    }
}

/// Review 2026-11-17: a frame the sender gave up on a full socket left its
/// later blocks queued, so the sender never looked idle again (no frame went
/// out block by block for the rest of the session) and the blocks never went
/// back to the pool. The ones still being built are waited for and returned
/// too.
#[test]
fn a_stalled_frame_returns_its_blocks_and_leaves_the_sender_idle() {
    for building in [false, true] {
        let fixture = crate::state::test_support::Fixture::new();
        let launch = fixture.launch(
            fixture.client(0),
            butterpollo_session::session::Role::Stream,
        );
        let session = Session::new(launch, Default::default());
        let sender = Sender::packetizer(
            session,
            fixture.host.config.read().unwrap().clone(),
            fixture.host.clone(),
            Instant::now(),
            None,
        )
        .unwrap();
        let slot = &sender.slot;
        let block = |tag: u8| SentBlock::from_packets(vec![vec![tag; 4]]);
        {
            let mut ready = slot.ready.lock().unwrap();
            ready.sending = true;
            ready.open = building;
            ready.blocks.push_back(block(3));
            ready.blocks.push_back(block(4));
        }
        std::thread::scope(|scope| {
            if building {
                // The packetizer hands over one more block, then the frame
                // is complete.
                scope.spawn(|| {
                    std::thread::sleep(Duration::from_millis(30));
                    let mut ready = slot.ready.lock().unwrap();
                    ready.blocks.push_back(block(5));
                    ready.open = false;
                    drop(ready);
                    slot.ready_changed.notify_one();
                });
            }
            transport::discard_rest(slot, vec![block(2)].into_iter(), true);
        });
        {
            let mut ready = slot.ready.lock().unwrap();
            // As the sender does once the frame is over.
            ready.sending = false;
            assert!(ready.idle(), "building {building}");
        }
        let mut pooled: Vec<_> = std::iter::from_fn(|| Some(slot.pool.take()))
            .take_while(|block| !block.is_empty())
            .collect();
        pooled.sort();
        let mut expected = vec![block(2).packets, block(3).packets, block(4).packets];
        if building {
            expected.push(block(5).packets);
        }
        assert_eq!(pooled, expected, "building {building}");
    }
}

/// The paced sender sends a streamed frame's later blocks too, and only
/// then counts the frame as sent.
#[test]
fn a_streamed_frame_goes_out_whole() {
    let fixture = crate::state::test_support::Fixture::new();
    let launch = fixture.launch(
        fixture.client(0),
        butterpollo_session::session::Role::Stream,
    );
    let session = Session::new(
        launch,
        butterpollo_media_policy::rtsp::Negotiated {
            codec: 3,
            bitrate_kbps: 80_000,
            ..Default::default()
        },
    );
    let client = UdpSocket::bind("127.0.0.1:0").unwrap();
    let sender = Sender::new(
        Arc::new(ReplySocket::bind("127.0.0.1:0").unwrap()),
        session.clone(),
        fixture.host.config.read().unwrap().clone(),
        fixture.host.clone(),
        Instant::now(),
        None,
    )
    .unwrap();
    sender
        .submit(
            vec![Encoded {
                framing: None,
                bytes: vec![42; 400_000],
                unit: butterpollo_win_graphics::encoder::Unit::Whole,
                idr: true,
                after_invalidation: false,
                frame_cap_bytes: None,
                reencoded: false,
                frame_cap_target: None,
                latency: None,
                presentation: None,
            }],
            client.local_addr().unwrap(),
            Duration::ZERO,
            Instant::now(),
            &mut Claims::default(),
        )
        .unwrap();
    for _ in 0..400 {
        if session.stats.frames.load(Ordering::Relaxed) > 0 {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(session.stats.frames.load(Ordering::Relaxed), 1);
    // Block 0 holds at most 255 packets; the frame needs about 290 and parity.
    assert!(session.stats.packets.load(Ordering::Relaxed) > 255);
    assert!(!sender.slot.ready.lock().unwrap().sending);
}

#[test]
fn failed_block_abandons_the_frame_and_the_next_frame_keeps_its_counters() {
    for fail_at in [0, 1] {
        let payload = vec![7u8; 400_000];
        let mut packetizer = VideoPacketizer {
            sequence: u32::MAX - 10,
            iv_counter: 100,
            frame: u32::MAX,
            packet_size: 1392,
            fec_percent: 0,
            min_fec: 2,
            key: Some([42; 16]),
        };
        let fec = || PyrowaveFec {
            records: false,
            critical_percentage: 20,
            detail_percentage: 0,
            wire_budget: 0,
            ipv6: false,
        };
        let mut dropped = DroppedFrames::default();
        let warnings = Warnings::default();
        let now = Instant::now();
        let mut sent = 0;
        {
            let (_, _, mut blocks) = packetizer
                .pyrowave_blocks(&payload, 9000, 543, fec())
                .unwrap();
            let mut prepared = 0;
            let failing = std::iter::from_fn(|| {
                let result = if prepared == fail_at {
                    Some(Err(anyhow::anyhow!("injected block preparation failure")))
                } else {
                    blocks.next()
                };
                prepared += 1;
                result
            });
            for block in failing {
                let Some(packets) = dropped.prepared(block, payload.len(), now, &warnings) else {
                    break;
                };
                sent += packets.len();
            }
            assert_eq!(prepared, fail_at + 1);
        }
        assert_eq!(warnings.snapshot()[0].code, "pyrowave_frame");
        let sequence = (u32::MAX - 10).wrapping_add(sent as u32);
        let frame = if fail_at == 0 { u32::MAX } else { 0 };
        assert_eq!(packetizer.sequence, sequence);
        assert_eq!(packetizer.iv_counter, 100 + sent as u64);
        assert_eq!(packetizer.frame, frame);
        let packets = dropped
            .prepared(
                packetizer.encode_pyrowave(&payload, 9750, 543, fec()),
                payload.len(),
                now,
                &warnings,
            )
            .unwrap();
        assert_eq!(&packets[0][..8], &(100 + sent as u64).to_le_bytes());
        assert_eq!(&packets[0][12..16], &frame.to_le_bytes());
        assert_eq!(
            packetizer.sequence,
            sequence.wrapping_add(packets.len() as u32)
        );
        assert_eq!(packetizer.iv_counter, 100 + (sent + packets.len()) as u64);
        assert_eq!(packetizer.frame, frame.wrapping_add(1));
    }
}

#[test]
fn malformed_frame_after_a_late_interval_is_dropped_by_detail_planning() {
    // Detail planning parses records only after a late frame. That parse
    // used to end the session through `?` before block preparation ran.
    let mut controller = DetailFec::new(60_000);
    let mut dropped = DroppedFrames::default();
    let warnings = Warnings::default();
    let now = Instant::now();
    let malformed = [0; 64];
    assert_eq!(
        dropped.prepared(
            controller.observe(&malformed, None, now, 100_000),
            64,
            now,
            &warnings
        ),
        Some((0, 0))
    );
    assert!(warnings.snapshot().is_empty());
    let late = now + Duration::from_millis(50);
    assert!(
        dropped
            .prepared(
                controller.observe(&malformed, None, late, 100_000),
                64,
                late,
                &warnings
            )
            .is_none()
    );
    assert_eq!(dropped.reported, Some(late));
    assert_eq!(warnings.snapshot()[0].code, "pyrowave_frame");
}

#[test]
fn frame_drop_warnings_report_the_first_failure_and_count_repeats() {
    let mut dropped = DroppedFrames::default();
    let warnings = Warnings::default();
    let now = Instant::now();
    for (millis, unreported) in [(0, 0), (1, 1), (4999, 2), (5000, 0)] {
        warnings.clear("pyrowave_frame");
        assert!(
            dropped
                .prepared::<()>(
                    Err(anyhow::anyhow!("bad frame")),
                    123,
                    now + Duration::from_millis(millis),
                    &warnings,
                )
                .is_none()
        );
        assert_eq!(dropped.unreported, unreported);
        assert_eq!(warnings.snapshot()[0].code, "pyrowave_frame");
        if millis == 0 || millis == 5000 {
            assert_eq!(dropped.reported, Some(now + Duration::from_millis(millis)));
        }
    }
    assert_eq!(
        dropped.prepared(Ok(42), 123, now + Duration::from_secs(6), &warnings),
        Some(42)
    );
    assert_eq!(dropped.reported, Some(now + Duration::from_secs(5)));
}

/// An encrypted 1080p60 frame filling the encoder's 400 Mbps budget, as
/// the sender's blocks, and the rate the sender paces it at.
fn budget_sized_1080p_frame() -> (Vec<Vec<Vec<u8>>>, u64, Duration) {
    use butterpollo_media_policy::pyrowave;
    let (packet_size, kbps, fps_millihz) = (1360, 400_000, 60_000);
    let period = Duration::from_secs_f64(1000. / f64::from(fps_millihz));
    let budget = pyrowave::budget(kbps, period, packet_size, true, 20);
    let size = |i: usize| 8 + (i * 73 % 504) * 4;
    let (mut count, mut total) = (0, 8);
    while total + size(count) <= budget {
        total += size(count);
        count += 1;
    }
    let mut raw = Vec::with_capacity(total);
    raw.extend_from_slice(&(0x80000000u32 | 1919 | (1079 << 14)).to_le_bytes());
    raw.extend_from_slice(&(count as u32).to_le_bytes());
    for i in 0..count {
        raw.extend_from_slice(&((size(i) as u32 / 4) << 16).to_le_bytes());
        raw.extend_from_slice(&((((i * 37) % count) as u32) << 8).to_le_bytes());
        raw.extend((0..size(i) - 8).map(|j| (i * 31 + j * 17) as u8));
    }
    let framed = pyrowave::record_frame(
        &raw,
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
        key: Some([42; 16]),
    };
    let fec = PyrowaveFec {
        records: true,
        critical_percentage: 20,
        detail_percentage: 0,
        wire_budget: 0,
        ipv6: false,
    };
    let (packets, _, blocks) = packetizer.pyrowave_blocks(&framed, 0, 0, fec).unwrap();
    let blocks = blocks.collect::<Result<Vec<_>>>().unwrap();
    let demand = demand_bps(packets, packet_size, true, false, fps_millihz);
    let bps = butterpollo_media_policy::network_pacing::pyrowave_rate_bps(
        0,
        kbps,
        0,
        demand,
        Default::default(),
    );
    (blocks, bps, period)
}

#[test]
fn paced_budget_sized_1080p_frame_leaves_within_85_percent_of_the_period() {
    let (blocks, bps, period) = budget_sized_1080p_frame();
    // Loopback costs on the RX 7900 XT machine: a 64 KB segmented send
    // takes about 0.2 ms and the precise timer wakes up to 0.1 ms late.
    let (send, wake) = (Duration::from_micros(250), Duration::from_micros(100));
    let start = Instant::now();
    let mut pacer = butterpollo_media_policy::network_pacing::Pacer::new(start);
    let mut now = start;
    let mut batches = 0;
    for block in &blocks {
        let mut remaining = block.as_slice();
        while !remaining.is_empty() {
            if pacer.due() > now {
                now = pacer.due() + wake;
            }
            let count = Batch::count(remaining, batch_limit(bps, remaining[0].len(), false));
            let bytes = remaining[..count].iter().map(Vec::len).sum();
            now += send;
            pacer.sent(now, bytes, count, false, bps);
            remaining = &remaining[count..];
            batches += 1;
        }
    }
    let spent = now - start;
    assert!(
        spent <= period.mul_f64(0.85),
        "{batches} batches at {bps} bps took {spent:?} of a {period:?} frame"
    );
}

/// Timing-dependent: run at normal priority on an otherwise idle machine
/// with `cargo test -p butterpollo pyrowave_loopback -- --ignored --nocapture`.
#[test]
#[ignore = "measures real loopback sends; needs an idle machine"]
fn pyrowave_loopback_sends_a_budget_sized_1080p_frame_within_85_percent_of_the_period() -> Result<()>
{
    let (blocks, bps, period) = budget_sized_1080p_frame();
    let receiver = UdpSocket::bind("127.0.0.1:0")?;
    socket2::SockRef::from(&receiver).set_recv_buffer_size(64 << 20)?;
    receiver.set_read_timeout(Some(Duration::from_millis(50)))?;
    let peer = receiver.local_addr()?;
    let stop = Arc::new(AtomicBool::new(false));
    let drain = {
        let stop = stop.clone();
        thread::spawn(move || {
            let mut buffer = [0; 2048];
            while !stop.load(Ordering::Relaxed) {
                let _ = receiver.recv(&mut buffer);
            }
        })
    };
    let socket = ReplySocket::bind("127.0.0.1:0")?;
    socket.set_nonblocking(true)?;
    butterpollo_win_io::net::configure_udp(&socket)?;
    let (timer, mut batch) = (Timer::new()?, Batch::default());
    let mut pacer = butterpollo_media_policy::network_pacing::Pacer::new(Instant::now());
    let mut spans = Vec::new();
    for _ in 0..180 {
        let begin = Instant::now();
        for block in &blocks {
            let mut remaining = block.as_slice();
            while !remaining.is_empty() {
                if pacer.due() > Instant::now() {
                    timer.until_paced(pacer.due());
                }
                let count = Batch::count(remaining, batch_limit(bps, remaining[0].len(), false));
                let bytes = batch.send(&socket, &remaining[..count], peer)?;
                pacer.sent(Instant::now(), bytes, count, false, bps);
                remaining = &remaining[count..];
            }
        }
        spans.push(begin.elapsed());
        timer.until_precise(begin + period);
    }
    stop.store(true, Ordering::Relaxed);
    drain.join().unwrap();
    spans.sort();
    let median = spans[spans.len() / 2];
    println!(
        "{bps} bps, frame send median {median:?}, p95 {:?}, max {:?}, period {period:?}, dropped {}",
        spans[spans.len() * 95 / 100],
        spans[spans.len() - 1],
        batch.dropped
    );
    assert!(median <= period.mul_f64(0.85));
    Ok(())
}
