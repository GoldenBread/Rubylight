//! Fake-driver tests for NVENC HDR10 metadata units, snapshotting, keyframe retries,
//! and decoded HDR metadata verification.

use super::*;

#[test]
fn hdr_metadata_uses_each_codecs_units_and_bounds_av1_chromaticities() {
    let source = butterpollo_media_policy::hdr::Metadata {
        minimum: 20,
        max_cll: 900,
        max_fall: 400,
        ..Default::default()
    };
    let mut hevc = Metadata::new(1);
    hevc.update(source, 1);
    assert_eq!(
        hevc.encoded(),
        EncodedMetadata {
            primaries: [[35400, 14600], [8500, 39850], [6550, 2300]],
            white: [15635, 16450],
            maximum: 10_000_000,
            minimum: 20,
            max_cll: 900,
            max_fall: 400,
        }
    );
    let mut av1 = Metadata::new(2);
    av1.update(source, 2);
    assert_eq!(
        av1.encoded(),
        EncodedMetadata {
            primaries: [[46399, 19137], [11141, 52232], [8585, 3015]],
            white: [20493, 21561],
            maximum: 256_000,
            minimum: 33,
            max_cll: 900,
            max_fall: 400,
        }
    );
    av1.update(
        butterpollo_media_policy::hdr::Metadata {
            primaries: [[u16::MAX; 2]; 3],
            white: [0, 50000],
            maximum_nits: u16::MAX,
            minimum: u16::MAX,
            ..source
        },
        2,
    );
    assert_eq!(av1.encoded().primaries, [[u16::MAX; 2]; 3]);
    assert_eq!(av1.encoded().white, [0, u16::MAX]);
    assert_eq!(av1.encoded().maximum, 16_776_960);
    assert_eq!(av1.encoded().minimum, 107_373);
}

#[test]
fn hdr_output_flags_and_submissions_respect_codec_api_and_sdr() -> Result<()> {
    for codec in [1, 2] {
        for api in [ApiVersion(13, 0), ApiVersion(12, 2), ApiVersion(12, 1)] {
            for hdr in [false, true] {
                for preset_hdr_metadata in [false, true] {
                    reset();
                    state(|d| d.preset_hdr_metadata = preset_hdr_metadata);
                    let mut encoder = session(
                        &Negotiated {
                            codec,
                            hdr,
                            ..Default::default()
                        },
                        api,
                    )?;
                    let enabled = hdr && api >= ApiVersion(13, 0);
                    // SAFETY: The configured codec selects its plain-data union member.
                    let (mastering, light) = unsafe {
                        if codec == 1 {
                            let config = encoder.config.encodeCodecConfig.hevcConfig;
                            (config.outputMasteringDisplay(), config.outputMaxCll())
                        } else {
                            let config = encoder.config.encodeCodecConfig.av1Config;
                            (config.outputMasteringDisplay(), config.outputMaxCll())
                        }
                    };
                    assert_eq!((mastering, light), (u32::from(enabled), u32::from(enabled)));
                    encoder.set_hdr_metadata(butterpollo_media_policy::hdr::Metadata {
                        maximum_nits: 1400,
                        max_cll: 1300,
                        max_fall: 800,
                        ..Default::default()
                    });
                    let slot = input(&mut encoder, 17)?;
                    encoder.submit(slot, None, false, Instant::now())?;
                    state(|d| {
                        let buffer = d
                            .outputs
                            .get(&(encoder.slots[slot].output as usize))
                            .unwrap();
                        assert_eq!(buffer.metadata.is_some(), enabled);
                        assert_eq!(buffer.light.is_some(), enabled);
                    });
                    assert_eq!(encoder.poll()?[0].idr, enabled);
                    drop(encoder);
                    assert_clean();
                }
            }
        }
    }
    Ok(())
}

