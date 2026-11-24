//! Unit tests for the RTP video queue (`RtpQueue`).

use super::*;
use crate::video::tests::config;

/// More packets than a test block uses, so one word covers it whichever way it wraps.
const BLOCK_PACKETS: u16 = 33;

fn packet(seq: u16) -> Packet {
    Packet {
        buf: vec![0u8; 64].into_boxed_slice(),
        len: 32,
        seq,
        timestamp: 0,
        receive_time_us: 0,
        presentation_time_us: 0,
        rtp_timestamp: 0,
        is_parity: false,
    }
}

/// A queue whose block already started at `lowest`.
fn queue_at(lowest: u16) -> RtpQueue {
    let mut queue = RtpQueue::new(&config(0x1000));
    queue.buffer_lowest_seq = lowest;
    queue.buffer_highest_seq = lowest.wrapping_add(BLOCK_PACKETS - 1);
    queue.next_contiguous_seq = lowest;
    queue.received_highest_seq = 0;
    queue.use_fast_queue_path = true;
    queue.seen = [0; 57];
    queue.seen_words = 0;
    queue
}

/// A block past the signed-16 midpoint must not read its first packet as out of order.
#[test]
fn a_block_starting_at_or_above_0x8000_takes_its_first_packet_in_order() {
    for lowest in [0x8000u16, 0x8001, 0xFFFF, 0x0000] {
        let mut queue = queue_at(lowest);
        assert!(
            queue.queue_packet(packet(lowest), false, false).is_ok(),
            "lowest {lowest:#06x}"
        );
        assert!(!queue.received_oos_data, "lowest {lowest:#06x}");
        assert_eq!(queue.next_contiguous_seq, lowest.wrapping_add(1));
    }
}

/// A duplicate after a reordered packet is rejected, inside the contiguous run or behind it.
#[test]
fn a_duplicate_is_rejected_under_reordering() {
    let mut queue = queue_at(0x8000);
    assert!(queue.queue_packet(packet(0x8002), false, false).is_ok());
    assert!(!queue.use_fast_queue_path);
    assert_eq!(queue.next_contiguous_seq, 0x8000);
    assert!(queue.queue_packet(packet(0x8000), false, false).is_ok());
    assert_eq!(queue.next_contiguous_seq, 0x8000);
    assert!(queue.queue_packet(packet(0x8000), false, false).is_err());
    assert!(queue.queue_packet(packet(0x8002), false, false).is_err());
    assert_eq!(queue.pending.len(), 2);
}

/// The presentation time is the timestamp's, whether computed for the block's first packet
/// or reused for the rest; a packet with another timestamp gets its own.
#[test]
fn a_blocks_packets_share_one_presentation_time() {
    let mut queue = queue_at(10);
    for (seq, timestamp) in [(10u16, 9000u32), (11, 9000), (12, 18000)] {
        let mut p = packet(seq);
        p.timestamp = timestamp;
        assert!(queue.queue_packet(p, false, false).is_ok());
    }
    let times: Vec<u64> = queue
        .pending
        .iter()
        .map(|p| p.presentation_time_us)
        .collect();
    assert_eq!(times, [100_000, 100_000, 200_000]);
}

/// The flag that lets a completed block skip its sort holds only while every packet came
/// in order.
#[test]
fn the_in_order_flag_drops_at_the_first_packet_out_of_order() {
    let mut queue = queue_at(10);
    for seq in [10u16, 11, 12] {
        assert!(queue.queue_packet(packet(seq), false, false).is_ok());
    }
    assert!(queue.use_fast_queue_path);
    assert!(queue.queue_packet(packet(14), false, false).is_ok());
    assert!(!queue.use_fast_queue_path);
}

fn av1_block_datagrams(
    frame: u32,
    frame_type: u8,
    block: u8,
    block_count: u8,
    seq_start: u16,
    data_count: usize,
    fec_percent: usize,
) -> Vec<Vec<u8>> {
    let packet_size = 1024usize;
    let slice = packet_size - 16;
    let mut data = vec![1, 0, 0, frame_type];
    data.extend_from_slice(&(slice as u16).to_le_bytes());
    data.extend_from_slice(&[0, 0]);
    data.resize(data_count * slice, 0x41);
    (0..data_count)
        .map(|i| {
            let mut s = vec![0u8; packet_size + 16];
            s[0] = 0x90;
            s[2..4].copy_from_slice(&seq_start.wrapping_add(i as u16).to_be_bytes());
            s[4..8].copy_from_slice(&(frame * 1500).to_be_bytes());
            s[16..20].copy_from_slice(
                &((frame.wrapping_mul(16).wrapping_add(i as u32)) << 8).to_le_bytes(),
            );
            s[20..24].copy_from_slice(&frame.to_le_bytes());
            s[24] = FLAG_CONTAINS_PIC_DATA
                | if i == 0 { FLAG_SOF } else { 0 }
                | if i + 1 == data_count { FLAG_EOF } else { 0 };
            s[26] = 0x10;
            s[27] = (block << 4) | ((block_count - 1) << 6);
            let info = ((i as u32) << 12)
                | ((data_count as u32) << 22)
                | ((fec_percent as u32) << 4);
            s[28..32].copy_from_slice(&info.to_le_bytes());
            s[32..32 + slice].copy_from_slice(&data[i * slice..(i + 1) * slice]);
            s
        })
        .collect()
}

