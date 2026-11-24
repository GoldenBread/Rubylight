//! Unit tests for the D3D12 compute 8-bit native 4:4:4 (AYUV) layout and shaders.

use super::*;

#[test]
fn ayuv_output_layout_uses_r8g8b8a8_uint_uav_and_never_ayuv_view() {
    assert_eq!(OutputLayout::Ayuv.texture_format(), DXGI_FORMAT_AYUV);
    assert_eq!(
        OutputLayout::Ayuv.uav_formats(),
        &[DXGI_FORMAT_R8G8B8A8_UINT]
    );
    // Creating a UAV with view format DXGI_FORMAT_AYUV removes the D3D12 device
    // (DXGI_ERROR_INVALID_CALL, Rubylight #150).
    assert!(!OutputLayout::Ayuv.uav_formats().contains(&DXGI_FORMAT_AYUV));
    assert_eq!(OutputLayout::Ayuv.uav_count(), 1);
    assert_eq!(OutputLayout::Ayuv.shader_entry(), b"yuv444_ayuv_cs\0");
    assert_eq!(OutputLayout::Ayuv.focus_entry(), b"yuv444_ayuv_focus_cs\0");
    assert_eq!(
        OutputLayout::Ayuv.menu_entry(false, 0),
        (&b"yuv444_ayuv_menu_cs\0"[..], [1, 1])
    );

    // 4:2:0 layouts remain unchanged.
    assert_eq!(OutputLayout::for_420(false), OutputLayout::Nv12);
    assert_eq!(OutputLayout::for_420(true), OutputLayout::P010);
    assert_eq!(OutputLayout::Nv12.texture_format(), DXGI_FORMAT_NV12);
    assert_eq!(
        OutputLayout::Nv12.uav_formats(),
        &[DXGI_FORMAT_R8_UNORM, DXGI_FORMAT_R8G8_UNORM]
    );
    assert_eq!(OutputLayout::P010.texture_format(), DXGI_FORMAT_P010);
    assert_eq!(
        OutputLayout::P010.uav_formats(),
        &[DXGI_FORMAT_R16_UNORM, DXGI_FORMAT_R16G16_UNORM]
    );
    assert_eq!(OutputLayout::Nv12.uav_count(), 2);
    assert_eq!(OutputLayout::P010.uav_count(), 2);
    assert_eq!(OutputLayout::Nv12.shader_entry(), b"yuv420_cs\0");
    assert_eq!(OutputLayout::Nv12.focus_entry(), b"yuv420_focus_cs\0");
    assert_eq!(
        OutputLayout::Nv12.menu_entry(false, 0),
        (&b"yuv420_menu_cs\0"[..], [2, 2])
    );
    assert_eq!(
        OutputLayout::Nv12.menu_entry(true, 1),
        (&b"chroma_pack_menu_cs\0"[..], [2, 1])
    );
    assert_eq!(
        OutputLayout::Nv12.menu_entry(true, 2),
        (&b"chroma_pack_menu_cs\0"[..], [2, 2])
    );

    // Dispatch groups for 16x16 AYUV thread groups match encoder focus's 16x16 age grid.
    for (width, height) in [(3456, 2160), (1920, 1080), (1366, 768), (64, 64)] {
        let groups = OutputLayout::Ayuv.dispatch_groups(width, height);
        assert_eq!(
            groups,
            butterpollo_media_policy::encoder_focus::age_grid(width, height),
            "{width}x{height}"
        );
        assert_eq!(
            groups,
            OutputLayout::Nv12.dispatch_groups(width, height),
            "{width}x{height}"
        );
    }

    // Packed 4:4:4 v2 owns 2x2 logical blocks per thread, halving vertical dispatch.
    for (coded_w, coded_h) in [(128, 144), (320, 368), (1920, 2176), (2560, 2880)] {
        let v1 = OutputLayout::Nv12.dispatch_groups_for(coded_w, coded_h, true, 1);
        let v2 = OutputLayout::Nv12.dispatch_groups_for(coded_w, coded_h, true, 2);
        assert_eq!(v1, ((coded_w / 2).div_ceil(8), (coded_h / 2).div_ceil(8)));
        assert_eq!(v2, ((coded_w / 2).div_ceil(8), (coded_h / 4).div_ceil(8)));
        assert!(v2.1 <= v1.1);
        assert_eq!(
            OutputLayout::Nv12.dispatch_groups_for(coded_w, coded_h, false, 2),
            v1,
            "non-packed dispatch ignores padding"
        );
    }
}

