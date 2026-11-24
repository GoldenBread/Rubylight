//! The video encoder of a stream. On Linux it is a VA-API session (H.264 High, HEVC Main and
//! AV1 profile 0, 8-bit 4:2:0, constant bitrate, one reference picture per picture) that encodes
//! a captured picture where it is in GPU memory, or a BGRA picture from system memory for the
//! codec probe. Encoding is synchronous: a call returns the picture it encoded, so nothing is
//! ever pending. 10-bit streams (HEVC Main10, AV1) are converted from the 8-bit picture by video
//! processing. HDR streams and 4:4:4 fail when the encoder is created on a captured picture; the
//! codec probe may ask for the HDR tags to find out the encoder takes them. The encoded-frame
//! types are as the senders read them.
use crate::{
    capture::{Device, GpuImage, Image, Pixel},
    vaapi::{
        Va,
        encoder::{Backend, Output},
        session::{Codec, Format},
    },
};
use anyhow::{Context, Result, bail};
use butterpollo_media_policy::{WarningSink, encoder_focus, rtsp::Negotiated};
use std::{sync::Arc, time::Instant};

/// How `Encoded::bytes` relates to a picture. Only slice delivery returns
/// anything but `Whole`: a P-frame then comes as `Slice` units in order,
/// ending with `Last`, all of one frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Whole,
    Slice { index: u32 },
    Last { index: u32 },
}

/// One encoded picture, or one slice of it.
pub struct Encoded {
    pub bytes: Vec<u8>,
    /// How `bytes` go on the wire. `None` sends them as they are. PyroWave
    /// sets its record framing or older clients' container here, as pieces of
    /// `bytes`, which the packetizer copies straight into packets
    /// ([`Self::payload`]): `bytes` then holds the codec's own output.
    pub framing: Option<butterpollo_media_policy::pyrowave::Framing>,
    pub unit: Unit,
    pub idr: bool,
    pub after_invalidation: bool,
    /// The encoder's frame size limit at submission. None means unavailable or
    /// unsupported; Some(0) means the encoder's unrestricted setting.
    pub frame_cap_bytes: Option<u64>,
    /// The encoder made this keyframe again in place of an oversize scene cut.
    pub reencoded: bool,
    /// The pace-based budget the host asked the encoder to hold this frame to
    /// (the last `set_frame_cap`), against which a keyframe's size is judged.
    pub frame_cap_target: Option<u64>,
    /// Submission through completed codec output, including asynchronous work.
    pub latency: Option<std::time::Duration>,
    pub presentation: Option<std::time::Instant>,
}
impl Encoded {
    /// The picture as the packetizer reads it: `bytes`, or `bytes` framed.
    pub fn payload(&self) -> Result<butterpollo_media_policy::pyrowave::Payload<'_>> {
        Ok(match &self.framing {
            None => butterpollo_media_policy::pyrowave::Payload::Bytes(&self.bytes),
            Some(framing) => framing.view(&self.bytes)?.into(),
        })
    }
    /// Bytes the picture takes on the wire, before packet headers.
    pub fn wire_len(&self) -> usize {
        self.framing.as_ref().map_or(
            self.bytes.len(),
            butterpollo_media_policy::pyrowave::Framing::len,
        )
    }
}
/// A large frame's buffer goes back to the encoders' pool
/// ([`crate::frame_pool`]) when the sender is done with it.
impl Drop for Encoded {
    fn drop(&mut self) {
        crate::frame_pool::give(std::mem::take(&mut self.bytes));
    }
}

/// The encoder of one stream. It offers none of the advanced features (frame caps, reference
/// invalidation, focus, overlays) and answers their queries as an encoder without them.
pub struct Encoder {
    backend: Backend,
    adapter: String,
    /// The device the encoder's surfaces live on.
    device: usize,
    /// When the last encode call handed its picture to the encoder.
    submitted: Option<Instant>,
}

/// Why an HDR stream is refused when it opens on a captured picture.
pub const NO_HDR_CAPTURE: &str = "HDR is not available on a Linux host yet: the capture gives \
    no HDR picture (gamescope's stream is SDR), and an SDR picture sent as HDR would look \
    wrong. Stream it as 10-bit SDR instead, or turn HDR off in the client";

