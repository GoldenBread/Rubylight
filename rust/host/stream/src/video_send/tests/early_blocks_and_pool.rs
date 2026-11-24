//! Unit tests for block-by-block whole-frame handoff, recovery lead progress, and packet block pooling.

use super::super::*;
use super::*;

/// Review 2026-11-07: the sender thread (031dc57e5) built every block of a
/// whole frame before the first left. An idle sender gets each block as it
/// is built, as parts of one frame that counts once in the backlog, and the
/// first part carries the frame's wire bytes for the recovery estimate.
#[test]
fn an_idle_sender_gets_a_whole_frame_block_by_block() {
    let fixture = crate::state::test_support::Fixture::new();
    let session = fixture.session(
        fixture.client(0),
        butterpollo_session::session::Role::Stream,
    );
    let sender = early_sender(session);
    submit_units(&sender, vec![keyframe(600)]);
    let first = take_prepared(&sender);
    assert_eq!(
        (first.stream, first.whole, first.idr, first.wire_frame),
        (
            Some(Part {
                first: true,
                last: false
            }),
            true,
            true,
            1
        )
    );
    let layout = first.packetizer.block_layout(600 * 1008);
    assert!(layout.len() >= 3, "600 shards take three blocks or more");
    assert_eq!(first.layout, vec![layout[0]]);
    assert_eq!(first.counts, vec![layout[0].0 + layout[0].1]);
    let packets: usize = layout.iter().map(|(data, parity)| data + parity).sum();
    assert_eq!(
        first.frame_wire_bytes,
        Some(packets * (1040 + butterpollo_media_policy::network_pacing::overhead(false)))
    );
    assert_eq!(first.blocks[0][0].len(), 1040);
    // The paced sender sends block 0 while the packetizer hands over block 1
    // and waits to hand over block 2: one frame in the backlog, not three.
    sender.slot.state.lock().unwrap().transmitting = true;
    {
        let state = wait_prepared(&sender);
        assert!(state.whole_open);
        assert_eq!(state.backlog().unwrap(), 1);
        assert_eq!(state.claim_backlog(None).unwrap(), 2, "a keyframe");
    }
    sender.slot.state.lock().unwrap().transmitting = false;
    for (index, block) in layout.iter().enumerate().skip(1) {
        let part = take_prepared(&sender);
        assert_eq!(
            (part.stream, part.frame_wire_bytes, part.counts),
            (
                Some(Part {
                    first: false,
                    last: index + 1 == layout.len()
                }),
                None,
                InlineBlocks::one(block.0 + block.1)
            )
        );
    }
    assert_eq!(sender.next_wire_frame().unwrap(), 2);
    // A busy sender gets the next frame whole, which a keyframe can skip.
    sender.slot.state.lock().unwrap().transmitting = true;
    submit_units(&sender, vec![frame(2)]);
    let whole = take_prepared(&sender);
    assert_eq!(
        (whole.stream, whole.whole, whole.frame_wire_bytes),
        (None, true, None)
    );
}

/// Every packet of a frame sent block by block reaches the client in order,
/// and the frame is timed once, as a whole frame and not a streamed one.
#[test]
fn a_whole_frame_sent_block_by_block_arrives_whole_and_is_timed_once() {
    let fixture = crate::state::test_support::Fixture::new();
    let session = fixture.session(
        fixture.client(0),
        butterpollo_session::session::Role::Stream,
    );
    let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
    receiver.set_read_timeout(Some(WAIT)).unwrap();
    // Slow enough that the receive buffer never overflows.
    let mut config = fixture.host.config.read().unwrap().clone();
    config
        .values
        .insert("packet_pacing".into(), "100000".into());
    let sender = Sender::new(
        Arc::new(ReplySocket::bind("127.0.0.1:0").unwrap()),
        session.clone(),
        config,
        fixture.host.clone(),
        Instant::now() - Duration::from_millis(500),
        handoff(packetizer(None)),
    )
    .unwrap();
    let (sink, rows) = butterpollo_media_policy::timing_record::channel(8);
    *session.timing_record.write().unwrap() = Some(Arc::new(sink));
    let now = Instant::now();
    let mut claims = Claims::default();
    claims.push(Claim {
        key: now,
        presented: now,
        acquired: now,
        published: now,
        claimed: now,
        submitted: now,
    });
    let mut picture = keyframe(600);
    picture.presentation = Some(now);
    sender
        .submit_tick(
            vec![picture],
            receiver.local_addr().unwrap(),
            Duration::ZERO,
            &mut claims,
            now,
        )
        .unwrap();
    let mut buffer = [0; 2048];
    let mut sequences = Vec::new();
    while let Ok(len) = receiver.recv(&mut buffer) {
        assert_eq!(len, 1040);
        sequences.push(u16::from_be_bytes(buffer[2..4].try_into().unwrap()));
        receiver
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
    }
    assert!(sequences.len() > 255, "more than block 0");
    assert_eq!(
        sequences.len() as u64,
        session.stats.packets.load(Ordering::Relaxed)
    );
    for pair in sequences.windows(2) {
        assert_eq!(pair[1], pair[0].wrapping_add(1));
    }
    let row = rows.recv_timeout(WAIT).unwrap();
    assert_eq!(row.wire_frame, 1);
    assert_eq!((row.first_unit_us, row.early_blocks), (None, None));
    assert!(rows.try_recv().is_err());
    assert_eq!(sender.next_wire_frame().unwrap(), 2);
}

