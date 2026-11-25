//! Rust-owned D3D11/Vulkan PyroWave encoder using the stable 2.0 bitstream.

use crate::capture::{Device, GpuImage, Image, Pixel};
use crate::encoder::Encoded;
use crate::gpu_color::PlanarConverter;
use anyhow::{Context, Result, bail};
use butterpollo_media_policy::{
    WarningSink,
    pyrowave::{self, PACKET_BOUNDARY},
    recorded_warnings,
    rtsp::Negotiated,
};
use butterpollo_win_abi::pyrowave as p;
use std::{
    cell::Cell,
    collections::{HashSet, VecDeque},
    ffi::c_void,
    ptr,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::{
            Direct3D11::*,
            Dxgi::{
                Common::*, DXGI_ERROR_DEVICE_REMOVED, DXGI_SHARED_RESOURCE_READ,
                DXGI_SHARED_RESOURCE_WRITE, IDXGIDevice, IDXGIResource1,
            },
        },
    },
    core::{Interface, PCWSTR},
};
#[cfg(test)]
mod decode_tests;
mod repeat;
mod slot;
use repeat::{RepeatCache, RepeatKey};
#[cfg(test)]
use slot::WARMUP_MIN_BUDGET;
use slot::{Slot, finish_warmup, source_rebuilds_slots, warmup_budget};
mod encode;
mod runtime;
pub use runtime::available;
use runtime::*;

