//! Video packetizer conventional FEC, block reuse, and recovery tests.

use super::*;

mod cauchy;
mod pyrowave;
mod wire;

#[test]
fn large_conventional_frames_report_the_wire_fec_cutoff() {
    assert_eq!(super::conventional_fec(848, 20, 0), (4, 20));
    // Past the cutoff the parity that still fits is kept.
    assert_eq!(super::conventional_fec(849, 20, 0), (4, 19));
    assert_eq!(super::conventional_fec(900, 20, 0), (4, 13));
    assert_eq!(super::conventional_fec(1000, 20, 0), (4, 2));
    assert_eq!(super::conventional_fec(1000, 20, 6), (4, 0));
    assert_eq!(super::conventional_fec(1020, 20, 0), (4, 0));
    assert_eq!(super::conventional_fec(5000, 20, 0), (4, 0));
    assert_eq!(super::conventional_fec(849, 0, 0), (4, 0));
    for shards in 849..1020 {
        let (blocks, percentage) = super::conventional_fec(shards, 20, 2);
        let per_block = shards.div_ceil(blocks);
        let parity = (per_block * percentage)
            .div_ceil(100)
            .max(if percentage > 0 { 2 } else { 0 });
        assert!(per_block + parity <= 255 || percentage == 0, "{shards}");
    }
}

#[test]
fn the_fec_whole_payload_limit_is_the_largest_frame_with_full_parity() {
    let mut packetizer = super::VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
        packet_size: 1392,
        fec_percent: 20,
        min_fec: 2,
        key: Some([0; 16]),
    };
    // 848 shards of 1376 bytes, less the 8-byte frame header: about 1.17 MB.
    let limit = packetizer.fec_whole_payload_limit();
    assert_eq!(limit, 848 * 1376 - 8);
    assert!(!packetizer.fec_limited(limit));
    assert!(packetizer.fec_limited(limit + 1));
    // Less parity fits more data; none fits all four full blocks.
    packetizer.fec_percent = 10;
    assert!(packetizer.fec_whole_payload_limit() > limit);
    assert!(!packetizer.fec_limited(packetizer.fec_whole_payload_limit()));
    packetizer.fec_percent = 0;
    assert_eq!(packetizer.fec_whole_payload_limit(), 1020 * 1376 - 8);
}

#[test]
fn host_processing_uses_punktfunks_truncated_saturating_tenths_of_a_millisecond() {
    // Punktfunk v0.44.0 c5ed1136, gamestream/stream.rs spawn_packetizer:
    // elapsed microseconds / 100, then saturating u16 conversion.
    for (micros, expected) in [
        (0, 0),
        (49, 0),
        (50, 0),
        (99, 0),
        (100, 1),
        (199, 1),
        (1234, 12),
        (6_553_599, 65_535),
        (u64::MAX, 65_535),
    ] {
        let mut packetizer = VideoPacketizer {
            sequence: 0,
            iv_counter: 0,
            frame: 1,
            packet_size: 1392,
            fec_percent: 0,
            min_fec: 0,
            key: None,
        };
        let packets = packetizer.encode(&[42; 32], false, 9000, micros).unwrap();
        assert_eq!(
            u16::from_le_bytes(packets[0][33..35].try_into().unwrap()),
            expected,
            "{micros}"
        );
    }
}

#[test]
fn minimum_fec_uses_the_receivers_exact_parity_count() {
    for data in 1..=1023 {
        for percentage in [0, 1, 20, 100, 255] {
            for minimum in [0, 2, 24, 42, 255] {
                let (parity, effective) = block_parity(data, percentage, minimum);
                assert!(effective <= 255);
                assert_eq!(parity, (data * effective).div_ceil(100));
                if percentage > 0 {
                    assert!(parity >= crate::pyrowave::minimum_parity(data, minimum));
                }
            }
        }
    }
    // 44 parity shards cannot be advertised exactly for 211 data shards.
    // The representable 45 exceed the 255-shard limit, so split the frame.
    assert_eq!(conventional_fec(211, 20, 44), (2, 20));
    for data in 1..=1020 {
        for percentage in [1, 20, 100] {
            for minimum in [2, 24, 42, 255] {
                let (blocks, effective) = conventional_fec(data, percentage, minimum);
                let aligned = data.div_ceil(blocks);
                for block in 0..blocks {
                    let count = data.saturating_sub(block * aligned).min(aligned);
                    assert!(count + block_parity(count, effective, minimum).0 <= 255);
                }
            }
        }
    }
}

