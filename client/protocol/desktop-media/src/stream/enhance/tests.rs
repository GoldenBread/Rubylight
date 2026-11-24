//! Unit and GPU tests for AI enhancement capability planning and video processor / AMF scaling.

use super::*;
use std::cell::RefCell;

#[derive(Default)]
struct Fake {
    amf: bool,
    intel: bool,
    nvidia: bool,
    nvidia_hdr: bool,
    asked: RefCell<Vec<&'static str>>,
}

impl Probe for Fake {
    fn amf(&self) -> bool { self.asked.borrow_mut().push("amf"); self.amf }
    fn intel_super_resolution(&self) -> bool { self.asked.borrow_mut().push("intel"); self.intel }
    fn nvidia_super_resolution(&self) -> bool { self.asked.borrow_mut().push("nvidia"); self.nvidia }
    fn nvidia_hdr(&self) -> bool { self.asked.borrow_mut().push("nvidia_hdr"); self.nvidia_hdr }
}

fn all() -> Fake {
    Fake { amf: true, intel: true, nvidia: true, nvidia_hdr: true, ..Default::default() }
}

#[test]
fn each_vendor_is_probed_for_its_own_features_only() {
    let fake = all();
    assert_eq!(capability(VENDOR_AMD, &fake), Some(Capability { engine: Engine::AmdAmf, super_resolution: true, sdr_to_hdr: false }));
    assert_eq!(*fake.asked.borrow(), ["amf"]);
    let fake = all();
    assert_eq!(capability(VENDOR_INTEL, &fake), Some(Capability { engine: Engine::IntelVpe, super_resolution: true, sdr_to_hdr: false }));
    assert_eq!(*fake.asked.borrow(), ["intel"]);
    let fake = all();
    assert_eq!(capability(VENDOR_NVIDIA, &fake), Some(Capability { engine: Engine::NvidiaRtx, super_resolution: true, sdr_to_hdr: true }));
    assert_eq!(*fake.asked.borrow(), ["nvidia", "nvidia_hdr"]);
    let fake = all();
    assert_eq!(capability(0x5143, &fake), None, "other vendors are not offered");
    assert!(fake.asked.borrow().is_empty());
}

#[test]
fn a_gpu_that_fails_its_probe_is_not_offered() {
    assert_eq!(capability(VENDOR_AMD, &Fake::default()), None, "no AMF runtime");
    assert_eq!(capability(VENDOR_INTEL, &Fake::default()), None);
    assert_eq!(capability(VENDOR_NVIDIA, &Fake::default()), None);
    // An NVIDIA driver with RTX Video HDR but without super resolution still offers the mode.
    let hdr_only = Fake { nvidia_hdr: true, ..Default::default() };
    assert_eq!(capability(VENDOR_NVIDIA, &hdr_only), Some(Capability { engine: Engine::NvidiaRtx, super_resolution: false, sdr_to_hdr: true }));
}

#[test]
fn pictures_get_what_v_plus_gives_them() {
    let nvidia = Capability { engine: Engine::NvidiaRtx, super_resolution: true, sdr_to_hdr: true };
    let intel = Capability { engine: Engine::IntelVpe, super_resolution: true, sdr_to_hdr: false };
    let amd = Capability { engine: Engine::AmdAmf, super_resolution: true, sdr_to_hdr: false };
    // NVIDIA: super resolution and RTX Video HDR on 8-bit SDR with an HDR display.
    assert_eq!(plan(nvidia, false, false, true), Plan { super_resolution: true, sdr_to_hdr: true, hdr_output: true });
    assert_eq!(plan(nvidia, false, false, false), Plan { super_resolution: true, sdr_to_hdr: false, hdr_output: false }, "SDR display");
    assert_eq!(plan(nvidia, false, true, true), Plan { super_resolution: true, sdr_to_hdr: false, hdr_output: false }, "10-bit SDR");
    assert_eq!(plan(nvidia, true, true, true), Plan { super_resolution: false, sdr_to_hdr: false, hdr_output: true }, "no NVIDIA VSR on HDR");
    // Intel's super resolution runs on HDR pictures only.
    assert_eq!(plan(intel, true, true, true), Plan { super_resolution: true, sdr_to_hdr: false, hdr_output: true });
    assert_eq!(plan(intel, false, false, true), Plan { super_resolution: false, sdr_to_hdr: false, hdr_output: false });
    // AMF scales everything and never turns SDR into HDR.
    assert_eq!(plan(amd, false, false, true), Plan { super_resolution: true, sdr_to_hdr: false, hdr_output: false });
    assert_eq!(plan(amd, true, true, true), Plan { super_resolution: true, sdr_to_hdr: false, hdr_output: true });
    assert_eq!(describe(nvidia, plan(nvidia, false, false, true)), "AI enhancement (NVIDIA RTX Video Super Resolution, RTX Video HDR)");
    assert_eq!(describe(intel, plan(intel, false, false, false)), "AI enhancement (video processor only)");
}