#[test]
fn ayuv_initial_pipeline_compiles_only_ayuv_shader_with_one_uav() {
    assert_eq!(InitialPipeline::Ayuv.shader_entry(), b"yuv444_ayuv_cs\0");
    assert_eq!(InitialPipeline::Ayuv.uav_range(), (0, 1));
    assert_eq!(InitialPipeline::Yuv.shader_entry(), b"yuv420_cs\0");
    assert_eq!(InitialPipeline::Yuv.uav_range(), (0, 2));
    assert_eq!(InitialPipeline::Planar.shader_entry(), b"pyro_cs\0");
    assert_eq!(InitialPipeline::Planar.uav_range(), (2, 3));

    for entry in [
        &b"yuv444_ayuv_cs\0"[..],
        &b"yuv444_ayuv_menu_cs\0"[..],
        &b"yuv444_ayuv_focus_cs\0"[..],
        &b"yuv444_planes_cs\0"[..],
        &b"yuv444_planes_menu_cs\0"[..],
    ] {
        let code = shader(entry).expect("4:4:4 compute shader missing from precompiled DXBC");
        assert!(!code.is_empty(), "empty DXBC for {entry:?}");
    }
}

#[test]
fn ayuv_channel_packing_matches_nvenc_and_color_hlsl() {
    // NV_ENC_BUFFER_FORMAT_AYUV (nvEncodeAPI.h): 32-bit word with V in bits 0..7,
    // U in bits 8..15, Y in bits 16..23, and A in bits 24..31. Through an
    // R8G8B8A8_UINT UAV on little-endian Windows, uint4(V, U, Y, 0) stores bytes
    // [V, U, Y, 0].
    let (y, u, v) = (81u8, 90u8, 240u8);
    let bytes = [v, u, y, 0u8];
    let word = u32::from_le_bytes(bytes);
    assert_eq!(word & 0xff, u32::from(v));
    assert_eq!((word >> 8) & 0xff, u32::from(u));
    assert_eq!((word >> 16) & 0xff, u32::from(y));
    assert_eq!((word >> 24) & 0xff, 0);

    // Keep the compute AYUV helper identical to the graphics AYUV pixel shader.
    let hlsl = butterpollo_win_abi::shaders::COLOR_HLSL;
    assert!(hlsl.contains("RWTexture2D<uint4> ayuvPlane : register(u0);"));
    let ayuv_pixel = hlsl
        .split("uint4 yuv444AyuvPixel(")
        .nth(1)
        .expect("yuv444AyuvPixel missing");
    assert!(
        ayuv_pixel
            .split('}')
            .next()
            .unwrap()
            .contains("return uint4(c.z, c.y, c.x, 0);")
    );
}

