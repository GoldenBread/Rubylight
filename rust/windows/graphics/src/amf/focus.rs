//! Encoder focus on AMF (docs/encoder-focus-design.md): the converter's
//! importance map goes on the input surface as the codec's ROI map, and the
//! startup probe proves from block QP feedback that the driver honours it.
use super::*;
use butterpollo_media_policy::encoder_focus::{self, Reason, Stats, Support};
use std::collections::VecDeque;

/// The probe map must move the unimportant half at least this many QP above
/// the important half ([`Q_INDEX_SCALE`] times as many AV1 q-index steps).
pub const ROI_MIN_DELTA: f32 = 2.0;
/// AV1 feedback is q-index (0..=255), about four steps per QP.
pub const Q_INDEX_SCALE: f32 = 4.0;
/// Probe frames before the measured ones, the first an IDR, so rate control settles.
pub(crate) const PROBE_WARMUP: u32 = 4;
/// Measured probe frames with the left half important, then as many with the right.
pub(crate) const PROBE_FRAMES: u32 = 3;
/// Frames after a keyframe, a recovery picture or the encoder's first frame
/// that carry no map. Any ROI map, even an all-zero one, lifts the
/// latency-constrained rate control's limit on big frames from the frame it
/// is on, so maps while the rate control settles after an IDR made frames of
/// 75 to 145 KB against an 83 KB cap (HEVC 1080p60 at 20 Mbps, RX 7900 XT,
/// 2026-10-27). Maps from the 16th frame on matched focus off; this is twice that.
pub(crate) const SETTLE_FRAMES: u32 = 30;
/// `IID_AMFSurface` (Surface.h), an inline function bindgen does not emit.
pub(crate) const SURFACE_IID: AMFGuid = AMFGuid {
    data1: 0x3075dbe3,
    data2: 0x8718,
    data3: 0x4cfa,
    data41: 0x86,
    data42: 0xfb,
    data43: 0x21,
    data44: 0x14,
    data45: 0xc0,
    data46: 0xa5,
    data47: 0xa4,
    data48: 0x51,
};

pub(crate) fn roi_name(codec: u8) -> &'static str {
    match codec {
        0 => "ROIData",
        1 => "HevcROIData",
        _ => "Av1ROIData",
    }
}
/// The ROI capability; AV1 has none.
pub(crate) fn cap_name(codec: u8) -> Option<&'static str> {
    match codec {
        0 => Some("ROIMap"),
        1 => Some("HevcROIMap"),
        _ => None,
    }
}
pub(crate) fn qp_feedback(codec: u8) -> &'static str {
    match codec {
        0 => "BlockQpFeedback",
        1 => "HevcBlockQpFeedback",
        _ => "Av1BlockQIndexFeedback",
    }
}
pub(crate) fn qp_map(codec: u8) -> &'static str {
    match codec {
        0 => "BlockQpMap",
        1 => "HevcBlockQpMap",
        _ => "Av1BlockQIndexMap",
    }
}
/// The size VCN codes a frame at, in whole 16x16 blocks: 1080 rows are 1088.
pub(crate) fn coded_size((width, height): (u32, u32)) -> (u32, u32) {
    (width.next_multiple_of(16), height.next_multiple_of(16))
}
/// A map whose grid is not the coded frame's would weight the wrong blocks.
pub(crate) fn check_grid(codec: u8, frame: (u32, u32), map: (u32, u32)) -> Result<()> {
    let (width, height) = coded_size(frame);
    let expected = encoder_focus::grid(codec, width, height);
    anyhow::ensure!(
        map == expected,
        "encoder focus map is {}x{} blocks, but the encoder codes {width}x{height} in {}x{} blocks",
        map.0,
        map.1,
        expected.0,
        expected.1
    );
    Ok(())
}

