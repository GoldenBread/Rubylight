//! BGRA to planar YCbCr on the CPU for the codec probe's pictures: full-range BT.709, the
//! colour the codec's GPU path produces from RGB, 4:2:0 by averaging each 2x2 block.

use super::ffi;
use crate::capture::Image;
use anyhow::{Result, ensure};

/// The planes of the last picture converted, kept between pictures.
#[derive(Default)]
pub struct Planes {
    y: Vec<u8>,
    cb: Vec<u8>,
    cr: Vec<u8>,
    chroma_width: usize,
}

/// Full-range BT.709 from 8-bit RGB, in 16.16 fixed point.
fn yuv(r: i32, g: i32, b: i32) -> (u8, u8, u8) {
    let y = (13933 * r + 46871 * g + 4732 * b + 32768) >> 16;
    let cb = 128 + ((-7509 * r - 25259 * g + 32768 * b + 32768) >> 16);
    let cr = 128 + ((32768 * r - 29763 * g - 3005 * b + 32768) >> 16);
    (
        y.clamp(0, 255) as u8,
        cb.clamp(0, 255) as u8,
        cr.clamp(0, 255) as u8,
    )
}

impl Planes {
    /// Converts `image` and describes the planes for `pyrowave_encoder_encode_cpu`; the
    /// pointers stay valid until the next call.
    pub fn fill(&mut self, image: &Image, yuv444: bool) -> Result<ffi::CpuBuffer> {
        let (w, h) = (image.width as usize, image.height as usize);
        ensure!(
            image.stride >= w * 4
                && image.bytes.len() >= image.stride * (h.saturating_sub(1)) + w * 4,
            "the picture's rows are shorter than its size"
        );
        let (cw, ch) = if yuv444 { (w, h) } else { (w / 2, h / 2) };
        self.y.resize(w * h, 0);
        self.cb.resize(cw * ch, 0);
        self.cr.resize(cw * ch, 0);
        self.chroma_width = cw;
        for row in 0..h {
            let src = &image.bytes[row * image.stride..][..w * 4];
            let dst = &mut self.y[row * w..][..w];
            for (px, out) in src.as_chunks::<4>().0.iter().zip(dst) {
                *out = yuv(px[2].into(), px[1].into(), px[0].into()).0;
            }
        }
        if yuv444 {
            for row in 0..h {
                let src = &image.bytes[row * image.stride..][..w * 4];
                for (x, px) in src.as_chunks::<4>().0.iter().enumerate() {
                    let (_, cb, cr) = yuv(px[2].into(), px[1].into(), px[0].into());
                    self.cb[row * cw + x] = cb;
                    self.cr[row * cw + x] = cr;
                }
            }
        } else {
            for cy in 0..ch {
                for cx in 0..cw {
                    let (mut r, mut g, mut b) = (0, 0, 0);
                    for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                        let at = (cy * 2 + dy) * image.stride + (cx * 2 + dx) * 4;
                        b += i32::from(image.bytes[at]);
                        g += i32::from(image.bytes[at + 1]);
                        r += i32::from(image.bytes[at + 2]);
                    }
                    let (_, cb, cr) = yuv((r + 2) >> 2, (g + 2) >> 2, (b + 2) >> 2);
                    self.cb[cy * cw + cx] = cb;
                    self.cr[cy * cw + cx] = cr;
                }
            }
        }
        Ok(ffi::CpuBuffer {
            data: [
                self.y.as_mut_ptr().cast(),
                self.cb.as_mut_ptr().cast(),
                self.cr.as_mut_ptr().cast(),
            ],
            row_stride_in_bytes: [w, cw, cw],
            plane_size_in_bytes: [w * h, cw * ch, cw * ch],
            width: w as i32,
            height: h as i32,
            format: if yuv444 {
                ffi::CPU_FORMAT_YUV444P
            } else {
                ffi::CPU_FORMAT_YUV420P
            },
        })
    }

    #[cfg(test)]
    pub fn planes(&self) -> [&[u8]; 3] {
        [&self.y, &self.cb, &self.cr]
    }
}
