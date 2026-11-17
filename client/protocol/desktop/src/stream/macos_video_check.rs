//! Hardware checks invoked by the demo build on Cocoa's main thread.
use std::ptr;

use nightjar_core::sdp::{VIDEO_FORMAT_H265, VIDEO_FORMAT_H265_MAIN10};
use nightjar_core::session::VideoSetup;
use sdl3_sys::everything::*;

use super::{Decoded, VideoTexture};
use crate::media::{self, Codec, Frame, Hardware, Picture};
use crate::stream::{color_probe, packed444, pyrowave_gpu, upscale};

/// Submit through the stream presenter, release the caller's AVFrame, then sample
/// the rendered picture. Only the presenter keeps an imported pool slot alive.
fn draw(
    renderer: *mut SDL_Renderer,
    texture: &mut VideoTexture,
    frame: Frame,
    format: u32,
) -> Vec<[u8; 3]> {
    let info = frame.info();
    let decoded = Decoded {
        picture: Picture::Frame(frame),
        setup: VideoSetup {
            format,
            width: info.width,
            height: info.height,
            fps: 60,
            chroma_packing: 0,
            logical_width: info.width,
            logical_height: info.height,
            color_space: 1,
            color_range: 0,
        },
        number: 1,
        rtp: 0,
        receive_us: 0,
        enqueue_us: 0,
        input_us: 0,
        ready_us: 0,
        host_latency: 0,
        early: 0,
        first_block_us: 0,
        race: Default::default(),
        bands: Vec::new(),
        race_seq: 0,
        visible: None,
    };
    assert!(
        texture.upload(renderer, &decoded, None),
        "{:?}",
        texture.failure
    );
    drop(decoded);
    // The renderer, texture and readback surface are live on the main thread.
    unsafe {
        assert!(SDL_RenderTexture(
            renderer,
            texture.sdl_texture(renderer),
            ptr::null(),
            ptr::null()
        ));
        let surface = SDL_RenderReadPixels(renderer, ptr::null());
        assert!(!surface.is_null(), "{}", crate::sdl::error());
        let mut colors = Vec::new();
        for y in [1, 3, 5, 7] {
            for x in [1, 3, 5, 7] {
                let (mut r, mut g, mut b, mut a) = (0, 0, 0, 0);
                assert!(
                    SDL_ReadSurfacePixel(
                        surface,
                        x * (*surface).w / 8,
                        y * (*surface).h / 8,
                        &mut r,
                        &mut g,
                        &mut b,
                        &mut a
                    ),
                    "{}",
                    crate::sdl::error()
                );
                colors.push([r, g, b]);
            }
        }
        SDL_DestroySurface(surface);
        colors
    }
}

