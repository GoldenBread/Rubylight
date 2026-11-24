//! Moving pictures between system memory and VA surfaces: the CPU encode path uploads a BGRA
//! picture, and reading a captured picture back downloads one. Both go through the surface's
//! derived image, which maps the surface's own memory.
use super::{
    display::{Va, check},
    ffi::*,
};
use anyhow::{Result, bail};
use std::ptr;

impl Va {
    /// Maps `surface` (a 32-bit RGB surface) and runs `visit` with its first plane's base
    /// pointer and pitch in bytes.
    fn with_rgb_image<T>(
        &self,
        surface: VASurfaceID,
        visit: impl FnOnce(*mut u8, usize) -> T,
    ) -> Result<T> {
        // SAFETY: the image describes the surface's own memory and is destroyed before return;
        // `visit` only touches rows inside the mapped buffer (callers bound their rows by the
        // surface's height and the pitch it is given).
        unsafe {
            let mut image: VAImage = std::mem::zeroed();
            check(
                self.dpy,
                vaDeriveImage(self.dpy, surface, &mut image),
                "vaDeriveImage",
            )?;
            let mut mapped = ptr::null_mut();
            let status = vaMapBuffer(self.dpy, image.buf, &mut mapped);
            if let Err(error) = check(self.dpy, status, "vaMapBuffer(image)") {
                vaDestroyImage(self.dpy, image.image_id);
                return Err(error);
            }
            let result = visit(
                mapped.cast::<u8>().add(image.offsets[0] as usize),
                image.pitches[0] as usize,
            );
            vaUnmapBuffer(self.dpy, image.buf);
            vaDestroyImage(self.dpy, image.image_id);
            Ok(result)
        }
    }

    /// Writes `height` rows of `width` BGRA pixels (`stride` bytes apart) into a BGRX surface.
    pub fn upload_bgra(
        &self,
        surface: VASurfaceID,
        width: u32,
        height: u32,
        stride: usize,
        bytes: &[u8],
    ) -> Result<()> {
        let row = width as usize * 4;
        if stride < row || bytes.len() < stride * (height as usize - 1) + row {
            bail!(
                "a {width}x{height} picture needs {stride}-byte rows; got {} bytes",
                bytes.len()
            );
        }
        self.with_rgb_image(surface, |base, pitch| {
            for y in 0..height as usize {
                // SAFETY: the source row is inside `bytes` (checked above) and the target row
                // inside the mapped surface, which is at least `row` bytes wide.
                unsafe {
                    ptr::copy_nonoverlapping(
                        bytes.as_ptr().add(y * stride),
                        base.add(y * pitch),
                        row,
                    );
                }
            }
        })
    }

    /// Reads `height` rows of `width` pixels out of a BGRX surface into `out`, `width * 4`
    /// bytes a row.
    pub fn download_bgra(
        &self,
        surface: VASurfaceID,
        width: u32,
        height: u32,
        out: &mut Vec<u8>,
    ) -> Result<()> {
        let row = width as usize * 4;
        out.clear();
        out.resize(row * height as usize, 0);
        self.sync(surface)?;
        self.with_rgb_image(surface, |base, pitch| {
            for y in 0..height as usize {
                // SAFETY: as in `upload_bgra`, in the other direction.
                unsafe {
                    ptr::copy_nonoverlapping(
                        base.add(y * pitch),
                        out.as_mut_ptr().add(y * row),
                        row,
                    );
                }
            }
        })
    }
}
