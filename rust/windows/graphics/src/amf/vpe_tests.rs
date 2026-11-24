//! AMF's video converter against the shader conversion, offline: the same
//! fixture frames through both, scored against `image_quality`'s float
//! reference, with each converter's chroma siting and per-frame time.
//!
//! `vpe_ab` is ignored: it needs a Radeon and its AMF runtime. Run it in
//! release on an idle GPU (no display, encoder or host changes):
//!
//! ```text
//! set RUBYLIGHT_VPE_REPORT=D:\CodexArtifacts\vpe\report.json
//! set RUBYLIGHT_VPE_CAPTURES=D:\CodexArtifacts\vpe\captures   (optional)
//! cargo test -p butterpollo-win-graphics --release --lib amf::vpe::tests::vpe_ab -- --ignored --nocapture
//! ```
//!
//! Captures are `NAME.json` beside `NAME.raw`, as `hdr_snapshot` writes them:
//! `{"width", "height", "stride", "format"}` with `format` starting `RGBA`
//! (scRGB half floats), `BGRA` (8-bit sRGB) or `RGB10A2` (PQ). Each scores
//! natively and downscaled to 1920x1080 and 1280x720; scRGB as PQ and HLG.
use super::*;
use crate::capture::{ComGuard, Device, GpuImage, Image, Pixel};
use butterpollo_media_policy::{
    amf_vpe::{Transfer, vui},
    image_quality as q,
    rtsp::Negotiated,
};
use serde_json::{Value, json};
use windows::{
    Win32::Graphics::{Direct3D11::*, Dxgi::Common::*},
    core::{GUID, Interface},
};

#[path = "vpe_test_support.rs"]
mod support;
use support::*;

#[test]
fn amf_numbers_match_the_vendored_headers() {
    let pairs: [(i64, i64); 22] = [
        (
            vui::PRIMARIES_BT709,
            AMF_COLOR_PRIMARIES_ENUM_AMF_COLOR_PRIMARIES_BT709 as i64,
        ),
        (
            vui::PRIMARIES_SMPTE170M,
            AMF_COLOR_PRIMARIES_ENUM_AMF_COLOR_PRIMARIES_SMPTE170M as i64,
        ),
        (
            vui::PRIMARIES_BT2020,
            AMF_COLOR_PRIMARIES_ENUM_AMF_COLOR_PRIMARIES_BT2020 as i64,
        ),
        (
            vui::PRIMARIES_CCCS,
            AMF_COLOR_PRIMARIES_ENUM_AMF_COLOR_PRIMARIES_CCCS as i64,
        ),
        (
            vui::TRANSFER_BT709,
            AMF_COLOR_TRANSFER_CHARACTERISTIC_ENUM_AMF_COLOR_TRANSFER_CHARACTERISTIC_BT709 as i64,
        ),
        (
            vui::TRANSFER_SMPTE170M,
            AMF_COLOR_TRANSFER_CHARACTERISTIC_ENUM_AMF_COLOR_TRANSFER_CHARACTERISTIC_SMPTE170M
                as i64,
        ),
        (
            vui::TRANSFER_LINEAR,
            AMF_COLOR_TRANSFER_CHARACTERISTIC_ENUM_AMF_COLOR_TRANSFER_CHARACTERISTIC_LINEAR as i64,
        ),
        (
            vui::TRANSFER_SRGB,
            AMF_COLOR_TRANSFER_CHARACTERISTIC_ENUM_AMF_COLOR_TRANSFER_CHARACTERISTIC_IEC61966_2_1
                as i64,
        ),
        (
            vui::TRANSFER_BT2020_10,
            AMF_COLOR_TRANSFER_CHARACTERISTIC_ENUM_AMF_COLOR_TRANSFER_CHARACTERISTIC_BT2020_10
                as i64,
        ),
        (
            vui::TRANSFER_PQ,
            AMF_COLOR_TRANSFER_CHARACTERISTIC_ENUM_AMF_COLOR_TRANSFER_CHARACTERISTIC_SMPTE2084
                as i64,
        ),
        (
            vui::TRANSFER_HLG,
            AMF_COLOR_TRANSFER_CHARACTERISTIC_ENUM_AMF_COLOR_TRANSFER_CHARACTERISTIC_ARIB_STD_B67
                as i64,
        ),
        (
            vui::MATRIX_RGB,
            AMF_COLOR_MATRIX_COEFF_ENUM_AMF_COLOR_MATRIX_COEFF_GBR as i64,
        ),
        (
            vui::MATRIX_BT709,
            AMF_COLOR_MATRIX_COEFF_ENUM_AMF_COLOR_MATRIX_COEFF_BT_709 as i64,
        ),
        (
            vui::MATRIX_BT601,
            AMF_COLOR_MATRIX_COEFF_ENUM_AMF_COLOR_MATRIX_COEFF_BT_601 as i64,
        ),
        (
            vui::MATRIX_BT2020_NCL,
            AMF_COLOR_MATRIX_COEFF_ENUM_AMF_COLOR_MATRIX_COEFF_BT_2020_NCL as i64,
        ),
        (
            vui::RANGE_STUDIO,
            AMF_COLOR_RANGE_ENUM_AMF_COLOR_RANGE_STUDIO as i64,
        ),
        (
            vui::RANGE_FULL,
            AMF_COLOR_RANGE_ENUM_AMF_COLOR_RANGE_FULL as i64,
        ),
        (
            vui::PROFILE_JPEG,
            AMF_VIDEO_CONVERTER_COLOR_PROFILE_ENUM_AMF_VIDEO_CONVERTER_COLOR_PROFILE_JPEG as i64,
        ),
        (
            vui::PROFILE_FULL_2020,
            AMF_VIDEO_CONVERTER_COLOR_PROFILE_ENUM_AMF_VIDEO_CONVERTER_COLOR_PROFILE_FULL_2020
                as i64,
        ),
        (
            vui::ACCELERATION_VPE_PREFERRED,
            AMF_VIDEO_CONVERTER_ACCELERATION_ENUM_AMF_VIDEO_CONVERTER_ACCELERATION_VPE_PREFERRED
                as i64,
        ),
        (
            vui::SCALE_BICUBIC,
            AMF_VIDEO_CONVERTER_SCALE_ENUM_AMF_VIDEO_CONVERTER_SCALE_BICUBIC as i64,
        ),
        (
            vui::TONEMAPPING_COPY,
            AMF_VIDEO_CONVERTER_TONEMAPPING_ENUM_AMF_VIDEO_CONVERTER_TONEMAPPING_COPY as i64,
        ),
    ];
    for (index, (ours, header)) in pairs.into_iter().enumerate() {
        assert_eq!(ours, header, "pair {index}");
    }
    use butterpollo_media_policy::amf_vpe::Format;
    assert_eq!(
        Format::Nv12.amf(),
        AMF_SURFACE_FORMAT_AMF_SURFACE_NV12 as i64
    );
    assert_eq!(
        Format::P010.amf(),
        AMF_SURFACE_FORMAT_AMF_SURFACE_P010 as i64
    );
    assert_eq!(
        Format::Bgra.amf(),
        AMF_SURFACE_FORMAT_AMF_SURFACE_BGRA as i64
    );
    assert_eq!(
        Format::RgbaF16.amf(),
        AMF_SURFACE_FORMAT_AMF_SURFACE_RGBA_F16 as i64
    );
    assert_eq!(
        Format::R10G10B10A2.amf(),
        AMF_SURFACE_FORMAT_AMF_SURFACE_R10G10B10A2 as i64
    );
}

