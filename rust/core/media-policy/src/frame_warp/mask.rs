//! HUD mask rate-limiting, letterboxed placement onto the picture, and morphological shaping.

use std::borrow::Cow;

use rubylight_protocol::frame_warp::Mask;

/// How often a changed HUD mask may be sent.
const MASK_EVERY_US: u64 = 1_000_000;
/// A mask that differs from the last sent in fewer cells than this is not sent.
const MASK_CHANGE: usize = 16;

/// Decides which HUD masks go out.
#[derive(Debug, Default)]
pub(super) struct MaskTracker {
    generation: u16,
    pub(super) sent: Option<Mask>,
    sent_at_us: Option<u64>,
    pub(super) pending: Option<Vec<u8>>,
}

impl MaskTracker {
    /// Sends the last mask again, scaled down to `max_width` if needed (a client
    /// that asked for width 0 receives no mask).
    pub(super) fn resend(&mut self, max_width: u16) {
        if max_width == 0 {
            self.pending = None;
            return;
        }
        let Some(mask) = self.sent.as_ref() else {
            return;
        };
        if mask.width <= max_width {
            self.pending = mask.encode();
            return;
        }
        let mut mask = mask.clone();
        while mask.width > max_width {
            mask = mask.halved();
        }
        self.queue(mask, None);
    }

    pub(super) fn clear_pending(&mut self) {
        self.pending = None;
    }

    fn queue(&mut self, mut mask: Mask, now_us: Option<u64>) {
        self.generation = self.generation.wrapping_add(1);
        mask.generation = self.generation;
        let mut wire = mask.encode();
        while wire.is_none() && mask.width > 1 {
            mask = mask.halved();
            wire = mask.encode();
        }
        let Some(wire) = wire else { return };
        self.pending = Some(wire);
        self.sent = Some(mask);
        if let Some(now_us) = now_us {
            self.sent_at_us = Some(now_us);
        }
    }

    /// A HUD mask, already shaped ([`shape`]).
    pub(super) fn offer(&mut self, shaped: &Mask, max_width: u16, now_us: u64) {
        let mut mask = Cow::Borrowed(shaped);
        while mask.width > max_width.max(1) {
            mask = Cow::Owned(mask.halved());
        }
        if let Some(sent) = &self.sent {
            if self
                .sent_at_us
                .is_some_and(|at| now_us.saturating_sub(at) < MASK_EVERY_US)
            {
                return;
            }
            if sent.width == mask.width && sent.height == mask.height {
                let changed: usize = sent
                    .words
                    .iter()
                    .zip(&mask.words)
                    .map(|(a, b)| (a ^ b).count_ones() as usize)
                    .sum();
                if changed < MASK_CHANGE {
                    return;
                }
            }
        } else if mask.is_clear() {
            return;
        }
        self.queue(mask.into_owned(), Some(now_us));
    }
}

/// Where the GPU's HUD grid, laid over the capture, lies on the stream's picture, which the
/// client masks: a capture of another shape is letterboxed into it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub picture: (u16, u16),
    /// Where the capture sits in the picture, `(x, y, width, height)`
    /// ([`crate::display_policy::letterbox`]); the rest is black bars.
    pub content: (u16, u16, u16, u16),
    /// The size of the mask the client gets, or 0 wide for none: its cells cover the picture.
    pub mask: (u16, u16),
}

impl Placement {
    /// `grid`, over the capture, as the client's mask over the picture: a cell is HUD where a
    /// grid cell whose centre falls in it is (as [`Mask::halved`] would make it), or, when none
    /// does, the grid cell under its centre. The bars count as HUD, so they hold still and the
    /// picture is not shifted in from them. A grid over the whole picture at the mask's size
    /// is already that mask.
    pub fn place<'a>(&self, grid: &'a Mask) -> Cow<'a, Mask> {
        let (x0, y0, cw, ch) = self.content;
        let (mw, mh) = self.mask;
        if (x0, y0, cw, ch) == (0, 0, self.picture.0, self.picture.1)
            && (grid.width, grid.height) == (mw, mh)
        {
            return Cow::Borrowed(grid);
        }
        let mut out = Mask::new(grid.generation, mw, mh);
        if grid.width == 0 || grid.height == 0 || cw == 0 || ch == 0 {
            return Cow::Owned(out);
        }
        let across: Vec<_> = (0..mw)
            .map(|x| grid_cells(x, mw, self.picture.0, x0, cw, grid.width))
            .collect();
        let mut row = vec![false; usize::from(grid.width)];
        for y in 0..mh {
            let Some(rows) = grid_cells(y, mh, self.picture.1, y0, ch, grid.height) else {
                (0..mw).for_each(|x| out.set(x, y, true));
                continue;
            };
            for (gx, hud) in (0..grid.width).zip(row.iter_mut()) {
                *hud = rows.clone().any(|gy| grid.get(gx, gy));
            }
            for (x, columns) in (0..mw).zip(&across) {
                let hud = columns.as_ref().is_none_or(|columns| {
                    row[usize::from(columns.start)..usize::from(columns.end)].contains(&true)
                });
                if hud {
                    out.set(x, y, true);
                }
            }
        }
        Cow::Owned(out)
    }
}

