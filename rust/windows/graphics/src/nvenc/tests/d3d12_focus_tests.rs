//! Fake-driver tests for D3D12 NVENC fences, output events, API 12 floor,
//! encoder focus QP delta maps, per-block QP statistics, and focus probe verdicts.

use super::*;

#[test]
fn a_d3d12_session_waits_for_the_conversion_on_the_gpu_and_signals_its_fence() -> Result<()> {
    reset();
    let stream = Negotiated::default();
    let mut encoder = d3d12_session(&stream, ApiVersion(13, 0), "")?;
    let (parameters, _) = initialized();
    // Completion comes from the output fence, not from events.
    assert_eq!(parameters.enableEncodeAsync, 0);
    assert_eq!(parameters.bufferFormat, buffer_format(&stream));
    let slot = d3d12_input(&mut encoder, 17)?;
    // The READBACK buffer is registered as a byte buffer for the bitstream.
    state(|d| {
        assert_eq!(
            d.resources[&(17 | 0x1000)],
            _NV_ENC_BUFFER_FORMAT_NV_ENC_BUFFER_FORMAT_U8
        );
        assert!(d.outputs.is_empty(), "no driver bitstream buffer");
    });
    encoder.submit(slot, converted(5), true, Instant::now())?;
    let output = encoder.poll()?;
    assert_eq!(output.len(), 1);
    assert!(output[0].idr);
    assert_eq!(output[0].bytes[..8], 1u64.to_le_bytes());
    state(|d| assert_eq!(d.fence_points, [(CONVERSION_FENCE, 5, 1)]));
    // Until the driver signals the frame's value, the poll does not lock.
    state(|d| d.hold_fence = true);
    encoder.submit(slot, converted(6), false, Instant::now())?;
    assert!(encoder.poll()?.is_empty());
    state(|d| assert!(!d.calls.ends_with(&["lock output"])));
    state(|d| d.fence = 2);
    assert_eq!(encoder.poll()?.len(), 1);
    // A frame the fence never reaches fails loudly.
    encoder.submit(slot, converted(7), false, Instant::now())?;
    encoder.pending.front_mut().unwrap().submitted = Instant::now() - COMPLETION_TIMEOUT * 2;
    let error = encoder.poll().err().unwrap();
    assert!(format!("{error:#}").contains("timed out"), "{error:#}");
    state(|d| d.fence = 3);
    encoder.pending.front_mut().unwrap().submitted = Instant::now();
    assert_eq!(encoder.poll()?.len(), 1);
    // A D3D12 frame without its conversion's fence, or a D3D11 input, is refused.
    let error = encoder
        .submit(slot, None, false, Instant::now())
        .err()
        .unwrap();
    assert!(
        format!("{error:#}").contains("conversion's fence"),
        "{error:#}"
    );
    let error = input(&mut encoder, 18).err().unwrap();
    assert!(format!("{error:#}").contains("D3D12"), "{error:#}");
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn a_game_holding_the_gpu_is_left_to_the_sessions_stall_policy() -> Result<()> {
    // The session recreates a silent encoder after 250 ms, doubling up to
    // 2 s. NVENC's own deadline at 100 ms came first: game loading on an
    // RTX 5070 recreated the encoder twice in 12 s.
    assert!(
        COMPLETION_TIMEOUT > butterpollo_media_policy::stream_policy::encoder_stall_limit(u32::MAX)
    );
    reset();
    let mut encoder = d3d12_session(&Negotiated::default(), ApiVersion(13, 0), "")?;
    let slot = d3d12_input(&mut encoder, 17)?;
    encoder.submit(slot, converted(1), true, Instant::now())?;
    assert_eq!(encoder.poll()?.len(), 1);
    // The frame's fence is 300 ms late: still waited for, then delivered.
    state(|d| d.hold_fence = true);
    encoder.submit(slot, converted(2), false, Instant::now())?;
    encoder.pending.front_mut().unwrap().submitted = Instant::now() - Duration::from_millis(300);
    assert!(encoder.poll()?.is_empty());
    assert_eq!(encoder.pending.len(), 1);
    state(|d| d.fence = 2);
    assert_eq!(encoder.poll()?.len(), 1);
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn slice_delivery_on_a_d3d12_session_names_the_conversion_not_the_driver() -> Result<()> {
    // A D3D12 session's driver reports no subframe readback, and the host
    // told the user to update the NVIDIA driver.
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("session.log");
    let file = std::fs::File::create(&path)?;
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_target(false)
        .with_max_level(tracing::Level::INFO)
        .with_writer(move || file.try_clone().unwrap())
        .finish();
    tracing::subscriber::with_default(subscriber, || -> Result<()> {
        reset();
        state(|d| d.no_subframe = true);
        let encoder = d3d12_session(
            &Negotiated::default(),
            ApiVersion(13, 0),
            "video_slice_delivery = 2\n",
        )?;
        assert!(!encoder.subframe);
        drop(encoder);
        assert_clean();
        Ok(())
    })?;
    let log = std::fs::read_to_string(path)?;
    assert!(!log.contains("Update the NVIDIA driver"), "{log}");
    assert!(
        log.contains("NVENC on the D3D12 conversion returns whole frames"),
        "{log}"
    );
    Ok(())
}
#[test]
fn the_output_event_is_armed_only_for_an_unwritten_d3d12_frame() -> Result<()> {
    reset();
    let event = crate::timing::Signal::new()?;
    let d3d11 = session(&Negotiated::default(), ApiVersion(13, 0))?;
    assert!(!d3d11.arm_output(&event)?, "a D3D11 session keeps its poll");
    drop(d3d11);
    reset();
    let mut encoder = d3d12_session(&Negotiated::default(), ApiVersion(13, 0), "")?;
    let slot = d3d12_input(&mut encoder, 17)?;
    assert!(!encoder.arm_output(&event)?, "nothing in flight");
    state(|d| d.hold_fence = true);
    encoder.submit(slot, converted(1), true, Instant::now())?;
    event.set()?;
    assert!(encoder.arm_output(&event)?);
    // The arm resets a stale event and asks for the frame's own value.
    let timer = crate::timing::Timer::new()?;
    assert!(!timer.until_or_event(Instant::now() + Duration::from_millis(1), Some(&event))?);
    state(|d| assert_eq!(d.notified, [1]));
    // Later waits for the same frame ask the fence no more; a late
    // notification of an earlier frame is reset rather than ending each wait.
    for _ in 0..10 {
        assert!(encoder.arm_output(&event)?);
    }
    event.set()?;
    assert!(encoder.arm_output(&event)?);
    assert!(!event.is_set());
    state(|d| assert_eq!(d.notified, [1]));
    // A removed device quietly falls back to the poll; collect reports it.
    state(|d| d.removed = true);
    assert!(!encoder.arm_output(&event)?);
    state(|d| d.removed = false);
    // Once the fence passed it, the wait ends at once.
    state(|d| d.fence = 1);
    assert!(encoder.arm_output(&event)?);
    assert!(event.is_set());
    assert_eq!(encoder.poll()?.len(), 1);
    assert!(!encoder.arm_output(&event)?);
    state(|d| assert_eq!(d.notified, [1]));
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn a_d3d12_session_needs_api_12_and_has_no_slices_or_weighted_prediction() -> Result<()> {
    reset();
    let error = d3d12_session(&Negotiated::default(), ApiVersion(11, 0), "")
        .err()
        .unwrap();
    assert!(format!("{error:#}").contains("522.25"), "{error:#}");
    reset();
    let hevc = Negotiated {
        codec: 1,
        ..Default::default()
    };
    let encoder = d3d12_session(
        &hevc,
        ApiVersion(13, 0),
        "video_slice_delivery = 2\nnvenc_weighted_prediction = true\n",
    )?;
    assert!(!encoder.subframe);
    let (parameters, _) = initialized();
    assert_eq!(parameters.enableWeightedPrediction(), 0);
    assert_eq!(parameters.enableSubFrameWrite(), 0);
    drop(encoder);
    assert_clean();
    // Each D3D12 candidate is API 12.0 or newer.
    reset();
    state(|d| d.reject_open = Some(ApiVersion(12, 0)));
    assert!(d3d12_session(&Negotiated::default(), ApiVersion(12, 0), "").is_err());
    state(|d| assert_eq!(d.opened, [ApiVersion(12, 0)]));
    assert_clean();
    Ok(())
}
#[test]
fn a_focus_map_rides_on_inter_frames_and_turns_on_without_a_reset() -> Result<()> {
    reset();
    let mut encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    // SAFETY: Initialization filled the rate control.
    assert_eq!(
        initialized().1.rcParams.qpMapMode,
        _NV_ENC_QP_MAP_MODE_NV_ENC_QP_MAP_DISABLED
    );
    let slot = input(&mut encoder, 17)?;
    // Without the delta map on, a map is not passed.
    encoder.next_map = Some(vec![-3, 3]);
    encoder.submit(slot, None, true, Instant::now())?;
    encoder.poll()?;
    encoder.enable_qp_map()?;
    state(|d| {
        let (parameters, config) = d.reconfigured.take().unwrap();
        assert_eq!((parameters.resetEncoder(), parameters.forceIDR()), (0, 0));
        assert_eq!(
            config.rcParams.qpMapMode,
            _NV_ENC_QP_MAP_MODE_NV_ENC_QP_MAP_DELTA
        );
    });
    assert!(!encoder.force_idr);
    encoder.enable_qp_map()?;
    state(|d| assert!(d.reconfigured.is_none(), "turned on once"));
    // A keyframe carries no map; an inter frame carries its own, once.
    encoder.next_map = Some(vec![-3, 3]);
    encoder.submit(slot, None, true, Instant::now())?;
    encoder.poll()?;
    encoder.next_map = Some(vec![-3, 3]);
    encoder.submit(slot, None, false, Instant::now())?;
    encoder.poll()?;
    encoder.submit(slot, None, false, Instant::now())?;
    encoder.poll()?;
    state(|d| assert_eq!(d.maps, [None, None, Some(vec![-3, 3]), None]));
    // A frame asked as an inter frame becomes a keyframe when the encoder
    // must make one, and the keyframe drops the map.
    encoder.force_idr = true;
    assert!(encoder.keyframe(false));
    encoder.next_map = Some(vec![-3, 3]);
    encoder.submit(slot, None, false, Instant::now())?;
    assert!(encoder.poll()?[0].idr);
    state(|d| assert_eq!(d.maps.last(), Some(&None)));
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn a_focus_maps_buffer_is_taken_back_and_refilled_for_the_next_map() -> Result<()> {
    reset();
    let mut encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    encoder.enable_qp_map()?;
    let slot = input(&mut encoder, 17)?;
    // A map left over from a frame that cannot carry one keeps its buffer.
    encoder.next_map = Some(vec![7]);
    encoder.submit(slot, None, true, Instant::now())?;
    encoder.poll()?;
    assert!(
        encoder.map_scratch.capacity() >= 1,
        "the refused map's buffer was dropped"
    );
    // Two inter frames carry maps; the second's is filled into the buffer
    // the first's slot handed back, so the maps ride one allocation.
    let mut address = None;
    for _ in 0..2 {
        let mut deltas = std::mem::take(&mut encoder.map_scratch);
        deltas.extend([-3, 3]);
        address.get_or_insert(deltas.as_ptr());
        encoder.next_map = Some(deltas);
        encoder.submit(slot, None, false, Instant::now())?;
        encoder.poll()?;
    }
    assert!(
        std::ptr::eq(encoder.map_scratch.as_ptr(), address.unwrap()),
        "the first map's buffer did not come back from its slot"
    );
    state(|d| {
        assert_eq!(
            d.maps,
            [None, Some(vec![-3, 3]), Some(vec![-3, 3])],
            "the recycled buffer carries the same values"
        )
    });
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn each_pictures_parameters_are_reset_not_left_from_the_frame_before() -> Result<()> {
    reset();
    let stream = Negotiated::default();
    let mut encoder = session(&stream, ApiVersion(13, 0))?;
    let slot = input(&mut encoder, 17)?;
    encoder.enable_qp_map()?;
    // A keyframe, an inter frame that carries a delta map, then one without.
    encoder.submit(slot, None, true, Instant::now())?;
    encoder.poll()?;
    encoder.next_map = Some(vec![-3, 3]);
    encoder.submit(slot, None, false, Instant::now())?;
    encoder.poll()?;
    encoder.submit(slot, None, false, Instant::now())?;
    encoder.poll()?;
    state(|d| assert_eq!(d.maps, [None, Some(vec![-3, 3]), None]));
    state(|d| {
        // Field by field: the struct has padding (after pictureType, for
        // one) that a copy need not preserve and a literal leaves undefined.
        // Each field is plain old data without padding of its own.
        fn as_bytes<T>(field: &T) -> &[u8] {
            // SAFETY: The field is plain old data; the bytes are only compared.
            unsafe {
                std::slice::from_raw_parts(
                    (field as *const T).cast::<u8>(),
                    std::mem::size_of::<T>(),
                )
            }
        }
        for (frame, p) in d.pictures.iter().enumerate() {
            // What one submission may set; everything else must be exactly
            // the zeroed block the session opened with, not what the frame
            // before left behind. The slot's pointers are per-slot state
            // the session writes the same each frame, so they are read from
            // the capture.
            let mut expected = NV_ENC_PIC_PARAMS {
                version: ApiVersion(13, 0).picture(),
                inputWidth: stream.width,
                inputHeight: stream.height,
                inputPitch: p.inputPitch,
                frameIdx: frame as u32 + 1,
                inputTimeStamp: frame as u64 + 1,
                inputBuffer: p.inputBuffer,
                outputBitstream: p.outputBitstream,
                completionEvent: p.completionEvent,
                bufferFmt: p.bufferFmt,
                pictureStruct: _NV_ENC_PIC_STRUCT_NV_ENC_PIC_STRUCT_FRAME,
                encodePicFlags: if frame == 0 {
                    _NV_ENC_PIC_FLAGS_NV_ENC_PIC_FLAG_FORCEIDR
                        | _NV_ENC_PIC_FLAGS_NV_ENC_PIC_FLAG_OUTPUT_SPSPPS
                } else {
                    0
                },
                ..Default::default()
            };
            if frame == 1 {
                expected.qpDeltaMap = p.qpDeltaMap;
                expected.qpDeltaMapSize = 2;
            }
            macro_rules! same {
                ($($field:ident),* $(,)?) => {$(
                    assert_eq!(
                        as_bytes(&p.$field),
                        as_bytes(&expected.$field),
                        "picture {frame}: {}",
                        stringify!($field)
                    );
                )*};
            }
            same!(
                version,
                inputWidth,
                inputHeight,
                inputPitch,
                encodePicFlags,
                frameIdx,
                inputTimeStamp,
                inputDuration,
                inputBuffer,
                outputBitstream,
                completionEvent,
                bufferFmt,
                pictureStruct,
                pictureType,
                codecPicParams,
                meHintCountsPerBlock,
                meExternalHints,
                reserved2,
                reserved5,
                qpDeltaMap,
                qpDeltaMapSize,
                reservedBitFields,
                meHintRefPicDist,
                reserved4,
                alphaBuffer,
                meExternalSbHints,
                meSbHintsCount,
                stateBufferIdx,
                outputReconBuffer,
                reserved3,
                reserved6,
            );
        }
    });
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn block_statistics_are_read_for_measured_frames_from_api_12_1() -> Result<()> {
    reset();
    let stream = Negotiated::default();
    // API 12.0 has no output statistics.
    let encoder = session_with(&stream, ApiVersion(12, 0), "encoder_focus_verify = true\n")?;
    assert!(!encoder.block_stats);
    assert_eq!(initialized().0.enableOutputStats(), 0);
    drop(encoder);
    reset();
    let mut tuning = Tuning::new(&Config::default(), &stream)?;
    tuning.qp_map = true;
    tuning.block_stats = true;
    let mut encoder = open_tuned(&stream, ApiVersion(13, 0), &tuning, None)?;
    assert!(encoder.block_stats && encoder.qp_map);
    let (parameters, config) = initialized();
    assert_eq!(parameters.enableOutputStats(), 1);
    assert_eq!(
        parameters.outputStatsLevel,
        _NV_ENC_OUTPUT_STATS_LEVEL_NV_ENC_OUTPUT_STATS_BLOCK_LEVEL
    );
    assert_eq!(
        config.rcParams.qpMapMode,
        _NV_ENC_QP_MAP_MODE_NV_ENC_QP_MAP_DELTA
    );
    let slot = input(&mut encoder, 17)?;
    encoder.submit(slot, None, true, Instant::now())?;
    encoder.poll()?;
    assert!(encoder.qp_grids.is_empty(), "only measured frames are read");
    let (columns, rows) = butterpollo_media_policy::nvenc::delta_grid(0, 1920, 1080);
    let blocks = (columns * rows) as usize;
    let map = (0..blocks)
        .map(|index| if index % 2 == 0 { -3 } else { 3 })
        .collect::<Vec<i8>>();
    encoder.next_map = Some(map);
    encoder.measure_next = true;
    encoder.submit(slot, None, false, Instant::now())?;
    encoder.poll()?;
    let (frame, grid) = encoder.qp_grids.pop_front().unwrap();
    assert_eq!(frame, 2);
    assert_eq!((grid.width, grid.height), (columns, rows));
    assert_eq!(grid.values[..4], [27, 33, 27, 33]);
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn the_focus_probe_verdict_needs_every_grid_and_a_two_qp_effect() {
    use crate::amf::focus::Grid;
    let half = |left: u32, right: u32| Grid {
        width: 4,
        height: 1,
        values: vec![left, left, right, right],
    };
    let frames = crate::amf::focus::PROBE_FRAMES as usize;
    let mut grids = vec![half(27, 33); frames];
    grids.extend(vec![half(33, 27); frames]);
    let probe = probe_verdict(0, grids.clone());
    assert_eq!(
        probe.support,
        encoder_focus::Support::Verified { delta: 6.0 }
    );
    assert_eq!(probe.control_delta, Some(0.0));
    assert_eq!(probe.feedback_grid, Some((4, 1)));
    // A map that moves QP by less than two is not focus.
    let mut weak = vec![half(30, 31); frames];
    weak.extend(vec![half(31, 30); frames]);
    assert!(matches!(
        probe_verdict(0, weak).support,
        encoder_focus::Support::Unsupported { .. }
    ));
    // AV1 needs the same in q-index.
    assert!(matches!(
        probe_verdict(2, grids.clone()).support,
        encoder_focus::Support::Unsupported { .. }
    ));
    // A missing grid leaves the effect unknown.
    grids.pop();
    assert!(matches!(
        probe_verdict(0, grids).support,
        encoder_focus::Support::Unsupported { .. }
    ));
}
#[test]
fn a_driver_controlled_vbv_reports_no_frame_cap() -> Result<()> {
    reset();
    let mut encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    encoder.config.rcParams.vbvBufferSize = 0;
    encoder.vbv_ceiling = 0;
    assert_eq!(encoder.set_frame_cap(1000)?, None);
    state(|d| assert!(d.reconfigured.is_none()));
    drop(encoder);
    assert_clean();
    Ok(())
}
