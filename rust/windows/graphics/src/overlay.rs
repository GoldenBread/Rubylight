//! The couch menu's panel as the converters draw it into the stream
//! (`docs/couch-menu-design.md` sections 6.2 and 6.3). While an overlay is
//! set, each conversion runs a second pass over the panel's blocks only, its
//! colours read from the panel image instead of the capture; closed, the
//! conversions record exactly what they did before.
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
    /// `cbuffer Menu` (b2, beside `Config` at b0) in `shaders/color.hlsl`:
    /// origin, size, the first block and the block count for `block`-sized
    /// threads (2x2, or 2x1 for packing version 1), and the scale.
    pub fn constants(&self, block: [u32; 2]) -> [u32; 12] {
        [
            self.origin[0],
            self.origin[1],
            self.size[0],
            self.size[1],
            self.origin[0] / block[0],
            self.origin[1] / block[1],
            self.size[0] / block[0],
            self.size[1] / block[1],
            self.scale,
            0,
            0,
            0,
        ]
    }
}

/// What the passes should write for a panel, on the CPU, for the tests.
#[cfg(test)]
#[allow(
    dead_code,
    reason = "the overlay pass tests use it once the pass lands"
)]
pub(crate) mod reference {
    use super::Overlay;

    /// The colour fields of `cbuffer Config`.
    #[derive(Clone, Copy, Debug)]
    pub(crate) struct Colour {
        hdr: bool,
        /// `hdr == 2`: color.hlsl's `hlg` instead of PQ.
        hlg: bool,
        matrix: u32,
        full_range: bool,
        ten_bit: bool,
        white: f64,
    }
    impl Colour {
        pub(crate) fn from_values(values: &[u32; 20]) -> Self {
            Self {
                hdr: values[5] != 0,
                hlg: values[5] == 2,
                matrix: values[6],
                full_range: values[7] != 0,
                ten_bit: values[8] != 0,
                white: f64::from(f32::from_bits(values[9])),
            }
        }
        fn maximum(self) -> f64 {
            if self.ten_bit { 1023. } else { 255. }
        }
        fn weights(self) -> [f64; 3] {
            match self.matrix {
                2 => [0.2627, 0.6780, 0.0593],
                0 => [0.299, 0.587, 0.114],
                _ => [0.2126, 0.7152, 0.0722],
            }
        }
        /// `menu_rgb`'s transform of a panel pixel.
        fn transform(self, rgb: [f64; 3]) -> [f64; 3] {
            let linear = || gamut2020(rgb.map(srgb_linear));
            if self.hlg {
                crate::color::hlg(linear().map(|v| v * self.white))
            } else if self.hdr {
                linear().map(|v| pq(v * self.white / 125.))
            } else if self.matrix == 2 {
                linear().map(bt2020_sdr)
            } else {
                rgb
            }
        }
        /// `chromaLight`: display light for HDR codes, SDR's codes as they are.
        fn light(self, rgb: [f64; 3]) -> [f64; 3] {
            if self.hlg {
                crate::color::hlg_light(rgb)
            } else if self.hdr {
                rgb.map(depq)
            } else {
                rgb
            }
        }
        /// `chromaCodes`, the inverse of `light`.
        fn codes(self, light: [f64; 3]) -> [f64; 3] {
            if self.hlg {
                crate::color::hlg(light)
            } else if self.hdr {
                light.map(pq)
            } else {
                light
            }
        }
        /// The chroma filter's weighted mean, in light for HDR (`leftChroma`, `pyroBlock`).
        fn filtered(self, samples: &[[f64; 3]], weights: &[f64]) -> [f64; 3] {
            let light: Vec<[f64; 3]> = samples.iter().map(|&s| self.light(s)).collect();
            self.codes(average(&light, weights))
        }
        fn luma(self, rgb: [f64; 3]) -> u16 {
            let [kr, kg, kb] = self.weights();
            let y = kr * rgb[0] + kg * rgb[1] + kb * rgb[2];
            let code = match (self.ten_bit, self.full_range) {
                (true, true) => 1023. * y,
                (true, false) => 64. + 876. * y,
                (false, true) => 255. * y,
                (false, false) => 16. + 219. * y,
            };
            (code.clamp(0., self.maximum()) + 0.5).floor() as u16
        }
        fn chroma(self, rgb: [f64; 3]) -> [u16; 2] {
            let [kr, kg, kb] = self.weights();
            let y = kr * rgb[0] + kg * rgb[1] + kb * rgb[2];
            let uv = [
                (rgb[2] - y) / (2. * (1. - kb)),
                (rgb[0] - y) / (2. * (1. - kr)),
            ];
            let (centre, range) = match (self.ten_bit, self.full_range) {
                (true, true) => (512., 1023.),
                (true, false) => (512., 896.),
                (false, true) => (128., 255.),
                (false, false) => (128., 224.),
            };
            uv.map(|v| ((centre + range * v).clamp(0., self.maximum()) + 0.5).floor() as u16)
        }
        /// Packing version 2's signed half-difference.
        fn detail(self, value: [u16; 2], base: [u16; 2]) -> [u16; 2] {
            let centre = if self.ten_bit { 512. } else { 128. };
            [0, 1].map(|i| {
                ((f64::from(value[i]) - f64::from(base[i])) * 0.5 + centre + 0.5)
                    .floor()
                    .clamp(0., self.maximum()) as u16
            })
        }
    }
    fn srgb_linear(v: f64) -> f64 {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    }
    fn bt2020_sdr(v: f64) -> f64 {
        let v = v.clamp(0., 1.);
        if v <= 0.018 {
            4.5 * v
        } else {
            1.099 * v.powf(0.45) - 0.099
        }
    }
    fn gamut2020(rgb: [f64; 3]) -> [f64; 3] {
        let dot = |m: [f64; 3]| m[0] * rgb[0] + m[1] * rgb[1] + m[2] * rgb[2];
        [
            dot([0.627404, 0.329283, 0.043313]),
            dot([0.069097, 0.919540, 0.011362]),
            dot([0.016391, 0.088013, 0.895595]),
        ]
    }
    fn pq(v: f64) -> f64 {
        let power = v.clamp(0., 1.).powf(2610. / 16384.);
        ((3424. / 4096. + 2413. / 128. * power) / (1. + 2392. / 128. * power)).powf(2523. / 32.)
    }
    fn depq(v: f64) -> f64 {
        let p = v.clamp(0., 1.).powf(32. / 2523.);
        ((p - 3424. / 4096.).max(0.) / (2413. / 128. - 2392. / 128. * p).max(1e-6))
            .powf(16384. / 2610.)
    }
    fn average(samples: &[[f64; 3]], weights: &[f64]) -> [f64; 3] {
        let total: f64 = weights.iter().sum();
        [0, 1, 2].map(|c| {
            samples
                .iter()
                .zip(weights)
                .map(|(s, w)| s[c] * w)
                .sum::<f64>()
                / total
        })
    }

