//! Headphone surround: renders Moonlight's 8 channels (FL, FR, FC, LFE, BL,
//! BR, SL, SR) to binaural stereo with one HRIR pair per speaker. Direct-form
//! FIR, so the first output sample of an impulse is its first tap: no samples
//! of delay beyond the filters' own time of arrival.
use super::hrir::{self, TAPS};
use anyhow::Result;

/// Moonlight's channel slots and their Windows speaker bits.
pub const SLOTS: [u32; 8] = [1, 2, 4, 8, 16, 32, 512, 1024];
const LFE: usize = 3;
/// LFE goes to both ears at -6 dB, unfiltered.
const LFE_GAIN: f32 = 0.5;
const SURROUND: u32 = 16 | 32 | 512 | 1024;
/// Frames rendered per register block.
const BLOCK: usize = 16;
/// Scaled taps below this are 0: the HRIRs' fade-out leaves ~1e-37 in the
/// last tap, and that times a normal sample is subnormal (slow on CPUs
/// without flush-to-zero). 1e-20 is ~-400 dB, far below any audible tap.
const TAP_FLOOR: f32 = 1e-20;

/// How a source endpoint lands in the 8 capture channels.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    /// Which slots the source fills.
    pub present: [bool; 8],
    /// Today's stereo downmix of the source, as weights of the 8 slots:
    /// applied to the 8-channel capture it gives what 2-channel capture gives.
    pub downmix: [[f32; 8]; 2],
    /// Squared Euclidean norm of each 8-channel capture row (`||row||^2`),
    /// weighting uncorrelated source energy per slot when height channels are
    /// folded and attenuated into the 8-channel capture matrix.
    slot_energy: [f32; 8],
}

impl Layout {
    pub fn new(source_channels: usize, mask: u32) -> Result<Self> {
        let stereo = crate::audio::mix_matrix(source_channels, mask, 2)?;
        let eight = crate::audio::mix_matrix(source_channels, mask, 8)?;
        let mut layout = Self {
            present: [false; 8],
            downmix: [[0.; 8]; 2],
            slot_energy: [0.; 8],
        };
        for (slot, row) in eight.iter().enumerate() {
            let norm2: f32 = row.iter().map(|v| v * v).sum();
            layout.present[slot] = norm2 > 0.;
            layout.slot_energy[slot] = norm2;
            if norm2 > 0. {
                for (downmix, ear) in layout.downmix.iter_mut().zip(&stereo) {
                    downmix[slot] = row.iter().zip(ear).map(|(a, b)| a * b).sum::<f32>() / norm2;
                }
            }
        }
        Ok(layout)
    }

    /// Whether there is anything to place around the listener: an endpoint
    /// without surround speakers keeps today's downmix.
    pub fn surround(&self) -> bool {
        SLOTS
            .iter()
            .zip(self.present)
            .any(|(bit, present)| present && bit & SURROUND != 0)
    }

    /// The direction of each slot's filters, `None` for LFE and absent ones.
    /// 7.1 puts the sides at 90 and the backs at 135 degrees; a 5.1 source's
    /// surround pair, whichever slots it uses, sits at 110.
    pub fn directions(&self) -> [Option<i16>; 8] {
        let both = self.present[4..8].iter().all(|p| *p);
        let azimuths = [
            30,
            -30,
            0,
            0,
            if both { 135 } else { 110 },
            if both { -135 } else { -110 },
            if both { 90 } else { 110 },
            if both { -90 } else { -110 },
        ];
        std::array::from_fn(|slot| (self.present[slot] && slot != LFE).then_some(azimuths[slot]))
    }

