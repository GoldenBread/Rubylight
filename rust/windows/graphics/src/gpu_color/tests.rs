//! GPU color conversion test hub and shared readback/timing fixtures.

use super::*;
use crate::capture::{ComGuard, Image};
use std::time::Instant;

#[path = "color_contract_tests.rs"]
mod color_contract;
#[path = "downscale_tests.rs"]
mod downscale;
#[path = "tests/hdr_hlg_tests.rs"]
mod hdr_hlg;
#[path = "overlay_tests.rs"]
mod overlay;
#[path = "packed444_tests.rs"]
mod packed444;
#[path = "packed444_performance_tests.rs"]
mod packed444_performance;
#[path = "tests/single_pass_tests.rs"]
mod single_pass;
#[path = "tests/yuv444_tests.rs"]
mod yuv444;

#[test]
#[ignore = "requires D3D11 native P010 views and GPU timestamp queries"]
fn gpu_conversion_timing() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let image = make_image(&[[0.; 3], [1.; 3], [12.5; 3], [125.; 3]], 492, 2184);
    let source = GpuImage::upload(&gpu, &image)?;
    let timer = crate::timing::Timer::new()?;
    let query = |kind| -> Result<ID3D11Query> {
        let mut query = None;
        // SAFETY: `gpu.device` is a live D3D11 device; the descriptor and out-slot are live locals.
        unsafe {
            gpu.device.CreateQuery(
                &D3D11_QUERY_DESC {
                    Query: kind,
                    MiscFlags: 0,
                },
                Some(&mut query),
            )?;
        }
        query.context("no timestamp query")
    };
    let mut reports = vec![];
    for (width, height) in [(1968, 2184), (3840, 2160)] {
        let config = butterpollo_media_policy::rtsp::Negotiated {
            width,
            height,
            codec: 1,
            hdr: true,
            ..Default::default()
        };
        let preparation = Instant::now();
        let mut converter =
            Converter::new(&gpu, &config, (image.width, image.height, image.pixel))?;
        let preparation_ms = preparation.elapsed().as_secs_f64() * 1000.;
        for _ in 0..16 {
            converter.convert(&source)?;
        }
        let disjoint = query(D3D11_QUERY_TIMESTAMP_DISJOINT)?;
        let queries: Vec<_> = (0..64)
            .map(|_| Ok((query(D3D11_QUERY_TIMESTAMP)?, query(D3D11_QUERY_TIMESTAMP)?)))
            .collect::<Result<_>>()?;
        let mut calls = vec![];
        // SAFETY: `disjoint` is a live query made on this device's context.
        unsafe {
            gpu.context.Begin(&disjoint);
        }
        for (start, end) in &queries {
            // SAFETY: `start` is a live timestamp query made on this device.
            unsafe {
                gpu.context.End(start);
            }
            let begin = Instant::now();
            converter.convert(&source)?;
            calls.push(begin.elapsed().as_secs_f64() * 1000.);
            // SAFETY: `end` is a live timestamp query made on this device.
            unsafe {
                gpu.context.End(end);
            }
        }
        // SAFETY: `disjoint` is a live query made on this device's context.
        unsafe {
            gpu.context.End(&disjoint);
            gpu.context.Flush();
        }
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        let mut clock = D3D11_QUERY_DATA_TIMESTAMP_DISJOINT::default();
        loop {
            // SAFETY: `clock` is a live local of exactly the size passed, the size the disjoint
            // query writes.
            unsafe {
                gpu.context.GetData(
                    &disjoint,
                    Some((&mut clock as *mut D3D11_QUERY_DATA_TIMESTAMP_DISJOINT).cast()),
                    std::mem::size_of_val(&clock) as u32,
                    D3D11_ASYNC_GETDATA_DONOTFLUSH.0 as u32,
                )?;
            }
            if clock.Frequency != 0 {
                break;
            }
            if Instant::now() >= deadline {
                bail!("GPU timestamp clock timed out");
            }
            timer.until(Instant::now() + std::time::Duration::from_micros(250));
        }
        if clock.Disjoint.as_bool() {
            bail!("GPU clock changed during conversion benchmark");
        }
        let mut durations = vec![];
        for (start, end) in queries {
            let mut ticks = [0u64; 2];
            for (query, value) in [start, end].iter().zip(ticks.iter_mut()) {
                loop {
                    // SAFETY: `value` is a live u64, the size a timestamp query writes.
                    unsafe {
                        gpu.context.GetData(
                            query,
                            Some((value as *mut u64).cast()),
                            std::mem::size_of::<u64>() as u32,
                            D3D11_ASYNC_GETDATA_DONOTFLUSH.0 as u32,
                        )?;
                    }
                    if *value != 0 {
                        break;
                    }
                    if Instant::now() >= deadline {
                        bail!("GPU timestamp result timed out");
                    }
                    timer.until(Instant::now() + std::time::Duration::from_micros(250));
                }
            }
            durations.push((ticks[1] - ticks[0]) as f64 * 1000. / clock.Frequency as f64);
        }
        let stats = |mut values: Vec<f64>| {
            values.sort_by(f64::total_cmp);
            serde_json::json!({"samples":values.len(),"mean_ms":values.iter().sum::<f64>() / values.len() as f64,"p95_ms":values[(values.len()-1)*95/100],"p99_ms":values[(values.len()-1)*99/100],"max_ms":values.last()})
        };
        reports.push(serde_json::json!({"width":width,"height":height,"preparation_ms":preparation_ms,"gpu":stats(durations),"cpu_call":stats(calls)}));
    }
    let report = serde_json::json!({"scope":"isolated FP16-to-P010 conversion, excludes capture, encoder, network and decoder", "source_width":image.width,"source_height":image.height,"adapter":gpu.display.adapter,"cases":reports});
    eprintln!("{report}");
    if let Some(path) = std::env::var_os("BUTTERPOLLO_TEST_GPU_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&report)?)?;
    }
    Ok(())
}

