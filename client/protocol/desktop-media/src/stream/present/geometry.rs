//! Picture viewport scaling, alignment-padding crop helpers, and 1:1 plane-pass checks.

use sdl3_sys::everything::SDL_FRect;

use super::Decoded;

/// A decoded H.264, HEVC or AV1 picture, which may carry an encoder's alignment padding; not a
/// PyroWave or packed 4:4:4 one, whose size the setup gives.
pub(super) fn plain(decoded: &Decoded) -> bool {
    decoded.setup.chroma_packing == 0 && decoded.setup.format & nightjar_core::sdp::VIDEO_FORMAT_MASK_PYROWAVE == 0
        && decoded.picture.frame().is_some()
}

/// The `visible` part of a `texture`-sized texture to show, when it is less than the texture;
/// None when the whole texture is, or the part would reach past it.
pub(super) fn visible_part(visible: (i32, i32), texture: (i32, i32)) -> Option<(i32, i32)> {
    (visible != texture && visible.0 <= texture.0 && visible.1 <= texture.1).then_some(visible)
}

/// Whether `picture` lands on `destination` one texel to one pixel, the only scale at which the
/// plane pass shows what SDL's draw of the video texture shows.
///
/// The texture SDL draws holds the picture converted to linear light (`pyrowave.hlsl`'s
/// `linearRgb`, `planes_direct.hlsl`'s `directFinish`), so SDL's bilinear taps when it scales are
/// in linear light. The plane pass samples the picture's Y'CbCr codes and converts afterwards, so
/// its taps are in the codes: filtering there and then converting darkens detail on an upscaled
/// picture and brightens it on a downscaled one, where converting first and then filtering in
/// linear light leaves the levels alone (issue 120: a PyroWave stream at another resolution than
/// the display came out dark or bright). Only an unscaled picture is drawn by the plane pass
/// (`direct_planes` on Windows, `planes_gpu`'s `planesDirect` elsewhere); anything else goes
/// through the texture.
pub fn draws_one_to_one(destination: &SDL_FRect, picture: (i32, i32)) -> bool {
    destination.w == picture.0 as f32
        && destination.h == picture.1 as f32
        && destination.x.fract() == 0.0
        && destination.y.fract() == 0.0
}

/// The shift of a picture drawn one texel to one pixel, rounded to whole picture pixels:
/// `packed444.hlsl` rounds it the same way at its own size. A fractional shift samples between
/// texels, which the plane pass filters in the codes rather than in the converted picture.
pub fn whole_pixel_shift(shift: [f32; 2], picture: (i32, i32)) -> [f32; 2] {
    let (width, height) = (picture.0 as f32, picture.1 as f32);
    if !(width > 0.0 && height > 0.0 && shift[0].is_finite() && shift[1].is_finite()) {
        return [0.0; 2];
    }
    [(shift[0] * width).round() / width, (shift[1] * height).round() / height]
}

/// The largest rectangle of the picture's aspect ratio inside `area`, centred.
#[cfg(test)]
pub fn letterbox(picture: (i32, i32), area: (i32, i32)) -> SDL_FRect {
    viewport(picture, area, crate::config::VideoScaling::Fit, false)
}

pub fn viewport(picture: (i32, i32), area: (i32, i32), scaling: crate::config::VideoScaling, top: bool) -> SDL_FRect {
    use crate::config::VideoScaling;
    let (aw, ah) = (area.0 as f32, area.1 as f32);
    if picture.0 <= 0 || picture.1 <= 0 || area.0 <= 0 || area.1 <= 0 {
        return SDL_FRect { x: 0.0, y: 0.0, w: aw.max(1.0), h: ah.max(1.0) };
    }
    let (pw, ph) = (picture.0 as f32, picture.1 as f32);
    let (w, h) = match scaling {
        VideoScaling::Stretch => (aw, ah),
        VideoScaling::Native => (pw, ph),
        VideoScaling::Fit | VideoScaling::Fill => {
            let scale = if scaling == VideoScaling::Fit { (aw / pw).min(ah / ph) } else { (aw / pw).max(ah / ph) };
            ((pw * scale).round(), (ph * scale).round())
        }
    };
    SDL_FRect {
        x: ((aw - w) / 2.0).trunc(),
        y: if top && matches!(scaling, VideoScaling::Fit | VideoScaling::Native) { 0.0 } else { ((ah - h) / 2.0).trunc() },
        w,
        h,
    }
}
