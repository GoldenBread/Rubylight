//! Encoder focus (ROI QP delta map) control, per-block QP verification,
//! and startup focus probe for NVENC.

use super::*;

// Encoder focus (docs/encoder-focus-design.md) through NVENC's QP delta
// map, with AMF's rules for which frames carry a map.
impl Encoder {
    /// Focus on with `settings`, or off. On needs the D3D12 conversion and
    /// non-packed chroma (4:2:0 or 8-bit native 4:4:4), and fails loudly
    /// without them, as on AMF.
    pub fn set_focus(&mut self, settings: Option<crate::compute::FocusSettings>) -> Result<()> {
        if settings.is_some() {
            anyhow::ensure!(
                self.config.chroma_packing == 0,
                "encoder focus: {}",
                Reason::Packed444.message()
            );
            let converter = self.compute.as_deref_mut().with_context(|| {
                format!(
                    "encoder focus: {}",
                    Reason::D3d12ConversionUnavailable.message()
                )
            })?;
            converter.enable_focus(encoder_focus::block(self.config.codec))?;
            self.session.enable_qp_map()?;
        }
        if let Some(converter) = self.compute.as_deref_mut() {
            converter.set_focus(settings)?;
        }
        self.focus.settings = settings;
        self.last_converted = None;
        Ok(())
    }
    /// Read block QP statistics on one frame in `every` that carries a map,
    /// for [`encoder_focus::Stats::qp_delta`]. Needs `encoder_focus_verify`
    /// when the encoder is made, since NVENC turns statistics on only then.
    pub fn set_focus_verify(&mut self, every: Option<u32>) {
        self.focus.verify = every.map(|every| every.max(1));
    }
    pub fn focus_stats(&self) -> encoder_focus::Stats {
        let mut stats = self.focus.stats;
        stats.qp_delta = (self.focus.qp_count > 0)
            .then(|| (self.focus.qp_sum / f64::from(self.focus.qp_count)) as f32);
        stats
    }
    /// Why a map failed on a frame since the last call. Focus is then off in
    /// this encoder; the stream turns it off for the session and says why.
    pub fn take_focus_failure(&mut self) -> Option<anyhow::Error> {
        self.focus.failure.take()
    }
    /// Before a submission asked as `idr`: the newest finished map from a
    /// conversion up to this frame's (`conversion`) as the frame's delta map,
    /// unless the frame is a keyframe, a recovery picture or the encoder's
    /// first, or one of the frames settling after them. A map that fails
    /// turns focus off and leaves the error for [`Self::take_focus_failure`];
    /// the frame goes on without it.
    pub(super) fn attach_focus(&mut self, idr: bool, conversion: Option<u64>) {
        let idr = self.session.keyframe(idr);
        let recover = self.session.references.confirming(idr);
        let first = self.last_submitted.is_none();
        if self.focus.fixed.is_none() && self.focus.settings.is_some() {
            if let Err(error) = self.attach_converted(idr, recover, first, conversion) {
                tracing::warn!(error = %format!("{error:#}"), "encoder focus failed on a frame, so it is off");
                self.focus.fail(error);
            }
            return;
        }
        self.focus.frame(idr, recover, first, None);
        let frame = self.session.next;
        if let Some(fixed) = &self.focus.fixed
            && encoder_focus::attach_for(idr, recover, first, false)
        {
            // The last map's buffer, refilled.
            let mut deltas = std::mem::take(&mut self.session.map_scratch);
            match butterpollo_media_policy::nvenc::focus_deltas(
                self.config.codec,
                (self.session.stream.width, self.session.stream.height),
                &fixed.values,
                (fixed.width, fixed.height),
                &mut deltas,
            ) {
                Ok(()) => self.session.next_map = Some(deltas),
                Err(error) => {
                    self.session.map_scratch = deltas;
                    tracing::warn!(%error, "encoder focus probe map refused");
                }
            }
        }
        if self.probing {
            self.session.measure_next = true;
            self.asked.push_back(Ask {
                frame,
                importance: self.focus.fixed.clone(),
                keep: true,
            });
        }
    }
    /// [`Self::attach_focus`] with focus on.
    fn attach_converted(
        &mut self,
        idr: bool,
        recover: bool,
        first: bool,
        conversion: Option<u64>,
    ) -> Result<()> {
        let converter = self
            .compute
            .as_deref()
            .context("encoder focus is on without the D3D12 conversion")?;
        let view = conversion.and_then(|conversion| {
            converter
                .latest_map(self.focus.last)
                .filter(|view| view.value <= conversion)
        });
        let picked = self.focus.frame(
            idr,
            recover,
            first,
            Some(view.as_ref().map(|view| (view.value, view.uniform))),
        );
        let Some(view) = view.filter(|_| picked == Some(crate::amf::focus::Pick::Attach)) else {
            return Ok(());
        };
        // The last map's buffer, refilled.
        let mut deltas = std::mem::take(&mut self.session.map_scratch);
        match butterpollo_media_policy::nvenc::focus_deltas(
            self.config.codec,
            (self.session.stream.width, self.session.stream.height),
            view.rows,
            (view.width, view.height),
            &mut deltas,
        ) {
            Ok(()) => self.session.next_map = Some(deltas),
            Err(error) => {
                self.session.map_scratch = deltas;
                return Err(anyhow::Error::msg(error));
            }
        }
        let Some(every) = self.focus.verify.filter(|_| self.session.block_stats) else {
            return Ok(());
        };
        self.focus.since_verify += 1;
        if self.focus.since_verify < every {
            return Ok(());
        }
        self.focus.since_verify = 0;
        self.session.measure_next = true;
        self.asked.push_back(Ask {
            frame: self.session.next,
            importance: Some(crate::amf::focus::Grid {
                width: view.width,
                height: view.height,
                values: view.rows.to_vec(),
            }),
            keep: false,
        });
        Ok(())
    }
    /// Pair the block QP grids the session read with the frames that asked.
    pub(super) fn focus_feedback(&mut self) {
        while let Some((frame, qp)) = self.session.qp_grids.pop_front() {
            // Asks older than this frame belong to frames that were lost.
            while self.asked.front().is_some_and(|ask| ask.frame < frame) {
                self.asked.pop_front();
            }
            if self.asked.front().is_none_or(|ask| ask.frame != frame) {
                continue;
            }
            let ask = self.asked.pop_front().expect("checked above");
            if let Some(importance) = &ask.importance
                && let Some(delta) = crate::amf::focus::importance_delta(&qp, importance)
            {
                self.focus.qp_sum += f64::from(delta);
                self.focus.qp_count += 1;
            }
            if ask.keep {
                self.answers.push(qp);
            }
        }
    }
    /// The startup check, as AMF's: frames with the left half at 10 and the
    /// right at 0, and as many the other way round, each read back as block
    /// QP. It runs on a session of its own with the delta map and block
    /// statistics, so the codec check never depends on either. NVENC caps
    /// the sessions open at once (GeForce drivers allow a few), so that
    /// session takes the stream session's place rather than opening beside
    /// it, and the stream session opens again after. An encoder failure is
    /// an error; a driver that ignores the map, or returns no statistics, is
    /// `Unsupported`.
    ///
    /// Only for the startup check's encoder (`state.rs`): the session that
    /// opens again keeps the HDR metadata, but not what a stream sets on it
    /// (frame cap, scene-cut limit, joined slices, the delta map), so an
    /// encoder with any of those is refused.
    pub fn probe_focus(&mut self) -> Result<crate::amf::focus::Probe> {
        use crate::amf::focus::Probe;
        if self.config.chroma_packing != 0 {
            return Ok(Probe::unsupported(
                Reason::Packed444,
                Reason::Packed444.message(),
            ));
        }
        if self.session.cuda.is_some() || !supports_compute(&self.config) {
            return Ok(Probe::unsupported(
                Reason::D3d12ConversionUnavailable,
                Reason::D3d12ConversionUnavailable.message(),
            ));
        }
        let mut tuning = self.tuning.clone();
        tuning.qp_map = true;
        tuning.block_stats = true;
        self.session.startup_only()?;
        let stream = self.session.stream.clone();
        let metadata = std::mem::take(&mut self.session.metadata);
        let last_submitted = self.last_submitted.take();
        self.close_session();
        let probed = self.probe_session(&stream, &tuning);
        self.close_session();
        let tuning = self.tuning.clone();
        self.session = self
            .open_session(&stream, &tuning)
            .context("NVENC could not open the stream session again after the focus probe")?;
        self.session.metadata = metadata;
        if self.dynamic.is_some() {
            self.session.dynamic_headroom = DYNAMIC_HEADROOM;
        }
        // The new session's first picture is a keyframe the stream asks for.
        self.session.force_idr = true;
        self.last_submitted = last_submitted;
        probed
    }
    /// A session on the stream session's device, so the probe's frames take
    /// the same conversion.
    fn open_session(&self, stream: &Negotiated, tuning: &Tuning) -> Result<Session> {
        let runtime = Runtime::load()?;
        let (device, fence) = directx_device(self.compute.as_deref(), &self.gpu)?;
        Session::open(
            runtime,
            device,
            _NV_ENC_DEVICE_TYPE_NV_ENC_DEVICE_TYPE_DIRECTX,
            None,
            fence,
            stream,
            tuning,
        )
    }
    /// [`Self::probe_focus`] once the stream session is closed: the probe
    /// session is left in its place.
    fn probe_session(
        &mut self,
        stream: &Negotiated,
        tuning: &Tuning,
    ) -> Result<crate::amf::focus::Probe> {
        use crate::amf::focus::Probe;
        let session = match self.open_session(stream, tuning) {
            Ok(session) if session.block_stats => session,
            Ok(_) => {
                return Ok(Probe::unsupported(
                    Reason::CodecUnsupported,
                    "this NVENC driver returns no block QP statistics (API 12.1, NVIDIA driver 531.61 or newer), so ROI cannot be verified",
                ));
            }
            Err(error) => {
                return Ok(Probe::unsupported(
                    Reason::CodecUnsupported,
                    format!("NVENC refused a session with the QP delta map: {error:#}"),
                ));
            }
        };
        let (columns, rows) = encoder_focus::grid(
            self.config.codec,
            self.session.stream.width,
            self.session.stream.height,
        );
        let half = crate::amf::focus::Grid {
            width: columns,
            height: rows,
            values: (0..columns * rows)
                .map(|index| if index % columns < columns / 2 { 10 } else { 0 })
                .collect(),
        };
        self.session = session;
        self.answers.clear();
        let measured = self.probe_frames(&half);
        self.focus.fixed = None;
        self.probing = false;
        self.asked.clear();
        measured?;
        Ok(probe_verdict(
            self.config.codec,
            std::mem::take(&mut self.answers),
        ))
    }
    fn probe_frames(&mut self, half: &crate::amf::focus::Grid) -> Result<()> {
        use crate::amf::focus::{PROBE_FRAMES, PROBE_WARMUP};
        let bitrate = self.config.bitrate_kbps;
        let mirrored = crate::amf::focus::Grid {
            values: half.values.iter().map(|value| 10 - value).collect(),
            ..half.clone()
        };
        for frame in 0..PROBE_WARMUP + 2 * PROBE_FRAMES {
            self.probing = frame >= PROBE_WARMUP;
            self.focus.fixed = (frame >= PROBE_WARMUP).then(|| {
                if frame < PROBE_WARMUP + PROBE_FRAMES {
                    half.clone()
                } else {
                    mirrored.clone()
                }
            });
            let image = crate::amf::focus::probe_picture(
                &self.gpu,
                self.config.width,
                self.config.height,
                frame,
            )?;
            let started = Instant::now();
            let mut idr = self
                .encode_gpu(&image, frame == 0, bitrate)?
                .iter()
                .any(|encoded| encoded.idr);
            while self.pending() {
                anyhow::ensure!(
                    started.elapsed() < Duration::from_secs(2),
                    "focus probe frame {frame} was not encoded within two seconds"
                );
                let polled = self.poll()?;
                idr |= polled.iter().any(|encoded| encoded.idr);
                if polled.is_empty() && self.pending() {
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
            anyhow::ensure!(
                !idr || frame == 0,
                "focus probe frame {frame} came out as a keyframe"
            );
        }
        Ok(())
    }
}

/// The focus probe's verdict from its QP grids, in frame order: the first
/// half with the left half important, the rest mirrored. Missing grids
/// leave the effect unknown, which is `Unsupported`.
pub(super) fn probe_verdict(
    codec: u8,
    grids: Vec<crate::amf::focus::Grid>,
) -> crate::amf::focus::Probe {
    use crate::amf::focus::{PROBE_FRAMES, effect, half_delta, verdict};
    let mean = |grids: &[crate::amf::focus::Grid]| -> Option<f32> {
        let deltas = grids.iter().map(half_delta).collect::<Option<Vec<_>>>()?;
        (!deltas.is_empty()).then(|| deltas.iter().sum::<f32>() / deltas.len() as f32)
    };
    let (qp_delta, control_delta) = if grids.len() == 2 * PROBE_FRAMES as usize {
        let (left, right) = grids.split_at(PROBE_FRAMES as usize);
        effect(mean(left), mean(right))
    } else {
        (None, None)
    };
    crate::amf::focus::Probe {
        support: verdict(codec, qp_delta),
        qp_delta,
        control_delta,
        feedback_grid: grids.first().map(|grid| (grid.width, grid.height)),
    }
}
