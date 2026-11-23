//! The decoder check behind "Prefer YUV 4:4:4" and "10-bit color (SDR)": decodes a fixture of each
//! 4:4:4 and 10-bit format with the decoder the stream will use and checks that the picture comes
//! out in a shape the presenter draws. Its result is `color::offer`'s `decodable` and reason.
//!
//! A hardware decoder is opened as required hardware: FFmpeg would otherwise decode a profile its
//! hardware lacks in software without a word, and the stream would not use the decoder it names.

use nightjar_core::sdp::*;
use sdl3_sys::everything::*;

use crate::media;

/// The fixtures: standard lossy 256x256 4:4:4 pictures (at least NVDEC's minimum size, Level 2.1,
/// no lossless transform/quantization bypass, matching real NVENC streams so hardware decoders on
/// NVIDIA and Intel accept them — lossless Level 8.5 fixtures were rejected, #169) and 256x256
/// Main10 ones. The 10-bit 4:4:4 one is 16 flat blocks of known codes (`media`'s software test
/// lists them).
const FIXTURES: [(u32, media::Codec, &str, &[u8]); 6] = [
    (
        VIDEO_FORMAT_H264_HIGH8_444,
        media::Codec::H264,
        "H.264 4:4:4",
        include_bytes!("../../tests/fixtures/h264-444-8.bin"),
    ),
    (
        VIDEO_FORMAT_H265_REXT8_444,
        media::Codec::Hevc,
        "HEVC 4:4:4",
        include_bytes!("../../tests/fixtures/hevc-444-8.bin"),
    ),
    (
        VIDEO_FORMAT_AV1_HIGH8_444,
        media::Codec::Av1,
        "AV1 4:4:4",
        include_bytes!("../../tests/fixtures/av1-444-8.bin"),
    ),
    (
        VIDEO_FORMAT_H265_MAIN10,
        media::Codec::Hevc,
        "HEVC Main10",
        include_bytes!("../../tests/fixtures/hevc-420-10.bin"),
    ),
    (
        VIDEO_FORMAT_AV1_MAIN10,
        media::Codec::Av1,
        "AV1 Main10",
        include_bytes!("../../tests/fixtures/av1-420-10.bin"),
    ),
    (
        VIDEO_FORMAT_H265_REXT10_444,
        media::Codec::Hevc,
        "HEVC 4:4:4 10-bit",
        include_bytes!("../../tests/fixtures/hevc-444-10.bin"),
    ),
];

/// The plain HEVC and AV1 formats, and the Main10 ones HDR offers, whose hardware decoding
/// [`probe_base`] checks before they are offered: single 256x256 4:2:0 key frames, above every
/// backend's smallest picture (NVDEC's HEVC minimum is 144x144, which 128x128 ones were under).
const BASE_FIXTURES: [(u32, media::Codec, &str, &[u8]); 4] = [
    (
        VIDEO_FORMAT_H265,
        media::Codec::Hevc,
        "HEVC",
        include_bytes!("../../tests/fixtures/hevc-420-8.bin"),
    ),
    (
        VIDEO_FORMAT_H265_MAIN10,
        media::Codec::Hevc,
        "HEVC Main10",
        include_bytes!("../../tests/fixtures/hevc-420-10.bin"),
    ),
    (
        VIDEO_FORMAT_AV1_MAIN8,
        media::Codec::Av1,
        "AV1",
        include_bytes!("../../tests/fixtures/av1-420-8.bin"),
    ),
    (
        VIDEO_FORMAT_AV1_MAIN10,
        media::Codec::Av1,
        "AV1 Main10",
        include_bytes!("../../tests/fixtures/av1-420-10.bin"),
    ),
];

/// The formats [`probe_base`] checks. H.264 is always offered.
pub const BASE_FORMATS: u32 =
    VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_MAIN10 | VIDEO_FORMAT_AV1_MAIN8 | VIDEO_FORMAT_AV1_MAIN10;

