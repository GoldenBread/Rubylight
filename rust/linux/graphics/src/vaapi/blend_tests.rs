//! Drawing a pointer's image into a captured frame with video processing: premultiplied alpha
//! blended over the frame, the rest of the frame kept, in the stream's NV12 and in BGRX.
use super::{
    BGRA, BGRX, NV12,
    display::Va,
    ffi::{VA_RT_FORMAT_RGB32, VA_RT_FORMAT_YUV420},
    pool::Pool,
    vpp::{Colors, Converter, Rect, Source},
};
use std::sync::Arc;

const FRAME: (u32, u32) = (256, 256);
const BLUE: [u8; 4] = [255, 0, 0, 255];
const AT: Rect = Rect {
    x: 96,
    y: 96,
    width: 64,
    height: 64,
};

/// A 64x64 pointer: its top half red at half coverage (premultiplied), its bottom half clear.
fn pointer() -> Vec<u8> {
    let mut bgra = Vec::new();
    for y in 0..64 {
        for _ in 0..64 {
            bgra.extend_from_slice(if y < 32 {
                &[0, 0, 128, 128]
            } else {
                &[0, 0, 0, 0]
            });
        }
    }
    bgra
}

fn flat(colour: [u8; 4], (width, height): (u32, u32)) -> Vec<u8> {
    colour.repeat((width * height) as usize)
}

fn pixel(bgrx: &[u8], x: u32, y: u32) -> [u8; 3] {
    let at = ((y * FRAME.0 + x) * 4) as usize;
    [bgrx[at + 2], bgrx[at + 1], bgrx[at]]
}

fn near(got: [u8; 3], want: [u8; 3], slack: u8, what: &str) {
    for (g, w) in got.iter().zip(want) {
        assert!(
            g.abs_diff(w) <= slack,
            "{what}: got RGB {got:?}, want {want:?}"
        );
    }
}

/// The frame after the pointer was drawn at [`AT`], read back as BGRX rows.
fn drawn(nv12: bool) -> Vec<u8> {
    let va = Arc::new(Va::open("/dev/dri/renderD128").unwrap());
    let converter = Converter::new(&va, FRAME.0, FRAME.1)
        .unwrap()
        .writing(Colors::BT709_LIMITED);
    let rgb = Pool::new_linear(&va, VA_RT_FORMAT_RGB32, BGRX, FRAME.0, FRAME.1, 2, 2).unwrap();
    let background = rgb.take().unwrap();
    va.upload_bgra(
        background.id(),
        FRAME.0,
        FRAME.1,
        FRAME.0 as usize * 4,
        &flat(BLUE, FRAME),
    )
    .unwrap();
    let image = Pool::new_linear(&va, VA_RT_FORMAT_RGB32, BGRA, 64, 64, 1, 1).unwrap();
    let image = image.take().unwrap();
    va.upload_bgra(image.id(), 64, 64, 256, &pointer()).unwrap();
    let from = Rect {
        x: 0,
        y: 0,
        width: 64,
        height: 64,
    };
    let out = rgb.take().unwrap();
    if nv12 {
        let video = Pool::new(&va, VA_RT_FORMAT_YUV420, NV12, FRAME.0, FRAME.1, 1, 1).unwrap();
        let video = video.take().unwrap();
        converter
            .convert(background.id(), Source::Rgb, FRAME, video.id(), FRAME)
            .unwrap();
        converter
            .blend(image.id(), from, video.id(), AT, false)
            .unwrap();
        converter.to_rgb(video.id(), FRAME, out.id()).unwrap();
    } else {
        converter
            .scale_rgb(background.id(), FRAME, out.id(), FRAME)
            .unwrap();
        converter
            .blend(image.id(), from, out.id(), AT, true)
            .unwrap();
    }
    let mut bgrx = Vec::new();
    va.download_bgra(out.id(), FRAME.0, FRAME.1, &mut bgrx)
        .unwrap();
    bgrx
}

fn check(bgrx: &[u8], slack: u8) {
    near(
        pixel(bgrx, 10, 10),
        [0, 0, 255],
        slack,
        "the frame away from the pointer",
    );
    near(
        pixel(bgrx, 200, 120),
        [0, 0, 255],
        slack,
        "the frame beside the pointer",
    );
    near(
        pixel(bgrx, 120, 104),
        [128, 0, 127],
        slack,
        "under the pointer's half-red",
    );
    near(
        pixel(bgrx, 120, 150),
        [0, 0, 255],
        slack,
        "under the pointer's clear half",
    );
}

#[test]
#[ignore = "needs a VA-API GPU with video processing"]
fn a_pointer_blends_into_rgb() {
    check(&drawn(false), 2);
}

#[test]
#[ignore = "needs a VA-API GPU with video processing"]
fn a_pointer_blends_into_the_streams_video() {
    // Through limited-range BT.709 and back: a few codes of slack.
    check(&drawn(true), 8);
}
