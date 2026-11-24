//! How far the picture's content moved between two captures, from small luma thumbnails.
//!
//! The camera turns the whole scene, so the estimate works on intensity profiles: for a band
//! of rows, the mean of each column (and for a band of columns, the mean of each row), with
//! its slow trend taken out. A shift of the profile is a shift of the content. The thumbnail
//! is split into a 3 by 3 grid; each cell finds its own shift by normalised correlation, and
//! the median of the cells that found one clearly is the picture's. Cells that disagree (the
//! HUD, a character crossing) do not move the median. The estimate only teaches the model;
//! nothing on screen depends on one frame's estimate.

/// A capture's luma, downscaled, as the GPU returns it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thumbnail {
    pub width: usize,
    pub height: usize,
    /// `width * height` bytes, rows top to bottom.
    pub luma: Vec<u8>,
    /// The capture's present time, microseconds on the session clock.
    pub at_us: u64,
}

impl Thumbnail {
    fn at(&self, x: usize, y: usize) -> f32 {
        f32::from(self.luma[y * self.width + x])
    }

    fn valid(&self) -> bool {
        self.width >= MIN_EDGE
            && self.height >= MIN_EDGE
            && self.luma.len() == self.width * self.height
    }
}

/// The content's shift from one thumbnail to the next, in thumbnail pixels (x right, y down).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Motion {
    pub dx: f32,
    pub dy: f32,
    /// 0 to 1: how many cells agree with the median, and how clearly they found it.
    pub confidence: f32,
    /// Each grid cell's own shift where it found one clearly, rows top to bottom and columns
    /// left to right ([`cell_spans`] says where they are). A camera turn moves a perspective
    /// picture's edges farther than its centre, which the cells show and the median hides
    /// ([`super::lens`]).
    pub cells: [Option<CellShift>; CELLS],
}

/// One grid cell's shift, in thumbnail pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellShift {
    pub dx: f32,
    pub dy: f32,
    pub correlation: f32,
    /// Where `dx` and where `dy` were measured; `None` for the whole cell alike.
    pub place: Option<[Place; 2]>,
}

/// Where one of a cell's shifts was measured, in thumbnail pixels from the thumbnail's centre. A
/// profile is followed by its slopes, each weighing in with its square, so a cell that is half
/// sky moves as its textured half does, a picture stretched toward its edge by a lens as the
/// less stretched part of the cell, and a cell half under a HUD as the half beside it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    /// The slopes' mean place along the shift's axis, and its mean square.
    pub along: f32,
    pub squared: f32,
    /// The mean place, across that axis, of the pixels behind the slopes, and the mean of the
    /// two places' product.
    pub across: f32,
    pub cross: f32,
}

impl Motion {
    /// A shift every cell shares, as a flat picture's is.
    pub fn uniform(dx: f32, dy: f32, confidence: f32) -> Self {
        Self {
            dx,
            dy,
            confidence,
            cells: [Some(CellShift {
                dx,
                dy,
                correlation: confidence,
                place: None,
            }); CELLS],
        }
    }
}

/// The thumbnail's longer side, as the GPU makes it.
pub const THUMBNAIL_WIDTH: usize = 256;
const MIN_EDGE: usize = 32;
pub const GRID: usize = 3;
/// Cells in the grid.
pub const CELLS: usize = GRID * GRID;
/// Edges left out: vignettes, letterboxing and edge HUD.
const MARGIN: f32 = 0.04;
/// Largest shift searched, as a fraction of the profile's length.
const SEARCH: f32 = 0.2;
/// Cells within this many pixels of the median agree with it.
const AGREE: f32 = 1.5;
/// A pixel this far from its right and its lower neighbour (in luma codes) is texture: it
/// changes whichever way it moves (an edge along the move would not).
const TEXTURE: u8 = 8;
/// A cell whose texture is this much unchanged stands still.
const STILL_SHARE: f32 = 0.9;
/// A cell needs this many texture pixels to tell whether it stands still.
const MIN_TEXTURE: usize = 16;

mod profile;
mod still;

use profile::{Lines, Shift, best_shift, median, profile_columns, profile_rows};
#[cfg(test)]
use profile::{detrend, place};
pub use still::Still;

/// The content's shift from `prev` to `cur`, or `None` when the thumbnails differ in size or
/// too few cells find a clear shift.
pub fn estimate(prev: &Thumbnail, cur: &Thumbnail) -> Option<Motion> {
    estimate_around(prev, cur, None)
}

/// [`estimate`], leaving out the thumbnail pixels `hud` marks (row by row, the thumbnail's
/// size): a HUD stays where it is, and a wide bar's rows left in would pull every vertical
/// shift toward 0. Ignored when its size does not match.
pub fn estimate_around(prev: &Thumbnail, cur: &Thumbnail, hud: Option<&[bool]>) -> Option<Motion> {
    estimate_with(prev, cur, hud, &mut Scratch::default())
}

