//! Fake-driver tests for NVENC scene-cut keyframe re-encoding and subframe slice delivery.

use super::*;

#[test]
fn an_oversize_scene_cut_alone_in_flight_is_encoded_again_as_a_keyframe() -> Result<()> {
    reset();
    let mut encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    let slot = input(&mut encoder, 17)?;
    let other = input(&mut encoder, 18)?;
    // Every fake picture is 8 bytes: past a 4-byte limit.
    encoder.cut_limit = Some(4);
    encoder.submit(slot, None, true, Instant::now())?;
    let output = encoder.poll()?;
    assert!(output[0].idr && !output[0].reencoded, "a keyframe is sent");
    let presentation = Instant::now() - Duration::from_millis(5);
    encoder.submit(slot, None, false, presentation)?;
    let output = encoder.poll()?;
    assert_eq!(output.len(), 1);
    assert!(output[0].idr && output[0].reencoded);
    // The keyframe keeps the cut's number and presentation.
    assert_eq!(output[0].bytes, 2u64.to_le_bytes());
    assert_eq!(output[0].presentation, Some(presentation));
    assert_eq!(encoder.reencoded_cuts, 1);
    assert_eq!(encoder.next, 3);
    // A cut with a later frame in flight is sent: that frame references it.
    encoder.submit(slot, None, false, Instant::now())?;
    encoder.submit(other, None, false, Instant::now())?;
    let output = encoder.poll()?;
    assert!(!output[0].idr && !output[0].reencoded);
    assert_eq!(output[0].bytes, 3u64.to_le_bytes());
    assert!(output[1].reencoded, "the newest is alone once collected");
    assert_eq!(encoder.reencoded_cuts, 2);
    // Without a limit nothing is encoded again.
    encoder.cut_limit = None;
    encoder.submit(slot, None, false, Instant::now())?;
    assert!(!encoder.poll()?[0].idr);
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn a_reencoded_scene_cut_has_its_own_completion_deadline() -> Result<()> {
    reset();
    let mut encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    let slot = input(&mut encoder, 17)?;
    encoder.cut_limit = Some(4);
    encoder.submit(slot, None, true, Instant::now())?;
    encoder.poll()?;
    encoder.submit(slot, None, false, Instant::now())?;
    // The inter frame took longer than the completion timeout to finish.
    let slow = COMPLETION_TIMEOUT + Duration::from_millis(50);
    encoder.pending[0].started -= slow;
    encoder.pending[0].submitted -= slow;
    // Its keyframe is not ready at the first lock; that is no timeout.
    state(|d| d.keyframe_busy = 1);
    assert!(encoder.poll()?.is_empty());
    assert_eq!(encoder.pending.len(), 1);
    let output = encoder.poll()?;
    assert!(output[0].idr && output[0].reencoded);
    // Its encode time still counts from the first submission.
    assert!(output[0].latency.unwrap() >= slow);
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn a_failed_scene_cut_reencode_sends_the_inter_frame_and_says_so() -> Result<()> {
    reset();
    let mut encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    let slot = input(&mut encoder, 17)?;
    encoder.submit(slot, None, true, Instant::now())?;
    encoder.poll()?;
    encoder.cut_limit = Some(4);
    encoder.submit(slot, None, false, Instant::now())?;
    state(|d| d.reject_submit = true);
    let output = encoder.poll()?;
    assert_eq!(output.len(), 1);
    assert!(!output[0].idr && !output[0].reencoded);
    assert!(encoder.reencode_failure.take().is_some());
    assert_eq!(encoder.reencoded_cuts, 0);
    // The rejected keyframe left nothing mapped, and the next frame is one.
    state(|d| {
        assert!(d.mapped.is_empty());
        d.reject_submit = false;
    });
    assert!(encoder.force_idr);
    encoder.submit(slot, None, false, Instant::now())?;
    let output = encoder.poll()?;
    assert!(output[0].idr && !output[0].reencoded);
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn keyframes_past_a_tenth_over_the_target_are_counted() -> Result<()> {
    reset();
    let mut encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    let slot = input(&mut encoder, 17)?;
    encoder.frame_cap_target = Some(7);
    encoder.submit(slot, None, true, Instant::now())?;
    encoder.poll()?;
    assert_eq!(encoder.keyframes_over_cap, 1, "8 bytes against 7");
    encoder.frame_cap_target = Some(8);
    encoder.submit(slot, None, true, Instant::now())?;
    encoder
        .submit(slot, None, false, Instant::now())
        .unwrap_err();
    encoder.poll()?;
    assert_eq!(encoder.keyframes_over_cap, 1);
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn slice_delivery_reads_p_frames_slice_by_slice_in_subframe_mode() -> Result<()> {
    reset();
    state(|d| {
        d.asynchronous = true;
        d.picture = 24;
    });
    let hevc = Negotiated {
        codec: 1,
        ..Default::default()
    };
    let mut encoder = session_with(&hevc, ApiVersion(13, 0), "video_slice_delivery = 2\n")?;
    assert!(encoder.subframe);
    let (parameters, config) = initialized();
    // Subframe readback runs only synchronously, and HEVC never splits it.
    assert_eq!(parameters.enableEncodeAsync, 0);
    assert_eq!(parameters.reportSliceOffsets(), 1);
    assert_eq!(parameters.enableSubFrameWrite(), 1);
    assert_eq!(parameters.splitEncodeMode(), SPLIT_DISABLED);
    // SAFETY: An HEVC session fills this union member.
    let codec = unsafe { config.encodeCodecConfig.hevcConfig };
    assert_eq!((codec.sliceMode, codec.sliceModeData), (3, 2));
    assert_eq!(encoder.slice_offsets.len(), 120 * 68);
    let slot = input(&mut encoder, 17)?;
    // A keyframe waits for all of its slices and leaves whole.
    state(|d| d.partial.push_back((1, 12)));
    encoder.submit(slot, None, true, Instant::now())?;
    assert!(encoder.poll()?.is_empty());
    let output = encoder.poll()?;
    assert_eq!(output.len(), 1);
    assert_eq!(output[0].unit, crate::encoder::Unit::Whole);
    assert!(output[0].idr);
    assert_eq!(output[0].bytes.len(), 24);
    // A P-frame leaves as NVENC writes it: nothing yet, the first of two
    // slices written, the rest.
    state(|d| d.partial.extend([(0, 0), (2, 20)]));
    encoder.submit(slot, None, false, Instant::now())?;
    assert!(encoder.poll()?.is_empty());
    let first = encoder.poll()?;
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].unit, crate::encoder::Unit::Slice { index: 0 });
    assert!(!first[0].idr);
    assert!(first[0].frame_cap_bytes.is_some());
    assert_eq!(first[0].bytes[..8], 2u64.to_le_bytes());
    let last = encoder.poll()?;
    assert_eq!(last.len(), 1);
    assert_eq!(last[0].unit, crate::encoder::Unit::Last { index: 1 });
    assert!(!last[0].idr);
    let mut picture = first[0].bytes.clone();
    picture.extend_from_slice(&last[0].bytes);
    assert_eq!(picture.len(), 24);
    assert_eq!(picture[8..], (8..24).map(|i| i as u8).collect::<Vec<_>>());
    // Slices already written leave together when the poll comes late; the
    // newest waits for the next poll.
    state(|d| d.partial.push_back((5, 20)));
    encoder.submit(slot, None, false, Instant::now())?;
    assert_eq!(encoder.poll()?[0].bytes.len(), 16);
    assert_eq!(encoder.poll()?[0].bytes.len(), 8);
    // A single slice written may be the frame's last, so it waits.
    state(|d| d.partial.push_back((1, 10)));
    encoder.submit(slot, None, false, Instant::now())?;
    assert!(encoder.poll()?.is_empty());
    assert_eq!(encoder.poll()?[0].unit, crate::encoder::Unit::Whole);
    // A frame done at its first poll leaves whole, with full FEC.
    encoder.submit(slot, None, false, Instant::now())?;
    let output = encoder.poll()?;
    assert_eq!(output[0].unit, crate::encoder::Unit::Whole);
    // A started frame ends as it began once slices are joined; the next is
    // waited for and leaves whole.
    state(|d| d.partial.push_back((2, 10)));
    encoder.submit(slot, None, false, Instant::now())?;
    assert_eq!(
        encoder.poll()?[0].unit,
        crate::encoder::Unit::Slice { index: 0 }
    );
    encoder.slices_joined = true;
    assert_eq!(
        encoder.poll()?[0].unit,
        crate::encoder::Unit::Last { index: 1 }
    );
    state(|d| d.partial.push_back((1, 10)));
    encoder.submit(slot, None, false, Instant::now())?;
    assert!(encoder.poll()?.is_empty());
    let output = encoder.poll()?;
    assert_eq!(output[0].unit, crate::encoder::Unit::Whole);
    assert_eq!(output[0].bytes.len(), 24);
    // As on AMF, slice delivery never encodes a scene cut again.
    encoder.slices_joined = false;
    encoder.cut_limit = Some(4);
    encoder.submit(slot, None, false, Instant::now())?;
    assert!(!encoder.poll()?[0].reencoded);
    assert_eq!(encoder.reencoded_cuts, 0);
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn slice_output_that_shrinks_fails_and_a_stuck_frame_times_out() -> Result<()> {
    reset();
    state(|d| d.picture = 24);
    let mut encoder = session_with(
        &Negotiated::default(),
        ApiVersion(13, 0),
        "video_slice_delivery = 4\n",
    )?;
    // SAFETY: An H.264 session fills this union member.
    let codec = unsafe { initialized().1.encodeCodecConfig.h264Config };
    assert_eq!((codec.sliceMode, codec.sliceModeData), (3, 4));
    let slot = input(&mut encoder, 17)?;
    encoder.submit(slot, None, true, Instant::now())?;
    encoder.poll()?;
    state(|d| d.partial.extend([(2, 12), (2, 4)]));
    encoder.submit(slot, None, false, Instant::now())?;
    assert_eq!(encoder.poll()?[0].bytes.len(), 6);
    let error = encoder.poll().err().unwrap();
    assert!(
        format!("{error:#}").contains("slice output size"),
        "{error:#}"
    );
    // The bytes already sent stay sent; the frame is still in flight.
    encoder.pending.front_mut().unwrap().submitted = Instant::now() - COMPLETION_TIMEOUT * 2;
    state(|d| d.partial.push_back((3, 18)));
    let error = encoder.poll().err().unwrap();
    assert!(format!("{error:#}").contains("timed out"), "{error:#}");
    encoder.pending.front_mut().unwrap().submitted = Instant::now();
    encoder.poll()?;
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn a_subframe_frame_ends_at_the_drivers_completion_and_bad_offsets_hold_it() -> Result<()> {
    reset();
    state(|d| d.picture = 24);
    let mut encoder = session_with(
        &Negotiated::default(),
        ApiVersion(13, 0),
        "video_slice_delivery = 4\n",
    )?;
    let slot = input(&mut encoder, 17)?;
    encoder.submit(slot, None, true, Instant::now())?;
    encoder.poll()?;
    // Every slice and every byte reported before the driver says the
    // picture is done: the slices but the newest leave early, and the last
    // unit carries the newest at the completion.
    state(|d| d.partial.push_back((4, 24)));
    encoder.submit(slot, None, false, Instant::now())?;
    let first = encoder.poll()?;
    assert_eq!(first[0].unit, crate::encoder::Unit::Slice { index: 0 });
    assert_eq!(first[0].bytes.len(), 18);
    let last = encoder.poll()?;
    assert_eq!(last[0].unit, crate::encoder::Unit::Last { index: 1 });
    assert_eq!(last[0].bytes.len(), 6);
    // Slice offsets past the bytes written send nothing early, once warned.
    state(|d| {
        d.bad_offsets = true;
        d.partial.push_back((2, 12));
    });
    encoder.submit(slot, None, false, Instant::now())?;
    assert!(encoder.poll()?.is_empty());
    assert!(encoder.offsets_warned);
    let output = encoder.poll()?;
    assert_eq!(output[0].unit, crate::encoder::Unit::Whole);
    assert_eq!(output[0].bytes.len(), 24);
    assert_eq!(encoder.subframe_traced, 3);
    let lock = NV_ENC_LOCK_BITSTREAM {
        numSlices: 3,
        bitstreamSizeInBytes: 30,
        ..Default::default()
    };
    assert!(slices_consistent(&lock, &[0, 10, 20, 0]));
    assert!(!slices_consistent(&lock, &[0, 10, 10, 0]));
    assert!(!slices_consistent(&lock, &[0, 10, 30, 0]));
    assert!(!slices_consistent(&lock, &[0, 10]));
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn without_subframe_readback_slice_delivery_stays_off_and_events_stay() -> Result<()> {
    reset();
    state(|d| {
        d.asynchronous = true;
        d.no_subframe = true;
    });
    let stream = Negotiated {
        intra_refresh: true,
        ..Default::default()
    };
    let encoder = session_with(&stream, ApiVersion(13, 0), "video_slice_delivery = 2\n")?;
    assert!(!encoder.subframe);
    let (parameters, config) = initialized();
    assert_eq!(parameters.enableEncodeAsync, 1);
    assert_eq!(parameters.enableSubFrameWrite(), 0);
    assert_eq!(parameters.reportSliceOffsets(), 0);
    // SAFETY: An H.264 session fills this union member.
    let codec = unsafe { config.encodeCodecConfig.h264Config };
    assert_eq!(
        codec.enableIntraRefresh(),
        1,
        "the client's intra refresh stays"
    );
    drop(encoder);
    // With subframe readback, slice delivery takes intra refresh's place, as
    // on AMF.
    reset();
    let encoder = session_with(&stream, ApiVersion(13, 0), "video_slice_delivery = 2\n")?;
    assert!(encoder.subframe);
    // SAFETY: An H.264 session fills this union member.
    let codec = unsafe { initialized().1.encodeCodecConfig.h264Config };
    assert_eq!(codec.enableIntraRefresh(), 0);
    drop(encoder);
    // HEVC split encoding on several engines keeps the frame budget, and
    // cannot run beside subframe readback; H.264 never splits.
    let hevc = Negotiated {
        codec: 1,
        ..Default::default()
    };
    reset();
    state(|d| d.engines = 2);
    let encoder = session_with(
        &hevc,
        ApiVersion(13, 0),
        "video_slice_delivery = 2\nnvenc_split_encode = forced\n",
    )?;
    assert!(!encoder.subframe);
    assert_eq!(initialized().0.splitEncodeMode(), 1);
    drop(encoder);
    // `auto` below the implicit split height never splits: slices leave.
    reset();
    state(|d| d.engines = 2);
    let encoder = session_with(&hevc, ApiVersion(13, 0), "video_slice_delivery = 2\n")?;
    assert!(encoder.subframe);
    assert_eq!(initialized().0.splitEncodeMode(), SPLIT_DISABLED);
    drop(encoder);
    reset();
    state(|d| d.engines = 2);
    let encoder = session_with(
        &hevc,
        ApiVersion(13, 0),
        "video_slice_delivery = 2\nnvenc_split_encode = disabled\n",
    )?;
    assert!(encoder.subframe);
    drop(encoder);
    // API 12.1 has no `splitEncodeMode` written by the session, so the
    // driver's default applies: slices stay off for `auto` as before, and
    // only disabled split encoding sends them.
    for (setting, subframe) in [("", false), ("nvenc_split_encode = disabled\n", true)] {
        reset();
        state(|d| d.engines = 2);
        let encoder = session_with(
            &hevc,
            ApiVersion(12, 1),
            &format!("video_slice_delivery = 2\n{setting}"),
        )?;
        assert_eq!(encoder.subframe, subframe, "API 12.1 {setting:?}");
        drop(encoder);
    }
    reset();
    state(|d| d.engines = 2);
    let encoder = session_with(
        &Negotiated::default(),
        ApiVersion(13, 0),
        "video_slice_delivery = 2\n",
    )?;
    assert!(encoder.subframe);
    drop(encoder);
    // Off by default at this size: nothing changes.
    reset();
    state(|d| d.asynchronous = true);
    let encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    assert!(!encoder.subframe);
    assert_eq!(initialized().0.enableEncodeAsync, 1);
    drop(encoder);
    assert_clean();
    Ok(())
}
