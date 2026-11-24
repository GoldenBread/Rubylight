//! HDR PQ, HLG, Dolby Vision 8.4, locking and UAV feature-level GPU color tests.

use super::*;

#[test]
#[ignore = "requires a D3D11 GPU with native P010 render views"]
fn hdr_full_and_limited_range_match_absolute_pq_on_both_gpu_paths() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let compute = crate::compute::Compute::for_device(&gpu.device)?;
    // scRGB 1.0 is 80 cd/m². These PQ values independently anchor black,
    // SDR reference white, 1000-nit highlights and the 10000-nit ceiling.
    let pq = [0., 0.48585677, 0.7518271, 1.];
    let image = make_image(&[[0.; 3], [1.; 3], [12.5; 3], [125.; 3]], 64, 64);
    let source = GpuImage::upload_settled(&gpu, &image)?;
    for full in [false, true] {
        let config = butterpollo_media_policy::rtsp::Negotiated {
            width: image.width,
            height: image.height,
            codec: 2,
            hdr: true,
            csc_mode: 4 | u8::from(full),
            ..Default::default()
        };
        let mut graphics = Converter::new(&gpu, &config, (image.width, image.height, image.pixel))?;
        graphics.compute = None;
        let graphics_pixels = readback(&gpu, graphics.convert(&source)?.as_ref())?;
        let mut converter =
            crate::compute::Converter::new(compute.clone(), image.width, image.height, true)?;
        converter.values = constants(
            &config,
            (image.width, image.height, image.pixel),
            [100., 1.],
            None,
        );
        let opened = compute.open(&source.texture)?;
        let converted =
            converter.convert(&opened, crate::compute::format(image.pixel), None, &source)?;
        converted.wait()?;
        let compute_pixels = readback_compute(&compute, &converted.texture)?;
        for (path, pixels) in [("D3D11", graphics_pixels), ("D3D12", compute_pixels)] {
            let width = image.width as usize;
            for (patch, pq) in pq.into_iter().enumerate() {
                let x = patch * 64 + 32;
                let wanted = if full { 1023. * pq } else { 64. + 876. * pq };
                let luma = pixels[32 * width + x];
                assert!(
                    (f64::from(luma) - wanted).abs() <= 2.,
                    "{path} full={full} patch={patch}: luma {luma}, wanted {wanted}"
                );
                for c in 0..2 {
                    let chroma = pixels[width * image.height as usize + 16 * width + x + c];
                    assert!(
                        chroma.abs_diff(512) <= 1,
                        "{path} full={full} patch={patch}: neutral chroma {chroma}"
                    );
                }
            }
        }
    }
    Ok(())
}

#[test]
fn hlg_streams_and_dolby_vision_8_4_tell_the_shader_hlg() {
    // cbuffer Config: hdr=5. rubylight_protocol::hdr: PQ 1, HLG 2, DV_81 4, DV_84 8.
    let hdr = |hdr, hdr_format| {
        let config = butterpollo_media_policy::rtsp::Negotiated {
            width: 64,
            height: 64,
            codec: 1,
            hdr,
            hdr_format,
            ..Default::default()
        };
        constants(&config, (64, 64, Pixel::RgbaF16), [100., 1.], None)[5]
    };
    assert_eq!(hdr(false, 0), 0);
    assert_eq!(hdr(false, 2), 0, "HLG needs an HDR stream");
    assert_eq!(hdr(true, 0), 1);
    assert_eq!(hdr(true, 1), 1);
    assert_eq!(hdr(true, 4), 1);
    assert_eq!(hdr(true, 2), 2);
    assert_eq!(hdr(true, 8), 2);
}

