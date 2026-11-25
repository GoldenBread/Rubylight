//! Native NVIDIA NVENC encoder, called through NVIDIA's driver ABI.
//!
//! Supports NVENC API 11 to 13 and reference frame invalidation. GPU
//! resources stay owned until their output completes; 10-bit 4:4:4 input goes
//! through CUDA interop (`cuda`). The frame budget guard, VBR rate control,
//! fixed split-encode strips, the codec check and spatial AQ by default follow
//! Foundation Sunshine (AlkaidLab/foundation-sunshine, GPL-3.0, src/nvenc/).
//! HDR10+, HDR Vivid and Dolby Vision come from the conversion's statistics
//! pass and are spliced in as with AMF (`amf::dynamic`). With the D3D12
//! compute conversion the session runs on its `ID3D12Device` and waits for
//! each conversion's fence on the GPU (API 12.0, driver 522.25 and newer).

mod cache;
mod config;
mod cuda;
mod drain;
mod encoder;
mod focus;
mod output;
mod params;
mod reconfigure;
mod runtime;
mod session;
mod slots;
mod submit;
mod teardown;

use cache::{CachedConversion, OutputEvent};
use encoder::refuse_native_444;
#[cfg(test)]
use encoder::{coded, readback_size};
#[cfg(test)]
use focus::probe_verdict;
#[cfg(test)]
use output::slices_consistent;
use params::*;
pub(crate) use runtime::system_library;
use runtime::{Fence, Runtime, check_raw, directx_device, fence_point, retry_version};
pub use runtime::{SessionLimit, session_limit};
use session::SessionDrain;
use teardown::cleanup_slot;

use crate::capture::{Device, GpuImage, Image, Pixel};
use crate::encoder::Encoded;
use anyhow::{Context, Result, bail};
use butterpollo_config::config::Config;
use butterpollo_media_policy::{
    WarningSink as Warnings,
    encoder_focus::{self, Reason},
    nvenc::{ApiVersion, Budget, RateControl, Recovery, References, SPLIT_DISABLED, Tuning},
    recorded_warnings,
    rtsp::Negotiated,
};
use butterpollo_win_abi::nvenc::*;
use std::{
    collections::VecDeque,
    ffi::c_void,
    ptr,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};
use windows::{
    Win32::Graphics::{
        Direct3D11::ID3D11Texture2D,
        Direct3D12::{ID3D12Fence, ID3D12Resource},
    },
    core::Interface,
};

const SUCCESS: NVENCSTATUS = _NVENCSTATUS_NV_ENC_SUCCESS;
const MAX_PENDING: usize = 8;
/// Slots whose cleanup the driver refused that a session keeps waiting for a
/// retry; one more registration fails instead of adding to them.
const MAX_DEFERRED: usize = MAX_PENDING;
/// A frame still unfinished this long after its submission means a hung
/// driver. A shorter wait belongs to the session's stall policy
/// (`encoder_stall_limit`: 250 ms, doubling up to 2 s), which waits out a
/// game holding the GPU instead of recreating the encoder. At 100 ms, game
/// loading on an RTX 5070 recreated it twice in 12 s: the picture froze
/// each time and the rest of the session converted on the graphics queue.
const COMPLETION_TIMEOUT: Duration = Duration::from_secs(3);
/// How long a synchronous drain (bitrate reset, reference invalidation,
/// teardown) blocks the stream thread.
const DRAIN_TIMEOUT: Duration = Duration::from_millis(100);
const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
/// The smallest VBV a bitrate change or a frame cap sets.
const MIN_VBV_BITS: u64 = 100_000;
/// The least time between warnings about a frame over twice its cap.
const OVERSIZE_WARNING_INTERVAL: Duration = Duration::from_secs(10);
/// Subframe frames whose locks are traced at debug level.
const TRACED_SUBFRAMES: u8 = 4;
/// Extra output buffer capacity reserved when dynamic HDR metadata will be
/// spliced in so `Dynamic::first_unit`/`last_unit` (including T.35 + Dolby
/// Vision 8.4 RPU) avoid reallocating.
const DYNAMIC_HEADROOM: usize = 768;