fn check_surfaces(renderer: *mut SDL_Renderer) {
    let mut texture = VideoTexture::new(false, false);
    for (format, ten_bit, fixture) in [
        (
            VIDEO_FORMAT_H265,
            0,
            include_bytes!("../../tests/fixtures/hevc-420-8.bin").as_slice(),
        ),
        (
            VIDEO_FORMAT_H265_MAIN10,
            1,
            include_bytes!("../../tests/fixtures/hevc-420-10.bin").as_slice(),
        ),
    ] {
        let mut decoder =
            media::VideoDecoder::open(Codec::Hevc, Hardware::VideoToolbox, 1, true).unwrap();
        assert_eq!(decoder.send(fixture, 42), 0);
        let media::Received::Frame(frame) = decoder.receive() else {
            panic!("no VideoToolbox frame")
        };
        assert!(
            matches!(decoder.receive(), media::Received::None),
            "polling an empty decoder must return EAGAIN"
        );
        let info = frame.info();
        assert_eq!(
            (info.kind, info.ten_bit, info.pts),
            (media::FRAME_VIDEOTOOLBOX, ten_bit, 42)
        );
        let mapped = frame.map_to_memory().expect("readable NV12/P010 planes");
        let imported = draw(renderer, &mut texture, frame, format);
        assert!(
            texture.held.is_some(),
            "the native Metal renderer must import the pixel buffer"
        );
        if super::super::metalfx::supported() {
            let (buffer, info) = texture.metalfx_source().expect("SDR VideoToolbox source");
            let input = texture.size();
            let mut pass = super::super::metalfx::Spatial::new(renderer, input, (input.0 * 2, input.1 * 2)).expect("MetalFX scaler");
            // Submit twice: catches output IOSurface lifetime and command ordering regressions.
            for _ in 0..2 {
                unsafe {
                    let scaled = pass.draw(renderer, buffer, &info).expect("MetalFX submission");
                    assert!(pass.gpu_ms.is_finite() && pass.gpu_ms > 0.0);
                    assert!(SDL_RenderTexture(renderer, scaled, ptr::null(), ptr::null()));
                    let surface = SDL_RenderReadPixels(renderer, ptr::null());
                    assert!(!surface.is_null(), "{}", crate::sdl::error());
                    let mut error = 0u32;
                    for (index, reference) in imported.iter().enumerate() {
                        let (mut r, mut g, mut b, mut a) = (0, 0, 0, 0);
                        let x = (index % 4 * 2 + 1) as i32 * (*surface).w / 8;
                        let y = (index / 4 * 2 + 1) as i32 * (*surface).h / 8;
                        assert!(SDL_ReadSurfacePixel(surface, x, y, &mut r, &mut g, &mut b, &mut a));
                        error += [r, g, b].iter().zip(reference).map(|(a,b)| a.abs_diff(*b) as u32).sum::<u32>();
                    }
                    let mean_error = error as f32 / (imported.len() * 3) as f32;
                    assert!(mean_error < 18.0, "MetalFX color error {mean_error}");
                    let path = std::ffi::CString::new(std::env::temp_dir().join(format!("nightjar-metalfx-{ten_bit}.bmp")).to_str().unwrap()).unwrap();
                    assert!(SDL_SaveBMP(surface, path.as_ptr()));
                    SDL_DestroySurface(surface);
                    println!("MetalFX {}x{} -> {}x{}, {} bit: {:.3} ms GPU, mean RGB difference {:.2}", input.0, input.1, input.0*2, input.1*2, if ten_bit==0 {8} else {10}, pass.gpu_ms, mean_error);
                }
            }
        } else { println!("MetalFX unavailable on this device; native fallback retained"); }
        let uploaded = draw(renderer, &mut texture, mapped, format);
        assert!(
            texture.held.is_none(),
            "a CPU upload must replace the external texture"
        );
        assert!(
            uploaded.iter().any(|rgb| *rgb != uploaded[0]),
            "the fixture must have visible detail"
        );
        for (pixel, reference) in imported.iter().zip(uploaded) {
            assert!(
                pixel
                    .iter()
                    .zip(reference)
                    .all(|(&a, b)| a.abs_diff(b) <= 2),
                "imported {pixel:?}, uploaded {reference:?}"
            );
        }
    }
    texture.destroy();
}

pub(crate) fn run() {
    upscale::check_scalers();
    // The test subprocess owns SDL and all its resources until this check exits.
    unsafe {
        assert!(SDL_Init(SDL_INIT_VIDEO), "{}", crate::sdl::error());
        let window = SDL_CreateWindow(
            c"Nightjar Metal check".as_ptr(),
            256,
            256,
            SDL_WINDOW_HIDDEN,
        );
        assert!(!window.is_null(), "{}", crate::sdl::error());
        let native = crate::sdl::create_renderer_with(window, false, false);
        assert!(!native.is_null(), "{}", crate::sdl::error());
        assert_eq!(crate::sdl::renderer_name(native), "metal");
        check_surfaces(native);
        crate::sdl::destroy_renderer(native);

        let renderer = crate::sdl::create_renderer_with(window, true, false);
        assert!(!renderer.is_null(), "{}", crate::sdl::error());
        for depth in [false, true] {
            pyrowave_gpu::probe(renderer, depth).expect("PyroWave fixture reconstruction");
            color_probe::planes_drawable(renderer, depth).expect("Metal plane shader");
        }
        let modes = packed444::probe(renderer, Hardware::VideoToolbox)
            .expect("packed 4:4:4 pixel comparison");
        assert_ne!(modes, 0, "no packed format passed");
        println!("Metal readback passed for NV12/P010 imports and uploads, both scalers, PyroWave and packed modes {modes:#x}");
        crate::sdl::destroy_renderer(renderer);
        benchmark_fixture(window);
        SDL_DestroyWindow(window);
        SDL_Quit();
    }
}