/// The buffers an estimate works in, kept from one estimate to the next.
#[derive(Debug, Default)]
pub struct Scratch {
    /// The previous thumbnail's detrended row profile for each column band, unshifted: the
    /// same in both vertical passes.
    before_rows: [Vec<f32>; GRID],
    /// Where across each of those profiles' rows its pixels lie ([`Lines::across`]).
    before_across: [Vec<f32>; GRID],
    buffers: Buffers,
}

#[derive(Debug, Default)]
struct Buffers {
    before: Vec<f32>,
    across: Vec<f32>,
    after: Vec<f32>,
    scores: Vec<Option<f32>>,
    lines: Lines,
}

/// [`estimate_around`] in `scratch`'s buffers.
pub fn estimate_with(
    prev: &Thumbnail,
    cur: &Thumbnail,
    hud: Option<&[bool]>,
    scratch: &mut Scratch,
) -> Option<Motion> {
    if !prev.valid() || !cur.valid() || prev.width != cur.width || prev.height != cur.height {
        return None;
    }
    let w = cur.width;
    // One branch here, so the loops below inline the test (or drop it without a HUD).
    match hud.filter(|hud| hud.len() == cur.width * cur.height) {
        None => measure(prev, cur, |_, _| true, scratch),
        Some(hud) => measure(prev, cur, |x, y| !hud[y * w + x], scratch),
    }
}

/// Where the cells are and how far each searches.
struct Grid {
    x_edges: [usize; GRID + 1],
    y_edges: [usize; GRID + 1],
    search_x: usize,
    search_y: usize,
}

/// [`estimate_around`] where `scene` is the picture (not the HUD).
fn measure<S: Fn(usize, usize) -> bool + Copy>(
    prev: &Thumbnail,
    cur: &Thumbnail,
    scene: S,
    scratch: &mut Scratch,
) -> Option<Motion> {
    let (w, h) = (cur.width, cur.height);
    let grid = Grid {
        x_edges: edges(w),
        y_edges: edges(h),
        search_x: ((w as f32 * SEARCH) as usize).max(2),
        search_y: ((h as f32 * SEARCH) as usize).max(2),
    };
    let (x_edges, y_edges) = (&grid.x_edges, &grid.y_edges);

    // A picture whose texture (the HUD left out) is mostly unchanged stands still, whatever
    // moves in one corner of it.
    let still = still_cells(prev, cur, scene, x_edges, y_edges);
    if still * 2 > GRID * GRID {
        return Some(Motion {
            dx: 0.0,
            dy: 0.0,
            confidence: still as f32 / (GRID * GRID) as f32,
            cells: [None; CELLS],
        });
    }

    // Vertical first: one row profile per column band, full height, correlated per row band.
    // Then horizontal: one column profile per row band, the current one taken that band's
    // vertical shift lower so a tilt does not blur it. Then vertical again, the current
    // column band taken that band's horizontal shift across, so a fast pan does not blur it.
    let Scratch {
        before_rows,
        before_across,
        buffers,
    } = scratch;
    for (band, (before, across)) in before_rows.iter_mut().zip(&mut *before_across).enumerate() {
        let (x0, x1) = (x_edges[band], x_edges[band + 1]);
        profile_rows(prev, x0, x1, 0, scene, &mut buffers.lines, before);
        buffers.lines.across(across);
    }
    let before_rows: [(&[f32], &[f32]); GRID] =
        core::array::from_fn(|band| (&before_rows[band][..], &before_across[band][..]));
    let first: [[Option<Shift>; GRID]; GRID] = core::array::from_fn(|band| {
        vertical_band(cur, scene, &grid, band, before_rows[band], 0, buffers)
    });
    let mut horizontal = [[None; GRID]; GRID];
    for cell in 0..GRID {
        let (y0, y1) = (y_edges[cell], y_edges[cell + 1]);
        let shifts: Vec<f32> = (0..GRID)
            .filter_map(|band| first[band][cell].map(|s| s.at))
            .collect();
        let shift_y = median(&shifts).unwrap_or(0.0).round() as isize;
        let Buffers {
            before,
            across,
            after,
            scores,
            lines,
        } = &mut *buffers;
        profile_columns(prev, y0, y1, 0, scene, lines, before);
        lines.across(across);
        profile_columns(cur, y0, y1, shift_y, scene, lines, after);
        for (band, column) in horizontal.iter_mut().enumerate() {
            column[cell] = best_shift(
                (before, across),
                after,
                x_edges[band],
                x_edges[band + 1],
                grid.search_x,
                scores,
            );
        }
    }
    let column_shift: [isize; GRID] = core::array::from_fn(|band| {
        let shifts: Vec<f32> = horizontal[band].iter().flatten().map(|s| s.at).collect();
        median(&shifts).unwrap_or(0.0).round() as isize
    });
    // A band that did not move across measures as it did the first time.
    let mut vertical = first;
    for (band, cells) in vertical.iter_mut().enumerate() {
        if column_shift[band] != 0 {
            *cells = vertical_band(
                cur,
                scene,
                &grid,
                band,
                before_rows[band],
                column_shift[band],
                buffers,
            );
        }
    }
    let mut cells = Vec::with_capacity(GRID * GRID);
    let mut each = [None; CELLS];
    for band in 0..GRID {
        for cell in 0..GRID {
            if let (Some(x), Some(y)) = (horizontal[band][cell], vertical[band][cell]) {
                let correlation = x.correlation.min(y.correlation);
                cells.push((x.at, y.at, correlation));
                each[cell * GRID + band] = Some(CellShift {
                    dx: x.at,
                    dy: y.at,
                    correlation,
                    place: x.place.zip(y.place).map(|(x, y)| [x, y]),
                });
            }
        }
    }
    if cells.len() < 3 {
        return None;
    }
    let dx = median(&cells.iter().map(|c| c.0).collect::<Vec<_>>())?;
    let dy = median(&cells.iter().map(|c| c.1).collect::<Vec<_>>())?;
    let agreeing: Vec<f32> = cells
        .iter()
        .filter(|c| (c.0 - dx).abs() <= AGREE && (c.1 - dy).abs() <= AGREE)
        .map(|c| c.2)
        .collect();
    if agreeing.len() < 3 {
        return None;
    }
    let mean_correlation = agreeing.iter().sum::<f32>() / agreeing.len() as f32;
    let confidence = (agreeing.len() as f32 / (GRID * GRID) as f32) * mean_correlation;
    Some(Motion {
        dx,
        dy,
        confidence: confidence.clamp(0.0, 1.0),
        cells: each,
    })
}

