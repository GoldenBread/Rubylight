//! Cursor composition, 10-bit SDR CSC and native 4:4:4 GPU conversion tests.

use super::*;

#[test]
#[ignore = "requires native D3D11 P010 conversion"]
fn gpu_desktop_pointer_matches_cpu_composition_for_sdr_and_hdr() -> Result<()> {
    use windows::Win32::Graphics::Dxgi::{
        DXGI_OUTDUPL_POINTER_SHAPE_INFO, DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME,
    };
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let shape = DXGI_OUTDUPL_POINTER_SHAPE_INFO {
        Type: DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME.0 as u32,
        Width: 4,
        Height: 2,
        Pitch: 1,
        ..Default::default()
    };
    let pointer = crate::cursor::Cursor::new(&gpu, &shape, &[0b00110000, 0b01010000])?;
    for pixel in [Pixel::Bgra8, Pixel::RgbaF16, Pixel::Rgba10Pq] {
        let mut source = if pixel == Pixel::RgbaF16 {
            make_image(&[[0.; 3]], 64, 64)
        } else {
            Image {
                width: 64,
                height: 64,
                stride: 256,
                bytes: vec![0; 64 * 256],
                pixel,
                captured: Instant::now(),
            }
        };
        let mut image = GpuImage::upload(&gpu, &source)?;
        image.cursor = Some(pointer.clone());
        pointer.blend(&mut source);
        let cpu_composited = GpuImage::upload(&gpu, &source)?;
        let config = butterpollo_media_policy::rtsp::Negotiated {
            width: 64,
            height: 64,
            codec: 1,
            hdr: pixel != Pixel::Bgra8,
            sdr_10bit: pixel == Pixel::Bgra8,
            ..Default::default()
        };
        let mut converter = Converter::new(&gpu, &config, (64, 64, pixel))?;
        let actual = readback(&gpu, converter.convert(&image)?.as_ref())?;
        let expected = readback(&gpu, converter.convert(&cpu_composited)?.as_ref())?;
        for (a, e) in actual.iter().zip(&expected) {
            assert!(a.abs_diff(*e) <= 2, "pointer {pixel:?}: {a} versus {e}");
        }
        assert!(
            actual[1] > actual[0] + 300,
            "white replacement was not rendered"
        );
        assert!(actual[3] > actual[2] + 300, "XOR pointer was not rendered");
        // Removing or moving a cursor must not retain the previous constants.
        image.cursor.as_mut().unwrap().position = [-1, 1];
        let shifted = readback(&gpu, converter.convert(&image)?.as_ref())?;
        assert!(shifted[64] > shifted[0] + 300);
        image.cursor = None;
        let absent = readback(&gpu, converter.convert(&image)?.as_ref())?;
        assert!(
            absent
                .iter()
                .take(4096)
                .all(|value| value.abs_diff(64) <= 1)
        );
    }
    Ok(())
}
#[test]
#[ignore = "requires a D3D11 GPU with native P010 render views"]
fn gpu_sdr_ten_bit_respects_client_matrix_and_full_range() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let image = Image {
        width: 64,
        height: 64,
        stride: 256,
        bytes: [0, 0, 255, 255].repeat(64 * 64),
        pixel: Pixel::Bgra8,
        captured: Instant::now(),
    };
    let uploaded = GpuImage::upload(&gpu, &image)?;
    for (csc_mode, expected) in [
        (0, [326, 361, 960]),
        (1, [306, 339, 1023]),
        (2, [250, 409, 960]),
        (3, [217, 395, 1023]),
        (4, [387, 371, 769]),
        (5, [378, 351, 805]),
    ] {
        let config = butterpollo_media_policy::rtsp::Negotiated {
            width: 64,
            height: 64,
            codec: 1,
            sdr_10bit: true,
            csc_mode,
            ..Default::default()
        };
        let mut convert = Converter::new(&gpu, &config, (64, 64, Pixel::Bgra8))?;
        let output = readback(&gpu, convert.convert(&uploaded)?.as_ref())?;
        let observed = [output[0], output[4096], output[4097]];
        for (actual, wanted) in observed.into_iter().zip(expected) {
            assert!(actual.abs_diff(wanted) <= 1, "CSC {csc_mode}: {observed:?}");
        }
    }
    Ok(())
}

