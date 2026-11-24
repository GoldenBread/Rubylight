//! Unit tests for sliced streamed-frame delivery, overflow fallback, send-drop handling, and Wi-Fi jitter stalls.

use super::super::*;
use super::*;

#[test]
fn a_streamed_frames_units_wait_in_the_pending_slot_through_a_keyframe_request() {
    let (entered, frames) = mpsc::channel();
    let (release, resume) = mpsc::channel::<()>();
    let sender = Sender::spawn("units".into(), 1, move |slot| {
        slot.run(|frame| {
            entered.send(frame.encoded.unit).unwrap();
            resume.recv_timeout(WAIT)?;
            Ok(1)
        })
    })
    .unwrap();
    submit_units(&sender, vec![slice(0, 1, 1)]);
    assert_eq!(
        frames.recv_timeout(WAIT).unwrap(),
        butterpollo_win_graphics::encoder::Unit::Slice { index: 0 }
    );
    submit_units(&sender, vec![last(1, 1, 2)]);
    // A whole frame waiting is skipped, a unit of a frame being sent is not.
    assert_eq!(sender.backlog().unwrap(), 2);
    assert_eq!(sender.skip_pending().unwrap(), 2);
    release.send(()).unwrap();
    assert_eq!(
        frames.recv_timeout(WAIT).unwrap(),
        butterpollo_win_graphics::encoder::Unit::Last { index: 1 }
    );
    release.send(()).unwrap();
    assert_eq!(sender.next_wire_frame().unwrap(), 1);
}

#[test]
fn only_a_streamed_frames_first_unit_takes_the_pictures_claim() {
    let (entered, frames) = mpsc::channel();
    let sender = Sender::spawn("claims".into(), 1, move |slot| {
        slot.run(|frame| {
            entered.send(frame.claim.is_some()).unwrap();
            Ok(1)
        })
    })
    .unwrap();
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
    let mut first = slice(0, 1, 1);
    first.presentation = Some(now);
    let mut second = last(1, 1, 2);
    second.presentation = Some(now);
    sender
        .submit(vec![first, second], peer(), Duration::ZERO, &mut claims)
        .unwrap();
    assert!(frames.recv_timeout(WAIT).unwrap());
    assert!(!frames.recv_timeout(WAIT).unwrap());
}

#[test]
fn the_packetizer_thread_sends_each_block_of_a_streamed_frame_as_one_part() {
    let fixture = crate::state::test_support::Fixture::new();
    let session = fixture.session(
        fixture.client(0),
        butterpollo_session::session::Role::Stream,
    );
    let sender = streaming_sender(session, 2, 2);
    submit_units(&sender, vec![slice(0, 20, 1)]);
    // Block 0 is ready before the frame's last unit exists.
    let first = take_prepared(&sender);
    assert_eq!(
        first.stream,
        Some(Part {
            first: true,
            last: false
        })
    );
    assert_eq!(
        (first.wire_frame, first.idr, first.counts.len()),
        (1, false, 1)
    );
    assert_eq!(first.layout.len(), 1);
    assert_eq!(first.counts[0], first.layout[0].0 + first.layout[0].1);
    assert_eq!(first.blocks[0].len(), first.counts[0]);
    // The next frame's number is spent as soon as block 0 is built.
    assert_eq!(first.payload_bytes, 20 * 1008);
    submit_units(&sender, vec![last(1, 20, 2)]);
    let second = take_prepared(&sender);
    assert_eq!(
        second.stream,
        Some(Part {
            first: false,
            last: true
        })
    );
    assert_eq!(second.wire_frame, 1);
    assert_eq!(second.payload_bytes, 40 * 1008);
    assert_eq!(sender.next_wire_frame().unwrap(), 2);
    // The next frame follows, whole.
    submit_units(&sender, vec![frame(2)]);
    let next = take_prepared(&sender);
    assert_eq!((next.stream, next.wire_frame), (None, 2));
}