/// The size of every fixture above, which its decoder is opened for (Nightjar issue 77): on
/// Qualcomm Iris a V4L2 M2M decoder opened without a size reinitialises its buffers on the
/// driver's first format event, and the probe then saw no picture.
const FIXTURE_WIDTH: u32 = 256;
const FIXTURE_HEIGHT: u32 = 256;

/// What a fixture of `format` is decoded as, opened for [`FIXTURE_WIDTH`]x[`FIXTURE_HEIGHT`].
fn fixture_pictures(format: u32, vaapi_node: media::VaapiNode) -> media::Pictures {
    media::Pictures {
        ten_bit: format & VIDEO_FORMAT_MASK_10BIT != 0,
        chroma444: format & VIDEO_FORMAT_MASK_YUV444 != 0,
        packed: false,
        waiting: 0,
        vaapi_node,
        width: FIXTURE_WIDTH,
        height: FIXTURE_HEIGHT,
        probe_only: true,
    }
}

/// What [`probe_base`] found: the formats of [`BASE_FORMATS`] the stream's decoder decodes, why
/// the others are not offered, and the backend that decoded each (what reference frame
/// invalidation is asked for by, `connect::video_capabilities`).
#[derive(Clone, Debug, PartialEq)]
pub struct Base {
    pub decodable: u32,
    pub reason: String,
    pub backends: Vec<(u32, media::Hardware)>,
}

impl Base {
    /// Software decoding: every format, by the software decoder.
    pub fn software() -> Self {
        Self {
            decodable: BASE_FORMATS,
            reason: String::new(),
            backends: BASE_FIXTURES
                .iter()
                .map(|&(format, ..)| (format, media::Hardware::None))
                .collect(),
        }
    }
}

/// The formats of [`BASE_FORMATS`] that `hardware` decodes in hardware. Each is decoded with the
/// stream's decoder order as required hardware: without the check a format the GPU lacks (HEVC
/// Main10 or AV1 on an older one) would be offered, and FFmpeg would decode it in software, or
/// for AV1 not at all. Software decoding decodes them all and is not checked. Only the formats of
/// `wanted` are decoded (`negotiate::base_probe_formats`): the others are not offered anyway.
pub fn probe_base(hardware: media::Hardware, wanted: u32, budget: &mut ProbeBudget) -> Base {
    if hardware == media::Hardware::None {
        return Base::software();
    }
    let mut base = Base {
        decodable: 0,
        reason: String::new(),
        backends: Vec::new(),
    };
    if wanted == 0 { return base; }
    let mut failures = Vec::new();
    let probe = media::ProbeHardware::isolate(hardware);
    // VAAPI on the GPU the stream will decode on.
    let vaapi_node = media::vaapi_node(hardware);
    for (format, codec, name, data) in BASE_FIXTURES {
        if format & wanted == 0 {
            continue;
        }
        let pictures = fixture_pictures(format, vaapi_node);
        match decode(probe.hardware, pictures, codec, data, budget) {
            Ok((_, backend)) => {
                eprintln!("nightjar: decoder probe: {name} {} ok", backend.label());
                base.decodable |= format;
                let reported = if matches!(backend, media::Hardware::D3d11(_)) { hardware } else { backend };
                base.backends.push((format, reported));
            }
            Err(reason) => {
                eprintln!("nightjar: decoder probe: {name} failed, so it is not offered: no hardware decoder decodes it here: {reason}");
                failures.push(format!("{name}: {reason}"));
            }
        }
    }
    base.reason = failures.join("; ");
    base
}