    /// The gain that makes uncorrelated noise in every present slot as loud
    /// through the HRIRs as through the downmix.
    pub fn level(&self) -> f32 {
        let downmix: f32 = (0..8)
            .filter(|slot| self.present[*slot])
            .map(|slot| {
                (self.downmix[0][slot].powi(2) + self.downmix[1][slot].powi(2))
                    * self.slot_energy[slot]
            })
            .sum();
        let mut binaural: f32 = self
            .directions()
            .iter()
            .enumerate()
            .filter_map(|(slot, azimuth)| {
                let azimuth = (*azimuth)?;
                let energy: f32 = hrir::pair(azimuth)
                    .iter()
                    .map(|h| h.iter().map(|v| v * v).sum::<f32>())
                    .sum();
                Some(energy * self.slot_energy[slot])
            })
            .sum();
        if self.present[LFE] {
            binaural += 2. * LFE_GAIN * LFE_GAIN * self.slot_energy[LFE];
        }
        if binaural == 0. {
            return 1.;
        }
        (downmix / binaural).sqrt()
    }
}

#[inline]
fn flush_tap(v: f32) -> f32 {
    if v.abs() < TAP_FLOOR { 0. } else { v }
}

fn build_taps(pair: &hrir::Pair, level: f32) -> Box<[[f32; TAPS]; 2]> {
    let mut taps = Box::new([[0.; TAPS]; 2]);
    for (ear, h) in taps.iter_mut().enumerate() {
        for (tap, v) in h.iter_mut().zip(pair[ear].iter().rev()) {
            *tap = flush_tap(v * level);
        }
    }
    taps
}

struct Speaker {
    slot: usize,
    /// Both ears' filters, time-reversed and scaled by the level.
    taps: Box<[[f32; TAPS]; 2]>,
    /// The last `TAPS - 1` samples, then the packet's.
    line: Vec<f32>,
}

/// A symmetric `(+az, -az)` speaker pair rendered with Mid/Side FIR
/// decomposition: `y_L = h_mid * (x_L + x_R) + h_side * (x_L - x_R)` and
/// `y_R = h_mid * (x_L + x_R) - h_side * (x_L - x_R)`, halving the 256-tap
/// convolutions from 4 per pair to 2.
struct SymmetricPair {
    slot_l: usize,
    slot_r: usize,
    /// `0.5 * (h_near + h_far)` and `0.5 * (h_near - h_far)`, time-reversed and scaled.
    taps_mid: Box<[f32; TAPS]>,
    taps_side: Box<[f32; TAPS]>,
    /// Direct near/far filters used when only one side of the pair is non-zero
    /// (same 2-filter cost, bit-exact single-channel impulse response).
    taps_near: Box<[f32; TAPS]>,
    taps_far: Box<[f32; TAPS]>,
    line_mid: Vec<f32>,
    line_side: Vec<f32>,
    left_tail: usize,
    right_tail: usize,
}

impl SymmetricPair {
    fn new(slot_l: usize, slot_r: usize, azimuth: i16, level: f32) -> Self {
        let pair = hrir::pair(azimuth);
        let mut taps_near = Box::new([0.; TAPS]);
        let mut taps_far = Box::new([0.; TAPS]);
        let mut taps_mid = Box::new([0.; TAPS]);
        let mut taps_side = Box::new([0.; TAPS]);
        for k in 0..TAPS {
            let near = flush_tap(pair[0][TAPS - 1 - k] * level);
            let far = flush_tap(pair[1][TAPS - 1 - k] * level);
            taps_near[k] = near;
            taps_far[k] = far;
            taps_mid[k] = flush_tap(0.5 * (near + far));
            taps_side[k] = flush_tap(0.5 * (near - far));
        }
        Self {
            slot_l,
            slot_r,
            taps_mid,
            taps_side,
            taps_near,
            taps_far,
            line_mid: vec![0.; TAPS - 1],
            line_side: vec![0.; TAPS - 1],
            left_tail: 0,
            right_tail: 0,
        }
    }

