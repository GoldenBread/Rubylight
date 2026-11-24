//! Regression and equivalence tests for Slice 4 packetization, PyroWave
//! framing/detail-FEC, audio passthrough, and resampler optimizations.
use super::*;
use crate::audio::Resampler;
use crate::audio_passthrough;
use crate::pyrowave::{DetailFec, Framing, Payload, PayloadCursor};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn packetizer(key: [u8; 16]) -> VideoPacketizer {
    VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
        packet_size: 1024,
        fec_percent: 20,
        min_fec: 2,
        key: Some(key),
    }
}

#[test]
fn video_sealer_is_cached_and_invalidated_on_key_change() {
    let mut vp = packetizer([0x22; 16]);
    let first = vp.sealer().unwrap();
    let second = vp.sealer().unwrap();
    assert!(
        Arc::ptr_eq(&first, &second),
        "repeated sealer lookups with unchanged key should reuse the cached Arc<PacketSealer>"
    );

    vp.key = Some([0x33; 16]);
    let rotated = vp.sealer().unwrap();
    assert!(
        !Arc::ptr_eq(&first, &rotated),
        "changing VideoPacketizer.key must rebuild the cached PacketSealer"
    );

    vp.key = None;
    assert!(vp.sealer().is_none());
}

#[test]
fn streamed_frame_take_pieces_preserves_allocations_and_into_buffer_compat() {
    let vp = packetizer([0x44; 16]);
    let mut frame = vp.streamed(1000, 200, 2, 2, 16 * 1024).unwrap();
    let unit = vec![0xAB; 256];
    let ptr = unit.as_ptr();
    let mut out = Vec::new();
    let mut vp_mut = vp.clone();
    frame
        .push(&mut vp_mut, unit, true, &mut out, |_, _| Ok(()))
        .unwrap();
    let pieces = frame.take_pieces();
    assert_eq!(pieces.len(), 1);
    assert_eq!(pieces[0].as_ptr(), ptr);
    assert_eq!(pieces[0].len(), 256);

    // into_buffer stays backwards-compatible and clears the outer Vec while retaining capacity.
    let mut frame2 = vp_mut.streamed(2000, 200, 2, 2, 16 * 1024).unwrap();
    let mut out2 = Vec::new();
    frame2
        .push(&mut vp_mut, vec![0xCD; 128], true, &mut out2, |_, _| Ok(()))
        .unwrap();
    let reused = frame2.into_buffer();
    assert!(reused.is_empty());
    assert!(reused.capacity() >= 1);
}

#[test]
fn pyrowave_blocks_pooled_preserves_shard_allocations_across_block_sizes() {
    let pool = BlockPool::default();
    let mut vp = packetizer([0x55; 16]);
    vp.key = None;
    let payload: Vec<u8> = (0..64 * 1024).map(|i| (i % 251) as u8).collect();
    let fec = PyrowaveFec {
        records: false,
        critical_percentage: 30,
        detail_percentage: 10,
        wire_budget: usize::MAX,
        ipv6: false,
    };

    // Prime the pool with one block whose shard capacity is large.
    let mut seed = PacketBlock::with_capacity(128);
    for _ in 0..128 {
        seed.push(Vec::with_capacity(vp.packet_size + 64));
    }
    let seed_ptrs: Vec<*const u8> = seed.iter().map(Vec::as_ptr).collect();
    pool.give(seed);

    let (_, _, blocks) = vp
        .pyrowave_blocks_pooled(payload.as_slice(), 100, 50, fec, Some(&pool))
        .unwrap();
    let mut produced = Vec::new();
    for item in blocks {
        let (block, filled) = item.unwrap();
        assert!(filled > 0 && filled <= block.len());
        produced.push((block, filled));
    }
    assert!(!produced.is_empty());
    // First block took our seeded PacketBlock without truncating its 128 pooled shard Vecs.
    assert_eq!(produced[0].0.len(), 128);
    assert!(produced[0].1 < 128);
    assert_eq!(produced[0].0[0].as_ptr(), seed_ptrs[0]);
    assert_eq!(produced[0].0[127].as_ptr(), seed_ptrs[127]);
    for (block, _) in produced {
        pool.give(block);
    }
}