#[test]
fn hdr_metadata_is_snapshotted_until_the_gpu_submission_completes() -> Result<()> {
    for codec in [1, 2] {
        reset();
        state(|d| d.asynchronous = true);
        let mut encoder = session(
            &Negotiated {
                codec,
                hdr: true,
                ..Default::default()
            },
            ApiVersion(13, 0),
        )?;
        let first = butterpollo_media_policy::hdr::Metadata {
            max_cll: 550,
            max_fall: 300,
            ..butterpollo_media_policy::hdr::Metadata::display(600., 0.002, 300.)
        };
        let second = butterpollo_media_policy::hdr::Metadata {
            max_cll: 1300,
            max_fall: 800,
            ..butterpollo_media_policy::hdr::Metadata::display(1400., 0.001, 800.)
        };
        let slot = input(&mut encoder, 17)?;
        encoder.set_hdr_metadata(first);
        encoder.submit(slot, None, false, Instant::now())?;
        encoder.set_hdr_metadata(second);
        let scale = if codec == 2 { 256 } else { 10000 };
        assert_eq!(encoder.slots[slot].metadata.mastering.maxLuma, 600 * scale);
        assert_eq!(encoder.slots[slot].metadata.light.maxContentLightLevel, 550);
        assert_eq!(
            encoder.slots[slot].metadata.light.maxPicAverageLightLevel,
            300
        );
        assert!(encoder.poll()?[0].idr);
        encoder.submit(slot, None, false, Instant::now())?;
        assert_eq!(encoder.slots[slot].metadata.mastering.maxLuma, 1400 * scale);
        assert_eq!(
            encoder.slots[slot].metadata.light.maxContentLightLevel,
            1300
        );
        assert!(encoder.poll()?[0].idr);
        // Identical refreshes, control-only values and changes superseded before
        // submission must not produce another keyframe.
        encoder.set_hdr_metadata(second);
        encoder.set_hdr_metadata(first);
        encoder.set_hdr_metadata(butterpollo_media_policy::hdr::Metadata {
            full_frame_nits: 900,
            ..second
        });
        encoder.submit(slot, None, false, Instant::now())?;
        assert!(!encoder.poll()?[0].idr);
        // Light-level-only updates also need a keyframe.
        encoder.set_hdr_metadata(butterpollo_media_policy::hdr::Metadata {
            max_fall: 700,
            ..second
        });
        encoder.submit(slot, None, false, Instant::now())?;
        assert!(encoder.poll()?[0].idr);
        encoder.submit(slot, None, false, Instant::now())?;
        assert!(!encoder.poll()?[0].idr);
        drop(encoder);
        assert_clean();
    }
    Ok(())
}

#[test]
fn hdr_keyframe_retries_failed_submissions_and_consumes_simultaneous_requests() -> Result<()> {
    for codec in [1, 2] {
        reset();
        let mut encoder = session(
            &Negotiated {
                codec,
                hdr: true,
                ..Default::default()
            },
            ApiVersion(13, 0),
        )?;
        let slot = input(&mut encoder, 17)?;
        encoder.submit(slot, None, true, Instant::now())?;
        assert!(encoder.poll()?[0].idr);
        let submitted = encoder.submitted_metadata;
        encoder.set_hdr_metadata(butterpollo_media_policy::hdr::Metadata {
            maximum_nits: 1400,
            max_cll: 1300,
            max_fall: 800,
            ..Default::default()
        });
        state(|d| d.reject_submit = true);
        assert!(encoder.submit(slot, None, false, Instant::now()).is_err());
        assert_eq!(encoder.submitted_metadata, submitted);
        assert!(encoder.force_idr);
        // Release the rejected registration before retrying; no frame was accepted.
        encoder.clear_inputs()?;
        state(|d| {
            d.reject_submit = false;
            d.need_more_input = true;
        });
        let slot = input(&mut encoder, 18)?;
        encoder.submit(slot, None, true, Instant::now())?;
        assert_eq!(encoder.submitted_metadata, Some(encoder.metadata.encoded()));
        assert!(!encoder.force_idr);
        assert!(encoder.poll()?[0].idr);
        encoder.submit(slot, None, false, Instant::now())?;
        assert!(!encoder.poll()?[0].idr);
        // An external IDR also consumes an internal bitrate/recovery request,
        // even when there is no metadata change to trigger it.
        encoder.force_idr = true;
        encoder.submit(slot, None, true, Instant::now())?;
        assert!(encoder.poll()?[0].idr);
        assert!(!encoder.force_idr);
        encoder.submit(slot, None, false, Instant::now())?;
        assert!(!encoder.poll()?[0].idr);
        drop(encoder);
        assert_clean();
    }
    Ok(())
}

