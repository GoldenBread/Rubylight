//! The PyroWave encoder of a Linux host: the Vulkan wavelet codec of the PyroWave SDK
//! (`libpyrowave-shared.so`, loaded at run time), fed a captured picture as a DMA-BUF imported
//! as Vulkan external memory, or a BGRA picture in CPU memory (the codec probe). It mirrors
//! the Windows `pyrowave::Encoder`: `encode_gpu`/`encode` return the frames done, each framed
//! for the client by `butterpollo_media_policy::pyrowave`. Encoding is synchronous, and every
//! frame is a key frame.
//!
//! Colour: the codec encodes full-range BT.709 YCbCr from sRGB-nonlinear RGB, the SDK's
//! default for both paths. An NV12 DMA-BUF is passed through unconverted, so its range and
//! matrix are the capture's own. Not yet: 10-bit and HDR.
//!
//! PyroWave is by Hans-Kristian Arntzen (MIT); see NOTICE and `rust/THIRD_PARTY.md`.

use crate::{
    capture::{Image, Pixel},
    encoder::{Encoded, Unit},
};
use anyhow::{Context, Result, bail, ensure};
use ash::vk;
use butterpollo_media_policy::{pyrowave, rtsp::Negotiated};
use std::{ptr, sync::Arc, time::Instant};

mod convert;
pub mod dmabuf;
pub mod ffi;
#[cfg(test)]
mod tests;

pub use dmabuf::{DmaBuf, Plane, export_surface};

/// What a stream's encoder is created with besides its negotiation.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// The GPU's PCI vendor and device ids, to pick it among several. `None` takes the
    /// library's first suitable device.
    pub gpu: Option<(u32, u32)>,
    /// The share of critical wavelet data protected by FEC, as the host's setting.
    pub critical_fec_percentage: u32,
}

/// The PCI vendor and device ids of a render node (`/dev/dri/renderD128`), as Vulkan reports
/// them.
pub fn render_node_ids(node: &std::path::Path) -> Result<(u32, u32)> {
    let name = node.file_name().context("render node name")?;
    let base = std::path::Path::new("/sys/class/drm")
        .join(name)
        .join("device");
    let read = |file: &str| -> Result<u32> {
        let text = std::fs::read_to_string(base.join(file))?;
        u32::from_str_radix(text.trim().trim_start_matches("0x"), 16).context("PCI id")
    };
    Ok((read("vendor")?, read("device")?))
}

/// The encoder of one PyroWave stream.
pub struct Encoder {
    api: Arc<ffi::Api>,
    device: ffi::Device,
    encoder: ffi::Encoder,
    config: Negotiated,
    interval: pyrowave::Interval,
    critical_fec: bool,
    critical_fec_percentage: u32,
    packets: Vec<ffi::Packet>,
    planes: convert::Planes,
    submitted: Option<Instant>,
}

// SAFETY: the library's handles are used from one thread at a time (`&mut self`), and the host
// moves an encoder to its stream thread once, as the Windows encoder.
unsafe impl Send for Encoder {}

fn check_config(config: &Negotiated) -> Result<()> {
    ensure!(
        !config.hdr && !config.sdr_10bit,
        "10-bit and HDR PyroWave encoding is not available on a Linux host yet"
    );
    ensure!(
        config.width > 0 && config.height > 0,
        "an empty PyroWave picture"
    );
    ensure!(
        config.yuv444 || (config.width.is_multiple_of(2) && config.height.is_multiple_of(2)),
        "4:2:0 PyroWave needs an even size"
    );
    Ok(())
}

/// The YCbCr range and chroma midpoint PyroWave's own conversion writes. An RGB picture (a
/// full-chroma capture, for PyroWave 4:4:4) is converted by the codec, which writes full range
/// unless told otherwise, so a limited-range stream asks for narrow range with the 8-bit
/// midpoint, as the client decodes it. An NV12 picture is already in the stream's range and
/// matrix (the capture's video processing made it) and is scaled without conversion.
fn output_range(planar: bool, full_range: bool) -> (vk::SamplerYcbcrRange, f32) {
    if planar || full_range {
        (vk::SamplerYcbcrRange::ITU_FULL, 0.5)
    } else {
        (vk::SamplerYcbcrRange::ITU_NARROW, 128.0 / 255.0)
    }
}