#[test]
fn enhancement_skips_no_op_and_native_amf_passes_to_preserve_direct_draw() {
    let nvidia = Capability { engine: Engine::NvidiaRtx, super_resolution: true, sdr_to_hdr: true };
    let intel = Capability { engine: Engine::IntelVpe, super_resolution: true, sdr_to_hdr: false };
    let amd = Capability { engine: Engine::AmdAmf, super_resolution: true, sdr_to_hdr: false };
    // No-op passes (NVIDIA HDR, Intel SDR) skip AI enhancement even when upscaling.
    assert!(!should_run(nvidia, true, true, true, (1920, 1080), (3840, 2160)));
    assert!(!should_run(intel, false, false, true, (1920, 1080), (3840, 2160)));
    // Useful passes run.
    assert!(should_run(nvidia, false, false, true, (1920, 1080), (1920, 1080)));
    assert!(should_run(intel, true, true, true, (1920, 1080), (3840, 2160)));
    // AMD AMF skips 1:1 native and downscaled outputs, running only when upscaling.
    assert!(!should_run(amd, false, false, false, (1920, 1080), (1920, 1080)));
    assert!(!should_run(amd, false, false, false, (2560, 1440), (1920, 1080)));
    assert!(should_run(amd, false, false, false, (1920, 1080), (2560, 1440)));
    assert!(should_run(amd, true, true, true, (1920, 1080), (3840, 2160)));
}

#[cfg(windows)]
mod gpu_tests {
    use super::super::super::amf_scaler::{self, HqScaler};
    use super::super::super::direct::Transfer;
    use super::super::*;
    use sdl3_sys::everything::SDL_COLORSPACE_BT709_LIMITED;
    use windows::core::Interface;
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
    use windows::Win32::Graphics::Direct3D11::*;
    use windows::Win32::Graphics::Dxgi::Common::*;
    use windows::Win32::Graphics::Dxgi::IDXGIDevice;