#[test]
fn streamed_frames_follow_the_block_count_of_a_rebuilt_encoder() {
    let fixture = crate::state::test_support::Fixture::new();
    let session = fixture.session(
        fixture.client(0),
        butterpollo_session::session::Role::Stream,
    );
    // Handed off at 1440p, whole frames; a switch to 4K brings slices.
    let sender = streaming_sender(session.clone(), 0, 2);
    sender.follow_slice_blocks(2);
    submit_units(&sender, vec![slice(0, 20, 1)]);
    let first = take_prepared(&sender);
    assert_eq!(first.stream.map(|part| part.first), Some(true));
    // A switch back while the frame is open: it ends with the blocks it began with.
    sender.follow_slice_blocks(0);
    submit_units(&sender, vec![last(1, 20, 2)]);
    let second = take_prepared(&sender);
    assert_eq!(second.stream.map(|part| part.last), Some(true));
    assert_eq!(sender.next_wire_frame().unwrap(), 2);
    submit_units(&sender, vec![frame(2)]);
    assert_eq!(take_prepared(&sender).stream, None);
    assert_eq!(
        session
            .stats
            .slice_delivery_overflows
            .load(Ordering::Relaxed),
        0
    );
}

#[test]
fn a_prepared_part_of_a_streamed_frame_survives_a_keyframe_request() {
    let fixture = crate::state::test_support::Fixture::new();
    let session = fixture.session(
        fixture.client(0),
        butterpollo_session::session::Role::Stream,
    );
    let sender = streaming_sender(session, 2, 2);
    submit_units(&sender, vec![slice(0, 20, 1)]);
    drop(wait_prepared(&sender));
    sender.skip_pending().unwrap();
    assert!(sender.slot.state.lock().unwrap().ready.is_some());
    assert_eq!(take_prepared(&sender).stream.map(|p| p.first), Some(true));
}

#[test]
fn a_frame_that_begins_over_an_open_streamed_frame_takes_its_number_after_it() {
    let fixture = crate::state::test_support::Fixture::new();
    let session = fixture.session(
        fixture.client(0),
        butterpollo_session::session::Role::Stream,
    );
    let sender = streaming_sender(session, 2, 2);
    // One shard: nothing can leave before the last unit.
    submit_units(&sender, vec![slice(0, 1, 1)]);
    // The last unit never comes.
    submit_units(&sender, vec![frame(2)]);
    let next = take_prepared(&sender);
    assert_eq!((next.stream, next.wire_frame), (None, 2));
    // A frame interrupted after block 0 keeps the number it took.
    submit_units(&sender, vec![slice(0, 20, 1)]);
    let part = take_prepared(&sender);
    assert_eq!(
        (part.wire_frame, part.stream.map(|p| p.first)),
        (3, Some(true))
    );
    submit_units(&sender, vec![frame(3)]);
    let next = take_prepared(&sender);
    assert_eq!((next.stream, next.wire_frame), (None, 4));
}

#[test]
fn a_streamed_frame_the_plan_cannot_hold_is_dropped_and_turns_delivery_off() {
    let fixture = crate::state::test_support::Fixture::new();
    let session = fixture.session(
        fixture.client(0),
        butterpollo_session::session::Role::Stream,
    );
    let sender = streaming_sender(session.clone(), 2, 2);
    // A frame that grows past what the encoder reported it could return.
    let mut first = slice(0, 2, 1);
    first.frame_cap_bytes = Some(4 * 1008);
    let mut second = last(1, 10, 2);
    second.frame_cap_bytes = Some(4 * 1008);
    submit_units(&sender, vec![first, second]);
    submit_units(&sender, vec![frame(2)]);
    let next = take_whole(&sender);
    assert_eq!(next.wire_frame, 2);
    assert_eq!(
        session
            .stats
            .slice_delivery_overflows
            .load(Ordering::Relaxed),
        1
    );
    let delivery = crate::stream::slices::Delivery::new(2);
    delivery.sync(&session.stats);
    assert!(delivery.joins());
    // A cap no plan holds fails the frame's first unit.
    let mut huge = slice(0, 1, 1);
    huge.frame_cap_bytes = Some(5000 * 1008);
    submit_units(&sender, vec![huge, last(1, 1, 2)]);
    submit_units(&sender, vec![frame(2)]);
    // The refused frame spent number 3.
    let next = take_whole(&sender);
    assert_eq!(next.wire_frame, 4);
    assert_eq!(
        session
            .stats
            .slice_delivery_overflows
            .load(Ordering::Relaxed),
        2
    );
}

