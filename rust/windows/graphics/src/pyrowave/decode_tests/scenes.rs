//! Synthetic test scenes and PSNR/SSIM image-quality metrics for PyroWave decode tests.

use super::*;

pub(super) fn linear(v: f64) -> f64 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}
pub(super) fn pq2020(rgb: [f64; 3]) -> [f64; 3] {
    let matrix = [
        [0.627404, 0.329283, 0.043313],
        [0.069097, 0.919540, 0.011362],
        [0.016391, 0.088013, 0.895595],
    ];
    matrix.map(|row| {
        let nits = row.iter().zip(rgb).map(|(k, v)| k * v * 80.).sum::<f64>();
        let p = (nits / 10000.).clamp(0., 1.).powf(2610. / 16384.);
        ((3424. / 4096. + 2413. / 128. * p) / (1. + 2392. / 128. * p)).powf(2523. / 32.)
    })
}

pub(super) fn text_ink(x: u32, y: u32, glyph: u32) -> bool {
    let glyphs = [
        [30, 17, 17, 30, 16, 16, 16],
        [17, 17, 10, 4, 4, 4, 4],
        [30, 17, 17, 30, 20, 18, 17],
        [14, 17, 17, 17, 17, 17, 14],
        [17, 17, 17, 21, 21, 21, 10],
        [14, 17, 17, 31, 17, 17, 17],
        [17, 17, 17, 17, 17, 10, 4],
        [31, 16, 16, 30, 16, 16, 31],
    ];
    x < 5 && y < 7 && glyphs[(glyph % 8) as usize][y as usize] & (1 << (4 - x)) != 0
}

pub(super) fn chart(x: u32, y: u32, width: u32, height: u32, phase: u32) -> [f64; 3] {
    let (half_width, half_height) = (width / 2, height / 2);
    if y < half_height && x < half_width {
        let bar = ((x * 8 / half_width + phase) % 8) as usize;
        return [
            [1., 1., 1.],
            [1., 1., 0.],
            [0., 1., 1.],
            [0., 1., 0.],
            [1., 0., 1.],
            [1., 0., 0.],
            [0., 0., 1.],
            [0., 0., 0.],
        ][bar];
    }
    if y < half_height {
        let v = f64::from(x - half_width) / f64::from(half_width - 1);
        return match y * 3 / half_height {
            0 => [v; 3],
            1 => [v, 1. - v, 0.25],
            _ => [0.25, v, 1. - v],
        };
    }
    if x < half_width {
        // 5x7 PYROWAVE glyphs at one and two pixels per stroke.
        let scale = if y < height * 3 / 4 { 1 } else { 2 };
        let (tx, ty) = ((x + phase * 3) / scale, (y - half_height) / scale);
        let ink = text_ink(tx % 6, ty % 10, tx / 6);
        return if ink {
            [0.92, 0.72, 0.25]
        } else {
            [0.06, 0.10, 0.16]
        };
    }
    let mut state = x.wrapping_mul(747796405)
        ^ y.wrapping_mul(2891336453)
        ^ (phase + 1).wrapping_mul(277803737);
    std::array::from_fn(|_| {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        0.25 + f64::from(state & 65535) / 65535. * 0.5
    })
}

#[derive(Clone, Copy, Debug)]
pub(super) enum Scene {
    Chart,
    Desktop,
    Game,
    Dark,
}

pub(super) fn noise(x: u32, y: u32) -> f64 {
    let mut n = x.wrapping_mul(747796405) ^ y.wrapping_mul(2891336453);
    n ^= n >> 16;
    n = n.wrapping_mul(2246822519);
    n ^= n >> 13;
    f64::from(n & 65535) / 65535.
}

