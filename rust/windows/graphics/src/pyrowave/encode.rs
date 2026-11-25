//! PyroWave frame submission, GPU encode scheduling, completion polling and packet readout.

use super::*;

impl Encoder {
    /// A CPU image, such as the codec probe's: its frame comes back from this
    /// call, with any frame still in flight.
    pub fn encode(&mut self, image: &Image, idr: bool, kbps: u32) -> Result<Vec<Encoded>> {
        anyhow::ensure!(
            self.overlay.is_none(),
            "couch menu: needs wgc_direct_encoder_input"
        );
        let mut frames = self.encode_gpu(&GpuImage::upload(&self.d3d, image)?, idr, kbps)?;
        frames.extend(self.finish()?);
        Ok(frames)
    }
    /// Submits `image` and returns the frames done by now, oldest first. With
    /// one frame in flight that is this one; with two, usually the one before
    /// it, and this one comes from [`Self::poll`].
    pub fn encode_gpu(&mut self, image: &GpuImage, _idr: bool, kbps: u32) -> Result<Vec<Encoded>> {
        let result = self.encode_gpu_inner(image, kbps);
        self.settled(result)
    }
    /// The frames done by now, oldest first.
    pub fn poll(&mut self) -> Result<Vec<Encoded>> {
        let result = self.collect(false);
        self.settled(result)
    }
    /// Waits for every frame in flight and returns them, oldest first.
    pub fn finish(&mut self) -> Result<Vec<Encoded>> {
        let result = self.collect(true);
        self.settled(result)
    }
    pub fn pending(&self) -> bool {
        !self.flights.is_empty()
    }
    /// Frames submitted and not yet returned.
    pub fn backlog(&self) -> usize {
        self.flights.len()
    }
    pub fn last_submitted(&self) -> Option<Instant> {
        self.submitted
    }
    /// An event set once the oldest frame in flight is done, armed by this
    /// call; `None` when nothing is in flight or there is no event, and the
    /// caller keeps its output poll, which stays the bound of any wait.
    pub fn output_event(&self) -> Option<&crate::timing::Signal> {
        let event = self.output_event.as_ref()?;
        match self.arm_output(event) {
            Ok(armed) => armed.then_some(event),
            Err(error) => {
                self.warnings.event(
                    "pyrowave_output_event",
                    &format!("PyroWave could not arm its output event ({error:#}); polling for output instead. Report this with the host log."),
                    butterpollo_media_policy::EVENT_PERIOD,
                );
                None
            }
        }
    }
    fn arm_output(&self, event: &crate::timing::Signal) -> Result<bool> {
        let Some(oldest) = self.flights.front() else {
            return Ok(false);
        };
        let fence = &self.slots[oldest.slot].sync.fence;
        let wanted = (oldest.slot, oldest.release);
        // SAFETY: The fence is the slot's live shared fence, which outlives the
        // frames in flight; `event` is an open event this encoder keeps until
        // Drop or until it arms it again.
        unsafe {
            if fence.GetCompletedValue() >= oldest.release {
                event.set()?;
            } else if self.armed.get() != Some(wanted) {
                event.reset()?;
                fence.SetEventOnCompletion(oldest.release, event.raw())?;
                self.armed.set(Some(wanted));
            } else if event.is_set() {
                event.reset()?;
                // Done meanwhile: its notification may have come before the reset.
                if fence.GetCompletedValue() >= oldest.release {
                    event.set()?;
                }
            }
        }
        Ok(true)
    }
    /// A submission whose completion is not confirmed, or a frame whose
    /// conversion or submission failed, still holds its resources.
    pub(super) fn unsettled(&self) -> bool {
        self.submitting.is_some() || self.flights.unconfirmed
    }
    /// Quarantines the adapter when `result` failed with GPU work unconfirmed.
    pub(super) fn settled<T>(&self, result: Result<T>) -> Result<T> {
        if result.is_err() && self.unsettled() {
            self.quarantine();
        }
        result
    }
    pub(super) fn quarantine(&self) {
        quarantined_adapters().block(self.adapter);
        if let Some(compute) = &self.compute {
            compute.compute().retire();
        }
        self.warnings.set("pyrowave_gpu_quarantine", "PyroWave stopped after GPU work could not be confirmed complete. Its resources are retained to protect capture; restart Rubylight before using PyroWave on this adapter again, or use HEVC/AV1.");
    }
    /// The frames done by now, oldest first; with `block`, every frame in
    /// flight, waiting for each.
    fn collect(&mut self, block: bool) -> Result<Vec<Encoded>> {
        let mut frames = Vec::new();
        while let Some(frame) = self.complete(block)? {
            frames.extend(frame);
        }
        Ok(frames)
    }
    /// The oldest frame in flight, read out once Vulkan confirms it complete:
    /// `None` while it is not (or nothing is in flight), `Some(None)` when it
    /// is dropped over the transport limit.
    fn complete(&mut self, block: bool) -> Result<Option<Option<Encoded>>> {
        let completion = self
            .completion
            .as_ref()
            .context("PyroWave completion wait")?;
        let slots = &self.slots;
        let completed = self.flights.complete(|flight| {
            settle(
                |timeout| {
                    completion.reached(slots[flight.slot].semaphore(), flight.release, timeout)
                },
                flight.submitted,
                Instant::now(),
                block,
            )
        });
        let completed = match completed {
            Ok(Some(completed)) => completed,
            Ok(None) => return Ok(None),
            Err(error) => {
                // A Vulkan loss (VK_ERROR_DEVICE_LOST) keeps the lease and
                // quarantines: nothing proves the driver stopped reading the
                // imported capture texture, unlike a D3D loss below.
                if let Some(loss) = crate::device_loss::DeviceLost::from_error(&error) {
                    self.lost = Some(loss);
                }
                return Err(error);
            }
        };
        // The lease is free whatever the wait proved: Vulkan never reads the
        // capture texture, only the encoder-owned planes of the slot. Ask now
        // whether a device died under the frame: a lost compute device
        // signals its fences to the maximum, which can satisfy that wait
        // before Vulkan has run the encode (Drop idles Vulkan before it
        // destroys anything). The output is garbage, and the host rebuilds on
        // a new device; the adapter is not quarantined. On the graphics path
        // (compute planes present, frame not shareable) the D3D11 conversion
        // may still be reading the capture when the lease is returned;
        // capture can then tear a buffer that is discarded anyway. This is
        // the second GetDeviceRemovedReason pair per frame (the first is the
        // entry check). Both stay: the entry check keeps a dead device from
        // being handed new work, this one is the only check that sees a loss
        // during the frame. Each is a runtime flag read on two devices, not a
        // GPU round trip; not measured.
        if let Some(loss) = self.device_removed() {
            self.device_lost(loss);
            return Err(loss.into());
        }
        self.read_out(completed).map(Some)
    }
    fn encode_gpu_inner(&mut self, image: &GpuImage, kbps: u32) -> Result<Vec<Encoded>> {
        // Refuse a device already known or found lost before it gets new work.
        if let Some(loss) = self.device_removed() {
            self.device_lost(loss);
            return Err(loss.into());
        }
        quarantined_adapters().check(self.adapter)?;
        anyhow::ensure!(
            !self.unsettled(),
            "PyroWave encoder has an unconfirmed GPU submission; rebuild it before encoding again"
        );
        if !self.accepts_gpu_device(image) {
            bail!("capture device changed");
        }
        let begin = Instant::now();
        let mut frames = Vec::new();
        let source = (image.width, image.height, image.pixel);
        if source_rebuilds_slots(self.source, source) {
            // Only the D3D11 fallback converter depends on source format/size;
            // keep Vulkan PlaneImages and D3D12 ComputeTargets intact.
            if self.slots.iter().any(|slot| slot.converter.is_some()) {
                frames.extend(self.collect(true)?);
                for slot in &mut self.slots {
                    slot.converter = None;
                }
            }
            self.repeat_cache.clear();
        }
        self.source = Some(source);
        let repeat = std::mem::take(&mut self.repeat);
        let interval = self.interval.next(begin, repeat);
        let budget = pyrowave::budget_with_minimum(
            kbps,
            interval,
            self.config.packet_size,
            self.config.pyrowave_records,
            self.critical_fec_percentage,
            self.config.min_fec,
        );
        if budget < 16 {
            frames.extend(self.collect(false)?);
            return Ok(frames);
        }
        let limit = pyrowave::max_frame_bytes(self.config.packet_size, self.critical_fec);
        let cache_repeat = if !repeat {
            self.repeat_cache.clear();
            None
        } else if self.config.pyrowave_records {
            let pending_match = self.flights.queue.iter().any(|flight| {
                flight.cache_repeat.as_ref().is_some_and(|key| {
                    key.matches(image, self.luminance, self.overlay.as_ref(), kbps)
                })
            });
            if !self.flights.is_empty()
                && (pending_match
                    || self.repeat_cache.matches(
                        image,
                        self.luminance,
                        self.overlay.as_ref(),
                        kbps,
                        limit,
                    ))
            {
                frames.extend(self.collect(true)?);
            }
            if let Some(bytes) =
                self.repeat_cache
                    .try_hit(image, self.luminance, self.overlay.as_ref(), kbps, limit)
            {
                self.submitted = Some(Instant::now());
                frames.push(Encoded {
                    bytes,
                    framing: None,
                    unit: crate::encoder::Unit::Whole,
                    idr: true,
                    after_invalidation: false,
                    frame_cap_bytes: None,
                    reencoded: false,
                    frame_cap_target: None,
                    latency: Some(begin.elapsed()),
                    presentation: Some(image.captured),
                });
                return Ok(frames);
            }
            Some(RepeatKey::new(
                image,
                self.luminance,
                self.overlay.as_ref(),
                kbps,
            ))
        } else {
            None
        };
        // A caller past the backlog waits for the oldest frame here.
        while self.flights.len() >= self.depth {
            match self.complete(true)? {
                Some(frame) => frames.extend(frame),
                None => break,
            }
        }
        let slot = self
            .flights
            .free_slot(self.depth)
            .context("no free PyroWave frame slot")?;
        if slot == self.slots.len() {
            let compute_device = self.compute.as_ref().map(|c| c.compute().clone());
            self.slots.push(timed("PyroWave slot create", || {
                Slot::new(
                    &self.api,
                    &self.d3d,
                    self.device,
                    &self.config,
                    Some(source),
                    compute_device.as_ref(),
                    &self.warnings,
                )
            })?);
        }
        let release = self.submit(slot, image, budget)?;
        let submitted = Instant::now();
        self.submitted = Some(submitted);
        // SDK 502a3b52 submits the metadata/bitstream host copies and their
        // COPY→HOST barrier before this same-queue release signal. Granite
        // signals it at ALL_COMMANDS, so this covers the output writes as
        // well as the input reads; the getters of the readout submit no GPU
        // work.
        #[cfg(test)]
        if let Some(fault) = self.hooks.after_submit.take() {
            fault();
        }
        let lease = self.submitting.take().context("PyroWave capture lease")?;
        self.flights.push(Flight {
            lease,
            slot,
            release,
            begin,
            submitted,
            captured: image.captured,
            plan: Plan {
                budget,
                kbps,
                interval,
            },
            seq_offset: self.repeat_cache.seq_offset(),
            cache_repeat,
        });
        // One frame in flight waits for it here, as the encode always did.
        frames.extend(self.collect(self.depth == 1)?);
        Ok(frames)
    }
    /// Converts `image` into `slot`'s planes and submits their encode;
    /// returns the fence value the encode signals when done.
    fn submit(&mut self, slot: usize, image: &GpuImage, budget: usize) -> Result<u64> {
        #[cfg(test)]
        if let Some(fault) = self.hooks.before_conversion.take() {
            fault();
        }
        let Slot {
            compute,
            planes,
            sync,
            converter,
            textures,
        } = &mut self.slots[slot];
        let images: [p::pyrowave_gpu_external_reference; 3] =
            std::array::from_fn(|plane| p::pyrowave_gpu_external_reference {
                image: planes[plane].image,
                queue_family_index: 0xfffffffe,
            });
        let views = [planes[0].view, planes[1].view, planes[2].view];
        let after = sync.counter;
        sync.counter = sync
            .counter
            .checked_add(1)
            .context("PyroWave fence exhausted")?;
        let compute = self.compute.as_mut().zip(
            compute
                .as_mut()
                .filter(|_| crate::compute::shareable(&image.texture)),
        );
        let converted = if let Some((converter, target)) = compute {
            converter.set_overlay(self.overlay.as_ref());
            timed("PyroWave compute conversion", || {
                target.convert(
                    converter,
                    image,
                    &self.config,
                    self.luminance,
                    after,
                    sync.counter,
                )
            })
        } else {
            (|| -> Result<()> {
                let converter = match converter {
                    Some(converter) => converter,
                    None => converter.insert(timed("PyroWave graphics converter create", || {
                        PlanarConverter::with_textures(
                            &self.d3d,
                            &self.config,
                            (image.width, image.height, image.pixel),
                            textures.clone(),
                        )
                    })?),
                };
                converter.set_overlay(self.overlay.as_ref());
                timed("PyroWave graphics conversion", || {
                    converter.convert(image, self.luminance)
                })?;
                // SAFETY: `sync.context` and `sync.fence` belong to the live D3D11 device
                // the conversion recorded on; Signal and Flush only queue work.
                unsafe {
                    sync.context.Signal(&sync.fence, sync.counter)?;
                    // Submit the conversion and the signal now, not whenever
                    // D3D11 next flushes: the encode below waits for that signal.
                    sync.context.Flush();
                }
                Ok(())
            })()
        };
        if let Err(error) = converted {
            return Err(self.conversion_failed(error));
        }
        let Slot { sync, .. } = &mut self.slots[slot];
        // SAFETY: `sync.sync` is the slot's live sync object on `self.device`.
        let semaphore = unsafe { (self.api.sync_object_get_semaphore)(sync.sync) };
        let acquire = p::pyrowave_gpu_sync_operation {
            images: images.as_ptr(),
            num_images: 3,
            sync: p::pyrowave_sync_point {
                semaphore,
                value: sync.counter,
            },
        };
        sync.counter = sync
            .counter
            .checked_add(1)
            .context("PyroWave fence exhausted")?;
        let release = p::pyrowave_gpu_sync_operation {
            sync: p::pyrowave_sync_point {
                value: sync.counter,
                ..acquire.sync
            },
            ..acquire
        };
        let buffers = p::pyrowave_gpu_buffers { planes: views };
        // SAFETY: `images`, `acquire`, `release` and `buffers` are live locals
        // naming the slot's images and views made on `self.device`; the call
        // records and submits before it returns.
        unsafe {
            if self.depth > 1 {
                let set = self
                    .api
                    .set_frame_context
                    .context("PyroWave frame contexts unavailable")?;
                check(set(self.encoder, slot as std::ffi::c_int))?;
            }
            // Only once execution reaches `encoder_encode_gpu` has GPU work started whose
            // completion is ambiguous on error; pre-submit validation/setup errors do not quarantine.
            self.submitting = Some(image.clone());
            timed("PyroWave encode submit", || {
                check((self.api.encoder_encode_gpu)(
                    self.encoder,
                    &acquire,
                    &release,
                    &buffers,
                    &p::pyrowave_rate_control {
                        maximum_bitstream_size: budget,
                    },
                ))
            })?;
        }
        Ok(release.sync.value)
    }
    /// A completed frame's bitstream, framed for the client.
    fn read_out(&mut self, frame: Completed) -> Result<Option<Encoded>> {
        let Plan {
            budget,
            kbps,
            interval,
        } = frame.plan;
        // SAFETY: The getters read the frame context's host-visible readback
        // of an encode Vulkan confirmed complete; `bytes` and `self.packets`
        // are sized as passed.
        let (mut bytes, written) = unsafe {
            if self.depth > 1 {
                let set = self
                    .api
                    .set_frame_context
                    .context("PyroWave frame contexts unavailable")?;
                check(set(self.encoder, frame.slot as std::ffi::c_int))?;
            }
            let (mut raw, mut meta) = (ptr::null(), ptr::null());
            let (mut size, mut meta_size) = (0, 0);
            check((self.api.encoder_get_mapped_raw_bitstream)(
                self.encoder,
                &mut raw,
                &mut size,
                &mut meta,
                &mut meta_size,
            ))?;
            if size > 64 * 1024 * 1024 {
                bail!("PyroWave bitstream exceeds the limit");
            }
            let boundary = if self.config.pyrowave_records {
                u32::MAX as usize
            } else {
                PACKET_BOUNDARY
            };
            let mut count = 0;
            check((self.api.encoder_compute_num_packets_with_padding)(
                self.encoder,
                boundary,
                0,
                &mut count,
            ))?;
            if count == 0 || count > 65535 {
                bail!("invalid PyroWave packet count");
            }
            // The codec writes every byte the packets cover; framing below
            // refers to these bytes instead of copying them again.
            let mut bytes = crate::frame_pool::take_filled(size + 8);
            self.packets
                .resize_with(count, || p::pyrowave_packet { offset: 0, size: 0 });
            let mut written = 0;
            check((self.api.encoder_packetize_with_padding)(
                self.encoder,
                self.packets.as_mut_ptr(),
                boundary,
                0,
                &mut written,
                bytes.as_mut_ptr().cast(),
                bytes.len(),
            ))?;
            if written == 0 || written > count {
                bail!("invalid PyroWave packetization");
            }
            (bytes, written)
        };
        let limit = pyrowave::max_frame_bytes(self.config.packet_size, self.critical_fec);
        let framing = if self.config.pyrowave_records {
            if written != 1 {
                bail!("record frame must be one codec packet");
            }
            let (offset, size) = (self.packets[0].offset, self.packets[0].size);
            let end = offset.checked_add(size).context("packet overflow")?;
            anyhow::ensure!(end <= bytes.len(), "invalid codec record range");
            // The record frame is the codec packet itself.
            if offset > 0 {
                bytes.copy_within(offset..end, 0);
            }
            bytes.truncate(size);
            if frame.seq_offset != 0 {
                repeat::shift_sequence(&mut bytes, frame.seq_offset)?;
            }
            let payload = pyrowave::aligned_payload(self.config.packet_size);
            let framing =
                pyrowave::Framing::records(&bytes, payload, limit, self.config.pyrowave_row_order)?;
            let chosen = if framing.len() <= budget {
                Some(framing)
            } else {
                // The framing pass left the layout; a picture that goes out
                // whole is parsed.
                let critical_shards = match framing.layout(payload) {
                    Some(stored) => stored.critical_shards,
                    None => pyrowave::layout(framing.view(&bytes)?, payload)?.critical_shards,
                };
                let minimum = self.config.min_fec.max(pyrowave::MIN_PARITY_SHARDS);
                let packets = (framing.len() + 8).div_ceil(self.config.packet_size - 16)
                    + pyrowave::critical_parity_bound(
                        critical_shards,
                        self.critical_fec_percentage as usize,
                        minimum,
                    );
                let allowance = (f64::from(kbps) * 125. * interval.as_secs_f64()) as usize;
                if packets * (self.config.packet_size + pyrowave::WIRE_OVERHEAD) <= allowance {
                    Some(framing)
                } else {
                    if self.config.pyrowave_row_order {
                        self.warnings.event(
                            "pyrowave_row_order",
                            &row_order_dropped_message(self.config.packet_size),
                            butterpollo_media_policy::EVENT_PERIOD,
                        );
                        if self
                            .row_order_notice
                            .due(Instant::now(), butterpollo_media_policy::EVENT_PERIOD)
                        {
                            tracing::warn!(
                                "PyroWave row order dropped: {} bytes framed ({} unframed) against a budget of {budget} bytes and a wire allowance of {allowance} bytes",
                                framing.len(),
                                bytes.len()
                            );
                        }
                    }
                    // The codec's packet as it is.
                    None
                }
            };
            // Framing that leaves the packet whole sends it as it is.
            chosen.filter(|framing| !framing.is_whole(&bytes))
        } else {
            Some(pyrowave::Framing::container(
                &bytes,
                self.packets[..written].iter().map(|p| (p.offset, p.size)),
            )?)
        };
        let encoded = Encoded {
            bytes,
            framing,
            unit: crate::encoder::Unit::Whole,
            idr: true,
            after_invalidation: false,
            frame_cap_bytes: None,
            reencoded: false,
            frame_cap_target: None,
            latency: Some(frame.begin.elapsed()),
            presentation: Some(frame.captured),
        };
        if encoded.wire_len() > limit {
            self.warnings.event(
                "pyrowave_frame_size",
                &frame_size_message(limit),
                butterpollo_media_policy::EVENT_PERIOD,
            );
            if self
                .size_notice
                .due(Instant::now(), butterpollo_media_policy::EVENT_PERIOD)
            {
                tracing::warn!(
                    "PyroWave frame of {} bytes dropped over the transport limit of {limit}",
                    encoded.wire_len()
                );
            }
            // Its buffer goes back to the pool as it drops.
            return Ok(None);
        }
        if let Some(key) = frame.cache_repeat
            && let Ok(payload) = encoded.payload()
            && let Err(error) = self
                .repeat_cache
                .store(key, payload.to_vec(), frame.seq_offset)
        {
            tracing::debug!(%error, "PyroWave repeat cache skipped");
        }
        Ok(Some(encoded))
    }
}
