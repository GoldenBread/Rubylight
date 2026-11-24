//! The couch menu's panel as an encoder's colour conversion draws it into the stream (`docs/couch-menu-design.md` sections 6.2 and 6.3). On Linux the published panel and its checks are here; no conversion draws it yet.
use anyhow::{Result, ensure};
use std::sync::Arc;

/// Target-pixel alignment of the panel's origin and size.
pub const ALIGN: u32 = 16;

/// One published panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Overlay {
    /// Rises whenever the panel changes; the converters upload it only then.
    pub revision: u64,
    /// Target pixels, multiples of [`ALIGN`].
    pub origin: [u32; 2],
    pub size: [u32; 2],
    /// Whole-number magnification of `rgba`.
    pub scale: u32,
    /// The panel at 1x, `size / scale` pixels, RGBA8 sRGB.
    pub rgba: Arc<[u8]>,
}

impl Overlay {
    pub fn new(
        revision: u64,
        origin: [u32; 2],
        size: [u32; 2],
        scale: u32,
        rgba: Arc<[u8]>,
    ) -> Self {
        Self {
            revision,
            origin,
            size,
            scale,
            rgba,
        }
    }
    /// The panel image's size, `size / scale`.
    pub fn image_size(&self) -> [u32; 2] {
        self.size.map(|v| v / self.scale.max(1))
    }
    /// Refuse a panel the passes cannot draw on a `target`-sized stream:
    /// every format's blocks must lie wholly inside or outside it.
    pub fn validate(&self, target: [u32; 2]) -> Result<()> {
        ensure!(self.scale >= 1, "couch menu overlay: scale 0");
        ensure!(
            self.size
                .iter()
                .all(|&v| v > 0 && v.is_multiple_of(self.scale)),
            "couch menu overlay: size {:?} is not a nonzero multiple of scale {}",
            self.size,
            self.scale
        );
        ensure!(
            self.origin
                .iter()
                .chain(&self.size)
                .all(|v| v.is_multiple_of(ALIGN)),
            "couch menu overlay: origin {:?} or size {:?} not aligned to {ALIGN}",
            self.origin,
            self.size
        );
        ensure!(
            (0..2).all(
                |i| u64::from(self.origin[i]) + u64::from(self.size[i]) <= u64::from(target[i])
            ),
            "couch menu overlay: {:?} at {:?} does not fit the {target:?} stream",
            self.size,
            self.origin
        );
        let [width, height] = self.image_size();
        ensure!(
            self.rgba.len() as u64 == u64::from(width) * u64::from(height) * 4,
            "couch menu overlay: {} bytes for a {width}x{height} image",
            self.rgba.len()
        );
        Ok(())
    }
}