/// Where the grid's cells are along an edge of `len` thumbnail pixels: each cell's first pixel
/// and the one past its last.
pub fn cell_spans(len: usize) -> [(usize, usize); GRID] {
    let edges = edges(len);
    core::array::from_fn(|i| (edges[i], edges[i + 1]))
}

/// The vertical shift of each cell in column band `band`: `before` (the previous thumbnail's
/// detrended row profile over the band) found in the current one's, its columns taken
/// `column_shift` across.
fn vertical_band<S: Fn(usize, usize) -> bool + Copy>(
    cur: &Thumbnail,
    scene: S,
    grid: &Grid,
    band: usize,
    before: (&[f32], &[f32]),
    column_shift: isize,
    buffers: &mut Buffers,
) -> [Option<Shift>; GRID] {
    let Buffers {
        after,
        scores,
        lines,
        ..
    } = buffers;
    let (x0, x1) = (grid.x_edges[band], grid.x_edges[band + 1]);
    profile_rows(cur, x0, x1, column_shift, scene, lines, after);
    core::array::from_fn(|cell| {
        best_shift(
            before,
            after,
            grid.y_edges[cell],
            grid.y_edges[cell + 1],
            grid.search_y,
            scores,
        )
    })
}

/// Cell boundaries along an edge of `len` pixels, margins left out.
fn edges(len: usize) -> [usize; GRID + 1] {
    let margin = (len as f32 * MARGIN) as usize;
    let inner = len - 2 * margin;
    let mut out = [0; GRID + 1];
    for (i, edge) in out.iter_mut().enumerate() {
        *edge = margin + inner * i / GRID;
    }
    out
}

/// How many grid cells stand still: their texture in `prev` where `scene` is at least
/// [`STILL_SHARE`] exactly the same in `cur`.
fn still_cells<S: Fn(usize, usize) -> bool>(
    prev: &Thumbnail,
    cur: &Thumbnail,
    scene: S,
    x_edges: &[usize; GRID + 1],
    y_edges: &[usize; GRID + 1],
) -> usize {
    let w = prev.width;
    let texture = |x: usize, y: usize| {
        let v = prev.luma[y * w + x];
        let right = prev.luma[y * w + (x + 1).min(w - 1)];
        let below = prev.luma[(y + 1).min(prev.height - 1) * w + x];
        v.abs_diff(right) >= TEXTURE && v.abs_diff(below) >= TEXTURE
    };
    let mut still = 0;
    for row in 0..GRID {
        for column in 0..GRID {
            let (mut textured, mut unchanged) = (0usize, 0usize);
            for y in y_edges[row]..y_edges[row + 1] {
                for x in x_edges[column]..x_edges[column + 1] {
                    if scene(x, y) && texture(x, y) {
                        textured += 1;
                        unchanged += usize::from(prev.luma[y * w + x] == cur.luma[y * w + x]);
                    }
                }
            }
            if textured >= MIN_TEXTURE && unchanged as f32 >= textured as f32 * STILL_SHARE {
                still += 1;
            }
        }
    }
    still
}

#[cfg(test)]
pub(crate) mod tests;
