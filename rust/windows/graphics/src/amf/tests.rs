//! AMF encoder unit and native test hub with shared external-tool timeout helpers.

use super::*;

/// How long an external tool (ffprobe, ffmpeg) may take before the test
/// fails. An ffprobe once sat on one AV1 frame for 40 minutes while the
/// test held the GPU.
const TOOL_TIMEOUT: Duration = Duration::from_secs(120);

/// `command`'s output, as `Command::output` gives it, but killed and failed
/// after `timeout`.
fn output_within_for(
    command: &mut std::process::Command,
    timeout: Duration,
) -> Result<std::process::Output> {
    use std::{io::Read, process::Stdio};
    let program = command.get_program().to_string_lossy().into_owned();
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("starting {program}"))?;
    let read = |stream: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut stream) = stream {
                let _ = stream.read_to_end(&mut bytes);
            }
            bytes
        })
    };
    let stdout = read(child.stdout.take().map(|s| Box::new(s) as _));
    let stderr = read(child.stderr.take().map(|s| Box::new(s) as _));
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("{program} did not finish within {timeout:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    Ok(std::process::Output {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

/// [`output_within_for`] with [`TOOL_TIMEOUT`].
fn output_within(command: &mut std::process::Command) -> Result<std::process::Output> {
    output_within_for(command, TOOL_TIMEOUT)
}

#[test]
fn a_tool_that_hangs_fails_the_test_instead_of_holding_it() {
    let started = Instant::now();
    let error = output_within_for(
        std::process::Command::new("cmd.exe").args(["/d", "/c", "ping -n 30 127.0.0.1 >nul"]),
        Duration::from_millis(500),
    )
    .unwrap_err();
    assert!(error.to_string().contains("did not finish"), "{error:#}");
    assert!(started.elapsed() < Duration::from_secs(10));
    // One that finishes gives its output as Command::output would.
    let done =
        output_within(std::process::Command::new("cmd.exe").args(["/d", "/c", "echo hi"])).unwrap();
    assert!(done.status.success());
    assert_eq!(String::from_utf8_lossy(&done.stdout).trim(), "hi");
}

#[test]
fn missing_output_type_uses_the_matching_submission_after_buffering_or_recreation() {
    // A buffered initial keyframe and a recovery keyframe at a continued wire
    // index still start a decoder when the driver cannot report its output type.
    for (pts, requested) in [(0, true), (1, false), (37, true), (38, false)] {
        let submission = Submission {
            pts,
            started: Instant::now(),
            accepted: Instant::now(),
            presentation: Instant::now(),
            idr: requested,
            after_invalidation: false,
            frame_cap_bytes: Some(100_000),
            frame_cap_target: Some(100_000),
            skipped: 0,
            cut_limit: None,
            reencoded: false,
            _capture: None,
            _converted: None,
            _converted12: None,
        };
        assert_eq!(output_is_idr(None, submission.idr), requested);
        // A real driver report remains authoritative over a requested type.
        assert!(output_is_idr(Some(0), submission.idr));
        for reported in [1, 2, 3] {
            assert!(!output_is_idr(Some(reported), submission.idr));
        }
    }
    // A late, already-expired output has no request metadata to fall back to.
    assert!(!output_is_idr(None, false));
}

#[test]
fn a_slow_first_probe_frame_is_logged_but_a_slow_stream_frame_warns_the_client() {
    // The 780M's service start: the startup probe's first H.264 frame took
    // over two seconds, then every codec worked. That is no warning.
    let Unreturned::Probe(message) = unreturned_notice(true, Some(0)) else {
        panic!("a probe frame must not raise a stream warning");
    };
    assert!(message.contains("probe's frame Some(0)"), "{message}");
    assert!(!message.contains("You may see a pause"), "{message}");
    let Unreturned::Warning(message) = unreturned_notice(false, Some(0)) else {
        panic!("a stream frame must warn");
    };
    assert!(message.contains("You may see a pause"), "{message}");
}

#[test]
fn a_frame_counts_as_unreturned_from_its_acceptance_not_from_its_conversion() {
    // The RX 7900 XT's packed probe: the first frame's upload and conversion
    // took 4.7 s, then AMF accepted it. Counting from the start dropped it in
    // the same encode call, before AMF had a chance to return it.
    let now = Instant::now();
    let submission = |started: Duration, accepted: Duration| Submission {
        pts: 0,
        started: now - started,
        accepted: now - accepted,
        presentation: now - started,
        idr: true,
        after_invalidation: false,
        frame_cap_bytes: None,
        frame_cap_target: None,
        skipped: 0,
        cut_limit: None,
        reencoded: false,
        _capture: None,
        _converted: None,
        _converted12: None,
    };
    let slow_conversion = submission(Duration::from_millis(4700), Duration::ZERO);
    assert!(!slow_conversion.unreturned(now));
    assert!(!slow_conversion.unreturned(now + Duration::from_millis(1900)));
    assert!(slow_conversion.unreturned(now + Duration::from_millis(2100)));
    // A frame AMF has held for over two seconds is still lost.
    let held = submission(Duration::from_millis(2600), Duration::from_millis(2500));
    assert!(held.unreturned(now));
}

#[test]
#[ignore = "requires a live AMD D3D11 adapter and AMF runtime"]
fn native_output_survives_a_late_poll_without_losing_submission_metadata() -> Result<()> {
    let _com = crate::capture::ComGuard::new()?;
    let device = Device::new("")?;
    let config = butterpollo_media_policy::rtsp::Negotiated {
        width: 640,
        height: 480,
        codec: 0,
        ..Default::default()
    };
    let presented = Instant::now();
    let image = GpuImage::upload(
        &device,
        &Image {
            width: config.width,
            height: config.height,
            stride: config.width as usize * 4,
            bytes: vec![128; config.width as usize * config.height as usize * 4],
            captured: presented,
            pixel: crate::capture::Pixel::Bgra8,
        },
    )?;
    let mut encoder = Encoder::new_device(&config, device)?;
    let mut converter = gpu::Converter::new(
        &encoder._device,
        &config,
        (image.width, image.height, image.pixel),
    )?;
    let (surface, converted) =
        converter.convert(encoder.context, &image, &mut encoder.ownership)?;
    encoder.gpu_convert = Some(converter);
    let (plan, idr) = encoder.prepare_surface(surface.0, true)?;
    // Submit without the convenience encode method's immediate poll. This
    // guarantees that QueryOutput must collect an accepted native picture,
    // independently of how quickly this particular GPU completes it.
    // SAFETY: The surface and component are owned by the live encoder/context;
    // AMF retains the input, and its ownership callback retains the texture.
    unsafe {
        ((*(*surface.0).pVtbl).SetPts.unwrap())(surface.0, encoder.index);
        check(((*(*encoder.component).pVtbl).SubmitInput.unwrap())(
            encoder.component,
            surface.0.cast(),
        ))?;
    }
    let delayed = Instant::now() - Duration::from_secs(3);
    encoder.in_flight.push_back(Submission {
        pts: encoder.index,
        started: delayed,
        accepted: delayed,
        presentation: presented,
        idr,
        after_invalidation: plan.after_invalidation,
        frame_cap_bytes: encoder.frame_cap(),
        frame_cap_target: encoder.frame_cap_target,
        skipped: plan.skipped,
        cut_limit: None,
        reencoded: false,
        _capture: Some(image),
        _converted: Some(converted),
        _converted12: None,
    });
    // As a submission does: the output thread queries while frames are in flight.
    if let Some(thread) = &encoder.output_thread {
        thread.want(true);
    }
    encoder.backlog.submitted();
    encoder.references.accepted(encoder.index as u64 + 1, &plan);
    encoder.index += 1;
    drop(surface);
    // Let the small synthetic frame complete while the caller does not poll,
    // as happens when its thread is descheduled while the GPU keeps working.
    std::thread::sleep(Duration::from_millis(100));
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut output = Vec::new();
    while encoder.pending() && Instant::now() < deadline {
        output.extend(encoder.poll()?);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        output.len(),
        1,
        "late polling must still query native output"
    );
    assert!(!encoder.pending());
    // The poll that took the last frame parks the output thread: it would
    // otherwise query until the next submission.
    assert!(
        encoder
            .output_thread
            .as_ref()
            .is_none_or(|thread| !thread.wanted()),
        "the output thread stops once nothing is in flight"
    );
    assert!(!output[0].bytes.is_empty());
    assert!(output[0].idr);
    assert_eq!(output[0].presentation, Some(presented));
    assert!(
        output[0]
            .latency
            .is_some_and(|latency| latency >= Duration::from_secs(3))
    );
    assert!(!encoder.recover, "a late poll is not a lost reference");
    Ok(())
}

#[test]
#[ignore = "requires a live AMD adapter with H.264, HEVC and AV1 encoding"]
fn native_runtime_frame_caps_survive_bitrate_changes_and_follow_submissions() -> Result<()> {
    let _com = crate::capture::ComGuard::new()?;
    let device = Device::new("")?;
    for codec in 0..=2 {
        let config = butterpollo_media_policy::rtsp::Negotiated {
            width: 1280,
            height: 720,
            codec,
            bitrate_kbps: 54_000,
            fps: 120,
            rate_millihz: 119_880,
            ..Default::default()
        };
        let mut image = GpuImage::upload(
            &device,
            &Image {
                width: config.width,
                height: config.height,
                stride: config.width as usize * 4,
                bytes: (0..config.width as usize * config.height as usize * 4)
                    .map(|index| (index % 251) as u8)
                    .collect(),
                captured: Instant::now(),
                pixel: crate::capture::Pixel::Bgra8,
            },
        )?;
        let mut encoder = Encoder::new_device(&config, device.clone())
            .with_context(|| format!("native runtime cap codec {codec}"))?;
        let mut expected = Vec::new();
        let mut output = Vec::new();
        for (pace, bitrate) in [
            (800_000_000, 54_000),
            (70_770_000, 54_000),
            (54_000_000, 27_000),
            (800_000_000, 54_000),
        ] {
            let cap = butterpollo_media_policy::network_pacing::recovery_frame_cap(
                pace,
                bitrate,
                Duration::from_secs_f64(1. / 119.88),
                10,
                1392,
                false,
            );
            assert_eq!(
                encoder.set_frame_cap(cap, 0),
                Some(cap),
                "codec {codec}: runtime cap rejected at pace {pace}"
            );
            image.captured = Instant::now();
            expected.push((image.captured, cap));
            output.extend(encoder.encode_gpu(&image, true, bitrate)?);
            assert_eq!(
                encoder.read(rate_control::frame_cap_name(codec)),
                Some((cap * 8) as i64),
                "codec {codec}: bitrate update changed the cap"
            );
            println!("codec={codec} pace_bps={pace} bitrate_kbps={bitrate} frame_cap_bytes={cap}");
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while encoder.pending() && Instant::now() < deadline {
            output.extend(encoder.poll()?);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(
            output.len(),
            expected.len(),
            "codec {codec}: missing output"
        );
        for frame in output {
            assert!(!frame.bytes.is_empty());
            assert!(frame.idr);
            let (_, cap) = expected
                .iter()
                .find(|(presentation, _)| Some(*presentation) == frame.presentation)
                .expect("output must match a submitted presentation");
            assert_eq!(
                frame.frame_cap_bytes,
                Some(*cap),
                "codec {codec}: output used a later cap"
            );
        }
    }
    Ok(())
}

#[test]
fn output_range_preserves_optional_input_and_limited_driver_defaults() -> Result<()> {
    for (codec, input, output) in [
        (0, "InputFullRangeColor", "FullRangeColor"),
        (1, "HevcInputFullRangeColor", "HevcNominalRange"),
        (2, "Av1InputFullRangeColor", "Av1NominalRange"),
    ] {
        let mut calls = Vec::new();
        let range = OutputRange::configure(codec, false, |name, full| {
            calls.push((name.to_string(), full));
            // Older drivers can reject either property. Limited remains the
            // documented default, including the RX 6800 XT issue #5 case.
            bail!("mock AMF_INVALID_ARG")
        });
        assert_eq!(
            calls,
            [(input.to_string(), false), (output.to_string(), false)]
        );
        assert_eq!(range.name, output);
        range.validate(None)?;
        range.validate(Some(0))?;
        assert!(range.validate(Some(1)).is_err());

        let range = OutputRange::configure(codec, true, |name, full| {
            assert!(full);
            if name == input {
                bail!("optional input property unavailable");
            }
            Ok(())
        });
        range.validate(Some(1))?;
        // A successful output setter remains sufficient if this driver
        // cannot read back a supported property.
        range.validate(None)?;
    }
    Ok(())
}

#[test]
fn output_range_verifies_rejected_setters_and_initialized_readback() -> Result<()> {
    for codec in 0..=2 {
        let rejected =
            OutputRange::configure(codec, true, |_, _| bail!("output setter unavailable"));
        // A preset or the colour profile can still have selected full range.
        rejected.validate(Some(1))?;
        assert!(rejected.validate(Some(0)).is_err());
        assert!(rejected.validate(None).is_err());

        let accepted = OutputRange::configure(codec, true, |_, _| Ok(()));
        accepted.validate(Some(1))?;
        // If Init reverted the setting, its current value wins over setter
        // success. Unknown values must not be interpreted as full range.
        assert!(accepted.validate(Some(0)).is_err());
        assert!(accepted.validate(Some(2)).is_err());
    }
    Ok(())
}

#[test]
fn settings_log_shows_each_codecs_profile_and_bit_depth() {
    // A 10-bit SDR stream is told apart from 8-bit only by these two, as the
    // driver applied them.
    for (codec, expected) in [
        (0, ["Profile", "ColorBitDepth"]),
        (1, ["HevcProfile", "HevcColorBitDepth"]),
        (2, ["Av1Profile", "Av1ColorBitDepth"]),
    ] {
        let names: Vec<_> = Encoder::setting_names(codec)
            .iter()
            .copied()
            .filter(|name| name.ends_with("Profile") || name.ends_with("ColorBitDepth"))
            .collect();
        assert_eq!(names, expected);
    }
}

#[test]
fn settings_log_queries_each_codecs_frame_skip_property() {
    for (codec, expected) in [
        (0, "RateControlSkipFrameEnable"),
        (1, "HevcRateControlSkipFrameEnable"),
        (2, "Av1RateControlSkipFrameEnable"),
    ] {
        let names: Vec<_> = Encoder::setting_names(codec)
            .iter()
            .copied()
            .filter(|name| name.contains("RateControlSkipFrame"))
            .collect();
        assert_eq!(names, [expected]);
    }
}

#[test]
fn settings_log_queries_screen_content_and_high_motion_tools() {
    // Whether the driver keeps these on by default decides what the opt-in
    // settings change, so the settings line reads them back.
    for (codec, expected) in [
        (0, &["HighMotionQualityBoostEnable"][..]),
        (1, &["HevcHighMotionQualityBoostEnable"][..]),
        (
            2,
            &[
                "Av1HighMotionQualityBoost",
                "Av1ScreenContentTools",
                "Av1PaletteMode",
                "Av1ForceIntegerMv",
            ][..],
        ),
    ] {
        let names: Vec<_> = Encoder::setting_names(codec)
            .iter()
            .copied()
            .filter(|name| {
                name.contains("HighMotionQualityBoost")
                    || ["ScreenContentTools", "PaletteMode", "ForceIntegerMv"]
                        .iter()
                        .any(|tool| name.ends_with(tool))
            })
            .collect();
        assert_eq!(names, expected);
    }
}

#[test]
fn amf_errors_name_the_result_and_say_when_the_hardware_cannot() {
    let text = super::amf_error_text(AMF_RESULT_AMF_NOT_SUPPORTED);
    assert!(text.starts_with("AMF error AMF_NOT_SUPPORTED ("), "{text}");
    assert!(text.contains("no encoder for the request"), "{text}");
    let text = super::amf_error_text(AMF_RESULT_AMF_ENCODER_NOT_PRESENT);
    assert!(text.contains("AMF_ENCODER_NOT_PRESENT"), "{text}");
    assert!(text.contains("no encoder for the request"), "{text}");
    // A code without a name keeps the number, as before.
    assert_eq!(super::amf_error_text(9999), "AMF error 9999");
    assert_eq!(
        super::check(AMF_RESULT_AMF_FAIL).unwrap_err().to_string(),
        "AMF error AMF_FAIL (1)"
    );
    assert!(super::check(AMF_RESULT_AMF_OK).is_ok());
}

#[test]
fn only_an_inter_frame_past_the_full_fec_bound_is_encoded_again_once() {
    // The October 28 run: a scene cut after steady 380 KB frames came out as
    // a 2.4 MB inter frame against a 1.17 MB bound and went without parity.
    let limit = Some(1_170_000);
    assert!(reencodes_cut(false, 2_400_000, limit, true));
    // A frame within the bound, a requested keyframe, the re-encode itself
    // (also a keyframe, so never a second time) and a stream without the
    // bound go as AMF made them.
    assert!(!reencodes_cut(false, 1_170_000, limit, true));
    assert!(!reencodes_cut(false, 380_000, limit, true));
    assert!(!reencodes_cut(true, 2_400_000, limit, true));
    assert!(!reencodes_cut(false, 2_400_000, None, true));
    // A later submission already references the frame, or one is being
    // prepared against the references as they are: it stays.
    assert!(!reencodes_cut(false, 2_400_000, limit, false));
}

#[test]
fn a_keyframe_is_over_its_cap_past_one_tenth_more() {
    // #59 rc.4 run 1: a 1,908,217 B recovery keyframe against a 1,166,893 B cap.
    assert!(over_cap_target(1_908_217, Some(1_166_893)));
    assert!(over_cap_target(1_100_001, Some(1_000_000)));
    assert!(!over_cap_target(1_100_000, Some(1_000_000)));
    assert!(!over_cap_target(400_000, Some(1_000_000)));
    // No target, or the driver's unrestricted one: nothing to be over.
    assert!(!over_cap_target(1_908_217, None));
    assert!(!over_cap_target(1_908_217, Some(0)));
}

#[test]
fn only_an_automatic_cap_is_a_keyframe_target() {
    assert_eq!(frame_cap_target(true, 1_166_893), Some(1_166_893));
    assert_eq!(frame_cap_target(true, 0), None);
    // An explicit amd_max_frame_size, or driver default: nothing to be over.
    assert_eq!(frame_cap_target(false, 1_166_893), None);
    assert!(!over_cap_target(
        1_908_217,
        frame_cap_target(false, 1_166_893)
    ));
}

mod hdr_tests;
mod native_hdr_tests;
mod native_recovery_tests;
mod rate_control_tests;