/// The shards of a streamed frame's packets in wire order: its payload.
fn streamed_payload(packets: &[Vec<u8>]) -> (Vec<u8>, usize) {
    let mut data = packets
        .iter()
        .filter_map(|packet| {
            let info = u32::from_le_bytes(packet[28..32].try_into().unwrap());
            let (index, count) = ((info >> 12) & 0x3ff, info >> 22);
            let block = (packet[27] >> 4) & 3;
            (index < count).then_some(((block, index), packet))
        })
        .collect::<Vec<_>>();
    data.sort_by_key(|(key, _)| *key);
    let blocks = packets
        .iter()
        .map(|packet| usize::from(packet[27] >> 6) + 1)
        .max()
        .unwrap_or(0);
    (
        data.iter()
            .flat_map(|(_, packet)| packet[32..].iter().copied())
            .collect(),
        blocks,
    )
}

#[test]
fn a_streamed_frame_goes_out_in_order_and_is_timed_once() {
    let fixture = crate::state::test_support::Fixture::new();
    let session = fixture.session(
        fixture.client(0),
        butterpollo_session::session::Role::Stream,
    );
    let socket = Arc::new(ReplySocket::bind("127.0.0.1:0").unwrap());
    let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
    receiver
        .set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    let now = Instant::now();
    let epoch = now - Duration::from_millis(500);
    let mut handoff = handoff(packetizer(None));
    handoff.slice_blocks = 2;
    handoff.slices = 2;
    let sender = Sender::new(
        socket,
        session.clone(),
        fixture.host.config.read().unwrap().clone(),
        fixture.host.clone(),
        epoch,
        handoff,
    )
    .unwrap();
    let (sink, rows) = butterpollo_media_policy::timing_record::channel(8);
    *session.timing_record.write().unwrap() = Some(Arc::new(sink));
    let mut claims = Claims::default();
    let key = now - Duration::from_millis(6);
    claims.push(Claim {
        key,
        presented: key,
        acquired: key,
        published: key,
        claimed: key,
        submitted: key,
    });
    let mut first = slice(0, 20, 1);
    first.presentation = Some(key);
    let mut second = last(1, 20, 2);
    second.presentation = Some(key);
    let addr = receiver.local_addr().unwrap();
    sender
        .submit_tick(vec![first], addr, Duration::ZERO, &mut claims, now)
        .unwrap();
    // The first block reaches the client before the encoder produced the rest.
    let mut packets = Vec::new();
    let mut buffer = [0; 2048];
    // The packetizer thread sends it: on a busy PC it stalled for over 500 ms,
    // before the first packet or within the block.
    receiver.set_read_timeout(Some(WAIT)).unwrap();
    while packets.len() <= 20 {
        let len = receiver.recv(&mut buffer).unwrap();
        packets.push(buffer[..len].to_vec());
    }
    receiver
        .set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    while let Ok(len) = receiver.recv(&mut buffer) {
        packets.push(buffer[..len].to_vec());
    }
    let early_packets = packets.len();
    assert!(early_packets > 20, "{early_packets}");
    assert!(rows.try_recv().is_err(), "the frame is not over yet");
    sender
        .submit_tick(vec![second], addr, Duration::ZERO, &mut claims, now)
        .unwrap();
    assert_eq!(sender.next_wire_frame().unwrap(), 2);
    while let Ok(len) = receiver.recv(&mut buffer) {
        packets.push(buffer[..len].to_vec());
    }
    assert!(packets.len() > early_packets);
    assert!(
        packets
            .iter()
            .all(|packet| packet[20..24] == 1u32.to_le_bytes())
    );
    // On the wire in order: blocks never go back, and the RTP sequence and the
    // stream packet index step by one per packet across the parts.
    let block = |packet: &Vec<u8>| (packet[27] >> 4) & 3;
    assert_eq!(block(&packets[0]), 0);
    assert_eq!(block(packets.last().unwrap()), 1);
    for pair in packets.windows(2) {
        assert!(block(&pair[0]) <= block(&pair[1]));
        let sequence = |p: &Vec<u8>| u16::from_be_bytes(p[2..4].try_into().unwrap());
        assert_eq!(sequence(&pair[1]), sequence(&pair[0]).wrapping_add(1));
    }
    // A data shard carries its packet index (the FEC covers it, so parity
    // shards do not): it is the same count.
    for packet in &packets {
        let info = u32::from_le_bytes(packet[28..32].try_into().unwrap());
        if (info >> 12) & 0x3ff < info >> 22 {
            let index = u32::from_le_bytes(packet[16..20].try_into().unwrap()) >> 8;
            assert_eq!(
                index,
                u32::from(u16::from_be_bytes(packet[2..4].try_into().unwrap()))
            );
        }
    }
    let (stream, blocks) = streamed_payload(&packets);
    assert_eq!(blocks, 2);
    let mut expected = vec![0; 8];
    expected.extend(std::iter::repeat_n(1u8, 20 * 1008));
    expected.extend(std::iter::repeat_n(2u8, 20 * 1008));
    assert_eq!(stream[8..8 + 40 * 1008], expected[8..]);
    // One record, one set of stage timings, for the whole frame.
    let row = rows.try_recv().unwrap();
    assert!(rows.try_recv().is_err());
    assert_eq!(row.wire_frame, 1);
    let points = row.points;
    // Packetizing began on the first unit and the first block went out; the last
    // unit was in hand only after that, and the record says how many blocks left
    // before it.
    assert!(points[6] <= points[7] && points[7] <= points[8]);
    assert!(
        points[8] < points[5],
        "the first block left before the last unit"
    );
    assert!(points[5] <= points[9]);
    let first_unit = row.first_unit_us.unwrap();
    assert!(first_unit <= points[6] && first_unit < points[5]);
    assert_eq!(row.early_blocks, Some(1));
    assert_eq!(session.info()["performance"]["stages"]["frames"], 1);
}