    fn feed(&mut self, input: &[f32]) -> (bool, bool) {
        let frames = input.len() / 8;
        let mut last_l = None;
        let mut last_r = None;
        for (i, frame) in input.as_chunks::<8>().0.iter().enumerate() {
            let l = frame[self.slot_l];
            let r = frame[self.slot_r];
            if l != 0. {
                last_l = Some(i);
            }
            if r != 0. {
                last_r = Some(i);
            }
            self.line_mid.push(l + r);
            self.line_side.push(l - r);
        }
        let active_l = self.left_tail > 0 || last_l.is_some();
        let active_r = self.right_tail > 0 || last_r.is_some();
        self.left_tail = match last_l {
            Some(i) => (TAPS - 1).saturating_sub(frames - 1 - i),
            None => self.left_tail.saturating_sub(frames),
        };
        self.right_tail = match last_r {
            Some(i) => (TAPS - 1).saturating_sub(frames - 1 - i),
            None => self.right_tail.saturating_sub(frames),
        };
        (active_l, active_r)
    }

    fn render(&mut self, input: &[f32], left: &mut [f32], right: &mut [f32]) {
        let frames = input.len() / 8;
        let (active_l, active_r) = self.feed(input);
        match (active_l, active_r) {
            (false, false) => {}
            (true, false) => {
                convolve_dual(
                    &self.line_mid,
                    &self.taps_near,
                    &self.taps_far,
                    frames,
                    left,
                    right,
                );
            }
            (false, true) => {
                convolve_dual(
                    &self.line_mid,
                    &self.taps_far,
                    &self.taps_near,
                    frames,
                    left,
                    right,
                );
            }
            (true, true) => {
                let blocks = frames / BLOCK * BLOCK;
                for start in (0..blocks).step_by(BLOCK) {
                    let (mut m, mut s) = ([0f32; BLOCK], [0f32; BLOCK]);
                    for (tap, (&hm, &hs)) in
                        self.taps_mid.iter().zip(self.taps_side.iter()).enumerate()
                    {
                        let xm: &[f32; BLOCK] = self.line_mid[start + tap..start + tap + BLOCK]
                            .try_into()
                            .unwrap();
                        let xs: &[f32; BLOCK] = self.line_side[start + tap..start + tap + BLOCK]
                            .try_into()
                            .unwrap();
                        for (((a, b), xm), xs) in m.iter_mut().zip(s.iter_mut()).zip(xm).zip(xs) {
                            *a += hm * xm;
                            *b += hs * xs;
                        }
                    }
                    for (((y, z), a), b) in left[start..start + BLOCK]
                        .iter_mut()
                        .zip(&mut right[start..start + BLOCK])
                        .zip(m)
                        .zip(s)
                    {
                        *y += a + b;
                        *z += a - b;
                    }
                }
                for n in blocks..frames {
                    let wm = &self.line_mid[n..n + TAPS];
                    let ws = &self.line_side[n..n + TAPS];
                    let m: f32 = self.taps_mid.iter().zip(wm).map(|(h, x)| h * x).sum();
                    let s: f32 = self.taps_side.iter().zip(ws).map(|(h, x)| h * x).sum();
                    left[n] += m + s;
                    right[n] += m - s;
                }
            }
        }
        self.line_mid.drain(..frames);
        self.line_side.drain(..frames);
    }

    fn listen(&mut self, input: &[f32]) {
        let frames = input.len() / 8;
        let _ = self.feed(input);
        self.line_mid.drain(..frames);
        self.line_side.drain(..frames);
    }
}

/// Center (`0°`) speaker: left and right HRIRs are identical, so one 256-tap
/// convolution feeds both ears.
struct CenterSpeaker {
    slot: usize,
    taps: Box<[f32; TAPS]>,
    line: Vec<f32>,
    tail: usize,
}

impl CenterSpeaker {
    fn new(slot: usize, level: f32) -> Self {
        let pair = hrir::pair(0);
        debug_assert_eq!(pair[0], pair[1]);
        let mut taps = Box::new([0.; TAPS]);
        for (tap, v) in taps.iter_mut().zip(pair[0].iter().rev()) {
            *tap = flush_tap(v * level);
        }
        Self {
            slot,
            taps,
            line: vec![0.; TAPS - 1],
            tail: 0,
        }
    }