const FRAMES: usize = 60;
const WARMUP: usize = 5;

/// Both converters on `picture`: scores, and per-frame times.
fn compare(
    gpu: &Device,
    runtime: &Runtime,
    acceleration: i64,
    name: &str,
    picture: &q::Picture,
    pixel: Pixel,
    config: &Negotiated,
) -> Result<Value> {
    let spec = spec(config, pixel);
    let upload = image(picture, pixel);
    let source = quantized(&upload);
    let reference = q::reference(&source, &spec);
    let capture = GpuImage::upload(gpu, &upload)?;
    let mut result = json!({
        "case": name,
        "source": [picture.width, picture.height],
        "stream": [config.width, config.height],
        "transfer": format!("{:?}", spec.transfer),
        "pixel": format!("{pixel:?}"),
    });
    // The shader conversion.
    let mut shader =
        crate::gpu_color::Converter::new(gpu, config, (capture.width, capture.height, pixel))?;
    let mut micros = vec![];
    let mut last = None;
    for frame in 0..WARMUP + FRAMES {
        let started = std::time::Instant::now();
        let texture = shader.convert(&capture)?;
        finish(gpu, &texture, 0)?;
        if frame >= WARMUP {
            micros.push(started.elapsed().as_secs_f64() * 1e6);
        }
        last = Some(texture);
    }
    let codes = read_codes(gpu, last.as_ref().unwrap(), 0, spec.width, spec.height)?;
    result["shader"] = score(&spec, &codes, &reference);
    result["shader"]["time"] = times(micros);
    // AMF's video converter.
    // SAFETY: `runtime` holds a live factory and D3D11 context that outlive `converter`.
    let made = unsafe { Vpe::new(runtime.factory, runtime.context, config) };
    let mut converter = match made {
        Ok(converter) => converter,
        Err(error) => {
            result["vpe"] = json!({"error": format!("{error:#}")});
            return Ok(result);
        }
    };
    converter.acceleration = acceleration;
    result["vpe_engine_reported"] = json!(converter.engine);
    let input = match converter.plan(&capture, [100., 1.], false) {
        Ok(input) => input,
        Err(refusal) => {
            result["vpe"] = json!({"refused": refusal.code()});
            return Ok(result);
        }
    };
    let mut ownership = gpu::Ownership::new();
    let mut micros = vec![];
    let mut codes = None;
    for frame in 0..WARMUP + FRAMES {
        let started = std::time::Instant::now();
        let converted = match converter.convert(&capture, input, &mut ownership) {
            Ok(converted) => converted,
            Err(error) => {
                result["vpe"] = json!({"error": format!("{error:#}")});
                return Ok(result);
            }
        };
        let (texture, index) = native(&converted.surface)?;
        finish(gpu, &texture, index)?;
        if frame >= WARMUP {
            micros.push(started.elapsed().as_secs_f64() * 1e6);
        }
        if frame + 1 == WARMUP + FRAMES {
            codes = Some(read_codes(gpu, &texture, index, spec.width, spec.height)?);
        }
    }
    result["vpe"] = score(&spec, codes.as_deref().unwrap_or_default(), &reference);
    result["vpe"]["time"] = times(micros);
    result["vpe"]["output"] = json!(format!("{:?}", converter.output().format));
    drop(converter);
    Ok(result)
}
/// Chroma siting of both converters for one stream kind, at native size.
fn siting_case(
    gpu: &Device,
    runtime: &Runtime,
    acceleration: i64,
    pixel: Pixel,
    config: &Negotiated,
) -> Result<Value> {
    let spec = spec(config, pixel);
    let mut out =
        json!({"transfer": format!("{:?}", spec.transfer), "pixel": format!("{pixel:?}")});
    let ramps = [q::ramp(&spec, true), q::ramp(&spec, false)].map(|ramp| image(&ramp, pixel));
    let captures = [
        GpuImage::upload(gpu, &ramps[0])?,
        GpuImage::upload(gpu, &ramps[1])?,
    ];
    let mut shader = crate::gpu_color::Converter::new(
        gpu,
        config,
        (captures[0].width, captures[0].height, pixel),
    )?;
    let mut codes = vec![];
    for capture in &captures {
        let texture = shader.convert(capture)?;
        codes.push(read_codes(gpu, &texture, 0, spec.width, spec.height)?);
    }
    out["shader"] = siting(&spec, &codes[0], &codes[1]);
    // SAFETY: `runtime` outlives `converter`.
    let mut converter = unsafe { Vpe::new(runtime.factory, runtime.context, config)? };
    converter.acceleration = acceleration;
    let mut ownership = gpu::Ownership::new();
    let mut codes = vec![];
    for capture in &captures {
        let Ok(input) = converter.plan(capture, [100., 1.], false) else {
            return Ok(out);
        };
        let converted = converter.convert(capture, input, &mut ownership)?;
        let (texture, index) = native(&converted.surface)?;
        codes.push(read_codes(gpu, &texture, index, spec.width, spec.height)?);
    }
    out["vpe"] = siting(&spec, &codes[0], &codes[1]);
    Ok(out)
}