impl Encoder {
    /// An encoder for the negotiated picture on the GPU `options` names.
    pub fn new(config: &Negotiated, options: &Options) -> Result<Self> {
        check_config(config)?;
        if config.yuv444 && config.color_matrix() != 1 {
            // The codec's own RGB conversion has BT.709 coefficients only.
            tracing::warn!(
                color_matrix = config.color_matrix(),
                "PyroWave 4:4:4 is encoded with BT.709 coefficients; colours shift slightly on a client that asked for another matrix"
            );
        }
        let api = Arc::new(ffi::Api::load()?);
        let (vid, pid) = options.gpu.unwrap_or((0, 0));
        let mut device = ptr::null_mut();
        let mut result = -1;
        // The encode queue runs ahead of the game's work when the system allows it (the process
        // needs CAP_SYS_NICE); the library falls back by itself, so MEDIUM is the retry.
        for priority in [
            vk::QueueGlobalPriorityKHR::HIGH,
            vk::QueueGlobalPriorityKHR::MEDIUM,
        ] {
            // SAFETY: null identifiers are allowed; `device` is a valid out-pointer.
            result = unsafe {
                (api.create_device_by_compat)(
                    vid,
                    pid,
                    ptr::null(),
                    ptr::null(),
                    ptr::null(),
                    priority,
                    &mut device,
                )
            };
            if result == ffi::SUCCESS {
                break;
            }
        }
        ffi::check(result).context("creating the PyroWave Vulkan device")?;
        let mut this = Self {
            api,
            device,
            encoder: ptr::null_mut(),
            config: config.clone(),
            interval: pyrowave::Interval::new(
                butterpollo_media_policy::framegen::Rate(config.fps_millihz()).period(),
                true,
            ),
            critical_fec: config.pyrowave_records && options.critical_fec_percentage > 0,
            critical_fec_percentage: options.critical_fec_percentage,
            packets: Vec::new(),
            planes: convert::Planes::default(),
            submitted: None,
        };
        let info = ffi::EncoderCreateInfo {
            device,
            width: config.width as i32,
            height: config.height as i32,
            chroma: i32::from(config.yuv444),
        };
        // SAFETY: a live device and info; Drop destroys the encoder before the device.
        ffi::check(unsafe { (this.api.encoder_create)(&info, &mut this.encoder) })
            .context("creating the PyroWave encoder")?;
        // Pipelines compile on the first frame: do that now, not on the stream's first.
        this.warm_up()?;
        Ok(this)
    }

    /// Encodes one grey frame and drops it.
    fn warm_up(&mut self) -> Result<()> {
        let (width, height) = (self.config.width, self.config.height);
        let grey = Image {
            width,
            height,
            stride: width as usize * 4,
            bytes: vec![128; width as usize * height as usize * 4],
            captured: Instant::now(),
            pixel: Pixel::Bgra8,
        };
        let kbps = self.config.bitrate_kbps.max(1000);
        self.encode(&grey, true, kbps)?;
        Ok(())
    }

    fn budget(&mut self, kbps: u32) -> (usize, std::time::Duration) {
        let interval = self.interval.next(Instant::now(), false);
        let budget = pyrowave::budget_with_minimum(
            kbps,
            interval,
            self.config.packet_size,
            self.config.pyrowave_records,
            self.critical_fec_percentage,
            self.config.min_fec,
        );
        (budget, interval)
    }

    /// Encodes a BGRA picture in CPU memory (the codec probe's); its frame comes back from
    /// this call.
    pub fn encode(&mut self, image: &Image, _idr: bool, kbps: u32) -> Result<Vec<Encoded>> {
        ensure!(
            image.pixel == Pixel::Bgra8,
            "PyroWave on Linux takes 8-bit BGRA pictures only for now"
        );
        ensure!(
            (image.width, image.height) == (self.config.width, self.config.height),
            "the picture is {}x{}, the encoder {}x{}",
            image.width,
            image.height,
            self.config.width,
            self.config.height
        );
        let begin = Instant::now();
        let (budget, interval) = self.budget(kbps);
        if budget < 16 {
            return Ok(Vec::new());
        }
        let buffer = self.planes.fill(image, self.config.yuv444)?;
        let rate = ffi::RateControl {
            maximum_bitstream_size: budget,
        };
        // SAFETY: `buffer` points into `self.planes`, which outlives the synchronous call.
        ffi::check(unsafe { (self.api.encoder_encode_cpu)(self.encoder, &buffer, &rate) })
            .context("PyroWave CPU encode")?;
        self.submitted = Some(Instant::now());
        self.read_out(begin, image.captured, budget, kbps, interval)
    }

