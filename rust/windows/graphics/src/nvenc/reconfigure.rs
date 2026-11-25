//! NVENC dynamic reconfiguration (bitrate, VBV frame cap, QP delta map)
//! and reference frame invalidation.

use super::*;

impl Session {
    /// Turn the delta map on for encoder focus: a reconfigure without a
    /// reset or keyframe, with frames in flight.
    pub(super) fn enable_qp_map(&mut self) -> Result<()> {
        if self.qp_map {
            return Ok(());
        }
        if self.functions.nvEncReconfigureEncoder.is_none() {
            bail!("this NVENC driver cannot turn on the QP delta map");
        }
        let _guard = self.guard()?;
        let mut config = Box::new(*self.config);
        config.rcParams.qpMapMode = _NV_ENC_QP_MAP_MODE_NV_ENC_QP_MAP_DELTA;
        self.apply(config, false, "turn on the QP delta map")?;
        self.qp_map = true;
        Ok(())
    }
    /// The bound the session targets for every frame: its one-frame VBV, or
    /// the host's cap when that is larger, since the VBV aims under it.
    /// `None` when the driver controls buffering.
    pub(super) fn frame_cap(&self) -> Option<u64> {
        let vbv = u64::from(self.config.rcParams.vbvBufferSize).div_ceil(8);
        (vbv > 0).then(|| vbv.max(self.frame_cap_target.unwrap_or(0)))
    }
    /// The host's pace-based frame cap, which AMF writes as its frame size
    /// limit. NVENC targets it for every frame, keyframes too, with
    /// `lowDelayKeyFrameScale` 1 and a one-frame VBV, as Sunshine does; the
    /// driver does not guarantee the bound, so `collect` warns about a frame
    /// over twice it. The VBV is under the cap on any link that carries the
    /// stream; a cap under the VBV lowers the VBV to it. Changes within a
    /// tenth are skipped and the VBV returns to its ceiling at once, so a
    /// steady link never reconfigures.
    pub(super) fn set_frame_cap(&mut self, bytes: u64) -> Result<Option<u64>> {
        if bytes == 0 {
            return Ok(self.frame_cap());
        }
        self.frame_cap_target = Some(bytes);
        let current = self.config.rcParams.vbvBufferSize;
        if self.vbv_ceiling == 0 || !self.dynamic_bitrate || current == 0 {
            return Ok(self.frame_cap());
        }
        let wanted = bytes.saturating_mul(8).clamp(
            MIN_VBV_BITS,
            u64::from(self.vbv_ceiling.max(MIN_VBV_BITS as u32)),
        ) as u32;
        let small = u64::from(wanted.abs_diff(current)) * 10 <= u64::from(current);
        if wanted == current || (small && wanted != self.vbv_ceiling) {
            self.cap_refused = false;
            return Ok(self.frame_cap());
        }
        // A cap the driver refused is not asked for again each frame, and stays refused.
        if self.cap_requested == Some(bytes) {
            return Ok(self.frame_cap());
        }
        self.cap_requested = Some(bytes);
        let reconfigured = self.reconfigure(
            self.config.rcParams.averageBitRate,
            wanted,
            false,
            "frame cap",
        );
        self.cap_refused = reconfigured.is_err();
        reconfigured?;
        Ok(self.frame_cap())
    }
    pub(super) fn queue_frame_cap(&mut self, bytes: u64) -> Option<u64> {
        if bytes > 0 {
            self.frame_cap_target = Some(bytes);
        }
        self.frame_cap()
    }
    #[cfg(test)]
    pub(super) fn bitrate(&mut self, bitrate: u32) -> Result<()> {
        let _ = self.apply_bitrate(bitrate, false)?;
        Ok(())
    }
    pub(super) fn apply_bitrate(
        &mut self,
        bitrate: u32,
        with_cap: bool,
    ) -> Result<Result<(), anyhow::Error>> {
        if bitrate == 0 {
            bail!("NVENC bitrate out of range");
        }
        let requested = bitrate;
        let bitrate = bitrate.min(butterpollo_media_policy::rtsp::MAX_BITRATE_KBPS);
        if bitrate != requested {
            tracing::warn!(
                requested,
                bitrate,
                "NVENC bitrate above the stream's maximum; using the maximum"
            );
        }
        if bitrate == self.stream.bitrate_kbps {
            return Ok(match (with_cap, self.frame_cap_target) {
                (true, Some(bytes)) => self.set_frame_cap(bytes).map(|_| ()),
                _ => Ok(()),
            });
        }
        if self.functions.nvEncReconfigureEncoder.is_none() {
            bail!("NVENC driver cannot change bitrate in this session");
        }
        let before = self.config.rcParams.averageBitRate;
        let after = bitrate * 1000;
        let ceiling = if self.vbv_ceiling > 0 {
            Tuning::vbv_bits(bitrate, self.stream.fps_millihz(), self.vbv_increase)
                .max(MIN_VBV_BITS as u32)
        } else {
            0
        };
        let scale = |bits: u32| {
            if bits > 0 && before > 0 {
                (u64::from(after) * u64::from(bits) / u64::from(before))
                    .clamp(MIN_VBV_BITS, u64::from(u32::MAX)) as u32
            } else {
                bits
            }
        };
        let scaled_vbv = if ceiling > 0 {
            scale(self.config.rcParams.vbvBufferSize).min(ceiling)
        } else {
            scale(self.config.rcParams.vbvBufferSize)
        };
        let capped_vbv = (with_cap && ceiling > 0 && self.dynamic_bitrate)
            .then(|| {
                self.frame_cap_target.map(|bytes| {
                    bytes
                        .saturating_mul(8)
                        .clamp(MIN_VBV_BITS, u64::from(ceiling.max(MIN_VBV_BITS as u32)))
                        as u32
                })
            })
            .flatten();
        let reset = !self.dynamic_bitrate;
        let mut cap_result = Ok(());
        let mut applied_cap = capped_vbv.is_some();
        if let Err(error) = self.reconfigure(
            after,
            capped_vbv.unwrap_or(scaled_vbv),
            reset,
            "change bitrate",
        ) {
            if reset {
                return Err(error);
            }
            if capped_vbv.is_some_and(|vbv| vbv != scaled_vbv) {
                if let Err(err) = self.reconfigure(after, scaled_vbv, false, "change bitrate") {
                    tracing::warn!(error = %format!("{err:#}"), bitrate, "NVENC refused a bitrate change in place; resetting the encoder with a keyframe");
                    self.reconfigure(after, scaled_vbv, true, "change bitrate with a reset")?;
                }
                cap_result = Err(error);
            } else {
                tracing::warn!(error = %format!("{error:#}"), bitrate, "NVENC refused a bitrate change in place; resetting the encoder with a keyframe");
                self.reconfigure(after, scaled_vbv, true, "change bitrate with a reset")?;
                applied_cap = false;
            }
        }
        self.vbv_ceiling = ceiling;
        self.cap_requested = applied_cap.then_some(self.frame_cap_target).flatten();
        self.cap_refused = cap_result.is_err();
        self.stream.bitrate_kbps = bitrate;
        Ok(cap_result)
    }
    /// Reconfigure the rate control to `bitrate` bits per second and a VBV of
    /// `vbv` bits. With `reset` the encoder resets and forces a keyframe,
    /// after the frames in flight are collected. Without it the change
    /// applies with frames in flight, as FFmpeg reconfigures, so it adds no
    /// wait and no keyframe.
    fn reconfigure(
        &mut self,
        bitrate: u32,
        vbv: u32,
        reset: bool,
        operation: &'static str,
    ) -> Result<()> {
        if reset {
            self.drain()?;
        }
        let _guard = self.guard()?;
        let mut config = Box::new(*self.config);
        config.rcParams.averageBitRate = bitrate;
        config.rcParams.maxBitRate = self.rate_control.max_bitrate(bitrate);
        config.rcParams.vbvBufferSize = vbv;
        if vbv > 0 {
            config.rcParams.vbvInitialDelay = vbv;
        }
        self.apply(config, reset, operation)
    }
    /// Hand the driver `config` in place of the session's. `reset` resets
    /// the encoder and forces a keyframe.
    fn apply(
        &mut self,
        mut config: Box<NV_ENC_CONFIG>,
        reset: bool,
        operation: &'static str,
    ) -> Result<()> {
        let mut reconfigure = NV_ENC_RECONFIGURE_PARAMS {
            version: self.api.reconfigure(),
            reInitEncodeParams: *self.initialize,
            ..Default::default()
        };
        reconfigure.reInitEncodeParams.encodeConfig = config.as_mut();
        reconfigure.set_resetEncoder(u32::from(reset));
        reconfigure.set_forceIDR(u32::from(reset));
        self.check(
            // SAFETY: Callers check that nvEncReconfigureEncoder exists (`bitrate`,
            // `dynamic_bitrate` or `enable_qp_map`), and the boxed `config` is kept as
            // `self.config` after the call.
            unsafe {
                (self.functions.nvEncReconfigureEncoder.unwrap())(self.raw, &mut reconfigure)
            },
            operation,
        )?;
        self.config = config;
        self.initialize.encodeConfig = self.config.as_mut();
        self.force_idr |= reset;
        Ok(())
    }
    pub(super) fn invalidate(&mut self, first: u64, last: u64) -> Result<bool> {
        if !self.invalidation || first == 0 || last < first || last >= self.next {
            return Ok(false);
        }
        if last < self.last_idr
            || self
                .references
                .plan(first, last, self.retained, self.invalidation)
                == Recovery::AlreadyApplied
        {
            return Ok(true);
        }
        if first <= self.last_idr
            && !self
                .pending
                .iter()
                .any(|p| p.frame > last && (p.idr || p.cut_limit.is_some()))
        {
            self.rfi_fallbacks += 1;
            tracing::debug!(
                first,
                last,
                fallbacks = self.rfi_fallbacks,
                "NVENC RFI span crosses keyframe; falling back to IDR"
            );
            return Ok(false);
        }
        let started = Instant::now();
        self.drain()?;
        let drain_ms = started.elapsed().as_secs_f64() * 1e3;
        let _guard = self.guard()?;
        match self
            .references
            .plan(first, last, self.retained, self.invalidation)
        {
            Recovery::AlreadyApplied => {
                tracing::debug!(
                    first,
                    last,
                    drain_ms,
                    "NVENC RFI drain completed; invalidation already covered"
                );
                Ok(true)
            }
            Recovery::Idr => {
                self.rfi_fallbacks += 1;
                tracing::debug!(
                    first,
                    last,
                    drain_ms,
                    fallbacks = self.rfi_fallbacks,
                    "NVENC RFI drain finished; falling back to IDR"
                );
                Ok(false)
            }
            Recovery::Invalidate { first, last } => {
                for frame in first..=last {
                    self.check(
                        // SAFETY: `plan` returns Invalidate only when `self.invalidation`, which
                        // requires this entry, is set.
                        unsafe {
                            (self.functions.nvEncInvalidateRefFrames.unwrap())(self.raw, frame)
                        },
                        "invalidate lost reference",
                    )?;
                }
                self.references.applied(first, last);
                tracing::debug!(
                    first,
                    last,
                    drain_ms,
                    "NVENC RFI invalidated reference frames"
                );
                Ok(true)
            }
        }
    }
}
