//! PyroWave packetization, block pooling, and jumbo record FEC tests.

use super::*;

#[test]
fn abandoning_pyrowave_blocks_keeps_frame_and_nonce_progress() {
    let payload = vec![7u8; 400_000];
    let mut p = VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
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
    let (total, _, mut blocks) = p.pyrowave_blocks(&payload, 9000, 543, fec()).unwrap();
    let first = blocks.next().unwrap().unwrap();
    assert!(first.len() < total);
    drop(blocks);
    assert_eq!(p.sequence, first.len() as u32);
    assert_eq!(p.iv_counter, first.len() as u64);
    assert_eq!(p.frame, 2);
    assert_eq!(&first[0][12..16], &1u32.to_le_bytes());

    let next = p.encode_pyrowave(&payload, 9750, 543, fec()).unwrap();
    assert_eq!(next.len(), total);
    assert_eq!(&next[0][..8], &(first.len() as u64).to_le_bytes());
    assert_eq!(&next[0][12..16], &2u32.to_le_bytes());
    assert_eq!(p.frame, 3);
    assert_eq!(p.sequence, (first.len() + total) as u32);
    assert_eq!(p.iv_counter, (first.len() + total) as u64);
}

/// Review 2026-11-07: PyroWave allocated and zero-filled every packet of every
/// frame. Blocks built in returned buffers, even ones of another size and
/// full of the last frame's bytes, are the same bytes, and the buffers stay.
#[test]
fn pyrowave_blocks_in_reused_buffers_match_fresh_ones() {
    let fec = || PyrowaveFec {
        records: false,
        critical_percentage: 20,
        detail_percentage: 0,
        wire_budget: 0,
        ipv6: false,
    };
    let make = || VideoPacketizer {
        sequence: 65000,
        iv_counter: 9,
        frame: 3,
        packet_size: 1392,
        fec_percent: 0,
        min_fec: 2,
        key: Some([42; 16]),
    };
    let pool = BlockPool::default();
    let (mut fresh, mut reused) = (make(), make());
    for (index, size) in [400_000usize, 90_000, 700_000, 1_000]
        .into_iter()
        .enumerate()
    {
        let payload: Vec<u8> = (0..size).map(|i| (i * 31 + index) as u8).collect();
        let expected: Vec<_> = fresh
            .pyrowave_blocks(&payload, 9000, 543, fec())
            .unwrap()
            .2
            .map(Result::unwrap)
            .collect();
        let (count, _, blocks) = reused
            .pyrowave_blocks_in(&payload, 9000, 543, fec(), Some(&pool))
            .unwrap();
        let actual: Vec<_> = blocks.map(Result::unwrap).collect();
        assert_eq!(actual, expected, "frame {index}");
        assert_eq!(actual.iter().map(Vec::len).sum::<usize>(), count);
        for block in actual {
            pool.give(block);
        }
        assert!(pool.len() > 0);
    }
    assert_eq!(
        (fresh.sequence, fresh.iv_counter, fresh.frame),
        (reused.sequence, reused.iv_counter, reused.frame)
    );
}

#[test]
fn a_block_pool_hands_blocks_back_in_order_and_keeps_a_few() {
    let pool = BlockPool::default();
    for size in 1..=20 {
        pool.give(vec![Vec::new(); size]);
    }
    assert_eq!(pool.len(), 16);
    let first = pool.take();
    assert_eq!(first.len(), 1);
    pool.unused(first);
    assert_eq!(pool.take().len(), 1);
    assert_eq!(pool.take().len(), 2);
    assert!(BlockPool::default().take().is_empty());
}

#[test]
fn legacy_pyrowave_uses_the_conventional_receivers_fec_bytes() {
    for key in [None, Some([42; 16])] {
        for (shards, minimum) in [(1, 2), (201, 42), (600, 2), (849, 2), (1020, 2), (1200, 2)] {
            let make = || VideoPacketizer {
                sequence: 65534,
                iv_counter: 65534,
                frame: 7,
                packet_size: 1392,
                fec_percent: 20,
                min_fec: minimum,
                key,
            };
            let payload: Vec<_> = (0..shards * 1376 - 8).map(|i| (i * 29) as u8).collect();
            let expected = make().encode(&payload, true, 9000, 543).unwrap();
            let mut pyro = make();
            let (count, limited, blocks) = pyro
                .pyrowave_blocks(
                    &payload,
                    9000,
                    543,
                    PyrowaveFec {
                        records: false,
                        critical_percentage: 20,
                        detail_percentage: 50,
                        wire_budget: usize::MAX,
                        ipv6: true,
                    },
                )
                .unwrap();
            let actual: Vec<_> = blocks.flat_map(|block| block.unwrap()).collect();
            assert_eq!(actual.len(), count);
            // Legacy framing changes neither parity bytes nor the ordinary
            // receiver's matrix selection, including minimum-parity rounding.
            assert_eq!(actual, expected, "{shards} shards, minimum {minimum}");
            assert_eq!(limited, conventional_fec(shards, 20, minimum).1 < 20);
            assert_eq!(count > shards, shards < 1020);
        }
    }
}