/// A >32,768 RTP sequence gap or >32,768 frame jump after roaming/dropouts must not be
/// filtered by the stale 16-bit sequence window, while duplicates in an active block still are.
#[test]
fn a_seq_gap_past_32768_on_a_new_frame_is_not_rejected() {
    let mut rx = super::super::VideoReceiver::new(config(0x1000));
    let mut events = Vec::new();
    for pkt in av1_block_datagrams(1, 2, 0, 1, 100, 2, 0) {
        rx.receive(&pkt, 1000, &mut events);
    }
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, super::super::VideoEvent::Frame(_)))
            .count(),
        1
    );

    // Next frame (an IDR keyframe after a roam/outage) arrives after a >32,768 packet gap
    // (seq 40_100 is "before" 102 in 16-bit modular arithmetic) and a >32,768 frame jump.
    events.clear();
    let jump_seq = 100u16.wrapping_add(40_000);
    let jump_frame = 40_000u32;
    for pkt in av1_block_datagrams(jump_frame, 2, 0, 1, jump_seq, 2, 0) {
        assert!(!rx.queue.rejects_datagram(&pkt));
        rx.receive(&pkt, 2000, &mut events);
    }
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, super::super::VideoEvent::Frame(_)))
            .count(),
        1
    );

    // An already-contiguous packet in the active in-progress block is still rejected.
    let active = av1_block_datagrams(jump_frame + 1, 1, 0, 1, jump_seq + 10, 3, 0);
    rx.receive(&active[0], 3000, &mut events);
    assert!(rx.queue.rejects_datagram(&active[0]));
}

/// Losing block 0 of frame N+1 after speculative RFI (or completion) on frame N must not emit
/// a fabricated 0%-loss FecStatus and must still notify frame lost for N+1.
#[test]
fn multi_fec_block_0_loss_emits_no_stale_fec_status_and_notifies_frame_lost() {
    use super::super::{FrameType, VideoEvent, VideoReceiver};

    // Case 1: clean completion on frame 1 (pending empty), then block 0 of frame 2 is lost.
    let mut rx = VideoReceiver::new(VideoConfig {
        rfi: true,
        ..config(0x1000)
    });
    let mut events = Vec::new();
    for pkt in av1_block_datagrams(1, 2, 0, 1, 100, 2, 20) {
        rx.receive(&pkt, 1000, &mut events);
    }
    rx.frame_decoded(FrameType::Idr);
    events.clear();

    let frame2_block1 = av1_block_datagrams(2, 1, 1, 2, 110, 2, 20);
    rx.receive(&frame2_block1[0], 2000, &mut events);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, VideoEvent::Signal(VideoSignal::FecStatus(_)))),
        "must not emit FecStatus from stale empty pending: {events:?}"
    );
    assert!(
        events.iter().any(|e| matches!(
            e,
            VideoEvent::Signal(VideoSignal::InvalidateFrames { first: 2, last: 2 })
        )),
        "must notify frame lost for frame 2: {events:?}"
    );

    // Case 2: fresh receiver with a decoded IDR on frame 1, then speculative RFI fires on
    // frame 2 (4 data + 1 parity, 2 data lost), then block 0 of frame 3 is lost and block 1
    // arrives.
    let mut rx = VideoReceiver::new(VideoConfig {
        rfi: true,
        ..config(0x1000)
    });
    events.clear();
    for pkt in av1_block_datagrams(1, 2, 0, 1, 100, 2, 20) {
        rx.receive(&pkt, 1000, &mut events);
    }
    rx.frame_decoded(FrameType::Idr);
    events.clear();
    let frame2 = av1_block_datagrams(2, 1, 0, 1, 110, 4, 20);
    rx.receive(&frame2[0], 3000, &mut events);
    rx.receive(&frame2[3], 3001, &mut events);
    assert!(events.iter().any(|e| matches!(
        e,
        VideoEvent::Signal(VideoSignal::InvalidateFrames { first: 2, last: 2 })
    )));
    events.clear();

    let frame3_block1 = av1_block_datagrams(3, 1, 1, 2, 120, 2, 20);
    rx.receive(&frame3_block1[0], 4000, &mut events);
    let fec_statuses: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            VideoEvent::Signal(VideoSignal::FecStatus(s)) => Some(*s),
            _ => None,
        })
        .collect();
    assert_eq!(
        fec_statuses.len(),
        1,
        "only frame 2's abandoned block: {fec_statuses:?}"
    );
    assert_eq!(fec_statuses[0].frame_index, 2);
    assert!(
        events.iter().any(|e| matches!(
            e,
            VideoEvent::Signal(VideoSignal::InvalidateFrames { first: 2, last: 3 })
                | VideoEvent::Signal(VideoSignal::InvalidateFrames { first: 3, last: 3 })
        )),
        "speculative RFI on frame 2 must not suppress frame 3's loss: {events:?}"
    );
}
