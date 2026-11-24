//! Early decode, beam race, and decoder capability (`RFI` / slice count)
//! decisions for desktop streams.

use super::*;

/// What early decode does on a stream. `NoRecords` is the failure the earlier measurements
/// hit unseen: a host whose version did not parse never offered records, so no chunk ever came.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::stream) enum EarlyState {
    Off,
    On,
    NoRecords,
}

pub(in crate::stream) const EARLY_NO_RECORDS: &str = "nightjar: early decode is on but the host sent no PyroWave records \
    (its version did not offer them, or the client did not ask), so frames are decoded whole";

pub(in crate::stream) fn early_state(requested: bool, pyrowave: bool, records: bool) -> EarlyState {
    match (requested && pyrowave, records) {
        (false, _) => EarlyState::Off,
        (true, true) => EarlyState::On,
        (true, false) => EarlyState::NoRecords,
    }
}

/// What the beam race setting does on a stream: the client asks the host for the row order and
/// decodes the picture in slices as its bands complete.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::stream) enum BeamRace {
    Off,
    On,
    NotWired,
    NoGpuDecoder,
}

impl BeamRace {
    /// Why the setting did nothing, for the overlay.
    pub(in crate::stream) fn reason(self) -> Option<&'static str> {
        match self {
            Self::NotWired => Some("off, the link is not a known wired one"),
            Self::NoGpuDecoder => Some("off, PyroWave is decoded on the CPU"),
            Self::Off | Self::On => None,
        }
    }
}

/// Everything the beam race setting asks of a stream, in one place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::stream) struct RaceRequest {
    pub(in crate::stream) row_order: bool,
    pub(in crate::stream) early_decode: bool,
    pub(in crate::stream) slices: u32,
}

pub(in crate::stream) fn race_request(
    beam: BeamRace,
    early_setting: bool,
    height: u32,
) -> RaceRequest {
    let on = beam == BeamRace::On;
    RaceRequest {
        row_order: on,
        early_decode: early_setting || on,
        slices: if on { beam_slices(height) } else { 0 },
    }
}

/// Whether the race runs on the stream.
pub(in crate::stream) fn race_runs(slices: u32, early: EarlyState) -> bool {
    slices > 0 && early == EarlyState::On
}

pub(in crate::stream) const BEAM_RACE_NO_GPU: &str = "nightjar: beam race is on but PyroWave is decoded on the CPU, so the host is not asked \
    for the row order and frames are decoded whole";

pub(in crate::stream) const BEAM_RACE_NOT_WIRED: &str = "nightjar: beam race is on but the link is not a known wired one, so the host is not asked \
    for the row order and frames are decoded whole";

/// An on or off switch in an environment variable: `1`, `true`, `yes` or `on`, and `0`, `false`,
/// `no` or `off` (any case). Anything else is not a switch.
pub(in crate::stream) fn flag_value(text: &str) -> Option<bool> {
    match text.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// The switch `name` names, if it is set. A value that is not a switch is said so on stderr and
/// ignored, never taken as on or off.
pub fn env_flag(name: &str) -> Option<bool> {
    let text = std::env::var(name).ok()?;
    let value = flag_value(&text);
    if value.is_none() {
        eprintln!(
            "nightjar: {name}={text:?} is not 1, true, yes, on, 0, false, no or off, so it is ignored"
        );
    }
    value
}

/// Whether a backend recovers from a lost frame with reference frame invalidation.
pub(in crate::stream) fn takes_rfi(backend: Hardware) -> bool {
    static VULKAN_RFI: std::sync::OnceLock<Option<bool>> = std::sync::OnceLock::new();
    static VIDEOTOOLBOX_RFI: std::sync::OnceLock<Option<bool>> = std::sync::OnceLock::new();
    takes_rfi_when(
        backend,
        VULKAN_RFI
            .get_or_init(|| env_flag("NIGHTJAR_VULKAN_RFI"))
            .unwrap_or_else(crate::media::vulkan_rfi_setting),
        VIDEOTOOLBOX_RFI
            .get_or_init(|| env_flag("NIGHTJAR_VIDEOTOOLBOX_RFI"))
            .unwrap_or_else(crate::media::videotoolbox_rfi_setting),
    )
}

/// `takes_rfi`, with Vulkan video taking it when `vulkan` says so and VideoToolbox when
/// `videotoolbox` does.
pub(in crate::stream) fn takes_rfi_when(
    backend: Hardware,
    vulkan: bool,
    videotoolbox: bool,
) -> bool {
    matches!(
        backend,
        Hardware::D3d11(_) | Hardware::Vaapi | Hardware::None
    ) || vulkan && matches!(backend, Hardware::Vulkan(_))
        || videotoolbox && is_videotoolbox(backend)
}

/// VideoToolbox, which exists on macOS only.
fn is_videotoolbox(backend: Hardware) -> bool {
    #[cfg(target_os = "macos")]
    return matches!(backend, Hardware::VideoToolbox);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = backend;
        false
    }
}