#[test]
fn the_shader_hlg_constants_match_arib_std_b67() {
    // ARIB STD-B67: b = 1 - 4a, c = 0.5 - a ln(4a), and the statistics' cap is
    // PQ at the 1000-nit grading (`color::tests` checks PQ itself).
    let a: f64 = 0.17883277;
    let pq_1000 = {
        let y = 0.1f64.powf(2610. / 16384.);
        ((3424. / 4096. + 2413. / 128. * y) / (1. + 2392. / 128. * y)).powf(2523. / 32.)
    };
    for (literal, value) in [
        ("0.17883277", a),
        ("0.28466892", 1. - 4. * a),
        ("0.55991073", 0.5 - a * (4. * a).ln()),
        ("0.7518271", pq_1000),
    ] {
        assert!(SHADER.contains(literal), "color.hlsl lost {literal}");
        assert!(
            (literal.parse::<f64>().unwrap() - value).abs() < 1e-7,
            "{literal} vs {value}"
        );
    }
    assert!(SHADER.contains("(1.0 - 1.2) / 1.2"), "system gamma 1.2");
    assert!(SHADER.contains("80.0 / 1000.0"), "the 1000-nit grading");
}

#[test]
#[ignore = "requires a D3D11 GPU with native P010 render views and D3D12 compute"]
fn gpu_hlg_matches_the_cpu_reference_in_every_converter() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    // Grey at 0, 80, 203, 1000 and 2000 nits, then saturated red at 1000.
    let colours = [
        [0.; 3],
        [1.; 3],
        [203. / 80.; 3],
        [12.5; 3],
        [25.; 3],
        [12.5, 0., 0.],
    ];
    let image = make_image(&colours, 64, 64);
    let config = butterpollo_media_policy::rtsp::Negotiated {
        width: image.width,
        height: image.height,
        codec: 1,
        hdr: true,
        hdr_format: 2,
        ..Default::default()
    };
    let source = (image.width, image.height, image.pixel);
    let uploaded = GpuImage::upload_settled(&gpu, &image)?;
    let mut pixel_passes = Converter::new(&gpu, &config, source)?;
    pixel_passes.compute = None;
    let pixel_passes = readback(&gpu, pixel_passes.convert(&uploaded)?.as_ref())?;
    let mut graphics = Converter::new(&gpu, &config, source)?;
    let graphics = readback(&gpu, graphics.convert(&uploaded)?.as_ref())?;
    let queue = crate::compute::Compute::for_device(&gpu.device)?;
    let mut converter =
        crate::compute::Converter::new(queue.clone(), image.width, image.height, true)?;
    converter.values = constants(&config, source, [100., 1.], None);
    let opened = queue.open(&uploaded.texture)?;
    let converted = converter.convert(
        &opened,
        crate::compute::format(image.pixel),
        None,
        &uploaded,
    )?;
    converted.wait()?;
    let compute = readback_compute(&queue, &converted.texture)?;
    let width = image.width as usize;
    for (patch, colour) in colours.iter().enumerate() {
        // BT.2020 primaries are Rec.709's for grey; red goes through the
        // shader's gamut matrix.
        let light = if patch == 5 {
            [0.627404, 0.069097, 0.016391].map(|k| k * 12.5)
        } else {
            colour.map(f64::from)
        };
        let rgb = crate::color::hlg(light);
        let y = 0.2627 * rgb[0] + 0.6780 * rgb[1] + 0.0593 * rgb[2];
        let expected = (64. + 876. * y).round().clamp(0., 1023.);
        let x = patch * 64 + 32;
        for (path, output) in [
            ("pixel passes", &pixel_passes),
            ("graphics", &graphics),
            ("compute", &compute),
        ] {
            let actual = f64::from(output[32 * width + x]);
            assert!(
                (actual - expected).abs() <= 2.,
                "{path} patch {patch}: luma {actual}, wanted {expected}"
            );
        }
    }
    // HLG's peak and BT.2408's reference white, in codes.
    assert!(graphics[32 * width + 3 * 64 + 32].abs_diff(940) <= 1);
    assert!(graphics[32 * width + 2 * 64 + 32].abs_diff(721) <= 1);
    Ok(())
}

