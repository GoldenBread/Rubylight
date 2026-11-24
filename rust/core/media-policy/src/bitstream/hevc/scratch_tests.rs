//! Multi-slice and scratch-buffer reuse tests for [`PictureOrder`].
use super::*;

const STREAM: &[u8] = include_bytes!("../fixtures/amf-hevc-ltr.bin");

/// Derive a later independent slice of the same picture as `first_nal` (an
/// Annex B slice NAL without start code), at CTB address `address`, appending
/// `tail` as simulated slice payload bytes after the header.
fn later_slice(first_nal: &[u8], sps: &Sps, pps: &Pps, address: u32, tail: &[u8]) -> Vec<u8> {
    let (rbsp, _) = unescape(&first_nal[2..], usize::MAX);
    let mut reader = Reader::new(&rbsp);
    assert!(reader.flag().unwrap(), "expected first slice");
    let kind = (first_nal[0] >> 1) & 63;
    assert!(!(16..=23).contains(&kind), "expected non-IRAP P slice");
    let pps_id = reader.ue().unwrap();
    let after_pps = reader.at;
    // Locate the alignment bit at the end of the slice header.
    reader.skip(pps.extra_bits).unwrap();
    let slice_type = reader.ue().unwrap();
    if pps.output_flag {
        reader.skip(1).unwrap();
    }
    if sps.separate_colour_plane {
        reader.skip(2).unwrap();
    }
    reader.skip(sps.lsb_bits as usize).unwrap();
    let owned_rps;
    let short_term = if reader.flag().unwrap() {
        &sps.sets[reader.bits(ceil_log2(sps.sets.len() as u32)).unwrap() as usize]
    } else {
        owned_rps = short_term_set(&mut reader, sps.sets.len(), sps.sets.len(), &sps.sets).unwrap();
        &owned_rps
    };
    let mut used = short_term.deltas().filter(|(_, used)| *used).count();
    if sps.long_term {
        for _ in 0..reader.ue().unwrap() {
            reader.skip(sps.lsb_bits as usize).unwrap();
            used += usize::from(reader.flag().unwrap());
            if reader.flag().unwrap() {
                reader.ue().unwrap();
            }
        }
    }
    let end = rest_of_header(&mut reader, sps, pps, slice_type, used as u32).unwrap();

    let mut writer = Writer::default();
    writer.flag(false);
    writer.ue(pps_id);
    if pps.dependent_slices {
        writer.flag(false);
    }
    writer.bits(sps.address_bits, address);
    writer.copy(&rbsp, after_pps, end);
    let mut nal = vec![0, 0, 0, 1, first_nal[0], first_nal[1]];
    super::super::rbsp::escape(&writer.finish(), &mut nal);
    nal.extend_from_slice(tail);
    nal
}

/// Reference hand-assembled rewrite using `unit()`.
fn rewrite_by_hand(order: &mut PictureOrder, bytes: &[u8]) -> Result<Option<Vec<u8>>> {
    let mut output = Vec::new();
    let mut cursor = 0;
    let mut changed = false;
    for (start, end) in units(bytes) {
        if let Some((head, rest)) = order.unit(&bytes[start..end])? {
            output.extend_from_slice(&bytes[cursor..start]);
            output.extend_from_slice(&head);
            output.extend_from_slice(&bytes[start + rest..end]);
            cursor = end;
            changed = true;
        }
    }
    output.extend_from_slice(&bytes[cursor..]);
    Ok(changed.then_some(output))
}

