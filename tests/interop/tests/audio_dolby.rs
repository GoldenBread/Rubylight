//! Dolby AC-3 / E-AC-3 chunking and RTP packetization interop between host and Nightjar's receiver.

/// The host's chunks and RTP packets (every chunk of a frame on its timestamp) through
/// Nightjar's receiver: whole frames in order, a lost chunk recovered by FEC, and the frames
/// of a block past recovery reported lost, once each.
#[test]
fn host_dolby_packets_come_out_of_nightjar_as_whole_frames() {
    use butterpollo_media_policy::{audio_passthrough::chunks, packet::AudioPacketizer};
    use nightjar_core::audio::{AudioConfig, AudioOutput, AudioReceiver};
    // 640 kbit/s AC3 5.1 frames, 2560 bytes and three chunks each.
    let frames: Vec<Vec<u8>> = (0..40u8)
        .map(|index| {
            let mut frame = vec![index; 2560];
            frame[..6].copy_from_slice(&[0x0B, 0x77, 0, 0, 36, 8 << 3]);
            frame
        })
        .collect();
    let mut packetizer = AudioPacketizer::new([0; 16], 0, false, 32);
    // The datagrams of each frame, sent at once every 32 ms.
    let sent: Vec<Vec<Vec<u8>>> = frames
        .iter()
        .enumerate()
        .map(|(index, frame)| {
            let parts = chunks(frame, index as u16);
            let last = parts.len() - 1;
            parts
                .iter()
                .enumerate()
                .flat_map(|(part, piece)| packetizer.encode_part(piece, part == last).unwrap())
                .collect()
        })
        .collect();
    let seq = |p: &[u8]| (p[1], u16::from_be_bytes([p[2], p[3]]));
    let mut receiver = AudioReceiver::new(AudioConfig {
        packet_duration_ms: 32,
        app_version: [7, 1, 431, -1],
        encryption: None,
    });
    receiver.assemble_frames();
    let mut played = Vec::new();
    for (index, datagrams) in sent.iter().enumerate() {
        for datagram in datagrams {
            // Seq 61 (frame 20) is recovered from its block's parity. Seq 64 to 66 (frame
            // 21 and 22) and that block's parity are lost: frames 21 and 22 are lost.
            match seq(datagram) {
                (97, 61 | 64 | 65 | 66) | (127, 68 | 69) => continue,
                _ => {}
            }
            receiver.receive(datagram, index as u64 * 32_000, &mut |out| {
                played.push(match out {
                    AudioOutput::Packet(frame) => Some(frame.to_vec()),
                    AudioOutput::Lost => None,
                })
            });
        }
        if index == 0 {
            // The first poll timeout ends the backlog drop.
            receiver.receive_timeout();
        }
    }
    let stats = receiver.stats();
    assert_eq!(stats.packet_count_fec_recovered, 1);
    assert_eq!(stats.packet_count_fec_failed, 1);
    assert_eq!(stats.packet_count_invalid, 0);
    let first = frames.len() - played.len();
    let mut expected: Vec<_> = frames[first..].iter().cloned().map(Some).collect();
    expected.splice(21 - first..23 - first, [None, None]);
    // Frame 0 is the dropped backlog, and frame 1's first chunk synchronizes the queue.
    assert_eq!(first, 2);
    assert!(
        played == expected,
        "frames out of order, missing or changed"
    );
}
