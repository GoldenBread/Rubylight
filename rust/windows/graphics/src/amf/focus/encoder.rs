//! AMF `Encoder` focus setup, startup ROI probe, QP feedback and diagnostic helpers.

use super::*;

impl Encoder {
    /// Focus on with `settings`, or off. On needs the D3D12 conversion and
    /// planar 4:2:0, and fails loudly without them.
    pub fn set_focus(&mut self, settings: Option<crate::compute::FocusSettings>) -> Result<()> {
        if settings.is_some() {
            anyhow::ensure!(
                self.config.chroma_packing == 0,
                "encoder focus: {}",
                Reason::Packed444.message()
            );
            let input = self.compute.as_mut().with_context(|| {
                format!(
                    "encoder focus: {}",
                    Reason::D3d12ConversionUnavailable.message()
                )
            })?;
            input
                .converter
                .enable_focus(encoder_focus::block(self.codec))?;
        }
        if let Some(input) = self.compute.as_mut() {
            input.converter.set_focus(settings)?;
        }
        self.focus.settings = settings;
        Ok(())
    }
    /// Ask for block QP feedback on one frame in `every` that carries a map,
    /// for [`Stats::qp_delta`]. Feedback may cost encoder time.
    pub fn set_focus_verify(&mut self, every: Option<u32>) {
        self.focus.verify = every.map(|every| every.max(1));
    }
    pub fn focus_stats(&self) -> Stats {
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
    /// After `prepare_surface`, before SubmitInput: the newest finished map
    /// from a conversion up to this frame's (`conversion`), unless the frame
    /// is a keyframe, a recovery picture or the encoder's first, or one of the
    /// [`SETTLE_FRAMES`] after them. A map that fails turns focus off and
    /// leaves the error for [`Self::take_focus_failure`]; the frame goes on
    /// without it.
    ///
    /// # Safety
    /// `surface` is this frame's live input surface.
    pub(crate) unsafe fn attach_focus(
        &mut self,
        surface: *mut AMFSurface,
        idr: bool,
        recover: bool,
        conversion: Option<u64>,
    ) -> Result<()> {
        let first = self.index == 0;
        if self.focus.fixed.is_none() && self.focus.settings.is_some() {
            // SAFETY: Per this function's contract.
            let attached =
                unsafe { self.attach_converted(surface, idr, recover, first, conversion) };
            if let Err(error) = attached {
                tracing::warn!(error = %format!("{error:#}"), "encoder focus failed on a frame, so it is off");
                self.focus.fail(error);
            }
            return Ok(());
        }
        self.focus.frame(idr, recover, first, None);
        let (context, codec) = (self.context, self.codec);
        if let Some(fixed) = &self.focus.fixed
            && pick(idr, recover, first, false, Some((u64::MAX, false)), 0) == Pick::Attach
        {
            // SAFETY: Per this function's contract.
            unsafe {
                attach(
                    context,
                    surface,
                    codec,
                    &fixed.values,
                    fixed.width,
                    fixed.height,
                )?
            };
        }
        if self.focus.keep {
            // SAFETY: Per this function's contract.
            unsafe { set(surface, qp_feedback(codec), gpu::boolean(true))? };
            self.focus.asked.push_back(Ask {
                pts: self.index,
                importance: self.focus.fixed.clone(),
                keep: true,
            });
        }
        Ok(())
    }
    /// [`Self::attach_focus`] with focus on.
    ///
    /// # Safety
    /// `surface` is this frame's live input surface.
    unsafe fn attach_converted(
        &mut self,
        surface: *mut AMFSurface,
        idr: bool,
        recover: bool,
        first: bool,
        conversion: Option<u64>,
    ) -> Result<()> {
        let (context, codec) = (self.context, self.codec);
        let input = self
            .compute
            .as_ref()
            .context("encoder focus is on without the D3D12 conversion")?;
        let view = conversion.and_then(|conversion| {
            input
                .converter
                .latest_map(self.focus.last)
                .filter(|view| view.value <= conversion)
        });
        let picked = self.focus.frame(
            idr,
            recover,
            first,
            Some(view.as_ref().map(|view| (view.value, view.uniform))),
        );
        let Some(view) = view.filter(|_| picked == Some(Pick::Attach)) else {
            return Ok(());
        };
        check_grid(codec, self.config.coded_size(), (view.width, view.height))?;
        // SAFETY: Per this function's contract.
        unsafe { attach(context, surface, codec, view.rows, view.width, view.height)? };
        let Some(every) = self.focus.verify else {
            return Ok(());
        };
        self.focus.since_verify += 1;
        if self.focus.since_verify < every {
            return Ok(());
        }
        self.focus.since_verify = 0;
        let importance = Grid {
            width: view.width,
            height: view.height,
            values: view.rows.to_vec(),
        };
        // SAFETY: Per this function's contract.
        unsafe { set(surface, qp_feedback(codec), gpu::boolean(true))? };
        self.focus.asked.push_back(Ask {
            pts: self.index,
            importance: Some(importance),
            keep: false,
        });
        Ok(())
    }
    /// Collect the QP map a frame asked for. With slice output it may come on
    /// any of the frame's units; `last` marks the frame's final one.
    ///
    /// # Safety
    /// `buffer` is a live output buffer of this encoder, for `pts`.
    pub(crate) unsafe fn focus_feedback(&mut self, buffer: *mut AMFBuffer, pts: i64, last: bool) {
        // Asks older than this output belong to frames that were lost.
        while self.focus.asked.front().is_some_and(|ask| ask.pts < pts) {
            self.focus.asked.pop_front();
        }
        if self.focus.asked.front().is_none_or(|ask| ask.pts != pts) {
            return;
        }
        // SAFETY: Per this function's contract.
        let qp = unsafe { read_grid(buffer, qp_map(self.codec)) };
        if matches!(qp, Ok(None)) && !last {
            return;
        }
        let ask = self.focus.asked.pop_front().expect("checked above");
        if let (Ok(Some(qp)), Some(importance)) = (&qp, &ask.importance)
            && let Some(delta) = importance_delta(qp, importance)
        {
            self.focus.qp_sum += f64::from(delta);
            self.focus.qp_count += 1;
        }
        if ask.keep {
            self.focus
                .answers
                .push(qp.map_err(|error| format!("{error:#}")));
        } else if let Err(error) = qp {
            tracing::warn!(error = %format!("{error:#}"), "encoder focus QP feedback unreadable");
        }
    }
    /// The startup check (design section 7): the ROI cap, then frames with the
    /// left half at 10 and the right at 0, and as many the other way round,
    /// each with block QP feedback. An encoder failure is an error; a driver
    /// that ignores the map is `Unsupported`.
    pub fn probe_focus(&mut self) -> Result<Probe> {
        if self.config.chroma_packing != 0 {
            return Ok(Probe::unsupported(
                Reason::Packed444,
                Reason::Packed444.message(),
            ));
        }
        if let Some(cap) = cap_name(self.codec)
            && self.capability(cap) == Some(0)
        {
            return Ok(Probe::unsupported(
                Reason::CodecUnsupported,
                Reason::CodecUnsupported.message(),
            ));
        }
        let (width, height) = coded_size(self.config.coded_size());
        let (columns, rows) = encoder_focus::grid(self.codec, width, height);
        let half = Grid {
            width: columns,
            height: rows,
            values: (0..columns * rows)
                .map(|index| if index % columns < columns / 2 { 10 } else { 0 })
                .collect(),
        };
        self.focus.answers.clear();
        let measured = self.probe_frames(&half);
        self.focus.fixed = None;
        self.focus.keep = false;
        self.focus.asked.clear();
        measured?;
        let answers = std::mem::take(&mut self.focus.answers);
        anyhow::ensure!(
            answers.len() == 2 * PROBE_FRAMES as usize,
            "the focus probe got {} of {} feedback answers",
            answers.len(),
            2 * PROBE_FRAMES
        );
        let mut grids = Vec::with_capacity(answers.len());
        for answer in answers {
            match answer {
                Ok(Some(grid)) => grids.push(grid),
                // Missing feedback leaves both deltas unknown.
                Ok(None) => {
                    grids.clear();
                    break;
                }
                Err(message) => {
                    return Ok(Probe::unsupported(
                        Reason::CodecUnsupported,
                        format!("the block QP map could not be read: {message}"),
                    ));
                }
            }
        }
        let mean = |grids: &[Grid]| -> Option<f32> {
            let deltas = grids.iter().map(half_delta).collect::<Option<Vec<_>>>()?;
            (!deltas.is_empty()).then(|| deltas.iter().sum::<f32>() / deltas.len() as f32)
        };
        let (qp_delta, control_delta) = if grids.is_empty() {
            (None, None)
        } else {
            let (left, right) = grids.split_at(PROBE_FRAMES as usize);
            effect(mean(left), mean(right))
        };
        Ok(Probe {
            support: verdict(self.codec, qp_delta),
            qp_delta,
            control_delta,
            feedback_grid: grids.first().map(|grid| (grid.width, grid.height)),
        })
    }
    fn probe_frames(&mut self, half: &Grid) -> Result<()> {
        let bitrate = self.config.bitrate_kbps;
        let mirrored = Grid {
            values: half.values.iter().map(|value| 10 - value).collect(),
            ..half.clone()
        };
        for frame in 0..PROBE_WARMUP + 2 * PROBE_FRAMES {
            self.focus.keep = frame >= PROBE_WARMUP;
            self.focus.fixed = (frame >= PROBE_WARMUP).then(|| {
                if frame < PROBE_WARMUP + PROBE_FRAMES {
                    half.clone()
                } else {
                    mirrored.clone()
                }
            });
            let image = probe_picture(&self._device, self.config.width, self.config.height, frame)?;
            let started = Instant::now();
            let output = self.encode_gpu(&image, frame == 0, bitrate)?;
            let mut idr = output.iter().any(|encoded| encoded.idr);
            while self.pending() {
                anyhow::ensure!(
                    started.elapsed() < Duration::from_secs(2),
                    "focus probe frame {frame} was not encoded within two seconds"
                );
                let polled = self.poll_now()?;
                if polled.is_empty() {
                    self.wait_after_empty_poll();
                }
                idr |= polled.iter().any(|encoded| encoded.idr);
            }
            anyhow::ensure!(
                !idr || frame == 0,
                "focus probe frame {frame} came out as a keyframe"
            );
        }
        Ok(())
    }
}

/// The row pitch, in bytes, of a host GRAY32 surface of `width`x`height` as
/// [`attach`] allocates it.
#[cfg(test)]
pub(crate) fn map_pitch(context: *mut AMFContext, width: u32, height: u32) -> Result<usize> {
    // SAFETY: The caller's context is live; the owned surface is released after
    // its plane's pitch is read.
    unsafe {
        let mut map = ptr::null_mut();
        check(((*(*context).pVtbl).AllocSurface.unwrap())(
            context,
            AMF_MEMORY_TYPE_AMF_MEMORY_HOST,
            AMF_SURFACE_FORMAT_AMF_SURFACE_GRAY32,
            width as i32,
            height as i32,
            &mut map,
        ))?;
        let plane = ((*(*map).pVtbl).GetPlaneAt.unwrap())(map, 0);
        let pitch = ((*(*plane).pVtbl).GetHPitch.unwrap())(plane) as usize;
        ((*(*map).pVtbl).Release.unwrap())(map);
        Ok(pitch)
    }
}
#[cfg(test)]
impl Encoder {
    pub(crate) fn context_for_tests(&self) -> *mut AMFContext {
        self.context
    }
    /// Diagnostics: `warmup` frames, then one frame per entry of `maps` (`None`
    /// carries no map), each asking for block QP feedback. Per measured frame:
    /// the QP grid and the coded bytes. `grain` adds fresh noise every frame.
    pub(crate) fn focus_diag(
        &mut self,
        warmup: u32,
        maps: &[Option<Grid>],
        grain: bool,
    ) -> Result<Vec<(Option<Grid>, usize)>> {
        self.focus.answers.clear();
        let bitrate = self.config.bitrate_kbps;
        let (width, height) = (self.config.width, self.config.height);
        let mut sizes = Vec::new();
        let run = (|| -> Result<()> {
            for frame in 0..warmup + maps.len() as u32 {
                let measured = frame.checked_sub(warmup).map(|index| index as usize);
                self.focus.keep = measured.is_some();
                self.focus.fixed = measured.and_then(|index| maps[index].clone());
                let image = if grain {
                    probe_picture(&self._device, width, height, frame)?
                } else {
                    smooth_picture(&self._device, width, height, frame)?
                };
                let started = Instant::now();
                let output = self.encode_gpu(&image, frame == 0, bitrate)?;
                let mut size: usize = output.iter().map(|encoded| encoded.bytes.len()).sum();
                let mut idr = output.iter().any(|encoded| encoded.idr);
                while self.pending() {
                    anyhow::ensure!(
                        started.elapsed() < Duration::from_secs(2),
                        "focus diag frame {frame} was not encoded within two seconds"
                    );
                    let polled = self.poll_now()?;
                    if polled.is_empty() {
                        self.wait_after_empty_poll();
                    }
                    for encoded in polled {
                        size += encoded.bytes.len();
                        idr |= encoded.idr;
                    }
                }
                anyhow::ensure!(
                    !idr || frame == 0,
                    "focus diag frame {frame} was a keyframe"
                );
                if measured.is_some() {
                    sizes.push(size);
                }
            }
            Ok(())
        })();
        self.focus.fixed = None;
        self.focus.keep = false;
        self.focus.asked.clear();
        run?;
        let answers = std::mem::take(&mut self.focus.answers);
        anyhow::ensure!(answers.len() == maps.len(), "{} answers", answers.len());
        answers
            .into_iter()
            .zip(sizes)
            .map(|(answer, size)| Ok((answer.map_err(anyhow::Error::msg)?, size)))
            .collect()
    }
}
/// A diagnostic frame without grain: the probe's scrolling cells alone.
#[cfg(test)]
fn smooth_picture(device: &Device, width: u32, height: u32, frame: u32) -> Result<GpuImage> {
    let mut bytes = vec![0u8; width as usize * height as usize * 4];
    for (y, row) in bytes.chunks_exact_mut(width as usize * 4).enumerate() {
        let y = y as u32;
        for (x, pixel) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let x = x as u32;
            let level = (48 + mix(((x + 3 * frame) / 8) | ((y / 8) << 16)) % 160) as u8;
            *pixel = [level, level, level, 255];
        }
    }
    let image = GpuImage::upload(
        device,
        &Image {
            width,
            height,
            stride: width as usize * 4,
            bytes,
            captured: Instant::now(),
            pixel: crate::capture::Pixel::Bgra8,
        },
    )?;
    image.readback(&mut None)?;
    Ok(image)
}
