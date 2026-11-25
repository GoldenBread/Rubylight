//! Captures a WGC display from an isolated helper process inside the
//! desktop's own session (`--capture-bridge`).
//!
//! Used only when the host runs as a Windows service (Session 0), where WGC
//! cannot initialize directly, and only when IDD push and Desktop Duplication
//! cannot capture the display.

use super::*;
use crate::user_helper::{HelperPipe, HelperPipeExt, UserChild, UserLauncher};
use butterpollo_config::config::Config;
use butterpollo_media_policy::WarningSink;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
#[cfg(test)]
use windows::Win32::Foundation::{ERROR_ACCESS_DENIED, WAIT_ABANDONED, WAIT_TIMEOUT};
use windows::Win32::System::Threading::GetCurrentProcessId;
#[cfg(test)]
use windows::core::HRESULT;

mod protocol;
mod worker;
pub(crate) use protocol::Lease;
pub use protocol::START_TIMEOUT;
pub(super) use protocol::desktop_available;
use protocol::*;
pub use worker::run_worker_with;
#[cfg(test)]
use worker::{frame_for_slot, helper_ended};
use worker::{helper_error, startup_failure};

/// A frame the helper announced: its slot, sequence, capture time, WGC
/// stamp and focus label.
type Announced = (usize, u64, Instant, Option<Instant>, Option<bool>);
use std::collections::VecDeque;