/// A streamed frame's first block, then a whole frame: the open frame is
/// abandoned on the packetizer thread, and the sender must account for the
/// second frame alone.
#[test]
fn a_frame_cut_off_after_its_first_block_leaves_no_stats_or_timing_behind() {
    let fixture = crate::state::test_support::Fixture::new();
    let session = fixture.session(
        fixture.client(0),
        butterpollo_session::session::Role::Stream,
    );
    let socket = Arc::new(ReplySocket::bind("127.0.0.1:0").unwrap());
    let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
    // Block 0 comes from the packetizer thread: on a busy PC after 500 ms.
    receiver.set_read_timeout(Some(WAIT)).unwrap();
    let now = Instant::now();
    let mut handoff = handoff(packetizer(None));
    handoff.slice_blocks = 2;
    handoff.slices = 2;
    let sender = Sender::new(
        socket,
        session.clone(),
        fixture.host.config.read().unwrap().clone(),
        fixture.host.clone(),
        now - Duration::from_millis(500),
        handoff,
    )
    .unwrap();
    let (sink, rows) = butterpollo_media_policy::timing_record::channel(8);
    *session.timing_record.write().unwrap() = Some(Arc::new(sink));
    let mut claims = Claims::default();
    for ms in [8, 4] {
        let key = now - Duration::from_millis(ms);
        claims.push(Claim {
            key,
            presented: key,
            acquired: key,
            published: key,
            claimed: key,
            submitted: key,
        });
    }
    let addr = receiver.local_addr().unwrap();
    let mut cut = slice(0, 20, 1);
    cut.presentation = Some(now - Duration::from_millis(8));
    sender
        .submit_tick(vec![cut], addr, Duration::ZERO, &mut claims, now)
        .unwrap();
    // Block 0 is on the wire.
    let mut buffer = [0; 2048];
    receiver.recv(&mut buffer).unwrap();
    let mut whole = frame(2);
    whole.presentation = Some(now - Duration::from_millis(4));
    sender
        .submit_tick(vec![whole], addr, Duration::ZERO, &mut claims, now)
        .unwrap();
    assert_eq!(sender.next_wire_frame().unwrap(), 3);
    // One frame was accounted: the whole one, which is number 2.
    let row = rows.try_recv().unwrap();
    assert_eq!(row.wire_frame, 2);
    assert!(rows.try_recv().is_err());
    assert_eq!(session.stats.frames.load(Ordering::Relaxed), 1);
    let performance = &session.info()["performance"];
    assert_eq!(performance["stages"]["frames"], 1);
    assert_eq!(performance["sample_frames"], 1);
}