pub(super) fn scene_rgb(
    scene: Scene,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    phase: u32,
) -> [f64; 3] {
    let (u, v) = (
        f64::from(x) / f64::from(width),
        f64::from(y) / f64::from(height),
    );
    match scene {
        Scene::Chart => chart(x, y, width, height, phase),
        Scene::Desktop => {
            let scale = (height / 540).max(1);
            let (tx, ty) = (x / scale, y / scale);
            let row = ty / 16;
            let text = tx % 48 < 42 && text_ink(tx % 6, ty % 16, tx / 6 + row);
            if v < 0.06 || v > 0.94 {
                return if text && tx % 80 < 54 {
                    [0.88; 3]
                } else {
                    [0.12, 0.14, 0.18]
                };
            }
            if u < 0.18 {
                if ty % 32 < 20 && tx % 80 < 10 {
                    return [0.20, 0.55, 0.83];
                }
                return if text && u > 0.04 {
                    [0.22; 3]
                } else {
                    [0.86, 0.88, 0.91]
                };
            }
            if u < 0.69 {
                if text && tx % 230 < 90 + row * 17 % 130 {
                    return match row % 4 {
                        0 => [0.12, 0.30, 0.68],
                        1 => [0.18, 0.40, 0.23],
                        _ => [0.16; 3],
                    };
                }
                if row == 12 + phase && tx % 230 < 180 {
                    return [0.74, 0.85, 0.98];
                }
                return [0.97; 3];
            }
            if v < 0.55 {
                let line = 0.29 + 0.12 * (u * 35. + f64::from(phase) * 0.15).sin();
                return if (v - line).abs() < 0.003 {
                    [0.12, 0.62, 0.34]
                } else if tx % 24 == 0 || ty % 24 == 0 {
                    [0.76; 3]
                } else {
                    [0.93; 3]
                };
            }
            if text {
                [0.24; 3]
            } else {
                [0.87 + 0.06 * u; 3]
            }
        }
        Scene::Game => {
            let sx = x + phase * 7;
            let sy = y + phase * 3;
            if v < 0.42 {
                if (u - 0.8).powi(2) + (v - 0.18).powi(2) < 0.001 {
                    return [1., 0.98, 0.90];
                }
                let cloud = ((u * 17. + f64::from(phase) * 0.03).sin() * (v * 25.).cos()).max(0.);
                return [
                    0.18 + 0.35 * v + 0.08 * cloud,
                    0.34 + 0.40 * v + 0.08 * cloud,
                    0.63 + 0.22 * v,
                ];
            }
            let coarse = noise(sx / 16, sy / 16);
            let fine = noise(sx, sy);
            let stone = 0.30 + 0.16 * noise(sx / 3, sy / 3) + 0.05 * fine;
            let mut rgb = if (u - 0.52).abs() < (v - 0.30) * 0.29 {
                [stone * 1.05, stone, stone * 0.89]
            } else {
                let leaf = 0.10 + 0.26 * coarse + 0.15 * fine;
                [0.08 + leaf * 0.45, leaf, 0.05 + leaf * 0.24]
            };
            if (0.22..0.38).contains(&u) && (0.48..0.74).contains(&v) {
                let brick = if sy % 18 < 2 || (sx + sy / 18 * 13) % 38 < 2 {
                    0.25
                } else {
                    0.58 + 0.06 * fine
                };
                rgb = [brick, brick * 0.55, brick * 0.32];
            }
            if (u - 0.5).abs() < 0.0015 && (v - 0.5).abs() < 0.015
                || (v - 0.5).abs() < 0.002 && (u - 0.5).abs() < 0.009
            {
                rgb = [0.95; 3];
            }
            if (0.04..0.25).contains(&u) && (0.88..0.90).contains(&v) {
                rgb = [0.75, 0.08, 0.05];
            }
            rgb
        }
        Scene::Dark => {
            let light = (1. - ((u - 0.65).powi(2) + (v - 0.36).powi(2)).sqrt() * 1.8).max(0.);
            let texture = noise(x + phase * 3, y) * 0.012;
            let edge = if (0.24..0.42).contains(&u) && v > 0.25 {
                0.018
            } else {
                0.
            };
            let value = 0.018 + 0.075 * light + texture + edge;
            [value * 0.90, value, value * 1.12]
        }
    }
}

