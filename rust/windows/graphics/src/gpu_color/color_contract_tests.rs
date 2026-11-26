use super::*;

#[test]
#[ignore = "requires D3D11 video views/UAVs and D3D12 compute"]
fn standard_chroma_is_left_sited_and_pyrowave_remains_center_sited() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let mut bytes = [0, 0, 0, 255].repeat(64 * 64);
    // The interior impulse has equal contributions at the chroma samples
    // on either side. The boundary impulse also checks target-edge clamping.
    for y in 0..64 {
        for x in [0, 5] {
            bytes[(y * 64 + x) * 4 + 2] = 255;
        }
    }
    let image = Image {
        width: 64,
        height: 64,
        stride: 256,
        bytes,
        pixel: Pixel::Bgra8,
        captured: Instant::now(),
    };
    let uploaded = GpuImage::upload_settled(&gpu, &image)?;
    let config = butterpollo_media_policy::rtsp::Negotiated {
        width: 64,
        height: 64,
        codec: 1,
        sdr_10bit: true,
        csc_mode: 2,
        ..Default::default()
    };
    let check_standard = |output: &[u16]| {
        // BT.709 limited range, with 75% red at x=0 and 25% at x=4/6.
        for (x, expected) in [
            (0, [435, 848]),
            (2, [512, 512]),
            (4, [486, 624]),
            (6, [486, 624]),
        ] {
            let at = 4096 + x;
            for (actual, expected) in output[at..at + 2].iter().zip(expected) {
                assert!(
                    actual.abs_diff(expected) <= 1,
                    "standard chroma x={x}: {actual} != {expected}"
                );
            }
        }
    };
    for compute in [false, true] {
        let mut converter = Converter::new(&gpu, &config, (64, 64, Pixel::Bgra8))?;
        if !compute {
            converter.compute = None;
        }
        check_standard(&readback(&gpu, converter.convert(&uploaded)?.as_ref())?);
        let mut planar = PlanarConverter::new(&gpu, &config, (64, 64, Pixel::Bgra8))?;
        if !compute {
            planar.compute = None;
        }
        planar.convert(&uploaded, [100., 1.])?;
        for (plane, expected) in [(1, [461_u16, 512, 461, 512]), (2, [736, 512, 736, 512])] {
            let bytes = plane_bytes(&gpu, &planar.textures[plane])?;
            // PyroWave uses full-normalized 10-bit codes in R16_UNORM planes.
            for (word, expected) in bytes[..8].as_chunks::<2>().0.iter().zip(expected) {
                let actual = (f64::from(u16::from_le_bytes(*word)) * 1023. / 65535.).round() as u16;
                assert!(
                    actual.abs_diff(expected) <= 1,
                    "PyroWave plane {plane}: {actual} != {expected}"
                );
            }
        }
    }
    let compute = crate::compute::Compute::for_device(&gpu.device)?;
    let mut converter = crate::compute::Converter::new(compute.clone(), 64, 64, true)?;
    converter.values = constants(&config, (64, 64, Pixel::Bgra8), [100., 1.], None);
    let converted = converter.convert(
        &compute.open(&uploaded.texture)?,
        DXGI_FORMAT_B8G8R8A8_UNORM,
        None,
        &uploaded,
    )?;
    converted.wait()?;
    check_standard(&readback_compute(&compute, &converted.texture)?);
    Ok(())
}

#[test]
#[ignore = "requires D3D11 P010 conversion"]
fn sdr_bt2020_gpu_matches_reference_primaries_and_transfer() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    for (bgra, linear, expected) in [
        ([0, 0, 255, 255], [1.25, 0., 0.], [387_u16, 371, 769]),
        ([128, 128, 128, 255], [0.2698256; 3], [460, 512, 512]),
    ] {
        let image = Image {
            width: 64,
            height: 64,
            stride: 256,
            bytes: bgra.repeat(64 * 64),
            pixel: Pixel::Bgra8,
            captured: Instant::now(),
        };
        let config = butterpollo_media_policy::rtsp::Negotiated {
            width: 64,
            height: 64,
            codec: 1,
            sdr_10bit: true,
            csc_mode: 4,
            // The clip path, whatever this PC's display runs.
            sdr_tone_map: false,
            ..Default::default()
        };
        // Advanced-color Windows desktops can deliver the same SDR picture
        // as FP16 scRGB, with 100-nit SDR white represented by 1.25.
        for image in [image, make_image(&[linear], 64, 64)] {
            let uploaded = GpuImage::upload_settled(&gpu, &image)?;
            for compute in [false, true] {
                let mut converter = Converter::new(&gpu, &config, (64, 64, image.pixel))?;
                if !compute {
                    converter.compute = None;
                }
                let output = readback(&gpu, converter.convert(&uploaded)?.as_ref())?;
                for (actual, expected) in [output[0], output[4096], output[4097]]
                    .into_iter()
                    .zip(expected)
                {
                    assert!(
                        actual.abs_diff(expected) <= 1,
                        "GPU: {actual} != {expected}"
                    );
                }
            }
        }
    }
    // Extended scRGB channels may be outside Rec.709 while the color is
    // inside BT.2020. Mapping must happen before clamping the SDR output.
    let image = make_image(&[[-0.125, 1.2, -0.0125]], 64, 64);
    let uploaded = GpuImage::upload_settled(&gpu, &image)?;
    let config = butterpollo_media_policy::rtsp::Negotiated {
        width: 64,
        height: 64,
        codec: 1,
        sdr_10bit: true,
        csc_mode: 4,
        // The clip path, whatever this PC's display runs.
        sdr_tone_map: false,
        ..Default::default()
    };
    for compute in [false, true] {
        let mut converter = Converter::new(&gpu, &config, (64, 64, Pixel::RgbaF16))?;
        if !compute {
            converter.compute = None;
        }
        let output = readback(&gpu, converter.convert(&uploaded)?.as_ref())?;
        for (actual, expected) in [output[0], output[4096], output[4097]]
            .into_iter()
            .zip([746, 256, 338])
        {
            assert!(
                actual.abs_diff(expected) <= 1,
                "extended scRGB: {actual} != {expected}"
            );
        }
    }
    Ok(())
}