#[test]
fn legacy_pyrowave_rejects_frames_past_the_legacy_shard_cap() {
    let mut packetizer = VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
        packet_size: 256,
        fec_percent: 0,
        min_fec: 2,
        key: None,
    };
    for rate in [0, 20] {
        assert!(
            packetizer
                .pyrowave_blocks(
                    &vec![0u8; 4000 * 240],
                    0,
                    0,
                    PyrowaveFec {
                        records: false,
                        critical_percentage: rate,
                        detail_percentage: 0,
                        wire_budget: 0,
                        ipv6: false,
                    }
                )
                .is_err()
        );
    }
}

#[test]
fn pyrowave_blocks_match_pre_pipeline_bytes() {
    use crate::pyrowave;
    // Captured before incremental sending, including parity, record restart
    // flags, encryption, and two frames crossing the sequence/frame wrap.
    let cases = [
        (
            1392,
            true,
            800,
            0,
            2,
            619,
            "b2b826c26f16471691fe0ca4d2207a6ffef47cd0c51ce2a5222856f5e7f9acec",
            [
                "169af59cd64cc956d735e679ebeaa8442224acc11f2c7f341cf1ac4e276b3539",
                "bb105d41f7fd6c13a6c16bd31213c8364ba4c514c8375d900a64e0a996b4f992",
            ],
        ),
        (
            1392,
            true,
            800,
            50,
            2,
            805,
            "b2b826c26f16471691fe0ca4d2207a6ffef47cd0c51ce2a5222856f5e7f9acec",
            [
                "06de60def4c061f8e9835050aa6233f71d8e6b8cae6734b2f5cea1713dd6e50d",
                "bada91533e981e5c142d2860ad726c9f2943b7cb40625b8dc759b4284d50ae5f",
            ],
        ),
        (
            1390,
            true,
            800,
            50,
            2,
            590,
            "4d6e3b3dc7f7f37d09442e2a532b7562e222d38a0541e14cd87190d33e5b8421",
            [
                "22573bace717c714caa86542103f4b494181c9f448b3225118f618fd2da5d071",
                "64900b28c9b4e171d0dad8898e15f6e243e21eb99b1bbed2b4e651e3340a4a80",
            ],
        ),
        (
            1400,
            false,
            800,
            0,
            2,
            585,
            "abb0c4381be111b03f15691e8d7a056e135f851358a2f8e46d4452a86c2740c5",
            [
                "e89b3bfb00e3087a807b5cb18df98b46645c866ea48199506123ca9944617979",
                "de8233c384c800bbc88f9f34395562a81ddcaff97e2a906f6ec9e93fbbcd5a6e",
            ],
        ),
        (
            256,
            true,
            24,
            50,
            4,
            102,
            "3e05f81bb5698a8d8b86b0613fcda7bf1c9ae99cde33ce1d9168e8ffea446e70",
            [
                "08962468200244aa2219af92e5058848da77016a1d08e55b87d6dcb330b63a66",
                "199475562fd3b81146a67a55f11307cab5c95106e63c4d19f341cae9cc354f89",
            ],
        ),
        (
            1392,
            true,
            24,
            0,
            2,
            20,
            "be5a30a5632247d5eff24f07c3bdc1de4f53237a02f6cd2b023b50d901b4aea4",
            [
                "1f5230361b737209b8fae71640a49c831527599629e69446884d839dd37e4252",
                "5514b5aefaf96cb6d6779a3fb57dfd1e8c0c08d1ce69df47a12d399cacba04fd",
            ],
        ),
    ];
    for (size, records, count, detail, minimum, packet_count, framed_hash, wire_hashes) in cases {
        let mut raw = Vec::new();
        raw.extend_from_slice(&(0x80000000u32 | 1919 | (1079 << 14)).to_le_bytes());
        raw.extend_from_slice(&(count as u32).to_le_bytes());
        for i in 0..count {
            let bytes = 8 + (i * 73 % 504) * 4;
            raw.extend_from_slice(&((bytes as u32 / 4) << 16).to_le_bytes());
            raw.extend_from_slice(&((((i * 37) % count) as u32) << 8).to_le_bytes());
            raw.extend((0..bytes - 8).map(|j| (i * 31 + j * 17) as u8));
        }
        let framed = if records {
            pyrowave::record_frame(
                &raw,
                pyrowave::aligned_payload(size),
                pyrowave::max_frame_bytes(size, true),
            )
            .unwrap()
        } else {
            pyrowave::container(&raw, &[(0, 8), (8, raw.len() - 8)]).unwrap()
        };
        assert_eq!(hex::encode(crypto::hash(&framed)), framed_hash);
        for (encrypted, expected) in [false, true].into_iter().zip(wire_hashes) {
            let mut p = VideoPacketizer {
                sequence: u32::MAX - 5,
                iv_counter: 65534,
                frame: u32::MAX,
                packet_size: size,
                fec_percent: 0,
                min_fec: minimum,
                key: encrypted.then(|| std::array::from_fn(|i| i as u8)),
            };
            let mut all = Vec::new();
            for _ in 0..2 {
                let (total, fec_limited, blocks) = p
                    .pyrowave_blocks(
                        &framed,
                        9000,
                        543,
                        PyrowaveFec {
                            records,
                            critical_percentage: if records { 20 } else { 0 },
                            detail_percentage: detail,
                            wire_budget: 2_000_000,
                            ipv6: true,
                        },
                    )
                    .unwrap();
                assert_eq!(total, packet_count);
                // The unaligned 1390-byte fixture cannot carry critical FEC.
                assert_eq!(fec_limited, records && size == 1390);
                let mut emitted = 0;
                for (block_index, block) in blocks.enumerate() {
                    let block = block.unwrap();
                    emitted += block.len();
                    for packet in block {
                        let plain = if encrypted {
                            crypto::gcm_open(
                                &std::array::from_fn(|i| i as u8),
                                &packet[..12],
                                &packet[16..32],
                                &packet[32..],
                            )
                            .unwrap()
                        } else {
                            packet.clone()
                        };
                        assert_eq!((plain[27] >> 4) & 3, block_index as u8);
                        all.extend(packet);
                    }
                }
                assert_eq!(emitted, total);
            }
            assert_eq!(hex::encode(crypto::hash(&all)), expected);
            assert_eq!(p.frame, 1);
            assert_eq!(
                p.sequence,
                (u32::MAX - 5).wrapping_add(2 * packet_count as u32)
            );
            assert_eq!(
                p.iv_counter,
                65534
                    + if encrypted {
                        2 * packet_count as u64
                    } else {
                        0
                    }
            );
        }
    }
}