#[test]
fn payload_cursor_matches_direct_reads_including_empty_pieces_and_rewind() {
    let pieces = vec![
        vec![0u8, 1, 2],
        Vec::new(),
        vec![3u8, 4, 5, 6, 7],
        Vec::new(),
        vec![8u8, 9],
    ];
    let payload = Payload::Pieces(&pieces);
    let mut cursor = PayloadCursor::new(payload);
    assert_eq!(cursor.len(), 10);

    let mut out = [0u8; 4];
    cursor.copy_to(0, &mut out);
    assert_eq!(out, [0, 1, 2, 3]);
    cursor.copy_to(4, &mut out);
    assert_eq!(out, [4, 5, 6, 7]);
    let mut tail = [0u8; 2];
    cursor.copy_to(8, &mut tail);
    assert_eq!(tail, [8, 9]);

    // Backward seek falls back cleanly.
    cursor.copy_to(2, &mut out);
    assert_eq!(out, [2, 3, 4, 5]);

    // Framed payload cursor parity.
    let mut raw = Vec::new();
    for w in 0u32..64 {
        raw.extend_from_slice(&w.to_le_bytes());
    }
    let framing = Framing::container(&raw, [(0, 64), (64, 128), (192, 64)].into_iter()).unwrap();
    let framed = framing.view(&raw).unwrap();
    let mut fcur = Payload::Framed(framed).cursor();
    let mut direct = vec![0u8; fcur.len()];
    framed.copy_to(0, &mut direct);
    let mut streamed = vec![0u8; fcur.len()];
    for off in (0..fcur.len()).step_by(17) {
        let end = (off + 17).min(fcur.len());
        fcur.copy_to(off, &mut streamed[off..end]);
    }
    assert_eq!(streamed, direct);
    // Rewind framed cursor too.
    let mut check = [0u8; 13];
    fcur.copy_to(5, &mut check);
    assert_eq!(&check, &direct[5..18]);
}

#[test]
fn audio_packetizer_fourth_packet_matches_cbc_and_handles_key_rotation() {
    let key_a = [0x12u8; 16];
    let key_b = [0x34u8; 16];
    let mut ap = AudioPacketizer::new(key_a, 0x0102_0304, true, 5);
    let mut batches = Vec::new();
    for i in 0..4u8 {
        let payload = vec![i.wrapping_mul(17).wrapping_add(3); 80];
        let out = ap.encode(&payload).unwrap();
        batches.push(out);
    }
    assert_eq!(batches[0].len(), 1);
    assert_eq!(batches[1].len(), 1);
    assert_eq!(batches[2].len(), 1);
    assert_eq!(
        batches[3].len(),
        3,
        "4th packet must emit 1 data + 2 FEC shards"
    );

    // Verify encrypted 4th packet decrypts to exact input using wire::crypto::cbc_open.
    let fourth = &batches[3][0];
    let seq = u16::from_be_bytes([fourth[2], fourth[3]]);
    let mut iv = [0u8; 16];
    iv[0..4].copy_from_slice(&(ap.key_id.wrapping_add(u32::from(seq))).to_be_bytes());
    let plain = rubylight_wire::crypto::cbc_open(&key_a, &iv, &fourth[12..]).unwrap();
    assert_eq!(plain, vec![3u8.wrapping_mul(17).wrapping_add(3); 80]);

    // Mutate key mid-stream and verify cached CbcKey updates.
    ap.key = key_b;
    let after = ap.encode(&[0x77; 40]).unwrap();
    let p = &after[0];
    let seq2 = u16::from_be_bytes([p[2], p[3]]);
    let mut iv2 = [0u8; 16];
    iv2[0..4].copy_from_slice(&(ap.key_id.wrapping_add(u32::from(seq2))).to_be_bytes());
    let plain2 = rubylight_wire::crypto::cbc_open(&key_b, &iv2, &p[12..]).unwrap();
    assert_eq!(plain2, vec![0x77; 40]);
}