    /// Encodes a captured picture where it is: the DMA-BUF is imported as Vulkan memory and
    /// converted and scaled to the encoder's size by the codec's own compute pass.
    pub fn encode_dmabuf(
        &mut self,
        picture: &DmaBuf,
        captured: Instant,
        _idr: bool,
        kbps: u32,
    ) -> Result<Vec<Encoded>> {
        let begin = Instant::now();
        let (budget, interval) = self.budget(kbps);
        if budget < 16 {
            return Ok(Vec::new());
        }
        let (_, planar) = picture.format()?;
        let (ycbcr_range, ycbcr_chroma_midpoint) = output_range(planar, self.config.full_range());
        let imported = dmabuf::Imported::new(&self.api, self.device, picture)?;
        let reference = ffi::ExternalReference {
            image: imported.image,
            queue_family_index: vk::QUEUE_FAMILY_FOREIGN_EXT,
        };
        // The picture is written by another driver or process: the implicit fences of the
        // DMA-BUF order the reads, so no semaphore is passed.
        let acquire = ffi::SyncOperation {
            images: &reference,
            num_images: 1,
            sync: ffi::SyncPoint {
                semaphore: vk::Semaphore::null(),
                value: 0,
            },
        };
        let scaled = ffi::ScaledEncodeInfo {
            view: imported.view,
            input_color_space: vk::ColorSpaceKHR::SRGB_NONLINEAR,
            output_color_space: vk::ColorSpaceKHR::SRGB_NONLINEAR,
            intermediate_plane_format: vk::Format::R8_UNORM,
            ycbcr_chroma_midpoint,
            force_linear_filtering: false,
            skip_dither: false,
            crop_rect: ptr::null(),
            ycbcr_range,
            ycbcr_range_bit_depth: 0,
        };
        let rate = ffi::RateControl {
            maximum_bitstream_size: budget,
        };
        // SAFETY: every structure lives across the call, which records and submits before it
        // returns; the imported image outlives the read-out below.
        ffi::check(unsafe {
            (self.api.encoder_encode_gpu_scaled)(self.encoder, &acquire, &acquire, &scaled, &rate)
        })
        .context("PyroWave GPU encode")?;
        self.submitted = Some(Instant::now());
        let frames = self.read_out(begin, captured, budget, kbps, interval);
        drop(imported);
        frames
    }