/// Review 2026-11-07: a keyframe sent block by block closed admission at
/// every block boundary, and the packetizer's claim held it closed until the
/// last block was queued. From its first part the remaining-time estimate
/// decides, across all of its parts.
#[test]
fn a_keyframe_sent_block_by_block_opens_admission_at_its_lead_across_parts() {
    let fixture = crate::state::test_support::Fixture::new();
    let session = fixture.session(
        fixture.client(0),
        butterpollo_session::session::Role::Stream,
    );
    let sender = early_sender(session);
    let lead = Some(Duration::from_millis(4));
    sender.claim_backlog(lead).unwrap();
    submit_units(&sender, vec![keyframe(600)]);
    let slot = &sender.slot;
    let mut seen = Vec::new();
    slot.transmit(|frame| {
        let Some(frame) = frame else {
            return Ok(false);
        };
        let part = frame.stream.unwrap();
        {
            let state = slot.state.lock().unwrap();
            seen.push((
                part.first,
                state.backlog().unwrap(),
                state.claim_backlog(lead).unwrap(),
            ));
        }
        if part.first {
            assert!(slot.recovery_progress(Some(Duration::from_millis(1))));
        }
        if part.last {
            slot.stop();
        }
        Ok(false)
    })
    .unwrap();
    assert!(seen.len() >= 3, "600 shards take three blocks or more");
    assert_eq!(seen[0], (true, 1, 2), "closed until the estimate");
    for &later in &seen[1..] {
        assert_eq!(later, (false, 1, 1), "open at the lead");
    }
}

/// Keyframe progress wakes the stream thread when the remaining time
/// crosses its lead, either way, and not on the batches in between.
#[test]
fn recovery_progress_stays_quiet_until_the_lead_is_crossed() {
    let fixture = crate::state::test_support::Fixture::new();
    let session = fixture.session(
        fixture.client(0),
        butterpollo_session::session::Role::Stream,
    );
    let sender = early_sender(session);
    let ms = |ms| Some(Duration::from_millis(ms));
    sender.claim_backlog(ms(4)).unwrap();
    assert!(
        !sender.slot.recovery_progress(ms(1)),
        "no keyframe in flight"
    );
    sender.slot.state.lock().unwrap().transmitting_keyframe = true;
    let woke: Vec<_> = [ms(10), ms(6), ms(5), ms(4), ms(3), ms(1), None, ms(9)]
        .into_iter()
        .map(|remaining| sender.slot.recovery_progress(remaining))
        .collect();
    assert_eq!(woke, [false, false, false, true, false, false, true, false]);
}

/// The estimate counts a whole frame sent block by block once, from its
/// first part; a streamed frame part by part.
#[test]
fn the_recovery_estimate_counts_a_whole_frame_from_its_first_part() {
    use super::super::transport::count_remaining;
    let whole = count_remaining((0, false), 1_000, Some(3_000));
    assert_eq!(whole, (3_000, true));
    assert_eq!(count_remaining(whole, 1_000, None), (3_000, true));
    let streamed = count_remaining((0, false), 1_000, None);
    assert_eq!(streamed, (1_000, false));
    assert_eq!(count_remaining(streamed, 1_200, None), (2_200, false));
}