/// Captures from `RUBYLIGHT_VPE_CAPTURES`: name, picture and pixel.
fn captures() -> Result<Vec<(String, q::Picture, Pixel)>> {
    let Ok(folder) = std::env::var("RUBYLIGHT_VPE_CAPTURES") else {
        return Ok(vec![]);
    };
    let mut found = vec![];
    for entry in std::fs::read_dir(&folder)? {
        let path = entry?.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let meta: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
        let raw = path.with_extension("raw");
        let (Some(width), Some(height), Some(stride), Some(format)) = (
            meta["width"].as_u64(),
            meta["height"].as_u64(),
            meta["stride"].as_u64(),
            meta["format"].as_str(),
        ) else {
            continue;
        };
        let pixel = if format.starts_with("RGBA") {
            Pixel::RgbaF16
        } else if format.starts_with("BGRA") {
            Pixel::Bgra8
        } else if format.starts_with("RGB10A2") {
            Pixel::Rgba10Pq
        } else {
            continue;
        };
        let bytes = std::fs::read(&raw).with_context(|| format!("{}", raw.display()))?;
        let per = if pixel == Pixel::RgbaF16 { 8 } else { 4 };
        let (width, height, stride) = (width as usize, height as usize, stride as usize);
        let mut packed = Vec::with_capacity(width * height * per);
        for y in 0..height {
            packed.extend_from_slice(&bytes[y * stride..y * stride + width * per]);
        }
        let picture = quantized(&Image {
            width: width as u32,
            height: height as u32,
            stride: width * per,
            bytes: packed,
            captured: std::time::Instant::now(),
            pixel,
        });
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        found.push((name, picture, pixel));
    }
    Ok(found)
}