pub(super) fn check_decoded_hdr_metadata(
    frame: &serde_json::Value,
    expected: butterpollo_media_policy::hdr::Metadata,
    codec: u8,
) -> Result<()> {
    let side_data = frame["side_data_list"]
        .as_array()
        .context("decoded frame has no HDR metadata")?;
    let mastering = side_data
        .iter()
        .find(|data| data["side_data_type"] == "Mastering display metadata")
        .context("decoded frame has no mastering metadata")?;
    let light = side_data
        .iter()
        .find(|data| data["side_data_type"] == "Content light level metadata")
        .context("decoded frame has no content light metadata")?;
    let check = |key: &str, expected: f64, unit: f64| -> Result<()> {
        let (num, den) = mastering[key]
            .as_str()
            .and_then(|s| s.split_once('/'))
            .with_context(|| format!("missing decoded {key}"))?;
        let actual = num.parse::<f64>()? / den.parse::<f64>()?;
        if !actual.is_finite() || (actual - expected).abs() > unit / 2. + 1e-9 {
            bail!("decoded {key}: expected {expected}, got {actual}");
        }
        Ok(())
    };
    let chroma_unit = if codec == 2 { 1. / 65536. } else { 1. / 50000. };
    for (keys, point) in [
        (["red_x", "red_y"], expected.primaries[0]),
        (["green_x", "green_y"], expected.primaries[1]),
        (["blue_x", "blue_y"], expected.primaries[2]),
        (["white_point_x", "white_point_y"], expected.white),
    ] {
        for (key, value) in keys.into_iter().zip(point) {
            check(key, f64::from(value) / 50000., chroma_unit)?;
        }
    }
    check(
        "max_luminance",
        f64::from(expected.maximum_nits),
        if codec == 2 { 1. / 256. } else { 1. / 10000. },
    )?;
    check(
        "min_luminance",
        f64::from(expected.minimum) / 10000.,
        if codec == 2 { 1. / 16384. } else { 1. / 10000. },
    )?;
    if light["max_content"].as_u64() != Some(u64::from(expected.max_cll))
        || light["max_average"].as_u64() != Some(u64::from(expected.max_fall))
    {
        bail!("decoded content light levels do not match the submitted metadata");
    }
    Ok(())
}

#[test]
fn decoded_hdr_check_rejects_wrong_av1_units_and_missing_light_metadata() -> Result<()> {
    let expected = butterpollo_media_policy::hdr::Metadata {
        minimum: 20,
        max_cll: 900,
        max_fall: 400,
        ..Default::default()
    };
    let frame = serde_json::json!({"side_data_list": [
        {"side_data_type": "Mastering display metadata",
         "red_x": "46399/65536", "red_y": "19137/65536",
         "green_x": "11141/65536", "green_y": "52232/65536",
         "blue_x": "8585/65536", "blue_y": "3015/65536",
         "white_point_x": "20493/65536", "white_point_y": "21561/65536",
         "max_luminance": "256000/256", "min_luminance": "33/16384"},
        {"side_data_type": "Content light level metadata", "max_content": 900, "max_average": 400}
    ]});
    check_decoded_hdr_metadata(&frame, expected, 2)?;
    let mut wrong_units = frame.clone();
    wrong_units["side_data_list"][0]["max_luminance"] = "10000000/256".into();
    assert!(check_decoded_hdr_metadata(&wrong_units, expected, 2).is_err());
    let mut missing_light = frame;
    missing_light["side_data_list"]
        .as_array_mut()
        .unwrap()
        .pop();
    assert!(check_decoded_hdr_metadata(&missing_light, expected, 2).is_err());
    Ok(())
}
