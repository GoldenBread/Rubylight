//! Ring delivery, fence-to-keyed fallback, reshape handling, and periodic health checks
//! for Rubylight virtual display driver frame push sessions.

use super::*;

impl Session {
    /// A failed fence allocation never prevents the keyed-mutex capture path.
    pub(super) fn make_ring(
        gpu: &Device,
        compute: Option<Arc<crate::compute::Compute>>,
        mut layout: Layout,
        zero_copy: bool,
        warnings: &dyn WarningSink,
        scope: FenceScope,
    ) -> Result<Ring> {
        if layout.kind == Kind::Fence {
            match compute
                .map_or_else(|| crate::compute::Compute::for_device(&gpu.device), Ok)
                .and_then(|compute| Ring::fenced(gpu, compute, layout, zero_copy))
            {
                Ok(ring) => return Ok(ring),
                Err(error) => {
                    FENCE_FAILURES.lock().unwrap().failed(scope);
                    let message = format!(
                        "GPU fence transport unavailable ({error:#}); using the keyed-mutex driver transport."
                    );
                    tracing::warn!("{message}");
                    warnings.set("capture_fence_ring", &message);
                }
            }
        }
        layout.kind = Kind::KeyedMutex;
        Ring::keyed(gpu, layout)
    }

    /// Replace a refused or unopened fence ring with the established transport in this
    /// session. Its generation changes, so a delayed completion from the old ring is ignored.
    pub(super) fn use_keyed_ring(&mut self, reason: &str, remember: bool) -> Result<()> {
        if remember {
            FENCE_FAILURES
                .lock()
                .unwrap()
                .failed(FenceScope::new(&self.monitor, self.adapter));
        }
        let message =
            format!("GPU fence transport {reason}; using the keyed-mutex driver transport.");
        tracing::warn!("{message}");
        self.pool.warnings.set("capture_fence_ring", &message);
        let mut layout = self.ring.layout;
        layout.kind = Kind::KeyedMutex;
        layout.generation = next_generation(layout.generation);
        self.ring = Ring::keyed(&self.gpu, layout)?;
        self.compute = None;
        self.consumer = Consumer::new(layout.generation, layout.slot_count);
        self.opened = Instant::now();
        self.started = false;
        self.deliver()
    }

    /// Never put a new ring behind an unresolved wait on the old capture context. The
    /// capture worker recreates its D3D11 device; the rejection makes that reopen keyed.
    pub(super) fn reopen_keyed(&self, reason: &str) -> Result<()> {
        FENCE_FAILURES
            .lock()
            .unwrap()
            .failed(FenceScope::new(&self.monitor, self.adapter));
        if let Some(fences) = &self.ring.fences {
            fences.failed.store(true, Ordering::Release);
        }
        let message = format!(
            "GPU fence transport {reason}; reopening with the keyed-mutex driver transport."
        );
        tracing::warn!("{message}");
        self.pool.warnings.set("capture_fence_ring", &message);
        bail!("GPU fence transport {reason}; recreate the capture device")
    }

    /// Hand the current ring to the driver, after lending a keyed-mutex ring's textures to
    /// the compute conversion or importing them into the compute copy.
    pub(super) fn deliver(&mut self) -> Result<()> {
        self.lender = None;
        if self.ring.fences.is_none() && self.zero_copy {
            let lent = crate::compute::Compute::for_device(&self.gpu.device)
                .and_then(|compute| crate::compute::Returns::new(compute, &self.gpu))
                .and_then(|mut returns| {
                    for slot in &self.ring.slots {
                        returns.import(&slot.texture, HANDLE(slot.shared.as_raw_handle()))?;
                    }
                    Ok(returns)
                });
            match lent {
                Ok(returns) => self.lender = Some(Arc::new(Mutex::new(returns))),
                Err(error) => {
                    tracing::warn!(error = %format!("{error:#}"), "the Rubylight frame ring cannot be read in place; copying it");
                    self.zero_copy = false;
                }
            }
        }
        if !self.zero_copy && self.wants_handoff {
            if self.pool.compute.is_none() {
                match crate::compute::Compute::for_device(&self.gpu.device)
                    .and_then(|compute| crate::compute::Handoff::new(compute, &self.gpu))
                {
                    Ok(handoff) => self.pool.compute = Some(handoff),
                    Err(error) => {
                        self.pool.warnings.set("capture_compute", &format!("Compute capture copies unavailable ({error:#}); copying on the graphics queue. Lower game GPU load or update the graphics driver if capture stutters."));
                        self.wants_handoff = false;
                    }
                }
            }
            if let Some(handoff) = &mut self.pool.compute {
                let imported = self.ring.slots.iter().try_for_each(|slot| {
                    handoff.import_shared(&slot.texture, HANDLE(slot.shared.as_raw_handle()))
                });
                if let Err(error) = imported {
                    self.pool.warnings.set("capture_compute", &format!("The Rubylight frame ring cannot use compute copies ({error:#}); copying on the graphics queue."));
                    self.pool.compute = None;
                    self.wants_handoff = false;
                }
            }
        }
        let layout = self.ring.layout;
        let mut delivery = Delivery {
            remote: &self.remote,
            values: Vec::new(),
            adopted: false,
        };
        let mut textures = [0; ring::MAX_SLOTS];
        for (value, slot) in textures.iter_mut().zip(&self.ring.slots) {
            *value = delivery.send(HANDLE(slot.shared.as_raw_handle()))?;
        }
        let request = SetFrameRingRequest {
            session_id: self.monitor.session_id,
            generation: layout.generation,
            slot_count: layout.slot_count,
            width: layout.width,
            height: layout.height,
            format: layout.format,
            header_bytes: ring::SECTION_BYTES as u32,
            adapter_luid_low: self.adapter.0,
            adapter_luid_high: self.adapter.1,
            header_section: delivery.send(HANDLE(self.ring.section.handle.as_raw_handle()))?,
            event: delivery.send(self.frame.raw())?,
            textures,
        };
        let refused = "the Rubylight display driver refused the frame ring";
        match &self.ring.fences {
            Some(fences) => {
                let [written, released] = &fences.handles;
                let request = SetFenceRingRequest {
                    ring: request,
                    written_fence: delivery.send(HANDLE(written.as_raw_handle()))?,
                    released_fence: delivery.send(HANDLE(released.as_raw_handle()))?,
                };
                request
                    .validate()
                    .map_err(|e| anyhow::anyhow!("frame ring request: {e}"))?;
                self.control.set_fence_ring(&request).context(refused)?;
            }
            None => {
                request
                    .validate()
                    .map_err(|e| anyhow::anyhow!("frame ring request: {e}"))?;
                self.control.set_frame_ring(&request).context(refused)?;
            }
        }
        delivery.adopted = true;
        Ok(())
    }