/// PyroWave's compute conversion (`pyrowave::ComputePlanes`) of an HLG stream:
/// the R16 luma plane holds code / 1023 and matches `color::hlg`.
#[test]
#[ignore = "requires D3D12 compute"]
fn pyrowave_planes_hlg_matches_the_cpu_reference() -> Result<()> {
    use windows::Win32::Graphics::Direct3D12::{D3D12_FENCE_FLAG_NONE, ID3D12Fence};
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let compute = crate::compute::Compute::for_device(&gpu.device)?;
    // Grey at 0, 80, 203, 1000 and 2000 nits.
    let colours = [[0.; 3], [1.; 3], [203. / 80.; 3], [12.5; 3], [25.; 3]];
    let image = GpuImage::upload_settled(&gpu, &make_image(&colours, 64, 64))?;
    // SAFETY: `compute.device` is a live D3D12 device.
    let fence: ID3D12Fence = unsafe { compute.device.CreateFence(0, D3D12_FENCE_FLAG_NONE)? };
    let width = image.width as usize;
    for (value, yuv444) in [(1, false), (2, true)] {
        let config = butterpollo_media_policy::rtsp::Negotiated {
            width: image.width,
            height: image.height,
            codec: 3,
            hdr: true,
            // rubylight_protocol::hdr::HLG
            hdr_format: 2,
            yuv444,
            ..Default::default()
        };
        let source = (image.width, image.height, image.pixel);
        let outputs = PlanarConverter::new(&gpu, &config, source)?;
        let mut converter = crate::compute::Converter::new(
            compute.clone(),
            config.width,
            config.height,
            config.ten_bit(),
        )?;
        // As `ComputePlanes::convert`.
        converter.values = constants(&config, source, [100., 1.], None);
        converter.values[11] = u32::from(config.yuv444);
        let planes = crate::compute::Planes {
            planes: [
                compute.open(&outputs.textures[0])?,
                compute.open(&outputs.textures[1])?,
                compute.open(&outputs.textures[2])?,
            ],
            format: DXGI_FORMAT_R16_UNORM,
            fence: fence.clone(),
            after: value - 1,
            done: value,
        };
        converter.convert_planes(
            &compute.open(&image.texture)?,
            crate::compute::format(image.pixel),
            None,
            None,
            &planes,
            Some(&image),
        )?;
        let deadline = Instant::now() + std::time::Duration::from_secs(2);
        // SAFETY: `fence` is a live D3D12 fence.
        while unsafe { fence.GetCompletedValue() } < value {
            assert!(
                Instant::now() < deadline,
                "the compute conversion did not finish"
            );
            std::thread::yield_now();
        }
        let luma: Vec<u16> = plane_bytes(&gpu, &outputs.textures[0])?
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| (f64::from(u16::from_le_bytes(*pair)) * 1023. / 65535.).round() as u16)
            .collect();
        let code = |patch: usize| luma[32 * width + patch * 64 + 32];
        for (patch, colour) in colours.iter().enumerate() {
            let rgb = crate::color::hlg(colour.map(f64::from));
            let y = 0.2627 * rgb[0] + 0.6780 * rgb[1] + 0.0593 * rgb[2];
            let expected = (64. + 876. * y).round().clamp(0., 1023.);
            assert!(
                (f64::from(code(patch)) - expected).abs() <= 1.,
                "4:4:4 {yuv444} patch {patch}: luma {}, wanted {expected}",
                code(patch)
            );
        }
        // HLG's peak and BT.2408's reference white, in codes.
        assert!(code(3).abs_diff(940) <= 1, "4:4:4 {yuv444}: {}", code(3));
        assert!(code(2).abs_diff(721) <= 1, "4:4:4 {yuv444}: {}", code(2));
    }
    Ok(())
}