    fn feed(&mut self, input: &[f32]) -> bool {
        let frames = input.len() / 8;
        let mut last = None;
        for (i, frame) in input.as_chunks::<8>().0.iter().enumerate() {
            let x = frame[self.slot];
            if x != 0. {
                last = Some(i);
            }
            self.line.push(x);
        }
        let active = self.tail > 0 || last.is_some();
        self.tail = match last {
            Some(i) => (TAPS - 1).saturating_sub(frames - 1 - i),
            None => self.tail.saturating_sub(frames),
        };
        active
    }

    fn render(&mut self, input: &[f32], left: &mut [f32], right: &mut [f32]) {
        let frames = input.len() / 8;
        if self.feed(input) {
            let blocks = frames / BLOCK * BLOCK;
            for start in (0..blocks).step_by(BLOCK) {
                let mut c = [0f32; BLOCK];
                for (tap, &h) in self.taps.iter().enumerate() {
                    let x: &[f32; BLOCK] = self.line[start + tap..start + tap + BLOCK]
                        .try_into()
                        .unwrap();
                    for (a, x) in c.iter_mut().zip(x) {
                        *a += h * x;
                    }
                }
                for ((y, z), a) in left[start..start + BLOCK]
                    .iter_mut()
                    .zip(&mut right[start..start + BLOCK])
                    .zip(c)
                {
                    *y += a;
                    *z += a;
                }
            }
            for n in blocks..frames {
                let window = &self.line[n..n + TAPS];
                let c: f32 = self.taps.iter().zip(window).map(|(h, x)| h * x).sum();
                left[n] += c;
                right[n] += c;
            }
        }
        self.line.drain(..frames);
    }

    fn listen(&mut self, input: &[f32]) {
        let frames = input.len() / 8;
        let _ = self.feed(input);
        self.line.drain(..frames);
    }
}

fn convolve_dual(
    line: &[f32],
    taps_l: &[f32; TAPS],
    taps_r: &[f32; TAPS],
    frames: usize,
    left: &mut [f32],
    right: &mut [f32],
) {
    // BLOCK frames at a time, tap by tap: both ears' sums stay in registers
    // and each step is a contiguous multiply-add across frames, so it vectorizes.
    let blocks = frames / BLOCK * BLOCK;
    for start in (0..blocks).step_by(BLOCK) {
        let (mut l, mut r) = ([0f32; BLOCK], [0f32; BLOCK]);
        for (tap, (hl, hr)) in taps_l.iter().zip(taps_r).enumerate() {
            let x: &[f32; BLOCK] = line[start + tap..start + tap + BLOCK].try_into().unwrap();
            for ((a, b), x) in l.iter_mut().zip(r.iter_mut()).zip(x) {
                *a += hl * x;
                *b += hr * x;
            }
        }
        for (y, a) in left[start..start + BLOCK].iter_mut().zip(l) {
            *y += a;
        }
        for (z, b) in right[start..start + BLOCK].iter_mut().zip(r) {
            *z += b;
        }
    }
    for n in blocks..frames {
        let window = &line[n..n + TAPS];
        left[n] += taps_l.iter().zip(window).map(|(h, x)| h * x).sum::<f32>();
        right[n] += taps_r.iter().zip(window).map(|(h, x)| h * x).sum::<f32>();
    }
}

pub struct Binaural {
    pairs: Vec<SymmetricPair>,
    centers: Vec<CenterSpeaker>,
    unpaired: Vec<Speaker>,
    /// Per-speaker mirror retained only for white-box unit tests (`binaural/tests.rs`).
    #[cfg(test)]
    speakers: Vec<Speaker>,
    lfe: Option<f32>,
    /// Each ear's output of the packet being rendered.
    left: Vec<f32>,
    right: Vec<f32>,
}