/// The formats of `color::YUV444_FORMATS`, `color::SDR10_FORMATS` and `color::YUV444_10_FORMATS`
/// that `hardware` decodes into pictures the presenter draws, and why the others are not ("" when
/// all are). `planes` says whether planar pictures (R16 planes when its argument is true) can be
/// drawn. Only the formats of `wanted` are decoded (`negotiate::color_probe_formats`): the others are not offered anyway.
pub fn probe(
    hardware: media::Hardware,
    wanted: u32,
    planes: impl Fn(bool) -> Result<(), String>,
    budget: &mut ProbeBudget,
) -> (u32, String) {
    let mut decodable = 0;
    if wanted == 0 { return (0, String::new()); }
    let mut failures = Vec::new();
    let probe = media::ProbeHardware::isolate(hardware);
    // VAAPI on the GPU the stream will decode on.
    let vaapi_node = media::vaapi_node(hardware);
    for (format, codec, name, data) in FIXTURES {
        if format & wanted == 0 {
            continue;
        }
        let yuv444 = format & VIDEO_FORMAT_MASK_YUV444 != 0;
        let pictures = fixture_pictures(format, vaapi_node);
        match decode(probe.hardware, pictures, codec, data, budget)
            .and_then(|(info, backend)| {
                drawable(&info, yuv444, &planes)?;
                keeps_depth(&info, pictures.ten_bit)?;
                Ok(backend)
            })
        {
            Ok(backend) => {
                eprintln!("nightjar: decoder probe: {name} {} ok", backend.label());
                decodable |= format;
            }
            Err(reason) => {
                eprintln!("nightjar: decoder probe: {name} failed, so it is not offered: {reason}");
                failures.push(format!("{name}: {reason}"));
            }
        }
    }
    #[cfg(windows)]
    if let Some(hint) = other_gpu_hint(hardware, wanted & !decodable) {
        failures.push(hint);
    }
    (decodable, failures.join("; "))
}

/// When HEVC 4:4:4 (of `failed`) does not decode on the renderer's GPU, D3D11VA's, but does on
/// another GPU of this PC (a laptop drawing on its integrated GPU): how to move Nightjar there.
#[cfg(windows)]
fn other_gpu_hint(hardware: media::Hardware, failed: u32) -> Option<String> {
    let media::Hardware::D3d11(device) = hardware else { return None };
    let ten_bit = if failed & VIDEO_FORMAT_H265_REXT8_444 != 0 {
        false
    } else if failed & VIDEO_FORMAT_H265_REXT10_444 != 0 {
        true
    } else {
        return None;
    };
    let depth = if ten_bit { " 10-bit" } else { "" };
    match crate::gpu_adapters::others_decoding_hevc444(device, ten_bit)? {
        (renderer, crate::gpu_adapters::Hevc444Gpus::Renderer) => {
            // Moving to another GPU would not help: the decoder itself lacks the profile.
            eprintln!("nightjar: decoder probe ran on {renderer}, whose DXVA lists HEVC 4:4:4{depth}, so this build's decoder failed, not the GPU choice");
            None
        }
        (renderer, crate::gpu_adapters::Hevc444Gpus::Others(others)) => {
            eprintln!("nightjar: decoder probe ran on {renderer}; other GPUs that decode HEVC 4:4:4{depth}: {others:?}");
            others.first().map(|other| crate::gpu_adapters::hint(&renderer, other))
        }
    }
}