/// The paced sender with the test loss given directly: block 0 of a streamed
/// frame loses more than its parity, so block 1 is never offered.
#[test]
fn a_streamed_frame_lost_in_its_first_block_stops_and_is_not_stamped() {
    let fixture = crate::state::test_support::Fixture::new();
    let session = fixture.session(
        fixture.client(0),
        butterpollo_session::session::Role::Stream,
    );
    session.link_sent.set_live(true);
    let socket = Arc::new(ReplySocket::bind("127.0.0.1:0").unwrap());
    let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
    receiver
        .set_read_timeout(Some(Duration::from_millis(300)))
        .unwrap();
    let now = Instant::now();
    let mut handoff = handoff(packetizer(None));
    handoff.slice_blocks = 2;
    handoff.slices = 2;
    let mut sender = Sender::packetizer(session.clone(), now, handoff, false).unwrap();
    let (sink, rows) = butterpollo_media_policy::timing_record::channel(8);
    *session.timing_record.write().unwrap() = Some(Arc::new(sink));
    let key = now - Duration::from_millis(4);
    let mut claims = Claims::default();
    claims.push(Claim {
        key,
        presented: key,
        acquired: key,
        published: key,
        claimed: key,
        submitted: key,
    });
    let addr = receiver.local_addr().unwrap();
    let mut first = slice(0, 20, 1);
    first.presentation = Some(key);
    sender
        .submit_tick(vec![first], addr, Duration::ZERO, &mut claims, now)
        .unwrap();
    // Block 0's packet count, read before the paced sender starts.
    let block0 = wait_prepared(&sender).ready.as_ref().unwrap().counts[0];
    let shared = sender.slot.clone();
    let s = session.clone();
    let config = fixture.host.config.read().unwrap().clone();
    let host = fixture.host.clone();
    sender.transport = Some(thread::spawn(move || {
        let result = transport::run_with(
            &shared,
            socket,
            s,
            config,
            host,
            now,
            Instant::now(),
            Default::default(),
            butterpollo_media_policy::stream_policy::SendDrop::parse("100:5"),
            None,
            Default::default(),
        );
        shared.finish(result);
    }));
    sender
        .submit_tick(vec![last(1, 20, 2)], addr, Duration::ZERO, &mut claims, now)
        .unwrap();
    assert_eq!(sender.next_wire_frame().unwrap(), 2);
    let stats = &session.stats;
    // Only block 0 was offered to the socket, and it was refused.
    let offered = stats.packets.load(Ordering::Relaxed) as usize;
    assert!(offered > 0 && offered <= block0, "{offered} of {block0}");
    let mut buffer = [0; 2048];
    assert!(
        receiver.recv(&mut buffer).is_err(),
        "nothing reached the client"
    );
    // The unrecoverable frame is counted, asks for a recovery frame, and has
    // no stage timings or record.
    assert_eq!(stats.send_loss_recoveries.load(Ordering::Relaxed), 1);
    assert!(rows.try_recv().is_err());
    assert_eq!(session.info()["performance"]["stages"]["frames"], 0);
    // The link-rate controller is told nothing left: neither the refused
    // block 0 nor the block 1 that was never offered.
    let mut recorded = Vec::new();
    session.link_sent.drain(&mut 0, |frame, packets, bytes, _| {
        recorded.push((frame, packets, bytes))
    });
    assert_eq!(recorded, [(1, 0, 0)]);
}
/// The paced sender with a test Wi-Fi stall given directly: it takes each
/// frame during the stall at once, so the backlog empties and claims carry
/// on, and the frames reach the client in order only once the stall is over.
#[test]
fn frames_during_a_test_stall_count_as_sent_and_go_out_in_order_after_it() {
    use butterpollo_media_policy::stream_policy::SendJitter;
    let ms = Duration::from_millis;
    // A seed whose first stall starts after the sender does and leaves ample
    // time to hand it three frames, even on a busy PC; `until` is the first
    // instant after it (and any stall straight after it) that is not held.
    let step = Duration::from_micros(100);
    let (jitter, from, until) = (1..)
        .find_map(|seed| {
            let jitter = SendJitter::parse(&format!("1000:600:{seed}")).unwrap();
            let mut probe = jitter.clone();
            let mut times = (0u32..).map(|i| i * step);
            let from = times.find(|&at| probe.stalled_until(at).is_some()).unwrap();
            let until = times.find(|&at| probe.stalled_until(at).is_none()).unwrap();
            (from >= ms(300) && until - from >= ms(400) && until <= ms(2000))
                .then_some((jitter, from, until))
        })
        .unwrap();
    let fixture = crate::state::test_support::Fixture::new();
    let session = fixture.session(
        fixture.client(0),
        butterpollo_session::session::Role::Stream,
    );
    let socket = Arc::new(ReplySocket::bind("127.0.0.1:0").unwrap());
    let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
    let start = Instant::now();
    let mut sender =
        Sender::packetizer(session.clone(), start, handoff(packetizer(None)), false).unwrap();
    let shared = sender.slot.clone();
    let s = session.clone();
    let config = fixture.host.config.read().unwrap().clone();
    let host = fixture.host.clone();
    sender.transport = Some(thread::spawn(move || {
        let result = transport::run_with(
            &shared,
            socket,
            s,
            config,
            host,
            start,
            Instant::now(),
            Default::default(),
            None,
            Some(jitter),
            Default::default(),
        );
        shared.finish(result);
    }));
    let addr = receiver.local_addr().unwrap();
    let mut claims = Claims::default();
    thread::sleep((start + from + ms(5)).saturating_duration_since(Instant::now()));
    for index in [2u8, 4, 5] {
        sender
            .submit(vec![frame(index)], addr, Duration::ZERO, &mut claims)
            .unwrap();
        // Taken and counted as sent while the stall lasts.
        sender.next_wire_frame().unwrap();
        assert_eq!(sender.backlog().unwrap(), 0);
    }
    let sent = session.stats.packets.load(Ordering::Relaxed) as usize;
    assert_eq!(session.stats.frames.load(Ordering::Relaxed), 3);
    assert!(
        Instant::now() < start + until,
        "the stall outlasts the handoff"
    );
    receiver.set_read_timeout(Some(WAIT)).unwrap();
    let mut buffer = [0; 2048];
    let mut packets = Vec::new();
    while packets.len() < sent {
        let len = receiver.recv(&mut buffer).unwrap();
        if packets.is_empty() {
            // Nothing during the stall, then the burst right after it.
            let arrived = Instant::now();
            assert!(
                arrived >= start + until,
                "{:?} early",
                start + until - arrived
            );
            let late = arrived - (start + until);
            assert!(late < ms(100), "released {late:?} after the stall");
        }
        packets.push(buffer[..len].to_vec());
    }
    // In the order they were sent: frame by frame, sequence by sequence.
    let order: Vec<(u32, u16)> = packets
        .iter()
        .map(|packet| {
            (
                u32::from_le_bytes(packet[20..24].try_into().unwrap()),
                u16::from_be_bytes([packet[2], packet[3]]),
            )
        })
        .collect();
    assert!(order.is_sorted(), "{order:?}");
    assert_eq!(order.first().unwrap().0, 1);
    assert_eq!(order.last().unwrap().0, 3);
}