/// One u32 per block, row by row. In a QP map, 0 marks a block the driver
/// returned no QP for.
#[derive(Clone, Debug, PartialEq)]
pub struct Grid {
    pub width: u32,
    pub height: u32,
    pub values: Vec<u32>,
}
/// Undo the layout the RX 7900 XT's driver returns block QP maps in: it packs
/// a map whose rows are `stride` values apart (the surface pitch) as if they
/// were `width` apart, so block (x, y) lands at flat index `y * stride + x`
/// of the `width`-wide grid, and rows past `width * height / stride` are lost.
/// A grid whose nonzero values all sit at such positions is laid out again,
/// with the lost blocks 0; any other grid is the plain layout and kept.
pub(crate) fn unstride(grid: Grid, stride: usize) -> Grid {
    let width = grid.width as usize;
    if stride <= width || grid.height < 2 {
        return grid;
    }
    let strided = grid
        .values
        .iter()
        .enumerate()
        .all(|(index, &value)| value == 0 || index % stride < width);
    if !strided {
        return grid;
    }
    let values = (0..grid.values.len())
        .map(|index| {
            let (x, y) = (index % width, index / width);
            grid.values.get(y * stride + x).copied().unwrap_or(0)
        })
        .collect();
    Grid { values, ..grid }
}
/// Mean of the right half's blocks minus the left half's; an odd middle
/// column and blocks without a QP count for neither.
pub(crate) fn half_delta(grid: &Grid) -> Option<f32> {
    let half = grid.width / 2;
    let (mut left, mut right) = ((0f64, 0u32), (0f64, 0u32));
    for (index, &value) in grid.values.iter().enumerate() {
        let x = index as u32 % grid.width.max(1);
        let side = if value == 0 {
            continue;
        } else if x < half {
            &mut left
        } else if x >= grid.width - half {
            &mut right
        } else {
            continue;
        };
        side.0 += f64::from(value);
        side.1 += 1;
    }
    (left.1 > 0 && right.1 > 0)
        .then(|| (right.0 / f64::from(right.1) - left.0 / f64::from(left.1)) as f32)
}
/// Mean QP of the least important blocks minus the most important, with
/// each QP block placed on the importance grid; `None` for a uniform map.
/// Blocks without a QP count for neither.
pub(crate) fn importance_delta(qp: &Grid, importance: &Grid) -> Option<f32> {
    let low = *importance.values.iter().min()?;
    let high = *importance.values.iter().max()?;
    if low == high || qp.width == 0 || qp.height == 0 {
        return None;
    }
    let (mut lows, mut highs) = ((0f64, 0u32), (0f64, 0u32));
    for (index, &value) in qp.values.iter().enumerate() {
        if value == 0 {
            continue;
        }
        let (x, y) = (index as u32 % qp.width, index as u32 / qp.width);
        let at = (y * importance.height / qp.height) * importance.width
            + x * importance.width / qp.width;
        let side = match importance.values.get(at as usize) {
            Some(&v) if v == low => &mut lows,
            Some(&v) if v == high => &mut highs,
            _ => continue,
        };
        side.0 += f64::from(value);
        side.1 += 1;
    }
    (lows.1 > 0 && highs.1 > 0)
        .then(|| (lows.0 / f64::from(lows.1) - highs.0 / f64::from(highs.1)) as f32)
}

/// What one frame does with the newest finished map, given as `(value, uniform)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Pick {
    /// A keyframe, recovery picture or first frame, or one of the
    /// [`SETTLE_FRAMES`] after them: no map.
    Skip,
    /// No map newer than the last one used.
    Stale,
    /// Every block alike: attaching it would change nothing.
    Uniform,
    Attach,
}
fn pick(
    idr: bool,
    recover: bool,
    first: bool,
    settling: bool,
    map: Option<(u64, bool)>,
    last: u64,
) -> Pick {
    if settling || !encoder_focus::attach_for(idr, recover, first, false) {
        return Pick::Skip;
    }
    match map {
        None => Pick::Stale,
        Some((value, _)) if value <= last => Pick::Stale,
        Some((_, true)) => Pick::Uniform,
        Some(_) => Pick::Attach,
    }
}
fn count(stats: &mut Stats, pick: Pick) {
    match pick {
        Pick::Skip => stats.skipped_recovery += 1,
        Pick::Stale => stats.stale += 1,
        Pick::Uniform => stats.uniform += 1,
        Pick::Attach => stats.maps += 1,
    }
}

