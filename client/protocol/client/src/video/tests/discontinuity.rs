//! Frame and sequence counter jumps: a lost frame stays loss, as before, while a counter no
//! streaming host produces is a discontinuity that resyncs the queue instead of moving its
//! position tens of thousands of frames (or packets) away, which dropped every real frame that
//! followed until the counter caught up.

use super::*;

/// `packets` with every datagram's frame field rewritten to `frame`.
fn with_frame(packets: Vec<Vec<u8>>, frame: u32) -> Vec<Vec<u8>> {
    packets
        .into_iter()
        .map(|mut packet| {
            packet[20..24].copy_from_slice(&frame.to_le_bytes());
            packet
        })
        .collect()
}

/// `packets` with every datagram's frame rewritten to `frame` and its sequence number to `seq`,
/// advanced by one per datagram: a host whose counters restarted from a clean position.
fn rewound(packets: Vec<Vec<u8>>, frame: u32, seq: &mut u16) -> Vec<Vec<u8>> {
    packets
        .into_iter()
        .map(|mut packet| {
            packet[20..24].copy_from_slice(&frame.to_le_bytes());
            packet[2..4].copy_from_slice(&seq.to_be_bytes());
            *seq = seq.wrapping_add(1);
            packet
        })
        .collect()
}

fn requests_idr(events: &[VideoEvent]) -> bool {
    events
        .iter()
        .any(|e| matches!(e, VideoEvent::Signal(VideoSignal::RequestIdr)))
}

/// A frame lost between two others is loss, counted as it was, and the next key frame is shown.
/// The bound must not turn this into a discontinuity.
#[test]
fn a_one_frame_gap_is_loss_and_waits_for_a_key_frame() {
    let mut host = Host::new();
    let mut rx = VideoReceiver::new(config(0x1));
    let mut events = Vec::new();
    for packet in host.frame(&h264_idr(), 2) {
        rx.receive(&packet, 1, &mut events);
    }
    rx.frame_decoded(FrameType::Idr);
    let _lost = host.frame(&h264_p(100), 1);
    let next = host.frame(&h264_p(3000), 1);
    // The gap is noticed at the first packet of the frame after it.
    rx.receive(&next[0], 2, &mut events);
    assert_eq!(rx.stats().frame_drops, 1, "a lost frame is loss, not a discontinuity");
    assert!(!requests_idr(&events), "a gap inside the bound is no discontinuity");
    for packet in &next[1..] {
        rx.receive(packet, 2, &mut events);
    }
    assert_eq!(frames(&events).len(), 1, "the frame after the gap waits for a key frame");
    assert!(requests_idr(&events), "and a whole frame after it asks for one");
    for packet in host.frame(&h264_idr(), 2) {
        rx.receive(&packet, 3, &mut events);
    }
    assert_eq!(frames(&events).len(), 2, "the key frame it waited for is shown");
}

/// A datagram whose frame number is 10,000 frames ahead (a bit flip, a buggy host, a packet from
/// an old session) is not taken as the stream's position: the queue resyncs on the real numbering
/// and asks for a key frame, counting no loss.
#[test]
fn a_wrong_frame_number_resyncs_instead_of_freezing() {
    let mut host = Host::new();
    let mut rx = VideoReceiver::new(config(0x1));
    let mut events = Vec::new();
    for packet in host.frame(&h264_idr(), 2) {
        rx.receive(&packet, 1, &mut events);
    }
    rx.frame_decoded(FrameType::Idr);

    let next = host.frame(&h264_p(3000), 1);
    let mut wrong = next[0].clone();
    wrong[20..24].copy_from_slice(&10_002u32.to_le_bytes());
    rx.receive(&wrong, 2, &mut events);
    assert!(requests_idr(&events), "a discontinuity asks for a key frame");
    assert_eq!(rx.stats().frame_drops, 0, "a discontinuity is not counted as loss");

    // The rest of the genuine frame and the key frame that follows are seen: before, they were
    // "before" the wrong counter and dropped for minutes.
    for packet in &next[1..] {
        rx.receive(packet, 3, &mut events);
    }
    for packet in host.frame(&h264_idr(), 2) {
        rx.receive(&packet, 4, &mut events);
    }
    let shown = frames(&events);
    assert_eq!(shown.len(), 2, "the stream continues after the wrong frame number");
    assert_eq!(shown[1].frame_type, FrameType::Idr);
    assert!(
        rx.stats().frame_drops <= 1,
        "finding the real position again may lose the frame whose datagrams were dropped"
    );
}