fn make_image(colors: &[[f32; 3]], patch: usize, height: usize) -> Image {
    let width = colors.len() * patch;
    let mut bytes = vec![0; width * height * 8];
    for (index, pixel) in bytes.as_chunks_mut::<8>().0.iter_mut().enumerate() {
        for (channel, value) in colors[(index % width) / patch].iter().enumerate() {
            pixel[channel * 2..channel * 2 + 2]
                .copy_from_slice(&half::f16::from_f32(*value).to_bits().to_le_bytes());
        }
        pixel[6..].copy_from_slice(&half::f16::ONE.to_bits().to_le_bytes());
    }
    Image {
        width: width as u32,
        height: height as u32,
        stride: width * 8,
        bytes,
        pixel: Pixel::RgbaF16,
        captured: Instant::now(),
    }
}
fn readback(gpu: &Device, texture: &ID3D11Texture2D) -> Result<Vec<u16>> {
    // SAFETY: `staging` is a CPU-readable copy of `texture`; each row read lies within the mapped
    // planes (P010 chroma follows luma) and stays valid until Unmap below.
    unsafe {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        texture.GetDesc(&mut desc);
        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        desc.MiscFlags = 0;
        let mut staging = None;
        gpu.device
            .CreateTexture2D(&desc, None, Some(&mut staging))?;
        let staging = staging.unwrap();
        gpu.context.CopyResource(&staging, texture);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        gpu.context
            .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        let mut values = Vec::new();
        let rows = if desc.Format == DXGI_FORMAT_P010 {
            desc.Height as usize * 3 / 2
        } else {
            desc.Height as usize
        };
        for y in 0..rows {
            let row = std::slice::from_raw_parts(
                (mapped.pData as *const u8).add(y * mapped.RowPitch as usize),
                desc.Width as usize * 2,
            );
            values.extend(
                row.as_chunks::<2>()
                    .0
                    .iter()
                    .map(|v| u16::from_le_bytes(*v) >> 6),
            );
        }
        gpu.context.Unmap(&staging, 0);
        Ok(values)
    }
}
/// A P010 texture on the compute device, as `readback` gives it.
fn readback_compute(
    compute: &crate::compute::Compute,
    texture: &windows::Win32::Graphics::Direct3D12::ID3D12Resource,
) -> Result<Vec<u16>> {
    let (samples, ten_bit) = compute_samples(compute, texture)?;
    Ok(if ten_bit {
        samples.into_iter().map(|v| v >> 6).collect()
    } else {
        samples
    })
}
/// An NV12 or P010 texture on the compute device, both planes row by row
/// as stored, and whether it is P010.
fn compute_samples(
    compute: &crate::compute::Compute,
    texture: &windows::Win32::Graphics::Direct3D12::ID3D12Resource,
) -> Result<(Vec<u16>, bool)> {
    use windows::Win32::Graphics::Direct3D12::*;
    // SAFETY: `buffer` is sized by GetCopyableFootprints for both planes, the copy is waited for
    // before Map, and each row read stays within its footprint until Unmap below.
    unsafe {
        let desc = texture.GetDesc();
        let pixel_bytes = if desc.Format == DXGI_FORMAT_NV12 {
            1
        } else {
            2
        };
        let mut layouts = [D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default(); 2];
        let mut total = 0u64;
        compute.device.GetCopyableFootprints(
            &desc,
            0,
            2,
            0,
            Some(layouts.as_mut_ptr()),
            None,
            None,
            Some(&mut total),
        );
        let mut buffer: Option<ID3D12Resource> = None;
        compute.device.CreateCommittedResource(
            &D3D12_HEAP_PROPERTIES {
                Type: D3D12_HEAP_TYPE_READBACK,
                ..Default::default()
            },
            D3D12_HEAP_FLAG_NONE,
            &D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
                Width: total,
                Height: 1,
                DepthOrArraySize: 1,
                MipLevels: 1,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
                ..Default::default()
            },
            D3D12_RESOURCE_STATE_COPY_DEST,
            None,
            &mut buffer,
        )?;
        let buffer = buffer.unwrap();
        let allocator: ID3D12CommandAllocator = compute
            .device
            .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_COMPUTE)?;
        let list: ID3D12GraphicsCommandList = compute.device.CreateCommandList(
            0,
            D3D12_COMMAND_LIST_TYPE_COMPUTE,
            &allocator,
            None,
        )?;
        for (plane, layout) in layouts.iter().enumerate() {
            let destination = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::ManuallyDrop::new(Some(buffer.clone())),
                Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                    PlacedFootprint: *layout,
                },
            };
            let source = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::ManuallyDrop::new(Some(texture.clone())),
                Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                    SubresourceIndex: plane as u32,
                },
            };
            list.CopyTextureRegion(&destination, 0, 0, 0, &source, None);
            drop(std::mem::ManuallyDrop::into_inner(destination.pResource));
            drop(std::mem::ManuallyDrop::into_inner(source.pResource));
        }
        list.Close()?;
        compute.wait(compute.execute(&list)?)?;
        let mut mapped = std::ptr::null_mut();
        buffer.Map(0, None, Some(&mut mapped))?;
        let mut values = Vec::new();
        for (plane, layout) in layouts.iter().enumerate() {
            let rows = if plane == 0 {
                desc.Height as usize
            } else {
                desc.Height as usize / 2
            };
            for y in 0..rows {
                let row = std::slice::from_raw_parts(
                    mapped
                        .cast::<u8>()
                        .add(layout.Offset as usize + y * layout.Footprint.RowPitch as usize),
                    desc.Width as usize * pixel_bytes,
                );
                if pixel_bytes == 1 {
                    values.extend(row.iter().map(|&v| u16::from(v)));
                } else {
                    values.extend(
                        row.as_chunks::<2>()
                            .0
                            .iter()
                            .map(|v| u16::from_le_bytes(*v)),
                    );
                }
            }
        }
        buffer.Unmap(0, None);
        Ok((values, pixel_bytes == 2))
    }
}
#[test]
#[ignore = "requires D3D12 compute and native P010 conversion"]
fn compute_conversion_matches_the_graphics_converter() -> Result<()> {
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
    // Desktop BGRA: a gradient with distinct channels.
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
    // Without a pointer, a monochrome (replace and XOR) pointer, and one
    // hanging off the left edge.
    let pointer = crate::cursor::Cursor::new(
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
    let mut placed = pointer.clone();
    placed.position = [20, 10];
    let mut clipped = pointer.clone();
    clipped.position = [-2, 30];
    // The same size, a downscale and a letterboxed shape.
    for (width, height) in [(96, 40), (48, 20), (64, 64)] {
        for (hdr, base, csc_mode) in [
            (true, &hdr_image, 2),
            (false, &sdr_image, 2),
            (false, &sdr_image, 4),
        ] {
            for cursor in [None, Some(&placed), Some(&clipped)] {
                let mut image = base.clone();
                image.cursor = cursor.cloned();
                let image = &image;
                let config = butterpollo_media_policy::rtsp::Negotiated {
                    width,
                    height,
                    codec: 1,
                    hdr,
                    sdr_10bit: !hdr,
                    csc_mode,
                    ..Default::default()
                };
                let mut graphics = Converter::new(&gpu, &config, (96, 40, image.pixel))?;
                graphics.compute = None;
                graphics.set_luminance([100., 1.]);
                let expected = readback(&gpu, graphics.convert(image)?.as_ref())?;
                let mut converter =
                    crate::compute::Converter::new(compute.clone(), width, height, true)?;
                converter.values = constants(
                    &config,
                    (image.width, image.height, image.pixel),
                    [100., 1.],
                    image.cursor.as_ref(),
                );
                let shape = cursor.map(|c| converter.pointer(c)).transpose()?;
                let source = compute.open(&image.texture)?;
                let converted = converter.convert(
                    &source,
                    crate::compute::format(image.pixel),
                    shape.as_ref(),
                    image,
                )?;
                converted.wait()?;
                let actual = readback_compute(&compute, &converted.texture)?;
                assert_eq!(actual.len(), expected.len());
                for (index, (a, e)) in actual.iter().zip(&expected).enumerate() {
                    assert!(
                        a.abs_diff(*e) <= 1,
                        "{width}x{height} hdr {hdr} pointer {:?}: value {index} is {a}, the graphics converter gives {e}",
                        cursor.map(|c| c.position)
                    );
                }
            }
        }
    }
    Ok(())
}
/// A one-plane texture's bytes, row by row without padding.
fn plane_bytes(gpu: &Device, texture: &ID3D11Texture2D) -> Result<Vec<u8>> {
    // SAFETY: `staging` is a CPU-readable copy of `texture` and each row read is `Width * pixel`
    // bytes within `RowPitch`, valid until Unmap below.
    unsafe {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        texture.GetDesc(&mut desc);
        let pixel = if matches!(desc.Format, DXGI_FORMAT_R16_UNORM | DXGI_FORMAT_P010) {
            2
        } else {
            1
        };
        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        desc.MiscFlags = 0;
        let mut staging = None;
        gpu.device
            .CreateTexture2D(&desc, None, Some(&mut staging))?;
        let staging = staging.unwrap();
        gpu.context.CopyResource(&staging, texture);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        gpu.context
            .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        let mut bytes = Vec::new();
        let rows = if matches!(desc.Format, DXGI_FORMAT_NV12 | DXGI_FORMAT_P010) {
            desc.Height * 3 / 2
        } else {
            desc.Height
        };
        for y in 0..rows as usize {
            bytes.extend_from_slice(std::slice::from_raw_parts(
                (mapped.pData as *const u8).add(y * mapped.RowPitch as usize),
                desc.Width as usize * pixel,
            ));
        }
        gpu.context.Unmap(&staging, 0);
        Ok(bytes)
    }
}

fn time_conversion(
    gpu: &Device,
    mut convert: impl FnMut() -> Result<()>,
) -> Result<serde_json::Value> {
    let query = |kind| -> Result<ID3D11Query> {
        let mut query = None;
        // SAFETY: The device and the descriptor/out-slot are live.
        unsafe {
            gpu.device.CreateQuery(
                &D3D11_QUERY_DESC {
                    Query: kind,
                    MiscFlags: 0,
                },
                Some(&mut query),
            )?;
        }
        query.context("no timestamp query")
    };
    for _ in 0..32 {
        convert()?;
    }
    let disjoint = query(D3D11_QUERY_TIMESTAMP_DISJOINT)?;
    let queries: Vec<_> = (0..128)
        .map(|_| Ok((query(D3D11_QUERY_TIMESTAMP)?, query(D3D11_QUERY_TIMESTAMP)?)))
        .collect::<Result<_>>()?;
    let mut calls = Vec::new();
    // SAFETY: Every query belongs to this context; GetData writes the exact-sized live locals.
    unsafe {
        gpu.context.Begin(&disjoint);
        for (start, end) in &queries {
            gpu.context.End(start);
            let begin = Instant::now();
            convert()?;
            calls.push(begin.elapsed().as_secs_f64() * 1000.);
            gpu.context.End(end);
        }
        gpu.context.End(&disjoint);
        gpu.context.Flush();
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        let mut clock = D3D11_QUERY_DATA_TIMESTAMP_DISJOINT::default();
        while clock.Frequency == 0 {
            gpu.context.GetData(
                &disjoint,
                Some((&mut clock as *mut D3D11_QUERY_DATA_TIMESTAMP_DISJOINT).cast()),
                std::mem::size_of_val(&clock) as u32,
                D3D11_ASYNC_GETDATA_DONOTFLUSH.0 as u32,
            )?;
            if Instant::now() >= deadline {
                bail!("GPU clock timed out");
            }
            std::thread::yield_now();
        }
        if clock.Disjoint.as_bool() {
            bail!("GPU clock changed during benchmark");
        }
        let mut gpu_ms = Vec::new();
        for (start, end) in queries {
            let mut ticks = [0u64; 2];
            for (query, value) in [start, end].iter().zip(&mut ticks) {
                while *value == 0 {
                    gpu.context.GetData(
                        query,
                        Some((value as *mut u64).cast()),
                        8,
                        D3D11_ASYNC_GETDATA_DONOTFLUSH.0 as u32,
                    )?;
                    if Instant::now() >= deadline {
                        bail!("GPU timestamp timed out");
                    }
                    std::thread::yield_now();
                }
            }
            gpu_ms.push((ticks[1] - ticks[0]) as f64 / clock.Frequency as f64 * 1000.);
        }
        let stats = |mut values: Vec<f64>| {
            values.sort_by(f64::total_cmp);
            serde_json::json!({"mean_ms":values.iter().sum::<f64>() / values.len() as f64,"p50_ms":values[(values.len()-1)/2],"p95_ms":values[(values.len()-1)*95/100],"max_ms":values.last()})
        };
        Ok(serde_json::json!({"gpu":stats(gpu_ms),"cpu_call":stats(calls)}))
    }
}

/// Pure red's Y, Cb and Cr codes under BT.601 (`csc_mode` 0 and 1) or
/// BT.709 (2 and 3), limited range for even modes, at `bits` bits. The
/// reference the native 4:4:4 tests below check the shader against.
fn red_codes(csc_mode: u8, bits: u32) -> [u16; 3] {
    let (kr, kb) = if csc_mode < 2 {
        (0.299, 0.114)
    } else {
        (0.2126, 0.0722)
    };
    let (y, cb, cr) = (kr, -kr / (2. * (1. - kb)), 0.5);
    let scale = f64::from(1u32 << (bits - 8));
    let max = f64::from((1u32 << bits) - 1);
    let mid = 128. * scale;
    let (black, luma_range, chroma_range) = if csc_mode.is_multiple_of(2) {
        (16. * scale, 219. * scale, 224. * scale)
    } else {
        (0., max, max)
    };
    let y = black + luma_range * y;
    let cb = mid + chroma_range * cb;
    let cr = mid + chroma_range * cr;
    [y, cb, cr].map(|code: f64| code.round().clamp(0., max) as u16)
}

/// An AYUV texture's pixels as `[V, U, Y, A]` bytes, its memory order.
fn readback_ayuv(gpu: &Device, texture: &ID3D11Texture2D) -> Result<Vec<[u8; 4]>> {
    // SAFETY: `staging` is a CPU-readable copy of `texture`; each row read is `Width * 4` bytes
    // within `RowPitch` and stays valid until Unmap below.
    unsafe {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        texture.GetDesc(&mut desc);
        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        desc.MiscFlags = 0;
        let mut staging = None;
        gpu.device
            .CreateTexture2D(&desc, None, Some(&mut staging))?;
        let staging = staging.unwrap();
        gpu.context.CopyResource(&staging, texture);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        gpu.context
            .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        let mut pixels = Vec::new();
        for y in 0..desc.Height as usize {
            let row = std::slice::from_raw_parts(
                (mapped.pData as *const u8).add(y * mapped.RowPitch as usize),
                desc.Width as usize * 4,
            );
            pixels.extend(row.as_chunks::<4>().0.iter().copied());
        }
        gpu.context.Unmap(&staging, 0);
        Ok(pixels)
    }
}
