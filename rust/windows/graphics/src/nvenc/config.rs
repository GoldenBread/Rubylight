//! NVENC session configuration: codec profiles, native 4:4:4 capability checks,
//! rate control and VBV sizing, split-frame encoding, subframe readback, and CUDA IO streams.
//!
//! Frame budget guard, VBR rate control, fixed split-encode strips, codec check and spatial AQ
//! follow Foundation Sunshine (AlkaidLab/foundation-sunshine, GPL-3.0, src/nvenc/).
//! Native YUV 4:4:4 capability checks follow LizardByte Sunshine (LizardByte/Sunshine, GPL-3.0,
//! commit ec2676b60bb8220a1807fc761bc2157e5e6dba63, src/nvenc/nvenc_base.cpp).

use super::*;

impl Session {
    pub(super) fn configure(&mut self, tuning: &Tuning) -> Result<()> {
        let _guard = self.guard()?;
        self.stream.validate()?;
        self.check_codec()?;
        let max_width = self.cap(_NV_ENC_CAPS_NV_ENC_CAPS_WIDTH_MAX)?.max(0) as u32;
        let max_height = self.cap(_NV_ENC_CAPS_NV_ENC_CAPS_HEIGHT_MAX)?.max(0) as u32;
        if self.stream.width > max_width || self.stream.height > max_height {
            return Err(crate::encoder::SizeTooLarge {
                encoder: "NVENC",
                width: self.stream.width,
                height: self.stream.height,
                max: Some((max_width, max_height)),
            }
            .into());
        }
        if self.stream.ten_bit()
            && (!self.optional_cap(_NV_ENC_CAPS_NV_ENC_CAPS_SUPPORT_10BIT_ENCODE)
                || (self.stream.codec == 0 && !self.api.modern_depth()))
        {
            bail!("this NVENC driver/codec cannot encode 10-bit input");
        }
        // YUV 4:4:4 capability check ported from LizardByte Sunshine
        // src/nvenc/nvenc_base.cpp `validate_encoder_capabilities`
        // (ec2676b60bb8220a1807fc761bc2157e5e6dba63), GPL-3.0: the GPU's
        // answer for this codec decides, beside the 10-bit check above for a
        // 10-bit stream.
        if self.stream.yuv444 {
            refuse_native_444(&self.stream)?;
            if self.d3d12.is_some() && !supports_compute(&self.stream) {
                bail!("NVENC on D3D12 does not support 10-bit 4:4:4; use D3D11 with CUDA");
            }
            if !self.optional_cap(_NV_ENC_CAPS_NV_ENC_CAPS_SUPPORT_YUV444_ENCODE) {
                bail!(
                    "this NVIDIA GPU's NVENC reports no {} 4:4:4 encoding (NV_ENC_CAPS_SUPPORT_YUV444_ENCODE is 0). Turn off YUV 4:4:4 on the device to stream 4:2:0",
                    codec_name(self.stream.codec)
                );
            }
        }
        // Slice delivery reads a frame while it encodes. The driver writes
        // each slice as it finishes only in synchronous mode, polled without
        // waiting; every other stream keeps completion events.
        self.slice_blocks = tuning.slice_blocks;
        let readback = tuning.slice_blocks > 0
            && self.stream.codec < 2
            && self.optional_cap(_NV_ENC_CAPS_NV_ENC_CAPS_SUPPORT_SUBFRAME_READBACK);
        // The engine count and weighted-prediction capability decide whether
        // split can engage, and whether the guard may count a speedup. Both
        // are needed before the subframe/split decision.
        let engines = self
            .cap(_NV_ENC_CAPS_NV_ENC_CAPS_NUM_ENCODER_ENGINES)
            .unwrap_or(1)
            .max(1) as u32;
        let weighted = tuning.weighted_prediction
            && self.d3d12.is_none()
            && self.optional_cap(_NV_ENC_CAPS_NV_ENC_CAPS_SUPPORT_WEIGHTED_PREDICTION);
        let split_capable = butterpollo_media_policy::nvenc::split_capable(
            self.stream.codec,
            self.api,
            engines,
            weighted,
        );
        let mut split = tuning.split;
        let mut split_reason = if split == SPLIT_DISABLED {
            "disabled"
        } else if split == 0 {
            if tuning.split_driver_decides {
                "driver_decides"
            } else {
                "implicit"
            }
        } else {
            "forced"
        };
        if !split_capable && split != SPLIT_DISABLED {
            split_reason = if self.stream.codec == 0 {
                "h264"
            } else if !self.api.modern_depth() {
                "api"
            } else if engines <= 1 {
                "one_engine"
            } else {
                "weighted"
            };
        } else if split == 0 && !tuning.split_driver_decides && self.stream.slices <= 1 {
            // `auto` may use both engines when one engine's estimate misses
            // the frame budget. This is a split decision, independent of
            // whether the guard lowers the preset.
            let one_engine = butterpollo_media_policy::nvenc::frame_budget(
                tuning.preset,
                (self.stream.width, self.stream.height),
                f64::from(self.stream.fps_millihz()) / 1000.,
                |_| false,
                engines,
                false,
            );
            if one_engine.configured_ms > one_engine.budget_ms {
                split = 1;
                split_reason = "budget";
            }
        } else if split == 0 && self.stream.slices > 1 {
            split_reason = "client_slices";
        }
        // HEVC split encoding and subframe readback exclude each other. Where
        // split encoding engages it keeps the frame budget (`frame_budget`),
        // so it is kept; where it would not, slices leave as they finish.
        let splits = butterpollo_media_policy::nvenc::SplitContext {
            codec: self.stream.codec,
            api: self.api,
            engines,
            weighted,
            subframe: false,
            slices: self.stream.slices,
            mode: split,
            height: self.stream.height,
        }
        .excludes_subframe(tuning.preset, tuning.frame_budget_guard);
        self.subframe = readback && !splits && self.d3d12.is_none();
        // A D3D12 session reports no subframe readback at all, so the reason
        // is the conversion, not the driver.
        if tuning.slice_blocks > 0 && self.stream.codec < 2 && self.d3d12.is_some() {
            tracing::info!(
                blocks = tuning.slice_blocks,
                "video_slice_delivery is on, but NVENC on the D3D12 conversion returns whole frames; frames leave whole. Set gpu_compute_conversion = false to send slices instead"
            );
        } else if readback && splits {
            tracing::info!(
                blocks = tuning.slice_blocks,
                "video_slice_delivery is on, but HEVC split encoding uses this GPU's NVENC engines and cannot return slices as they finish; frames leave whole. Set nvenc_split_encode = disabled to send slices instead"
            );
        } else if tuning.slice_blocks > 0 && !self.subframe {
            tracing::warn!(
                blocks = tuning.slice_blocks,
                "video_slice_delivery is on, but this NVENC driver cannot return a frame's slices as they finish; frames leave whole. Update the NVIDIA driver"
            );
        }
        self.subframe_slices = if self.subframe {
            tuning.subframe_slices(self.stream.slices)
        } else {
            0
        };
        self.slice_offsets = if self.subframe {
            vec![0; (self.stream.width.div_ceil(16) * self.stream.height.div_ceil(16)) as usize]
        } else {
            Vec::new()
        };
        // HEVC split encoding cannot run in subframe mode.
        let split = if self.subframe && self.stream.codec == 1 {
            split_reason = "subframe";
            SPLIT_DISABLED
        } else {
            split
        };
        // The guard tries each candidate preset with the split that preset
        // would get: implicit split only inside its thresholds, a forced mode
        // at every preset.
        let split_context = butterpollo_media_policy::nvenc::SplitContext {
            codec: self.stream.codec,
            api: self.api,
            engines,
            weighted,
            subframe: self.subframe,
            slices: self.stream.slices,
            mode: split,
            height: self.stream.height,
        };
        // A D3D12 session completes through its output fence.
        let asynchronous = !self.subframe
            && self.d3d12.is_none()
            && self.optional_cap(_NV_ENC_CAPS_NV_ENC_CAPS_ASYNC_ENCODE_SUPPORT)
            && self.functions.nvEncRegisterAsyncEvent.is_some()
            && self.functions.nvEncUnregisterAsyncEvent.is_some();
        self.retained = if self.optional_cap(_NV_ENC_CAPS_NV_ENC_CAPS_SUPPORT_MULTIPLE_REF_FRAMES) {
            if self.stream.references > 0 {
                self.stream.references
            } else if self.stream.codec == 2 {
                8
            } else {
                5
            }
        } else {
            1
        };
        self.invalidation = self.retained > 1
            && self.functions.nvEncInvalidateRefFrames.is_some()
            && self.optional_cap(_NV_ENC_CAPS_NV_ENC_CAPS_SUPPORT_REF_PIC_INVALIDATION);
        self.dynamic_bitrate = self.functions.nvEncReconfigureEncoder.is_some()
            && self.optional_cap(_NV_ENC_CAPS_NV_ENC_CAPS_SUPPORT_DYN_BITRATE_CHANGE);
        let mut preset = NV_ENC_PRESET_CONFIG {
            version: self.api.preset(),
            presetCfg: NV_ENC_CONFIG {
                version: self.api.config(),
                ..Default::default()
            },
            ..Default::default()
        };
        let budget = butterpollo_media_policy::nvenc::frame_budget(
            tuning.preset,
            (self.stream.width, self.stream.height),
            f64::from(self.stream.fps_millihz()) / 1000.,
            |preset| split_context.engages(preset),
            engines,
            tuning.frame_budget_guard,
        );
        if budget.effective < budget.configured {
            tracing::warn!(
                configured = budget.configured,
                effective = budget.effective,
                budget_ms = budget.budget_ms,
                configured_ms = budget.configured_ms,
                engines = budget.engines,
                split = budget.split,
                split_reason,
                over_at_floor = budget.over_at_floor,
                "NVENC frame budget guard lowered the preset so encoding keeps up with the frame rate. Turn off nvenc_frame_budget_guard to keep the configured preset"
            );
        }
        self.budget = Some(budget);
        let guid = codec_guid(self.stream.codec)?;
        let preset_guid = preset_guid(budget.effective);
        let result = if let Some(get) = self.functions.nvEncGetEncodePresetConfigEx {
            // SAFETY: `get` is the driver's own preset entry, `self.raw` is the open session and
            // `preset` is versioned.
            unsafe {
                get(
                    self.raw,
                    guid,
                    preset_guid,
                    NV_ENC_TUNING_INFO_NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY,
                    &mut preset,
                )
            }
        } else {
            _NVENCSTATUS_NV_ENC_ERR_UNIMPLEMENTED
        };
        if result != SUCCESS {
            if let Some(get) = self.functions.nvEncGetEncodePresetConfig {
                tracing::warn!(
                    code = result,
                    "NVENC low-latency preset query failed; using a compatible preset with explicit low-latency settings. Update the NVIDIA driver if frame times worsen"
                );
                preset = NV_ENC_PRESET_CONFIG {
                    version: self.api.preset(),
                    presetCfg: NV_ENC_CONFIG {
                        version: self.api.config(),
                        ..Default::default()
                    },
                    ..Default::default()
                };
                self.check(
                    // SAFETY: `get` is the driver's legacy preset entry for the open session and
                    // `preset` is versioned.
                    unsafe { get(self.raw, guid, preset_guid, &mut preset) },
                    "query compatible preset",
                )?;
            } else {
                self.check(result, "query low latency preset")?;
            }
        }
        *self.config = preset.presetCfg;
        self.config.version = self.api.config();
        self.config.gopLength = u32::MAX;
        self.config.frameIntervalP = 1;
        self.config.frameFieldMode =
            _NV_ENC_PARAMS_FRAME_FIELD_MODE_NV_ENC_PARAMS_FRAME_FIELD_MODE_FRAME;
        let custom_vbv = self.optional_cap(_NV_ENC_CAPS_NV_ENC_CAPS_SUPPORT_CUSTOM_VBV_BUF_SIZE);
        let temporal_aq =
            tuning.temporal_aq && self.optional_cap(_NV_ENC_CAPS_NV_ENC_CAPS_SUPPORT_TEMPORAL_AQ);
        if !custom_vbv {
            tracing::warn!(
                "NVENC custom VBV unavailable; the driver controls buffering and may increase latency. Update the NVIDIA driver"
            );
        }
        if tuning.temporal_aq && !temporal_aq {
            tracing::warn!(
                "NVENC temporal AQ unavailable; encoding without the requested quality feature. Update the NVIDIA driver or disable temporal AQ"
            );
        }
        let vbr_supported = self
            .cap(_NV_ENC_CAPS_NV_ENC_CAPS_SUPPORTED_RATECONTROL_MODES)
            .unwrap_or(0)
            & _NV_ENC_PARAMS_RC_MODE_NV_ENC_PARAMS_RC_VBR as i32
            != 0;
        self.rate_control = match tuning.rate_control {
            RateControl::Vbr { .. } if !vbr_supported => {
                tracing::warn!(
                    "NVENC VBR rate control unavailable on this GPU; encoding at a constant bitrate. Update the NVIDIA driver or set nvenc_rate_control to cbr"
                );
                RateControl::Cbr
            }
            chosen => chosen,
        };
        let rate_control = self.rate_control;
        let rc = &mut self.config.rcParams;
        rc.version = self.api.structure(1, false);
        rc.averageBitRate = self.stream.bitrate_kbps * 1000;
        rc.maxBitRate = rate_control.max_bitrate(rc.averageBitRate);
        match rate_control {
            RateControl::Cbr => rc.rateControlMode = _NV_ENC_PARAMS_RC_MODE_NV_ENC_PARAMS_RC_CBR,
            RateControl::Vbr { target_quality, .. } => {
                rc.rateControlMode = _NV_ENC_PARAMS_RC_MODE_NV_ENC_PARAMS_RC_VBR;
                rc.targetQuality = target_quality;
            }
        }
        rc.set_zeroReorderDelay(1);
        rc.set_enableLookahead(0);
        rc.lookaheadDepth = 0;
        rc.lowDelayKeyFrameScale = 1;
        rc.multiPass = tuning.multipass;
        rc.set_enableAQ(u32::from(tuning.spatial_aq));
        rc.set_enableTemporalAQ(u32::from(temporal_aq));
        if custom_vbv {
            rc.vbvBufferSize = tuning.vbv(self.stream.bitrate_kbps, self.stream.fps_millihz());
            rc.vbvInitialDelay = rc.vbvBufferSize;
            self.vbv_ceiling = rc.vbvBufferSize;
            self.vbv_increase = tuning.vbv_increase;
        }
        if let Some(qp) = tuning.min_qp {
            rc.set_enableMinQP(1);
            rc.minQP = NV_ENC_QP {
                qpInterP: qp,
                qpInterB: qp,
                qpIntra: qp,
            };
        }
        let intra_refresh = self.stream.intra_refresh
            && !self.subframe
            && self.optional_cap(_NV_ENC_CAPS_NV_ENC_CAPS_SUPPORT_INTRA_REFRESH);
        let single_slice =
            intra_refresh && self.optional_cap(_NV_ENC_CAPS_NV_ENC_CAPS_SINGLE_SLICE_INTRA_REFRESH);
        if self.stream.intra_refresh && self.subframe {
            // As AMF does with slice delivery.
            tracing::warn!(
                "video_slice_delivery is on: intra refresh is off for this stream and a lost frame is recovered with a keyframe"
            );
        } else if self.stream.intra_refresh && !intra_refresh {
            tracing::warn!(
                "NVENC intra-refresh unavailable; keeping IDR recovery, which can cause larger recovery frames. Reduce bitrate on lossy links or update the NVIDIA driver"
            );
        }
        let ten = self.stream.ten_bit();
        let depth = if ten { 10 } else { 8 };
        let chroma = if self.stream.yuv444 { 3 } else { 1 };
        let metadata = self.stream.hdr && self.api >= ApiVersion(13, 0);
        let cabac = !tuning.cavlc && self.optional_cap(_NV_ENC_CAPS_NV_ENC_CAPS_SUPPORT_CABAC);
        let slice_mode = self.subframe || tuning.slice_mode(self.stream.slices, self.api, split);
        let slices = if self.subframe {
            self.subframe_slices
        } else {
            self.stream.slices.max(1)
        };
        if self.stream.hdr && !metadata {
            tracing::warn!(
                "NVENC driver API cannot embed HDR mastering metadata; only control-channel metadata is available. Update the NVIDIA driver if HDR tone mapping is wrong"
            );
        }
        if self.stream.codec == 0 && !tuning.cavlc && !cabac {
            tracing::warn!(
                "NVENC CABAC unavailable; using CAVLC with lower compression efficiency. Update the NVIDIA driver or select HEVC or AV1"
            );
        }
        match self.stream.codec {
            0 => {
                self.config.profileGUID = if self.stream.yuv444 {
                    NV_ENC_H264_PROFILE_HIGH_444_GUID
                } else if ten {
                    NV_ENC_H264_PROFILE_HIGH_10_GUID
                } else {
                    NV_ENC_H264_PROFILE_HIGH_GUID
                };
                // SAFETY: The preset was queried for the H.264 GUID, so this union member is
                // active; it holds only integers and raw pointers.
                let codec = unsafe { &mut self.config.encodeCodecConfig.h264Config };
                codec.set_repeatSPSPPS(1);
                codec.idrPeriod = u32::MAX;
                if slice_mode {
                    codec.sliceMode = 3;
                    codec.sliceModeData = slices;
                }
                codec.chromaFormatIDC = chroma;
                codec.set_enableFillerDataInsertion(u32::from(tuning.filler));
                codec.entropyCodingMode = if cabac {
                    _NV_ENC_H264_ENTROPY_CODING_MODE_NV_ENC_H264_ENTROPY_CODING_MODE_CABAC
                } else {
                    _NV_ENC_H264_ENTROPY_CODING_MODE_NV_ENC_H264_ENTROPY_CODING_MODE_CAVLC
                };
                codec.maxNumRefFrames = self.retained;
                codec.numRefL0 = 1;
                if self.api.modern_depth() {
                    codec.inputBitDepth = depth;
                    codec.outputBitDepth = depth;
                }
                codec.h264VUIParameters = vui(&self.stream);
                codec.set_enableIntraRefresh(u32::from(intra_refresh));
                codec.set_singleSliceIntraRefresh(u32::from(single_slice));
                if intra_refresh {
                    codec.intraRefreshPeriod = 300;
                    codec.intraRefreshCnt = 299;
                    codec.set_outputRecoveryPointSEI(1);
                }
            }
            1 => {
                self.config.profileGUID = if self.stream.yuv444 {
                    NV_ENC_HEVC_PROFILE_FREXT_GUID
                } else if ten {
                    NV_ENC_HEVC_PROFILE_MAIN10_GUID
                } else {
                    NV_ENC_HEVC_PROFILE_MAIN_GUID
                };
                // SAFETY: The preset was queried for the HEVC GUID, so this union member is active;
                // it holds only integers and raw pointers.
                let codec = unsafe { &mut self.config.encodeCodecConfig.hevcConfig };
                codec.set_repeatSPSPPS(1);
                codec.idrPeriod = u32::MAX;
                if slice_mode {
                    codec.sliceMode = 3;
                    codec.sliceModeData = slices;
                }
                codec.set_chromaFormatIDC(chroma);
                codec.set_enableFillerDataInsertion(u32::from(tuning.filler));
                codec.maxNumRefFramesInDPB = self.retained;
                codec.numRefL0 = 1;
                if self.api.modern_depth() {
                    codec.inputBitDepth = depth;
                    codec.outputBitDepth = depth;
                } else if ten {
                    codec.set_reserved3(2);
                }
                codec.hevcVUIParameters = vui(&self.stream);
                codec.set_enableIntraRefresh(u32::from(intra_refresh));
                codec.set_singleSliceIntraRefresh(u32::from(single_slice));
                if intra_refresh {
                    codec.intraRefreshPeriod = 300;
                    codec.intraRefreshCnt = 299;
                    codec.set_outputRecoveryPointSEI(1);
                }
                if self.api >= ApiVersion(13, 0) {
                    codec.set_outputMasteringDisplay(u32::from(metadata));
                    codec.set_outputMaxCll(u32::from(metadata));
                }
            }
            2 => {
                self.config.profileGUID = NV_ENC_AV1_PROFILE_MAIN_GUID;
                // SAFETY: The preset was queried for the AV1 GUID, so this union member is active;
                // it holds only integers and raw pointers.
                let codec = unsafe { &mut self.config.encodeCodecConfig.av1Config };
                codec.set_repeatSeqHdr(1);
                codec.idrPeriod = u32::MAX;
                codec.set_chromaFormatIDC(chroma);
                codec.set_enableBitstreamPadding(u32::from(tuning.filler));
                codec.maxNumRefFramesInDPB = self.retained;
                codec.numFwdRefs = 1;
                if self.api.modern_depth() {
                    codec.inputBitDepth = depth;
                    codec.outputBitDepth = depth;
                } else if ten {
                    codec.set_enableTemporalSVC(1);
                    codec.set_reserved4(1);
                }
                let (primaries, transfer, matrix) = colors(&self.stream);
                codec.colorPrimaries = primaries;
                codec.transferCharacteristics = transfer;
                codec.matrixCoefficients = matrix;
                codec.colorRange = u32::from(self.stream.full_range());
                codec.chromaSamplePosition = u32::from(!self.stream.yuv444);
                if self.stream.slices > 1 {
                    let power = (self.stream.slices as f64).log2();
                    codec.numTileRows = 1 << (power / 2.).ceil() as u32;
                    codec.numTileColumns = 1 << (power / 2.).floor() as u32;
                }
                if self.api >= ApiVersion(13, 0) {
                    codec.set_outputMasteringDisplay(u32::from(metadata));
                    codec.set_outputMaxCll(u32::from(metadata));
                }
            }
            _ => unreachable!(),
        }
        *self.initialize = NV_ENC_INITIALIZE_PARAMS {
            version: self.api.initialize(),
            encodeGUID: guid,
            presetGUID: preset_guid,
            encodeWidth: self.stream.width,
            encodeHeight: self.stream.height,
            darWidth: self.stream.width,
            darHeight: self.stream.height,
            frameRateNum: self.stream.fps_millihz(),
            frameRateDen: 1000,
            enableEncodeAsync: u32::from(asynchronous),
            enablePTD: 1,
            tuningInfo: NV_ENC_TUNING_INFO_NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY,
            encodeConfig: self.config.as_mut(),
            ..Default::default()
        };
        // The D3D12 interface has no weighted prediction (`nvEncodeAPI.h`).
        if tuning.weighted_prediction && self.d3d12.is_some() {
            tracing::warn!(
                "nvenc_weighted_prediction is on, but NVENC on the D3D12 conversion cannot use it; encoding without it. Set gpu_compute_conversion = false to keep it"
            );
        }
        if self.d3d12.is_some() {
            // Read only by the D3D12 interface.
            self.initialize.bufferFormat = buffer_format(&self.stream);
        }
        self.initialize
            .set_enableWeightedPrediction(u32::from(weighted));
        if self.stream.codec != 0
            && self.api.modern_depth()
            && (!(self.stream.codec == 1 && weighted) || split == SPLIT_DISABLED)
        {
            self.initialize.set_splitEncodeMode(split);
        }
        if tuning.qp_map {
            self.config.rcParams.qpMapMode = _NV_ENC_QP_MAP_MODE_NV_ENC_QP_MAP_DELTA;
            self.qp_map = true;
        }
        // Per-block QP for `encoder_focus_verify` and the focus probe.
        self.block_stats = tuning.block_stats
            && self.api.block_stats()
            && self.optional_cap(_NV_ENC_CAPS_NV_ENC_CAPS_OUTPUT_BLOCK_STATS);
        if self.block_stats {
            self.initialize.set_enableOutputStats(1);
            self.initialize.outputStatsLevel =
                _NV_ENC_OUTPUT_STATS_LEVEL_NV_ENC_OUTPUT_STATS_BLOCK_LEVEL;
        }
        self.initialize
            .set_reportSliceOffsets(u32::from(self.subframe));
        self.initialize
            .set_enableSubFrameWrite(u32::from(self.subframe));
        // SAFETY: `validate` checked nvEncInitializeEncoder; `initialize` and the config it points
        // to are boxed in `self`, so both addresses stay valid for the session.
        let status = unsafe {
            (self.functions.nvEncInitializeEncoder.unwrap())(self.raw, self.initialize.as_mut())
        };
        self.check(status, "initialize encoder")?;
        self.initialized = true;
        if let (Some(cuda), Some(set_streams)) = (&self.cuda, self.functions.nvEncSetIOCudaStreams)
            && let Some(stream_ptr) = cuda.stream_ptr()
        {
            // SAFETY: `self.raw` is initialized, `stream_ptr` points to the live `CUstream` in
            // `Rc<cuda::Context>` (`NV_ENC_CUSTREAM_PTR` = `CUstream*`, nvEncodeAPI.h:116,3532),
            // and `self.cuda` drops only after `nvEncDestroyEncoder`.
            let status = unsafe { set_streams(self.raw, stream_ptr, stream_ptr) };
            match self.check(status, "set IO CUDA streams") {
                Ok(()) => cuda.mark_stream_bound(),
                Err(error) => tracing::warn!(
                    %error,
                    "NVENC refused non-blocking CUDA IO stream; falling back to default stream"
                ),
            }
        }
        tracing::info!(
            api_major = self.api.0,
            api_minor = self.api.1,
            preset = budget.effective,
            rate_control = ?self.rate_control,
            engines,
            configured_split = tuning.split,
            split,
            split_reason,
            split_effective = budget.split,
            asynchronous,
            d3d12 = self.d3d12.is_some(),
            subframe_slices = self.subframe_slices,
            references = self.retained,
            rfi = self.invalidation,
            width = self.stream.width,
            height = self.stream.height,
            ten_bit = ten,
            yuv444 = self.stream.yuv444,
            "Rust native NVENC initialized"
        );
        Ok(())
    }
}