#[test]
#[ignore = "requires D3D12 compute with DXGI_FORMAT_AYUV UAV support"]
fn native_ayuv_compute_writes_full_resolution_checkerboard_and_supports_focus() -> Result<()> {
    use crate::capture::{ComGuard, Device, GpuImage, Image, Pixel};
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let compute = Compute::for_device(&gpu.device)?;
    let (width, height) = (64u32, 64u32);
    // Alternate 1-pixel red and blue columns so any 4:2:0 subsampling fails.
    let mut bytes = Vec::with_capacity((width * height * 4) as usize);
    for _y in 0..height {
        for x in 0..width {
            if x % 2 == 0 {
                bytes.extend_from_slice(&[0, 0, 255, 255]); // red (BGRA)
            } else {
                bytes.extend_from_slice(&[255, 0, 0, 255]); // blue (BGRA)
            }
        }
    }
    let image = GpuImage::upload_settled(
        &gpu,
        &Image {
            width,
            height,
            stride: width as usize * 4,
            bytes,
            pixel: Pixel::Bgra8,
            captured: std::time::Instant::now(),
        },
    )?;
    let config = butterpollo_media_policy::rtsp::Negotiated {
        width,
        height,
        codec: 1,
        yuv444: true,
        csc_mode: 2,
        ..Default::default()
    };
    let mut converter = Converter::new_ayuv(compute.clone(), width, height)?;
    converter.prepare_yuv()?;
    converter.values =
        crate::gpu_color::constants(&config, (width, height, Pixel::Bgra8), [100., 1.], None);
    let source = compute.open(&image.texture)?;
    let converted = converter.convert(&source, format(Pixel::Bgra8), None, &image)?;
    converted.wait()?;

    // Read back the DXGI_FORMAT_AYUV resource (1 subresource, 4 bytes/pixel).
    // SAFETY: The target completed above; footprints and map bounds stay in range.
    let pixels = unsafe {
        let desc = converted.texture.GetDesc();
        assert_eq!(desc.Format, DXGI_FORMAT_AYUV);
        let mut layout = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
        let mut total = 0u64;
        compute.device.GetCopyableFootprints(
            &desc,
            0,
            1,
            0,
            Some(&mut layout),
            None,
            None,
            Some(&mut total),
        );
        let buffer = focus_buffer(
            &compute.device,
            D3D12_HEAP_TYPE_READBACK,
            total,
            D3D12_RESOURCE_STATE_COPY_DEST,
        )?;
        let allocator: ID3D12CommandAllocator = compute
            .device
            .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_COMPUTE)?;
        let list: ID3D12GraphicsCommandList = compute.device.CreateCommandList(
            0,
            D3D12_COMMAND_LIST_TYPE_COMPUTE,
            &allocator,
            None,
        )?;
        let dst = D3D12_TEXTURE_COPY_LOCATION {
            pResource: std::mem::ManuallyDrop::new(Some(buffer.clone())),
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                PlacedFootprint: layout,
            },
        };
        let src = D3D12_TEXTURE_COPY_LOCATION {
            pResource: std::mem::ManuallyDrop::new(Some(converted.texture.as_ref().clone())),
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                SubresourceIndex: 0,
            },
        };
        list.CopyTextureRegion(&dst, 0, 0, 0, &src, None);
        drop(std::mem::ManuallyDrop::into_inner(dst.pResource));
        drop(std::mem::ManuallyDrop::into_inner(src.pResource));
        list.Close()?;
        compute.wait(compute.execute(&list)?)?;
        let mut mapped = std::ptr::null_mut();
        buffer.Map(0, None, Some(&mut mapped))?;
        let mut out = Vec::with_capacity((width * height) as usize);
        for y in 0..height as usize {
            let row = std::slice::from_raw_parts(
                mapped
                    .cast::<u8>()
                    .add(layout.Offset as usize + y * layout.Footprint.RowPitch as usize),
                width as usize * 4,
            );
            out.extend(row.as_chunks::<4>().0.iter().copied());
        }
        buffer.Unmap(0, None);
        out
    };
    let [red_v, red_u, _red_y, red_a] = pixels[0];
    let [blue_v, blue_u, _blue_y, blue_a] = pixels[1];
    assert_eq!((red_a, blue_a), (0, 0));
    assert!(red_v > blue_v + 100, "red Cr {red_v} vs blue Cr {blue_v}");
    assert!(blue_u > red_u + 100, "blue Cb {blue_u} vs red Cb {red_u}");

    // Encoder focus runs on the AYUV converter and produces a map.
    converter.enable_focus(64)?;
    converter.set_focus(Some(FocusSettings {
        mode: butterpollo_media_policy::encoder_focus::Mode::Static,
        levels: butterpollo_media_policy::encoder_focus::Levels::default(),
        cursor: None,
    }))?;
    converter
        .convert(&source, format(Pixel::Bgra8), None, &image)?
        .wait()?;
    assert!(converter.latest_map(0).is_some());
    Ok(())
}
