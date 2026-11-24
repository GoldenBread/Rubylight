//! Wire headers, encryption, replay window, audio packetization, and jumbo streamability tests.

use super::*;

#[test]
fn replay_window_accepts_reordering_but_rejects_duplicates_and_wraps() {
    let mut window = ReplayWindow::default();
    for n in [100, 102, 101, 103, 99] {
        assert!(window.accept(n));
    }
    for n in [100, 99, 102] {
        assert!(!window.accept(n));
    }
    // A keyframe request resent behind hundreds of motion reports.
    for n in 104..=900 {
        assert!(window.accept(n));
    }
    assert!(window.accept(1));
    assert!(!window.accept(1));
    // Older than the window, or reusing a slot a newer sequence cleared.
    assert!(window.accept(10_000));
    assert!(!window.accept(10_000 - 4096));
    assert!(window.accept(10_000 - 4095));
    assert!(!window.accept(10_000 - 4095));
    assert!(window.accept(u32::MAX));
    assert!(!window.accept(0));
}
#[test]
fn forged_legacy_input_cannot_change_the_next_iv() {
    let key = [3; 16];
    let mut input = LegacyInput::new(0x12345678);
    let original = input.iv;
    let raw = vec![7; 40];
    let (tag, encrypted) = crypto::gcm_seal(&key, &original, &raw).unwrap();
    let mut packet = ((tag.len() + encrypted.len()) as u32)
        .to_be_bytes()
        .to_vec();
    packet.extend_from_slice(&tag);
    packet.extend_from_slice(&encrypted);
    let mut bad = packet.clone();
    bad[5] ^= 1;
    assert!(input.open(&key, &bad).is_err());
    assert_eq!(input.iv, original);
    assert_eq!(input.open(&key, &packet).unwrap(), raw);
    assert_eq!(input.iv, packet[packet.len() - 16..]);
}

