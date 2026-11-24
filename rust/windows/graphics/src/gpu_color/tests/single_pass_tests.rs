//! Single-pass D3D11 compute 4:2:0 and PyroWave planar conversion tests.

use super::*;

#[test]
#[ignore = "requires native D3D11 video UAVs"]
fn gpu_420_single_pass_matches_pixel_passes() -> Result<()> {
    use windows::Win32::Graphics::Dxgi::{
        DXGI_OUTDUPL_POINTER_SHAPE_INFO, DXGI_OUTDUPL_POINTER_SHAPE_TYPE_COLOR,
        DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME,
    };
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let hdr_image = GpuImage::upload(
        &gpu,
        &make_image(
            &[
                [0.; 3],
                [1.; 3],
                [12.5; 3],
                [1.2, 0.1, 0.05],
                [0.05, 0.8, 0.3],
                [0.2, 0.3, 4.],
            ],
            16,
            40,
        ),
    )?;
    let mut bytes = vec![0u8; 96 * 40 * 4];
    for (index, pixel) in bytes.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (x, y) = (index % 96, index / 96);
        *pixel = [(x * 2) as u8, (y * 6) as u8, (255 - x * 2) as u8, 255];
    }
    let sdr_image = GpuImage::upload(
        &gpu,
        &Image {
            width: 96,
            height: 40,
            stride: 96 * 4,
            bytes,
            pixel: Pixel::Bgra8,
            captured: Instant::now(),
        },
    )?;
    let mut pointer = crate::cursor::Cursor::new(
        &gpu,
        &DXGI_OUTDUPL_POINTER_SHAPE_INFO {
            Type: DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME.0 as u32,
            Width: 4,
            Height: 2,
            Pitch: 1,
            ..Default::default()
        },
        &[0b00110000, 0b01010000],
    )?;
    pointer.position = [20, 10];
    let mut clipped = pointer.clone();
    clipped.position = [-2, 30];
    let mut color = crate::cursor::Cursor::new(
        &gpu,
        &DXGI_OUTDUPL_POINTER_SHAPE_INFO {
            Type: DXGI_OUTDUPL_POINTER_SHAPE_TYPE_COLOR.0 as u32,
            Width: 2,
            Height: 2,
            Pitch: 8,
            ..Default::default()
        },
        &[0, 0, 255, 255, 0, 128, 0, 128, 255, 0, 0, 255, 0, 0, 0, 0],
    )?;
    color.position = [7, 7];
    let mut cases = 0;
    for (width, height) in [(96, 40), (48, 20), (64, 64), (130, 70)] {
        for (hdr, ten_bit, source) in [
            (false, false, &sdr_image),
            (false, true, &sdr_image),
            (false, false, &hdr_image),
            (false, true, &hdr_image),
            (true, true, &hdr_image),
        ] {
            for csc_mode in [0, 1, 2, 3, 4, 5] {
                for cursor in [None, Some(&pointer), Some(&clipped), Some(&color)] {
                    let mut image = source.clone();
                    image.cursor = cursor.cloned();
                    let config = butterpollo_media_policy::rtsp::Negotiated {
                        width,
                        height,
                        codec: 1,
                        hdr,
                        sdr_10bit: ten_bit && !hdr,
                        csc_mode,
                        ..Default::default()
                    };
                    let source = (image.width, image.height, image.pixel);
                    let mut converter = Converter::new(&gpu, &config, source)?;
                    let output = converter.convert(&image)?;
                    assert!(
                        converter.compute.is_some(),
                        "video UAV conversion is unavailable"
                    );
                    let actual = plane_bytes(&gpu, &output)?;
                    converter.compute = None;
                    let expected = plane_bytes(&gpu, converter.convert(&image)?.as_ref())?;
                    assert!(
                        actual == expected,
                        "video {width}x{height} hdr {hdr} ten_bit {ten_bit} csc {csc_mode} cursor {:?}: first mismatch {:?}",
                        cursor.map(|c| c.position),
                        actual
                            .iter()
                            .zip(&expected)
                            .enumerate()
                            .find(|(_, (a, b))| a != b)
                    );
                    let mut planar = PlanarConverter::new(&gpu, &config, source)?;
                    assert!(
                        planar.compute.is_some(),
                        "planar UAV conversion is unavailable"
                    );
                    planar.convert(&image, [100., 1.])?;
                    let actual = planar
                        .textures
                        .iter()
                        .map(|p| plane_bytes(&gpu, p))
                        .collect::<Result<Vec<_>>>()?;
                    planar.compute = None;
                    planar.convert(&image, [100., 1.])?;
                    let expected = planar
                        .textures
                        .iter()
                        .map(|p| plane_bytes(&gpu, p))
                        .collect::<Result<Vec<_>>>()?;
                    assert!(
                        actual == expected,
                        "planar {width}x{height} hdr {hdr} ten_bit {ten_bit} csc {csc_mode} cursor {:?}",
                        cursor.map(|c| c.position)
                    );
                    cases += 1;
                }
            }
        }
    }
    eprintln!("D3D11 single-pass parity: {cases} video and {cases} planar cases");
    Ok(())
}