#[test]
fn minimum_fec_recovers_loss_using_the_wire_selected_matrix() {
    for key in [None, Some([42; 16])] {
        let mut packetizer = VideoPacketizer {
            sequence: 0,
            iv_counter: 0,
            frame: 1,
            packet_size: 1392,
            fec_percent: 20,
            min_fec: 42,
            key,
        };
        let payload: Vec<_> = (0..201 * 1376 - 8)
            .map(|i| (i * 29 + i / 1376) as u8)
            .collect();
        let packets: Vec<_> = packetizer
            .encode(&payload, false, 9000, 500)
            .unwrap()
            .into_iter()
            .map(|packet| match key {
                Some(key) => {
                    crypto::gcm_open(&key, &packet[..12], &packet[16..32], &packet[32..]).unwrap()
                }
                None => packet,
            })
            .collect();
        let info = u32::from_le_bytes(packets[0][28..32].try_into().unwrap());
        let data = (info >> 22) as usize;
        let percentage = ((info >> 4) & 255) as usize;
        let parity = (data * percentage).div_ceil(100);
        assert_eq!((data, percentage, parity), (201, 21, 43));
        assert_eq!(packets.len(), data + parity);
        // Recover a missing data shard with the client's matrix formula. The
        // advertised parity count selects the coefficients as well as the size.
        let missing = 17;
        for offset in [32, 40, 255, 1407] {
            let mut recovered = packets[data][offset];
            for (column, packet) in packets[..data].iter().enumerate() {
                if column != missing {
                    recovered ^= gf_mul(packet[offset], inverse((parity + column) as u8));
                }
            }
            recovered = gf_mul(recovered, (parity + missing) as u8);
            assert_eq!(recovered, packets[missing][offset]);
        }
    }
}

#[test]
fn conventional_blocks_reuse_shards_and_match_collected_packets() {
    for key in [None, Some([42; 16])] {
        let make = || VideoPacketizer {
            sequence: 0xFFFFFF,
            iv_counter: 0xFFFFFFFE,
            frame: u32::MAX,
            packet_size: 1392,
            fec_percent: 20,
            min_fec: 2,
            key,
        };
        let mut expected = make();
        let mut actual = make();
        let mut buffers = Vec::new();
        let mut pointers = Vec::new();
        for size in [792_568, 10, 264_184, 792_568] {
            let payload: Vec<u8> = (0..size).map(|i| (i * 29) as u8).collect();
            let expected_packets = expected
                .encode_recovery(&payload, false, true, 9000, 500)
                .unwrap();
            let (count, mut next) = actual
                .conventional_blocks(&payload, false, true, 9000, 500)
                .unwrap();
            assert_eq!(count, expected_packets.len());
            let mut offset = 0;
            loop {
                let count = next(&mut buffers).unwrap();
                if count == 0 {
                    break;
                }
                for (buffer, pointer) in buffers.iter().zip(&pointers) {
                    assert_eq!(buffer.as_ptr(), *pointer);
                }
                pointers = buffers.iter().map(Vec::as_ptr).collect();
                assert_eq!(&buffers[..count], &expected_packets[offset..offset + count]);
                offset += count;
            }
            assert_eq!(offset, expected_packets.len());
            drop(next);
            assert_eq!(actual.frame, expected.frame);
            assert_eq!(actual.sequence, expected.sequence);
            assert_eq!(actual.iv_counter, expected.iv_counter);
        }
    }
}

#[test]
fn the_precomputed_conventional_plan_matches_the_blocks_built_on_the_fly() {
    for key in [None, Some([42; 16])] {
        let make = || VideoPacketizer {
            sequence: 0xFFFFFF,
            iv_counter: 0xFFFFFFFE,
            frame: u32::MAX,
            packet_size: 1392,
            fec_percent: 20,
            min_fec: 2,
            key,
        };
        for size in [10, 1376, 264_184, 792_568, 1_300_000] {
            let payload: Vec<u8> = (0..size).map(|i| (i * 29) as u8).collect();
            let mut expected = make();
            let mut actual = make();
            let plan = actual.conventional_plan(payload.len());
            assert_eq!(plan.layout, expected.block_layout(payload.len()));
            assert_eq!(
                plan.limited(actual.fec_percent),
                expected.fec_limited(payload.len())
            );
            let (count, mut expected_next) = expected
                .conventional_blocks(&payload, false, true, 9000, 500)
                .unwrap();
            let (planned_count, mut actual_next) = actual
                .conventional_blocks_planned(&payload, false, true, 9000, 500, &plan)
                .unwrap();
            assert_eq!(planned_count, count);
            let (mut expected_shards, mut actual_shards) = (Vec::new(), Vec::new());
            loop {
                let ec = expected_next(&mut expected_shards).unwrap();
                let ac = actual_next(&mut actual_shards).unwrap();
                assert_eq!(ec, ac);
                if ec == 0 {
                    break;
                }
                assert_eq!(&expected_shards[..ec], &actual_shards[..ac], "size {size}");
            }
            drop((expected_next, actual_next));
            assert_eq!(actual.frame, expected.frame);
            assert_eq!(actual.sequence, expected.sequence);
            assert_eq!(actual.iv_counter, expected.iv_counter);
        }
    }
}