/// A frame that asked AMF for its block QP map.
struct Ask {
    pts: i64,
    /// The importances it carried, for `qp_delta`.
    importance: Option<Grid>,
    /// The probe keeps the answer.
    keep: bool,
}
/// The encoder's focus state.
#[derive(Default)]
pub(crate) struct State {
    pub(crate) settings: Option<crate::compute::FocusSettings>,
    /// The value of the last map a frame used.
    pub(crate) last: u64,
    /// Frames since the last keyframe, recovery picture or first frame.
    settled: u32,
    pub(crate) stats: Stats,
    /// Ask for QP feedback on one frame in this many that carry a map.
    pub(crate) verify: Option<u32>,
    pub(crate) since_verify: u32,
    pub(crate) qp_sum: f64,
    pub(crate) qp_count: u32,
    /// The probe's map, attached instead of the converter's.
    pub(crate) fixed: Option<Grid>,
    /// The probe asks every frame for feedback.
    keep: bool,
    asked: VecDeque<Ask>,
    /// The probe's answers in frame order: the QP map, `None` when AMF sent
    /// none, or why it could not be read.
    answers: Vec<std::result::Result<Option<Grid>, String>>,
    /// Why a map failed on a frame, until the stream takes it.
    pub(crate) failure: Option<anyhow::Error>,
}
impl State {
    /// One frame at submission. Counts it towards the settle window, with
    /// focus off too so that turning it on just after a keyframe still
    /// waits. With focus on (`map` is `Some`, holding the newest finished map
    /// as `(value, uniform)` if there is one), picks what the frame does with
    /// that map, counts it and marks the map used.
    pub(crate) fn frame(
        &mut self,
        idr: bool,
        recover: bool,
        first: bool,
        map: Option<Option<(u64, bool)>>,
    ) -> Option<Pick> {
        self.settled = if idr || recover || first {
            0
        } else {
            self.settled.saturating_add(1)
        };
        let map = map?;
        let settling = self.settled <= SETTLE_FRAMES;
        let picked = pick(idr, recover, first, settling, map, self.last);
        count(&mut self.stats, picked);
        if let (Pick::Attach | Pick::Uniform, Some((value, _))) = (picked, map) {
            self.last = value;
        }
        Some(picked)
    }
    /// A keyframe the encoder made without being asked, such as a
    /// pre-analysis scene change: the frames after it wait as after a
    /// requested one.
    pub(super) fn keyframe_out(&mut self) {
        self.settled = 0;
    }
    /// A map failed on a frame: focus is off in this encoder, the frame goes
    /// without one, and the stream takes `error` to turn it off and say so.
    pub(crate) fn fail(&mut self, error: anyhow::Error) {
        self.settings = None;
        self.failure = Some(error);
    }
}