impl Binaural {
    pub fn new(layout: &Layout, level: f32) -> Self {
        let directions = layout.directions();
        let mut used = [false; 8];
        let mut pairs = Vec::new();
        let mut centers = Vec::new();
        let mut unpaired = Vec::new();
        for slot in 0..8 {
            if used[slot] {
                continue;
            }
            let Some(azimuth) = directions[slot] else {
                continue;
            };
            if azimuth == 0 {
                used[slot] = true;
                centers.push(CenterSpeaker::new(slot, level));
                continue;
            }
            if azimuth > 0
                && let Some(mirror) =
                    (slot + 1..8).find(|&other| !used[other] && directions[other] == Some(-azimuth))
            {
                used[slot] = true;
                used[mirror] = true;
                pairs.push(SymmetricPair::new(slot, mirror, azimuth, level));
                continue;
            }
            used[slot] = true;
            unpaired.push(Speaker {
                slot,
                taps: build_taps(hrir::pair(azimuth), level),
                line: vec![0.; TAPS - 1],
            });
        }
        #[cfg(test)]
        let speakers = directions
            .iter()
            .enumerate()
            .filter_map(|(slot, azimuth)| {
                Some(Speaker {
                    slot,
                    taps: build_taps(hrir::pair((*azimuth)?), level),
                    line: vec![0.; TAPS - 1],
                })
            })
            .collect();
        Self {
            pairs,
            centers,
            unpaired,
            #[cfg(test)]
            speakers,
            lfe: layout.present[LFE].then_some(LFE_GAIN * level),
            left: Vec::new(),
            right: Vec::new(),
        }
    }

    /// Renders interleaved 8-channel `input` into interleaved stereo `output`.
    pub fn render(&mut self, input: &[f32], output: &mut [f32]) {
        let frames = input.len() / 8;
        debug_assert_eq!(output.len(), frames * 2);
        let (left, right) = (&mut self.left, &mut self.right);
        left.clear();
        right.clear();
        match self.lfe {
            Some(gain) => {
                left.extend(input.iter().skip(LFE).step_by(8).map(|x| x * gain));
                right.extend_from_slice(left);
            }
            None => {
                left.resize(frames, 0.);
                right.resize(frames, 0.);
            }
        }
        for pair in &mut self.pairs {
            pair.render(input, left, right);
        }
        for center in &mut self.centers {
            center.render(input, left, right);
        }
        for speaker in &mut self.unpaired {
            speaker.feed(input);
            convolve_dual(
                &speaker.line,
                &speaker.taps[0],
                &speaker.taps[1],
                frames,
                left,
                right,
            );
            speaker.line.drain(..frames);
        }
        #[cfg(test)]
        for speaker in &mut self.speakers {
            speaker.feed(input);
            speaker.line.drain(..frames);
        }
        for (out, (l, r)) in output
            .as_chunks_mut::<2>()
            .0
            .iter_mut()
            .zip(left.iter().zip(right.iter()))
        {
            out[0] = *l;
            out[1] = *r;
        }
    }

    /// Keeps the filters' history current without rendering, so switching to
    /// [`Self::render`] later starts without a click.
    pub fn listen(&mut self, input: &[f32]) {
        let frames = input.len() / 8;
        for pair in &mut self.pairs {
            pair.listen(input);
        }
        for center in &mut self.centers {
            center.listen(input);
        }
        for speaker in &mut self.unpaired {
            speaker.feed(input);
            speaker.line.drain(..frames);
        }
        #[cfg(test)]
        for speaker in &mut self.speakers {
            speaker.feed(input);
            speaker.line.drain(..frames);
        }
    }
}

impl Speaker {
    fn feed(&mut self, input: &[f32]) {
        self.line
            .extend(input.iter().skip(self.slot).step_by(8).copied());
    }
}