#[test]
#[ignore = "requires a D3D11 GPU with native P010 render views"]
fn gpu_hdr_preserves_absolute_luminance_gamut_and_linear_resize() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let image = make_image(
        &[
            [0.; 3],
            [1.; 3],
            [12.5; 3],
            [125.; 3],
            [1., 0., 0.],
            [0., 1., 0.],
            [0., 0., 1.],
            [-0.5, 1., 0.5],
        ],
        64,
        64,
    );
    let config = butterpollo_media_policy::rtsp::Negotiated {
        width: image.width,
        height: image.height,
        codec: 1,
        hdr: true,
        ..Default::default()
    };
    let mut convert = Converter::new(&gpu, &config, (image.width, image.height, image.pixel))?;
    let uploaded = GpuImage::upload(&gpu, &image)?;
    let output = readback(&gpu, convert.convert(&uploaded)?.as_ref())?;
    let mut reference = vec![];
    crate::color::hdr_rgba(&image, &mut reference);
    let width = image.width as usize;
    for patch in 0..8 {
        let x = patch * 64 + 32;
        let offset = (32 * width + x) * 8;
        let rgb: [f32; 3] = std::array::from_fn(|c| {
            u16::from_le_bytes([reference[offset + c * 2], reference[offset + c * 2 + 1]]) as f32
                / 65535.
        });
        let y = 0.2627 * rgb[0] + 0.6780 * rgb[1] + 0.0593 * rgb[2];
        let expected = [
            (64. + 876. * y).round(),
            (512. + 896. * (rgb[2] - y) / (2. * (1. - 0.0593))).round(),
            (512. + 896. * (rgb[0] - y) / (2. * (1. - 0.2627))).round(),
        ];
        let actual = [
            output[32 * width + x],
            output[width * image.height as usize + 16 * width + x],
            output[width * image.height as usize + 16 * width + x + 1],
        ];
        for c in 0..3 {
            assert!(
                (actual[c] as f32 - expected[c]).abs() <= 2.,
                "patch {patch} channel {c}: {} vs {}",
                actual[c],
                expected[c]
            );
        }
    }
    assert_eq!(output[32 * width + 32], 64);
    assert_eq!(output[32 * width + 3 * 64 + 32], 940);
    convert.set_luminance([100., 2.]);
    let expanded = readback(&gpu, convert.convert(&uploaded)?.as_ref())?;
    let expected = (64. + 876. * 0.82742465f32).round() as u16; // ST.2084 at 2000 nits
    assert!(expanded[32 * width + 2 * 64 + 32].abs_diff(expected) <= 2);
    let sdr = Image {
        width: 64,
        height: 64,
        stride: 256,
        bytes: vec![255; 64 * 256],
        captured: Instant::now(),
        pixel: Pixel::Bgra8,
    };
    let sdr = GpuImage::upload(&gpu, &sdr)?;
    let mut sdr_convert = Converter::new(
        &gpu,
        &butterpollo_media_policy::rtsp::Negotiated {
            width: 64,
            height: 64,
            hdr: true,
            codec: 1,
            ..Default::default()
        },
        (64, 64, Pixel::Bgra8),
    )?;
    for (white, pq) in [
        (100., 0.5080784f32),
        (200., 0.5791332f32),
        (480., 0.6721828f32),
    ] {
        sdr_convert.set_luminance([white, 1.]);
        let output = readback(&gpu, sdr_convert.convert(&sdr)?.as_ref())?;
        assert!(output[32 * 64 + 32].abs_diff((64. + 876. * pq).round() as u16) <= 2);
    }
    let resized = make_image(&[[0.; 3], [25.; 3], [0.; 3], [25.; 3]], 1, 4);
    let config = butterpollo_media_policy::rtsp::Negotiated {
        width: 2,
        height: 2,
        codec: 1,
        hdr: true,
        ..Default::default()
    };
    let mut convert = Converter::new(
        &gpu,
        &config,
        (resized.width, resized.height, resized.pixel),
    )?;
    let output = readback(
        &gpu,
        convert
            .convert(&GpuImage::upload(&gpu, &resized)?)?
            .as_ref(),
    )?;
    for value in &output[..4] {
        assert!(value.abs_diff(723) <= 1, "1000-nit linear average: {value}");
    }
    // A wider source is letterboxed: an 8x2 image fills one row of 4x4.
    let wide = make_image(&[[125.; 3]], 8, 2);
    let mut convert = Converter::new(
        &gpu,
        &butterpollo_media_policy::rtsp::Negotiated {
            width: 4,
            height: 4,
            codec: 1,
            hdr: true,
            ..Default::default()
        },
        (wide.width, wide.height, wide.pixel),
    )?;
    let output = readback(
        &gpu,
        convert.convert(&GpuImage::upload(&gpu, &wide)?)?.as_ref(),
    )?;
    for (row, picture) in [(0, false), (1, true), (2, false), (3, false)] {
        for value in &output[row * 4..row * 4 + 4] {
            assert_eq!(*value > 100, picture, "row {row}: {value}");
        }
    }
    Ok(())
}
#[test]
#[ignore = "requires a D3D11 GPU with native P010 render views"]
fn converters_take_their_contexts_lock_once_and_hold_it_per_conversion() -> Result<()> {
    use windows::core::{IUnknown, Interface};
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let context = gpu.context.cast::<IUnknown>()?;
    let config = butterpollo_media_policy::rtsp::Negotiated {
        width: 64,
        height: 64,
        codec: 1,
        hdr: true,
        ..Default::default()
    };
    let image = GpuImage::upload(&gpu, &make_image(&[[1.; 3]], 64, 64))?;
    let mut convert = Converter::new(&gpu, &config, (64, 64, Pixel::RgbaF16))?;
    assert_eq!(convert.lock.cast::<IUnknown>()?.as_raw(), context.as_raw());
    let mut planar = PlanarConverter::new(&gpu, &config, (64, 64, Pixel::RgbaF16))?;
    let planar_lock = planar.lock.cast::<IUnknown>()?;
    assert_eq!(planar_lock.as_raw(), context.as_raw());
    // Every conversion enters and leaves it, and converts the same frame alike each time.
    let first = readback(&gpu, convert.convert(&image)?.as_ref())?;
    for _ in 0..3 {
        planar.convert(&image, [100., 1.])?;
        assert_eq!(readback(&gpu, convert.convert(&image)?.as_ref())?, first);
    }
    Ok(())
}
#[test]
#[ignore = "requires a D3D11 GPU with native P010 render views"]
fn gpu_pool_does_not_overwrite_frames_held_by_the_codec() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let black = make_image(&[[0.; 3]], 64, 64);
    let white = make_image(&[[125.; 3]], 64, 64);
    let config = butterpollo_media_policy::rtsp::Negotiated {
        width: 64,
        height: 64,
        codec: 1,
        hdr: true,
        ..Default::default()
    };
    let mut convert = Converter::new(&gpu, &config, (64, 64, Pixel::RgbaF16))?;
    let black = GpuImage::upload(&gpu, &black)?;
    let white = GpuImage::upload(&gpu, &white)?;
    let first = convert.convert(&black)?;
    let mut held = vec![first.clone()];
    for _ in 0..7 {
        held.push(convert.convert(&white)?);
    }
    assert!(convert.convert(&white).is_err());
    assert!(readback(&gpu, &first)?[..64 * 64].iter().all(|y| *y == 64));
    held.pop();
    let next = convert.convert(&white)?;
    assert!(readback(&gpu, &next)?[..64 * 64].iter().all(|y| *y == 940));
    Ok(())
}