#[test]
#[ignore = "requires native D3D11 video UAVs and an idle GPU"]
fn gpu_420_single_pass_timing() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let image = make_image(&[[0.; 3], [1.; 3], [12.5; 3], [125.; 3]], 960, 2160);
    let hdr_source = GpuImage::upload(&gpu, &image)?;
    let mut sdr = Image {
        width: image.width,
        height: image.height,
        stride: image.width as usize * 4,
        bytes: vec![0; image.width as usize * image.height as usize * 4],
        pixel: Pixel::Bgra8,
        captured: Instant::now(),
    };
    for (index, pixel) in sdr.bytes.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        *pixel = [
            (index % 256) as u8,
            ((index / image.width as usize) % 256) as u8,
            (255 - index % 256) as u8,
            255,
        ];
    }
    let sdr_source = GpuImage::upload(&gpu, &sdr)?;
    let mut reports = Vec::new();
    for (width, height) in [(1920, 1080), (2560, 1440), (3840, 2160)] {
        for hdr in [false, true] {
            let config = butterpollo_media_policy::rtsp::Negotiated {
                width,
                height,
                codec: 1,
                hdr,
                ..Default::default()
            };
            let source = if hdr { &hdr_source } else { &sdr_source };
            let shape = (source.width, source.height, source.pixel);
            let mut single = Converter::new(&gpu, &config, shape)?;
            let mut baseline = Converter::new(&gpu, &config, shape)?;
            baseline.compute = None;
            single.convert(source)?;
            assert!(single.compute.is_some());
            let mut single_planar = PlanarConverter::new(&gpu, &config, shape)?;
            let mut baseline_planar = PlanarConverter::new(&gpu, &config, shape)?;
            baseline_planar.compute = None;
            assert!(single_planar.compute.is_some());
            for planar in [false, true] {
                let mut runs = Vec::new();
                for compute in [false, true, true, false] {
                    let stats = time_conversion(&gpu, || {
                        if planar {
                            let converter = if compute {
                                &mut single_planar
                            } else {
                                &mut baseline_planar
                            };
                            converter.convert(source, [100., 1.])?;
                        } else {
                            let converter = if compute { &mut single } else { &mut baseline };
                            converter.convert(source)?;
                        }
                        Ok(())
                    })?;
                    runs.push(serde_json::json!({"single_pass":compute,"stats":stats}));
                }
                reports.push(serde_json::json!({"width":width,"height":height,"hdr":hdr,"planar":planar,"runs":runs}));
            }
        }
    }
    let report = serde_json::json!({"scope":"D3D11 fallback conversion; four ABBA runs per case, 128 GPU timestamps each; excludes capture/encoding/network/decoding", "adapter":gpu.display.adapter,"cases":reports});
    eprintln!("{report}");
    if let Some(path) = std::env::var_os("BUTTERPOLLO_TEST_GPU_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&report)?)?;
    }
    Ok(())
}