    /// The bitstream of the frame just encoded, packetized and framed for the client.
    fn read_out(
        &mut self,
        begin: Instant,
        captured: Instant,
        budget: usize,
        kbps: u32,
        interval: std::time::Duration,
    ) -> Result<Vec<Encoded>> {
        let (mut raw, mut meta) = (ptr::null(), ptr::null());
        let (mut size, mut meta_size) = (0, 0);
        // SAFETY: valid out-pointers; the getters read the frame's host-visible readback, which
        // the blocking call waits for.
        ffi::check(unsafe {
            (self.api.encoder_get_mapped_raw_bitstream)(
                self.encoder,
                &mut raw,
                &mut size,
                &mut meta,
                &mut meta_size,
            )
        })?;
        ensure!(size <= 64 << 20, "the PyroWave bitstream exceeds the limit");
        let boundary = if self.config.pyrowave_records {
            u32::MAX as usize
        } else {
            pyrowave::PACKET_BOUNDARY
        };
        let mut count = 0;
        // SAFETY: a valid out-pointer after a successful encode.
        ffi::check(unsafe {
            (self.api.encoder_compute_num_packets_with_padding)(
                self.encoder,
                boundary,
                0,
                &mut count,
            )
        })?;
        ensure!(count > 0 && count <= 65535, "invalid PyroWave packet count");
        let mut bytes = crate::frame_pool::take_filled(size + 8);
        self.packets.resize(count, ffi::Packet::default());
        let mut written = 0;
        // SAFETY: `packets` holds `count` entries and `bytes` its own length.
        ffi::check(unsafe {
            (self.api.encoder_packetize_with_padding)(
                self.encoder,
                self.packets.as_mut_ptr(),
                boundary,
                0,
                &mut written,
                bytes.as_mut_ptr().cast(),
                bytes.len(),
            )
        })?;
        ensure!(
            written > 0 && written <= count,
            "invalid PyroWave packetization"
        );
        let limit = pyrowave::max_frame_bytes(self.config.packet_size, self.critical_fec);
        let framing = if self.config.pyrowave_records {
            ensure!(written == 1, "a record frame is one codec packet");
            let (offset, len) = (self.packets[0].offset, self.packets[0].size);
            let end = offset.checked_add(len).context("packet overflow")?;
            ensure!(end <= bytes.len(), "invalid codec record range");
            if offset > 0 {
                bytes.copy_within(offset..end, 0);
            }
            bytes.truncate(len);
            let payload = pyrowave::aligned_payload(self.config.packet_size);
            let framing =
                pyrowave::Framing::records(&bytes, payload, limit, self.config.pyrowave_row_order)?;
            let allowance = (f64::from(kbps) * 125. * interval.as_secs_f64()) as usize;
            let chosen = if framing.len() <= budget {
                Some(framing)
            } else {
                let critical = match framing.layout(payload) {
                    Some(stored) => stored.critical_shards,
                    None => pyrowave::layout(framing.view(&bytes)?, payload)?.critical_shards,
                };
                let minimum = self.config.min_fec.max(pyrowave::MIN_PARITY_SHARDS);
                let packets = (framing.len() + 8).div_ceil(self.config.packet_size - 16)
                    + pyrowave::critical_parity_bound(
                        critical,
                        self.critical_fec_percentage as usize,
                        minimum,
                    );
                (packets * (self.config.packet_size + pyrowave::WIRE_OVERHEAD) <= allowance)
                    .then_some(framing)
            };
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
            unit: Unit::Whole,
            idr: true,
            after_invalidation: false,
            frame_cap_bytes: None,
            reencoded: false,
            frame_cap_target: None,
            latency: Some(begin.elapsed()),
            presentation: Some(captured),
        };
        if encoded.wire_len() > limit {
            tracing::warn!(
                "PyroWave frame of {} bytes dropped over the transport limit of {limit}",
                encoded.wire_len()
            );
            return Ok(Vec::new());
        }
        Ok(vec![encoded])
    }

    /// Frames still in flight: none, encoding is synchronous.
    pub fn poll(&mut self) -> Result<Vec<Encoded>> {
        Ok(Vec::new())
    }
    pub fn finish(&mut self) -> Result<Vec<Encoded>> {
        Ok(Vec::new())
    }
    pub fn pending(&self) -> bool {
        false
    }
    pub fn backlog(&self) -> usize {
        0
    }
    pub fn last_submitted(&self) -> Option<Instant> {
        self.submitted
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: the encoder goes before its device, as the library requires.
        unsafe {
            if !self.encoder.is_null() {
                (self.api.encoder_destroy)(self.encoder);
            }
            if !self.device.is_null() {
                (self.api.device_destroy)(self.device);
            }
        }
    }
}

/// The optional-codec flags PyroWave adds to the host's capability word when this machine can
/// encode: 4:2:0 and 4:4:4 SDR (the Windows probe's bits `0x0080_0000` and `0x0100_0000`).
/// HDR modes (`0x0200_0000`, `0x0400_0000`) are not offered. The probe encodes one 640x480
/// frame in each mode.
pub fn probe_flags(options: &Options) -> Result<u32> {
    let image = Image {
        width: 640,
        height: 480,
        stride: 2560,
        bytes: vec![128; 640 * 480 * 4],
        captured: Instant::now(),
        pixel: Pixel::Bgra8,
    };
    let mut flags = 0;
    let mut errors = Vec::new();
    for (yuv444, bit) in [(false, 0x0080_0000), (true, 0x0100_0000)] {
        let mode = Negotiated {
            width: 640,
            height: 480,
            fps: 30,
            bitrate_kbps: 2000,
            codec: 3,
            yuv444,
            ..Default::default()
        };
        match Encoder::new(&mode, options).and_then(|mut e| e.encode(&image, true, 2000)) {
            Ok(frames) if !frames.is_empty() => flags |= bit,
            Ok(_) => errors.push(format!("4:4:4={yuv444}: no encoded frame")),
            Err(error) => errors.push(format!("4:4:4={yuv444}: {error:#}")),
        }
    }
    if flags == 0 {
        bail!("PyroWave has no usable codec mode: {}", errors.join("; "));
    }
    Ok(flags)
}