#[test]
fn multi_slice_frames_rewrite_bit_identically_whole_or_slice_by_slice_and_reuse_scratch() {
    let nals = units(STREAM);
    let rbsp = |unit: (usize, usize)| unescape(&STREAM[unit.0 + 2..unit.1], usize::MAX).0;
    let sps = sequence_parameter_set(&rbsp(nals[1])).unwrap().1;
    let pps = picture_parameter_set(&rbsp(nals[2])).unwrap().1;
    assert!(!sps.sets.is_empty(), "fixture exercises SPS RPS borrowing");

    // VPS, SPS, PPS and IDR as the opening frame, followed by 40 four-slice P-frames
    // (crossing the 4-bit POC wrap twice and exercising LTR references).
    // Each slice carries > HEADER_FIRST payload bytes so `unescape_into` hits
    // steady-state buffer sizes from the first frame.
    let pad = [0x77u8; 300];
    let mut frames: Vec<Vec<Vec<u8>>> = vec![vec![STREAM[..nals[3].1].to_vec()]];
    for (frame_idx, &(start, end)) in nals[4..44].iter().enumerate() {
        let first_nal = &STREAM[start..end];
        let mut first = vec![0, 0, 0, 1];
        first.extend_from_slice(first_nal);
        first.extend_from_slice(&[0xAB, (frame_idx & 0xFF) as u8, 0xCD]);
        first.extend_from_slice(&pad);
        let mut slices = vec![first];
        for slice_idx in 1..4u32 {
            let mut tail = vec![0x10 + slice_idx as u8, (frame_idx & 0xFF) as u8, 0x77, 0x99];
            tail.extend_from_slice(&pad);
            slices.push(later_slice(first_nal, &sps, &pps, slice_idx * 16, &tail));
        }
        frames.push(slices);
    }

    let whole_stream: Vec<u8> = frames
        .iter()
        .flat_map(|slices| slices.iter().flat_map(|s| s.iter().copied()))
        .collect();

    // 1. Hand-assembled reference across the whole multi-slice stream.
    let expected = rewrite_by_hand(&mut PictureOrder::default(), &whole_stream)
        .unwrap()
        .unwrap();

    // 2. One-shot `rewrite_into` across the whole multi-slice stream.
    let mut whole_out = Vec::new();
    assert_eq!(
        PictureOrder::default().rewrite_into(&whole_stream, &mut whole_out),
        Ok(true)
    );
    assert_eq!(whole_out, expected);

    // 3. Frame-by-frame (4 slices per buffer), verifying bit-identical output
    //    and stable scratch buffer capacities after warmup.
    let mut order = PictureOrder::default();
    let mut frame_buf = Vec::new();
    let mut by_frame = Vec::new();
    let mut warmup_caps = None;
    for (index, slices) in frames.iter().enumerate() {
        let frame_bytes: Vec<u8> = slices.concat();
        assert_eq!(order.rewrite_into(&frame_bytes, &mut frame_buf), Ok(true));
        by_frame.extend_from_slice(&frame_buf);
        if index == 20 {
            warmup_caps = Some((
                order.rbsp.starts.capacity(),
                order.rbsp.nals.capacity(),
                order.rbsp.edits.capacity(),
                order.rbsp.heads.capacity(),
                order.rbsp.rbsp.capacity(),
                order.rbsp.from.capacity(),
            ));
        }
    }
    assert_eq!(by_frame, expected);
    let caps = warmup_caps.unwrap();
    assert!(caps.0 > 0 && caps.2 > 0 && caps.3 > 0 && caps.4 > 0);
    assert_eq!(order.rbsp.starts.capacity(), caps.0);
    assert_eq!(order.rbsp.nals.capacity(), caps.1);
    assert_eq!(order.rbsp.edits.capacity(), caps.2);
    assert_eq!(order.rbsp.rbsp.capacity(), caps.4);
    assert_eq!(order.rbsp.from.capacity(), caps.5);

    // 4. Slice-by-slice (as AMF slice delivery hands units to `repaired`),
    //    verifying bit-identical output across separate buffers per slice.
    let mut order = PictureOrder::default();
    let mut by_slice = Vec::new();
    for slices in &frames {
        for slice in slices {
            assert_eq!(order.rewrite_into(slice, &mut frame_buf), Ok(true));
            by_slice.extend_from_slice(&frame_buf);
        }
    }
    assert_eq!(by_slice, expected);
    // Already-widened multi-slice stream returns `Ok(false)` with empty output.
    assert_eq!(
        PictureOrder::default().rewrite_into(&expected, &mut frame_buf),
        Ok(false)
    );
    assert!(frame_buf.is_empty());
}