/// A sequence number that outran what the frames can carry, with a plausible frame number, would
/// otherwise have the queue reject every real packet behind it until the sequence wrapped.
#[test]
fn a_wrong_sequence_number_resyncs_instead_of_freezing() {
    let mut host = Host::new();
    let mut rx = VideoReceiver::new(config(0x1));
    let mut events = Vec::new();
    for packet in host.frame(&h264_idr(), 2) {
        rx.receive(&packet, 1, &mut events);
    }
    rx.frame_decoded(FrameType::Idr);

    let next = host.frame(&h264_p(3000), 1);
    let mut wrong = next[0].clone();
    // 30,000 packets behind one frame is more than four blocks of 3,632 packets can carry.
    let seq = u16::from_be_bytes([wrong[2], wrong[3]]).wrapping_add(30_000);
    wrong[2..4].copy_from_slice(&seq.to_be_bytes());
    rx.receive(&wrong, 2, &mut events);
    assert!(requests_idr(&events), "a discontinuity asks for a key frame");
    assert_eq!(rx.stats().frame_drops, 0, "a discontinuity is not counted as loss");

    for packet in &next[1..] {
        rx.receive(packet, 3, &mut events);
    }
    // The next frame is a new one, so it resets the block window from its own sequence number
    // and is not rejected behind the wrong one: the frame lost with the jump is counted, and
    // the key frame asked for recovers the picture.
    for packet in host.frame(&h264_idr(), 2) {
        rx.receive(&packet, 4, &mut events);
    }
    let shown = frames(&events);
    assert_eq!(shown.len(), 2, "the stream continues after the wrong sequence number");
    assert_eq!(shown[1].frame_type, FrameType::Idr);
}

/// A 100 ms burst at 500 Mbps skips six frames and 6,250 packets: both counters advanced
/// together, so it is loss. It is counted, invalidates the skipped frames over RFI, and asks for
/// no key frame.
#[test]
fn a_burst_of_loss_counts_as_loss_and_uses_rfi() {
    let mut host = Host::new();
    let mut cfg = config(0x1);
    cfg.rfi = true;
    let mut rx = VideoReceiver::new(cfg);
    let mut events = Vec::new();
    for packet in host.frame(&h264_idr(), 2) {
        rx.receive(&packet, 1, &mut events);
    }
    rx.frame_decoded(FrameType::Idr);
    events.clear();
    // Frame 2 arrives missing a data packet and its parity, so its block waits for the next one.
    let mut partial = host.frame(&h264_p(3000), 1);
    let _missing = partial.remove(1);
    let _parity = partial.pop();
    for packet in &partial {
        rx.receive(packet, 2, &mut events);
    }
    assert!(fec_blocks(&events).is_empty(), "the block still waits");

    // The first frame after the burst is still arriving: its block waits, so no picture is
    // dropped again behind the gap.
    let mut burst = host.frame(&h264_p(2000), 1);
    let frame = 3u32 + 6;
    let seq = u16::from_be_bytes([burst[0][2], burst[0][3]]).wrapping_add(6_250);
    burst[0][20..24].copy_from_slice(&frame.to_le_bytes());
    burst[0][2..4].copy_from_slice(&seq.to_be_bytes());
    rx.receive(&burst[0], 3, &mut events);

    assert_eq!(
        rx.stats().frame_drops,
        1,
        "the burst is one lost frame, not a discontinuity"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, VideoEvent::Signal(VideoSignal::InvalidateFrames { .. }))),
        "RFI invalidates the frames the burst skipped"
    );
    assert!(!requests_idr(&events), "no key frame is asked for: the invalidation is enough");
    let blocks = fec_blocks(&events);
    assert!(
        blocks.iter().any(|block| block.frame == 2 && block.unrecovered),
        "the block the burst cut off is reported: {blocks:?}"
    );
}

/// A packet 75 ms late is a straggler: dropped, without moving the queue or asking for a key
/// frame, and the stream carries on.
#[test]
fn a_straggler_is_dropped_without_a_resync() {
    let mut host = Host::new();
    let mut rx = VideoReceiver::new(config(0x1));
    let mut events = Vec::new();
    for packet in host.frame(&h264_idr(), 2) {
        rx.receive(&packet, 1, &mut events);
    }
    rx.frame_decoded(FrameType::Idr);
    for i in 0..5 {
        for packet in host.frame(&h264_p(500), 1) {
            rx.receive(&packet, 2 + i, &mut events);
        }
    }
    let shown = frames(&events).len();
    let drops = rx.stats().frame_drops;
    let current = rx.current_frame_number();

    // Four frames and 4,700 packets back: 75 ms at 60 fps and 500 Mbps.
    let next = host.frame(&h264_p(500), 1);
    let mut straggler = next[0].clone();
    straggler[20..24].copy_from_slice(&(current - 4).to_le_bytes());
    let seq = u16::from_be_bytes([straggler[2], straggler[3]]).wrapping_sub(4_700);
    straggler[2..4].copy_from_slice(&seq.to_be_bytes());
    rx.receive(&straggler, 9, &mut events);

    assert_eq!(rx.current_frame_number(), current, "the queue stays where it was");
    assert_eq!(rx.stats().frame_drops, drops, "a straggler is no loss");
    assert!(!requests_idr(&events), "and no discontinuity");
    for packet in &next {
        rx.receive(packet, 10, &mut events);
    }
    assert_eq!(frames(&events).len(), shown + 1, "the stream carries on");
}

