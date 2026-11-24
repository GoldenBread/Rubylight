//! Frame Warp's passes over captures (docs/frame-warp.md): a luma thumbnail of each new capture for the host's motion measurement and, while the camera turns, a HUD mask. On Linux the pass descriptions and the mask grid are here; no GPU runs the passes yet, so making them for a capture fails with an error that says so.
use crate::capture::GpuImage;
use anyhow::Result;

/// The largest thumbnail: `butterpollo_media_policy::frame_warp::THUMBNAIL_WIDTH`
/// by the tallest `thumbnail_height`.
pub const THUMBNAIL_EDGE: u32 = 256;
/// HUD cells across or down at most
/// (`rubylight_protocol::frame_warp::MAX_MASK_EDGE`).
pub const MAX_GRID_EDGE: u32 = 2048;

/// What to run over one capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pass {
    /// The thumbnail's width, a multiple of 4, and height, each at most
    /// [`THUMBNAIL_EDGE`].
    pub thumbnail: (u32, u32),
    /// The HUD work while the camera turns; `None` while it stands still.
    pub hud: Option<Hud>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hud {
    /// The mask cells across and down ([`hud_grid`]).
    pub grid: (u32, u32),
    /// Copy the mask bits back for [`FrameWarpGpu::poll`]; ignored while one
    /// copy is still being read.
    pub read: bool,
    /// How far apart, in the `at` of [`FrameWarpGpu::submit`], the captures
    /// the pass compares are at least: the captures in between run none, so
    /// that a slow turn at a high frame rate still moves the scene far enough
    /// between the two for its pixels to change. 0 compares each capture with
    /// the one before.
    pub every_us: u64,
}
impl Pass {
    fn check(&self) -> Result<()> {
        let (width, height) = self.thumbnail;
        anyhow::ensure!(
            width % 4 == 0
                && (4..=THUMBNAIL_EDGE).contains(&width)
                && (1..=THUMBNAIL_EDGE).contains(&height),
            "Frame Warp thumbnail of {width}x{height}"
        );
        if let Some(hud) = &self.hud {
            anyhow::ensure!(
                (1..=MAX_GRID_EDGE).contains(&hud.grid.0)
                    && (1..=MAX_GRID_EDGE).contains(&hud.grid.1),
                "Frame Warp HUD grid of {}x{}",
                hud.grid.0,
                hud.grid.1
            );
        }
        Ok(())
    }
}

/// The HUD cells of a mask `width` cells wide over a `source` picture: as many
/// rows as keep the cells' shape the picture's, and no more cells than pixels.
/// `None` for no mask.
pub fn hud_grid(width: u16, source: (u32, u32)) -> Option<(u32, u32)> {
    if width == 0 || source.0 == 0 || source.1 == 0 {
        return None;
    }
    let across = u32::from(width).min(source.0).min(MAX_GRID_EDGE);
    let down =
        (u64::from(across) * u64::from(source.1) + u64::from(source.0) / 2) / u64::from(source.0);
    Some((across, (down as u32).clamp(1, source.1.min(MAX_GRID_EDGE))))
}

/// A finished thumbnail: `width` x `height` luma bytes, rows top to bottom.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThumbnailReadout {
    /// What [`FrameWarpGpu::submit`] was given for the capture.
    pub at: u64,
    pub width: usize,
    pub height: usize,
    pub luma: Vec<u8>,
}
/// A finished HUD mask: `height` rows of `width.div_ceil(32)` words, bit `i`
/// of word `w` being column `32 * w + i`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaskReadout {
    pub width: u32,
    pub height: u32,
    pub words: Vec<u32>,
}

/// The passes on the GPU a capture was made on. None can be made on Linux yet.
pub struct FrameWarpGpu {
    _private: (),
}

impl FrameWarpGpu {
    /// The passes on the GPU `image` was captured on. Not available on Linux yet.
    pub fn for_image(_image: &GpuImage) -> Result<Self> {
        Err(crate::unsupported("Frame Warp's GPU passes"))
    }

    /// Whether `image` is on the capture device these passes were made for.
    pub fn accepts(&self, _image: &GpuImage) -> bool {
        false
    }

    /// Run `pass` over `image`; its thumbnail comes back from [`FrameWarpGpu::poll`] with
    /// `at`. `Ok(None)` when the capture is skipped, as every capture is without a GPU to
    /// run the passes; a pass that could never run is still refused.
    pub fn submit(&mut self, _image: &GpuImage, pass: &Pass, _at: u64) -> Result<Option<bool>> {
        pass.check()?;
        Ok(None)
    }

    /// The thumbnails whose passes finished, oldest first, and the mask copy once it
    /// finished. Never waits; nothing finishes where nothing ran.
    pub fn poll(&mut self) -> Result<(Vec<ThumbnailReadout>, Option<MaskReadout>)> {
        Ok((Vec::new(), None))
    }
}
