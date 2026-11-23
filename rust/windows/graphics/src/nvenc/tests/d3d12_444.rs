//! Fake-driver tests for 8-bit native 4:4:4 (AYUV) on D3D12 NVENC, D3D11 fallback,
//! and keeping 10-bit 4:4:4 on D3D11 + CUDA (Rubylight #150).

use super::*;

#[test]
fn d3d12_8bit_native_444_configures_ayuv_and_runs_split_rfi_reconfigure_and_focus() -> Result<()> {
    for api in [ApiVersion(13, 0), ApiVersion(12, 2)] {
        for codec in [0u8, 1] {
            reset();
            state(|d| d.engines = 2);
            let stream = Negotiated {
                width: 3456,
                height: 2160,
                fps: 120,
                codec,
                yuv444: true,
                ..Default::default()
            };
            assert!(supports_compute(&stream));
            let mut encoder = d3d12_session(&stream, api, "")?;
            let (params, config) = initialized();
            assert_eq!(params.enableEncodeAsync, 0);
            assert_eq!(
                params.bufferFormat,
                _NV_ENC_BUFFER_FORMAT_NV_ENC_BUFFER_FORMAT_AYUV
            );
            if codec == 0 {
                assert!(same_guid(
                    &config.profileGUID,
                    &NV_ENC_H264_PROFILE_HIGH_444_GUID
                ));
                // SAFETY: H.264 was configured; the union member is plain data.
                let h264 = unsafe { config.encodeCodecConfig.h264Config };
                assert_eq!(h264.chromaFormatIDC, 3);
                assert_eq!((h264.inputBitDepth, h264.outputBitDepth), (8, 8));
                assert_eq!(h264.h264VUIParameters.chromaSampleLocationFlag, 0);
            } else {
                assert!(same_guid(
                    &config.profileGUID,
                    &NV_ENC_HEVC_PROFILE_FREXT_GUID
                ));
                // SAFETY: HEVC was configured; the union member is plain data.
                let hevc = unsafe { config.encodeCodecConfig.hevcConfig };
                assert_eq!(hevc.chromaFormatIDC(), 3);
                assert_eq!((hevc.inputBitDepth, hevc.outputBitDepth), (8, 8));
                assert_eq!(hevc.hevcVUIParameters.chromaSampleLocationFlag, 0);
                assert!(
                    encoder.budget.unwrap().split,
                    "HEVC 4:4:4 at 2160 rows on two engines keeps split encoding"
                );
            }

            let slot = d3d12_input_waiting(
                &mut encoder,
                0x444,
                Some((CONVERSION_FENCE as *mut c_void, 5)),
            )?;
            state(|d| {
                assert_eq!(
                    d.resources[&0x444],
                    _NV_ENC_BUFFER_FORMAT_NV_ENC_BUFFER_FORMAT_AYUV
                );
                assert_eq!(
                    d.resources[&(0x444 | 0x1000)],
                    _NV_ENC_BUFFER_FORMAT_NV_ENC_BUFFER_FORMAT_U8
                );
                assert_eq!(d.register_fence_points, [Some((CONVERSION_FENCE, 5)), None]);
            });

            // Frame 1: IDR on D3D12 AYUV.
            encoder.submit(slot, converted(5), true, Instant::now())?;
            let idr = encoder.poll()?;
            assert_eq!(idr.len(), 1);
            assert!(idr[0].idr);
            state(|d| assert_eq!(d.fence_points, [(CONVERSION_FENCE, 5, 1)]));

            // Enable QP delta map without reset, submit inter frame with deltas.
            encoder.enable_qp_map()?;
            encoder.next_map = Some(vec![-3, 3]);
            encoder.submit(slot, converted(6), false, Instant::now())?;
            let inter = encoder.poll()?;
            assert_eq!(inter.len(), 1);
            assert!(!inter[0].idr);
            state(|d| assert_eq!(d.maps, [None, Some(vec![-3, 3])]));

            // Dynamic bitrate reconfigure, reference frame invalidation, forced IDR.
            encoder.bitrate(40_000)?;
            assert!(encoder.invalidate(2, 2)?);
            state(|d| assert_eq!(d.invalidated, [2]));
            encoder.submit(slot, converted(7), true, Instant::now())?;
            let forced = encoder.poll()?;
            assert_eq!(forced.len(), 1);
            assert!(forced[0].idr);

            drop(encoder);
            assert_clean();
        }
    }
    Ok(())
}

