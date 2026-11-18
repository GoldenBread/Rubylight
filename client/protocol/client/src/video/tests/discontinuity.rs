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
    assert_eq!(rx.stats().frame_drops, 0);
}

/// A datagram whose sequence number is 5,000 packets ahead, with a plausible frame number, would
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
    let seq = u16::from_be_bytes([wrong[2], wrong[3]]).wrapping_add(5_000);
    wrong[2..4].copy_from_slice(&seq.to_be_bytes());
    rx.receive(&wrong, 2, &mut events);
    assert!(requests_idr(&events), "a discontinuity asks for a key frame");
    assert_eq!(rx.stats().frame_drops, 0, "a discontinuity is not counted as loss");

    for packet in &next[1..] {
        rx.receive(packet, 3, &mut events);
    }
    // The next frame resyncs the queue on the real sequence numbers; it is not shown because it
    // arrived before the key frame asked for.
    for packet in host.frame(&h264_p(3000), 1) {
        rx.receive(&packet, 4, &mut events);
    }
    for packet in host.frame(&h264_idr(), 2) {
        rx.receive(&packet, 5, &mut events);
    }
    let shown = frames(&events);
    assert_eq!(shown.len(), 2, "the stream continues after the wrong sequence number");
    assert_eq!(shown[1].frame_type, FrameType::Idr);
    assert_eq!(rx.stats().frame_drops, 0);
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
