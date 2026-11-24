//! What video processing writes for flat colours: the Windows host's codes for each colour
//! space a client negotiates, from desktop RGB, from gamescope's BT.709 video, and back.
use super::{
    BGRX, NV12,
    display::Va,
    ffi::{VA_RT_FORMAT_RGB32, VA_RT_FORMAT_YUV420},
    pool::{Pool, Pooled},
    vpp::{Colors, Converter, Source},
};
use std::os::fd::AsRawFd;
use std::sync::Arc;

const SIZE: (u32, u32) = (64, 64);
const COLOURS: [[u8; 3]; 4] = [[200, 100, 50], [16, 200, 240], [250, 250, 250], [5, 5, 5]];

/// The Windows host's `lumaCode` and `chromaCode` for one pixel.
fn codes(rgb: [u8; 3], colors: Colors) -> [u8; 3] {
    let k: [f64; 3] = match colors.matrix {
        2 => [0.2627, 0.6780, 0.0593],
        0 => [0.299, 0.587, 0.114],
        _ => [0.2126, 0.7152, 0.0722],
    };
    let [r, g, b] = rgb.map(|c| f64::from(c) / 255.0);
    let y = r * k[0] + g * k[1] + b * k[2];
    let code = |v: f64| (v.clamp(0.0, 255.0) + 0.5).floor() as u8;
    let (luma, scale) = if colors.full_range {
        (255.0 * y, 255.0)
    } else {
        (16.0 + 219.0 * y, 224.0)
    };
    [
        code(luma),
        code(128.0 + scale * (b - y) / (2.0 * (1.0 - k[2]))),
        code(128.0 + scale * (r - y) / (2.0 * (1.0 - k[0]))),
    ]
}

/// The Y, Cb and Cr at the middle of a flat linear NV12 surface.
fn middle(surface: &Pooled) -> [u8; 3] {
    surface.va().sync(surface.id()).unwrap();
    let exported = crate::pyrowave::dmabuf::export_surface(surface).unwrap();
    let fd = exported.fd.as_raw_fd();
    // SAFETY: a seek on the exported descriptor.
    let size = unsafe { libc::lseek(fd, 0, libc::SEEK_END) } as usize;
    // SAFETY: a read-only shared mapping of the whole DMA-BUF, unmapped below.
    let mapped = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            size,
            libc::PROT_READ,
            libc::MAP_SHARED,
            fd,
            0,
        )
    };
    assert_ne!(mapped, libc::MAP_FAILED, "mapping the surface");
    // SAFETY: `size` mapped bytes.
    let bytes = unsafe { std::slice::from_raw_parts(mapped.cast::<u8>(), size) };
    let pitch = exported.planes[0].pitch as usize;
    let chroma = exported.planes[1].offset as usize + 16 * pitch + 32;
    let samples = [bytes[32 * pitch + 32], bytes[chroma], bytes[chroma + 1]];
    // SAFETY: the mapping made above.
    unsafe { libc::munmap(mapped, size) };
    samples
}

fn close(got: [u8; 3], want: [u8; 3], tolerance: u8) -> bool {
    got.iter()
        .zip(want)
        .all(|(&a, b)| a.abs_diff(b) <= tolerance)
}

/// Each colour space's codes are the Windows host's, within a code: converted from desktop
/// RGB, converted from gamescope's BT.709 limited video (the capture's path), and read back
/// to RGB within two codes of the colour drawn.
#[test]
fn video_processing_changes_only_the_matrix() {
    let Some(node) = super::node::nodes().first().cloned() else {
        eprintln!("skipped: no render node");
        return;
    };
    let va = Arc::new(Va::open(&node.path).expect("VA-API"));
    let linear = |rt, pixel| Pool::new_linear(&va, rt, pixel, SIZE.0, SIZE.1, 2, 2).unwrap();
    let (rgb, video) = (
        linear(VA_RT_FORMAT_RGB32, BGRX),
        linear(VA_RT_FORMAT_YUV420, NV12),
    );
    let gamescope = Converter::new(&va, SIZE.0, SIZE.1).unwrap();
    let mut failures = Vec::new();
    for colour in COLOURS {
        let source = rgb.take().unwrap();
        let bgra: Vec<u8> = (0..SIZE.0 * SIZE.1)
            .flat_map(|_| [colour[2], colour[1], colour[0], 255])
            .collect();
        va.upload_bgra(source.id(), SIZE.0, SIZE.1, SIZE.0 as usize * 4, &bgra)
            .unwrap();
        va.sync(source.id()).unwrap();
        // What gamescope hands the capture.
        let bt709 = video.take().unwrap();
        gamescope
            .convert(source.id(), Source::Rgb, SIZE, bt709.id(), SIZE)
            .unwrap();
        let want = codes(colour, Colors::BT709_LIMITED);
        if !close(middle(&bt709), want, 1) {
            failures.push(format!(
                "{colour:?} as BT.709: {:?}, not {want:?}",
                middle(&bt709)
            ));
        }
        for colors in [(0, false), (0, true), (1, false), (1, true), (2, false)]
            .map(|(matrix, full_range)| Colors { matrix, full_range })
        {
            let converter = Converter::new(&va, SIZE.0, SIZE.1).unwrap().writing(colors);
            let want = codes(colour, colors);
            for (path, input, kind) in [
                ("RGB", source.id(), Source::Rgb),
                (
                    "BT.709 video",
                    bt709.id(),
                    Source::Video(Colors::BT709_LIMITED),
                ),
            ] {
                let target = video.take().unwrap();
                converter
                    .convert(input, kind, SIZE, target.id(), SIZE)
                    .unwrap();
                let got = middle(&target);
                if !close(got, want, 1) {
                    failures.push(format!(
                        "{colour:?} from {path} as {colors:?}: {got:?}, not {want:?}"
                    ));
                }
                if path == "RGB" {
                    let back = rgb.take().unwrap();
                    converter.to_rgb(target.id(), SIZE, back.id()).unwrap();
                    let mut bytes = Vec::new();
                    va.download_bgra(back.id(), SIZE.0, SIZE.1, &mut bytes)
                        .unwrap();
                    let at = (32 * SIZE.0 as usize + 32) * 4;
                    let got = [bytes[at + 2], bytes[at + 1], bytes[at]];
                    if !close(got, colour, 2) {
                        failures.push(format!("{colour:?} read back from {colors:?}: {got:?}"));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