    /// A hardware device made as SDL makes the renderer's (no video support flag).
    fn device() -> (ID3D11Device, ID3D11DeviceContext, u32) {
        // SAFETY: Plain D3D11 calls on a device made and used on this thread.
        unsafe {
            let (mut device, mut context) = (None, None);
            D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None,
                D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut context)).unwrap();
            let device: ID3D11Device = device.unwrap();
            let vendor = device.cast::<IDXGIDevice>().unwrap().GetAdapter().unwrap().GetDesc().unwrap().VendorId;
            (device, context.unwrap(), vendor)
        }
    }

    /// An NV12 picture whose luma and chroma come from `code(x, y)` (chroma per 2x2 block).
    fn nv12(device: &ID3D11Device, width: u32, height: u32, code: impl Fn(u32, u32) -> [u8; 3]) -> ID3D11Texture2D {
        let mut data = vec![0u8; (width * height * 3 / 2) as usize];
        for y in 0..height {
            for x in 0..width {
                let [luma, cb, cr] = code(x, y);
                data[(y * width + x) as usize] = luma;
                let at = (width * height + (y / 2) * width + (x / 2) * 2) as usize;
                data[at] = cb;
                data[at + 1] = cr;
            }
        }
        // SAFETY: The initial data holds both planes at a pitch of `width`.
        unsafe {
            let mut texture = None;
            device.CreateTexture2D(&D3D11_TEXTURE2D_DESC { Width: width, Height: height, MipLevels: 1, ArraySize: 1, Format: DXGI_FORMAT_NV12,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, Usage: D3D11_USAGE_DEFAULT, BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32, ..Default::default() },
                Some(&D3D11_SUBRESOURCE_DATA { pSysMem: data.as_ptr().cast(), SysMemPitch: width, SysMemSlicePitch: 0 }), Some(&mut texture)).unwrap();
            texture.unwrap()
        }
    }

    fn target(device: &ID3D11Device, width: u32, height: u32, format: DXGI_FORMAT, bind: u32) -> ID3D11Texture2D {
        // SAFETY: A plain texture on a live device.
        unsafe {
            let mut texture = None;
            device.CreateTexture2D(&D3D11_TEXTURE2D_DESC { Width: width, Height: height, MipLevels: 1, ArraySize: 1, Format: format,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, Usage: D3D11_USAGE_DEFAULT, BindFlags: bind, ..Default::default() }, None, Some(&mut texture)).unwrap();
            texture.unwrap()
        }
    }

    /// The rows of `texture` as bytes (for NV12, the luma rows then the chroma rows).
    fn read(device: &ID3D11Device, context: &ID3D11DeviceContext, texture: &ID3D11Texture2D, row_bytes: usize, rows: usize) -> Vec<u8> {
        // SAFETY: A staging copy mapped on this thread.
        unsafe {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);
            let mut staging = None;
            device.CreateTexture2D(&D3D11_TEXTURE2D_DESC { Usage: D3D11_USAGE_STAGING, BindFlags: 0, CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32, MiscFlags: 0, ..desc },
                None, Some(&mut staging)).unwrap();
            let staging = staging.unwrap();
            context.CopyResource(&staging, texture);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).unwrap();
            let mut out = Vec::new();
            for y in 0..rows {
                out.extend_from_slice(std::slice::from_raw_parts(mapped.pData.cast::<u8>().add(y * mapped.RowPitch as usize), row_bytes));
            }
            context.Unmap(&staging, 0);
            out
        }
    }

    #[test]
    fn the_video_processor_reads_pq_only_when_the_picture_is_pq() {
        use sdl3_sys::everything::{SDL_COLORSPACE_BT2020_FULL, SDL_COLORSPACE_BT2020_LIMITED};
        // 10-bit SDR is BT.2020 with a gamma transfer, not PQ.
        assert_eq!(colors(SDL_COLORSPACE_BT2020_LIMITED, Transfer::Sdr, false).0, DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P2020);
        assert_eq!(colors(SDL_COLORSPACE_BT2020_FULL, Transfer::Sdr, false).0, DXGI_COLOR_SPACE_YCBCR_FULL_G22_LEFT_P2020);
        // A PQ picture is read as PQ whatever its VUI matrix says.
        assert_eq!(colors(SDL_COLORSPACE_BT2020_LIMITED, Transfer::Pq, true).0, DXGI_COLOR_SPACE_YCBCR_STUDIO_G2084_LEFT_P2020);
        assert_eq!(colors(SDL_COLORSPACE_BT709_LIMITED, Transfer::Pq, true).0, DXGI_COLOR_SPACE_YCBCR_STUDIO_G2084_LEFT_P2020);
        // HLG as HLG, never as PQ or gamma.
        assert_eq!(colors(SDL_COLORSPACE_BT2020_LIMITED, Transfer::Hlg, true).0, DXGI_COLOR_SPACE_YCBCR_STUDIO_GHLG_TOPLEFT_P2020);
        assert_eq!(colors(SDL_COLORSPACE_BT2020_FULL, Transfer::Hlg, false).0, DXGI_COLOR_SPACE_YCBCR_FULL_GHLG_TOPLEFT_P2020);
    }

    #[test]
    #[ignore = "needs an AMD GPU with the AMF runtime"]
    fn the_probe_offers_amf_on_an_amd_gpu() {
        let (device, _, vendor) = device();
        if vendor != VENDOR_AMD { eprintln!("not an AMD GPU ({vendor:#06x}): skipped"); return; }
        assert_eq!(probe(device.as_raw()).unwrap(), Some(Capability { engine: Engine::AmdAmf, super_resolution: true, sdr_to_hdr: false }));
    }

    #[test]
    #[ignore = "needs a GPU with a Direct3D 11 video processor"]
    fn the_video_processor_upscales_nv12_into_the_rgb_picture() {
        let (device, context, _) = device();
        // BT.709 limited-range white, black and red bands, 32 pixels wide each.
        let bands = [[235, 128, 128], [16, 128, 128], [63, 102, 240]];
        let input = nv12(&device, 96, 32, |x, _| bands[(x / 32) as usize]);
        let output = target(&device, 192, 64, DXGI_FORMAT_R10G10B10A2_UNORM, (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32);
        // SAFETY: Textures on the test's device, used on this thread.
        unsafe {
            let processor = Processor::new(&device, &input, (96, 32), &output, 60, colors(SDL_COLORSPACE_BT709_LIMITED, Transfer::Sdr, false), None).unwrap();
            processor.blt().unwrap();
        }
        let pixels = read(&device, &context, &output, 192 * 4, 64);
        let at = |x: usize, y: usize| {
            let v = u32::from_le_bytes(pixels[(y * 192 + x) * 4..][..4].try_into().unwrap());
            [v & 1023, (v >> 10) & 1023, (v >> 20) & 1023]
        };
        let near = |got: [u32; 3], want: [u32; 3]| got.iter().zip(want).all(|(g, w)| g.abs_diff(w) <= 24);
        for (band, want) in [[1023, 1023, 1023], [0, 0, 0], [1023, 0, 0]].into_iter().enumerate() {
            let got = at(band * 64 + 32, 32);
            assert!(near(got, want), "band {band}: {got:?}, expected about {want:?}");
        }
    }

    #[test]
    fn amd_scales_the_picture_without_its_alignment_padding() {
        // A 2184x1968 stream's AV1 picture in its 2240x1968 texture.
        assert_eq!(amf_input((2184, 1968), (2240, 1968)), (2184, 1968));
        assert_eq!(amf_input((1920, 1080), (1920, 1082)), (1920, 1080));
        // As before: a picture of the texture's size, and an odd one in SDL's even texture.
        assert_eq!(amf_input((1920, 1080), (1920, 1080)), (1920, 1080));
        assert_eq!(amf_input((1365, 767), (1366, 768)), (1366, 768));
    }

    #[test]
    #[ignore = "needs an AMD GPU with the AMF runtime"]
    fn amfs_hq_scaler_leaves_a_padded_textures_padding_out() {
        let (device, context, vendor) = device();
        if vendor != VENDOR_AMD { eprintln!("not an AMD GPU ({vendor:#06x}): skipped"); return; }
        // A 320x180 picture in a 384x192 texture whose padding is white.
        let input = nv12(&device, 384, 192, |x, y| if x < 320 && y < 180 { [128, 128, 128] } else { [235, 128, 128] });
        let output = target(&device, 640, 360, DXGI_FORMAT_NV12, D3D11_BIND_SHADER_RESOURCE.0 as u32);
        let (width, height) = amf_input((320, 180), (384, 192));
        let parameters = amf_scaler::Parameters { width, height, output_width: 640, output_height: 360, frame_rate: 60, ten_bit: false,
            sharpness: amf_scaler::sharpness(true, false) };
        // SAFETY: Textures and contexts of the test's device, used on this thread.
        unsafe {
            let mut scaler = HqScaler::new(&device, parameters).unwrap();
            for _ in 0..3 { scaler.render(&input, &output, &context).unwrap(); }
        }
        let pixels = read(&device, &context, &output, 640, 360 * 3 / 2);
        // The scaler's sharpening may darken the edges a little; the white padding would brighten them.
        for (what, x, y) in [("right edge", 638, 180), ("bottom edge", 320, 358), ("corner", 638, 358)] {
            let luma = pixels[y * 640 + x];
            assert!(luma <= 136, "{what} came out {luma}: the padding (235) is in the picture");
        }
        assert!(pixels[180 * 640 + 320].abs_diff(128) <= 3, "the middle came out {}", pixels[180 * 640 + 320]);
    }

    #[test]
    #[ignore = "needs an AMD GPU with the AMF runtime"]
    fn amfs_hq_scaler_upscales_a_flat_picture_and_leaves_the_pipeline_state_alone() {
        let (device, context, vendor) = device();
        if vendor != VENDOR_AMD { eprintln!("not an AMD GPU ({vendor:#06x}): skipped"); return; }
        let input = nv12(&device, 320, 180, |_, _| [128, 100, 150]);
        let output = target(&device, 640, 360, DXGI_FORMAT_NV12, D3D11_BIND_SHADER_RESOURCE.0 as u32);
        let parameters = amf_scaler::Parameters { width: 320, height: 180, output_width: 640, output_height: 360, frame_rate: 60, ten_bit: false, sharpness: amf_scaler::sharpness(true, false) };
        // SAFETY: Textures and contexts of the test's device, used on this thread.
        unsafe {
            let mut scaler = HqScaler::new(&device, parameters).unwrap();
            // Something SDL would have bound, which AMF must not change.
            let bound = target(&device, 4, 4, DXGI_FORMAT_R8G8B8A8_UNORM, D3D11_BIND_SHADER_RESOURCE.0 as u32);
            let mut view = None;
            device.CreateShaderResourceView(&bound, None, Some(&mut view)).unwrap();
            context.PSSetShaderResources(0, Some(&[view.clone()]));
            let mut state = None;
            device.cast::<ID3D11Device1>().unwrap().CreateDeviceContextState(0, &[device.GetFeatureLevel()], D3D11_SDK_VERSION, &ID3D11Device::IID, None, Some(&mut state)).unwrap();
            let context1 = context.cast::<ID3D11DeviceContext1>().unwrap();
            for _ in 0..3 {
                let mut previous = None;
                context1.SwapDeviceContextState(state.as_ref(), Some(&mut previous));
                let rendered = scaler.render(&input, &output, &context);
                context1.SwapDeviceContextState(previous.as_ref(), None);
                rendered.unwrap();
            }
            let mut still = [None];
            context.PSGetShaderResources(0, Some(&mut still));
            assert_eq!(still[0].as_ref().map(|v| v.as_raw()), view.as_ref().map(|v| v.as_raw()), "SDL's bound view survives AMF");
        }
        let pixels = read(&device, &context, &output, 640, 360 * 3 / 2);
        for (what, at, want) in [("luma", 180 * 640 + 320, 128u8), ("Cb", 360 * 640 + 90 * 640 + 320, 100), ("Cr", 360 * 640 + 90 * 640 + 321, 150)] { assert!(pixels[at].abs_diff(want) <= 3, "{what} came out {}, expected about {want}", pixels[at]); }
    }
}