    /// Rebuild the ring at the shape the driver reported, before any frame was taken.
    pub(super) fn reshape(&mut self, width: u32, height: u32, format: u32) -> Result<()> {
        self.reshapes += 1;
        anyhow::ensure!(
            self.reshapes <= RESHAPES_BEFORE_FIRST_FRAME,
            "the Rubylight display surface keeps changing shape"
        );
        let mut layout = self.ring.layout;
        layout.width = width;
        layout.height = height;
        layout.format = format;
        layout.generation = next_generation(layout.generation);
        tracing::info!(
            width,
            height,
            format,
            "rebuilding the Rubylight frame ring for the composed surface"
        );
        self.ring = Self::make_ring(
            &self.gpu,
            self.compute.clone(),
            layout,
            self.zero_copy,
            &self.pool.warnings,
            FenceScope::new(&self.monitor, self.adapter),
        )?;
        self.compute = self
            .ring
            .fences
            .as_ref()
            .map(|fences| fences.compute.clone());
        self.consumer = Consumer::new(layout.generation, layout.slot_count);
        self.opened = Instant::now();
        if let Err(error) = self.deliver() {
            if self.ring.fences.is_none() {
                return Err(error);
            }
            self.use_keyed_ring(&format!("ring rebuild failed: {error:#}"), true)?;
        }
        Ok(())
    }

    pub(super) fn check(&mut self) -> Result<()> {
        if let Some(loss) = crate::device_loss::DeviceLost::d3d11(&self.gpu.device) {
            return Err(loss.into());
        }
        if let Some(loss) = self
            .compute
            .as_ref()
            .and_then(|compute| crate::device_loss::DeviceLost::d3d12(&compute.device))
        {
            return Err(loss.into());
        }
        let header = self.ring.section.header();
        if let Some(fences) = &self.ring.fences
            && let Some(recovery) = fence_recovery(
                header.status().0,
                self.started,
                self.opened.elapsed(),
                header.counters(),
                fences.failed.load(Ordering::Acquire),
                fences.compute.is_stuck(),
            )
        {
            return match recovery {
                FenceRecovery::Keyed { reason, remember } => self.use_keyed_ring(reason, remember),
                FenceRecovery::Reopen(reason) => self.reopen_keyed(reason),
            };
        }
        match verdict(header.status(), header.surface(), self.started) {
            Verdict::Fine => {}
            Verdict::Renew => {
                // Same shape, new resources and fences: all old frame readers keep their
                // own ownership until done. This is not an unstable-shape retry.
                let reshapes = self.reshapes;
                self.reshapes = 0;
                let layout = self.ring.layout;
                let renewed = self.reshape(layout.width, layout.height, layout.format);
                self.reshapes = reshapes;
                self.started = false;
                return renewed;
            }
            Verdict::Reshape(width, height, format) => return self.reshape(width, height, format),
            Verdict::Fail(message) => bail!(message),
        }
        let counters = header.counters();
        if self.counted.0.elapsed() >= COUNTERS_EVERY {
            let last = std::mem::replace(&mut self.counted, (Instant::now(), counters)).1;
            tracing::debug!(
                target: "idd_push",
                acquired = counters.acquired.saturating_sub(last.acquired),
                pushed = counters.pushed.saturating_sub(last.pushed),
                dropped = counters.dropped.saturating_sub(last.dropped),
                overwritten = counters.overwritten.saturating_sub(last.overwritten),
                "Rubylight frame ring counters"
            );
        }
        if self.started {
            anyhow::ensure!(
                !stalled(header.heartbeat(), qpc_now(), qpc_frequency(), DRAIN_STALL),
                "the Rubylight display driver stopped delivering frames"
            );
        } else {
            anyhow::ensure!(
                self.opened.elapsed() < FIRST_FRAME_TIMEOUT,
                "the Rubylight display driver delivered no frame"
            );
        }
        Ok(())
    }
}
