//! Native AMD AMF bitstream HDR10, HLG, Dolby Vision and AV1 geometry tests.

use super::super::*;
use super::output_within;

#[test]
#[ignore = "requires AMD AMF and an independent FFprobe"]
fn hdr10_metadata_and_range_reach_the_bitstream() -> Result<()> {
    use crate::capture::{ComGuard, Pixel};
    use std::{os::windows::process::CommandExt, process::Command};
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let ffprobe =
        std::env::var_os("BUTTERPOLLO_TEST_FFPROBE").context("set BUTTERPOLLO_TEST_FFPROBE")?;
    let directory = tempfile::tempdir()?;
    let (width, height) = (1968u32, 2184u32);
    let pixels = (0..width * height)
        .flat_map(|index| {
            let value = half::f16::from_f32([0.05, 1., 6.][(index % width * 3 / width) as usize]);
            [value, value, value, half::f16::ONE]
        })
        .flat_map(|value| value.to_le_bytes())
        .collect();
    let source = GpuImage::upload(
        &gpu,
        &Image {
            width,
            height,
            stride: width as usize * 8,
            bytes: pixels,
            captured: Instant::now(),
            pixel: Pixel::RgbaF16,
        },
    )?;
    source.readback(&mut None)?;
    let mut metadata = butterpollo_media_policy::hdr::Metadata::display(1015., 0.005, 400.);
    metadata.max_cll = 1000;
    metadata.max_fall = 400;
    let options = butterpollo_config::config::Config::parse(
        "amd_usage=ultralowlatency\namd_quality=speed\namd_smart_access_video=disabled\namd_ltr_frames=4\n",
    )?;
    let defaults = butterpollo_config::config::Config::default();
    for codec in [1u8, 2] {
        for full_range in [false, true] {
            for compute in [false, true] {
                for (references, metadata, options) in [
                    (1, metadata, &defaults),
                    (4, metadata, &defaults),
                    (
                        1,
                        butterpollo_media_policy::hdr::Metadata::display(1015., 0., 1015.),
                        &options,
                    ),
                    (
                        4,
                        butterpollo_media_policy::hdr::Metadata::display(1015., 0., 1015.),
                        &options,
                    ),
                    (4, metadata, &options),
                ] {
                    let config = butterpollo_media_policy::rtsp::Negotiated {
                        width,
                        height,
                        codec,
                        hdr: true,
                        csc_mode: 4 | u8::from(full_range),
                        fps: 120,
                        bitrate_kbps: 80000,
                        references,
                        ..Default::default()
                    };
                    let queue = compute
                        .then(|| crate::compute::Compute::for_device(&gpu.device))
                        .transpose()?;
                    let mut encoder = Encoder::new_gpu(&config, gpu.clone(), options, queue)?;
                    assert_eq!(
                        encoder.supports_invalidation(),
                        // `references` sets the client's limit; whether LTR
                        // recovery is enabled also depends on the options.
                        butterpollo_media_policy::encoder_policy::amf_ltr_frames(options, &config)
                            > 0
                    );
                    let changes = [
                        metadata,
                        butterpollo_media_policy::hdr::Metadata {
                            maximum_nits: 600,
                            max_cll: 0,
                            max_fall: 0,
                            ..metadata
                        },
                        butterpollo_media_policy::hdr::Metadata {
                            maximum_nits: 1400,
                            max_cll: 1200,
                            max_fall: 500,
                            ..metadata
                        },
                    ];
                    let mut output = vec![];
                    for frame in 0..12 {
                        // Repeated refreshes must not keep inserting keyframes.
                        encoder.set_hdr_metadata(changes[frame / 4]);
                        output.extend(encoder.encode_gpu(
                            &source,
                            // Later keyframes must come from changed metadata,
                            // even with no unrelated recovery/client request.
                            frame == 0,
                            config.bitrate_kbps,
                        )?);
                        // Drain before changing a component-wide metadata property.
                        if frame % 4 != 3 {
                            continue;
                        }
                        let deadline = Instant::now() + Duration::from_secs(3);
                        while encoder.pending() {
                            output.extend(encoder.poll()?);
                            if Instant::now() >= deadline {
                                bail!("HDR metadata probe output timed out");
                            }
                            std::thread::sleep(Duration::from_millis(1));
                        }
                    }
                    assert_eq!(output.len(), 12);
                    for (frame, encoded) in output.iter().enumerate() {
                        assert_eq!(
                            encoded.idr,
                            frame % 4 == 0,
                            "unexpected keyframe at {frame}"
                        );
                    }
                    for frame in [0, 4, 8] {
                        let metadata = changes[frame / 4];
                        assert!(output[frame].idr);
                        let name = format!(
                            "codec{codec}-full{full_range}-compute{compute}-refs{references}-cll{}-defaults{}-frame{frame}",
                            metadata.max_cll,
                            options.values.is_empty()
                        );
                        let bitstream = directory.path().join(format!("{name}.bin"));
                        // Decode each IDR separately: a decoder retains earlier SEI.
                        // Moonlight's decode-unit dump strips HEVC prefix SEI, so inspect
                        // the encoder output before client depacketization.
                        std::fs::write(&bitstream, &output[frame].bytes)?;
                        let probed = output_within(
                            Command::new(&ffprobe)
                                .args([
                                    "-v",
                                    "error",
                                    "-f",
                                    if codec == 1 { "hevc" } else { "obu" },
                                    "-i",
                                ])
                                .arg(&bitstream)
                                .args(["-show_frames", "-of", "json"])
                                .creation_flags(0x08000000),
                        )?;
                        assert!(
                            probed.status.success(),
                            "{}",
                            String::from_utf8_lossy(&probed.stderr)
                        );
                        let probed: serde_json::Value = serde_json::from_slice(&probed.stdout)?;
                        assert_eq!(probed["frames"].as_array().map(Vec::len), Some(1), "{name}");
                        let first = &probed["frames"][0];
                        assert_eq!(first["key_frame"], 1, "{name}: {first}");
                        assert_eq!(
                            first["color_range"],
                            if full_range { "pc" } else { "tv" },
                            "{name}: {first}"
                        );
                        assert_eq!(first["color_transfer"], "smpte2084", "{name}: {first}");
                        // The matrix is left to the driver to derive from the colour
                        // profile; a client converting with BT.709 would shift colours.
                        assert_eq!(first["color_primaries"], "bt2020", "{name}: {first}");
                        assert_eq!(first["color_space"], "bt2020nc", "{name}: {first}");
                        let side = first["side_data_list"]
                            .as_array()
                            .with_context(|| format!("{name}: no side data in {first}"))?;
                        let mastering = side
                            .iter()
                            .find(|s| s["side_data_type"] == "Mastering display metadata")
                            .with_context(|| format!("{name}: no mastering display metadata"))?;
                        // Mastering peak and BT.2020 red (0.708) in each codec's units.
                        let (luminance, minimum, red) = if codec == 1 {
                            (
                                format!("{}/10000", u32::from(metadata.maximum_nits) * 10000),
                                format!("{}/10000", metadata.minimum),
                                "35400/50000",
                            )
                        } else {
                            (
                                format!("{}/256", u32::from(metadata.maximum_nits) * 256),
                                format!("{}/16384", u32::from(metadata.minimum) * 16384 / 10000),
                                "46399/65536",
                            )
                        };
                        assert_eq!(mastering["max_luminance"], luminance, "{name}: {mastering}");
                        assert_eq!(mastering["min_luminance"], minimum, "{name}: {mastering}");
                        assert_eq!(mastering["red_x"], red, "{name}: {mastering}");
                        let light = side
                            .iter()
                            .find(|s| s["side_data_type"] == "Content light level metadata");
                        if metadata.max_cll == 0 && frame == 0 {
                            assert!(light.is_none(), "{name}: unexpected content light level");
                        } else if metadata.max_cll == 0 {
                            // After a known value, AMF keeps the SEI/OBU and writes
                            // 0/0, which means unknown (CTA-861.3). Only a stale
                            // non-zero level would be wrong.
                            if let Some(light) = light {
                                assert_eq!(
                                    light["max_content"], 0,
                                    "{name}: stale content light level"
                                );
                                assert_eq!(
                                    light["max_average"], 0,
                                    "{name}: stale content light level"
                                );
                            }
                        } else {
                            let light =
                                light.with_context(|| format!("{name}: no content light level"))?;
                            assert_eq!(light["max_content"], metadata.max_cll, "{name}");
                            assert_eq!(light["max_average"], metadata.max_fall, "{name}");
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
#[test]
fn hlg_and_dolby_vision_8_4_signal_arib_std_b67() {
    // rubylight_protocol::hdr: PQ 1, HLG 2, DV_81 4, DV_84 8.
    let stream = |codec, hdr, hdr_format, csc_mode| butterpollo_media_policy::rtsp::Negotiated {
        codec,
        hdr,
        hdr_format,
        csc_mode,
        ..Default::default()
    };
    for codec in [1, 2] {
        assert_eq!(transfer_characteristic(&stream(codec, true, 2, 4)), 18);
        assert_eq!(transfer_characteristic(&stream(codec, true, 0, 4)), 16);
        assert_eq!(transfer_characteristic(&stream(codec, true, 1, 4)), 16);
        // HLG needs an HDR stream: SDR keeps its matrix's curve.
        assert_eq!(transfer_characteristic(&stream(codec, false, 2, 2)), 1);
    }
    assert_eq!(transfer_characteristic(&stream(1, true, 8, 4)), 18);
    assert_eq!(transfer_characteristic(&stream(1, true, 4, 4)), 16);
    assert_eq!(transfer_characteristic(&stream(0, false, 0, 0)), 6);
    assert_eq!(transfer_characteristic(&stream(0, false, 0, 2)), 1);
}
#[test]
#[ignore = "requires AMD AMF and an independent FFprobe"]
fn hlg_and_dolby_vision_8_4_reach_the_bitstream() -> Result<()> {
    use crate::capture::{ComGuard, Pixel};
    use std::{os::windows::process::CommandExt, process::Command};
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let ffprobe =
        std::env::var_os("BUTTERPOLLO_TEST_FFPROBE").context("set BUTTERPOLLO_TEST_FFPROBE")?;
    let directory = tempfile::tempdir()?;
    let (width, height) = (1920u32, 1080u32);
    let pixels = (0..width * height)
        .flat_map(|index| {
            let value =
                half::f16::from_f32([0.05, 2.5, 12.5][(index % width * 3 / width) as usize]);
            [value, value, value, half::f16::ONE]
        })
        .flat_map(|value| value.to_le_bytes())
        .collect();
    let source = GpuImage::upload(
        &gpu,
        &Image {
            width,
            height,
            stride: width as usize * 8,
            bytes: pixels,
            captured: Instant::now(),
            pixel: Pixel::RgbaF16,
        },
    )?;
    source.readback(&mut None)?;
    let options = butterpollo_config::config::Config::default();
    // rubylight_protocol::hdr: HLG 2, DV_84 8 (HEVC only).
    for (codec, hdr_format) in [(1u8, 2u8), (2, 2), (1, 8)] {
        for compute in [false, true] {
            let config = butterpollo_media_policy::rtsp::Negotiated {
                width,
                height,
                codec,
                hdr: true,
                hdr_format,
                csc_mode: 4,
                fps: 60,
                bitrate_kbps: 40000,
                ..Default::default()
            };
            let queue = compute
                .then(|| crate::compute::Compute::for_device(&gpu.device))
                .transpose()?;
            let mut encoder = Encoder::new_gpu(&config, gpu.clone(), &options, queue)?;
            encoder.set_hdr_metadata(butterpollo_media_policy::hdr::Metadata::display(
                1000., 0.005, 400.,
            ));
            let mut output = encoder.encode_gpu(&source, true, config.bitrate_kbps)?;
            let deadline = Instant::now() + Duration::from_secs(2);
            while encoder.pending() {
                output.extend(encoder.poll()?);
                if Instant::now() >= deadline {
                    bail!("HLG probe output timed out");
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            let name = format!("codec{codec}-format{hdr_format}-compute{compute}");
            let frame = output
                .first()
                .with_context(|| format!("{name}: no output"))?;
            assert!(frame.idr, "{name}");
            let bitstream = directory.path().join(format!("{name}.bin"));
            std::fs::write(&bitstream, &frame.bytes)?;
            let probed = output_within(
                Command::new(&ffprobe)
                    .args([
                        "-v",
                        "error",
                        "-f",
                        if codec == 1 { "hevc" } else { "obu" },
                        "-i",
                    ])
                    .arg(&bitstream)
                    .args(["-show_frames", "-of", "json"])
                    .creation_flags(0x08000000),
            )?;
            assert!(
                probed.status.success(),
                "{}",
                String::from_utf8_lossy(&probed.stderr)
            );
            let probed: serde_json::Value = serde_json::from_slice(&probed.stdout)?;
            let first = &probed["frames"][0];
            // VUI / sequence header transfer_characteristics 18.
            assert_eq!(first["color_transfer"], "arib-std-b67", "{name}: {first}");
            assert_eq!(first["color_primaries"], "bt2020", "{name}: {first}");
            assert_eq!(first["color_space"], "bt2020nc", "{name}: {first}");
            // Dolby Vision 8.4 ends the access unit with an RPU, NAL type 62;
            // plain HLG carries none. HEVC payloads never hold a start code.
            if codec == 1 {
                let rpu = frame
                    .bytes
                    .windows(4)
                    .any(|w| w[..3] == [0, 0, 1] && (w[3] >> 1) & 0x3f == 62);
                assert_eq!(rpu, hdr_format == 8, "{name}: RPU");
            }
        }
    }
    Ok(())
}
#[test]
#[ignore = "requires AMD AMF, independent FFprobe and an artifact directory"]
fn native_av1_geometry_and_hdr_are_preserved() -> Result<()> {
    use crate::capture::{ComGuard, Pixel};
    use std::{os::windows::process::CommandExt, path::PathBuf, process::Command};
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let directory = PathBuf::from(
        std::env::var_os("BUTTERPOLLO_TEST_GEOMETRY_DIRECTORY")
            .context("set BUTTERPOLLO_TEST_GEOMETRY_DIRECTORY")?,
    );
    std::fs::create_dir_all(&directory)?;
    let ffprobe =
        std::env::var_os("BUTTERPOLLO_TEST_FFPROBE").context("set BUTTERPOLLO_TEST_FFPROBE")?;
    let ffmpeg =
        std::env::var_os("BUTTERPOLLO_TEST_FFMPEG").context("set BUTTERPOLLO_TEST_FFMPEG")?;
    let options = butterpollo_config::config::Config::parse(
        "amd_usage=ultralowlatency\namd_quality=speed\namd_smart_access_video=disabled\n",
    )?;
    let mut reports = vec![];
    let mut exact = true;
    for (width, height) in [(1920, 1080), (1968, 2184), (2184, 1968)] {
        let mut pixels = Vec::with_capacity(width as usize * height as usize * 8);
        for y in 0..height {
            for x in 0..width {
                let value = [0.1, 0.5, 1., 4.]
                    [usize::from(x >= width / 2) + 2 * usize::from(y >= height / 2)];
                for value in [value, value, value, 1.] {
                    pixels.extend_from_slice(&half::f16::from_f32(value).to_le_bytes());
                }
            }
        }
        let source = GpuImage::upload(
            &gpu,
            &Image {
                width,
                height,
                stride: width as usize * 8,
                bytes: pixels,
                captured: Instant::now(),
                pixel: Pixel::RgbaF16,
            },
        )?;
        for hdr in [false, true] {
            let source = if hdr {
                source.clone()
            } else {
                let bytes = (0..width * height)
                    .flat_map(|index| {
                        let x = index % width;
                        let y = index / width;
                        let value = [25, 128, 255, 255]
                            [usize::from(x >= width / 2) + 2 * usize::from(y >= height / 2)];
                        [value, value, value, 255]
                    })
                    .collect();
                GpuImage::upload(
                    &gpu,
                    &Image {
                        width,
                        height,
                        stride: width as usize * 4,
                        bytes,
                        captured: Instant::now(),
                        pixel: Pixel::Bgra8,
                    },
                )?
            };
            for alignment in [3, 4] {
                let config = butterpollo_media_policy::rtsp::Negotiated {
                    width,
                    height,
                    codec: 2,
                    hdr,
                    fps: 120,
                    bitrate_kbps: 80000,
                    ..Default::default()
                };
                let result = Encoder::new_device_alignment_options(
                    &config,
                    gpu.clone(),
                    &options,
                    alignment,
                );
                let mut encoder = match result {
                    Ok(encoder) => encoder,
                    Err(error) => {
                        reports.push(serde_json::json!({"width":width,"height":height,"hdr":hdr,"alignment":alignment,"init_error":format!("{error:#}")}));
                        exact &= alignment != 3;
                        continue;
                    }
                };
                let read = |name: &str| -> Result<AMFVariantStruct> {
                    let mut value = int(0);
                    // SAFETY: `encoder.component` is the live AMF component the encoder owns; the
                    // name and `value` outlive the call.
                    unsafe {
                        check(((*(*encoder.component).pVtbl).GetProperty.unwrap())(
                            encoder.component,
                            to_wide(name).as_ptr(),
                            &mut value,
                        ))?;
                    }
                    Ok(value)
                };
                let applied_alignment = read("Av1AlignmentMode")?;
                let applied_size = read("Av1FrameSize")?;
                // SAFETY: Every AMFVariant union member read here is plain integers, valid for any
                // bit pattern.
                let properties = unsafe {
                    serde_json::json!({
                        "alignment_variant":applied_alignment.type_,
                        "alignment":applied_alignment.__bindgen_anon_1.int64Value,
                        "frame_size_variant":applied_size.type_,
                        "width":applied_size.__bindgen_anon_1.sizeValue.width,
                        "height":applied_size.__bindgen_anon_1.sizeValue.height,
                    })
                };
                let mut output = vec![];
                for frame in 0..8 {
                    output.extend(encoder.encode_gpu(&source, frame == 0, config.bitrate_kbps)?);
                }
                let deadline = Instant::now() + Duration::from_secs(3);
                while encoder.pending() {
                    output.extend(encoder.poll()?);
                    if Instant::now() >= deadline {
                        bail!("AV1 geometry probe output timed out");
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                assert_eq!(output.len(), 8);
                let name = format!("av1-{width}x{height}-hdr{hdr}-align{alignment}");
                let bitstream = directory.join(format!("{name}.obu"));
                std::fs::write(
                    &bitstream,
                    output
                        .iter()
                        .flat_map(|packet| packet.bytes.iter().copied())
                        .collect::<Vec<_>>(),
                )?;
                let decoded = output_within(
                    Command::new(&ffprobe)
                        .args(["-v", "error", "-f", "obu", "-i"])
                        .arg(&bitstream)
                        .args(["-show_frames", "-show_streams", "-of", "json"])
                        .creation_flags(0x08000000),
                )?;
                assert!(
                    decoded.status.success(),
                    "{}",
                    String::from_utf8_lossy(&decoded.stderr)
                );
                let decoded: serde_json::Value = serde_json::from_slice(&decoded.stdout)?;
                std::fs::write(
                    directory.join(format!("{name}.decoded.json")),
                    serde_json::to_vec_pretty(&decoded)?,
                )?;
                let frames = decoded["frames"]
                    .as_array()
                    .context("no independently decoded AV1 frames")?;
                let matches = frames.len() == 8
                    && frames.iter().all(|frame| {
                        frame["width"] == width
                            && frame["height"] == height
                            && (!hdr
                                || (frame["color_primaries"] == "bt2020"
                                    && frame["color_transfer"] == "smpte2084"))
                    });
                exact &= alignment != 3 || matches;
                let trace = output_within(
                    Command::new(&ffmpeg)
                        .args(["-hide_banner", "-nostdin", "-f", "obu", "-i"])
                        .arg(&bitstream)
                        .args([
                            "-map",
                            "0:v",
                            "-frames:v",
                            "1",
                            "-c:v",
                            "copy",
                            "-bsf:v",
                            "trace_headers",
                            "-f",
                            "null",
                            "-",
                        ])
                        .creation_flags(0x08000000),
                )?;
                assert!(trace.status.success(), "bitstream trace failed");
                std::fs::write(directory.join(format!("{name}.headers.log")), trace.stderr)?;
                reports.push(serde_json::json!({"width":width,"height":height,"hdr":hdr,"alignment":alignment,
                    "properties":properties,"exact_decoded_geometry_and_hdr":matches,"first_frame":frames.first()}));
            }
        }
    }
    std::fs::write(
        directory.join("geometry.json"),
        serde_json::to_vec_pretty(&reports)?,
    )?;
    assert!(
        exact,
        "AV1 exact decoded geometry failed; inspect geometry.json and encoded headers"
    );
    Ok(())
}