/// A PyroWave bitstream of `records` records: its header, the record count and
/// per record the size and index words, then filler.
fn record_bitstream(records: &[(u32, usize)]) -> Vec<u8> {
    let mut out = vec![];
    out.extend_from_slice(&(0x80000000u32 | 1919 | (1079 << 14)).to_le_bytes());
    out.extend_from_slice(&(records.len() as u32).to_le_bytes());
    for &(index, size) in records {
        out.extend_from_slice(&((size as u32 / 4) << 16).to_le_bytes());
        out.extend_from_slice(&(index << 8).to_le_bytes());
        out.resize(out.len() + size - 8, 42);
    }
    out
}

/// Issue #113: jumbo PyroWave records packetize at 8192 bytes, and a lost data
/// shard of the first FEC block is recoverable from its parity.
#[test]
fn jumbo_pyrowave_records_packetize_and_recover_fec() {
    use crate::pyrowave;
    let packet_size = usize::from(rubylight_protocol::video_packet::MAX_VIDEO_PACKET);
    assert_eq!(packet_size, 8192);
    let bitstream = record_bitstream(&(0..6).map(|index| (index, 8192)).collect::<Vec<_>>());
    let payload = pyrowave::record_frame(
        &bitstream,
        pyrowave::aligned_payload(packet_size),
        pyrowave::max_frame_bytes(packet_size, true),
    )
    .unwrap();
    let critical = pyrowave::layout(&payload, pyrowave::aligned_payload(packet_size))
        .unwrap()
        .critical_shards;
    assert!(critical > 0);
    let mut packetizer = VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
        packet_size,
        fec_percent: 0,
        min_fec: 2,
        key: None,
    };
    let packets = packetizer
        .encode_pyrowave(
            &payload,
            9000,
            500,
            PyrowaveFec {
                records: true,
                critical_percentage: 20,
                detail_percentage: 0,
                wire_budget: 0,
                ipv6: true,
            },
        )
        .unwrap();
    // Every datagram is the negotiated size plus Moonlight's 16-byte header.
    assert!(
        packets
            .iter()
            .all(|packet| packet.len() == packet_size + 16)
    );
    assert!(packets.len() > critical);
    // Recover a lost data shard of the first block from its parity, as the
    // client does: the advertised counts select both size and coefficients.
    let info = u32::from_le_bytes(packets[0][28..32].try_into().unwrap());
    let data = (info >> 22) as usize;
    let percentage = ((info >> 4) & 255) as usize;
    let parity = (data * percentage).div_ceil(100);
    assert!(data > 2 && parity > 0);
    let block = &packets[..data + parity];
    let missing = data / 2;
    for offset in [32, 40, packet_size + 15] {
        let mut recovered = block[data][offset];
        for (column, packet) in block[..data].iter().enumerate() {
            if column != missing {
                recovered ^= gf_mul(packet[offset], inverse((parity + column) as u8));
            }
        }
        recovered = gf_mul(recovered, (parity + missing) as u8);
        assert_eq!(recovered, block[missing][offset], "offset {offset}");
    }
}