/// SetProperty on an input surface.
///
/// # Safety
/// `surface` is live, and an interface in `value` stays live for the call.
unsafe fn set(surface: *mut AMFSurface, name: &'static str, value: AMFVariantStruct) -> Result<()> {
    // SAFETY: Per the contract; AMF copies the terminated name and value and
    // acquires its own reference to an interface value.
    unsafe {
        check(((*(*surface).pVtbl).SetProperty.unwrap())(
            surface,
            static_wide(name).as_ptr(),
            value,
        ))
        .with_context(|| format!("AMF surface property {name}"))
    }
}
/// Put `map` on `surface` as the codec's ROI map, as AMD's SimpleROI does: a
/// host GRAY32 surface of the grid, filled row by row at its pitch.
///
/// # Safety
/// `context` and `surface` are this encoder's, live, and not yet submitted.
pub(crate) unsafe fn attach(
    context: *mut AMFContext,
    surface: *mut AMFSurface,
    codec: u8,
    rows: &[u32],
    width: u32,
    height: u32,
) -> Result<()> {
    anyhow::ensure!(
        width > 0 && rows.len() == width as usize * height as usize,
        "encoder focus map of {} values is not {width}x{height}",
        rows.len()
    );
    // SAFETY: The context allocates an owned host surface; plane 0's native
    // memory holds `height` rows of `pitch` bytes, checked to fit a row, until
    // our reference is released after SetProperty took its own.
    unsafe {
        let mut map = ptr::null_mut();
        check(((*(*context).pVtbl).AllocSurface.unwrap())(
            context,
            AMF_MEMORY_TYPE_AMF_MEMORY_HOST,
            AMF_SURFACE_FORMAT_AMF_SURFACE_GRAY32,
            width as i32,
            height as i32,
            &mut map,
        ))
        .context("AMF ROI map surface")?;
        anyhow::ensure!(!map.is_null(), "AMF returned no ROI map surface");
        let filled = (|| -> Result<()> {
            let plane = ((*(*map).pVtbl).GetPlaneAt.unwrap())(map, 0);
            anyhow::ensure!(!plane.is_null(), "AMF ROI map surface has no plane");
            let v = &*(*plane).pVtbl;
            let native = (v.GetNative.unwrap())(plane).cast::<u8>();
            let pitch = (v.GetHPitch.unwrap())(plane) as usize;
            anyhow::ensure!(
                !native.is_null() && pitch >= width as usize * 4,
                "AMF ROI map surface is not writable host memory"
            );
            for (y, row) in rows.chunks_exact(width as usize).enumerate() {
                ptr::copy_nonoverlapping(
                    row.as_ptr().cast::<u8>(),
                    native.add(y * pitch),
                    row.len() * 4,
                );
            }
            set(
                surface,
                roi_name(codec),
                AMFVariantStruct {
                    type_: AMF_VARIANT_TYPE_AMF_VARIANT_INTERFACE,
                    __bindgen_anon_1: AMFVariantStruct__bindgen_ty_1 {
                        pInterface: map.cast(),
                    },
                },
            )
        })();
        ((*(*map).pVtbl).Release.unwrap())(map);
        filled
    }
}
/// The GRAY32 map named `name` on an output buffer: `None` when AMF set none.
///
/// # Safety
/// `buffer` is a live output buffer.
unsafe fn read_grid(buffer: *mut AMFBuffer, name: &'static str) -> Result<Option<Grid>> {
    // SAFETY: GetProperty hands over one reference to an interface value, which
    // is released once; QueryInterface adds the surface's own, released after
    // its host plane, converted first if needed, has been copied out.
    unsafe {
        let mut value: AMFVariantStruct = std::mem::zeroed();
        let found = ((*(*buffer).pVtbl).GetProperty.unwrap())(
            buffer,
            static_wide(name).as_ptr(),
            &mut value,
        );
        if found != AMF_RESULT_AMF_OK {
            return Ok(None);
        }
        anyhow::ensure!(
            value.type_ == AMF_VARIANT_TYPE_AMF_VARIANT_INTERFACE,
            "{name} is variant type {}, not an interface",
            value.type_
        );
        let interface = value.__bindgen_anon_1.pInterface;
        if interface.is_null() {
            return Ok(None);
        }
        let mut queried = ptr::null_mut();
        let result =
            ((*(*interface).pVtbl).QueryInterface.unwrap())(interface, &SURFACE_IID, &mut queried);
        ((*(*interface).pVtbl).Release.unwrap())(interface);
        check(result).with_context(|| format!("{name} is not a surface"))?;
        let surface = queried.cast::<AMFSurface>();
        anyhow::ensure!(!surface.is_null(), "{name} returned no surface");
        let read = (|| -> Result<Grid> {
            let v = &*(*surface).pVtbl;
            if (v.GetMemoryType.unwrap())(surface) != AMF_MEMORY_TYPE_AMF_MEMORY_HOST {
                check((v.Convert.unwrap())(
                    surface,
                    AMF_MEMORY_TYPE_AMF_MEMORY_HOST,
                ))
                .with_context(|| format!("{name} to host memory"))?;
            }
            let plane = (v.GetPlaneAt.unwrap())(surface, 0);
            anyhow::ensure!(!plane.is_null(), "{name} has no plane");
            let p = &*(*plane).pVtbl;
            let (width, height) = ((p.GetWidth.unwrap())(plane), (p.GetHeight.unwrap())(plane));
            let pitch = (p.GetHPitch.unwrap())(plane) as usize;
            let native = (p.GetNative.unwrap())(plane).cast::<u8>().cast_const();
            anyhow::ensure!(
                width > 0
                    && height > 0
                    && width <= 4096
                    && height <= 4096
                    && !native.is_null()
                    && pitch >= width as usize * 4,
                "{name} is an unreadable {width}x{height} plane"
            );
            let mut values = Vec::with_capacity(width as usize * height as usize);
            for y in 0..height as usize {
                let row = native.add(y * pitch).cast::<u32>();
                values.extend((0..width as usize).map(|x| row.add(x).read_unaligned()));
            }
            Ok(unstride(
                Grid {
                    width: width as u32,
                    height: height as u32,
                    values,
                },
                pitch / 4,
            ))
        })();
        ((*(*surface).pVtbl).Release.unwrap())(surface);
        read.map(Some)
    }
}