#[test]
#[ignore = "requires a Radeon with its AMF runtime; no display, encoder or host changes"]
fn vpe_ab() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let runtime = Runtime::new(&gpu)?;
    // `RUBYLIGHT_VPE_ACCELERATION=compute` asks AMF for its compute path,
    // to tell the VPE apart from the driver's own fallback.
    let acceleration = match std::env::var("RUBYLIGHT_VPE_ACCELERATION").as_deref() {
        Ok("compute") => vui::ACCELERATION_COMPUTE,
        _ => vui::ACCELERATION_VPE_PREFERRED,
    };
    let kinds = [
        (Transfer::Sdr, Pixel::Bgra8),
        (Transfer::Pq, Pixel::RgbaF16),
        (Transfer::Pq, Pixel::Rgba10Pq),
        (Transfer::Hlg, Pixel::RgbaF16),
    ];
    let mut cases = vec![];
    let mut sitings = vec![];
    // A case that fails is recorded with its error and the run goes on, so one failure (an
    // HLG prepass the device refuses) does not lose every other result and the report.
    let mut failed = 0;
    let mut record =
        |list: &mut Vec<serde_json::Value>, name: String, result: Result<serde_json::Value>| {
            match result {
                Ok(value) => list.push(value),
                Err(error) => {
                    failed += 1;
                    eprintln!("{name}: {error:#}");
                    list.push(json!({"name": name, "error": format!("{error:#}")}));
                }
            }
        };
    for (transfer, pixel) in kinds {
        let native = negotiated(transfer, 1920, 1080);
        record(
            &mut sitings,
            format!("siting {transfer:?} {pixel:?}"),
            siting_case(&gpu, &runtime, acceleration, pixel, &native),
        );
        // Native, the two common integer reductions, a fractional one, an
        // enlargement and a letterbox (21:9 on 16:9).
        for (source, target) in [
            ((1920, 1080), (1920, 1080)),
            ((3840, 2160), (1920, 1080)),
            ((3840, 2160), (1280, 720)),
            ((2560, 1440), (1920, 1080)),
            ((1280, 720), (1920, 1080)),
            ((3440, 1440), (1920, 1080)),
        ] {
            let config = negotiated(transfer, target.0, target.1);
            let picture = card(&spec(&config, pixel), source.0, source.1);
            let name = format!(
                "card {}x{} to {}x{}",
                source.0, source.1, target.0, target.1
            );
            let result = compare(
                &gpu,
                &runtime,
                acceleration,
                &name,
                &picture,
                pixel,
                &config,
            );
            if let Ok(result) = &result {
                eprintln!("{result}");
            }
            record(&mut cases, format!("{name} {transfer:?} {pixel:?}"), result);
        }
    }
    for (name, picture, pixel) in captures()? {
        let transfers: &[Transfer] = if pixel == Pixel::Bgra8 {
            &[Transfer::Sdr]
        } else {
            &[Transfer::Pq, Transfer::Hlg]
        };
        for &transfer in transfers {
            for target in [
                (picture.width as u32, picture.height as u32),
                (1920, 1080),
                (1280, 720),
            ] {
                if target.0 % 2 != 0 || target.1 % 2 != 0 {
                    continue;
                }
                let config = negotiated(transfer, target.0, target.1);
                let label = format!("capture {name} to {}x{}", target.0, target.1);
                let result = compare(
                    &gpu,
                    &runtime,
                    acceleration,
                    &label,
                    &picture,
                    pixel,
                    &config,
                );
                if let Ok(result) = &result {
                    eprintln!("{result}");
                }
                record(&mut cases, format!("{label} {transfer:?}"), result);
            }
        }
    }
    let report = json!({
        "scope": "offline conversion only: the same frames through the shader conversion and AMF's video converter, scored against a float reference; no encode",
        "adapter": gpu.display.adapter,
        "acceleration": if acceleration == vui::ACCELERATION_COMPUTE { "compute" } else { "vpe_preferred" },
        "frames_timed": FRAMES,
        "time": "CPU time from submitting a conversion to reading two of its pixels back, per frame",
        "siting": sitings,
        "cases": cases,
        "failed": failed,
    });
    let text = serde_json::to_string_pretty(&report)?;
    match std::env::var("RUBYLIGHT_VPE_REPORT") {
        Ok(path) => std::fs::write(&path, &text).with_context(|| path.clone())?,
        Err(_) => println!("{text}"),
    }
    // Written in full first; the run still fails when a case did.
    anyhow::ensure!(
        failed == 0,
        "{failed} cases failed; the report has their errors"
    );
    Ok(())
}