pub struct Session {
    pipe: Box<dyn HelperPipe>,
    pub(super) gpu: Device,
    textures: Vec<Texture>,
    /// Immutable `(width, height, pixel)` of every shared helper texture.
    shape: (u32, u32, Pixel),
    owned: GpuPool,
    slots: Slots,
    pending: VecDeque<Announced>,
    /// The focus label of the frames taken from `pending` since the last
    /// published image: a dropped frame's focus change is the next one's.
    focus: Option<bool>,
    published: u64,
    held: Option<(GpuImage, Instant)>,
    grid: Option<Arc<Mutex<ClaimGrid>>>,
    intervals: butterpollo_media_policy::capture_policy::Intervals,
    origin: Instant,
    last_publish: Instant,
    desktop_check: Instant,
    heartbeat: Instant,
    last_reply: Instant,
    last_poll: Instant,
    staging: Option<ID3D11Texture2D>,
    process: Box<dyn UserChild>,
    /// Set by the helper after each frame: the capture worker wakes for it
    /// instead of finding it on its next poll.
    frame: Arc<crate::timing::Signal>,
    /// Images read in place from the helper's textures, with the slots
    /// their leases gave back since the last poll.
    returns: Option<Arc<Mutex<crate::compute::Returns>>>,
    freed: Arc<Mutex<Vec<usize>>>,
}
impl Session {
    /// Captures display `name`, or `window` (an HWND) with `name`'s GPU.
    pub(super) fn new(
        name: &str,
        window: Option<usize>,
        hdr: bool,
        config: &Config,
        warnings: Arc<dyn WarningSink>,
        launcher: Option<&dyn UserLauncher>,
    ) -> Result<Self> {
        let launcher = launcher.context("WGC user-session launcher is not configured")?;
        ensure!(
            desktop_available(),
            "WGC requires the unlocked user desktop"
        );
        let (adapter_name, pnp_id) = crate::capture::stream_adapter(
            Some(name),
            config.get("adapter_name", ""),
            config.get("adapter_pnp_id", ""),
        );
        let gpu = Device::for_display(name, &adapter_name, &pnp_id)?;
        let _ = crate::gpu_priority::configure(&gpu, config);
        let (pipe, pipe_name) = launcher.pipe_server(PIPE_PREFIX)?;
        let program = std::env::current_exe()?;
        let args = [
            "--wgc-worker".into(),
            pipe_name.into(),
            "--wgc-parent".into(),
            // SAFETY: GetCurrentProcessId has no preconditions.
            unsafe { GetCurrentProcessId() }.to_string().into(),
        ];
        let started = Instant::now();
        let process = launcher
            .spawn_user(&program, &args)
            .context("start WGC in the signed-in user's session")?;
        let pid = process.pid();
        let deadline = Instant::now() + START_TIMEOUT;
        while !pipe.connected(pid)? {
            ensure!(
                process.exit_code()?.is_none(),
                "WGC helper exited before connecting"
            );
            ensure!(Instant::now() < deadline, "WGC helper connection timed out");
            std::thread::sleep(Duration::from_millis(2));
        }
        let connected = Instant::now();
        tracing::debug!(
            pid,
            elapsed_ms = started.elapsed().as_secs_f64() * 1000.,
            "WGC helper connected"
        );
        let frame = Arc::new(crate::timing::Signal::new()?);
        pipe.send_json(&Request::Start {
            version: VERSION,
            name: gpu.display.display_name.clone(),
            hdr,
            config: capture_config(config),
            wake: process.duplicate_into(frame.raw().0 as usize)?,
            window: window.map(|window| window as u64),
        })?;
        let (handles, width, height, format, worker_compute) = loop {
            match pipe.receive_json::<Reply>()? {
                Some(Reply::Ready {
                    version,
                    handles,
                    width,
                    height,
                    format,
                    compute,
                }) => {
                    ensure!(version == VERSION, "WGC helper version mismatch");
                    break (handles, width, height, format, compute);
                }
                Some(Reply::Error {
                    message,
                    device_removed,
                }) => return Err(helper_error(message, device_removed)),
                Some(_) => bail!("WGC helper sent a frame before its textures"),
                None => {}
            }
            ensure!(
                process.exit_code()?.is_none(),
                "WGC helper exited during startup"
            );
            ensure!(
                Instant::now() < deadline,
                "WGC helper did not produce a first frame"
            );
            std::thread::sleep(Duration::from_millis(2));
        };
        tracing::debug!(
            pid,
            elapsed_ms = started.elapsed().as_secs_f64() * 1000.,
            ready_ms = connected.elapsed().as_secs_f64() * 1000.,
            "WGC helper ready"
        );
        let device: ID3D11Device1 = gpu.device.cast()?;
        let mut owned = GpuPool {
            warnings: warnings.clone(),
            ..Default::default()
        };
        let mut opened = Vec::with_capacity(SLOTS);
        for handle in handles {
            let handle = process
                .duplicate_resource(usize::try_from(handle)?)
                .map_err(|error| startup_failure(error, &*pipe, &*process))?;
            // SAFETY: `handle` is the helper's shared texture duplicated into this process; `desc`
            // is checked below.
            let texture: windows::core::Result<ID3D11Texture2D> =
                unsafe { device.OpenSharedResource1(raw(&handle)) };
            let texture =
                texture.map_err(|error| startup_failure(error.into(), &*pipe, &*process))?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            // SAFETY: `texture` is a live texture and `desc` a live local.
            unsafe { texture.GetDesc(&mut desc) };
            validate_texture(&desc, width, height, format)?;
            opened.push((texture, handle));
        }
        let pixel = Pixel::try_from_dxgi_raw(format).context("unsupported WGC shared format")?;
        // Prefer in-place `Returns`; only create `Handoff` (and import handles
        // into it) when zero-copy is off or unavailable.
        let mut returns = None;
        if config.boolean("wgc_compute_copy", true) && crate::compute::enabled(config) {
            if config.boolean("wgc_helper_zero_copy", true) {
                let lent = crate::compute::Compute::for_device(&gpu.device)
                    .and_then(|compute| crate::compute::Returns::new(compute, &gpu))
                    .and_then(|mut lender| {
                        opened
                            .iter()
                            .try_for_each(|(t, h)| lender.import(t, raw(h)))?;
                        Ok(lender)
                    });
                match lent {
                    Ok(lender) => returns = Some(lender),
                    Err(error) => {
                        tracing::warn!(error = %format!("{error:#}"), "WGC helper textures cannot be read in place; copying them")
                    }
                }
            }
            if returns.is_none()
                && let Some(mut copy) = handoff(&gpu, config, &*warnings)
            {
                match opened.iter().try_for_each(|(t, h)| copy.import_shared(t, raw(h))) {
                    Ok(()) => owned.compute = Some(copy),
                    Err(error) => warnings.set("capture_compute", &format!("WGC shared texture cannot use compute ({error:#}); using the graphics queue. Lower game GPU load or update the graphics driver if capture stutters.")),
                }
            }
        }
        let textures: Vec<Texture> = opened
            .into_iter()
            .map(|(texture, _)| {
                Ok(Texture {
                    mutex: texture.cast()?,
                    texture: Arc::new(texture),
                })
            })
            .collect::<Result<_>>()?;
        tracing::info!(
            pid,
            output = %gpu.display.display_name,
            window = window.is_some(),
            width,
            height,
            format,
            pixel = ?pixel,
            in_hdr = gpu.display_in_hdr(),
            sdr_white_nits = gpu.sdr_white_nits(),
            compute = owned.compute.is_some() || returns.is_some(),
            worker_compute,
            zero_copy = returns.is_some(),
            "Windows Graphics Capture running in the signed-in user's process"
        );
        let now = Instant::now();
        Ok(Self {
            pipe,
            gpu,
            textures,
            shape: (width, height, pixel),
            owned,
            slots: Slots::default(),
            pending: VecDeque::with_capacity(SLOTS),
            focus: None,
            published: 0,
            held: None,
            grid: None,
            intervals: Default::default(),
            origin: now,
            last_publish: now,
            desktop_check: now,
            heartbeat: now + Duration::from_secs(1),
            last_reply: now,
            last_poll: now,
            staging: None,
            process,
            frame,
            returns: returns.map(|returns| Arc::new(Mutex::new(returns))),
            freed: Arc::default(),
        })
    }
    pub(super) fn frame_signal(&self) -> &crate::timing::Signal {
        &self.frame
    }
    pub(super) fn pending_frame(&self) -> bool {
        !self.pending.is_empty()
    }
    pub(super) fn set_claim_grid(&mut self, grid: Option<Arc<Mutex<ClaimGrid>>>) {
        self.grid = grid;
    }
    pub(super) fn publication_deadline(&self) -> Option<Instant> {
        self.held.as_ref().map(|(_, deadline)| *deadline)
    }
    pub(super) fn next_frame(&mut self) -> Result<Option<Image>> {
        self.next_gpu(0)?
            .map(|image| image.readback(&mut self.staging))
            .transpose()
    }
    pub(super) fn next_gpu(&mut self, origin: u64) -> Result<Option<std::sync::Arc<GpuImage>>> {
        let now = Instant::now();
        if now >= self.desktop_check {
            self.desktop_check = now + Duration::from_millis(100);
            ensure!(
                desktop_available(),
                "WGC user desktop changed; Desktop Duplication required"
            );
            ensure!(self.process.exit_code()?.is_none(), "WGC helper exited");
        }
        // If capture polling paused longer than a heartbeat interval, send a Ping first and
        // give the helper a fresh `START_TIMEOUT` to answer instead of tripping on stale time.
        if now.saturating_duration_since(self.last_poll) >= Duration::from_secs(1) {
            self.heartbeat = now;
            self.last_reply = now;
        }
        self.last_poll = now;
        if now >= self.heartbeat {
            self.pipe.send_json(&Request::Ping)?;
            self.heartbeat = now + Duration::from_secs(1);
        }
        let freed = std::mem::take(&mut *self.freed.lock().unwrap());
        for slot in freed {
            self.slots.release(slot)?;
            self.pipe.send_json(&Request::Release { slot })?;
        }
        // At most three frames can be outstanding. Never accumulate a queue
        // behind an encoder; copy only the newest frame whose GPU work is done.
        for _ in 0..=SLOTS {
            match self.pipe.receive_json::<Reply>()? {
                Some(Reply::Frame {
                    slot,
                    sequence,
                    qpc,
                    focus,
                }) => {
                    self.slots.publish(slot, sequence)?;
                    self.last_reply = now;
                    let (captured, stamp) = qpc_timestamps(qpc);
                    self.pending
                        .push_back((slot, sequence, captured, stamp, focus));
                }
                Some(Reply::Alive) => self.last_reply = now,
                Some(Reply::ComputeFallback { message }) => {
                    self.last_reply = now;
                    self.owned.warnings.set("capture_helper_compute", &format!("WGC helper compute copy failed ({message}); transfer uses the graphics queue. Lower game GPU load or update the graphics driver if capture stutters."));
                }
                Some(Reply::Error {
                    message,
                    device_removed,
                }) => return Err(helper_error(message, device_removed)),
                Some(Reply::Ready { .. }) => bail!("WGC helper replaced live textures"),
                None => break,
            }
        }
        ensure!(
            now.duration_since(self.last_reply) < START_TIMEOUT,
            "WGC helper stopped responding"
        );
        let mut newest = None;
        let mut attempted_copy = false;
        for index in (0..self.pending.len()).rev() {
            if self.returns.is_none() && !self.owned.has_capacity() {
                // Keep the helper's bounded slots until a snapshot is free.
                // Its worker retains the newest unsent native frame, including
                // the last update of a desktop that has since become static.
                break;
            }
            let (slot, sequence, captured, stamp, focus) = self.pending[index];
            let texture = &self.textures[slot];
            if !acquire(&texture.mutex, 1)? {
                continue;
            }
            let copied = if !attempted_copy && sequence > self.published {
                attempted_copy = true;
                match &self.returns {
                    Some(returns) => self.lend(slot, returns.clone()).map(Some),
                    None => self.owned.copy(&self.gpu, &texture.texture, None),
                }
            } else {
                Ok(None)
            };
            self.pending.remove(index);
            self.focus = butterpollo_media_policy::capture_policy::merge_focus(self.focus, focus);
            // A lent texture goes back when its image is dropped.
            if !matches!(&copied, Ok(Some(image)) if image.lease.is_some()) {
                // Handoff orders the context after its compute copy. Releasing the
                // keyed mutex therefore cannot let the helper overwrite a texture
                // while our GPU is still reading it, even without a CPU wait.
                // SAFETY: This process acquired the keyed mutex with key 1 above, and the context is
                // this device's.
                unsafe {
                    texture.mutex.ReleaseSync(0)?;
                    self.gpu.context.Flush();
                }
                self.slots.release(slot)?;
                self.pipe.send_json(&Request::Release { slot })?;
            }
            if let Some(mut image) = copied? {
                image.captured = captured;
                image.wgc_stamp = stamp;
                self.published = sequence;
                newest = Some(image);
            }
        }
        if let Some(image) = newest {
            let composition = self.intervals.observe(nanos(now, self.origin));
            let deadline = self
                .grid
                .as_ref()
                .and_then(|grid| {
                    let grid = grid.lock().unwrap();
                    butterpollo_media_policy::capture_policy::publication_deadline(
                        grid.period.as_nanos().min(i64::MAX as u128) as i64,
                        nanos(now, grid.anchor),
                        composition,
                        nanos(self.last_publish, grid.anchor),
                    )
                    .and_then(|deadline| {
                        if deadline >= 0 {
                            grid.anchor
                                .checked_add(Duration::from_nanos(deadline as u64))
                        } else {
                            grid.anchor
                                .checked_sub(Duration::from_nanos(deadline.unsigned_abs()))
                        }
                    })
                })
                .unwrap_or(now);
            self.held = Some((image, deadline));
        }
        if self
            .held
            .as_ref()
            .is_some_and(|(_, deadline)| *deadline <= now)
        {
            self.last_publish = now;
            return Ok(self.held.take().map(|(mut image, _)| {
                image.focus = self.focus.take();
                image.published = Instant::now();
                image.origin = origin;
                std::sync::Arc::new(image)
            }));
        }
        Ok(None)
    }
}
impl Session {
    /// An image of helper texture `slot`, read in place; this process holds
    /// its keyed mutex (key 1) until the image's lease hands it back.
    fn lend(&self, slot: usize, returns: Arc<Mutex<crate::compute::Returns>>) -> Result<GpuImage> {
        let texture = &self.textures[slot];
        let (width, height, pixel) = self.shape;
        let (ready, compute_read) = returns.lock().unwrap().acquired()?;
        let mut image = GpuImage::from_texture(
            self.gpu.clone(),
            texture.texture.clone(),
            width,
            height,
            pixel,
            Some(ready),
        );
        image.lease = Some(Arc::new(Lease {
            slot,
            back: Back::Keyed(crate::compute::KeyedLease::new(
                texture.mutex.clone(),
                returns,
                compute_read,
                0,
            )),
            notify: Some((self.freed.clone(), self.frame.clone())),
        }));
        Ok(image)
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.pipe.send_json(&Request::Stop);
        // The owned job also kills the helper if it crashes, hangs in WGC, or
        // outlives this session. No helper survives capture/device recovery.
    }
}

#[cfg(test)]
mod tests;