#[test]
fn conventional_blocks_overwrite_every_byte_of_dirty_buffers() {
    // Reused shards are not cleared as a whole; stale bytes must never leak
    // into a header, a payload tail, a parity row or the envelope. 1,300,000
    // bytes take four blocks with reduced parity and end on a short shard.
    for (key, (fec_percent, min_fec), size) in [None, Some([42; 16])]
        .into_iter()
        .flat_map(|key| [(20, 2), (0, 0)].map(|fec| (key, fec)))
        .flat_map(|(key, fec)| {
            [10, 1375, 1376, 44_024, 264_184, 1_300_000].map(|size| (key, fec, size))
        })
    {
        let make = || VideoPacketizer {
            sequence: 77,
            iv_counter: 5,
            frame: 3,
            packet_size: 1392,
            fec_percent,
            min_fec,
            key,
        };
        if size == 1_300_000 {
            let layout = make().block_layout(size);
            assert_eq!(layout.len(), 4);
            assert!(!(size + 8).is_multiple_of(1376));
            if fec_percent > 0 {
                assert!(make().fec_limited(size));
                assert!(layout.iter().all(|&(_, parity)| parity > 0));
            }
        }
        let payload: Vec<u8> = (0..size).map(|i| (i * 31 + 7) as u8).collect();
        let expected = make().encode(&payload, true, 9000, 500).unwrap();
        let mut buffers: Vec<Vec<u8>> = (0..300).map(|_| vec![0xA5; 1500]).collect();
        let mut actual = make();
        let (_, mut next) = actual
            .conventional_blocks(&payload, true, false, 9000, 500)
            .unwrap();
        let mut wire = Vec::new();
        loop {
            let count = next(&mut buffers).unwrap();
            if count == 0 {
                break;
            }
            wire.extend(buffers[..count].iter().cloned());
            for buffer in &mut buffers {
                buffer.fill(0xA5);
            }
        }
        assert_eq!(
            wire,
            expected,
            "size {size}, fec {fec_percent}%, encrypted {}",
            key.is_some()
        );
    }
}

#[test]
fn abandoning_conventional_blocks_keeps_frame_and_nonce_progress() {
    let payload = vec![7; 792_568];
    let mut p = VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
        packet_size: 1392,
        fec_percent: 20,
        min_fec: 2,
        key: Some([42; 16]),
    };
    let mut buffers = Vec::new();
    let (_, next) = p
        .conventional_blocks(&payload, false, false, 9000, 543)
        .unwrap();
    drop(next);
    assert_eq!((p.sequence, p.iv_counter, p.frame), (0, 0, 1));
    let (total, mut next) = p
        .conventional_blocks(&payload, false, false, 9000, 543)
        .unwrap();
    let first = next(&mut buffers).unwrap();
    assert!(first < total);
    drop(next);
    assert_eq!(p.sequence, first as u32);
    assert_eq!(p.iv_counter, first as u64);
    assert_eq!(p.frame, 2);
    assert_eq!(&buffers[0][12..16], &1u32.to_le_bytes());
    let (count, mut next) = p
        .conventional_blocks(&payload, true, false, 9750, 543)
        .unwrap();
    assert_eq!(count, total);
    assert_eq!(next(&mut buffers).unwrap(), first);
    assert_eq!(&buffers[0][..8], &(first as u64).to_le_bytes());
    assert_eq!(&buffers[0][12..16], &2u32.to_le_bytes());
}

#[test]
fn conventional_blocks_match_pre_pipeline_fingerprints() {
    let cases = [
        (
            32,
            false,
            "bc399e222e5a4ae7e26c450a17255b398230f83b22887ecb1a154fe184a32b76",
        ),
        (
            32,
            true,
            "8ba8584ce4c91aaf819f46ddecca4ca225afbcc57df358cce0d083f05fc1eef2",
        ),
        (
            192,
            false,
            "ef623c7f9e369b6f484c8b25674b640051689bddf0d143c179a0f22e778425a2",
        ),
        (
            192,
            true,
            "fd2014942f4fe1b55b4656ece29610ace6e30c4c5ef9342be2c3e04a7be27da5",
        ),
        (
            576,
            false,
            "6651ed521955b8d3e198bea9bad990362efa386d686968acada77b0d56e8a8ad",
        ),
        (
            576,
            true,
            "f42a64f087e69d821b868d5dbbc5310f43b320d1cb6f139d1583af19fc2b187e",
        ),
    ];
    for (shards, encrypted, fingerprint) in cases {
        let payload: Vec<u8> = (0..shards * 1376 - 8)
            .map(|i| ((i * 73 + (i >> 3)) & 255) as u8)
            .collect();
        let mut p = VideoPacketizer {
            sequence: 65000,
            iv_counter: 65534,
            frame: 7,
            packet_size: 1392,
            fec_percent: 20,
            min_fec: 2,
            key: encrypted.then(|| std::array::from_fn(|i| i as u8)),
        };
        let (_, mut next) = p
            .conventional_blocks(&payload, true, false, 9000, 500)
            .unwrap();
        let mut buffers = Vec::new();
        let mut wire = Vec::new();
        loop {
            let count = next(&mut buffers).unwrap();
            if count == 0 {
                break;
            }
            for packet in &buffers[..count] {
                wire.extend_from_slice(packet);
            }
        }
        assert_eq!(hex::encode(crypto::hash(&wire)), fingerprint);
    }
}
