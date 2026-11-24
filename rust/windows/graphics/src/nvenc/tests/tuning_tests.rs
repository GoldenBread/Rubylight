//! Fake-driver tests for NVENC VBR, spatial AQ, frame budget guard, split encoding,
//! codec listing, HLG/PQ transfer signaling, packed/native 4:4:4, and throughput caps.

use super::*;

#[test]
fn vbr_sets_its_peak_and_quality_keeps_them_on_bitrate_changes_and_falls_back_loudly() -> Result<()>
{
    reset();
    let stream = Negotiated {
        codec: 2,
        ..Default::default()
    };
    let mut encoder = session_with(
        &stream,
        ApiVersion(13, 0),
        "nvenc_rate_control = vbr\nnvenc_target_quality = 40\n",
    )?;
    let (_, config) = initialized();
    assert_eq!(
        config.rcParams.rateControlMode,
        _NV_ENC_PARAMS_RC_MODE_NV_ENC_PARAMS_RC_VBR
    );
    assert_eq!(config.rcParams.averageBitRate, 20_000_000);
    assert_eq!(config.rcParams.maxBitRate, 30_000_000);
    assert_eq!(config.rcParams.targetQuality, 40);
    encoder.bitrate(40000)?;
    state(|d| {
        let (_, config) = d.reconfigured.as_ref().unwrap();
        assert_eq!(config.rcParams.averageBitRate, 40_000_000);
        assert_eq!(config.rcParams.maxBitRate, 60_000_000);
    });
    drop(encoder);
    // A GPU without VBR encodes CBR, with a warning in the log.
    reset();
    state(|d| d.rate_modes = Some(_NV_ENC_PARAMS_RC_MODE_NV_ENC_PARAMS_RC_CBR as i32));
    let encoder = session_with(&stream, ApiVersion(13, 0), "nvenc_rate_control = vbr\n")?;
    assert_eq!(encoder.rate_control, RateControl::Cbr);
    let (_, config) = initialized();
    assert_eq!(
        config.rcParams.rateControlMode,
        _NV_ENC_PARAMS_RC_MODE_NV_ENC_PARAMS_RC_CBR
    );
    assert_eq!(config.rcParams.maxBitRate, config.rcParams.averageBitRate);
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn spatial_aq_is_on_by_default_and_the_default_preset_stays_p1() -> Result<()> {
    reset();
    let encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    let (parameters, config) = initialized();
    assert_eq!(config.rcParams.enableAQ(), 1);
    assert!(same_preset(parameters.presetGUID, 1));
    assert_eq!(encoder.budget.unwrap().effective, 1);
    drop(encoder);
    reset();
    let encoder = session_with(
        &Negotiated::default(),
        ApiVersion(13, 0),
        "nvenc_spatial_aq = false\n",
    )?;
    assert_eq!(initialized().1.rcParams.enableAQ(), 0);
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn the_frame_budget_guard_lowers_a_slow_preset_unless_turned_off() -> Result<()> {
    let uhd = Negotiated {
        codec: 1,
        width: 3840,
        height: 2160,
        fps: 120,
        ..Default::default()
    };
    reset();
    let encoder = session_with(&uhd, ApiVersion(13, 0), "nvenc_preset = 4\n")?;
    assert!(same_preset(initialized().0.presetGUID, 1));
    assert_eq!(encoder.budget.unwrap().configured, 4);
    drop(encoder);
    // Two engines that split encoding may use leave P4 in budget, unless
    // split encoding is off.
    reset();
    state(|d| d.engines = 2);
    let encoder = session_with(&uhd, ApiVersion(13, 0), "nvenc_preset = 4\n")?;
    assert!(same_preset(initialized().0.presetGUID, 4));
    drop(encoder);
    reset();
    state(|d| d.engines = 2);
    let encoder = session_with(
        &uhd,
        ApiVersion(13, 0),
        "nvenc_preset = 4\nnvenc_split_encode = disabled\n",
    )?;
    assert!(same_preset(initialized().0.presetGUID, 1));
    drop(encoder);
    reset();
    let encoder = session_with(
        &uhd,
        ApiVersion(13, 0),
        "nvenc_preset = 4\nnvenc_frame_budget_guard = false\n",
    )?;
    assert!(same_preset(initialized().0.presetGUID, 4));
    assert!(encoder.budget.unwrap().estimated_ms > encoder.budget.unwrap().budget_ms);
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn auto_forces_split_for_a_wide_high_rate_mode_but_driver_decides_does_not() -> Result<()> {
    let wide = Negotiated {
        codec: 1,
        width: 3440,
        height: 1440,
        fps: 240,
        ..Default::default()
    };
    reset();
    state(|d| d.engines = 2);
    let encoder = session_with(&wide, ApiVersion(13, 0), "nvenc_preset = 4\n")?;
    // One engine misses 240 Hz, so auto uses both engines and the guard
    // keeps the slowest preset that fits: P3.
    assert_eq!(encoder.budget.unwrap().effective, 3);
    assert!(encoder.budget.unwrap().split);
    assert_eq!(initialized().0.splitEncodeMode(), 1);
    drop(encoder);
    reset();
    state(|d| d.engines = 2);
    let encoder = session_with(
        &wide,
        ApiVersion(13, 0),
        "nvenc_preset = 4\nnvenc_split_encode = driver_decides\n",
    )?;
    // The escape hatch keeps NVIDIA's implicit split only, which 1440p does
    // not reach; the guard falls to P1.
    assert_eq!(encoder.budget.unwrap().effective, 1);
    assert!(!encoder.budget.unwrap().split);
    assert_eq!(initialized().0.splitEncodeMode(), 0);
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn split_strips_and_the_slice_mode_follow_the_settings() -> Result<()> {
    let hevc = Negotiated {
        codec: 1,
        ..Default::default()
    };
    let slice_mode = |config: &NV_ENC_CONFIG| {
        // SAFETY: HEVC was configured; the union member is plain integers and pointers.
        let hevc = unsafe { config.encodeCodecConfig.hevcConfig };
        (hevc.sliceMode, hevc.sliceModeData)
    };
    reset();
    let encoder = session_with(
        &hevc,
        ApiVersion(13, 0),
        "nvenc_split_encode = two_strips\n",
    )?;
    let (parameters, config) = initialized();
    assert_eq!(parameters.splitEncodeMode(), 2);
    // One slice and split encoding allowed: no fixed slice mode.
    assert_eq!(slice_mode(&config), (0, 0));
    drop(encoder);
    reset();
    let encoder = session_with(&hevc, ApiVersion(13, 0), "nvenc_split_encode = disabled\n")?;
    let (parameters, config) = initialized();
    assert_eq!(parameters.splitEncodeMode(), SPLIT_DISABLED);
    assert_eq!(slice_mode(&config), (3, 1));
    drop(encoder);
    reset();
    let sliced = Negotiated {
        slices: 4,
        ..hevc.clone()
    };
    let encoder = session(&sliced, ApiVersion(13, 0))?;
    assert_eq!(slice_mode(&initialized().1), (3, 4));
    drop(encoder);
    reset();
    let encoder = session(&hevc, ApiVersion(12, 1))?;
    assert_eq!(slice_mode(&initialized().1), (3, 1));
    drop(encoder);
    assert_clean();
    Ok(())
}
#[test]
fn a_codec_the_gpu_lacks_is_refused_by_name_before_the_session_configures() -> Result<()> {
    reset();
    state(|d| d.codecs = Some(vec![0, 1]));
    let av1 = Negotiated {
        codec: 2,
        ..Default::default()
    };
    let error = format!("{:#}", session(&av1, ApiVersion(13, 0)).err().unwrap());
    assert!(error.contains("cannot encode AV1"), "{error}");
    assert!(error.contains("Select another codec"), "{error}");
    state(|d| assert!(d.initialized.is_none()));
    assert_clean();
    let hevc = Negotiated {
        codec: 1,
        ..Default::default()
    };
    drop(session(&hevc, ApiVersion(13, 0))?);
    assert_clean();
    Ok(())
}
#[test]
fn hlg_streams_signal_the_hlg_transfer_and_pq_streams_pq() -> Result<()> {
    for (hlg, transfer) in [(false, 16), (true, 18)] {
        reset();
        let mut stream = Negotiated {
            codec: 1,
            hdr: true,
            ..Default::default()
        };
        if hlg {
            // rubylight_protocol::hdr::HLG
            stream.hdr_format = 2;
        }
        assert_eq!(stream.hlg(), hlg);
        let encoder = session(&stream, ApiVersion(13, 0))?;
        // SAFETY: HEVC was configured; the union member is plain integers and pointers.
        let vui = unsafe { initialized().1.encodeCodecConfig.hevcConfig }.hevcVUIParameters;
        assert_eq!(vui.transferCharacteristics, transfer);
        drop(encoder);
    }
    assert_clean();
    Ok(())
}
#[test]
fn packed_444_codes_a_larger_sdr_picture_like_amf() {
    let stream = Negotiated {
        codec: 1,
        hdr: true,
        width: 1920,
        height: 1080,
        chroma_packing: 1,
        ..Default::default()
    };
    let plain = coded(&Negotiated::default());
    assert_eq!(plain.width, 1920);
    assert_eq!(plain.chroma_packing, 0);
    stream.validate().unwrap();
    let packed = coded(&stream);
    assert_eq!((packed.width, packed.height), stream.coded_size());
    assert!(!packed.hdr && packed.sdr_10bit && !packed.yuv444);
    assert_eq!(packed.csc_mode, 3);
    assert_eq!(packed.chroma_packing, 0);
    packed.validate().unwrap();
    refuse_native_444(&packed).unwrap();
}
/// A native 4:4:4 stream of `codec`, 10-bit HDR or 8-bit SDR.
pub(super) fn native_444(codec: u8, hdr: bool) -> Negotiated {
    Negotiated {
        codec,
        hdr,
        yuv444: true,
        ..Default::default()
    }
}
#[test]
fn only_av1_444_is_refused_before_a_session_since_sdk_13_has_none() {
    for hdr in [false, true] {
        refuse_native_444(&coded(&native_444(1, hdr))).unwrap();
        let error = refuse_native_444(&coded(&native_444(2, hdr)))
            .unwrap_err()
            .to_string();
        assert!(
            error.starts_with("NVENC cannot encode AV1 4:4:4"),
            "{error}"
        );
        assert!(error.contains("Choose HEVC"), "{error}");
    }
    refuse_native_444(&coded(&native_444(0, false))).unwrap();
    refuse_native_444(&coded(&Negotiated::default())).unwrap();
}
/// The configuration each native 4:4:4 mode initializes, as Sunshine's
/// `nvenc_base.cpp` builds it: High 4:4:4 for H.264, Rext (FREXT) for HEVC,
/// `chromaFormatIDC` 3, the bit depths, VUI chroma location off, and the
/// buffer format the input registers with (AYUV for 8-bit, YUV444_10BIT for
/// 10-bit). Rate control, spatial AQ and the frame budget guard stay as for
/// 4:2:0.
#[test]
fn native_444_configures_each_codec_and_depth_like_sunshine() -> Result<()> {
    for api in [ApiVersion(13, 0), ApiVersion(12, 1)] {
        for (codec, hdr) in [(0, false), (1, false), (1, true)] {
            reset();
            let stream = native_444(codec, hdr);
            let mut encoder = session(&stream, api)?;
            let (initialize, config) = initialized();
            assert!(initialize.encodeWidth == 1920 && initialize.encodeHeight == 1080);
            assert_eq!(
                config.rcParams.rateControlMode,
                _NV_ENC_PARAMS_RC_MODE_NV_ENC_PARAMS_RC_CBR
            );
            assert_eq!(config.rcParams.averageBitRate, stream.bitrate_kbps * 1000);
            assert_eq!(config.rcParams.enableAQ(), 1, "spatial AQ stays on");
            assert!(encoder.budget.is_some(), "the frame budget guard ran");
            let depth = if hdr { 10 } else { 8 };
            match codec {
                0 => {
                    assert!(same_guid(
                        &config.profileGUID,
                        &NV_ENC_H264_PROFILE_HIGH_444_GUID
                    ));
                    // SAFETY: H.264 was configured; the member is plain data.
                    let h264 = unsafe { config.encodeCodecConfig.h264Config };
                    assert_eq!(h264.chromaFormatIDC, 3);
                    assert_eq!(h264.h264VUIParameters.chromaSampleLocationFlag, 0);
                    if api.modern_depth() {
                        assert_eq!((h264.inputBitDepth, h264.outputBitDepth), (8, 8));
                    }
                }
                _ => {
                    assert!(same_guid(
                        &config.profileGUID,
                        &NV_ENC_HEVC_PROFILE_FREXT_GUID
                    ));
                    // SAFETY: HEVC was configured; the member is plain data.
                    let hevc = unsafe { config.encodeCodecConfig.hevcConfig };
                    assert_eq!(hevc.chromaFormatIDC(), 3);
                    assert_eq!(hevc.hevcVUIParameters.chromaSampleLocationFlag, 0);
                    if api.modern_depth() {
                        assert_eq!((hevc.inputBitDepth, hevc.outputBitDepth), (depth, depth));
                    } else {
                        // pixelBitDepthMinus8 in the 12.x layout.
                        assert_eq!(hevc.reserved3(), if hdr { 2 } else { 0 });
                    }
                    if hdr {
                        assert_eq!(hevc.hevcVUIParameters.transferCharacteristics, 16);
                        assert_eq!(hevc.hevcVUIParameters.colourMatrix, 9);
                    }
                }
            }
            let wanted = if hdr {
                _NV_ENC_BUFFER_FORMAT_NV_ENC_BUFFER_FORMAT_YUV444_10BIT
            } else {
                _NV_ENC_BUFFER_FORMAT_NV_ENC_BUFFER_FORMAT_AYUV
            };
            assert_eq!(buffer_format(&stream), wanted);
            let slot = input(&mut encoder, 0x444)?;
            state(|d| assert_eq!(d.resources[&0x444], wanted));
            encoder.submit(slot, None, true, Instant::now())?;
            drop(encoder);
            assert_clean();
        }
    }
    Ok(())
}
#[test]
fn a_gpu_without_444_refuses_the_stream_by_name_and_never_codes_4_2_0() -> Result<()> {
    for (codec, hdr) in [(0, false), (1, false), (1, true)] {
        reset();
        state(|d| d.no_yuv444 = true);
        let stream = native_444(codec, hdr);
        let error = format!("{:#}", session(&stream, ApiVersion(13, 0)).err().unwrap());
        assert!(error.contains("reports no"), "{error}");
        assert!(error.contains("4:4:4 encoding"), "{error}");
        assert!(error.contains(codec_name(codec)), "{error}");
        assert!(
            error.contains("Turn off YUV 4:4:4 on the device"),
            "{error}"
        );
        // Refused before the encoder initializes: no 4:2:0 session in its place.
        state(|d| assert!(d.initialized.is_none()));
        assert_clean();
        // The same GPU still codes 4:2:0.
        let plain = Negotiated {
            codec,
            hdr,
            ..Default::default()
        };
        drop(session(&plain, ApiVersion(13, 0))?);
        assert_clean();
    }
    // 10-bit 4:4:4 needs both answers.
    reset();
    state(|d| d.no_10bit = true);
    let stream = native_444(1, true);
    let error = format!("{:#}", session(&stream, ApiVersion(13, 0)).err().unwrap());
    assert!(error.contains("10-bit"), "{error}");
    state(|d| assert!(d.initialized.is_none()));
    assert_clean();
    // AV1 4:4:4 is refused whatever the driver answers.
    reset();
    let stream = native_444(2, false);
    let error = format!("{:#}", session(&stream, ApiVersion(13, 0)).err().unwrap());
    assert!(error.contains("AV1 4:4:4"), "{error}");
    state(|d| assert!(d.initialized.is_none()));
    assert_clean();
    Ok(())
}
#[test]
fn throughput_names_map_to_their_codec_only() {
    assert_eq!(throughput_codec("MaxThroughput"), Some(0));
    assert_eq!(throughput_codec("HevcMaxThroughput"), Some(1));
    assert_eq!(throughput_codec("Av1CapMaxThroughput"), Some(2));
    assert_eq!(throughput_codec("MaxBitrate"), None);
    assert_eq!(codec_name(2), "AV1");
}
