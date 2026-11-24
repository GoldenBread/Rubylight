//! Native AMD AMF reference-budget, recovery, oversize scene-cut and drain tests.

use super::super::*;
use super::output_within;

#[test]
#[ignore = "requires AMD AMF, independent FFmpeg and a reference-budget report path"]
fn native_h264_one_reference_budget_decodes_without_ltr_corruption() -> Result<()> {
    use crate::capture::{ComGuard, Pixel};
    use std::{os::windows::process::CommandExt, path::PathBuf, process::Command};
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let decoder = std::env::var_os("BUTTERPOLLO_TEST_FFMPEG")
        .context("set BUTTERPOLLO_TEST_FFMPEG to an independent decoder")?;
    let report = PathBuf::from(
        std::env::var_os("BUTTERPOLLO_TEST_REFERENCE_BUDGET_REPORT")
            .context("set BUTTERPOLLO_TEST_REFERENCE_BUDGET_REPORT")?,
    );
    let directory = report
        .parent()
        .context("report requires a parent directory")?;
    std::fs::create_dir_all(directory)?;
    let config = butterpollo_media_policy::rtsp::Negotiated {
        width: 640,
        height: 480,
        references: 1,
        ..Default::default()
    };
    let options = butterpollo_config::config::Config::parse(
        "amd_ltr_frames=4\namd_usage=ultralowlatency\namd_quality=speed\n",
    )?;
    // Synthetic input avoids capturing or changing any user's desktop/window.
    let source = GpuImage::upload(
        &gpu,
        &Image {
            width: config.width,
            height: config.height,
            stride: config.width as usize * 4,
            bytes: (0..config.width * config.height)
                .flat_map(|index| {
                    let x = index % config.width;
                    let y = index / config.width;
                    [x as u8, y as u8, (x ^ y) as u8, 255]
                })
                .collect(),
            captured: Instant::now(),
            pixel: Pixel::Bgra8,
        },
    )?;
    let mut encoder = Encoder::new_device_options(&config, gpu, &options)?;
    let supports_invalidation = encoder.supports_invalidation();
    let mut output = vec![];
    for frame in 0..64 {
        output.extend(encoder.encode_gpu(&source, frame == 0, config.bitrate_kbps)?);
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    while encoder.pending() {
        output.extend(encoder.poll()?);
        if Instant::now() > deadline {
            bail!("AMF reference-budget fixture output timed out");
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let bitstream = directory.join(format!("amf-one-reference-{}.h264", std::process::id()));
    std::fs::write(
        &bitstream,
        output
            .iter()
            .flat_map(|packet| packet.bytes.iter().copied())
            .collect::<Vec<_>>(),
    )?;
    // Inspect the encoder's actual SPS independently. A software-only decode
    // can pass an oversized SPS that Moonlight's one-reference fixup would reject.
    let trace = output_within(
        Command::new(&decoder)
            .args(["-hide_banner", "-nostdin", "-f", "h264", "-i"])
            .arg(&bitstream)
            .args([
                "-map",
                "0:v",
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
    let headers = String::from_utf8_lossy(&trace.stderr);
    let values = |field: &str| -> Vec<u32> {
        headers
            .lines()
            .filter(|line| line.contains(field))
            .filter_map(|line| line.rsplit_once('=')?.1.trim().parse().ok())
            .collect()
    };
    let max_num_ref_frames = values("max_num_ref_frames");
    let max_dec_frame_buffering = values("max_dec_frame_buffering");
    std::fs::write(bitstream.with_extension("headers.log"), &trace.stderr)?;
    let decoded = output_within(
        Command::new(&decoder)
            .args([
                "-hide_banner",
                "-nostdin",
                "-v",
                "error",
                "-xerror",
                "-err_detect",
                "explode",
                "-f",
                "h264",
                "-i",
            ])
            .arg(&bitstream)
            .args([
                "-an",
                "-progress",
                "pipe:1",
                "-nostats",
                "-fps_mode",
                "passthrough",
                "-f",
                "null",
                "-",
            ])
            .creation_flags(0x08000000),
    )?;
    std::fs::write(bitstream.with_extension("decode.log"), &decoded.stderr)?;
    let decoded_frames = String::from_utf8_lossy(&decoded.stdout)
        .lines()
        .filter_map(|line| line.strip_prefix("frame=")?.trim().parse::<u32>().ok())
        .next_back()
        .unwrap_or(0);
    let observation = serde_json::json!({
        "negotiated_references": config.references,
        "requested_ltr_frames": 4,
        "supports_invalidation": supports_invalidation,
        "applied_max_num_ref_frames": encoder.read("MaxNumRefFrames"),
        "applied_max_ltr_frames": encoder.read("MaxOfLTRFrames"),
        "encoded_frames": output.len(),
        "decoded_frames": decoded_frames,
        "sps_max_num_ref_frames": max_num_ref_frames,
        "vui_max_dec_frame_buffering": max_dec_frame_buffering,
        "header_trace_success": trace.status.success(),
        "decode_success": decoded.status.success(),
        "decode_errors": String::from_utf8_lossy(&decoded.stderr),
        "bitstream": bitstream,
    });
    std::fs::write(&report, serde_json::to_vec_pretty(&observation)?)?;
    eprintln!("{observation}");
    assert!(
        !supports_invalidation,
        "one-reference clients must use IDR recovery"
    );
    assert_eq!(output.len(), 64);
    assert!(trace.status.success(), "independent SPS trace failed");
    assert!(
        !max_num_ref_frames.is_empty(),
        "trace contained no SPS reference count"
    );
    assert!(
        max_num_ref_frames.iter().all(|&count| count <= 1),
        "oversized SPS reference budget"
    );
    assert!(
        max_dec_frame_buffering.iter().all(|&count| count <= 1),
        "oversized VUI decoder buffer budget"
    );
    assert!(
        decoded.status.success() && decoded.stderr.is_empty(),
        "independent decoder rejected stream"
    );
    assert_eq!(decoded_frames, 64);
    Ok(())
}

#[test]
#[ignore = "requires a live AMD D3D11 adapter and AMF runtime"]
fn native_reference_recovery_decodes_after_dropping_the_invalidated_packets() -> Result<()> {
    let _com = crate::capture::ComGuard::new()?;
    let mut capture = crate::capture::Capture::new_format("", "wgc", false)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let image = loop {
        if let Some(image) = capture.next_gpu(0)? {
            break image;
        }
        if Instant::now() > deadline {
            bail!("capture fixture timed out");
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    let options = butterpollo_config::config::Config::parse("amd_ltr_frames=4\n")?;
    let mut reports = vec![];
    for codec in 0..3 {
        let config = butterpollo_media_policy::rtsp::Negotiated {
            width: 640,
            height: 480,
            codec,
            ..Default::default()
        };
        let mut encoder = Encoder::new_device_options(&config, image.gpu.clone(), &options)?;
        let supported = encoder.supports_invalidation();
        let mut output = vec![];
        for frame in 1..=64 {
            let mut idr = frame == 1;
            if frame == 9 && supported {
                assert!(encoder.invalidate_ref_frames(5, 8));
            }
            if frame == 21 && supported {
                idr = !encoder.invalidate_ref_frames(17, 20);
                assert_eq!(idr, codec == 0);
            }
            output.extend(encoder.encode_gpu(&image, idr, config.bitrate_kbps)?);
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while encoder.pending() {
            output.extend(encoder.poll()?);
            if Instant::now() > deadline {
                bail!("AMF output fixture timed out");
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(output.len(), 64);
        if supported {
            assert!(output[8].after_invalidation);
            assert!(!output[8].idr);
            assert_eq!(output[20].after_invalidation, codec != 0);
            assert_eq!(output[20].idr, codec == 0);
        }
        use std::os::windows::process::CommandExt;
        let decoder = std::env::var_os("BUTTERPOLLO_TEST_FFMPEG")
            .context("set BUTTERPOLLO_TEST_FFMPEG to an independent decoder")?;
        let report = std::path::PathBuf::from(
            std::env::var_os("BUTTERPOLLO_TEST_RFI_REPORT")
                .context("set BUTTERPOLLO_TEST_RFI_REPORT")?,
        );
        let directory = report
            .parent()
            .context("report requires a parent directory")?;
        let format = match codec {
            0 => "h264",
            1 => "hevc",
            _ => "obu",
        };
        let path = directory.join(format!("amf-rfi-{}-{codec}.{format}", std::process::id()));
        let bitstream: Vec<_> = output
            .iter()
            .enumerate()
            .filter(|(index, _)| {
                !supported || !((4..8).contains(index) || (16..20).contains(index))
            })
            .flat_map(|(_, packet)| packet.bytes.iter().copied())
            .collect();
        std::fs::write(&path, bitstream)?;
        std::fs::write(
            path.with_extension(format!("full.{format}")),
            output
                .iter()
                .flat_map(|packet| packet.bytes.iter().copied())
                .collect::<Vec<_>>(),
        )?;
        eprintln!(
            "codec={codec}, supported={supported}, fixture={}",
            path.display()
        );
        let decoded_output = output_within(
            std::process::Command::new(&decoder)
                .args([
                    "-hide_banner",
                    "-nostdin",
                    "-v",
                    "error",
                    "-xerror",
                    "-err_detect",
                    "explode",
                    "-f",
                    format,
                    "-i",
                ])
                .arg(&path)
                .args([
                    "-an",
                    "-progress",
                    "pipe:1",
                    "-nostats",
                    "-fps_mode",
                    "passthrough",
                    "-f",
                    "null",
                    "-",
                ])
                .creation_flags(0x08000000),
        )?;
        let stderr = String::from_utf8_lossy(&decoded_output.stderr);
        assert!(
            decoded_output.status.success() && stderr.is_empty(),
            "codec={codec}, {stderr}"
        );
        let decoded = String::from_utf8_lossy(&decoded_output.stdout)
            .lines()
            .filter_map(|line| line.strip_prefix("frame=")?.trim().parse::<u32>().ok())
            .next_back()
            .context("decoder reported no frames")?;
        assert_eq!(decoded, if supported { 56 } else { 64 });
        reports.push(serde_json::json!({"codec":codec,"frames":output.len(),"decoded":decoded,"ltr_supported":supported,"recovery_frames":output.iter().filter(|frame|frame.after_invalidation).count(),"wrap_idr_fallback":supported&&codec==0,"dropped_frames":if supported{8}else{0}}));
    }
    if let Some(path) = std::env::var_os("BUTTERPOLLO_TEST_RFI_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&reports)?)?;
    }
    eprintln!("{}", serde_json::to_string(&reports)?);
    Ok(())
}

#[test]
#[ignore = "requires a live AMD D3D11 adapter and AMF runtime"]
fn native_an_oversize_scene_cut_goes_out_once_as_a_keyframe_at_its_own_index() -> Result<()> {
    let _com = crate::capture::ComGuard::new()?;
    let device = Device::new("")?;
    let options = butterpollo_config::config::Config::parse("amd_ltr_frames=4\n")?;
    for codec in 0..3 {
        let config = butterpollo_media_policy::rtsp::Negotiated {
            width: 1280,
            height: 720,
            codec,
            ..Default::default()
        };
        let image = |noise: bool| {
            let mut seed = 0x2545_f491_4f6c_dd1d_u64;
            let bytes = (0..config.width as usize * config.height as usize * 4)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    if noise { seed as u8 } else { 128 }
                })
                .collect();
            GpuImage::upload(
                &device,
                &Image {
                    width: config.width,
                    height: config.height,
                    stride: config.width as usize * 4,
                    bytes,
                    captured: Instant::now(),
                    pixel: crate::capture::Pixel::Bgra8,
                },
            )
        };
        let (flat, noise) = (image(false)?, image(true)?);
        let mut encoder = Encoder::new_device_options(&config, device.clone(), &options)?;
        assert!(encoder.supports_invalidation(), "codec {codec}: LTR");
        // Flat frames are a few hundred bytes; the cut to noise is far past it.
        encoder.set_cut_limit(100_000);
        let mut output = vec![];
        for frame in 1..=30 {
            let image = if frame < 21 { &flat } else { &noise };
            output.extend(encoder.encode_gpu(image, frame == 1, config.bitrate_kbps)?);
            let deadline = Instant::now() + Duration::from_secs(2);
            while encoder.pending() && Instant::now() < deadline {
                output.extend(encoder.poll()?);
            }
        }
        // One output a frame, the cut a keyframe in place of its inter frame,
        // and the static noise after it inter frames that reference it.
        let keyframes: Vec<_> = output
            .iter()
            .enumerate()
            .filter(|(_, frame)| frame.idr)
            .map(|(index, _)| index + 1)
            .collect();
        assert_eq!(output.len(), 30, "codec {codec}");
        assert_eq!(keyframes, [1, 21], "codec {codec}");
        assert_eq!(encoder.reencoded_cuts(), 1, "codec {codec}");
        // The keyframe is held to the frame cap, as a requested one is.
        let sizes: Vec<_> = output[19..].iter().map(|frame| frame.bytes.len()).collect();
        println!(
            "codec {codec}: frames 20 to 30 {sizes:?}, cap {:?}",
            encoder.frame_cap()
        );
        assert!(output[21..].iter().all(|frame| frame.bytes.len() < 100_000));
        // A loss after the cut is repaired from a reference since the keyframe
        // (whatever it costs: noise makes a repair as large as a keyframe).
        encoder.references.repair_whatever_it_costs();
        assert!(encoder.invalidate_ref_frames(25, 26), "codec {codec}");
        assert!(!encoder.recover);
    }
    Ok(())
}

#[test]
#[ignore = "requires a live AMD D3D11 adapter and AMF runtime"]
fn native_a_stream_larger_than_the_encoder_takes_fails_as_size_too_large() -> Result<()> {
    let _com = crate::capture::ComGuard::new()?;
    for codec in [0, 1, 2] {
        let config = butterpollo_media_policy::rtsp::Negotiated {
            width: 8192,
            height: 8192,
            codec,
            ..Default::default()
        };
        let Err(error) = Encoder::new_device(&config, Device::new("")?) else {
            bail!("AMF took 8192x8192 for codec {codec}");
        };
        let size = error
            .downcast_ref::<crate::encoder::SizeTooLarge>()
            .with_context(|| format!("codec {codec}: {error:#}"))?;
        assert_eq!((size.width, size.height), (8192, 8192), "codec {codec}");
        let (max_w, max_h) = size.max.context("AMF gave no limit")?;
        assert!(max_w < 8192 || max_h < 8192, "codec {codec}: {size}");
    }
    Ok(())
}

#[test]
fn a_released_encoder_drains_until_amf_says_every_frame_is_out() {
    let far = Instant::now() + Duration::from_secs(5);
    // Two frames in flight, then EOF.
    let mut outputs = vec![
        (AMF_RESULT_AMF_EOF, false),
        (AMF_RESULT_AMF_OK, true),
        (AMF_RESULT_AMF_REPEAT, false),
        (AMF_RESULT_AMF_OK, true),
    ];
    let drained = drain_outputs(far, || AMF_RESULT_AMF_OK, || outputs.pop().unwrap());
    assert_eq!(
        drained,
        Drained {
            outputs: 2,
            complete: true
        }
    );

    // A full queue gives up an output before Drain is taken.
    let mut answers = vec![AMF_RESULT_AMF_OK, AMF_RESULT_AMF_INPUT_FULL];
    let mut outputs = vec![(AMF_RESULT_AMF_EOF, false), (AMF_RESULT_AMF_OK, true)];
    let drained = drain_outputs(far, || answers.pop().unwrap(), || outputs.pop().unwrap());
    assert_eq!(drained.outputs, 1);
    assert!(drained.complete);

    // An error ends the drain; so does the deadline.
    let drained = drain_outputs(far, || AMF_RESULT_AMF_OK, || (AMF_RESULT_AMF_FAIL, false));
    assert!(!drained.complete);
    let past = Instant::now();
    let drained = drain_outputs(past, || unreachable!(), || unreachable!());
    assert_eq!(drained.outputs, 0);
    assert!(!drained.complete);
    let soon = Instant::now() + Duration::from_millis(20);
    let drained = drain_outputs(
        soon,
        || AMF_RESULT_AMF_OK,
        || (AMF_RESULT_AMF_REPEAT, false),
    );
    assert!(
        !drained.complete,
        "a frame that never leaves is given up on"
    );
}