/// Whether NVENC can convert and encode `config` on the D3D12 compute path
/// (4:2:0, packed 4:4:4, and 8-bit native 4:4:4 AYUV; 10-bit 4:4:4 stays on
/// D3D11 + CUDA).
pub(crate) fn supports_compute(config: &Negotiated) -> bool {
    butterpollo_media_policy::nvenc::supports_d3d12_compute(config.yuv444, config.ten_bit())
}
/// Whether a failed D3D12 NVENC creation retries on the D3D11 graphics queue:
/// yes whenever compute was attempted and the failure is not the session limit.
pub(crate) fn falls_back_to_d3d11(attempted_compute: bool, error: &anyhow::Error) -> bool {
    attempted_compute && !session_limit(error)
}

struct Input {
    key: usize,
    raw: *mut c_void,
    kind: NV_ENC_INPUT_RESOURCE_TYPE,
    pitch: u32,
    _texture: Option<ID3D11Texture2D>,
    cuda: Option<cuda::Input>,
    /// A D3D12 session's input resource and its READBACK output buffer.
    d3d12: Option<D3d12Input>,
}
/// What a D3D12 slot registers: the conversion's texture as its input, the
/// conversion fence value the driver waits for if registration touches the
/// resource on the GPU, and a READBACK buffer the driver writes the bitstream
/// into.
struct D3d12Input {
    _resource: Option<ID3D12Resource>,
    wait: Option<(*mut c_void, u64)>,
    output: *mut c_void,
    size: u32,
    _output: Option<ID3D12Resource>,
}

