//! Golden wire fixtures for streamed P-frame client reassembly tests.

use super::*;

/// A frame of a fixture: its kind, payload and packets.
type FixtureFrame = (u8, Vec<u8>, Vec<Vec<u8>>);

/// A fixture of packets the host built, for a receiver in another repository
/// to reassemble: `rust/tests/fixtures/slice-delivery-<name>.bin`.
///
/// Little endian: the magic `RLSLICE1`; the packet size (u32); the GCM key (16
/// bytes, all zero when the packets are plain); the frame count (u32); then per
/// frame its kind (1 P, 2 IDR), its payload (u32 length, bytes) and its
/// packets (u32 count, then u16 length and bytes each).
fn fixture(key: Option<[u8; 16]>, frames: &[FixtureFrame]) -> Vec<u8> {
    let mut out = b"RLSLICE1".to_vec();
    out.extend_from_slice(&1392u32.to_le_bytes());
    out.extend_from_slice(&key.unwrap_or([0; 16]));
    out.extend_from_slice(&(frames.len() as u32).to_le_bytes());
    for (kind, payload, packets) in frames {
        out.push(*kind);
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(payload);
        out.extend_from_slice(&(packets.len() as u32).to_le_bytes());
        for packet in packets {
            out.extend_from_slice(&(packet.len() as u16).to_le_bytes());
            out.extend_from_slice(packet);
        }
    }
    out
}

/// An H.264 NAL (start code, header, then bytes with no zero in them) so the
/// client's depacketizer takes it for a frame.
fn h264_frame(size: usize, idr: bool) -> Vec<u8> {
    let mut frame = if idr {
        vec![
            0, 0, 0, 1, 0x67, 0x42, 0x00, 0x1f, 0, 0, 0, 1, 0x68, 0xce, 0x3c, 0x80, 0, 0, 0, 1,
            0x65,
        ]
    } else {
        vec![0, 0, 0, 1, 0x41]
    };
    let head = frame.len();
    frame.extend((0..size - head).map(|i| ((i * 31 + i / 7) % 249) as u8 | 1));
    frame
}

/// What a receiver reads off the wire of one streamed frame: the frame header,
/// and `streamPacketIndex` (packet bytes 16..20) on every data shard.
fn assert_wire(
    packets: &[Vec<u8>],
    key: Option<[u8; 16]>,
    payload: &[u8],
    first_sequence: u32,
    early: bool,
) {
    let plain: Vec<_> = packets.iter().map(|p| open(p, key)).collect();
    let (stream, layout) = reassemble(packets, key);
    assert_identical(&stream, payload);
    let stream_len = 8 + payload.len();
    // The last payload length is not known when block 0 leaves, so a frame
    // that sent a block early says "full" (only AV1 reads it, and it is not
    // streamed); one that did not is framed as a whole frame.
    let last = match stream_len % SLICE {
        _ if early => SLICE,
        0 => SLICE,
        n => n,
    };
    assert_eq!(&stream[4..6], &(last as u16).to_le_bytes());
    assert_eq!(&stream[1..3], &5u16.to_le_bytes(), "latency in 100 us");
    let mut at = 0;
    let mut sequence = first_sequence;
    for (data, parity) in layout {
        for (index, packet) in plain[at..at + data].iter().enumerate() {
            let stream_index = u32::from_le_bytes(packet[16..20].try_into().unwrap());
            assert_eq!(stream_index, (sequence + index as u32) << 8);
            assert_eq!(
                u16::from_be_bytes([packet[2], packet[3]]),
                (sequence + index as u32) as u16
            );
        }
        at += data + parity;
        sequence += (data + parity) as u32;
    }
}

#[test]
fn streamed_frames_match_the_wire_fixtures_the_client_reassembles() {
    for (name, key) in [("plain", None), ("gcm", Some([7; 16]))] {
        let mut p = packetizer(20, 2, key);
        p.sequence = 4000;
        p.frame = 1;
        let mut frames = Vec::new();
        let idr = h264_frame(12 * SLICE + 77, true);
        let packets = p.encode(&idr, true, 1000, 500).unwrap();
        frames.push((2, idr, packets));
        let mut next_sequence = p.sequence;
        for (units, size, blocks) in [(4, 88 * SLICE + 913, 4), (3, 31 * SLICE, 2)] {
            let payload = h264_frame(size, false);
            let mut random = {
                let mut state = 17 + size as u64;
                move |limit: usize| {
                    state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                    (state >> 33) as usize % limit
                }
            };
            let parts = cuts(size, units, &mut random);
            let first_sequence = next_sequence;
            let (sent, result) = run(&mut p, &payload, &parts, blocks, units, 120 * SLICE);
            result.unwrap();
            assert!(sent.done && sent.early > 0);
            assert_wire(&sent.packets, key, &payload, first_sequence, sent.early > 0);
            next_sequence = p.sequence;
            frames.push((1, payload, sent.packets));
        }
        let bytes = fixture(key, &frames);
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../tests/fixtures/slice-delivery-{name}.bin"));
        if std::env::var_os("UPDATE_SLICE_FIXTURES").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &bytes).unwrap();
        }
        let stored = std::fs::read(&path).unwrap_or_default();
        assert!(
            stored == bytes,
            "{} is out of date: run this test with UPDATE_SLICE_FIXTURES=1 and commit it \
             (and the copy the Nightjar client tests read)",
            path.display()
        );
    }
}