pub(super) fn picture(config: &Negotiated, phase: u32, scene: Scene) -> (Image, Vec<Vec<u16>>) {
    let (width, height) = (config.width, config.height);
    let pixel = if !config.ten_bit() {
        Pixel::Bgra8
    } else if config.hdr && phase == 1 {
        Pixel::Rgba10Pq
    } else {
        Pixel::RgbaF16
    };
    let stride = width as usize * if pixel == Pixel::RgbaF16 { 8 } else { 4 };
    let mut bytes = Vec::with_capacity(stride * height as usize);
    let mut source = Vec::with_capacity((width * height) as usize);
    for y in 0..height {
        for x in 0..width {
            let rgb = scene_rgb(scene, x, y, width, height, phase);
            let rgb = if pixel == Pixel::Bgra8 {
                let codes = rgb.map(|v| (v * 255.).round() as u8);
                bytes.extend_from_slice(&[codes[2], codes[1], codes[0], 255]);
                codes.map(|v| f64::from(v) / 255.)
            } else {
                let white = if !config.hdr {
                    1.25
                } else {
                    match scene {
                        Scene::Desktop => 2.5,
                        Scene::Game if y >= height * 42 / 100 || rgb[0] < 1. => 2.5,
                        _ => 12.5,
                    }
                };
                let sc = rgb.map(|v| linear(v) * white);
                if pixel == Pixel::Rgba10Pq {
                    let codes = pq2020(sc).map(|v| (v * 1023.).round() as u32);
                    bytes.extend_from_slice(
                        &(codes[0] | (codes[1] << 10) | (codes[2] << 20) | (3 << 30)).to_le_bytes(),
                    );
                    codes.map(|v| f64::from(v) / 1023.)
                } else {
                    let sc = sc.map(half::f16::from_f64);
                    for v in sc {
                        bytes.extend_from_slice(&v.to_le_bytes());
                    }
                    bytes.extend_from_slice(&half::f16::ONE.to_le_bytes());
                    let sc = sc.map(half::f16::to_f64);
                    if config.hdr {
                        pq2020(sc)
                    } else {
                        sc.map(|v| {
                            let v = (v / 1.25).clamp(0., 1.);
                            if v <= 0.0031308 {
                                12.92 * v
                            } else {
                                1.055 * v.powf(1. / 2.4) - 0.055
                            }
                        })
                    }
                }
            };
            source.push(rgb);
        }
    }
    let peak = if config.ten_bit() { 1023. } else { 255. };
    let k = if config.hdr {
        [0.2627, 0.6780, 0.0593]
    } else {
        [0.2126, 0.7152, 0.0722]
    };
    let yuv = |rgb: [f64; 3]| {
        let y = k.iter().zip(rgb).map(|(k, v)| k * v).sum::<f64>();
        [
            y,
            (rgb[2] - y) / (2. * (1. - k[2])),
            (rgb[0] - y) / (2. * (1. - k[0])),
        ]
    };
    let mut planes = vec![Vec::new(), Vec::new(), Vec::new()];
    for rgb in &source {
        planes[0].push((yuv(*rgb)[0] * peak).round().clamp(0., peak) as u16);
    }
    let divisor = if config.yuv444 { 1 } else { 2 };
    // PQ 4:2:0 averages its 2x2 block in light and codes the mean, as
    // `pyroBlock` in color.hlsl does; SDR averages its codes.
    let filter_in_light = config.hdr && divisor == 2;
    for y in (0..height).step_by(divisor) {
        for x in (0..width).step_by(divisor) {
            let mut rgb = [0.; 3];
            for dy in 0..divisor {
                for dx in 0..divisor {
                    let sample = source[(y as usize + dy) * width as usize + x as usize + dx];
                    for c in 0..3 {
                        let value = if filter_in_light {
                            butterpollo_media_policy::image_quality::depq(sample[c])
                        } else {
                            sample[c]
                        };
                        rgb[c] += value / (divisor * divisor) as f64;
                    }
                }
            }
            if filter_in_light {
                rgb = rgb.map(butterpollo_media_policy::image_quality::pq);
            }
            let values = yuv(rgb);
            for c in 1..3 {
                planes[c].push(
                    ((peak + 1.) / 2. + peak * values[c])
                        .round()
                        .clamp(0., peak) as u16,
                );
            }
        }
    }
    (
        Image {
            width,
            height,
            stride,
            bytes,
            captured: Instant::now(),
            pixel,
        },
        planes,
    )
}