/// The decoder's `CAPABILITY_*` for the launch.
pub(in crate::stream) fn video_capabilities(
    base: &color_probe::Base,
    software: bool,
    rfi: bool,
    slices: bool,
    cores: usize,
) -> u32 {
    let family = |mask: u32| {
        let mut backends = base
            .backends
            .iter()
            .filter(|&&(format, _)| format & mask != 0)
            .peekable();
        backends.peek().is_some() && backends.all(|&(_, backend)| takes_rfi(backend))
    };
    let mut capabilities = 0;
    if rfi && family(VIDEO_FORMAT_MASK_H265) {
        capabilities |= CAPABILITY_REFERENCE_FRAME_INVALIDATION_HEVC;
    }
    if rfi && family(VIDEO_FORMAT_MASK_AV1) {
        capabilities |= CAPABILITY_REFERENCE_FRAME_INVALIDATION_AV1;
    }
    if software && slices {
        capabilities |= capability_slices_per_frame(cores.clamp(1, 4) as u8);
    }
    capabilities
}

/// The setting, unless `NIGHTJAR_BEAM_RACE` says on or off.
pub(in crate::stream) fn beam_race(setting: bool, link_mbps: u32, gpu_decoder: bool) -> BeamRace {
    beam_race_for(
        env_flag("NIGHTJAR_BEAM_RACE"),
        setting,
        link_mbps,
        gpu_decoder,
    )
}

pub(in crate::stream) fn beam_race_for(
    forced: Option<bool>,
    setting: bool,
    link_mbps: u32,
    gpu_decoder: bool,
) -> BeamRace {
    match forced {
        Some(false) => BeamRace::Off,
        Some(true) if gpu_decoder => BeamRace::On,
        Some(true) => BeamRace::NoGpuDecoder,
        _ if !setting => BeamRace::Off,
        _ if !gpu_decoder => BeamRace::NoGpuDecoder,
        _ if link_mbps == 0 => BeamRace::NotWired,
        _ => BeamRace::On,
    }
}

/// Most slices per frame: a frame has a present for each batch of slices and one for the picture,
/// and the timing CSV has room for `PRESENTS` of them.
pub(in crate::stream) const MAX_RACE_SLICES: u32 = crate::timing_csv::PRESENTS as u32 - 1;

/// Slices per frame: 4, or 3 at 4K where each slice decode costs more.
pub(in crate::stream) fn beam_slices(height: u32) -> u32 {
    let default = if height >= 2000 { 3 } else { 4 };
    match std::env::var("NIGHTJAR_BEAM_RACE_SLICES") {
        Ok(text) => slices_override(&text).unwrap_or_else(|| {
            eprintln!("nightjar: NIGHTJAR_BEAM_RACE_SLICES={text:?} is not a number from 1 to {MAX_RACE_SLICES}, so {default} slices are used");
            default
        }),
        Err(_) => default,
    }
}

pub(in crate::stream) fn slices_override(text: &str) -> Option<u32> {
    text.trim()
        .parse()
        .ok()
        .filter(|n| (1..=MAX_RACE_SLICES).contains(n))
}

/// The setting, unless `NIGHTJAR_EARLY_DECODE` is `1` or `0` (for measurement runs).
pub(in crate::stream) fn early_decode(setting: bool) -> bool {
    match std::env::var("NIGHTJAR_EARLY_DECODE").as_deref() {
        Ok("1") => true,
        Ok("0") => false,
        _ => setting,
    }
}