/// Optional reproducible comparison using a local single-frame Annex-B HEVC fixture.
/// Timings include synchronous readback for every mode; they are not live-stream latency.
fn benchmark_fixture(window: *mut SDL_Window) {
    let Some(path) = std::env::var_os("NIGHTJAR_METALFX_FIXTURE") else { return; };
    let bytes = std::fs::read(path).expect("read benchmark fixture");
    for mode in [crate::config::Upscaling::MetalFx, crate::config::Upscaling::Fsr1, crate::config::Upscaling::Sgsr1] {
        if mode == crate::config::Upscaling::MetalFx && !super::super::metalfx::supported() {
            println!("MetalFX unavailable on this device; benchmark skips it");
            continue;
        }
        unsafe {
            let renderer = crate::sdl::create_renderer_with(window, mode.is_shader(), false);
            assert!(!renderer.is_null());
            let mut decoder = media::VideoDecoder::open(Codec::Hevc, Hardware::VideoToolbox, 1, true).unwrap();
            assert_eq!(decoder.send(&bytes, 1), 0);
            let media::Received::Frame(frame) = decoder.receive() else { panic!("benchmark decoded frame"); };
            let mut video = VideoTexture::new(false, false);
            let _ = draw(renderer, &mut video, frame, VIDEO_FORMAT_H265);
            let input = video.size();
            let output = (input.0 * 3 / 2, input.1 * 3 / 2);
            let mut native = (mode == crate::config::Upscaling::MetalFx).then(|| super::super::metalfx::Spatial::new(renderer, input, output).expect("MetalFX benchmark"));
            let mut shader = mode.is_shader().then(|| upscale::Upscaler::new(renderer, input, output, mode).expect("shader benchmark"));
            let target = SDL_CreateTexture(renderer, SDL_PIXELFORMAT_RGBA32, SDL_TEXTUREACCESS_TARGET, output.0, output.1);
            assert!(!target.is_null());
            let mut times = Vec::new();
            let mut gpu = Vec::new();
            for index in 0..35 {
                let start = std::time::Instant::now();
                let scaled = if let Some(native) = &mut native {
                    let (buffer, info) = video.metalfx_source().unwrap();
                    let texture = native.draw(renderer, buffer, &info).expect("native benchmark draw");
                    if index >= 5 { gpu.push(native.gpu_ms); }
                    texture
                } else { shader.as_mut().unwrap().draw(renderer, video.sdl_texture(renderer), 50).expect("shader benchmark draw") };
                assert!(SDL_SetRenderTarget(renderer, target));
                assert!(SDL_RenderTexture(renderer, scaled, ptr::null(), ptr::null()));
                let surface = SDL_RenderReadPixels(renderer, ptr::null());
                assert!(!surface.is_null());
                if index >= 5 { times.push(start.elapsed().as_secs_f64() * 1000.0); }
                if index == 34 {
                    let path = std::env::temp_dir().join(format!("nightjar-{:?}-quality.bmp", mode));
                    let path = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
                    assert!(SDL_SaveBMP(surface, path.as_ptr()));
                }
                SDL_DestroySurface(surface);
                SDL_SetRenderTarget(renderer, ptr::null_mut());
            }
            times.sort_by(f64::total_cmp);
            gpu.sort_by(f64::total_cmp);
            println!("{:?} {}x{} -> {}x{}: completed draw + readback median {:.3} ms, p95 {:.3} ms (30 frames); native conversion + scaler + blit GPU median {:?} ms", mode, input.0, input.1, output.0, output.1, times[15], times[28], gpu.get(15));
            SDL_DestroyTexture(target);
            drop(native); drop(shader); video.destroy();
            crate::sdl::destroy_renderer(renderer);
        }
    }
}