#[derive(serde::Serialize)]
pub(super) struct Error {
    pub(super) psnr: f64,
    pub(super) max: f64,
}
pub(super) fn compare(
    decoded: &[u16],
    expected: &[u16],
    ten_bit: bool,
    width: usize,
) -> [Error; 5] {
    assert_eq!(decoded.len(), expected.len());
    let peak = if ten_bit { 1023. } else { 255. };
    let scale = if ten_bit { peak / 65535. } else { 1. };
    let mut sums = [(0., 0_f64, 0); 5];
    for (index, (&actual, &expected)) in decoded.iter().zip(expected).enumerate() {
        let error = (f64::from(actual) * scale - f64::from(expected)).abs();
        let panel = 1
            + usize::from(index % width >= width / 2)
            + 2 * usize::from(index >= decoded.len() / 2);
        for at in [0, panel] {
            sums[at].0 += error * error;
            sums[at].1 = sums[at].1.max(error);
            sums[at].2 += 1;
        }
    }
    sums.map(|(squares, maximum, count)| Error {
        psnr: if squares == 0. {
            f64::INFINITY
        } else {
            10. * (peak * peak * count as f64 / squares).log10()
        },
        max: maximum,
    })
}

pub(super) fn ssim(decoded: &[u16], expected: &[u16], ten_bit: bool, width: usize) -> f64 {
    let peak = if ten_bit { 1023. } else { 255. };
    let storage = if ten_bit { 65535. } else { 255. };
    let height = decoded.len() / width;
    let (mut total, mut windows) = (0., 0);
    // Uniform, non-overlapping 8x8 windows keep this diagnostic inexpensive.
    for y in (0..height).step_by(8) {
        for x in (0..width).step_by(8) {
            let (mut a, mut b, mut aa, mut bb, mut ab, mut count) = (0., 0., 0., 0., 0., 0.);
            for dy in y..(y + 8).min(height) {
                for dx in x..(x + 8).min(width) {
                    let i = dy * width + dx;
                    let av = f64::from(decoded[i]) / storage;
                    let bv = f64::from(expected[i]) / peak;
                    a += av;
                    b += bv;
                    aa += av * av;
                    bb += bv * bv;
                    ab += av * bv;
                    count += 1.;
                }
            }
            a /= count;
            b /= count;
            let va = (aa / count - a * a).max(0.);
            let vb = (bb / count - b * b).max(0.);
            let cov = ab / count - a * b;
            total += (2. * a * b + 0.0001) * (2. * cov + 0.0009)
                / ((a * a + b * b + 0.0001) * (va + vb + 0.0009));
            windows += 1;
        }
    }
    total / f64::from(windows)
}

#[test]
fn quality_metrics() {
    let reference: Vec<_> = (0..256).collect();
    assert!(
        compare(&reference, &reference, false, 16)[0]
            .psnr
            .is_infinite()
    );
    assert!((ssim(&reference, &reference, false, 16) - 1.).abs() < 1e-12);
    let black = vec![0; 81];
    let white = vec![255; 81];
    assert_eq!(compare(&black, &white, false, 9)[0].psnr, 0.);
    assert!((ssim(&black, &white, false, 9) - 0.0001 / 1.0001).abs() < 1e-12);
    assert!((ssim(&[65535; 81], &[1023; 81], true, 9) - 1.).abs() < 1e-12);
    let inverse: Vec<_> = reference.iter().map(|v| 255 - v).collect();
    assert!(ssim(&inverse, &reference, false, 16) < 0.);
}
