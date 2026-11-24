//! Shared video encoder output types (`Unit`, `Encoded`), error formatting
//! (`SizeTooLarge`, AMF/NVENC failure guidance), backend selection, and cursor focus helpers.

use crate::capture::GpuImage;
use anyhow::Result;
use butterpollo_media_policy::{WarningSink, encoder_gpu};

/// How `Encoded::bytes` relates to a picture. Only slice delivery
/// (`video_slice_delivery`: AMF's slice output mode, NVENC's subframe
/// readback) returns anything but `Whole`: a P-frame then comes as `Slice`
/// units in order, ending with `Last`, all of one frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Whole,
    Slice { index: u32 },
    Last { index: u32 },
}

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
    /// Driver-read AMF frame limit, or NVENC's VBV bound, at submission. None
    /// means unavailable or unsupported; Some(0) means AMF's
    /// unrestricted/default setting.
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

/// A stream size the encoder cannot take: no driver update or sign in helps,
/// a lower resolution on the device does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SizeTooLarge {
    pub encoder: &'static str,
    pub width: u32,
    pub height: u32,
    /// The largest width and height, where the driver says them.
    pub max: Option<(u32, u32)>,
}
impl std::fmt::Display for SizeTooLarge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}x{} is larger than {} on this GPU can encode",
            self.width, self.height, self.encoder
        )?;
        if let Some((width, height)) = self.max {
            write!(f, " (at most {width}x{height})")?;
        }
        f.write_str("; choose a lower resolution on the device")
    }
}
impl std::error::Error for SizeTooLarge {}

/// The size error in `error`, said once with the adapter, in place of the
/// driver and sign-in advice that does not apply to it.
pub(super) fn size_failed(
    error: &anyhow::Error,
    encoder: &str,
    adapter: &str,
) -> Option<anyhow::Error> {
    let size = error.downcast_ref::<SizeTooLarge>()?;
    Some(anyhow::anyhow!(
        "{encoder} could not start on {adapter}: {size}"
    ))
}

/// Whether an AMF encoder that failed with a compute queue is tried again on
/// the graphics queue: not for a size the encoder cannot take on any queue.
pub(super) fn retries_on_graphics(error: &anyhow::Error) -> bool {
    error.downcast_ref::<SizeTooLarge>().is_none()
}

/// What a stream reports when AMF does not start on `adapter`.
pub(super) fn amf_failed(error: anyhow::Error, adapter: &str) -> anyhow::Error {
    if let Some(size) = size_failed(&error, "AMF", adapter) {
        return size;
    }
    error.context(format!(
        "AMF could not start on {adapter}. If Windows is at the sign-in screen or an RDP session was disconnected, sign in to and unlock the console session, then retry. Otherwise update the AMD graphics driver or select another codec on the device"
    ))
}

/// What a stream reports when NVENC does not start on `adapter`.
pub(super) fn nvenc_failed(error: anyhow::Error, adapter: &str) -> anyhow::Error {
    if let Some(size) = size_failed(&error, "NVENC", adapter) {
        return size;
    }
    // The cause names the apps holding the sessions; a driver update or
    // signing in frees none.
    if crate::nvenc::session_limit(&error) {
        return error.context(format!(
            "NVENC could not start on {adapter}. Close the app recording or streaming with it, then retry"
        ));
    }
    error.context(format!(
        "NVENC could not start on {adapter}. If Windows is at the sign-in screen or an RDP session was disconnected, sign in to and unlock the console session, then retry. Otherwise update the NVIDIA graphics driver or select another codec on the device"
    ))
}

/// Rubylight encodes only on an AMD or an NVIDIA GPU. A device on another
/// vendor's GPU is refused in words, naming the GPU to use when the PC has
/// one, and never handed to an encoder to fail obscurely.
pub(super) fn require_encoding_gpu(device: &crate::capture::Device) -> Result<()> {
    if encoder_gpu::encodes(device.vendor) {
        return Ok(());
    }
    require_encoding_among(
        &device.display.adapter,
        device.vendor,
        &crate::capture::gpus().unwrap_or_default(),
    )
}
pub(super) fn require_encoding_among(
    name: &str,
    vendor: u32,
    gpus: &[crate::capture::Gpu],
) -> Result<()> {
    let all = crate::capture::gpu_adapters(gpus);
    let adapter = encoder_gpu::Adapter {
        name: name.into(),
        vendor,
    };
    Ok(encoder_gpu::check_adapter(&adapter, &all)?)
}
/// AMF or NVENC for this device and the `encoder` setting
/// ([`butterpollo_media_policy::encoder_policy::backend`]).
pub(super) fn backend(
    device: &crate::capture::Device,
    tuning: &butterpollo_config::config::Config,
) -> Result<&'static str> {
    butterpollo_media_policy::encoder_policy::backend(
        tuning.get("encoder", "auto"),
        device.vendor,
        &device.display.adapter,
    )
}

/// Preserve startup warnings when the selected encoder adopts the session's
/// shared reporter; future encoder warnings must use that same reporter too.
pub(super) fn attach_warnings(
    current: &mut std::sync::Arc<dyn WarningSink>,
    reported: &std::sync::Arc<dyn WarningSink>,
) {
    current.replay(&**reported);
    *current = reported.clone();
}

/// The visible cursor's centre in `target` pixels, for `centre` focus.
pub fn focus_cursor(image: &GpuImage, (width, height): (u32, u32)) -> Option<(i32, i32)> {
    let cursor = image.cursor.as_ref()?;
    let scale = |position: i32, size: u32, source: u32, target: u32| {
        ((i64::from(position) + i64::from(size / 2)) * i64::from(target) / i64::from(source.max(1)))
            as i32
    };
    Some((
        scale(cursor.position[0], cursor.width, image.width, width),
        scale(cursor.position[1], cursor.height, image.height, height),
    ))
}