pub struct Encoder {
    warnings: Arc<dyn WarningSink>,
    api: Arc<Api>,
    device: p::pyrowave_device,
    encoder: p::pyrowave_encoder,
    completion: Option<CompletionWait>,
    /// The capture lease of the frame being submitted, from before its
    /// conversion until it is in flight. It remains set on every failed
    /// conversion or submission.
    submitting: Option<GpuImage>,
    /// Frames on the GPU, oldest first, each pinning its capture lease until
    /// Vulkan confirms it complete.
    flights: Flights<GpuImage>,
    /// Frames in flight at most: two with `pyrowave_encode_overlap`, the next
    /// one's conversion overlapping the last one's encode; one without, which
    /// waits for each frame in its encode call.
    depth: usize,
    lost: Option<crate::device_loss::DeviceLost>,
    adapter: [u8; 8],
    d3d: Device,
    source: Option<(u32, u32, Pixel)>,
    /// One conversion target per frame in flight, made when first needed.
    slots: Vec<Slot>,
    /// Single shared D3D12 compute converter driving every slot's target.
    compute: Option<crate::compute::Converter>,
    config: Negotiated,
    critical_fec: bool,
    /// Parity for each frame's critical block, as a percentage; 0 without.
    critical_fec_percentage: u32,
    interval: pyrowave::Interval,
    /// The next picture repeats the last one encoded.
    pub(crate) repeat: bool,
    repeat_cache: RepeatCache,
    pub(crate) luminance: [f32; 2],
    /// The couch menu's panel, drawn by the conversion while set.
    pub(crate) overlay: Option<crate::overlay::Overlay>,
    packets: Vec<p::pyrowave_packet>,
    /// When the last frame was submitted, after its conversion.
    submitted: Option<Instant>,
    /// Set once the oldest frame in flight is done, with two in flight.
    output_event: Option<crate::timing::Signal>,
    /// The slot and fence value the event was last armed for.
    armed: Cell<Option<(usize, u64)>>,
    row_order_notice: Notice,
    size_notice: Notice,
    #[cfg(test)]
    pub(crate) hooks: TestHooks,
}
/// Runs a test's fault at an exact point of the next frame; each fires once.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct TestHooks {
    /// After the entry checks, before the conversion is recorded.
    pub(crate) before_conversion: Option<Box<dyn FnOnce() + Send>>,
    /// After the encode is submitted, before its completion is awaited.
    pub(crate) after_submit: Option<Box<dyn FnOnce() + Send>>,
}
impl Encoder {
    pub fn new(config: &Negotiated, display: &str) -> Result<Self> {
        Self::new_device(config, Device::new(display)?, &Default::default())
    }
    pub fn new_device(
        config: &Negotiated,
        d3d: Device,
        tuning: &butterpollo_config::config::Config,
    ) -> Result<Self> {
        Self::new_device_reported(config, d3d, tuning, recorded_warnings())
    }
    pub fn new_device_reported(
        config: &Negotiated,
        d3d: Device,
        tuning: &butterpollo_config::config::Config,
        warnings: Arc<dyn WarningSink>,
    ) -> Result<Self> {
        Self::new_inner(config, d3d, None, tuning, warnings)
    }
    /// Creates a PyroWave encoder for a stream whose first captured picture
    /// `image` is already available, pre-creating its conversion slots and
    /// warming up Vulkan compute pipelines before frame 1 enters `encode_gpu`.
    pub fn new_gpu_reported(
        config: &Negotiated,
        image: &GpuImage,
        tuning: &butterpollo_config::config::Config,
        warnings: Arc<dyn WarningSink>,
    ) -> Result<Self> {
        Self::new_inner(
            config,
            image.gpu.clone(),
            Some((image.width, image.height, image.pixel)),
            tuning,
            warnings,
        )
    }
    fn new_inner(
        config: &Negotiated,
        d3d: Device,
        source: Option<(u32, u32, Pixel)>,
        tuning: &butterpollo_config::config::Config,
        warnings: Arc<dyn WarningSink>,
    ) -> Result<Self> {
        let api = Api::load()?;
        // SAFETY: `d3d.device` is a live D3D11 device; GetAdapter and GetDesc only read it.
        let desc = unsafe { d3d.device.cast::<IDXGIDevice>()?.GetAdapter()?.GetDesc()? };
        let mut luid = p::pyrowave_luid { luid: [0; 8] };
        luid.luid[..4].copy_from_slice(&desc.AdapterLuid.LowPart.to_le_bytes());
        luid.luid[4..].copy_from_slice(&desc.AdapterLuid.HighPart.to_le_bytes());
        quarantined_adapters().check(luid.luid)?;
        let critical_fec_percentage = tuning.pyrowave_fec_percentage().clamp(0, 255) as u32;
        let critical_fec = config.pyrowave_records && critical_fec_percentage > 0;
        let overlap = tuning.pyrowave_encode_overlap();
        if overlap && api.set_frame_context.is_none() {
            tracing::warn!(
                "the PyroWave runtime has no frame contexts; encoding one frame at a time"
            );
        }
        let depth = if overlap && api.set_frame_context.is_some() {
            MAX_DEPTH
        } else {
            1
        };
        tracing::info!(frames_in_flight = depth, "PyroWave encode overlap");
        let compute = if crate::compute::enabled(tuning) {
            match crate::compute::Compute::for_device(&d3d.device).and_then(|compute| {
                crate::compute::Converter::new_planar(
                    compute,
                    config.width,
                    config.height,
                    config.ten_bit(),
                )
            }) {
                Ok(converter) => Some(converter),
                Err(error) => {
                    warnings.set("pyrowave_conversion", &format!("PyroWave compute conversion unavailable ({error:#}); converting on the graphics queue. Game rendering may delay frames; update the AMD driver or lower GPU load."));
                    None
                }
            }
        } else {
            None
        };
        let mut s = Self {
            warnings,
            api,
            device: ptr::null_mut(),
            encoder: ptr::null_mut(),
            completion: None,
            submitting: None,
            flights: Flights::default(),
            depth,
            lost: None,
            adapter: luid.luid,
            d3d,
            source,
            slots: Vec::with_capacity(depth),
            compute,
            config: config.clone(),
            critical_fec,
            critical_fec_percentage,
            interval: interval(config),
            repeat: false,
            repeat_cache: RepeatCache::default(),
            luminance: [100., 1.],
            overlay: None,
            packets: vec![],
            submitted: None,
            output_event: if depth > 1 {
                crate::timing::Signal::new().ok()
            } else {
                None
            },
            armed: Cell::new(None),
            row_order_notice: Notice::default(),
            size_notice: Notice::default(),
            #[cfg(test)]
            hooks: Default::default(),
        };
        // SAFETY: `luid` and `info` are live locals, and `s` owns the device and encoder, which
        // Drop destroys.
        unsafe {
            // The encode queue runs ahead of the game's work, as the D3D12 copy and
            // conversion queue does: high priority, never real-time. A driver
            // that refuses the priority gets the default one.
            let wanted = p::VkQueueGlobalPriority_VK_QUEUE_GLOBAL_PRIORITY_HIGH;
            let mut result = p::pyrowave_result_PYROWAVE_ERROR_GENERIC;
            for priority in [
                wanted,
                p::VkQueueGlobalPriority_VK_QUEUE_GLOBAL_PRIORITY_MEDIUM,
            ] {
                result = (s.api.create_device_by_compat)(
                    0,
                    0,
                    ptr::null(),
                    ptr::null(),
                    &luid,
                    priority,
                    &mut s.device,
                );
                if result == p::pyrowave_result_PYROWAVE_SUCCESS {
                    break;
                }
            }
            check(result)?;
            let granted = (s.api.device_get_global_priority)(s.device);
            tracing::info!(
                requested = wanted,
                granted,
                "PyroWave encode queue priority"
            );
            if !(s.api.device_confirm_interop_support)(s.device) {
                bail!("Vulkan device cannot import D3D11 textures and fences");
            }
            if let Err(error) = check((s.api.device_set_queue_type)(
                s.device,
                p::VkQueueFlagBits_VK_QUEUE_COMPUTE_BIT,
            )) {
                s.warnings.set("pyrowave_queue", &format!("PyroWave compute queue selection failed ({error:#}); using the runtime's default queue. Game rendering may delay encoding; update the AMD driver or lower game GPU load."));
            }
            let info = p::pyrowave_encoder_create_info {
                device: s.device,
                width: config.width as i32,
                height: config.height as i32,
                chroma: u32::from(config.yuv444),
            };
            check((s.api.encoder_create)(&info, &mut s.encoder))?;
            s.completion = Some(CompletionWait::new(&s.api, s.device)?);
        }
        // Build slots and compile conversion + Vulkan encode pipelines now,
        // before the session enters `encode_gpu` on frame 1 (issue #147).
        let slot_started = Instant::now();
        let compute_device = s.compute.as_ref().map(|c| c.compute().clone());
        for _ in 0..s.depth {
            s.slots.push(Slot::new(
                &s.api,
                &s.d3d,
                s.device,
                &s.config,
                source,
                compute_device.as_ref(),
                &s.warnings,
            )?);
        }
        let slot_ms = slot_started.elapsed().as_millis() as u64;
        let warmup_started = Instant::now();
        let warmed = s.warm_up();
        s.settled(warmed)?;
        let warmup_ms = warmup_started.elapsed().as_millis() as u64;
        tracing::info!(
            slots = s.slots.len(),
            compute = s.converts_on_compute(),
            slot_ms,
            warmup_ms,
            "PyroWave encoder ready"
        );
        Ok(s)
    }
    /// Runs one discarded Vulkan encode on every frame context `0..self.depth`
    /// so Granite compiles its compute pipelines and allocates every frame
    /// context's staging/readback buffers during creation rather than on frames 1–2.
    fn warm_up(&mut self) -> Result<()> {
        let budget = warmup_budget(
            &self.config,
            self.critical_fec,
            self.critical_fec_percentage,
        );
        for slot_index in 0..self.depth {
            let release_value = self.slots[slot_index].warm_up_submit(
                &self.api,
                self.encoder,
                self.depth,
                slot_index,
                budget,
            )?;
            let submitted = Instant::now();
            let completion = self
                .completion
                .as_ref()
                .context("PyroWave completion wait")?;
            let semaphore = self.slots[slot_index].semaphore();
            let waited = settle(
                |timeout| completion.reached(semaphore, release_value, timeout),
                submitted,
                Instant::now(),
                true,
            );
            let removed = self.device_removed();
            finish_warmup(
                waited,
                removed,
                &mut self.flights.unconfirmed,
                &mut self.lost,
            )?;
        }
        Ok(())
    }
    pub fn accepts_gpu_device(&self, image: &GpuImage) -> bool {
        self.d3d.device.as_raw() == image.gpu.device.as_raw()
    }
    pub(crate) fn device_removed(&self) -> Option<crate::device_loss::DeviceLost> {
        self.lost
            .or_else(|| crate::device_loss::DeviceLost::d3d11(&self.d3d.device))
            .or_else(|| self.compute_device_removed())
    }
    /// The D3D12 device that converts on the compute queue, when it is lost.
    fn compute_device_removed(&self) -> Option<crate::device_loss::DeviceLost> {
        crate::device_loss::DeviceLost::d3d12(&self.compute.as_ref()?.compute().device)
    }
    /// A D3D device is lost: its queued work is dead, so the capture leases
    /// are free and the adapter is not quarantined; the host rebuilds the
    /// encoder on a new device. Vulkan reads only the slots' own planes, which
    /// stay until Drop has idled it. A frame whose completion was already
    /// unconfirmed keeps everything for the quarantine.
    fn device_lost(&mut self, loss: crate::device_loss::DeviceLost) {
        self.lost = Some(loss);
        if !self.flights.unconfirmed {
            self.submitting = None;
            self.flights.abandon();
        }
    }
    /// A conversion failed before anything reached Vulkan. When the device it
    /// ran on is lost, see [`Self::device_lost`]. Any other failure keeps the
    /// lease and quarantines.
    fn conversion_failed(&mut self, error: anyhow::Error) -> anyhow::Error {
        match self.device_removed() {
            Some(loss) => {
                self.device_lost(loss);
                error.context(loss)
            }
            None => error,
        }
    }
    /// The next encode gets a first frame's budget, one stream period.
    #[cfg(test)]
    pub(crate) fn restart_interval(&mut self) {
        self.interval = interval(&self.config);
    }
    /// Encodes `image` and waits for it, for tests that check each frame.
    #[cfg(test)]
    pub(crate) fn encode_now(
        &mut self,
        image: &GpuImage,
        idr: bool,
        kbps: u32,
    ) -> Result<Vec<Encoded>> {
        let mut frames = self.encode_gpu(image, idr, kbps)?;
        frames.extend(self.finish()?);
        Ok(frames)
    }
    /// Whether the first slot converts on the compute queue.
    pub(crate) fn converts_on_compute(&self) -> bool {
        self.compute.is_some()
            && self
                .slots
                .first()
                .is_some_and(|slot| slot.compute.is_some())
    }
    /// The compute converter of the encoder.
    #[cfg(test)]
    pub(crate) fn compute(&self) -> Option<&Arc<crate::compute::Compute>> {
        Some(self.compute.as_ref()?.compute())
    }
    /// The Vulkan images of the first slot's planes.
    #[cfg(test)]
    pub(crate) fn plane_images(&self) -> Vec<usize> {
        self.slots.first().map_or_else(Vec::new, |slot| {
            slot.planes.iter().map(|i| i.view.image as usize).collect()
        })
    }
    /// Capture leases the encoder holds.
    #[cfg(test)]
    pub(crate) fn leases(&self) -> usize {
        self.flights.len() + usize::from(self.submitting.is_some())
    }
}
impl Drop for Encoder {
    fn drop(&mut self) {
        // The frames still in flight, each within its own completion bound,
        // as at the end of a stream or a rebuild.
        let confirmed = !self.unsettled()
            && match &self.completion {
                Some(completion) => {
                    let slots = &self.slots;
                    self.flights
                        .settle_all(|flight| {
                            settle(
                                |timeout| {
                                    completion.reached(
                                        slots[flight.slot].semaphore(),
                                        flight.release,
                                        timeout,
                                    )
                                },
                                flight.submitted,
                                Instant::now(),
                                true,
                            )
                        })
                        .is_ok()
                }
                None => self.flights.is_empty(),
            };
        if !confirmed {
            // Keep both APIs, all imported images/fences, conversion commands,
            // and the capture leases alive. No queued wait is added to capture.
            self.quarantine();
            tracing::warn!("retaining PyroWave resources after an unconfirmed GPU submission");
            retain_pending(&mut self.submitting);
            self.flights.retain();
            retain_pending(&mut self.completion);
            std::mem::forget(std::mem::take(&mut self.slots));
            retain_pending(&mut self.compute);
            std::mem::forget(self.output_event.take());
            std::mem::forget(self.api.clone());
            std::mem::forget(self.d3d.clone());
            return;
        }
        // SAFETY: `encoder` and `device` were created in `new_device_reported`; the slots' images
        // go before the device. After a device loss the frames in flight are already gone while
        // Vulkan may still be running or queued, because the lost fence satisfies the completion
        // wait early. `pyrowave_encoder_destroy` goes first: pyrowave.h (SDK 502a3b52,
        // "Implementation ensures GPU is idle before destroying objects") idles the GPU there,
        // before the images and the device are destroyed.
        unsafe {
            if !self.encoder.is_null() {
                (self.api.encoder_destroy)(self.encoder);
            }
            self.slots.clear();
            if !self.device.is_null() {
                (self.api.device_destroy)(self.device);
            }
        }
    }
}

#[cfg(test)]
mod tests;