#[test]
fn packet_insert_crosses_sources_at_every_boundary() {
    for h in 0..10 {
        for s in 0..16 {
            for a in 0..25 {
                for b in 0..25 {
                    let aa = vec![1; a];
                    let bb = vec![2; b];
                    let joined = [aa.clone(), bb.clone()].concat();
                    let expected: Vec<u8> = if s == 0 {
                        vec![]
                    } else {
                        joined
                            .chunks(s)
                            .flat_map(|p| [vec![0; h], p.to_vec()].concat())
                            .collect()
                    };
                    assert_eq!(concat_and_insert(h, s, &aa, &bb).unwrap(), expected);
                }
            }
        }
    }
}
#[test]
fn video_headers_follow_moonlight_layout() {
    let mut p = VideoPacketizer {
        sequence: 65535,
        iv_counter: 0,
        frame: 1,
        packet_size: 1024,
        fec_percent: 0,
        min_fec: 0,
        key: None,
    };
    let packets = p.encode(&vec![3; 1100], true, 900, 200).unwrap();
    assert_eq!(packets.len(), 2);
    assert_eq!(&packets[0][..4], &[0x90, 0, 0xff, 0xff]);
    assert_eq!(packets[0][24], 5);
    assert_eq!(packets[1][24], 3);
    assert_eq!(&packets[0][32..40], &[1, 2, 0, 2, 100, 0, 0, 0]);
    assert_eq!(&packets[1][2..4], &[0, 0]);
    let stream_index = |packet: &[u8]| u32::from_le_bytes(packet[16..20].try_into().unwrap()) >> 8;
    assert_eq!(stream_index(&packets[0]), 65535);
    assert_eq!(stream_index(&packets[1]), 65536);
    let recovered = p.encode_recovery(&[3; 16], false, true, 1000, 300).unwrap();
    assert_eq!(recovered[0][35], 5); // Moonlight recovery marker
    assert_eq!(stream_index(&recovered[0]), 65537);
    let keyframe = p.encode_recovery(&[3; 16], true, true, 1100, 300).unwrap();
    assert_eq!(keyframe[0][35], 2); // IDR takes precedence
    p.sequence = 0xFFFFFF;
    let wrapped = p.encode(&vec![3; 1100], true, 1200, 300).unwrap();
    assert_eq!(stream_index(&wrapped[0]), 0xFFFFFF);
    assert_eq!(stream_index(&wrapped[1]), 0);
    assert_eq!(&wrapped[0][2..4], &[0xff, 0xff]);
    assert_eq!(&wrapped[1][2..4], &[0, 0]);
}
#[test]
fn encrypted_video_nonce_is_unique_and_authenticated() {
    let k = [5; 16];
    let mut p = VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
        packet_size: 1024,
        fec_percent: 20,
        min_fec: 1,
        key: Some(k),
    };
    let b = p.encode(&vec![1; 1100], false, 123, 0).unwrap();
    assert_ne!(&b[0][..12], &b[1][..12]);
    let plain = crypto::gcm_open(&k, &b[0][..12], &b[0][16..32], &b[0][32..]).unwrap();
    assert_eq!(plain[0], 0x90);
    assert_eq!(plain[24], 5);
    p.iv_counter = u64::MAX;
    assert!(p.encode(&[1], true, 124, 0).is_err());
    assert_eq!(p.iv_counter, u64::MAX);
}
#[test]
fn encrypted_video_matches_independent_per_packet_sealing_across_fec_and_wraps() {
    let key = std::array::from_fn(|i| i as u8);
    for packet_size in [256, 1392] {
        for fec_percent in [0, 20] {
            let make = |key| VideoPacketizer {
                sequence: 0xFFFFFF,
                iv_counter: 0xFFFFFFFE,
                frame: u32::MAX,
                packet_size,
                fec_percent,
                min_fec: 1,
                key,
            };
            let mut plain = make(None);
            let mut encrypted = make(Some(key));
            let slice = packet_size - 16;
            for size in [1, slice - 7, 192 * slice - 8, 576 * slice + 13] {
                let payload: Vec<_> = (0..size).map(|i| (i * 29) as u8).collect();
                let frame = plain.frame;
                let nonce = encrypted.iv_counter;
                let expected = plain.encode(&payload, true, 9000, 500).unwrap();
                let actual = encrypted.encode(&payload, true, 9000, 500).unwrap();
                assert_eq!(actual.len(), expected.len());
                for (i, (packet, shard)) in actual.iter().zip(expected).enumerate() {
                    let mut iv = [0; 12];
                    iv[..8].copy_from_slice(&(nonce + i as u64).to_le_bytes());
                    iv[11] = b'V';
                    let (tag, bytes) = crypto::gcm_seal(&key, &iv, &shard).unwrap();
                    assert_eq!(&packet[..12], &iv);
                    assert_eq!(&packet[12..16], &frame.to_le_bytes());
                    assert_eq!(&packet[16..32], &tag);
                    assert_eq!(&packet[32..], bytes);
                }
                assert_eq!(plain.frame, encrypted.frame);
                assert_eq!(plain.sequence, encrypted.sequence);
                assert_eq!(encrypted.iv_counter, nonce + actual.len() as u64);
            }
        }
    }
}
fn frame_index(packet: &[u8]) -> u32 {
    u32::from_le_bytes(packet[20..24].try_into().unwrap())
}
#[test]
fn a_frame_beyond_the_packet_limit_still_takes_its_frame_index() {
    // Moonlight sees a lost frame only from a gap in the frame index.
    let mut p = VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
        packet_size: 1392,
        fec_percent: 20,
        min_fec: 2,
        key: None,
    };
    let first = p.encode(&[1; 5000], true, 0, 0).unwrap();
    assert_eq!(frame_index(&first[0]), 1);
    assert!(p.encode(&vec![2; 1376 * 4092], false, 1500, 0).is_err());
    let third = p.encode(&[3; 5000], false, 3000, 0).unwrap();
    assert_eq!(frame_index(&third[0]), 3);
}
#[test]
fn a_large_minimum_parity_fits_the_fec_percentage_field() {
    for min_fec in [2, 3, 10, 255] {
        let mut p = VideoPacketizer {
            sequence: 0,
            iv_counter: 0,
            frame: 1,
            packet_size: 1392,
            fec_percent: 20,
            min_fec,
            key: None,
        };
        let packets = p.encode(&[1; 10], false, 0, 0).unwrap();
        for (index, packet) in packets.iter().enumerate() {
            let info = u32::from_le_bytes(packet[28..32].try_into().unwrap());
            let (shard, data, percentage) = ((info >> 12) & 0x3ff, info >> 22, (info >> 4) & 0xff);
            assert_eq!(shard as usize, index, "min_fec {min_fec}");
            // moonlight-common-c derives the parity count from the percentage.
            assert_eq!(
                ((data * percentage).div_ceil(100) + data) as usize,
                packets.len()
            );
        }
    }
}
#[test]
fn block_layout_matches_the_packets_each_frame_is_sent_as() {
    for (packet_size, fec_percent, min_fec) in
        [(1392, 20, 2), (1024, 0, 0), (1392, 50, 6), (512, 10, 1)]
    {
        let mut p = VideoPacketizer {
            sequence: 0,
            iv_counter: 0,
            frame: 1,
            packet_size,
            fec_percent,
            min_fec,
            key: None,
        };
        for size in [
            1, 100, 1376, 1377, 40_000, 290_000, 1_180_000, 1_400_000, 3_000_000,
        ] {
            let Ok(packets) = p.encode(&vec![7; size], false, 0, 0) else {
                assert!(size > (packet_size - 16) * 4092 - 8);
                continue;
            };
            let mut sent: Vec<(usize, usize)> = Vec::new();
            for packet in &packets {
                let block = usize::from((packet[27] >> 4) & 3);
                let data = (u32::from_le_bytes(packet[28..32].try_into().unwrap()) >> 22) as usize;
                if sent.len() == block {
                    sent.push((data, 0));
                }
                sent[block].1 += 1;
            }
            let sent: Vec<_> = sent
                .into_iter()
                .map(|(data, packets)| (data, packets - data))
                .collect();
            assert_eq!(
                p.block_layout(size),
                sent,
                "size {size} packet {packet_size} fec {fec_percent}"
            );
            assert_eq!(
                sent.iter().map(|(d, f)| d + f).sum::<usize>(),
                packets.len()
            );
        }
    }
}