#[test]
fn detail_fec_ignores_minor_jitter_below_15_percent_at_120hz() {
    let mut det = DetailFec::new(120_000);
    // Minimal invalid-but-nonempty slice: if observe hashed it, `records()` would fail.
    let bogus = [0xFFu8; 16];
    let t0 = Instant::now();
    assert_eq!(det.observe(&bogus, None, t0, 50_000).unwrap(), (0, 0));
    // 120 Hz nominal is 8.333 ms; 9.0 ms is +8% jitter (< 15%), must skip record parsing/hashing.
    assert_eq!(
        det.observe(&bogus, None, t0 + Duration::from_micros(9_000), 50_000)
            .unwrap(),
        (0, 0)
    );
    // 9.5 ms is +14% (< 15%), still skipped.
    assert_eq!(
        det.observe(&bogus, None, t0 + Duration::from_micros(18_500), 50_000)
            .unwrap(),
        (0, 0)
    );
    // 10.5 ms is +26% (> 15%), triggers the record path (which errors on bogus data).
    assert!(
        det.observe(&bogus, None, t0 + Duration::from_micros(29_000), 50_000)
            .is_err()
    );
}

#[test]
fn resampler_batch_push_and_wrapped_48k_fast_path() {
    let mut r = Resampler::new(48_000, 2).unwrap();
    // Push and drain partially so the VecDeque wraps across its ring boundary.
    let a: Vec<f32> = (0..600).map(|i| i as f32 / 1000.0).collect();
    assert_eq!(r.push_slice_clamped(&a), 0);
    let mut out = Vec::new();
    assert!(r.read_into(200, &mut out));
    assert_eq!(out, a[..400]);

    assert_eq!(r.push_silence(50), 0);
    let b: Vec<f32> = (0..400).map(|i| -(i as f32) / 1000.0).collect();
    assert_eq!(r.push_slice_clamped(&b), 0);

    // Read across remaining `a`, silence, and `b` in one wrapped read_into.
    assert!(r.read_into(350, &mut out));
    assert_eq!(out.len(), 700);
    assert_eq!(&out[..200], &a[400..600]);
    assert!(out[200..300].iter().all(|&s| s == 0.0));
    assert_eq!(&out[300..700], &b[..400]);
}

#[test]
fn audio_passthrough_into_helpers_reuse_buffers() {
    let samples = [-1.5f32, -0.5, 0.0, 0.5, 1.5];
    let mut pcm = Vec::with_capacity(64);
    let ptr = pcm.as_ptr();
    audio_passthrough::pcm_s16le_into(&samples, &mut pcm);
    assert_eq!(pcm, audio_passthrough::pcm_s16le(&samples));
    assert_eq!(pcm.as_ptr(), ptr);

    let frame = vec![0x5Au8; 1500];
    let mut reusable = Vec::new();
    let count1 = audio_passthrough::chunks_into(&frame, 7, &mut reusable);
    assert_eq!(count1, audio_passthrough::chunks(&frame, 7).len());
    assert_eq!(
        &reusable[..count1],
        audio_passthrough::chunks(&frame, 7).as_slice()
    );
    let first_ptr = reusable[0].as_ptr();

    let small = vec![0xA5u8; 200];
    let count2 = audio_passthrough::chunks_into(&small, 8, &mut reusable);
    assert_eq!(count2, 1);
    assert_eq!(reusable[0], audio_passthrough::chunks(&small, 8)[0]);
    assert_eq!(reusable[0].as_ptr(), first_ptr);
    assert!(
        reusable.len() >= count1,
        "chunks_into keeps surplus chunk Vecs pooled for subsequent larger frames"
    );
}