/// The codec and picture format a negotiation asks for, or why this host cannot encode it.
/// `picture_is_hdr` is whether the pictures to come carry HDR: a stream's do not, as the
/// capture is SDR, so an HDR stream is refused; the codec probe's test picture only checks that
/// the 10-bit encoder works, so it may ask for the HDR tags.
pub(crate) fn format_of(config: &Negotiated, picture_is_hdr: bool) -> Result<(Codec, Format)> {
    if config.hdr && !picture_is_hdr {
        bail!(NO_HDR_CAPTURE);
    }
    if config.yuv444 {
        bail!("4:4:4 encoding is not available on a Linux host yet");
    }
    let codec = match config.codec {
        0 => Codec::H264,
        1 => Codec::Hevc,
        2 => Codec::Av1,
        other => bail!("codec {other} is not available on a Linux host yet"),
    };
    let depth = if config.ten_bit() { 10 } else { 8 };
    if depth == 10 && codec == Codec::H264 {
        bail!("H.264 has no 10-bit mode on this host; choose HEVC or AV1");
    }
    Ok((
        codec,
        Format {
            depth,
            hdr: config.hdr,
        },
    ))
}

impl Encoder {
    fn on(
        va: &Arc<Va>,
        device: &Device,
        config: &Negotiated,
        picture_is_hdr: bool,
    ) -> Result<Self> {
        let (codec, format) = format_of(config, picture_is_hdr)?;
        let backend = Backend::new(
            va,
            codec,
            format,
            config.width,
            config.height,
            config.fps.max(1),
        )
        .with_context(|| {
            format!(
                "the {} encoder could not start on {} ({}x{} at {} fps)",
                codec.name(),
                device.display.adapter,
                config.width,
                config.height,
                config.fps
            )
        })?;
        Ok(Self {
            backend,
            adapter: device.display.adapter.clone(),
            device: device.key(),
            submitted: None,
        })
    }

    /// The bytes of one encoded picture as the senders take them.
    fn encoded(output: Output, started: Instant) -> Vec<Encoded> {
        vec![Encoded {
            bytes: output.bytes,
            framing: None,
            unit: Unit::Whole,
            idr: output.idr,
            after_invalidation: output.recovered,
            frame_cap_bytes: None,
            reencoded: false,
            frame_cap_target: None,
            latency: Some(started.elapsed()),
            presentation: None,
        }]
    }

