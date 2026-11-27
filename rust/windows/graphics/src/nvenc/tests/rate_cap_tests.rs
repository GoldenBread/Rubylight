//! Fake-driver tests for NVENC frame caps, dynamic bitrate changes, startup focus checks,
//! slot registration limits, deferred cleanup retries, and oversize frame warnings.

use super::*;

#[test]
fn the_frame_cap_lowers_the_vbv_under_it_and_a_steady_link_never_reconfigures() -> Result<()> {
    reset();
    let mut encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    let ceiling = encoder.config.rcParams.vbvBufferSize;
    assert!(ceiling > 0);
    assert_eq!(encoder.vbv_ceiling, ceiling);
    let frame = u64::from(ceiling).div_ceil(8);
    assert_eq!(encoder.frame_cap(), Some(frame));
    // A cap over the one-frame VBV is already held: no reconfigure, and the
    // cap is the bound frames report.
    assert_eq!(encoder.set_frame_cap(frame * 3)?, Some(frame * 3));
    state(|d| assert!(d.reconfigured.is_none()));
    // A cap under it lowers the VBV, without a reset or a keyframe.
    assert_eq!(encoder.set_frame_cap(frame / 2)?, Some(frame / 2));
    assert_eq!(
        u64::from(encoder.config.rcParams.vbvBufferSize),
        frame / 2 * 8
    );
    assert_eq!(encoder.config.rcParams.averageBitRate, 20_000_000);
    state(|d| {
        let (parameters, config) = d.reconfigured.take().unwrap();
        assert_eq!(parameters.resetEncoder(), 0);
        assert_eq!(parameters.forceIDR(), 0);
        assert_eq!(u64::from(config.rcParams.vbvBufferSize), frame / 2 * 8);
    });
    assert!(!encoder.force_idr);
    // Jitter within a tenth leaves it.
    encoder.set_frame_cap(frame / 2 + frame / 40)?;
    encoder.set_frame_cap(frame / 2 - frame / 40)?;
    state(|d| assert!(d.reconfigured.is_none()));
    // The submitted frame carries the cap and the host's target.
    let slot = input(&mut encoder, 17)?;
    encoder.submit(slot, None, true, Instant::now())?;
    let output = encoder.poll()?;
    assert_eq!(output[0].frame_cap_target, Some(frame / 2 - frame / 40));
    assert_eq!(output[0].frame_cap_bytes, Some(frame / 2));
    // Back over the ceiling: the VBV returns to it at once.
    encoder.set_frame_cap(frame * 2)?;
    assert_eq!(encoder.config.rcParams.vbvBufferSize, ceiling);
    state(|d| assert!(d.reconfigured.take().is_some()));
    // A bitrate change moves the ceiling with the rate.
    encoder.bitrate(10000)?;
    assert_eq!(encoder.vbv_ceiling, ceiling / 2);
    assert_eq!(encoder.config.rcParams.vbvBufferSize, ceiling / 2);
    // A refused change is not retried for the same cap.
    state(|d| d.reject_reconfigure = true);
    assert!(encoder.set_frame_cap(frame / 8).is_err());
    state(|d| d.reconfigured = None);
    assert!(encoder.set_frame_cap(frame / 8).is_ok());
    state(|d| assert!(d.reconfigured.is_none()));
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn a_five_frame_vbv_is_the_bound_until_the_host_caps_under_it() -> Result<()> {
    reset();
    let one = session(&Negotiated::default(), ApiVersion(13, 0))?;
    let single = u64::from(one.config.rcParams.vbvBufferSize);
    drop(one);
    reset();
    // "Reference quality" sets nvenc_vbv_increase = 400: five frames.
    let mut encoder = session_with(
        &Negotiated::default(),
        ApiVersion(13, 0),
        "nvenc_vbv_increase = 400\n",
    )?;
    let ceiling = encoder.config.rcParams.vbvBufferSize;
    assert!(u64::from(ceiling).abs_diff(single * 5) <= 5, "{ceiling}");
    let five = u64::from(ceiling).div_ceil(8);
    assert_eq!(encoder.frame_cap(), Some(five));
    // A cap over the five frames is the reported bound without a change.
    assert_eq!(encoder.set_frame_cap(five * 2)?, Some(five * 2));
    state(|d| assert!(d.reconfigured.is_none()));
    // A link that carries two frames' worth lowers the VBV to it.
    let link = single.div_ceil(8) * 2;
    assert_eq!(encoder.set_frame_cap(link)?, Some(link));
    assert_eq!(u64::from(encoder.config.rcParams.vbvBufferSize), link * 8);
    state(|d| assert!(d.reconfigured.take().is_some()));
    // Back over it: the five-frame VBV returns at once.
    assert_eq!(encoder.set_frame_cap(five + 1)?, Some(five + 1));
    assert_eq!(encoder.config.rcParams.vbvBufferSize, ceiling);
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn a_frame_cap_and_bitrate_changes_apply_with_frames_in_flight() -> Result<()> {
    reset();
    state(|d| d.asynchronous = true);
    let mut encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    let frame = encoder.frame_cap().unwrap();
    let slot = input(&mut encoder, 17)?;
    encoder.submit(slot, None, true, Instant::now())?;
    // Lowering the VBV adds no wait for the frame in flight.
    encoder.set_frame_cap(frame / 2)?;
    state(|d| assert!(d.reconfigured.take().is_some()));
    assert_eq!(encoder.pending.len(), 1);
    // Neither does a lower bitrate, nor a higher one: the driver changes
    // the rate in place without a reset or keyframe.
    encoder.bitrate(10000)?;
    assert_eq!(encoder.pending.len(), 1);
    encoder.bitrate(40000)?;
    assert_eq!(encoder.pending.len(), 1);
    state(|d| assert_eq!(d.reconfigured.take().unwrap().0.resetEncoder(), 0));
    assert!(!encoder.force_idr);
    assert_eq!(encoder.poll()?.len(), 1);
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn a_bitrate_change_refused_in_place_applies_with_a_reset() -> Result<()> {
    reset();
    state(|d| d.reject_in_place = true);
    let mut encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    encoder.bitrate(40000)?;
    assert_eq!(encoder.config.rcParams.averageBitRate, 40_000_000);
    assert_eq!(encoder.stream.bitrate_kbps, 40000);
    state(|d| {
        let (parameters, _) = d.reconfigured.take().unwrap();
        assert_eq!((parameters.resetEncoder(), parameters.forceIDR()), (1, 1));
    });
    assert!(encoder.force_idr);
    let slot = input(&mut encoder, 17)?;
    encoder.submit(slot, None, false, Instant::now())?;
    assert!(encoder.poll()?[0].idr);
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn the_focus_probe_refuses_a_session_a_stream_has_set_up() -> Result<()> {
    reset();
    let mut encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    encoder.startup_only()?;
    let refused = |encoder: &Session| {
        let error = encoder.startup_only().unwrap_err().to_string();
        assert!(error.contains("startup check"), "{error}");
    };
    encoder.cut_limit = Some(4);
    refused(&encoder);
    encoder.cut_limit = None;
    encoder.set_frame_cap(1000)?;
    refused(&encoder);
    encoder.frame_cap_target = None;
    encoder.slices_joined = true;
    refused(&encoder);
    encoder.slices_joined = false;
    encoder.qp_map = true;
    refused(&encoder);
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn inputs_the_driver_keeps_do_not_use_up_the_registration_limit() -> Result<()> {
    reset();
    state(|d| {
        d.reject_output = true;
        d.reject_unregister = true;
    });
    let mut encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    for key in 0..MAX_DEFERRED {
        let error = input(&mut encoder, 100 + key).unwrap_err();
        assert!(!format!("{error:#}").contains("limit"), "{error:#}");
    }
    assert!(encoder.slots.iter().all(|slot| slot.input.key == 0));
    // The driver lets go: the next registration releases them and reuses the room.
    state(|d| {
        d.reject_output = false;
        d.reject_unregister = false;
    });
    let slot = input(&mut encoder, 17)?;
    encoder.submit(slot, None, true, Instant::now())?;
    assert!(encoder.poll()?[0].idr);
    assert_eq!(encoder.slots.len(), 1);
    drop(encoder);
    assert_clean();
    Ok(())
}
/// Counts the completion events slots still hold open.
fn open_events(encoder: &Session) -> usize {
    encoder
        .slots
        .iter()
        .filter(|slot| slot.event.is_some())
        .count()
}
#[test]
fn a_persistent_cleanup_fault_keeps_the_deferred_slots_and_their_events_bounded() -> Result<()> {
    reset();
    state(|d| {
        d.asynchronous = true;
        d.reject_register_event = true;
        d.reject_unregister = true;
    });
    let mut encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    for key in 0..MAX_DEFERRED {
        let error = input(&mut encoder, 100 + key).unwrap_err();
        assert!(
            format!("{error:#}").contains("completion event"),
            "{error:#}"
        );
    }
    assert_eq!(open_events(&encoder), MAX_DEFERRED);
    // Past the bound registration fails visibly, without another slot or handle.
    for key in 0..4 * MAX_DEFERRED {
        let error = input(&mut encoder, 200 + key).unwrap_err();
        assert!(format!("{error:#}").contains("keeps failing"), "{error:#}");
    }
    assert_eq!(encoder.slots.len(), MAX_DEFERRED);
    assert_eq!(open_events(&encoder), MAX_DEFERRED);
    // The driver lets go: the retry closes every handle and the list shrinks.
    state(|d| {
        d.reject_register_event = false;
        d.reject_unregister = false;
    });
    let slot = input(&mut encoder, 17)?;
    assert_eq!(encoder.slots.len(), 1);
    assert_eq!(open_events(&encoder), 1);
    assert_eq!(slot, 0);
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn a_fault_that_clears_on_retry_does_not_lengthen_the_slot_list() -> Result<()> {
    reset();
    state(|d| {
        d.asynchronous = true;
        d.reject_register_event = true;
    });
    let mut encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    let live = input_ok(&mut encoder, 17)?;
    for round in 0..4 * MAX_DEFERRED {
        // Only the cleanup fails this round; the next registration retries it.
        state(|d| d.reject_unregister = true);
        input(&mut encoder, 100 + round).unwrap_err();
        state(|d| d.reject_unregister = false);
        assert_eq!(encoder.slots.len(), 2);
        input(&mut encoder, 300 + round).unwrap_err();
        assert_eq!(encoder.slots.len(), 1);
    }
    assert_eq!(open_events(&encoder), 1);
    assert_eq!(live, 0);
    drop(encoder);
    assert_clean();
    Ok(())
}
fn input_ok(encoder: &mut Session, key: usize) -> Result<usize> {
    state(|d| d.reject_register_event = false);
    let slot = input(encoder, key);
    state(|d| d.reject_register_event = true);
    slot
}
#[test]
fn without_dynamic_bitrate_a_change_resets_after_the_frames_in_flight() -> Result<()> {
    reset();
    state(|d| {
        d.asynchronous = true;
        d.no_dynamic_bitrate = true;
    });
    let mut encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    let slot = input(&mut encoder, 17)?;
    encoder.submit(slot, None, true, Instant::now())?;
    // A lower rate resets too: the driver cannot take either in place.
    encoder.bitrate(10000)?;
    assert!(encoder.pending.is_empty());
    state(|d| {
        let (parameters, _) = d.reconfigured.take().unwrap();
        assert_eq!((parameters.resetEncoder(), parameters.forceIDR()), (1, 1));
    });
    assert!(encoder.force_idr);
    assert_eq!(encoder.poll()?.len(), 1);
    encoder.submit(slot, None, false, Instant::now())?;
    assert!(encoder.poll()?[0].idr);
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn a_frame_over_twice_its_cap_warns_at_most_every_ten_seconds() -> Result<()> {
    reset();
    let mut encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    let cap = encoder.frame_cap().unwrap() as usize;
    let slot = input(&mut encoder, 17)?;
    state(|d| d.picture = cap * 2);
    encoder.submit(slot, None, true, Instant::now())?;
    assert_eq!(encoder.poll()?[0].bytes.len(), cap * 2);
    assert!(encoder.oversize_warned.is_none());
    state(|d| d.picture = cap * 2 + 1);
    encoder.submit(slot, None, false, Instant::now())?;
    encoder.poll()?;
    let warned = encoder.oversize_warned.unwrap();
    encoder.submit(slot, None, false, Instant::now())?;
    encoder.poll()?;
    assert_eq!(encoder.oversize_warned, Some(warned));
    let earlier = warned - OVERSIZE_WARNING_INTERVAL;
    encoder.oversize_warned = Some(earlier);
    encoder.submit(slot, None, false, Instant::now())?;
    encoder.poll()?;
    assert!(encoder.oversize_warned.unwrap() > earlier);
    drop(encoder);
    assert_clean();
    Ok(())
}

#[test]
fn keyframe_overshoots_tighten_vbv_down_to_two_frames_and_relax_back_to_target() -> Result<()> {
    reset();
    state(|d| d.engines = 2);
    let stream = Negotiated {
        width: 3840,
        height: 2160,
        codec: 1,
        fps: 100,
        bitrate_kbps: 219_500,
        ..Default::default()
    };
    let mut encoder = session_with(&stream, ApiVersion(13, 0), "nvenc_vbv_increase = 400\n")?;
    assert!(!encoder.subframe);
    let slot = input(&mut encoder, 17)?;
    let target = 1_166_893_u64;
    let two_frames = 548_750_u64;

    // Initial automatic cap lowers the 5-frame VBV (~1.37 MB) to `target` (~1.17 MB).
    assert_eq!(
        encoder.set_frame_cap_toward(target, two_frames)?,
        Some(target)
    );
    assert_eq!(u64::from(encoder.config.rcParams.vbvBufferSize), target * 8);
    assert_eq!(
        encoder.config.rcParams.vbvInitialDelay,
        encoder.config.rcParams.vbvBufferSize
    );
    state(|d| d.reconfigured = None);

    // Keyframe 1 overshoots VBV by 1.635x (1,908,217 B > 1.1 * target), as in #59.
    state(|d| d.picture = 1_908_217);
    encoder.submit(slot, None, true, Instant::now())?;
    let first = encoder.poll()?;
    assert_eq!(first[0].frame_cap_target, Some(target));
    assert_eq!(first[0].frame_cap_bytes, Some(target));
    assert_eq!(encoder.keyframes_over_cap, 1);
    assert!(encoder.keyframe_cap.scale() < 0.62);

    // Next frame cap update tightens VBV in place (`resetEncoder = 0`) >= `two_frames`.
    let tightened = encoder.set_frame_cap_toward(target, two_frames)?.unwrap();
    assert!(tightened < target && tightened >= two_frames, "{tightened}");
    state(|d| {
        let (params, cfg) = d.reconfigured.take().unwrap();
        assert_eq!((params.resetEncoder(), params.forceIDR()), (0, 0));
        assert_eq!(u64::from(cfg.rcParams.vbvBufferSize), tightened * 8);
        assert_eq!(cfg.rcParams.vbvInitialDelay, cfg.rcParams.vbvBufferSize);
    });

    // Keyframe 2 overshoots the tightened VBV by the same 1.635x factor and now
    // lands within `target` (preserving Moonlight's 4-FEC-block parity).
    state(|d| d.picture = (tightened as f64 * 1.635) as usize);
    encoder.submit(slot, None, true, Instant::now())?;
    let second = encoder.poll()?;
    assert_eq!(second[0].frame_cap_target, Some(target));
    assert_eq!(second[0].frame_cap_bytes, Some(tightened));
    assert_eq!(encoder.keyframes_over_cap, 1);

    // Calm keyframes held to the active VBV relax `KeyframeCapScale` step by step,
    // including the final clamped ~2.1% (<10%) step from 0.9795 back to 1.0.
    let mut steps = 0;
    while encoder.keyframe_cap.scale() < 1. {
        let active_cap = encoder.set_frame_cap_toward(target, two_frames)?.unwrap();
        state(|d| d.picture = active_cap as usize);
        encoder.submit(slot, None, true, Instant::now())?;
        encoder.poll()?;
        steps += 1;
        assert!(steps <= 6, "did not relax in 6 keyframes: {steps}");
    }
    assert!(
        u64::from(
            encoder
                .config
                .rcParams
                .vbvBufferSize
                .abs_diff((target * 8) as u32)
        ) * 10
            <= u64::from(encoder.config.rcParams.vbvBufferSize),
        "penultimate step must sit within 10% of target to test the clamped final step"
    );
    state(|d| d.reconfigured = None);
    assert_eq!(
        encoder.set_frame_cap_toward(target, two_frames)?,
        Some(target)
    );
    assert_eq!(u64::from(encoder.config.rcParams.vbvBufferSize), target * 8);
    assert!(
        state(|d| d.reconfigured.take().is_some()),
        "the clamped final relax step back to target must reconfigure NVENC"
    );

    // Extreme overshoot clamps at `two_frames` even when `0.5 * target` would be smaller.
    let high_floor = 750_000_u64;
    state(|d| d.picture = (target * 4) as usize);
    encoder.submit(slot, None, true, Instant::now())?;
    encoder.poll()?;
    assert_eq!(encoder.keyframe_cap.scale(), KeyframeCapScale::FLOOR);
    assert_eq!(
        encoder.set_frame_cap_toward(target, high_floor)?,
        Some(high_floor)
    );
    assert_eq!(
        u64::from(encoder.config.rcParams.vbvBufferSize),
        high_floor * 8
    );
    drop(encoder);
    assert_clean();
    Ok(())
}

#[test]
fn a_driver_that_ignores_lower_vbv_on_keyframes_restores_target_vbv_and_one_frame_ceiling_stays_inert()
-> Result<()> {
    reset();
    let stream = Negotiated {
        width: 3840,
        height: 2160,
        codec: 2,
        fps: 100,
        bitrate_kbps: 219_500,
        ..Default::default()
    };
    let mut encoder = session_with(&stream, ApiVersion(13, 0), "nvenc_vbv_increase = 400\n")?;
    let slot = input(&mut encoder, 19)?;
    let target = 1_166_893_u64;
    let two_frames = 548_750_u64;
    encoder.set_frame_cap_toward(target, two_frames)?;

    // Two identical 1.91 MB keyframes prove the driver ignored the tightened VBV:
    // scale returns to 1.0 and VBV restores to `target` so inter frames are not squeezed.
    state(|d| d.picture = 1_908_217);
    encoder.submit(slot, None, true, Instant::now())?;
    encoder.poll()?;
    let lowered = encoder.set_frame_cap_toward(target, two_frames)?.unwrap();
    assert!(lowered < target);
    encoder.submit(slot, None, true, Instant::now())?;
    encoder.poll()?;
    assert_eq!(encoder.keyframe_cap.scale(), 1.);
    assert_eq!(
        encoder.set_frame_cap_toward(target, two_frames)?,
        Some(target)
    );
    assert_eq!(u64::from(encoder.config.rcParams.vbvBufferSize), target * 8);
    drop(encoder);
    assert_clean();

    // With default 1-frame VBV (`nvenc_vbv_increase = 0`), `vbv_ceiling` (274,375 B)
    // is already below `two_frames` (548,750 B): VBV stays at `vbv_ceiling` and scale stays 1.
    reset();
    let mut one_frame = session(&stream, ApiVersion(13, 0))?;
    let ceiling = one_frame.vbv_ceiling;
    let slot = input(&mut one_frame, 21)?;
    assert_eq!(
        one_frame.set_frame_cap_toward(target, two_frames)?,
        Some(target)
    );
    state(|d| d.picture = 1_908_217);
    one_frame.submit(slot, None, true, Instant::now())?;
    let out = one_frame.poll()?;
    assert_eq!(out[0].frame_cap_target, Some(target));
    assert_eq!(one_frame.keyframes_over_cap, 1);
    assert_eq!(one_frame.keyframe_cap.scale(), 1.);
    assert_eq!(one_frame.config.rcParams.vbvBufferSize, ceiling);
    drop(one_frame);
    assert_clean();

    // Subframe slice delivery keeps `frame_cap` unscaled so slice block sizing stays fixed.
    reset();
    let h264_4k = Negotiated { codec: 0, ..stream };
    let mut sliced = session_with(
        &h264_4k,
        ApiVersion(13, 0),
        "nvenc_vbv_increase = 400\nvideo_slice_delivery = 4\n",
    )?;
    assert!(sliced.subframe);
    let slot = input(&mut sliced, 22)?;
    assert_eq!(
        sliced.set_frame_cap_toward(target, two_frames)?,
        Some(target)
    );
    state(|d| d.picture = 1_908_217);
    sliced.submit(slot, None, true, Instant::now())?;
    sliced.poll()?;
    assert_eq!(sliced.keyframe_cap.scale(), 1.);
    assert_eq!(
        sliced.set_frame_cap_toward(target, two_frames)?,
        Some(target)
    );
    drop(sliced);
    assert_clean();
    Ok(())
}

#[test]
fn reencoded_scene_cut_overshoot_tightens_vbv_and_coalesces_with_apply_bitrate() -> Result<()> {
    reset();
    state(|d| d.engines = 2);
    let stream = Negotiated {
        width: 3840,
        height: 2160,
        codec: 1,
        fps: 100,
        bitrate_kbps: 219_500,
        ..Default::default()
    };
    let mut encoder = session_with(&stream, ApiVersion(13, 0), "nvenc_vbv_increase = 400\n")?;
    assert!(!encoder.subframe);
    let slot = input(&mut encoder, 23)?;
    let target = 1_166_893_u64;
    let two_frames = 548_750_u64;

    let _ = encoder.queue_frame_cap(target, two_frames);
    encoder.apply_bitrate(219_500, true)??;
    encoder.cut_limit = Some(target);

    // Startup IDR fits.
    state(|d| d.picture = 800_000);
    encoder.submit(slot, None, true, Instant::now())?;
    assert_eq!(encoder.poll()?.len(), 1);
    assert_eq!(encoder.keyframe_cap.scale(), 1.);

    // Oversize P-frame scene cut (> `cut_limit`) re-encodes as IDR; when that IDR
    // also overshoots `1.1 * target`, `keyframe_cap` tightens VBV on the next coalesced apply.
    state(|d| d.picture = 1_800_000);
    encoder.submit(slot, None, false, Instant::now())?;
    let cut = encoder.poll()?;
    assert!(cut[0].idr && cut[0].reencoded);
    assert_eq!(cut[0].frame_cap_target, Some(target));
    assert_eq!(encoder.reencoded_cuts, 1);
    assert!(encoder.keyframe_cap.scale() < 0.70);

    state(|d| d.reconfigure_count = 0);
    let _ = encoder.queue_frame_cap(target, two_frames);
    encoder.apply_bitrate(200_000, true)??;
    state(|d| {
        assert_eq!(d.reconfigure_count, 1);
        let (_, cfg) = d.reconfigured.take().unwrap();
        assert_eq!(cfg.rcParams.averageBitRate, 200_000 * 1000);
        assert!(
            u64::from(cfg.rcParams.vbvBufferSize) < target * 8
                && u64::from(cfg.rcParams.vbvBufferSize) >= two_frames * 8
        );
        assert_eq!(cfg.rcParams.vbvInitialDelay, cfg.rcParams.vbvBufferSize);
    });
    drop(encoder);
    assert_clean();
    Ok(())
}