fn decode(
    hardware: media::Hardware,
    pictures: media::Pictures,
    codec: media::Codec,
    data: &[u8],
    budget: &mut ProbeBudget,
) -> Result<(media::FrameInfo, media::Hardware), String> {
    if hardware == media::Hardware::None {
        let mut decoder = media::VideoDecoder::open(codec, hardware, 1, false)?;
        return decode_with(&mut decoder, data, budget).map(|info| (info, hardware));
    }
    let order = hardware.order();
    let mut attempt_order = order.clone();
    let mut attempt = 0;
    loop {
        // The stream's decoder order, as the stream goes on after a backend that fails mid-stream.
        let tried = first_that_decodes(attempt_order.clone(), |order| {
            let mut decoder = media::VideoDecoder::open_any(codec, pictures, order, true, |_, _| {})?;
            // VAAPI's 4:4:4 picture is one interleaved surface the presenter draws only mapped
            // into the GPU renderer's Vulkan images, as the stream maps it.
            if pictures.chroma444 && decoder.hardware == media::Hardware::Vaapi {
                super::pipeline::map_to_vulkan(hardware, &mut decoder);
            }
            Ok((decoder.hardware, decode_with(&mut decoder, data, budget)))
        });
        match tried {
            // A V4L2 M2M decoder that made no picture gets one more go on a decoder opened anew:
            // on Qualcomm Iris the first open of a run sometimes never starts decoding (Nightjar
            // issue 77: the fixture's picture never came), while a second open decodes it. The
            // backends before it already did their part, so the retry starts at V4L2. A start's
            // checks share one budget for the long waits and the retry (see ProbeBudget).
            Err(reason) if attempt < V4L2_PROBE_RETRIES && retry_worthwhile(&order, &reason) => {
                if !budget.retry() {
                    eprintln!(
                        "nightjar: decoder probe: V4L2 M2M made no picture of the fixture ({reason}), and a start's {} of V4L2 probe time is spent; not opening it again",
                        V4L2_PROBE_BUDGET.as_millis()
                    );
                    return Err(reason);
                }
                attempt += 1;
                eprintln!(
                    "nightjar: decoder probe: V4L2 M2M made no picture of the fixture ({reason}); trying it once more on a decoder opened anew"
                );
                std::thread::sleep(V4L2_PROBE_SETTLE);
                attempt_order = order.iter().copied().skip_while(|&backend| backend != media::Hardware::V4l2m2m).collect();
            }
            other => return other,
        }
    }
}

/// How many times a [V4L2 M2M](media::Hardware::V4l2m2m) decoder that made no picture of a
/// fixture is opened anew ([`decode`]): one, so a stream's probes cannot hang on a device.
const V4L2_PROBE_RETRIES: usize = 1;

/// How long all the decoder checks of one start may spend waiting for a V4L2 M2M picture, across
/// every format they check ([`ProbeBudget`]): Nightjar issue 77's Iris sometimes never makes the
/// first picture of an open, and the checks decode up to five HEVC fixtures (V4L2 M2M has no AV1
/// decoder in this FFmpeg); a second and a half each, and a retry each, would add many seconds to
/// a start.
const V4L2_PROBE_BUDGET: std::time::Duration = std::time::Duration::from_millis(2_000);

/// What the decoder checks of one start have left of [`V4L2_PROBE_BUDGET`]: every V4L2 M2M attempt
/// is charged the time it waited, picture or not, and [`decode`] takes a retry's settle from it
/// too. Once it is spent, an attempt still waits [`ASYNC_PROBE_FLOOR`] (as every attempt did before
/// the budget) and nothing is retried, so a decoder that answers late is not called undecodable
/// because an earlier format used the budget up.
#[derive(Clone, Copy, Debug)]
pub struct ProbeBudget {
    left: std::time::Duration,
}

impl Default for ProbeBudget {
    fn default() -> Self {
        Self { left: V4L2_PROBE_BUDGET }
    }
}

impl ProbeBudget {
    /// How long to wait for a picture of this attempt: what is left, at most [`ASYNC_PROBE_WAIT`],
    /// at least [`ASYNC_PROBE_FLOOR`].
    fn wait(&self) -> std::time::Duration {
        self.left.min(ASYNC_PROBE_WAIT).max(ASYNC_PROBE_FLOOR)
    }

    /// An attempt waited `waited` of the budget.
    fn spent(&mut self, waited: std::time::Duration) {
        self.left = self.left.saturating_sub(waited);
    }

    /// Whether a decoder opened anew may be tried, charging its settle to the budget when it may.
    fn retry(&mut self) -> bool {
        if self.left <= V4L2_PROBE_SETTLE {
            return false;
        }
        self.left -= V4L2_PROBE_SETTLE;
        true
    }

    #[cfg(test)]
    fn left(&self) -> std::time::Duration {
        self.left
    }
}

/// How long [`decode`] lets a V4L2 M2M decoder release its device between the two opens: on
/// Qualcomm Iris the decoder's firmware and buffers are gone by then, as they are for a new process.
const V4L2_PROBE_SETTLE: std::time::Duration = std::time::Duration::from_millis(200);

