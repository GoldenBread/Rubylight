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