/// Along one axis, the grid cells a mask cell takes ([`Placement::place`]): mask cell `cell` of
/// `cells` over `picture` pixels, and `grid` cells over the `length` pixels from `start`. `None`
/// when the cell reaches past them, into a bar.
fn grid_cells(
    cell: u16,
    cells: u16,
    picture: u16,
    start: u16,
    length: u16,
    grid: u16,
) -> Option<std::ops::Range<u16>> {
    let (cell, cells, picture) = (u64::from(cell), u64::from(cells), u64::from(picture));
    let (start, length, grid) = (u64::from(start), u64::from(length), u64::from(grid));
    let (a, b) = (cell * picture / cells, (cell + 1) * picture / cells);
    if a < start || b > start + length {
        return None;
    }
    // Grid cell k's centre is (2k + 1) * length / (2 * grid) pixels from the start: the first
    // whose centre is at `p` or past it.
    let first = |p: u64| {
        (2 * grid * (p - start))
            .saturating_sub(length)
            .div_ceil(2 * length)
            .min(grid)
    };
    let (low, high) = (first(a), first(b));
    let (low, high) = if low < high {
        (low, high)
    } else {
        let at = ((a + b - 2 * start) * grid / (2 * length)).min(grid - 1);
        (at, at + 1)
    };
    Some(low as u16..high as u16)
}

/// The thumbnail pixels whose centre falls in a HUD cell of `mask`, row by row.
pub fn hud_pixels(mask: &Mask, (width, height): (usize, usize)) -> Vec<bool> {
    let (mw, mh) = (usize::from(mask.width), usize::from(mask.height));
    let mut out = vec![false; width * height];
    if mw == 0 || mh == 0 {
        return out;
    }
    for y in 0..height {
        let cy = ((2 * y + 1) * mh / (2 * height)).min(mh - 1);
        for x in 0..width {
            let cx = ((2 * x + 1) * mw / (2 * width)).min(mw - 1);
            out[y * width + x] = mask.get(cx as u16, cy as u16);
        }
    }
    out
}

/// HUD pieces up to twice this share of the picture's height apart are joined into one region
/// (16 pixels each way at 1080p). A translucent HUD, grey text on a panel or a ring of icons,
/// changes with the scene behind it, so the GPU finds it only where it is nearly opaque, in
/// pieces; left in pieces, it would break up as the scene shifts between them.
const JOIN_HEIGHT: f32 = 0.015;

/// The mask the client gets from the GPU's HUD cells: pieces close together joined
/// ([`JOIN_HEIGHT`]), then grown by one cell all round, since the HUD's anti-aliased edges change
/// a little with what is behind them and left out they would shift with the scene.
pub fn shape(mask: &Mask) -> Mask {
    let width = usize::from(mask.width);
    let mut cells: Vec<bool> = (0..mask.height)
        .flat_map(|y| (0..mask.width).map(move |x| mask.get(x, y)))
        .collect();
    let join = (f32::from(mask.height) * JOIN_HEIGHT).round() as usize;
    if join > 0 && cells.contains(&true) {
        // A closing: grown, then shrunk back, which fills the gaps it closed.
        grow(&mut cells, width, join, true);
        grow(&mut cells, width, join, false);
    }
    grow(&mut cells, width, 1, true);
    let mut out = Mask::new(mask.generation, mask.width, mask.height);
    for (i, _) in cells.iter().enumerate().filter(|(_, set)| **set) {
        out.set((i % width) as u16, (i / width) as u16, true);
    }
    out
}

/// Spreads `value` from every cell that has it to the cells within `reach` (a square), `width`
/// cells to a row: grows the set cells, or with `false` shrinks them. Nothing spreads from past
/// the edges, so a HUD at an edge keeps its size.
fn grow(cells: &mut [bool], width: usize, reach: usize, value: bool) {
    if width == 0 {
        return;
    }
    let mut line = Vec::with_capacity(width);
    for row in cells.chunks_exact_mut(width) {
        line.clear();
        line.extend_from_slice(row);
        spread(&line, reach, value, |x| row[x] = value);
    }
    // Down the columns a row at a time, which reads memory in order: per column, how many cells
    // of the window [y - reach, y + reach] had `value` before this pass.
    let before = cells.to_vec();
    let height = cells.len() / width;
    let mut counts = vec![0usize; width];
    let count = |counts: &mut [usize], y: usize, add: bool| {
        for (count, &cell) in counts.iter_mut().zip(&before[y * width..(y + 1) * width]) {
            if cell == value {
                if add {
                    *count += 1;
                } else {
                    *count -= 1;
                }
            }
        }
    };
    for y in 0..reach.min(height) {
        count(&mut counts, y, true);
    }
    for y in 0..height {
        if y + reach < height {
            count(&mut counts, y + reach, true);
        }
        if y > reach {
            count(&mut counts, y - reach - 1, false);
        }
        for (cell, &count) in cells[y * width..(y + 1) * width].iter_mut().zip(&counts) {
            if count > 0 {
                *cell = value;
            }
        }
    }
}

/// Calls `put` for each cell of `line` without `value` that has a cell with it within `reach`.
fn spread(line: &[bool], reach: usize, value: bool, mut put: impl FnMut(usize)) {
    let n = line.len();
    // How many cells of the window [i - reach, i + reach] have `value`.
    let mut count = line[..reach.min(n)].iter().filter(|&&v| v == value).count();
    for i in 0..n {
        if i + reach < n && line[i + reach] == value {
            count += 1;
        }
        if i > reach && line[i - reach - 1] == value {
            count -= 1;
        }
        if count > 0 && line[i] != value {
            put(i);
        }
    }
}
