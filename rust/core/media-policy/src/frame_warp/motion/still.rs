//! Per-pixel static-region tracking to exclude unmasked HUD rims from motion estimates.

use super::{GRID, Motion, Thumbnail, edges};

/// A pixel that stays within this many luma codes of what it was has not changed: a HUD's
/// pixel in the thumbnail is the mean of many, so a game's dither over it comes to less.
const STILL_CHANGE: u8 = 2;
/// A move that should have changed a pixel by at least this much tells whether the pixel moved
/// along; less is within [`STILL_CHANGE`] and the capture's noise.
const STILL_STEP: f32 = 3.0;
/// A pixel stands still once the moves since it last changed should have changed it by this
/// many luma codes in all, of which one move counts for a quarter at most: where the measured
/// shift is wrong for once (a thin pole that passes over a pixel of sky), it is not four times.
const STILL_TOTAL: f32 = 24.0;
/// Moves shorter than this (thumbnail pixels) say nothing about what stands still.
const STILL_MIN_MOVE: f32 = 1.0;

/// What stands still in a picture that moves: a HUD's frames, bars and letters, down to the
/// thumbnail's pixel. The host's HUD mask ([`super::Mask`]) is what the client holds still, so
/// it takes only cells that are HUD all over and textured, and leaves out a HUD's rim and its
/// plain parts; and a client may want none. Left in the measurement, a minimap's frame or a
/// health bar's edge in a corner cell holds that cell's shift back, and the corners are where
/// a lens shows most ([`super::lens`]).
///
/// A pixel is told by what the moves should have done to it: each brings it what its cell's
/// shift says lay that far away, and what that would change it by adds up, one way or the
/// other, for as long as the pixel stays what it was. One that should have changed by
/// [`STILL_TOTAL`] and has not stands still, until it changes. A pixel the moves bring more of
/// the same (a sky, a horizon while the camera pans, the middle of a plain panel) is not told
/// and not marked.
#[derive(Debug, Default)]
pub struct Still {
    size: (usize, usize),
    /// Per pixel: what it was when it last changed, and what the moves since should have
    /// changed it by.
    pixels: Vec<(u8, f32)>,
    /// Per pixel: whether it or a neighbour stands still (a pixel on a HUD's rim is part HUD).
    mask: Vec<bool>,
    /// Number of pixels standing still within each pixel's 3x3 neighbourhood,
    /// avoiding full-frame rebuilds when only a handful of pixels flip.
    counts: Vec<u8>,
    marked: usize,
}