/// A block that does not match the frame's layout gives up the frame after
/// its first block went out: the error comes back, the rest is not queued,
/// its buffers are recycled, and a sender waiting for the next block no
/// longer holds a keyframe in flight.
#[test]
fn a_whole_frame_given_up_part_way_queues_no_more_blocks() {
    let fixture = crate::state::test_support::Fixture::new();
    let session = fixture.session(
        fixture.client(0),
        butterpollo_session::session::Role::Stream,
    );
    let sender = early_sender(session);
    let slot = &sender.slot;
    let picture = keyframe(600);
    let mut wire = packetizer(None);
    let mut layout = wire.block_layout(picture.bytes.len());
    assert!(layout.len() >= 3);
    layout[1].1 += 1;
    let plan = butterpollo_media_policy::packet::ConventionalPlan {
        layout: layout.iter().copied().collect(),
        percentage: wire.conventional_plan(picture.bytes.len()).percentage,
    };
    let template = wire.clone();
    let now = Instant::now();
    let make = |blocks, counts, block, part, frame_wire_bytes| Prepared {
        blocks,
        counts,
        layout: InlineBlocks::one(block),
        packetizer: template.clone(),
        wire_frame: 1,
        idr: true,
        after_invalidation: false,
        frame_cap_bytes: None,
        cap_target_bytes: None,
        reencoded: false,
        fec_limited: false,
        payload_bytes: picture.bytes.len(),
        peer: peer(),
        claimed: now,
        polled: now,
        last_polled: now,
        early_blocks: 0,
        packetized: now,
        prepared: now,
        latency: 0,
        processing: 0,
        age: 0,
        claim: None,
        timestamp: 0,
        stream: Some(part),
        whole: true,
        frame_wire_bytes,
    };
    let dropped = std::thread::scope(|scope| {
        let send = scope.spawn(|| {
            early::send(
                slot,
                &mut wire,
                &picture,
                0,
                0,
                &layout,
                &plan,
                peer(),
                make,
            )
            .unwrap()
        });
        // The sender takes block 0 and waits for block 1.
        let first = take_prepared(&sender);
        assert!(first.stream.unwrap().first);
        let mut state = slot.state.lock().unwrap();
        state.whole_open = true;
        state.transmitting_keyframe = true;
        state.transmitting_remaining = Some(Duration::from_millis(1));
        drop(state);
        send.join().unwrap()
    });
    let error = dropped.expect("the frame was given up");
    assert!(format!("{error:#}").contains("block layout"), "{error:#}");
    slot.abandon_whole();
    let state = slot.state.lock().unwrap();
    assert!(state.ready.is_none());
    assert!(!state.whole_open && !state.transmitting_keyframe);
    assert_eq!(state.transmitting_remaining, None);
    assert_eq!(state.recycled.len(), 1, "block 1's buffers");
}
/// Whole frames and blocks sent one at a time draw on one pool: after warm-up,
/// alternating cycles allocate no block and the pool stays within its cap.
#[test]
fn block_pool_serves_whole_and_block_by_block_frames_without_allocating() {
    let mut state = empty_state();
    let mut misses = 0;
    // A whole frame of four blocks stays in flight for two more frames; a
    // block-by-block frame is one block, recycled as soon as it is sent.
    let mut in_flight = std::collections::VecDeque::new();
    let mut warmed_up = None;
    for round in 0..24 {
        if round % 2 == 0 {
            let frame: Vec<_> = (0..4)
                .map(|_| build_block(&mut state, &mut misses))
                .collect();
            in_flight.push_back(frame);
            if in_flight.len() > 2 {
                let mut spill = Vec::new();
                state.recycle(in_flight.pop_front().unwrap(), &mut spill);
                assert!(spill.is_empty());
            }
        } else {
            let block = build_block(&mut state, &mut misses);
            let mut spill = Vec::new();
            state.recycle(vec![block], &mut spill);
            assert!(spill.is_empty());
        }
        assert!(state.recycled.len() <= RECYCLED_BLOCKS);
        if round == 6 {
            warmed_up = Some(misses);
        }
    }
    assert!(misses > 0, "the first cycles must allocate");
    assert_eq!(warmed_up, Some(misses), "blocks allocated after warm-up");
    assert!(!state.recycled.is_empty());
}

/// Blocks past the cap are handed back for the caller to free outside the lock,
/// and the pool keeps exactly its cap.
#[test]
fn block_pool_hands_blocks_past_its_cap_to_the_caller() {
    let mut state = empty_state();
    let mut misses = 0;
    let blocks: Vec<_> = (0..RECYCLED_BLOCKS + 4)
        .map(|_| build_block(&mut state, &mut misses))
        .collect();
    let mut spill = Vec::new();
    state.recycle(blocks, &mut spill);
    assert_eq!(state.recycled.len(), RECYCLED_BLOCKS);
    assert_eq!(spill.len(), 4);
}