/// Whether a failed probe is worth the one more go of [`V4L2_PROBE_RETRIES`]: the order opens a
/// V4L2 M2M decoder (the one that can behave so on Iris) and that decoder made no picture, rather
/// than the device refusing to open (a retry cannot fix that) or another backend failing.
fn retry_worthwhile(order: &[media::Hardware], reason: &str) -> bool {
    let label = media::Hardware::V4l2m2m.label();
    order.contains(&media::Hardware::V4l2m2m)
        && reason.split("; ").any(|failure| failure.starts_with(label) && failure.contains("made no picture"))
}

/// The first backend of `order` that decodes: `attempt` opens the first of the backends it is
/// given that opens (the error names each that did not) and decodes with it. A backend that opens
/// but fails on the fixture has the ones after it tried, not just the first that opens.
fn first_that_decodes<T>(
    mut order: Vec<media::Hardware>,
    mut attempt: impl FnMut(
        Vec<media::Hardware>,
    ) -> Result<(media::Hardware, Result<T, String>), String>,
) -> Result<(T, media::Hardware), String> {
    let mut failures = Vec::new();
    loop {
        match attempt(order.clone()) {
            Ok((backend, Ok(decoded))) => return Ok((decoded, backend)),
            Ok((backend, Err(reason))) => {
                failures.push(format!("{}: {reason}", backend.label()));
                order = super::pipeline::backends_after(order, backend);
                if order.is_empty() {
                    break;
                }
            }
            Err(reason) => {
                failures.push(reason);
                break;
            }
        }
    }
    Err(failures.join("; "))
}

/// How long [`decode_with`] waits for an asynchronous decoder's (V4L2 M2M's) picture of a
/// fixture: a lone frame is held until the input ends and the driver then decodes it. A quarter
/// second was too short on Qualcomm Iris, where the first picture of a decoder waits for its
/// firmware (Nightjar issue 77); stock FFmpeg decodes the same fixture there in under 250 ms, so
/// this stays well inside a stream's startup.
const ASYNC_PROBE_WAIT: std::time::Duration = std::time::Duration::from_millis(1_500);

/// The least [`decode_with`] waits for an asynchronous decoder's picture, budget spent or not: the
/// quarter second every attempt had before [`ProbeBudget`].
const ASYNC_PROBE_FLOOR: std::time::Duration = std::time::Duration::from_millis(250);

/// How often [`decode_with`] asks an asynchronous decoder again for the picture.
const ASYNC_PROBE_POLL: std::time::Duration = std::time::Duration::from_millis(5);

/// Decodes the fixture `data` with `decoder`, required to stay its backend.
fn decode_with(decoder: &mut media::VideoDecoder, data: &[u8], budget: &mut ProbeBudget) -> Result<media::FrameInfo, String> {
    let status = decoder.send(data, 0);
    if status < 0 {
        return Err(format!("the decoder rejected the fixture ({status}: {})", media::error(status)));
    }
    // An asynchronous decoder (V4L2 M2M) holds a lone frame until it is told the input ended,
    // then says "no picture yet" while the driver decodes.
    let deadline = if decoder.asynchronous() {
        let status = decoder.drain();
        if status < 0 {
            return Err(format!("the decoder refused the end of input ({status}: {})", media::error(status)));
        }
        let now = std::time::Instant::now();
        Some((now + budget.wait(), now))
    } else {
        None
    };
    let mut received = decoder.receive();
    while matches!(received, media::Received::None)
        && deadline.is_some_and(|(deadline, _)| std::time::Instant::now() < deadline)
    {
        std::thread::sleep(ASYNC_PROBE_POLL);
        received = decoder.receive();
    }
    if let Some((_, started)) = deadline {
        budget.spent(started.elapsed());
    }
    match received {
        media::Received::Frame(_) if decoder.refused() => {
            Err("the picture came out in memory, not the hardware decoder's".into())
        }
        media::Received::Frame(frame) => Ok(frame.info()),
        media::Received::None => Err("the decoder made no picture of the fixture".into()),
        media::Received::Error => Err("the decoder failed on the fixture".into()),
    }
}