#[test]
#[ignore = "requires the native D3D11 debug layer and video UAVs"]
fn gpu_420_video_plane_uavs_are_disjoint() -> Result<()> {
    use windows::{
        Win32::{
            Foundation::HMODULE,
            Graphics::{Direct3D::D3D_DRIVER_TYPE_UNKNOWN, Dxgi::IDXGIDevice},
        },
        core::Interface,
    };
    let _com = ComGuard::new()?;
    let mut gpu = Device::new("")?;
    // SAFETY: The adapter is owned, every descriptor/out-slot is live, and query buffers are
    // aligned and sized from GetMessage before their contents are read.
    unsafe {
        let adapter = gpu.device.cast::<IDXGIDevice>()?.GetAdapter()?;
        let mut device = None;
        let mut context = None;
        D3D11CreateDevice(
            &adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_DEBUG,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
        .context("native D3D11 debug device unavailable")?;
        gpu.device = device.unwrap();
        gpu.context = context.unwrap();
        let _ = gpu
            .context
            .cast::<ID3D11Multithread>()?
            .SetMultithreadProtected(true);
        let messages: ID3D11InfoQueue = gpu.device.cast()?;
        for hdr in [false, true] {
            let config = butterpollo_media_policy::rtsp::Negotiated {
                width: 64,
                height: 64,
                codec: 1,
                hdr,
                ..Default::default()
            };
            let mut converter = Converter::new(&gpu, &config, (64, 64, Pixel::Bgra8))?;
            let index = converter.target()?;
            let views = converter.targets[index]
                .unordered
                .as_ref()
                .and_then(|(y, uv)| Some([y, uv.as_ref()?]))
                .context("native video UAVs unavailable")?;
            messages.ClearStoredMessages();
            let bound_in = views.map(|v| Some(v.clone()));
            gpu.context
                .CSSetUnorderedAccessViews(0, 2, Some(bound_in.as_ptr()), None);
            let mut bound = [None, None];
            gpu.context.CSGetUnorderedAccessViews(0, Some(&mut bound));
            gpu.context
                .CSSetUnorderedAccessViews(0, 2, Some([None, None].as_ptr()), None);
            for index in 0..messages.GetNumStoredMessages() {
                let mut size = 0;
                messages.GetMessage(index, None, &mut size)?;
                let mut storage = vec![0u64; size.div_ceil(8)];
                let message = storage.as_mut_ptr().cast::<D3D11_MESSAGE>();
                messages.GetMessage(index, Some(message), &mut size)?;
                let message = &*message;
                assert!(
                    message.Severity.0 > D3D11_MESSAGE_SEVERITY_WARNING.0,
                    "video UAV binding hdr {hdr}: {}",
                    String::from_utf8_lossy(std::slice::from_raw_parts(
                        message.pDescription,
                        message.DescriptionByteLength
                    ))
                );
            }
            for (actual, expected) in bound.iter().zip(views) {
                assert!(
                    actual.as_ref() == Some(expected),
                    "video UAV binding was rejected for hdr {hdr}"
                );
            }
        }
    }
    Ok(())
}

#[test]
#[ignore = "requires D3D12 compute"]
fn compute_planes_match_the_pyrowave_graphics_planes() -> Result<()> {
    use windows::Win32::Graphics::Direct3D12::{D3D12_FENCE_FLAG_NONE, ID3D12Fence};
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let compute = crate::compute::Compute::for_device(&gpu.device)?;
    let colors = [
        [0., 0., 0.],
        [1., 1., 1.],
        [12.5, 12.5, 12.5],
        [1.2, 0.1, 0.05],
        [0.05, 0.8, 0.3],
        [0.2, 0.3, 4.0],
    ];
    let hdr_image = GpuImage::upload_settled(&gpu, &make_image(&colors, 16, 40))?;
    let mut bytes = vec![0u8; 96 * 40 * 4];
    for (index, pixel) in bytes.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (x, y) = (index % 96, index / 96);
        *pixel = [(x * 2) as u8, (y * 6) as u8, (255 - x * 2) as u8, 255];
    }
    let sdr_image = GpuImage::upload_settled(
        &gpu,
        &Image {
            width: 96,
            height: 40,
            stride: 96 * 4,
            bytes,
            pixel: Pixel::Bgra8,
            captured: Instant::now(),
        },
    )?;
    let mut pointer = crate::cursor::Cursor::new(
        &gpu,
        &windows::Win32::Graphics::Dxgi::DXGI_OUTDUPL_POINTER_SHAPE_INFO {
            Type: windows::Win32::Graphics::Dxgi::DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME.0
                as u32,
            Width: 4,
            Height: 2,
            Pitch: 1,
            ..Default::default()
        },
        &[0b00110000, 0b01010000],
    )?;
    pointer.position = [20, 10];
    // SAFETY: `compute.device` is a live D3D12 device.
    let fence: ID3D12Fence = unsafe { compute.device.CreateFence(0, D3D12_FENCE_FLAG_NONE)? };
    let mut value = 0;
    for (width, height) in [(96, 40), (48, 20), (64, 64)] {
        for yuv444 in [true, false] {
            for (hdr, sdr_10bit, base, csc_mode) in [
                (true, false, &hdr_image, 2),
                (false, false, &sdr_image, 2),
                (false, true, &sdr_image, 2),
                (false, true, &sdr_image, 4),
            ] {
                for cursor in [None, Some(&pointer)] {
                    let mut image = base.clone();
                    image.cursor = cursor.cloned();
                    let config = butterpollo_media_policy::rtsp::Negotiated {
                        width,
                        height,
                        codec: 3,
                        hdr,
                        yuv444,
                        sdr_10bit,
                        csc_mode,
                        ..Default::default()
                    };
                    let source = (image.width, image.height, image.pixel);
                    let mut graphics = PlanarConverter::new(&gpu, &config, source)?;
                    graphics.compute = None;
                    graphics.convert(&image, [100., 1.])?;
                    let expected = graphics
                        .textures
                        .iter()
                        .map(|plane| plane_bytes(&gpu, plane))
                        .collect::<Result<Vec<_>>>()?;
                    let mut single_pass = PlanarConverter::new(&gpu, &config, source)?;
                    assert!(single_pass.compute.is_some(), "no D3D11 planar shader");
                    single_pass.convert(&image, [100., 1.])?;
                    for (texture, expected) in single_pass.textures.iter().zip(&expected) {
                        assert_eq!(
                            &plane_bytes(&gpu, texture)?,
                            expected,
                            "D3D11 compute and graphics differ: {width}x{height} 4:4:4 {yuv444} hdr {hdr} ten_bit {} cursor {}",
                            config.ten_bit(),
                            cursor.is_some(),
                        );
                    }
                    if yuv444 {
                        graphics.use_combined = false;
                        graphics.convert(&image, [100., 1.])?;
                        for (texture, expected) in graphics.textures.iter().zip(&expected) {
                            assert_eq!(
                                &plane_bytes(&gpu, texture)?,
                                expected,
                                "MRT and separate graphics passes differ"
                            );
                        }
                    }
                    // Fresh planes filled with a sentinel, so nothing left
                    // from the graphics pass can match.
                    let outputs = PlanarConverter::new(&gpu, &config, source)?;
                    for plane in &outputs.textures {
                        let mut desc = D3D11_TEXTURE2D_DESC::default();
                        // SAFETY: `plane` is a live texture and `desc` a live local.
                        unsafe { plane.GetDesc(&mut desc) };
                        let pitch = desc.Width * if config.ten_bit() { 2 } else { 1 };
                        let fill = vec![0xab_u8; (pitch * desc.Height) as usize];
                        // SAFETY: `fill` holds `pitch * Height` bytes, exactly one plane at the
                        // pitch passed.
                        unsafe {
                            gpu.context.UpdateSubresource(
                                plane.as_ref(),
                                0,
                                None,
                                fill.as_ptr().cast(),
                                pitch,
                                0,
                            );
                            gpu.context.Flush();
                        }
                    }
                    let mut converter = crate::compute::Converter::new(
                        compute.clone(),
                        width,
                        height,
                        config.ten_bit(),
                    )?;
                    converter.values = constants(&config, source, [100., 1.], cursor);
                    converter.values[11] = u32::from(yuv444);
                    let shape = cursor.map(|c| converter.pointer(c)).transpose()?;
                    let planes = crate::compute::Planes {
                        planes: [
                            compute.open(&outputs.textures[0])?,
                            compute.open(&outputs.textures[1])?,
                            compute.open(&outputs.textures[2])?,
                        ],
                        format: if config.ten_bit() {
                            DXGI_FORMAT_R16_UNORM
                        } else {
                            DXGI_FORMAT_R8_UNORM
                        },
                        fence: fence.clone(),
                        after: value,
                        done: value + 1,
                    };
                    value += 1;
                    converter.convert_planes(
                        &compute.open(&image.texture)?,
                        crate::compute::format(image.pixel),
                        shape.as_ref(),
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
                    for (plane, (texture, expected)) in
                        outputs.textures.iter().zip(&expected).enumerate()
                    {
                        assert_eq!(
                            &plane_bytes(&gpu, texture)?,
                            expected,
                            "{width}x{height} 4:4:4 {yuv444} hdr {hdr} pointer {}: plane {plane}",
                            cursor.is_some()
                        );
                    }
                }
            }
        }
    }
    Ok(())
}