#[test]
fn the_red_reference_matches_the_published_csc_tables() {
    // The 10-bit codes are the ones the 4:2:0 test above checks the GPU
    // against; the 8-bit ones follow from the same matrices at 8 bits.
    for (csc_mode, eight, ten) in [
        (0, [81, 90, 240], [326, 361, 960]),
        (1, [76, 85, 255], [306, 339, 1023]),
        (2, [63, 102, 240], [250, 409, 960]),
        (3, [54, 99, 255], [217, 395, 1023]),
    ] {
        assert_eq!(red_codes(csc_mode, 8), eight, "CSC {csc_mode}");
        assert_eq!(red_codes(csc_mode, 10), ten, "CSC {csc_mode}");
    }
}
#[test]
fn native_444_targets_follow_sunshines_nvenc_input_formats() {
    let mut config = butterpollo_media_policy::rtsp::Negotiated {
        width: 64,
        height: 64,
        codec: 1,
        yuv444: true,
        ..Default::default()
    };
    assert_eq!(Layout::of(&config), Layout::Ayuv);
    assert_eq!(
        Layout::Ayuv.formats(),
        (DXGI_FORMAT_AYUV, DXGI_FORMAT_R8G8B8A8_UINT, None)
    );
    assert_eq!(Layout::Ayuv.rows(64), 64);
    for (hdr, sdr_10bit) in [(true, false), (false, true)] {
        config.hdr = hdr;
        config.sdr_10bit = sdr_10bit;
        assert_eq!(Layout::of(&config), Layout::Planes444);
    }
    assert_eq!(
        Layout::Planes444.formats(),
        (DXGI_FORMAT_R16_UINT, DXGI_FORMAT_R16_UINT, None)
    );
    assert_eq!(Layout::Planes444.rows(64), 192);
    config.yuv444 = false;
    assert_eq!(Layout::of(&config), Layout::P010);
    config.hdr = false;
    config.sdr_10bit = false;
    assert_eq!(Layout::of(&config), Layout::Nv12);
    assert_eq!(Layout::Nv12.rows(64), 64);
}

#[test]
#[ignore = "requires a D3D11 GPU with AYUV render views"]
fn gpu_native_444_eight_bit_writes_full_resolution_ayuv_codes() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let image = Image {
        width: 64,
        height: 64,
        stride: 256,
        bytes: [0, 0, 255, 255].repeat(64 * 64),
        pixel: Pixel::Bgra8,
        captured: Instant::now(),
    };
    let uploaded = GpuImage::upload(&gpu, &image)?;
    for csc_mode in 0..4u8 {
        let config = butterpollo_media_policy::rtsp::Negotiated {
            width: 64,
            height: 64,
            codec: 1,
            yuv444: true,
            csc_mode,
            ..Default::default()
        };
        let mut convert = Converter::new(&gpu, &config, (64, 64, Pixel::Bgra8))?;
        let output = readback_ayuv(&gpu, convert.convert(&uploaded)?.as_ref())?;
        assert_eq!(output.len(), 64 * 64);
        let [y, cb, cr] = red_codes(csc_mode, 8);
        for [v, u, luma, _] in [output[0], output[64 * 64 - 1]] {
            let observed = [luma, u, v].map(u16::from);
            for (actual, wanted) in observed.into_iter().zip([y, cb, cr]) {
                assert!(actual.abs_diff(wanted) <= 1, "CSC {csc_mode}: {observed:?}");
            }
        }
    }
    Ok(())
}
#[test]
#[ignore = "requires a D3D11 GPU with R16_UINT render views"]
fn gpu_native_444_ten_bit_stacks_y_u_and_v_planes() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    // Red on the left half and blue on the right, so a plane that is
    // subsampled or misplaced shows in the chroma rows.
    let red_half = [0, 0, 255, 255].repeat(32);
    let blue_half = [255, 0, 0, 255].repeat(32);
    let image = Image {
        width: 64,
        height: 64,
        stride: 256,
        bytes: [red_half, blue_half].concat().repeat(64),
        pixel: Pixel::Bgra8,
        captured: Instant::now(),
    };
    let uploaded = GpuImage::upload(&gpu, &image)?;
    for csc_mode in 0..4u8 {
        let config = butterpollo_media_policy::rtsp::Negotiated {
            width: 64,
            height: 64,
            codec: 1,
            yuv444: true,
            sdr_10bit: true,
            csc_mode,
            ..Default::default()
        };
        let mut convert = Converter::new(&gpu, &config, (64, 64, Pixel::Bgra8))?;
        // The R16_UINT target holds 16-bit codes; `readback` keeps the top ten bits.
        let output = readback(&gpu, convert.convert(&uploaded)?.as_ref())?;
        assert_eq!(output.len(), 64 * 64 * 3);
        let red = red_codes(csc_mode, 10);
        for row in [0, 63] {
            for (plane, wanted) in red.into_iter().enumerate() {
                let at = plane * 4096 + row * 64;
                let observed = output[at + 31];
                assert!(
                    observed.abs_diff(wanted) <= 1,
                    "CSC {csc_mode} plane {plane} row {row}: {observed} versus {wanted}"
                );
                // The blue neighbour's Cb is high and its Cr low: a chroma
                // plane averaged across pixels would not keep both columns.
                let blue = output[at + 32];
                match plane {
                    1 => assert!(blue > observed + 300, "CSC {csc_mode}: Cb {blue}"),
                    2 => assert!(observed > blue + 300, "CSC {csc_mode}: Cr {blue}"),
                    _ => {}
                }
            }
        }
    }
    Ok(())
}