#[test]
fn every_d3d11_uav_fits_feature_level_11_0() {
    // The D3D11 device runs at feature level 11_0 (capture.rs), where a compute shader has
    // u0-u7: a UAV at u8 or above fails CreateComputeShader (E_INVALIDARG), as u9 did for
    // the HLG prepass. Only D3D12's root UAVs may sit higher.
    const D3D12_ONLY: [&str; 1] = ["frameWarp"];
    let mut seen = 0;
    for line in SHADER.lines() {
        let Some(at) = line.find(": register(u") else {
            continue;
        };
        let slot: u32 = line[at + 12..]
            .trim_end_matches(|c: char| !c.is_ascii_digit())
            .parse()
            .unwrap_or_else(|_| panic!("unreadable UAV register: {line}"));
        let name = line[..at].split_whitespace().last().unwrap();
        seen += 1;
        assert!(
            slot < 8 || D3D12_ONLY.contains(&name),
            "{name} at u{slot} is past feature level 11_0's u0-u7"
        );
    }
    assert!(seen >= 10, "found {seen} UAVs");
}

#[test]
fn sdr_white_scale_constants_follow_windows_nits() {
    let config = butterpollo_media_policy::rtsp::Negotiated {
        width: 1920,
        height: 1080,
        codec: 1,
        hdr: true,
        ..Default::default()
    };
    let c280 = constants(&config, (1920, 1080, Pixel::Bgra8), [280., 1.], None);
    let c480 = constants(&config, (1920, 1080, Pixel::Bgra8), [480., 1.], None);
    assert_eq!(c280[9], 3.5f32.to_bits());
    assert_eq!(c480[9], 6.0f32.to_bits());
}
