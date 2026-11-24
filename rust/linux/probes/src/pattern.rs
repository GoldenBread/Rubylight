//! Test pictures: a loop of frames with fine detail, flat areas, sharp edges
//! and steady motion, so an encoder has real prediction and residual work to
//! do. The loop is seamless: its last frame leads into its first.

/// One 4:2:0 picture in two planes: luma, then interleaved Cb and Cr at half
/// size. `bytes_per_sample` is 1 (NV12) or 2 (P010, value in the high bits).
pub struct Picture {
    pub luma: Vec<u8>,
    pub chroma: Vec<u8>,
}

/// Frames in the loop. The scene moves `SHIFT` pixels a frame over a texture
/// that repeats every `FRAMES * SHIFT` pixels.
pub const FRAMES: usize = 16;
const SHIFT: usize = 8;
const PERIOD: usize = FRAMES * SHIFT;

/// A small deterministic hash: the same texture on every run.
fn hash(x: u32, y: u32) -> u32 {
    let mut v = x.wrapping_mul(0x9E37_79B1) ^ y.wrapping_mul(0x85EB_CA77);
    v ^= v >> 15;
    v = v.wrapping_mul(0x2C1B_3C6D);
    v ^= v >> 12;
    v = v.wrapping_mul(0x297A_2D39);
    v ^ (v >> 15)
}

/// Luma of the still scene at a texture position, 0..=255.
fn scene(x: usize, y: usize, height: usize) -> u32 {
    let band = y * 4 / height.max(1);
    match band {
        // Per-pixel noise over a gradient: grass, foliage, film grain.
        0 => {
            let tx = x % PERIOD;
            64 + (hash(tx as u32, y as u32) & 0x7f) / 2 + (tx as u32) / 2
        }
        // Blocks of 8 pixels with hard edges: user interface and text.
        1 => {
            let cell = hash(((x % PERIOD) / 8) as u32, (y / 8) as u32);
            if cell & 3 == 0 {
                235
            } else {
                16 + (cell >> 8 & 0x3f)
            }
        }
        // Smooth waves: sky, walls, out-of-focus backgrounds.
        2 => {
            let phase = (x % PERIOD) as f32 / PERIOD as f32 * std::f32::consts::TAU;
            let wave = (phase.sin() + (y as f32 / 37.0).cos()) * 50.0 + 128.0;
            wave.clamp(16.0, 235.0) as u32
        }
        // Diagonal stripes two pixels wide: fences, fabric, moiré.
        _ => {
            if (((x % PERIOD) + y) / 2).is_multiple_of(2) {
                200
            } else {
                40
            }
        }
    }
}

pub fn generate(width: u32, height: u32, depth: u8) -> Vec<Picture> {
    let (w, h) = (width as usize, height as usize);
    let wide = depth > 8;
    let bytes = if wide { 2 } else { 1 };
    let put = |plane: &mut Vec<u8>, value: u32| {
        if wide {
            // 10 significant bits in the high bits of a 16-bit sample.
            plane.extend_from_slice(&((value << 8) as u16).to_le_bytes());
        } else {
            plane.push(value as u8);
        }
    };
    (0..FRAMES)
        .map(|frame| {
            let shift = frame * SHIFT;
            let mut luma = Vec::with_capacity(w * h * bytes);
            for y in 0..h {
                for x in 0..w {
                    put(&mut luma, scene(x + shift, y, h));
                }
            }
            let mut chroma = Vec::with_capacity(w * h / 2 * bytes);
            for y in 0..h / 2 {
                for x in 0..w / 2 {
                    // Slow colour drift that moves with the scene.
                    let tx = (x * 2 + shift) % PERIOD;
                    put(&mut chroma, 96 + (tx as u32) / 2);
                    put(&mut chroma, 160 - ((y * 2 * 64 / h.max(1)) as u32));
                }
            }
            Picture { luma, chroma }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_loop_is_seamless() {
        // One more step after the last frame lands on the first frame.
        for y in [0, 100, 200, 300] {
            for x in 0..64 {
                assert_eq!(scene(x + FRAMES * SHIFT, y, 400), scene(x, y, 400));
            }
        }
    }

    #[test]
    fn planes_have_the_size_of_their_format() {
        let eight = generate(64, 32, 8);
        assert_eq!(eight.len(), FRAMES);
        assert_eq!(eight[0].luma.len(), 64 * 32);
        assert_eq!(eight[0].chroma.len(), 64 * 16);
        let ten = generate(64, 32, 10);
        assert_eq!(ten[0].luma.len(), 64 * 32 * 2);
        assert_eq!(ten[0].chroma.len(), 64 * 16 * 2);
    }

    #[test]
    fn frames_differ() {
        let frames = generate(64, 32, 8);
        assert_ne!(frames[0].luma, frames[1].luma);
    }
}