#[test]
fn audio_frame_pieces_share_a_timestamp_and_fec_names_the_block_start() {
    let timestamp = |packet: &[u8]| u32::from_be_bytes(packet[4..8].try_into().unwrap());
    let mut opus = AudioPacketizer::new([0; 16], 0, false, 5);
    let mut stamps = Vec::new();
    for _ in 0..4 {
        stamps.push(opus.encode(&[1; 40]).unwrap());
    }
    assert_eq!(
        stamps
            .iter()
            .map(|out| timestamp(&out[0]))
            .collect::<Vec<_>>(),
        [0, 5, 10, 15]
    );
    // The FEC packets carry the next timestamp and, at 16..20, the block's first.
    let fec = &stamps[3][1..];
    assert_eq!(fec.len(), 2);
    assert!(fec.iter().all(|p| p[1] == 127 && timestamp(p) == 20));
    assert!(fec.iter().all(|p| p[16..20] == 0u32.to_be_bytes()));
    // AC3: three pieces of frame 0 at 0, then frame 1 at 32 opening a block of its own pieces.
    let mut ac3 = AudioPacketizer::new([0; 16], 0, false, 32);
    let mut sent = Vec::new();
    for (index, last) in [false, false, true, false, true].into_iter().enumerate() {
        sent.extend(ac3.encode_part(&[index as u8; 1028], last).unwrap());
    }
    let data: Vec<_> = sent.iter().filter(|p| p[1] == 97).collect();
    assert_eq!(
        data.iter().map(|p| timestamp(p)).collect::<Vec<_>>(),
        [0, 0, 0, 32, 32]
    );
    let fec: Vec<_> = sent.iter().filter(|p| p[1] == 127).collect();
    assert_eq!(fec.len(), 2);
    assert!(fec.iter().all(|p| p[16..20] == 0u32.to_be_bytes()));
    assert!(fec.iter().all(|p| p.len() == 24 + 1028));
    assert_eq!(ac3.timestamp, 64);
}

#[test]
fn audio_packets_reused_across_calls_are_the_allocated_ones_byte_for_byte() {
    // The sender keeps one output and hands it back; the packetizer pools the
    // buffers. Over three FEC blocks of varying sizes, the wire bytes must be
    // the fresh buffers', packet by packet.
    for encrypted in [false, true] {
        let mut fresh = AudioPacketizer::new(*b"0123456789abcdef", 9, encrypted, 5);
        let mut pooled = AudioPacketizer::new(*b"0123456789abcdef", 9, encrypted, 5);
        let mut kept = Vec::new();
        let mut parity = 0;
        for size in 1..13u8 {
            let opus: Vec<u8> = (0..usize::from(size) * 23 % 251 + 1)
                .map(|i| (i.wrapping_mul(31) ^ usize::from(size)) as u8)
                .collect();
            let want = fresh.encode(&opus).unwrap();
            pooled.encode_into(&opus, &mut kept).unwrap();
            parity += kept.iter().filter(|p| p[1] == 127).count();
            assert_eq!(kept, want, "packet {size}, encrypted={encrypted}");
        }
        // Twelve packets made three FEC blocks, each with two parity packets.
        assert_eq!(parity, 6);
        assert_eq!(pooled.sequence, fresh.sequence);
        assert_eq!(pooled.timestamp, fresh.timestamp);
    }
}

#[test]
fn jumbo_packets_are_streamable() {
    let packetizer = VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
        packet_size: usize::from(rubylight_protocol::video_packet::MAX_VIDEO_PACKET),
        fec_percent: 0,
        min_fec: 0,
        key: None,
    };
    let cap = 2 * 1023 * (8192 - 16) - 8;
    assert!(packetizer.streamable(2, cap));
    assert!(!packetizer.streamable(2, cap + 8176));
    let mut framed = packetizer.clone();
    let mut streamed = framed.streamed(0, 0, 2, 1, cap).unwrap();
    let mut shards = Vec::new();
    let mut packets_seen = 0;
    streamed
        .push(
            &mut framed,
            vec![7u8; 20_000],
            true,
            &mut shards,
            |packets: &mut [Vec<u8>], _| {
                assert!(packets.iter().all(|packet| packet.len() == 8192 + 16));
                packets_seen += packets.len();
                Ok(())
            },
        )
        .unwrap();
    assert!(packets_seen > 0);
    assert!(streamed.is_done());
    // A stock packet size is still accepted, and above the constant is not.
    let mut stock = packetizer.clone();
    stock.packet_size = 1400;
    assert!(stock.streamable(2, 2 * 1023 * (1400 - 16) - 8));
    let mut past = packetizer;
    past.packet_size = 8192 + 16;
    assert!(!past.streamable(2, cap));
}