#[test]
fn d3d12_8bit_444_failure_falls_back_to_d3d11_while_session_limit_does_not() -> Result<()> {
    let stream = native_444(1, false);
    assert!(supports_compute(&stream));

    // 1. Driver refuses AYUV on D3D12 -> falls back to D3D11 AYUV.
    reset();
    state(|d| {
        d.reject_d3d12_format = Some(_NV_ENC_BUFFER_FORMAT_NV_ENC_BUFFER_FORMAT_AYUV);
    });
    let error = d3d12_session(&stream, ApiVersion(13, 0), "")
        .err()
        .expect("driver refusing AYUV on D3D12 must fail");
    assert!(falls_back_to_d3d11(true, &error));
    assert!(!falls_back_to_d3d11(false, &error));
    assert_clean();

    state(|d| d.d3d12 = false);
    let mut fallback = session(&stream, ApiVersion(13, 0))?;
    assert!(fallback.d3d12.is_none());
    let slot = input(&mut fallback, 0x444)?;
    state(|d| {
        assert_eq!(
            d.resources[&0x444],
            _NV_ENC_BUFFER_FORMAT_NV_ENC_BUFFER_FORMAT_AYUV
        );
    });
    fallback.submit(slot, None, true, Instant::now())?;
    assert_eq!(fallback.poll()?.len(), 1);
    drop(fallback);
    assert_clean();

    // 2. Older driver (API 11.0 < 12.0) fails D3D12 -> falls back to D3D11 AYUV.
    reset();
    let error = d3d12_session(&stream, ApiVersion(11, 0), "")
        .err()
        .expect("API 11.0 cannot open D3D12");
    assert!(format!("{error:#}").contains("522.25"), "{error:#}");
    assert!(falls_back_to_d3d11(true, &error));
    state(|d| d.d3d12 = false);
    drop(session(&stream, ApiVersion(11, 0))?);
    assert_clean();

    // 3. Session limit never retries on D3D11.
    for status in [
        _NVENCSTATUS_NV_ENC_ERR_OUT_OF_MEMORY,
        _NVENCSTATUS_NV_ENC_ERR_INCOMPATIBLE_CLIENT_KEY,
    ] {
        reset();
        state(|d| d.open_status = Some(status));
        let error = d3d12_session(&stream, ApiVersion(13, 0), "")
            .err()
            .expect("session limit must fail");
        assert!(session_limit(&error));
        assert!(!falls_back_to_d3d11(true, &error));
        assert_clean();
    }
    Ok(())
}

#[test]
fn ten_bit_native_444_stays_on_d3d11_and_cuda_never_d3d12() -> Result<()> {
    for (hdr, sdr_10bit) in [(true, false), (false, true)] {
        let stream = Negotiated {
            codec: 1,
            hdr,
            sdr_10bit,
            yuv444: true,
            ..Default::default()
        };
        assert!(!supports_compute(&stream));

        reset();
        let error = d3d12_session(&stream, ApiVersion(13, 0), "")
            .err()
            .expect("D3D12 session must refuse 10-bit 4:4:4");
        assert!(format!("{error:#}").contains("10-bit 4:4:4"), "{error:#}");
        state(|d| {
            assert!(
                d.initialized.is_none(),
                "10-bit 4:4:4 must fail before nvEncInitializeEncoder on D3D12"
            );
        });
        assert_clean();

        // Non-D3D12 path (D3D11 + CUDA) keeps working with YUV444_10BIT.
        reset();
        let mut cuda_session = session(&stream, ApiVersion(13, 0))?;
        let slot = input(&mut cuda_session, 0x4440)?;
        state(|d| {
            assert_eq!(
                d.resources[&0x4440],
                _NV_ENC_BUFFER_FORMAT_NV_ENC_BUFFER_FORMAT_YUV444_10BIT
            );
        });
        cuda_session.submit(slot, None, true, Instant::now())?;
        assert_eq!(cuda_session.poll()?.len(), 1);
        drop(cuda_session);
        assert_clean();
    }

    // AV1 4:4:4 remains refused on D3D12 too.
    reset();
    let av1 = native_444(2, false);
    let error = d3d12_session(&av1, ApiVersion(13, 0), "")
        .err()
        .expect("AV1 4:4:4 must stay refused on D3D12");
    assert!(format!("{error:#}").contains("AV1 4:4:4"), "{error:#}");
    state(|d| assert!(d.initialized.is_none()));
    assert_clean();
    Ok(())
}

#[test]
fn d3d12_readback_buffer_accounts_for_full_chroma_without_changing_420() {
    let sdr_420 = Negotiated::default();
    assert_eq!(readback_size(&sdr_420), 1920 * 1080 * 3);
    let hdr_420 = Negotiated {
        hdr: true,
        ..Default::default()
    };
    assert_eq!(readback_size(&hdr_420), 1920 * 1080 * 6);
    let sdr_444 = Negotiated {
        yuv444: true,
        ..Default::default()
    };
    assert_eq!(readback_size(&sdr_444), 1920 * 1080 * 6);
    let uhd_444 = Negotiated {
        width: 3456,
        height: 2160,
        yuv444: true,
        ..Default::default()
    };
    assert_eq!(readback_size(&uhd_444), MAX_FRAME_BYTES as u32);
}