    /// Best-effort pace-based frame cap; the cap in effect, `None` without one.
    pub fn set_frame_cap(&mut self, _bytes: u64, _floor: u64) -> Option<u64> {
        None
    }
    pub fn frame_cap(&self) -> Option<u64> {
        None
    }
    /// The largest inter frame sent as encoded.
    pub fn set_cut_limit(&mut self, _bytes: u64) {}
    /// Inter frames encoded again as keyframes since the encoder opened.
    pub fn reencoded_cuts(&self) -> u64 {
        0
    }
    pub fn keyframes_over_cap(&self) -> u64 {
        0
    }
    /// How far under the pace-based target the cap is written; 1 without a scale.
    pub fn keyframe_cap_scale(&self) -> f64 {
        1.
    }
    /// The HDR10 metadata a stream with HDR pictures carries; SDR streams carry none.
    pub fn set_hdr_metadata(&mut self, metadata: butterpollo_media_policy::hdr::Metadata) {
        self.backend.set_hdr_metadata(metadata);
    }
    /// The receiver's display peak, for dynamic metadata that names a target
    /// display.
    pub fn set_hdr_target(&mut self, _nits: u16) {}
    /// SDR white as absolute luminance, and the scRGB scale.
    pub fn set_luminance(&mut self, _white_nits: f32, _linear_scale: f32) {}
    /// The next picture repeats the one last encoded.
    pub fn set_repeat(&mut self, _repeat: bool) {}
    /// An encoder for pictures on the capture's GPU.
    pub fn new_gpu(config: &Negotiated, image: &GpuImage) -> Result<Self> {
        Self::new_gpu_options(
            config,
            image,
            &butterpollo_config::config::Config::default(),
        )
    }
    pub fn new_gpu_options(
        config: &Negotiated,
        image: &GpuImage,
        _tuning: &butterpollo_config::config::Config,
    ) -> Result<Self> {
        Self::on(image.gpu.va(), &image.gpu, config, false)
    }
    /// [`Self::new_gpu_options`], with the session's warnings.
    pub fn new_gpu_reported(
        config: &Negotiated,
        image: &GpuImage,
        tuning: &butterpollo_config::config::Config,
        warnings: &std::sync::Arc<dyn WarningSink>,
    ) -> Result<Self> {
        warnings.clear("encoder_conversion");
        warnings.clear("encoder_frame_size");
        warnings.clear("encoder_intra_refresh");
        warnings.clear("hdr_dynamic_metadata");
        warnings.clear("encoder_vpe");
        Self::new_gpu_options(config, image, tuning)
    }
    /// Encodes a captured picture where it is, at `bitrate` kilobits a second.
    pub fn encode_gpu(
        &mut self,
        image: &GpuImage,
        idr: bool,
        bitrate: u32,
    ) -> Result<Vec<Encoded>> {
        let started = Instant::now();
        let surface = image
            .surface()
            .context("the picture is not on a VA-API surface")?;
        self.submitted = Some(started);
        let output = self.backend.encode_surface(
            surface,
            (image.width, image.height),
            idr,
            bitrate.saturating_mul(1000),
        )?;
        Ok(Self::encoded(output, started))
    }
    /// The encoder's own colour converter gave way and the encoder should be rebuilt
    /// without it.
    pub fn converter_gave_way(&self) -> bool {
        false
    }
    /// Whether this encoder's frames are converted on a compute queue.
    pub fn converts_on_compute(&self) -> bool {
        false
    }
    /// Whether the encoder fell back from its compute session to the graphics queue on
    /// this creation.
    pub fn compute_fell_back(&self) -> bool {
        false
    }
    /// Whether the picture is on the device the encoder was made on.
    pub fn accepts_gpu_device(&self, image: &GpuImage) -> bool {
        image.gpu.key() == self.device
    }
    pub fn pending(&self) -> bool {
        false
    }
    /// An event that ends a wait for output as soon as the oldest frame is
    /// done. `None` keeps the caller's output poll.
    pub fn output_event(&self) -> Option<&crate::timing::Signal> {
        None
    }
    /// Whether a poll can wait in the driver.
    pub fn poll_blocks(&self) -> bool {
        false
    }
    /// The earliest a poll can find output. `None` polls as before.
    pub fn output_due(&self) -> Option<std::time::Instant> {
        None
    }
    /// When the encoder accepted the input of the last encode call, after
    /// its colour conversion.
    pub fn last_submitted(&self) -> Option<std::time::Instant> {
        self.submitted
    }
    /// Frames the GPU encoder has accepted and not yet finished.
    pub fn backlog(&self) -> usize {
        0
    }
    pub fn backlog_limit(&self) -> usize {
        butterpollo_media_policy::encoder_backlog::DEFAULT_LIMIT
    }
    pub fn recovery_backlog_limit(&self) -> usize {
        self.backlog_limit()
    }
    pub fn recovery_lead(&self) -> Option<std::time::Duration> {
        None
    }
    pub fn log_stall(&self) {}
    pub fn device_removed(&self) -> Option<crate::device_loss::DeviceLost> {
        None
    }
    /// The GPU the encoder opened on, for wording what it cannot do.
    pub fn adapter(&self) -> Option<&str> {
        Some(&self.adapter)
    }
    /// An integer capability of the encoder, such as its throughput limit.
    pub fn capability(&self, _name: &str) -> Option<i64> {
        None
    }
    /// The encoder family, as the encoder setting names it.
    pub fn backend(&self) -> &'static str {
        "vaapi"
    }
    /// Whether a loss report can be answered with a picture that refers to an older one.
    pub fn supports_invalidation(&self) -> bool {
        self.backend.supports_invalidation()
    }
    /// Pictures numbered `first` to `last` on the wire did not arrive: the next picture refers
    /// to one from before them. False asks for a keyframe, as no picture held is that old.
    pub fn invalidate_ref_frames(&mut self, first: u64, last: u64) -> bool {
        self.backend.invalidate(first, last)
    }
    /// Whether the repair needs a keyframe as a backstop. An older picture is a plain
    /// reference every decoder follows, so it does not.
    pub fn needs_recovery_confirmation(&self) -> bool {
        false
    }
    pub fn poll(&mut self) -> Result<Vec<Encoded>> {
        Ok(vec![])
    }
    /// Waits for every frame still in flight and returns them.
    pub fn finish(&mut self) -> Result<Vec<Encoded>> {
        Ok(vec![])
    }
    /// Blocks a P-frame is sent in as its slices leave the encoder; 0 when
    /// slices are not delivered apart.
    pub fn slice_delivery(&self) -> u8 {
        0
    }
    /// Slice delivery is off for the rest of the session: later frames come
    /// back whole.
    pub fn join_slices(&mut self) {}
    /// A recreated encoder must keep the wire frame numbers used by RFI.
    pub fn set_next_frame(&mut self, frame: u64) {
        self.backend.set_next_frame(frame);
    }
    pub fn new(config: &Negotiated, display: &str) -> Result<Self> {
        Self::new_options(
            config,
            display,
            &butterpollo_config::config::Config::default(),
        )
    }
    /// An encoder for CPU images, such as the codec probe's, on the GPU the adapter settings
    /// name.
    pub fn new_options(
        config: &Negotiated,
        display: &str,
        tuning: &butterpollo_config::config::Config,
    ) -> Result<Self> {
        format_of(config, true)?;
        let device = Device::new_adapter(
            display,
            tuning.get("adapter_name", ""),
            tuning.get("adapter_pnp_id", ""),
        )?;
        Self::on(device.va(), &device, config, true)
    }
    /// Encodes a BGRA picture in system memory, at `bitrate` kilobits a second.
    pub fn encode(&mut self, image: &Image, idr: bool, bitrate: u32) -> Result<Vec<Encoded>> {
        if image.pixel != Pixel::Bgra8 {
            bail!(
                "the encoder takes 8-bit BGRA pictures from system memory, not {:?}",
                image.pixel
            );
        }
        let started = Instant::now();
        self.submitted = Some(started);
        let output = self.backend.encode_bgra(
            (image.width, image.height, image.stride, &image.bytes),
            idr,
            bitrate.saturating_mul(1000),
        )?;
        Ok(Self::encoded(output, started))
    }
}
// The couch menu's hook.
impl Encoder {
    /// Draw the couch menu's panel into the next pictures, or stop with
    /// `None`. No picture is encoded on Linux, so none gets a panel.
    pub fn set_overlay(&mut self, _overlay: Option<crate::overlay::Overlay>) {}
    /// Whether a panel is set.
    pub fn has_overlay(&self) -> bool {
        false
    }
}
// Encoder focus: an importance map per codec block. No Linux encoder takes one.
impl Encoder {
    /// Why there is no map.
    fn no_focus(&self) -> encoder_focus::Reason {
        encoder_focus::Reason::CodecUnsupported
    }
    /// The map's settings for the next frames, or `None` for no map. A map is refused.
    pub fn set_focus(&mut self, settings: Option<crate::compute::FocusSettings>) -> Result<()> {
        match settings {
            Some(_) => Err(anyhow::anyhow!(
                "encoder focus: {}",
                self.no_focus().message()
            )),
            None => Ok(()),
        }
    }
    pub fn set_focus_verify(&mut self, _every: Option<u32>) {}
    pub fn focus_stats(&self) -> std::result::Result<encoder_focus::Stats, encoder_focus::Reason> {
        Err(self.no_focus())
    }
    /// Why a map failed on a frame since the last call.
    pub fn take_focus_failure(&mut self) -> Option<anyhow::Error> {
        None
    }
    /// The startup check: unsupported, with the reason.
    pub fn probe_focus(&mut self) -> Result<encoder_focus::Support> {
        Ok(encoder_focus::Support::Unsupported {
            reason: self.no_focus(),
            message: self.no_focus().message().into(),
        })
    }
}
/// The visible cursor's centre in `target` pixels, for `centre` focus. `None`: no capture
/// on Linux supplies the cursor apart from the picture.
pub fn focus_cursor(_image: &GpuImage, _target: (u32, u32)) -> Option<(i32, i32)> {
    None
}