#[test]
fn an_open_streamed_frame_counts_toward_the_claim_backlog_only() {
    let fixture = crate::state::test_support::Fixture::new();
    let session = fixture.session(
        fixture.client(0),
        butterpollo_session::session::Role::Stream,
    );
    let sender = streaming_sender(session, 2, 2);
    submit_units(&sender, vec![slice(0, 20, 1)]);
    take_prepared(&sender);
    // The packetizer is idle again; the frame still waits for its last unit.
    {
        let state = sender.slot.state.lock().unwrap();
        let (state, timeout) = sender
            .slot
            .changed
            .wait_timeout_while(state, WAIT, |s| s.sending || s.pending.is_some())
            .unwrap();
        assert!(!timeout.timed_out());
        assert!(state.streaming);
    }
    assert_eq!(sender.backlog().unwrap(), 0);
    assert_eq!(sender.claim_backlog(None).unwrap(), 1);
    // The recreated encoder need not wait for it.
    assert_eq!(sender.next_wire_frame().unwrap(), 2);
    submit_units(&sender, vec![last(1, 20, 2)]);
    take_prepared(&sender);
    {
        let state = sender.slot.state.lock().unwrap();
        let (state, timeout) = sender
            .slot
            .changed
            .wait_timeout_while(state, WAIT, |s| s.sending || s.streaming)
            .unwrap();
        assert!(!timeout.timed_out());
        assert_eq!(state.claim_backlog(None).unwrap(), 0);
    }
    // A frame that begins over an open one closes it.
    submit_units(&sender, vec![slice(0, 20, 1)]);
    take_prepared(&sender);
    submit_units(&sender, vec![frame(2)]);
    take_prepared(&sender);
    let state = sender.slot.state.lock().unwrap();
    let (state, timeout) = sender
        .slot
        .changed
        .wait_timeout_while(state, WAIT, |s| s.sending || s.streaming)
        .unwrap();
    assert!(!timeout.timed_out());
    assert_eq!(state.claim_backlog(None).unwrap(), 0);
}
