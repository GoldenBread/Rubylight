//! Intensity profile extraction, detrending, normalised correlation, and sub-pixel refinement.

use super::{GRID, Place, Thumbnail};

/// Correlation a cell needs to count.
const MIN_CORRELATION: f32 = 0.6;
/// How much a cell's best shift must beat any other more than 2 pixels away.
const MIN_UNIQUENESS: f32 = 0.04;
/// Profile variance below this is a flat cell (sky, a wall in shadow).
const MIN_VARIANCE: f32 = 0.5;

/// What a profile is built in ([`profile_rows`], [`profile_columns`]).
#[derive(Debug, Default)]
pub(super) struct Lines {
    line: Vec<f32>,
    /// Per sample of the profile: its pixels' sum, their count and the sum of their places
    /// across the profile.
    sums: Vec<(f32, u32, f32)>,
    prefix: Vec<f32>,
    count: Vec<u32>,
}

/// Into `out`: each row's profile over columns `x0..x1`, the columns taken `shift` to the right
/// (clamped at the edges): the mean of its pixels where `scene` (not the HUD), each less its
/// own column's moving average ([`detrend`]); NaN for a row of HUD only. Taking the average out
/// column by column leaves the profile of a whole picture as taking it out of the rows' means
/// would, and keeps a HUD from marking it: rows that a HUD covers half of are the mean of other
/// columns than their neighbours, at another level, and that step, standing where the HUD
/// stands in every capture, would hold the measured shift back.
pub(super) fn profile_rows<S: Fn(usize, usize) -> bool>(
    t: &Thumbnail,
    x0: usize,
    x1: usize,
    shift: isize,
    scene: S,
    lines: &mut Lines,
    out: &mut Vec<f32>,
) {
    let last = t.width as isize - 1;
    let Lines {
        line,
        sums,
        prefix,
        count,
    } = lines;
    sums.clear();
    sums.resize(t.height, (0.0, 0, 0.0));
    for x in x0..x1 {
        let x = (x as isize + shift).clamp(0, last) as usize;
        line.clear();
        line.extend((0..t.height).map(|y| if scene(x, y) { t.at(x, y) } else { f32::NAN }));
        detrend(line, prefix, count);
        gather(line, x as f32 + 0.5 - t.width as f32 * 0.5, sums);
    }
    means(sums, out);
}

/// Into `out`: each column's profile over rows `y0..y1`, the rows taken `shift` lower (clamped
/// at the edges), as [`profile_rows`] with each pixel less its own row's moving average; NaN
/// for a column of HUD only.
pub(super) fn profile_columns<S: Fn(usize, usize) -> bool>(
    t: &Thumbnail,
    y0: usize,
    y1: usize,
    shift: isize,
    scene: S,
    lines: &mut Lines,
    out: &mut Vec<f32>,
) {
    let Lines {
        line,
        sums,
        prefix,
        count,
    } = lines;
    sums.clear();
    sums.resize(t.width, (0.0, 0, 0.0));
    for y in y0..y1 {
        let y = (y as isize + shift).clamp(0, t.height as isize - 1) as usize;
        line.clear();
        line.extend((0..t.width).map(|x| if scene(x, y) { t.at(x, y) } else { f32::NAN }));
        detrend(line, prefix, count);
        gather(line, y as f32 + 0.5 - t.height as f32 * 0.5, sums);
    }
    means(sums, out);
}

/// Adds a detrended line's known samples to the profile's sums; `at` is the line's place
/// across the profile, from the thumbnail's centre.
fn gather(line: &[f32], at: f32, sums: &mut [(f32, u32, f32)]) {
    for (value, (sum, n, across)) in line.iter().zip(sums) {
        if !value.is_nan() {
            *sum += value;
            *n += 1;
            *across += at;
        }
    }
}

fn means(sums: &[(f32, u32, f32)], out: &mut Vec<f32>) {
    out.clear();
    out.extend(
        sums.iter()
            .map(|&(sum, n, _)| if n == 0 { f32::NAN } else { sum / n as f32 }),
    );
}

impl Lines {
    /// Into `out`: where across the profile built last each sample's pixels lie on average,
    /// from the thumbnail's centre; NaN for a sample of HUD only.
    pub(super) fn across(&self, out: &mut Vec<f32>) {
        out.clear();
        out.extend(self.sums.iter().map(
            |&(_, n, across)| {
                if n == 0 { f32::NAN } else { across / n as f32 }
            },
        ));
    }
}

