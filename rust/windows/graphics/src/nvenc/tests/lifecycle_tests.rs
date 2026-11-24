//! Fake-driver tests for NVENC API negotiation, session limits, teardown,
//! async completion events, RFI draining, and bitrate reconfiguration failures.

use super::*;

#[test]
fn rejected_versions_destroy_partial_sessions_and_keep_legacy_ten_bit_fields() -> Result<()> {
    reset();
    state(|d| {
        d.reject_open = Some(ApiVersion(13, 0));
        d.reject_initialize = Some(ApiVersion(12, 2));
        d.preset_fallback = true;
    });
    let config = Negotiated {
        codec: 1,
        hdr: true,
        rate_millihz: 119880,
        ..Default::default()
    };
    let encoder = session(&config, ApiVersion(13, 0))?;
    assert_eq!(encoder.api, ApiVersion(12, 1));
    // SAFETY: The session configured HEVC; the union member holds only integers and pointers.
    let format = unsafe { encoder.config.encodeCodecConfig.hevcConfig };
    assert_eq!(format.reserved3(), 2);
    assert_eq!(format.inputBitDepth, 0);
    assert_eq!(format.outputBitDepth, 0);
    assert_eq!(format.hevcVUIParameters.colourPrimaries, 9);
    assert_eq!(format.hevcVUIParameters.transferCharacteristics, 16);
    assert_eq!(encoder.initialize.frameRateNum, 119880);
    assert_eq!(encoder.initialize.frameRateDen, 1000);
    assert_eq!(encoder.config.frameIntervalP, 1);
    assert_eq!(encoder.config.rcParams.enableLookahead(), 0);
    state(|d| {
        assert_eq!(
            d.opened,
            vec![ApiVersion(13, 0), ApiVersion(12, 2), ApiVersion(12, 1)]
        );
        assert_eq!(d.destroyed, vec![ApiVersion(13, 0), ApiVersion(12, 2)]);
    });
    drop(encoder);
    assert_clean();
    reset();
    let av1 = session(
        &Negotiated {
            codec: 2,
            hdr: true,
            ..Default::default()
        },
        ApiVersion(12, 1),
    )?;
    // SAFETY: The session configured AV1; the union member holds only integers and pointers.
    let format = unsafe { av1.config.encodeCodecConfig.av1Config };
    assert_eq!(format.enableTemporalSVC(), 1);
    assert_eq!(format.reserved4(), 1);
    assert_eq!(format.inputBitDepth, 0);
    assert_eq!(format.outputBitDepth, 0);
    drop(av1);
    assert_clean();
    Ok(())
}
#[test]
fn an_open_past_the_session_limit_is_named_and_no_other_api_is_tried() {
    for status in [
        _NVENCSTATUS_NV_ENC_ERR_OUT_OF_MEMORY,
        _NVENCSTATUS_NV_ENC_ERR_INCOMPATIBLE_CLIENT_KEY,
    ] {
        reset();
        state(|d| d.open_status = Some(status));
        let Err(error) = session(&Negotiated::default(), ApiVersion(13, 0)) else {
            panic!("a session opened past the limit");
        };
        assert!(session_limit(&error));
        let message = format!("{error:#}");
        assert!(
            message.starts_with("NVENC session limit reached"),
            "{message}"
        );
        assert!(message.contains("OBS, ShadowPlay or Discord"), "{message}");
        // Out of memory may also be the GPU's memory.
        assert_eq!(
            message.contains("GPU memory exhausted"),
            status == _NVENCSTATUS_NV_ENC_ERR_OUT_OF_MEMORY,
            "{message}"
        );
        state(|d| assert_eq!(d.opened, vec![ApiVersion(13, 0)]));
        assert_clean();
    }
    reset();
    state(|d| d.open_status = Some(_NVENCSTATUS_NV_ENC_ERR_INVALID_DEVICE));
    let Err(error) = session(&Negotiated::default(), ApiVersion(13, 0)) else {
        panic!("a session opened on a rejected device");
    };
    assert!(!session_limit(&error));
    assert_clean();
}
#[test]
fn a_closed_session_releases_everything_and_refuses_more_work() -> Result<()> {
    reset();
    state(|d| d.asynchronous = true);
    let mut encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    let slot = input(&mut encoder, 17)?;
    encoder.submit(slot, None, true, Instant::now())?;
    // Closing collects the frame in flight, then destroys the session, so
    // another session can open in its place (the focus probe).
    encoder.close();
    assert_clean();
    let error = encoder
        .submit(slot, None, false, Instant::now())
        .unwrap_err();
    assert!(error.to_string().contains("closed"), "{error}");
    assert!(encoder.bitrate(10000).is_err());
    let next = session(&Negotiated::default(), ApiVersion(13, 0))?;
    drop(encoder);
    state(|d| assert_eq!(d.destroyed.len(), 1));
    drop(next);
    state(|d| assert_eq!(d.destroyed.len(), 2));
    assert_clean();
    Ok(())
}
#[test]
fn completed_output_retains_inputs_and_async_event_survives_a_busy_lock() -> Result<()> {
    reset();
    state(|d| {
        d.asynchronous = true;
        d.need_more_input = true;
        d.lock_busy = 1;
    });
    let mut encoder = session(&Negotiated::default(), ApiVersion(11, 0))?;
    let slot = input(&mut encoder, 17)?;
    let presentation = Instant::now() - Duration::from_millis(10);
    encoder.submit(slot, None, true, presentation)?;
    assert!(encoder.poll()?.is_empty());
    assert_eq!(encoder.pending.len(), 1);
    state(|d| assert!(d.mapped.contains(&17)));
    assert!(encoder.submit(slot, None, false, Instant::now()).is_err());
    let result = encoder.poll()?;
    assert_eq!(result.len(), 1);
    assert!(result[0].idr);
    assert!(!result[0].after_invalidation);
    assert_eq!(result[0].bytes, 1u64.to_le_bytes());
    assert!(result[0].latency.is_some());
    assert_eq!(result[0].presentation, Some(presentation));
    state(|d| assert!(d.mapped.is_empty()));
    encoder.submit(slot, None, false, Instant::now())?;
    assert_eq!(encoder.poll()?[0].bytes, 2u64.to_le_bytes());
    drop(encoder);
    assert_clean();
    state(|d| {
        let position = |name| d.calls.iter().rposition(|v| *v == name).unwrap();
        assert!(position("unlock output") < position("unmap input"));
        assert!(position("unmap input") < position("unregister event"));
        assert!(position("destroy output") < position("unregister input"));
        assert!(position("unregister input") < position("destroy session"));
    });
    Ok(())
}
#[test]
fn timed_out_submission_retains_resources_until_native_session_destruction() -> Result<()> {
    reset();
    state(|d| {
        d.asynchronous = true;
        d.lock_busy = usize::MAX;
        d.destroy_busy = true;
    });
    let mut encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    let slot = input(&mut encoder, 17)?;
    encoder.submit(slot, None, true, Instant::now())?;
    encoder.pending.front_mut().unwrap().submitted =
        Instant::now() - COMPLETION_TIMEOUT - Duration::from_millis(1);
    assert!(encoder.poll().is_err());
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn partial_output_allocation_is_released_and_reference_caps_control_advertisement() -> Result<()> {
    reset();
    state(|d| d.reject_output = true);
    let mut encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    assert!(input(&mut encoder, 17).is_err());
    // The failed slot is gone and its registration released, so the
    // texture registers afresh and encodes once the driver allows it.
    assert!(encoder.slots.is_empty());
    state(|d| {
        assert!(d.resources.is_empty() && d.outputs.is_empty());
        d.reject_output = false;
    });
    let slot = input(&mut encoder, 17)?;
    assert_eq!(slot, 0);
    encoder.submit(slot, None, true, Instant::now())?;
    assert!(encoder.poll()?[0].idr);
    drop(encoder);
    assert_clean();
    reset();
    state(|d| d.no_multiple_refs = true);
    let encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    assert_eq!(encoder.retained, 1);
    assert!(!encoder.invalidation);
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn loss_feedback_drains_pending_frames_and_only_confirms_successful_invalidation() -> Result<()> {
    reset();
    let mut encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    let slot = input(&mut encoder, 17)?;
    for frame in 1..=12 {
        encoder.submit(slot, None, frame == 1, Instant::now())?;
        encoder.poll()?;
    }
    // Feedback can arrive while a later picture is still pending. Drain into
    // the output queue; the feedback range includes all dependent pictures.
    encoder.submit(slot, None, false, Instant::now())?;
    assert!(encoder.invalidate(11, 11)?);
    state(|d| assert_eq!(d.invalidated, vec![11, 12, 13]));
    assert_eq!(encoder.poll()?[0].bytes, 13u64.to_le_bytes());
    encoder.submit(slot, None, false, Instant::now())?;
    assert!(encoder.poll()?[0].after_invalidation);
    encoder.submit(slot, None, false, Instant::now())?;
    assert!(!encoder.poll()?[0].after_invalidation);
    assert!(encoder.invalidate(11, 12)?);
    state(|d| assert_eq!(d.invalidated.len(), 3));
    state(|d| d.reject_invalidate = Some(15));
    assert!(encoder.invalidate(14, 14).is_err());
    encoder.submit(slot, None, false, Instant::now())?;
    assert!(!encoder.poll()?[0].after_invalidation);
    encoder.submit(slot, None, true, Instant::now())?;
    assert!(encoder.poll()?[0].idr);
    assert!(encoder.invalidate(14, 15)?); // Old GOP feedback is already resolved.
    assert!(!encoder.invalidate(17, 17)?); // A lost current IDR needs another IDR.
    drop(encoder);
    assert_clean();
    Ok(())
}
/// An API fallback must say what the older driver API costs the stream: the
/// retry used to drop HDR10 metadata or split encoding with only a log line.
#[test]
fn an_api_fallback_says_which_features_the_older_api_loses() {
    let hdr = Negotiated {
        codec: 1,
        hdr: true,
        ..Default::default()
    };
    let tuning = Tuning::new(&butterpollo_config::config::Config::default(), &hdr).unwrap();
    let warning = api_fallback_warning(
        &["13.0: NV_ENC_ERR_UNSUPPORTED_PARAM".into()],
        ApiVersion(12, 2),
        &hdr,
        &tuning,
    );
    assert!(warning.contains("HDR10"), "{warning}");
    assert!(warning.contains("control channel"), "{warning}");
    // API 13 keeps its in-bitstream metadata: nothing is lost there.
    let mut split = tuning.clone();
    split.split = 2;
    let warning = api_fallback_warning(&["13.0: refused".into()], ApiVersion(13, 0), &hdr, &split);
    assert!(!warning.contains("HDR10"), "{warning}");
    // Pre-12.2 has no split-frame encoding.
    let warning = api_fallback_warning(&["13.0: refused".into()], ApiVersion(12, 0), &hdr, &split);
    assert!(warning.contains("split-frame encoding"), "{warning}");
    // H.264 never splits, whatever the API.
    let h264 = Negotiated {
        codec: 0,
        ..Default::default()
    };
    let warning = api_fallback_warning(&["13.0: refused".into()], ApiVersion(11, 0), &h264, &split);
    assert!(!warning.contains("split-frame"), "{warning}");
}

#[test]
fn failed_bitrate_reconfigure_keeps_the_config_and_a_rise_needs_no_keyframe() -> Result<()> {
    reset();
    let stream = Negotiated {
        codec: 1,
        hdr: true,
        ..Default::default()
    };
    let mut encoder = session(&stream, ApiVersion(13, 0))?;
    let before = encoder.config.rcParams.averageBitRate;
    state(|d| d.reject_reconfigure = true);
    assert!(encoder.bitrate(40000).is_err());
    assert_eq!(encoder.config.rcParams.averageBitRate, before);
    assert_eq!(encoder.stream.bitrate_kbps, 20000);
    state(|d| d.reject_reconfigure = false);
    encoder.bitrate(40000)?;
    assert_eq!(encoder.config.rcParams.averageBitRate, 40_000_000);
    assert_eq!(
        encoder.initialize.encodeConfig,
        encoder.config.as_mut() as *mut _
    );
    state(|d| {
        let (parameters, config) = d.reconfigured.as_ref().unwrap();
        // A driver with dynamic bitrate changes takes a rise in place, as AMF.
        assert_eq!(parameters.forceIDR(), 0);
        assert_eq!(parameters.resetEncoder(), 0);
        assert_eq!(config.rcParams.maxBitRate, 40_000_000);
        assert_eq!(
            // SAFETY: HEVC was configured; the union member is plain integers and pointers.
            unsafe { config.encodeCodecConfig.hevcConfig }
                .hevcVUIParameters
                .transferCharacteristics,
            16
        );
    });
    assert!(!encoder.force_idr);
    let slot = input(&mut encoder, 17)?;
    encoder.submit(slot, None, true, Instant::now())?;
    assert!(encoder.poll()?[0].idr);
    encoder.submit(slot, None, false, Instant::now())?;
    assert!(!encoder.poll()?[0].idr, "a bitrate rise forces no keyframe");
    encoder.bitrate(10000)?;
    assert!(!encoder.force_idr);
    assert!(encoder.bitrate(0).is_err());
    // The reconfigure ceiling is the stream's own: 2 Gbit/s, not 800 Mbit/s.
    encoder.bitrate(1_500_000)?;
    assert_eq!(encoder.stream.bitrate_kbps, 1_500_000);
    assert_eq!(encoder.config.rcParams.averageBitRate, 1_500_000_000);
    encoder.bitrate(2_500_000)?;
    assert_eq!(
        encoder.stream.bitrate_kbps,
        butterpollo_media_policy::rtsp::MAX_BITRATE_KBPS
    );
    assert_eq!(encoder.config.rcParams.averageBitRate, 2_000_000_000);
    // The cap is applied before the stored rate is compared: a request above
    // the ceiling at the stream's current rate changes nothing.
    state(|d| d.reconfigured = None);
    encoder.bitrate(butterpollo_media_policy::rtsp::MAX_BITRATE_KBPS + 500_000)?;
    assert!(state(|d| d.reconfigured.is_none()));
    drop(encoder);
    assert_clean();
    Ok(())
}