/// A D3D12 session's output fence, the next value to assign, and the highest
/// value accepted by `nvEncEncodePicture`.
struct D3d12 {
    fence: Rc<dyn Fence>,
    value: u64,
    submitted: u64,
}
/// A D3D12 slot's output registration and mapping, and the resource
/// structures each picture passes in place of buffers. Boxed, so the
/// driver's pointers to them stay put.
#[derive(Default)]
struct D3d12Slot {
    registered: NV_ENC_REGISTERED_PTR,
    mapped: NV_ENC_INPUT_PTR,
    input: NV_ENC_INPUT_RESOURCE_D3D12,
    output: NV_ENC_OUTPUT_RESOURCE_D3D12,
}
/// What a pending frame keeps of its converted input until it is collected,
/// so the texture is not converted into again and a scene cut can be
/// encoded once more.
#[derive(Clone)]
enum Kept {
    D3d11 {
        _texture: Arc<ID3D11Texture2D>,
    },
    /// The D3D12 conversion's texture and the fence point the driver waits
    /// for before it reads it.
    D3d12 {
        _texture: Option<Arc<ID3D12Resource>>,
        _fence: Option<ID3D12Fence>,
        fence: *mut c_void,
        value: u64,
    },
}
struct Slot {
    input: Input,
    metadata: Metadata,
    registered: NV_ENC_REGISTERED_PTR,
    mapped: NV_ENC_INPUT_PTR,
    /// The driver's bitstream buffer, or a D3D12 slot's boxed
    /// `NV_ENC_OUTPUT_RESOURCE_D3D12`, which the lock takes in its place.
    output: NV_ENC_OUTPUT_PTR,
    d3d12: Option<Box<D3d12Slot>>,
    /// The QP delta map of the frame in flight, which the driver reads.
    deltas: Vec<i8>,
    event: Option<crate::timing::Signal>,
    event_registered: bool,
    signaled: bool,
}
impl Slot {
    /// Its registration failed and the driver kept part of it: no input finds
    /// it, it waits for a retry or the session's destruction.
    fn deferred(&self) -> bool {
        self.input.key == 0 && !self.released()
    }
    /// A deferred slot that holds nothing any more (`release`): the list
    /// reuses or trims it.
    fn released(&self) -> bool {
        self.input.key == 0
            && self.event.is_none()
            && self.registered.is_null()
            && self.mapped.is_null()
            && self.output.is_null()
            && !self.event_registered
            && self
                .d3d12
                .as_ref()
                .is_none_or(|slot| slot.registered.is_null() && slot.mapped.is_null())
    }
    /// After `cleanup_slot` succeeded: close the completion event, which the
    /// driver no longer has registered, and let go of the input, and leave
    /// the slot without a key so no input finds it.
    fn release(&mut self) {
        self.event = None;
        self.input.key = 0;
        self.input._texture = None;
        self.input.cuda = None;
        self.input.d3d12 = None;
    }
}
struct Pending {
    slot: usize,
    frame: u64,
    started: Instant,
    /// When this submission reached the driver: its completion deadline
    /// counts from here, even for a frame encoded again, whose `started`
    /// stays the first submission's.
    submitted: Instant,
    presentation: Instant,
    after_invalidation: bool,
    /// Submitted as a keyframe.
    idr: bool,
    /// The keyframe encoded again in place of an oversize scene cut.
    reencoded: bool,
    /// [`Session::frame_cap`], [`Session::frame_cap_target`] and
    /// [`Session::cut_limit`] at submission.
    frame_cap_bytes: Option<u64>,
    frame_cap_target: Option<u64>,
    cut_limit: Option<u64>,
    /// With slice delivery, a P-frame that may leave slice by slice: not a
    /// keyframe or a recovery frame, and under a frame cap.
    streams: bool,
    /// The units of a frame that has begun to leave slice by slice.
    units: Option<Units>,
    /// The D3D12 output fence value the frame signals once it is written.
    signal: Option<u64>,
    /// The lock reads the frame's block QP statistics.
    measured: bool,
    _converted: Option<Kept>,
}
/// How far a frame sent slice by slice has left.
#[derive(Clone, Copy, Debug, Default)]
struct Units {
    /// Bytes of the picture already handed on.
    sent: usize,
    /// The index of the next unit.
    next: u32,
}
/// A frame submitted once more ([`Session::reencode`]): it keeps its number
/// and its encode time counts from the first submission, as AMF counts it.
#[derive(Clone, Copy)]
struct Again {
    frame: u64,
    started: Instant,
}
struct Session {
    raw: *mut c_void,
    functions: NV_ENCODE_API_FUNCTION_LIST,
    api: ApiVersion,
    /// The driver APIs this session tried and the driver refused, when it
    /// opened an older one; reported to the stream as a session warning.
    api_fallbacks: Vec<String>,
    /// `qpMapMode` is the delta map, for encoder focus.
    qp_map: bool,
    /// The driver returns per-block statistics (`enableOutputStats`).
    block_stats: bool,
    /// The QP delta map for the next submission, left off a keyframe.
    next_map: Option<Vec<i8>>,
    /// The buffer the next delta map is filled into, taken back from the
    /// slot whose last frame carried one.
    map_scratch: Vec<i8>,
    /// The picture parameters every submission resets into and reuses,
    /// zeroed once at open: the reserved areas NV_ENC_PIC_PARAMS requires
    /// to stay zero are never written at all.
    picture: NV_ENC_PIC_PARAMS,
    /// The next submission's block QP statistics are read.
    measure_next: bool,
    stats_buffer: Vec<NV_ENC_OUTPUT_STATS_BLOCK>,
    /// Block QP grids of measured frames, by frame number.
    qp_grids: VecDeque<(u64, crate::amf::focus::Grid)>,
    config: Box<NV_ENC_CONFIG>,
    initialize: Box<NV_ENC_INITIALIZE_PARAMS>,
    stream: Negotiated,
    metadata: Metadata,
    submitted_metadata: Option<EncodedMetadata>,
    slots: Vec<Slot>,
    pending: VecDeque<Pending>,
    ready: VecDeque<Encoded>,
    references: References,
    retained: u32,
    invalidation: bool,
    dynamic_bitrate: bool,
    /// The rate control the driver accepted, for bitrate changes.
    rate_control: RateControl,
    /// The frame budget guard's verdict on the configured preset.
    budget: Option<Budget>,
    /// The one-frame VBV, in bits, that the settings give at the current
    /// bitrate; [`Session::set_frame_cap`] lowers the session's VBV under it
    /// and never raises it above. 0 when the driver controls buffering.
    vbv_ceiling: u32,
    /// Configured `nvenc_vbv_increase` percentage used to recompute `vbv_ceiling`
    /// across bitrate changes.
    vbv_increase: u32,
    /// The last frame cap the VBV was set for, against small changes.
    cap_requested: Option<u64>,
    /// The driver refused the VBV of `cap_requested`, which is not asked for again: frames
    /// are not held to it.
    cap_refused: bool,
    /// The host's last pace-based frame cap ([`Session::set_frame_cap`]).
    frame_cap_target: Option<u64>,
    /// The largest inter frame sent as encoded ([`Session::reencodes`]).
    cut_limit: Option<u64>,
    reencoded_cuts: u64,
    keyframes_over_cap: u64,
    rfi_fallbacks: u64,
    /// When a frame over twice its cap was last reported.
    oversize_warned: Option<Instant>,
    /// Why encoding a scene cut again failed, until the encoder reports it.
    reencode_failure: Option<String>,
    /// The output fence value the encoder's output event was last armed
    /// for ([`Session::arm_output`]); 0 before any.
    armed: std::cell::Cell<u64>,
    /// The output fence value `drain_event` was last armed for; 0 before any.
    drain_armed: u64,
    /// Blocks a P-frame is sent in when slice delivery is on, else 0.
    slice_blocks: u8,
    /// NVENC writes each slice as it finishes (`enableSubFrameWrite`), and
    /// frames are read while they encode: slice delivery is on.
    subframe: bool,
    /// The slices of each frame in subframe mode.
    subframe_slices: u32,
    /// Subframe frames completed, for the first frames' lock trace.
    subframe_traced: u8,
    /// A subframe lock's slice offsets were inconsistent once.
    offsets_warned: bool,
    /// Slice delivery is off for the rest of the session
    /// ([`Encoder::join_slices`]).
    slices_joined: bool,
    /// `sliceOffsets` for a subframe lock, one entry per macroblock as the
    /// API asks.
    slice_offsets: Vec<u32>,
    /// A frame sent slice by slice came out a keyframe, and the log says so.
    unit_idr_reported: bool,
    next: u64,
    last_idr: u64,
    dynamic_headroom: usize,
    initialized: bool,
    force_idr: bool,
    /// A session on the D3D12 conversion's device ([`Session::open`]).
    /// Declared after `slots` and `pending` and before `drain_event` so
    /// registered resources release before the fence and the fence releases
    /// before the event it may signal.
    d3d12: Option<D3d12>,
    /// The event a D3D12 drain waits on for the oldest frame
    /// ([`Session::wait_oldest`]) or teardown completion, created on first use.
    drain_event: Option<crate::timing::Signal>,
    cuda: Option<Rc<cuda::Context>>,
    /// Declared last so `nvEncodeAPI64.dll` stays loaded until every session
    /// resource has dropped.
    _runtime: Arc<Runtime>,
}