/// Whether a decoded fixture picture is one the presenter draws as what was asked for.
fn drawable(
    info: &media::FrameInfo,
    yuv444: bool,
    planes: &impl Fn(bool) -> Result<(), String>,
) -> Result<(), String> {
    // A Vulkan video picture is drawn with the plane shader too, from its copied planes, and a
    // D3D11VA 4:4:4 surface (AYUV, Y410, Y416) from its copy.
    let vulkan = media::PlanarLayout::of_vulkan(info);
    let planar = media::PlanarLayout::of(info).or(vulkan).or_else(|| media::PlanarLayout::of_d3d11(info));
    if yuv444 && !planar.is_some_and(|layout| layout.chroma444) {
        return Err(format!(
            "the decoder's pictures (kind {}) are not 4:4:4 pictures this app draws",
            info.kind
        ));
    }
    if !yuv444
        && (info.ten_bit == 0
            || !(vulkan.is_some()
                || matches!(
                    info.kind,
                    media::FRAME_P010
                        | media::FRAME_VIDEOTOOLBOX
                        | media::FRAME_D3D11
                        | media::FRAME_YUV420P10
                        | media::FRAME_YUV444P10
                )))
    {
        return Err(format!(
            "the decoder's pictures (kind {}) are not 10-bit pictures this app draws",
            info.kind
        ));
    }
    match planar {
        Some(layout) => planes(layout.ten_bit),
        None => Ok(()),
    }
}

/// Whether a decoded fixture picture kept the depth asked for: a 10-bit 4:4:4 format decoded into
/// 8-bit pictures would be drawn without its two low bits.
fn keeps_depth(info: &media::FrameInfo, ten_bit: bool) -> Result<(), String> {
    if ten_bit && info.ten_bit == 0 {
        return Err(format!("the decoder's pictures (kind {}) are 8-bit, not the 10-bit pictures asked for", info.kind));
    }
    Ok(())
}

/// Whether `renderer` draws planar pictures: the plane shader pass of its renderer. The renderer's
/// device is passed to [`planes_drawable_device`], which a launch's worker runs with the device it
/// holds (the renderer itself stays on its own thread).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))] // macOS checks it at start.
pub fn planes_drawable(renderer: *mut SDL_Renderer, ten_bit: bool) -> Result<(), String> {
    #[cfg(windows)]
    let device = crate::render_device::live_renderer_pointer(renderer, SDL_PROP_RENDERER_D3D11_DEVICE_POINTER);
    #[cfg(unix)]
    let device = crate::sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_GPU_DEVICE_POINTER);
    #[cfg(not(any(windows, unix)))]
    let device = std::ptr::null_mut();
    planes_drawable_device(device, ten_bit)
}

/// [`planes_drawable`] from the renderer's device alone: the Direct3D 11 device on Windows, the
/// SDL GPU device on Unix. Null is a renderer without the device the pass needs. The Windows pass
/// only compiles its shaders; the Unix one builds the plane shader on SDL's GPU device, which the
/// settings probe also does off the presenting thread (`pyrowave_mode::settings_probe`).
pub fn planes_drawable_device(device: *mut std::ffi::c_void, ten_bit: bool) -> Result<(), String> {
    if device.is_null() {
        return Err(if cfg!(windows) {
            "drawing these pictures needs the Direct3D 11 renderer".into()
        } else {
            "drawing these pictures needs the SDL GPU renderer".into()
        });
    }
    #[cfg(windows)]
    {
        let _ = ten_bit;
        super::d3d11::PlanesPass::compiles()
    }
    #[cfg(unix)]
    {
        // SAFETY: The renderer's live GPU device, held by the probe's caller for the whole check.
        unsafe { super::planes_gpu::PlanesPass::compiles(device.cast(), ten_bit) }
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = (device, ten_bit);
        Err("this platform has no plane shader pass".into())
    }
}

#[cfg(test)]
mod tests;