/// The startup check's result for one codec and bit depth.
#[derive(Clone, Debug, PartialEq)]
pub struct Probe {
    pub support: Support,
    /// The map's effect: half of the right-minus-left mean QP (AV1 q-index)
    /// with the left half important minus the same with the right half
    /// important, so a difference the picture itself makes cancels.
    pub qp_delta: Option<f32>,
    /// The mean of the two: the right-minus-left difference the picture and
    /// encoder make whatever the map.
    pub control_delta: Option<f32>,
    /// The feedback map's size in blocks.
    pub feedback_grid: Option<(u32, u32)>,
}
impl Probe {
    pub(crate) fn unsupported(reason: Reason, message: impl Into<String>) -> Self {
        Self {
            support: Support::Unsupported {
                reason,
                message: message.into(),
            },
            qp_delta: None,
            control_delta: None,
            feedback_grid: None,
        }
    }
}
/// The map's effect and the picture's own difference from the mean
/// right-minus-left differences with the left half important and with the
/// right half important.
pub(crate) fn effect(
    left_important: Option<f32>,
    right_important: Option<f32>,
) -> (Option<f32>, Option<f32>) {
    match (left_important, right_important) {
        (Some(left), Some(right)) => (Some((left - right) / 2.0), Some((left + right) / 2.0)),
        _ => (None, None),
    }
}
/// The probe's verdict from the map's measured effect.
pub(crate) fn verdict(codec: u8, effect: Option<f32>) -> Support {
    let (scale, unit) = if codec == 2 {
        (Q_INDEX_SCALE, "q-index")
    } else {
        (1.0, "QP")
    };
    let unsupported = |message: String| Support::Unsupported {
        reason: Reason::CodecUnsupported,
        message,
    };
    let Some(effect) = effect else {
        return unsupported(
            "the driver returned no block QP map, so ROI cannot be verified".into(),
        );
    };
    if effect >= ROI_MIN_DELTA * scale {
        Support::Verified { delta: effect }
    } else {
        unsupported(format!(
            "the ROI map moved {unit} by {effect:+.1}, under the {:+.1} required",
            ROI_MIN_DELTA * scale
        ))
    }
}
fn mix(mut value: u32) -> u32 {
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb_352d);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846c_a68b);
    value ^ (value >> 16)
}
/// A probe frame: 8x8 cells of hashed grey scrolling 3 pixels a frame, with
/// fresh grain, so both halves code the same kind of residual every frame.
pub(crate) fn probe_picture(
    device: &Device,
    width: u32,
    height: u32,
    frame: u32,
) -> Result<GpuImage> {
    let mut bytes = vec![0u8; width as usize * height as usize * 4];
    for (y, row) in bytes.chunks_exact_mut(width as usize * 4).enumerate() {
        let y = y as u32;
        for (x, pixel) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let x = x as u32;
            let cell = mix(((x + 3 * frame) / 8) | ((y / 8) << 16)) % 160;
            let grain = mix(x ^ (y << 13) ^ (frame << 26) ^ 0x5bd1_e995) % 24;
            let level = (48 + cell + grain) as u8;
            *pixel = [level, level, level, 255];
        }
    }
    let image = GpuImage::upload(
        device,
        &Image {
            width,
            height,
            stride: width as usize * 4,
            bytes,
            captured: Instant::now(),
            pixel: crate::capture::Pixel::Bgra8,
        },
    )?;
    // Finish the D3D11 upload before a D3D12 conversion reads it.
    image.readback(&mut None)?;
    Ok(image)
}

mod encoder;
#[cfg(test)]
pub(crate) use encoder::map_pitch;
#[cfg(test)]
mod tests;