/// A line of the picture, in place, less its moving average over 13 samples: lighting and slow
/// gradients out, edges and texture kept. NaN samples (HUD) stay NaN, and so does a sample with
/// one among those 13: its average would lean to the side away from the HUD, and what a slow
/// gradient leaves of itself there stands where the HUD stands in every capture, holding the
/// measured shift back as the HUD itself would. (At the picture's own edge the average leans
/// too; the cells keep a margin from it.) `prefix` and `count` are scratch.
pub(super) fn detrend(profile: &mut [f32], prefix: &mut Vec<f32>, count: &mut Vec<u32>) {
    const RADIUS: usize = 6;
    let n = profile.len();
    prefix.clear();
    count.clear();
    prefix.push(0.0f32);
    count.push(0u32);
    for v in profile.iter() {
        let known = !v.is_nan();
        prefix.push(prefix.last().unwrap() + if known { *v } else { 0.0 });
        count.push(count.last().unwrap() + u32::from(known));
    }
    // Each sample's average comes from the running sums, not its neighbours, so the profile
    // can take the result as it goes.
    for (i, value) in profile.iter_mut().enumerate() {
        let a = i.saturating_sub(RADIUS);
        let b = (i + RADIUS + 1).min(n);
        let known = count[b] - count[a];
        *value = if known as usize == b - a {
            *value - (prefix[b] - prefix[a]) / known as f32
        } else {
            f32::NAN
        };
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Shift {
    /// Sub-pixel shift: `after[i + at] ~ before[i]`.
    pub(super) at: f32,
    pub(super) correlation: f32,
    /// Where the shift was measured.
    pub(super) place: Option<Place>,
}

/// The shift of `before[from..to]` within `after`, searched over `-search..=search`, by
/// normalised correlation and refined to sub-pixel by a parabola through the peak. `None` for
/// a flat cell, a weak or an ambiguous peak. `before` comes with where across it its samples'
/// pixels lie ([`Lines::across`]); `scores` is scratch.
pub(super) fn best_shift(
    (before, across): (&[f32], &[f32]),
    after: &[f32],
    from: usize,
    to: usize,
    search: usize,
    scores: &mut Vec<Option<f32>>,
) -> Option<Shift> {
    let len = to.checked_sub(from)?;
    let known = || before[from..to].iter().copied().filter(|v| !v.is_nan());
    let count = known().count();
    if len < 8 || count < 8 {
        return None;
    }
    let mean = known().sum::<f32>() / count as f32;
    let variance = known().map(|v| (v - mean) * (v - mean)).sum::<f32>() / count as f32;
    if variance < MIN_VARIANCE {
        return None;
    }
    let search = search as isize;
    scores.clear();
    for s in -search..=search {
        scores.push(correlation(before, after, from, to, s));
    }
    let (best, peak) = scores
        .iter()
        .enumerate()
        .filter_map(|(i, s)| s.map(|s| (i, s)))
        .max_by(|a, b| a.1.total_cmp(&b.1))?;
    if peak < MIN_CORRELATION {
        return None;
    }
    let runner_up = scores
        .iter()
        .enumerate()
        .filter(|(i, _)| i.abs_diff(best) > 2)
        .filter_map(|(_, s)| *s)
        .fold(f32::NEG_INFINITY, f32::max);
    if peak - runner_up < MIN_UNIQUENESS {
        return None;
    }
    // A peak at the edge of the search is not a peak.
    if best == 0 || best + 1 == scores.len() {
        return None;
    }
    let (l, r) = (scores[best - 1]?, scores[best + 1]?);
    let curvature = l - 2.0 * peak + r;
    let offset = if curvature < 0.0 {
        (0.5 * (l - r) / curvature).clamp(-0.5, 0.5)
    } else {
        0.0
    };
    let whole = best as f32 - search as f32;
    Some(Shift {
        at: refine(before, after, from, to, whole + offset, whole),
        correlation: peak,
        place: place(before, across, from, to),
    })
}

/// Where `refine` measures the shift of `before[from..to]`: its steps weigh each sample with its
/// squared slope, so the place is those weights' mean, along the profile from its middle and
/// across it as `across` has each sample. `None` for a profile without slopes.
pub(super) fn place(before: &[f32], across: &[f32], from: usize, to: usize) -> Option<Place> {
    let middle = before.len() as f32 * 0.5;
    let mut sums = [0.0f64; 5];
    for i in from.max(1)..to.min(before.len().saturating_sub(1)) {
        let slope = before[i + 1] - before[i - 1];
        let beside = across.get(i).copied().unwrap_or(f32::NAN);
        if slope.is_nan() || beside.is_nan() {
            continue;
        }
        let (weight, at, beside) = (
            f64::from(slope * slope),
            f64::from(i as f32 + 0.5 - middle),
            f64::from(beside),
        );
        for (sum, term) in sums.iter_mut().zip([1.0, at, at * at, beside, at * beside]) {
            *sum += weight * term;
        }
    }
    let [weight, along, squared, beside, cross] = sums;
    (weight > f64::from(f32::EPSILON)).then(|| Place {
        along: (along / weight) as f32,
        squared: (squared / weight) as f32,
        across: (beside / weight) as f32,
        cross: (cross / weight) as f32,
    })
}

/// The sub-pixel shift, from `start` by gradient steps (Lucas-Kanade on the profiles): a
/// parabola through three correlation samples pulls shifts under half a pixel toward the
/// nearest whole one, which would teach the model too small a gain for slow turns. Stays
/// within a pixel of `whole`, the best whole shift, and keeps `start` when the profile has no
/// slope to follow.
fn refine(before: &[f32], after: &[f32], from: usize, to: usize, start: f32, whole: f32) -> f32 {
    // `after` at fractional index `x`, linearly interpolated; NaN outside or where unknown.
    let sample = |x: f32| -> f32 {
        if x < 0.0 || x > (after.len() - 1) as f32 {
            return f32::NAN;
        }
        let i = (x.floor() as usize).min(after.len() - 1);
        let j = (i + 1).min(after.len() - 1);
        let t = x - i as f32;
        after[i] * (1.0 - t) + after[j] * t
    };
    let mut at = start;
    for _ in 0..4 {
        let (mut num, mut den) = (0.0f32, 0.0f32);
        for (i, &a) in before.iter().enumerate().take(to).skip(from) {
            let x = i as f32 + at;
            let (b, slope) = (sample(x), 0.5 * (sample(x + 1.0) - sample(x - 1.0)));
            if a.is_nan() || b.is_nan() || slope.is_nan() {
                continue;
            }
            num += slope * (a - b);
            den += slope * slope;
        }
        if den <= f32::EPSILON {
            return start;
        }
        let step = (num / den).clamp(-0.5, 0.5);
        at = (at + step).clamp(whole - 1.0, whole + 1.0);
        if step.abs() < 0.005 {
            break;
        }
    }
    at
}

/// Normalised correlation of `before[from..to]` with `after[from + s..to + s]`, over the part
/// that stays inside `after` and saw some of the scene in both; `None` when that is less than
/// 60% of the cell.
fn correlation(before: &[f32], after: &[f32], from: usize, to: usize, s: isize) -> Option<f32> {
    let lo = (from as isize).max(-s).max(0) as usize;
    let hi = ((to as isize).min(after.len() as isize - s)).max(0) as usize;
    if hi <= lo {
        return None;
    }
    // The pairs where both profiles saw some of the scene.
    let (a, b) = (
        &before[lo..hi],
        &after[(lo as isize + s) as usize..(hi as isize + s) as usize],
    );
    let pairs = || {
        a.iter()
            .zip(b)
            .map(|(&a, &b)| (a, b))
            .filter(|(a, b)| !a.is_nan() && !b.is_nan())
    };
    let (mut count, mut sa, mut sb) = (0usize, 0.0f32, 0.0f32);
    for (a, b) in pairs() {
        count += 1;
        sa += a;
        sb += b;
    }
    if count * 10 < (to - from) * 6 {
        return None;
    }
    let n = count as f32;
    let (ma, mb) = (sa / n, sb / n);
    let (mut ab, mut aa, mut bb) = (0.0f32, 0.0f32, 0.0f32);
    for (a, b) in pairs() {
        let (a, b) = (a - ma, b - mb);
        ab += a * b;
        aa += a * a;
        bb += b * b;
    }
    if aa <= f32::EPSILON || bb <= f32::EPSILON {
        return None;
    }
    Some(ab / (aa * bb).sqrt())
}

pub(super) fn median(values: &[f32]) -> Option<f32> {
    if values.is_empty() {
        return None;
    }
    // At most a value per cell here: sorted on the stack.
    let mut buffer = [0.0f32; GRID * GRID];
    let mut owned = Vec::new();
    let sorted = if values.len() <= buffer.len() {
        let sorted = &mut buffer[..values.len()];
        sorted.copy_from_slice(values);
        sorted
    } else {
        owned.extend_from_slice(values);
        owned.as_mut_slice()
    };
    sorted.sort_by(f32::total_cmp);
    let mid = sorted.len() / 2;
    Some(if sorted.len() % 2 == 1 {
        sorted[mid]
    } else {
        0.5 * (sorted[mid - 1] + sorted[mid])
    })
}