pub struct Encoder {
    session: Session,
    /// The D3D12 compute conversion the session encodes from. Declared after
    /// the session, so its targets and fences outlive the frames in flight.
    compute: Option<Box<crate::compute::Converter>>,
    gpu: Device,
    /// The stream as negotiated; the session codes [`coded`].
    config: Negotiated,
    converter: Option<crate::gpu_color::Converter>,
    /// The D3D12 session failed and the caller converted on the graphics
    /// queue; later rebuilds keep that choice.
    compute_fallback: bool,
    source: Option<(u32, u32, Pixel)>,
    luminance: [f32; 2],
    /// The couch menu's panel, drawn into GPU frames by the conversion.
    pub overlay: Option<crate::overlay::Overlay>,
    /// The session's reporter, shared with the stream.
    pub(crate) warnings: Arc<dyn Warnings>,
    dynamic: Option<Box<crate::amf::dynamic::Dynamic>>,
    /// The D3D11 conversion's statistics pass behind HDR10+, HDR Vivid and
    /// measured Dolby Vision light levels.
    stats: Option<encoder::HdrStatsBox>,
    last_submitted: Option<Instant>,
    /// The frame cap failed and its warning is up.
    cap_warned: bool,
    tuning: Tuning,
    /// Encoder focus, with AMF's rules ([`crate::amf::focus`]).
    focus: crate::amf::focus::State,
    /// Frames whose block QP statistics were asked for.
    asked: VecDeque<Ask>,
    /// The focus probe's QP grids in frame order.
    answers: Vec<crate::amf::focus::Grid>,
    /// The focus probe is running: every frame is measured.
    probing: bool,
    /// Next frame repeats the previous picture (`Encoder::set_repeat`).
    repeat: bool,
    /// Last converted surface retained for static repeat-frame reuse.
    last_converted: Option<CachedConversion>,
    /// `nvenc_output_event`: D3D12 output fence event or D3D11/CUDA async slot
    /// completion event. Declared last: it outlives the fence that may set it.
    output_event: OutputEvent,
}
/// A frame whose block QP statistics were asked for.
struct Ask {
    frame: u64,
    /// The importances it carried, for `qp_delta`.
    importance: Option<crate::amf::focus::Grid>,
    /// The probe keeps the answer.
    keep: bool,
}

#[cfg(test)]
mod tests;