    /// The planes a conversion writes, as the tests read them back.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum Layout {
        /// NV12 or P010 as one plane: luma rows, then interleaved chroma rows.
        Yuv420,
        /// Packed 4:4:4 (`chroma_pack_cs`) in the same one-plane form.
        Pack(u32),
        /// PyroWave's three planes, chroma halved unless 4:4:4.
        Pyro { yuv444: bool },
    }

    /// Expected value of a stored output sample after the menu pass.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum Sample {
        /// Outside the panel or `alpha == 0`: must match the closed capture bit-for-bit.
        Capture,
        /// Inside the panel with all contributing taps at `alpha == 255`: must match `code ± 1`.
        Panel(u16),
        /// Single-tap pixel at `0 < alpha < 255`: must lie between capture and `code` (`±2`).
        BlendedSingle(u16),
        /// Multi-tap chroma/detail block straddling a partial-alpha corner boundary.
        MixedBlock,
    }

    /// The panel's premultiplied colour and alpha at a target pixel, clamped
    /// into the panel as `menu_texel` / `menu_rgb` do.
    pub(crate) struct Panel<'a> {
        overlay: &'a Overlay,
        colour: Colour,
    }
    impl<'a> Panel<'a> {
        pub(crate) fn new(overlay: &'a Overlay, colour: Colour) -> Self {
            Self { overlay, colour }
        }
        fn inside(&self, [x, y]: [u32; 2]) -> bool {
            let o = self.overlay;
            (o.origin[0]..o.origin[0] + o.size[0]).contains(&x)
                && (o.origin[1]..o.origin[1] + o.size[1]).contains(&y)
        }
        fn texel(&self, [x, y]: [u32; 2]) -> [u8; 4] {
            let o = self.overlay;
            let [width, height] = o.image_size();
            let local = |v: u32, origin: u32, limit: u32| {
                (v.saturating_sub(origin) / o.scale).min(limit - 1) as usize
            };
            let index =
                (local(y, o.origin[1], height) * width as usize + local(x, o.origin[0], width)) * 4;
            [
                o.rgba[index],
                o.rgba[index + 1],
                o.rgba[index + 2],
                o.rgba[index + 3],
            ]
        }
        fn classify(
            &self,
            inside_panel: bool,
            taps: &[[u32; 2]],
            straight_code: impl FnOnce() -> u16,
        ) -> Sample {
            if !inside_panel || taps.iter().all(|&p| self.texel(p)[3] == 0) {
                Sample::Capture
            } else if taps.iter().all(|&p| self.texel(p)[3] == 255) {
                Sample::Panel(straight_code())
            } else if taps.len() == 1 {
                Sample::BlendedSingle(straight_code())
            } else {
                Sample::MixedBlock
            }
        }
        fn at(&self, p: [u32; 2]) -> [f64; 3] {
            let [r, g, b, a] = self.texel(p);
            let denom = f64::from(a.max(1));
            let rgb = [r, g, b].map(|c| (f64::from(c) / denom).clamp(0., 1.));
            self.colour.transform(rgb)
        }
        /// Blends a premultiplied panel texel over a nonlinear capture signal,
        /// matching `menu_rgb` in `color.hlsl`.
        pub(crate) fn blend(&self, p: [u32; 2], capture: [f64; 3]) -> [f64; 3] {
            if !self.inside(p) {
                return capture;
            }
            let a = self.texel(p)[3];
            if a == 0 {
                return capture;
            }
            let panel = self.at(p);
            if a == 255 {
                return panel;
            }
            let t = f64::from(a) / 255.;
            [0, 1, 2].map(|c| capture[c] + (panel[c] - capture[c]) * t)
        }
    }

    /// For each plane, every stored sample in order: [`Sample::Panel`] where all
    /// taps are opaque, [`Sample::Capture`] outside or where all taps have
    /// `alpha == 0`, and [`Sample::Blended`] along premultiplied rounded-corner arcs.
    pub(crate) fn expected(
        overlay: &Overlay,
        colour: Colour,
        layout: Layout,
        [width, height]: [u32; 2],
    ) -> Vec<Vec<Sample>> {
        let panel = Panel::new(overlay, colour);
        let luma = |p: [u32; 2]| panel.classify(panel.inside(p), &[p], || colour.luma(panel.at(p)));
        match layout {
            Layout::Yuv420 => {
                let mut plane: Vec<_> = (0..height)
                    .flat_map(|y| (0..width).map(move |x| [x, y]))
                    .map(luma)
                    .collect();
                for y in 0..height / 2 {
                    for x in 0..width / 2 {
                        let at = [x * 2, y * 2];
                        let left = at[0].saturating_sub(1);
                        let taps = [
                            [left, at[1]],
                            [left, at[1] + 1],
                            at,
                            [at[0], at[1] + 1],
                            [at[0] + 1, at[1]],
                            [at[0] + 1, at[1] + 1],
                        ];
                        let uv_pair = || {
                            let samples = taps.map(|p| panel.at(p));
                            colour.chroma(colour.filtered(&samples, &[1., 1., 2., 2., 1., 1.]))
                        };
                        let pair =
                            if !panel.inside(at) || taps.iter().all(|&p| panel.texel(p)[3] == 0) {
                                [Sample::Capture, Sample::Capture]
                            } else if taps.iter().all(|&p| panel.texel(p)[3] == 255) {
                                let uv = uv_pair();
                                [Sample::Panel(uv[0]), Sample::Panel(uv[1])]
                            } else {
                                [Sample::MixedBlock, Sample::MixedBlock]
                            };
                        plane.extend(pair);
                    }
                }
                vec![plane]
            }
            Layout::Pack(version) => {
                let size = [(width + 63) & !63, (height + 7) & !7];
                let half = size[0] / 2;
                let in_panel = |p: [u32; 2]| p[0] < width && p[1] < height && panel.inside(p);
                let chroma = |p: [u32; 2]| colour.chroma(panel.at(p));
                let mut plane = Vec::new();
                for y in 0..size[1] * 2 {
                    for x in 0..size[0] {
                        plane.push(if y < size[1] {
                            panel.classify(in_panel([x, y]), &[[x, y]], || {
                                colour.luma(panel.at([x, y]))
                            })
                        } else {
                            let pixel = [(x % half) * 2 + 1, y - size[1]];
                            let anchor = [pixel[0] & !1, pixel[1] & !1];
                            let code = || {
                                let mut uv = chroma(pixel);
                                if version == 2 {
                                    uv = colour.detail(uv, chroma(anchor));
                                }
                                uv[usize::from(x >= half)]
                            };
                            if version == 2 {
                                panel.classify(in_panel(pixel), &[pixel, anchor], code)
                            } else {
                                panel.classify(in_panel(pixel), &[pixel], code)
                            }
                        });
                    }
                }
                for y in 0..size[1] {
                    for x in 0..half {
                        let pair = if version == 2 {
                            let at = [x * 2, (y % (size[1] / 2)) * 2];
                            let below = [at[0], at[1] + 1];
                            if y < size[1] / 2 {
                                let [u, v] = chroma(at);
                                [
                                    panel.classify(in_panel(at), &[at], || u),
                                    panel.classify(in_panel(at), &[at], || v),
                                ]
                            } else {
                                let [u, v] = colour.detail(chroma(below), chroma(at));
                                [
                                    panel.classify(in_panel(at), &[at, below], || u),
                                    panel.classify(in_panel(at), &[at, below], || v),
                                ]
                            }
                        } else {
                            let at = [x * 2, y];
                            let [u, v] = chroma(at);
                            [
                                panel.classify(in_panel(at), &[at], || u),
                                panel.classify(in_panel(at), &[at], || v),
                            ]
                        };
                        plane.extend(pair);
                    }
                }
                vec![plane]
            }
            Layout::Pyro { yuv444 } => {
                let pixels: Vec<[u32; 2]> = (0..height)
                    .flat_map(|y| (0..width).map(move |x| [x, y]))
                    .collect();
                let y = pixels.iter().map(|&p| luma(p)).collect();
                let chroma: Vec<[Sample; 2]> = if yuv444 {
                    pixels
                        .iter()
                        .map(|&p| {
                            let [u, v] = colour.chroma(panel.at(p));
                            [
                                panel.classify(panel.inside(p), &[p], || u),
                                panel.classify(panel.inside(p), &[p], || v),
                            ]
                        })
                        .collect()
                } else {
                    (0..height / 2)
                        .flat_map(|y| (0..width / 2).map(move |x| [x * 2, y * 2]))
                        .map(|at| {
                            let taps = [
                                at,
                                [at[0] + 1, at[1]],
                                [at[0], at[1] + 1],
                                [at[0] + 1, at[1] + 1],
                            ];
                            let [u, v] = colour
                                .chroma(colour.filtered(&taps.map(|p| panel.at(p)), &[1.; 4]));
                            [
                                panel.classify(panel.inside(at), &taps, || u),
                                panel.classify(panel.inside(at), &taps, || v),
                            ]
                        })
                        .collect()
                };
                vec![
                    y,
                    chroma.iter().map(|uv| uv[0]).collect(),
                    chroma.iter().map(|uv| uv[1]).collect(),
                ]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlay(origin: [u32; 2], size: [u32; 2], scale: u32) -> Overlay {
        let pixels = (size[0] / scale.max(1)) * (size[1] / scale.max(1));
        Overlay {
            revision: 1,
            origin,
            size,
            scale,
            rgba: vec![255; pixels as usize * 4].into(),
        }
    }

    #[test]
    fn overlay_validation_rejects_unaligned_or_outside_panels() {
        let target = [1280, 720];
        overlay([16, 32], [576, 272], 1).validate(target).unwrap();
        overlay([0, 0], [1280, 720], 2).validate(target).unwrap();
        overlay([640, 448], [640, 272], 1).validate(target).unwrap();
        for (origin, size, scale) in [
            ([8, 32], [576, 272], 1),
            ([16, 40], [576, 272], 1),
            ([16, 32], [584, 272], 1),
            ([16, 32], [576, 280], 1),
            ([720, 32], [576, 272], 1),
            ([16, 464], [576, 272], 1),
            ([16, 32], [576, 272], 0),
            ([16, 32], [576, 272], 3),
            ([16, 32], [0, 272], 1),
        ] {
            assert!(
                overlay(origin, size, scale).validate(target).is_err(),
                "{origin:?} {size:?} x{scale}"
            );
        }
        let mut short = overlay([16, 32], [576, 272], 2);
        short.rgba = vec![255; 288 * 136 * 4 - 4].into();
        assert!(short.validate(target).is_err());
        let huge = Overlay {
            origin: [u32::MAX - 15, 0],
            ..overlay([0, 0], [16, 16], 1)
        };
        assert!(huge.validate(target).is_err());
    }

    #[test]
    fn overlay_constants_count_blocks_of_the_pass() {
        let panel = overlay([32, 48], [576, 272], 2);
        assert_eq!(
            panel.constants([2, 2]),
            [32, 48, 576, 272, 16, 24, 288, 136, 2, 0, 0, 0]
        );
        assert_eq!(
            panel.constants([2, 1]),
            [32, 48, 576, 272, 16, 48, 288, 272, 2, 0, 0, 0]
        );
    }

    #[test]
    fn rounded_corner_alpha_preserves_capture_outside_and_blends_fringe() {
        let mut rgba = vec![255u8; 16 * 16 * 4];
        // Transparent top-left corner `[0, 0]` (`alpha == 0`) and half-coverage `[1, 0]`
        // (`premultiplied rgb = [100, 50, 25], alpha = 128`).
        rgba[..4].copy_from_slice(&[0, 0, 0, 0]);
        rgba[4..8].copy_from_slice(&[100, 50, 25, 128]);
        let ov = Overlay {
            revision: 1,
            origin: [16, 16],
            size: [32, 32],
            scale: 2,
            rgba: rgba.into(),
        };
        let mut values = [0u32; 20];
        values[6] = 1; // BT.709
        values[7] = 1; // full range
        let colour = reference::Colour::from_values(&values);
        let exp = reference::expected(&ov, colour, reference::Layout::Yuv420, [64, 64]);
        // Top-left 2x2 target pixels `(16, 16)` have `alpha == 0`, `[1, 0]` at `(18, 16)` is blended,
        // and `(24, 24)` is opaque interior.
        assert_eq!(
            exp[0][16 * 64 + 16],
            reference::Sample::Capture,
            "transparent corner preserves capture luma"
        );
        assert!(
            matches!(exp[0][16 * 64 + 18], reference::Sample::BlendedSingle(_)),
            "anti-aliased fringe is classified as blended"
        );
        assert!(
            matches!(exp[0][24 * 64 + 24], reference::Sample::Panel(_)),
            "opaque interior writes panel luma"
        );
        let pyro = reference::expected(
            &ov,
            colour,
            reference::Layout::Pyro { yuv444: false },
            [64, 64],
        );
        assert_eq!(
            pyro[1][(16 / 2) * (64 / 2) + (16 / 2)],
            reference::Sample::Capture,
            "Pyro420 transparent corner preserves capture chroma"
        );

        let panel = reference::Panel::new(&ov, colour);
        let cap = [0.2, 0.4, 0.8];
        assert_eq!(
            panel.blend([16, 16], cap),
            cap,
            "alpha 0 keeps capture untouched"
        );
        let blended = panel.blend([18, 16], cap);
        let t = 128.0 / 255.0;
        let straight = [100.0 / 128.0, 50.0 / 128.0, 25.0 / 128.0];
        for c in 0..3 {
            let want = cap[c] + (straight[c] - cap[c]) * t;
            assert!(
                (blended[c] - want).abs() < 1e-6,
                "channel {c}: got {}, want {want}",
                blended[c]
            );
        }
    }
}