impl Still {
    /// Takes the move `motion` from `prev` to `cur`. Returns whether the mask changed.
    pub fn observe(&mut self, prev: &Thumbnail, cur: &Thumbnail, motion: &Motion) -> bool {
        let (w, h) = (cur.width, cur.height);
        if !prev.valid() || !cur.valid() || prev.width != w || prev.height != h {
            return false;
        }
        if self.size != (w, h) {
            self.size = (w, h);
            self.pixels.clear();
            self.pixels
                .extend(prev.luma.iter().map(|value| (*value, 0.0)));
            self.mask.clear();
            self.mask.resize(w * h, false);
            self.counts.clear();
            self.counts.resize(w * h, 0);
            self.marked = 0;
        }
        let (x_edges, y_edges) = (edges(w), edges(h));
        // A cell's pixels, and the margin beside an outer cell with it.
        let span = |edges: &[usize; GRID + 1], cell: usize, len: usize| {
            let from = if cell == 0 { 0 } else { edges[cell] };
            let to = if cell + 1 == GRID {
                len
            } else {
                edges[cell + 1]
            };
            from..to
        };
        let mut changed = false;
        for row in 0..GRID {
            for column in 0..GRID {
                // The cell's own shift while it goes with the picture's: a lens moves a corner
                // up to half again as far as the median cell. A cell that is mostly HUD finds
                // no shift, or the HUD's, which is none.
                let whole = motion.dx.hypot(motion.dy);
                let (dx, dy) = match motion.cells[row * GRID + column] {
                    Some(cell)
                        if (cell.dx - motion.dx).hypot(cell.dy - motion.dy)
                            <= 0.5 + 0.6 * whole =>
                    {
                        (cell.dx, cell.dy)
                    }
                    _ => (motion.dx, motion.dy),
                };
                let moved = dx.hypot(dy) >= STILL_MIN_MOVE;
                for y in span(&y_edges, row, h) {
                    for x in span(&x_edges, column, w) {
                        let at = y * w + x;
                        let (was, should) = &mut self.pixels[at];
                        let stood = should.abs() >= STILL_TOTAL;
                        if was.abs_diff(cur.luma[at]) > STILL_CHANGE {
                            (*was, *should) = (cur.luma[at], 0.0);
                        } else if moved
                            && let Some(brought) = sample(prev, x as f32 - dx, y as f32 - dy)
                        {
                            let step = brought - f32::from(prev.luma[at]);
                            if step.abs() >= STILL_STEP {
                                let most = STILL_TOTAL * 0.25;
                                *should += step.clamp(-most, most);
                            }
                        }
                        let now_stood = should.abs() >= STILL_TOTAL;
                        if stood != now_stood {
                            changed = true;
                            if now_stood {
                                self.marked += 1;
                            } else {
                                self.marked -= 1;
                            }
                            let x_start = x.saturating_sub(1);
                            let x_end = (x + 1).min(w - 1);
                            for ny in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                                let base = ny * w;
                                for nx in x_start..=x_end {
                                    let idx = base + nx;
                                    if now_stood {
                                        self.counts[idx] += 1;
                                        self.mask[idx] = true;
                                    } else {
                                        self.counts[idx] -= 1;
                                        self.mask[idx] = self.counts[idx] > 0;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        changed
    }

    /// Per thumbnail pixel, row by row, whether it stands still; `None` while none does.
    pub fn mask(&self) -> Option<&[bool]> {
        (self.marked > 0).then_some(&self.mask[..])
    }

    /// Forgets what stood still.
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

/// The thumbnail at a place between its pixels; `None` outside it.
fn sample(t: &Thumbnail, x: f32, y: f32) -> Option<f32> {
    if x < 0.0 || y < 0.0 || x > (t.width - 1) as f32 || y > (t.height - 1) as f32 {
        return None;
    }
    let (x0, y0) = (x as usize, y as usize);
    let (x1, y1) = ((x0 + 1).min(t.width - 1), (y0 + 1).min(t.height - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let top = t.at(x0, y0) * (1.0 - fx) + t.at(x1, y0) * fx;
    let bottom = t.at(x0, y1) * (1.0 - fx) + t.at(x1, y1) * fx;
    Some(top * (1.0 - fy) + bottom * fy)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient_frame(w: usize, h: usize, frame: usize, flip_a: bool, flip_b: bool) -> Thumbnail {
        let mut luma = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let is_a = (x, y) == (10, 10);
                let is_b = (x, y) == (12, 10);
                let val = if (is_a && !flip_a) || (is_b && !flip_b) {
                    (x * 6) as u8
                } else {
                    ((x * 6 + frame * 15) % 240) as u8
                };
                luma.push(val);
            }
        }
        Thumbnail {
            width: w,
            height: h,
            luma,
            at_us: frame as u64 * 16_000,
        }
    }

    #[test]
    fn incremental_counts_track_overlapping_grow_and_shrink() {
        let (w, h) = (32, 32);
        let prev = gradient_frame(w, h, 0, false, false);
        let motion = Motion::uniform(4.0, 0.0, 1.0);
        let mut still = Still::default();

        for step in 1..=4 {
            let cur = gradient_frame(w, h, step, false, false);
            let changed = still.observe(&prev, &cur, &motion);
            assert_eq!(changed, step == 4);
        }

        assert_eq!(still.marked, 2);
        let mask = still.mask().expect("two pixels stood still");
        assert_eq!(still.counts[10 * w + 11], 2);
        assert!(mask[10 * w + 9] && mask[10 * w + 11] && mask[10 * w + 13]);

        // Un-marking (10, 10) decrements its 3x3 neighborhood while keeping the
        // overlap at (11, 10) active for (12, 10).
        let cur = gradient_frame(w, h, 5, true, false);
        assert!(still.observe(&prev, &cur, &motion));
        assert_eq!(still.marked, 1);
        let mask = still.mask().expect("(12, 10) remains still");
        assert!(!mask[10 * w + 9]);
        assert_eq!(still.counts[10 * w + 11], 1);
        assert!(mask[10 * w + 11] && mask[10 * w + 13]);

        // Un-marking (12, 10) clears all counts and disables the mask.
        let cur = gradient_frame(w, h, 6, true, true);
        assert!(still.observe(&prev, &cur, &motion));
        assert_eq!(still.marked, 0);
        assert!(still.mask().is_none());
        assert!(still.counts.iter().all(|&c| c == 0));
        assert!(still.mask.iter().all(|&m| !m));
    }
}