#[test]
#[ignore = "requires D3D11 P010 conversion and D3D12 compute"]
fn hdr_chroma_is_filtered_in_light_on_every_path() -> Result<()> {
    use butterpollo_media_policy::amf_vpe::{Source, Transfer};
    use butterpollo_media_policy::image_quality as q;
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let compute = crate::compute::Compute::for_device(&gpu.device)?;
    // Rubylight #130: Segoe UI's "ill" as ClearType draws it, black on white, on an HDR desktop
    // with SDR content at 200 nits (scRGB 2.5). Averaging these fringes' codes tinted it magenta.
    let (w, y, m, c) = (
        [255.; 3],
        [255., 255., 182.],
        [102., 0., 58.],
        [144., 219., 255.],
    );
    let row = [w, y, m, c, y, m, c, w, y, m, c, w];
    let colors: Vec<[f32; 3]> = (0..64)
        .map(|x| row[x % row.len()].map(|v: f64| (q::srgb_decode(v / 255.) * 2.5) as f32))
        .collect();
    let image = make_image(&colors, 1, 16);
    let source = GpuImage::upload_settled(&gpu, &image)?;
    let picture = q::Picture {
        width: 64,
        height: 16,
        rgb: (0..64 * 16)
            .map(|i| colors[i % 64].map(|v| f64::from(half::f16::from_f32(v).to_f32())))
            .collect(),
    };
    for (transfer, hdr_format) in [(Transfer::Pq, 0), (Transfer::Hlg, 2)] {
        let spec = q::Spec {
            width: 64,
            height: 16,
            source: Source::ScRgbF16,
            transfer,
            matrix: 2,
            full_range: false,
            ten_bit: true,
        };
        let wanted = q::reference(&picture, &spec);
        // The pattern tells light from codes: a filter of the codes lands far from the
        // reference. (The reference filters SDR's codes, and the transfer changes nothing else.)
        let nonlinear = q::nonlinear(&picture, &spec);
        let of_codes = q::planes_from_rgb(
            &nonlinear,
            &q::Spec {
                transfer: Transfer::Sdr,
                ..spec
            },
        );
        let apart = wanted
            .cr
            .iter()
            .zip(&of_codes.cr)
            .map(|(a, b)| (a - b).abs())
            .fold(0., f64::max);
        assert!(
            apart > 8.,
            "{transfer:?}: the pattern does not separate light from codes: {apart}"
        );
        let config = butterpollo_media_policy::rtsp::Negotiated {
            width: 64,
            height: 16,
            codec: 2,
            hdr: true,
            hdr_format,
            csc_mode: 4,
            ..Default::default()
        };
        let mut paths = Vec::new();
        for with_compute in [false, true] {
            let mut converter = Converter::new(&gpu, &config, (64, 16, image.pixel))?;
            if !with_compute {
                converter.compute = None;
            }
            paths.push((
                if with_compute {
                    "D3D11 compute"
                } else {
                    "D3D11 graphics"
                },
                readback(&gpu, converter.convert(&source)?.as_ref())?,
            ));
        }
        let mut converter = crate::compute::Converter::new(compute.clone(), 64, 16, true)?;
        converter.values = constants(&config, (64, 16, image.pixel), [100., 1.], None);
        let converted = converter.convert(
            &compute.open(&source.texture)?,
            crate::compute::format(image.pixel),
            None,
            &source,
        )?;
        converted.wait()?;
        paths.push((
            "D3D12 compute",
            readback_compute(&compute, &converted.texture)?,
        ));
        for (path, pixels) in paths {
            for j in 0..8 {
                for i in 0..32 {
                    let at = 64 * 16 + j * 64 + i * 2;
                    for (plane, value) in [(&wanted.cb, pixels[at]), (&wanted.cr, pixels[at + 1])] {
                        let expected = plane[j * 32 + i];
                        assert!(
                            (f64::from(value) - expected).abs() <= 1.,
                            "{transfer:?} {path} chroma ({i}, {j}): {value}, wanted {expected:.2}"
                        );
                    }
                }
            }
        }
    }
    Ok(())
}