/// A host whose counters restart (frame 1, sequence 0) is followed once its datagrams keep
/// coming from there, and then the stream runs on.
#[test]
fn a_host_counter_reset_resyncs_once() {
    // Smaller packets: the restarted key frame's block carries enough parity for the datagrams
    // the queue drops while it confirms the rewind.
    let mut host = Host::with_packet_size(256);
    let mut cfg = config(0x1);
    cfg.packet_size = 256;
    let mut rx = VideoReceiver::new(cfg);
    let mut events = Vec::new();
    for packet in host.frame(&h264_idr(), 2) {
        rx.receive(&packet, 1, &mut events);
    }
    rx.frame_decoded(FrameType::Idr);
    // The stream had run on before its counters restarted.
    rx.queue.seek(1_000);
    let mut seq = 0u16;
    for packet in rewound(host.frame(&h264_idr(), 2), 1, &mut seq) {
        rx.receive(&packet, 2, &mut events);
    }
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, VideoEvent::Signal(VideoSignal::RequestIdr)))
            .count(),
        1,
        "the restarted stream is followed once, with one key frame asked for"
    );
    // The frame after the restart's first is shown, and the stream goes on from the new position.
    for packet in rewound(host.frame(&h264_idr(), 2), 2, &mut seq) {
        rx.receive(&packet, 3, &mut events);
    }
    let shown = frames(&events);
    assert_eq!(shown.last().map(|frame| frame.number), Some(2));
    assert_eq!(rx.current_frame_number(), 3);
}

/// An encrypted stream's packets carry the frame number in the clear, and the receive path drops
/// frames behind the queue's position before decrypting them. A jumped position must not make
/// that drop eat the real frames, or the queue could never resync back.
#[test]
fn an_encrypted_wrong_frame_number_resyncs_instead_of_freezing() {
    use aes_gcm::aead::generic_array::GenericArray;
    use aes_gcm::aead::{AeadInPlace, KeyInit};
    let key = [7u8; 16];
    let cipher = aes_gcm::Aes128Gcm::new(GenericArray::from_slice(&key));
    let mut nonce = 0u64;
    let encrypt = |packet: &[u8], frame: u32, nonce: &mut u64| {
        let mut body = packet.to_vec();
        let mut iv = [0u8; 12];
        iv[..8].copy_from_slice(&nonce.to_le_bytes());
        *nonce += 1;
        let tag = cipher
            .encrypt_in_place_detached(GenericArray::from_slice(&iv), &[], &mut body)
            .unwrap();
        let mut datagram = iv.to_vec();
        datagram.extend_from_slice(&frame.to_le_bytes());
        datagram.extend_from_slice(&tag);
        datagram.extend_from_slice(&body);
        datagram
    };
    let mut cfg = config(0x1);
    cfg.encryption_key = Some(key);
    let mut rx = VideoReceiver::new(cfg);
    let mut host = Host::new();
    let mut events = Vec::new();
    for packet in host.frame(&h264_idr(), 2) {
        let encrypted = encrypt(&packet, 1, &mut nonce);
        rx.receive(&encrypted, 1, &mut events);
    }
    rx.frame_decoded(FrameType::Idr);

    // The corrupt frame field is inside the packet, whose clear frame number stays genuine: the
    // early drop cannot know which of the two the queue will reject.
    let next = host.frame(&h264_p(3000), 1);
    for (i, packet) in next.iter().enumerate() {
        let mut packet = packet.clone();
        if i == 0 {
            packet[20..24].copy_from_slice(&10_002u32.to_le_bytes());
        }
        let encrypted = encrypt(&packet, 2, &mut nonce);
        rx.receive(&encrypted, 2, &mut events);
    }
    assert!(requests_idr(&events), "a discontinuity asks for a key frame");
    assert_eq!(rx.stats().frame_drops, 0, "a discontinuity is not counted as loss");
    for packet in host.frame(&h264_idr(), 2) {
        let encrypted = encrypt(&packet, 3, &mut nonce);
        rx.receive(&encrypted, 3, &mut events);
    }
    let shown = frames(&events);
    assert_eq!(shown.len(), 2, "the stream continues after the wrong frame number");
    assert_eq!(shown[1].frame_type, FrameType::Idr);
}

/// The 16-bit wrap hosts count frames in is a normal step, not a jump: the queue follows it.
#[test]
fn a_frame_counter_wrap_is_not_a_discontinuity() {
    let mut host = Host::new();
    let mut rx = VideoReceiver::new(config(0x1));
    let mut events = Vec::new();
    // As if the frames before it were received: the queue expects frame 0xFFFF next.
    rx.queue.seek(0xFFFF);
    let across = [0xFFFFu32, 0x1_0000, 0x1_0001];
    for (i, frame) in across.into_iter().enumerate() {
        let payload = if i == 0 { h264_idr() } else { h264_p(500) };
        let kind = if i == 0 { 2 } else { 1 };
        for packet in with_frame(host.frame(&payload, kind), frame) {
            rx.receive(&packet, 1 + i as u64, &mut events);
        }
    }
    let shown = frames(&events);
    assert_eq!(shown.len(), 3, "every frame across the wrap is shown");
    assert_eq!(
        shown.iter().map(|frame| frame.number).collect::<Vec<_>>(),
        across,
        "the low 16 bits go from 0xFFFF to 0"
    );
    assert!(!requests_idr(&events), "the wrap is no discontinuity");
}