/// Applies the 2x8 `downmix` to interleaved 8-channel `input`, skipping zero
/// weights so a stereo source comes out bit for bit.
pub fn downmix(weights: &[[f32; 8]; 2], input: &[f32], output: &mut [f32]) {
    for (out, frame) in output
        .as_chunks_mut::<2>()
        .0
        .iter_mut()
        .zip(input.as_chunks::<8>().0)
    {
        for (ear, row) in weights.iter().enumerate() {
            let mut sum = None;
            for (w, x) in row.iter().zip(frame) {
                if *w != 0. {
                    let v = if *w == 1. { *x } else { w * x };
                    sum = Some(sum.map_or(v, |s: f32| s + v));
                }
            }
            out[ear] = sum.unwrap_or(0.);
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod mid_side_tests {
    use super::*;

    #[test]
    fn mid_side_and_center_decomposition_matches_direct_convolution_within_1e_5() {
        for (channels, mask, expected_pairs, expected_centers) in
            [(8, 0x63f, 3, 1), (6, 0x3f, 2, 1), (6, 0x60f, 2, 1)]
        {
            let layout = Layout::new(channels, mask).unwrap();
            let level = layout.level();
            let mut binaural = Binaural::new(&layout, level);
            assert_eq!(binaural.pairs.len(), expected_pairs);
            assert_eq!(binaural.centers.len(), expected_centers);
            assert!(binaural.unpaired.is_empty());
            for pair in &binaural.pairs {
                for &v in pair
                    .taps_mid
                    .iter()
                    .chain(pair.taps_side.iter())
                    .chain(pair.taps_near.iter())
                    .chain(pair.taps_far.iter())
                {
                    assert!(v == 0. || v.abs() >= TAP_FLOOR, "{v:e}");
                }
            }
            for center in &binaural.centers {
                for &v in center.taps.iter() {
                    assert!(v == 0. || v.abs() >= TAP_FLOOR, "{v:e}");
                }
            }
            // Deterministic pseudo-random multi-channel input across 1200 frames.
            let frames = 1200;
            let mut state = 0x9e37_79b9_7f4a_7c15u64 ^ u64::from(mask);
            let input: Vec<f32> = (0..frames * 8)
                .map(|_| {
                    state = state
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    ((state >> 40) as f32 / (1u64 << 24) as f32) - 0.5
                })
                .collect();
            let mut direct_speakers: Vec<Speaker> = layout
                .directions()
                .iter()
                .enumerate()
                .filter_map(|(slot, az)| {
                    Some(Speaker {
                        slot,
                        taps: build_taps(hrir::pair((*az)?), level),
                        line: vec![0.; TAPS - 1],
                    })
                })
                .collect();
            let mut direct_l = vec![0f32; frames];
            let mut direct_r = vec![0f32; frames];
            if layout.present[LFE] {
                for (i, frame) in input.as_chunks::<8>().0.iter().enumerate() {
                    let v = frame[LFE] * LFE_GAIN * level;
                    direct_l[i] = v;
                    direct_r[i] = v;
                }
            }
            for sp in &mut direct_speakers {
                sp.feed(&input);
                convolve_dual(
                    &sp.line,
                    &sp.taps[0],
                    &sp.taps[1],
                    frames,
                    &mut direct_l,
                    &mut direct_r,
                );
            }
            let mut got = vec![0f32; frames * 2];
            for (in_chunk, out_chunk) in input.chunks(240 * 8).zip(got.chunks_mut(240 * 2)) {
                binaural.render(in_chunk, out_chunk);
            }
            let mut max_err = 0f32;
            for i in 0..frames {
                max_err = max_err
                    .max((got[i * 2] - direct_l[i]).abs())
                    .max((got[i * 2 + 1] - direct_r[i]).abs());
            }
            assert!(max_err < 1e-5, "{channels}ch {mask:#x}: max_err={max_err}");
        }
    }
}
