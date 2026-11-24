//! AMF runtime rate-control scaling, ceiling clamping and property readback tests.

use super::super::*;

#[test]
fn rate_limits_scale_by_the_requested_over_the_last_applied_bitrate() {
    // The arithmetic set_bitrate uses after 80 applied, 40 rejected and 20
    // accepted: the divisor is the applied 80, so the limits are quartered.
    for (current, expected) in [
        (96_000_000, 24_000_000),
        (666_667, 166_666),
        (2_666_668, 666_667),
    ] {
        assert_eq!(scale_rate_control(current, 80_000, 20_000), expected);
        assert_eq!(scale_rate_control(current, 80_000, 80_000), current);
    }
    assert_eq!(scale_rate_control(0, 80_000, 20_000), 0);
    assert_eq!(scale_rate_control(i64::MAX, 1, u32::MAX), i64::MAX);
    // Clamp only the final value, not the multiplication before division.
    assert_eq!(scale_rate_control(i64::MAX, 80_000, 80_000), i64::MAX);
    assert_eq!(scale_rate_control(i64::MAX, 80_000, 40_000), i64::MAX / 2);
}

struct FakeRateProperties {
    values: std::collections::BTreeMap<String, rate_control::Limit>,
    rejected: std::collections::BTreeSet<String>,
    unreadable: std::collections::BTreeSet<String>,
    writes: Vec<(String, i64)>,
    reads: usize,
}

impl FakeRateProperties {
    fn new(codec: u8) -> Self {
        let prefix = match codec {
            0 => "",
            1 => "Hevc",
            _ => "Av1",
        };
        let values = [
            ("PeakBitrate", 96_000_000),
            ("VBVBufferSize", 2_666_668),
            (
                if codec == 2 {
                    "MaxCompressedFrameSize"
                } else {
                    "MaxAUSize"
                },
                666_668,
            ),
        ]
        .into_iter()
        .map(|(suffix, value)| {
            (
                format!("{prefix}{suffix}"),
                rate_control::Limit {
                    value,
                    minimum: None,
                    maximum: None,
                },
            )
        })
        .collect();
        Self {
            values,
            rejected: Default::default(),
            unreadable: Default::default(),
            writes: vec![],
            reads: 0,
        }
    }

    fn apply(
        &mut self,
        state: &mut rate_control::State,
        codec: u8,
        bitrate: u32,
        warnings: &dyn butterpollo_media_policy::WarningSink,
    ) {
        let driver = std::cell::RefCell::new(self);
        state.update(
            codec,
            bitrate,
            warnings,
            |name| {
                let mut driver = driver.borrow_mut();
                driver.reads += 1;
                if driver.unreadable.contains(name) {
                    None
                } else {
                    driver.values.get(name).copied()
                }
            },
            |name, value| {
                let mut driver = driver.borrow_mut();
                driver.writes.push((name.to_string(), value));
                if driver.rejected.contains(name) {
                    bail!("mock rejected {name}");
                }
                if let Some(limit) = driver.values.get_mut(name) {
                    limit.value = value;
                }
                Ok(())
            },
        );
    }
}

#[test]
fn frame_size_rejection_or_readback_mismatch_warns_without_blocking_streams() -> Result<()> {
    for codec in 0..=2 {
        let stream = butterpollo_media_policy::rtsp::Negotiated {
            codec,
            ..Default::default()
        };
        for text in ["", "amd_max_frame_size=2", "amd_max_frame_size=1"] {
            let config = butterpollo_config::config::Config::parse(text)?;
            let properties =
                butterpollo_media_policy::encoder_policy::amf_rate_control(&config, &stream)?;
            for failure in [
                "mock AMF_NOT_SUPPORTED",
                "mock readback differs from requested size",
            ] {
                let warnings = butterpollo_media_policy::RecordedWarnings::default();
                let mut attempted = Vec::new();
                rate_control::configure(&properties, &warnings, |property| {
                    attempted.push(property.name.clone());
                    bail!("{failure}")
                })?;
                assert_eq!(attempted, [properties[0].name.clone()]);
                let messages = warnings.snapshot();
                assert_eq!(messages.len(), 1);
                assert_eq!(messages[0].code, "encoder_frame_size");
                assert!(messages[0].message.contains(failure));
                assert!(messages[0].message.contains(&properties[0].name));
                assert!(messages[0].message.contains("streaming continues"));
            }
        }
        for key in ["amd_peak_bitrate_ratio", "amd_vbv_buffer_frames"] {
            let config = butterpollo_config::config::Config::parse(&format!("{key}=1"))?;
            let properties =
                butterpollo_media_policy::encoder_policy::amf_rate_control(&config, &stream)?;
            let warnings = butterpollo_media_policy::RecordedWarnings::default();
            assert!(
                rate_control::configure(&properties, &warnings, |_| {
                    bail!("mock AMF_NOT_SUPPORTED")
                })
                .is_err()
            );
            assert!(
                warnings.snapshot().is_empty(),
                "explicit peak/VBV stay strict"
            );
        }
    }
    Ok(())
}

#[test]
fn rate_control_keeps_each_failed_property_on_its_own_applied_bitrate() {
    for codec in 0..=2 {
        let initial = FakeRateProperties::new(codec).values;
        for failed in initial.keys() {
            let mut driver = FakeRateProperties::new(codec);
            let mut state = rate_control::State::new(80_000, true);
            let warnings = butterpollo_media_policy::RecordedWarnings::default();
            driver.rejected.insert(failed.clone());
            driver.apply(&mut state, codec, 40_000, &warnings);
            assert_eq!(driver.values[failed].value, initial[failed].value);
            assert!(warnings.snapshot().iter().any(
                |warning| warning.code == "encoder_buffer" && warning.message.contains(failed)
            ));
            driver.rejected.clear();
            driver.apply(&mut state, codec, 20_000, &warnings);
            for (name, original) in &initial {
                assert_eq!(
                    driver.values[name].value,
                    original.value / 4,
                    "{name}, previously rejected {failed}"
                );
            }
            assert!(warnings.snapshot().is_empty());
        }
    }
}

#[test]
fn only_explicit_frame_caps_scale_with_bitrate_and_driver_default_stays_disabled() -> Result<()> {
    for codec in 0..=2 {
        for text in [
            "",
            "amd_max_frame_size=auto",
            "amd_max_frame_size=2",
            "amd_max_frame_size=0",
        ] {
            let stream = butterpollo_media_policy::rtsp::Negotiated {
                codec,
                bitrate_kbps: 80_000,
                fps: 120,
                rate_millihz: 120_000,
                ..Default::default()
            };
            let config = butterpollo_config::config::Config::parse(text)?;
            let properties =
                butterpollo_media_policy::encoder_policy::amf_rate_control(&config, &stream)?;
            let mut driver = FakeRateProperties::new(codec);
            let cap = match codec {
                0 => "MaxAUSize",
                1 => "HevcMaxAUSize",
                _ => "Av1MaxCompressedFrameSize",
            };
            driver.values.get_mut(cap).unwrap().value = 0;
            let warnings = butterpollo_media_policy::RecordedWarnings::default();
            rate_control::configure(&properties, &warnings, |property| {
                let butterpollo_media_policy::encoder_policy::Value::Integer(value) =
                    property.value
                else {
                    panic!("rate limits are integer bit budgets")
                };
                driver.values.get_mut(&property.name).unwrap().value = value;
                Ok(())
            })?;
            let disabled = text.ends_with("=0");
            assert_eq!(
                driver.values[cap].value,
                if disabled { 0 } else { 1_333_333 }
            );
            let automatic =
                butterpollo_media_policy::encoder_policy::amf_frame_cap_automatic(&config);
            let mut state = rate_control::State::new(stream.bitrate_kbps, !automatic && !disabled);
            driver.apply(&mut state, codec, 40_000, &warnings);
            assert_eq!(
                driver.values[cap].value,
                if disabled {
                    0
                } else if automatic {
                    1_333_333
                } else {
                    666_666
                }
            );
            assert_eq!(
                driver.writes.iter().any(|(name, _)| name == cap),
                !disabled && !automatic
            );
            if disabled {
                // Driver default is an opt-out even if AMF reports a nonzero
                // default limit after its own rate-control reconfiguration.
                driver.values.get_mut(cap).unwrap().value = 1_777_777;
                driver.apply(&mut state, codec, 20_000, &warnings);
                assert_eq!(driver.values[cap].value, 1_777_777);
                assert!(!driver.writes.iter().any(|(name, _)| name == cap));
            }
            assert!(warnings.snapshot().is_empty());
        }
    }
    Ok(())
}

#[test]
fn rate_control_retains_failed_basis_and_warning_across_read_failures_and_clamps() {
    let mut driver = FakeRateProperties::new(1);
    let mut state = rate_control::State::new(80_000, true);
    let warnings = butterpollo_media_policy::RecordedWarnings::default();
    let peak = "HevcPeakBitrate";
    driver.rejected.insert(peak.into());
    driver.apply(&mut state, 1, 40_000, &warnings);
    driver.rejected.clear();
    driver.unreadable.insert(peak.into());
    driver.apply(&mut state, 1, 20_000, &warnings);
    assert_eq!(driver.values[peak].value, 96_000_000);
    assert!(
        warnings
            .snapshot()
            .iter()
            .any(|warning| warning.code == "encoder_buffer"
                && warning.message.contains("cannot be verified"))
    );
    driver.unreadable.clear();
    let limit = driver.values.get_mut(peak).unwrap();
    limit.minimum = Some(22_000_000);
    limit.maximum = Some(30_000_000);
    driver.apply(&mut state, 1, 10_000, &warnings);
    // 96 * 10 / 80 = 12, clamped to 22. Using the unrelated target20
    // as denominator would produce 48, incorrectly clamped to 30.
    assert_eq!(driver.values[peak].value, 22_000_000);
    assert!(warnings.snapshot().is_empty());
    driver.apply(&mut state, 1, 40_000, &warnings);
    assert_eq!(driver.values[peak].value, 30_000_000);
    let limit = driver.values.get_mut(peak).unwrap();
    limit.minimum = None;
    limit.maximum = None;
    driver.apply(&mut state, 1, 20_000, &warnings);
    // Both clamped writes succeeded: the basis is now target40, not target80.
    assert_eq!(driver.values[peak].value, 15_000_000);
}

#[test]
fn rate_control_target_rejection_preserves_limits_and_existing_buffer_warning() {
    let mut driver = FakeRateProperties::new(2);
    let mut state = rate_control::State::new(80_000, true);
    let warnings = butterpollo_media_policy::RecordedWarnings::default();
    driver.rejected.insert("Av1PeakBitrate".into());
    driver.apply(&mut state, 2, 40_000, &warnings);
    let reads = driver.reads;
    driver.rejected.insert("Av1TargetBitrate".into());
    driver.apply(&mut state, 2, 20_000, &warnings);
    assert_eq!(
        driver.reads, reads,
        "no limit updates after a target rejection"
    );
    assert!(
        warnings
            .snapshot()
            .iter()
            .any(|warning| warning.code == "encoder_bitrate")
    );
    assert!(
        warnings
            .snapshot()
            .iter()
            .any(|warning| warning.code == "encoder_buffer")
    );
    let writes = driver.writes.len();
    driver.apply(&mut state, 2, 20_000, &warnings);
    assert_eq!(
        driver.writes.len(),
        writes,
        "do not retry a rejected target every frame"
    );
    driver.rejected.clear();
    driver.apply(&mut state, 2, 10_000, &warnings);
    assert_eq!(driver.values["Av1PeakBitrate"].value, 12_000_000);
    assert_eq!(driver.values["Av1VBVBufferSize"].value, 333_333);
    assert_eq!(driver.values["Av1MaxCompressedFrameSize"].value, 83_333);
    assert!(warnings.snapshot().is_empty());
}

#[test]
fn bitrate_ceiling_takes_the_lower_driver_report_and_ignores_nonpositive_ones() {
    // The range and the MaxBitrate cap: an RX 7900 XT refuses above the cap.
    assert_eq!(
        rate_control::ceiling(Some(i64::MAX), Some(1_000_000_000)),
        Some(1_000_000_000)
    );
    assert_eq!(
        rate_control::ceiling(Some(600_000_000), Some(1_000_000_000)),
        Some(600_000_000)
    );
    assert_eq!(
        rate_control::ceiling(None, Some(600_000_000)),
        Some(600_000_000)
    );
    assert_eq!(
        rate_control::ceiling(Some(600_000_000), None),
        Some(600_000_000)
    );
    assert_eq!(rate_control::ceiling(Some(0), Some(-1)), None);
    assert_eq!(rate_control::ceiling(None, None), None);
    // #80: a client's 1.2 Gbps AV1 request against a driver ceiling.
    assert_eq!(rate_control::held(959_000, Some(600_000_000)), 600_000);
    assert_eq!(rate_control::held(300_000, Some(600_000_000)), 300_000);
    assert_eq!(rate_control::held(959_000, None), 959_000);
    assert_eq!(rate_control::held(959_000, Some(999)), 1);
}

/// An explicit peak ratio over a target held at its ceiling would ask the
/// driver for a peak above it (AMF_OUT_OF_RANGE on Av1PeakBitrate, measured
/// on an RX 7900 XT): start-up peak and VBV limits are held to their ranges,
/// and the peak to the bitrate ceiling where the driver gives no range.
#[test]
fn start_up_peak_and_vbv_limits_are_held_to_their_property_ranges() {
    use butterpollo_media_policy::encoder_policy::{Property, Value};
    let property = |name: &str, value| Property {
        name: name.into(),
        value: Value::Integer(value),
        required: true,
    };
    let mut limits = vec![
        property("Av1PeakBitrate", 2_000_000_000),
        property("Av1VBVBufferSize", 40_000_000),
        property("Av1MaxCompressedFrameSize", 9_000_000_000),
    ];
    let held = rate_control::hold_to_ranges(&mut limits, Some(1_200_000_000), |name| match name {
        "Av1PeakBitrate" => Some(1_000_000_000),
        "Av1VBVBufferSize" => Some(0),
        _ => Some(1),
    });
    assert_eq!(
        held,
        [("Av1PeakBitrate".to_string(), 2_000_000_000, 1_000_000_000)]
    );
    assert!(matches!(limits[0].value, Value::Integer(1_000_000_000)));
    assert!(
        matches!(limits[1].value, Value::Integer(40_000_000)),
        "a range of 0 says nothing"
    );
    assert!(
        matches!(limits[2].value, Value::Integer(9_000_000_000)),
        "frame caps are not peak or VBV limits"
    );
    // No range reported (the RX 7900 XT): the bitrate ceiling holds the peak.
    let mut unranged = vec![property("Av1PeakBitrate", 2_000_000_000)];
    let held = rate_control::hold_to_ranges(&mut unranged, Some(1_000_000_000), |_| None);
    assert_eq!(
        held,
        [("Av1PeakBitrate".to_string(), 2_000_000_000, 1_000_000_000)]
    );
    let mut under = vec![property("Av1PeakBitrate", 600_000_000)];
    assert!(rate_control::hold_to_ranges(&mut under, Some(1_000_000_000), |_| None).is_empty());
}

#[test]
#[ignore = "requires AMD AMF"]
fn a_bitrate_above_the_driver_ceiling_starts_held_to_it_on_every_codec() -> Result<()> {
    let _com = crate::capture::ComGuard::new()?;
    let gpu = Device::new("")?;
    let options = butterpollo_config::config::Config::default();
    let requested = 1_900_000u32;
    for (codec, width, height, target) in [
        (0u8, 1920u32, 1080u32, "TargetBitrate"),
        (1, 3840, 2160, "HevcTargetBitrate"),
        (2, 3840, 2160, "Av1TargetBitrate"),
    ] {
        let config = butterpollo_media_policy::rtsp::Negotiated {
            width,
            height,
            codec,
            fps: 100,
            bitrate_kbps: requested,
            ..Default::default()
        };
        let encoder = Encoder::new_gpu(&config, gpu.clone(), &options, None)
            .with_context(|| format!("codec {codec} at {requested} Kbps"))?;
        let ceiling = encoder.bitrate_ceiling();
        let applied = encoder.read(target);
        println!(
            "codec={codec} ceiling_bits={ceiling:?} cap_bits={:?} {target}={applied:?}",
            encoder.bitrate_cap()
        );
        // The ceiling is the lower of the target's range and the cap, and the
        // target sits at the lower of it and the request.
        println!(
            "codec={codec} range_bits={:?}",
            encoder.range_maximum(target)
        );
        assert_eq!(
            ceiling,
            rate_control::ceiling(encoder.range_maximum(target), encoder.bitrate_cap()),
            "codec {codec}"
        );
        if let (Some(ceiling), Some(applied)) = (ceiling, applied) {
            assert_eq!(
                applied,
                ceiling.min(i64::from(requested) * 1000) / 1000 * 1000,
                "codec {codec}"
            );
        }
        let held = ceiling.is_some_and(|bits| bits < i64::from(requested) * 1000);
        assert_eq!(
            encoder
                .warnings
                .snapshot()
                .iter()
                .any(|warning| warning.code == "encoder_bitrate_ceiling"),
            held,
            "codec {codec}"
        );
    }
    // An explicit peak ratio over the held target starts too: its peak is held
    // to Av1PeakBitrate's range instead of being refused.
    let options = butterpollo_config::config::Config::parse("amd_peak_bitrate_ratio=2")?;
    let config = butterpollo_media_policy::rtsp::Negotiated {
        width: 3840,
        height: 2160,
        codec: 2,
        fps: 100,
        bitrate_kbps: requested,
        ..Default::default()
    };
    let encoder = Encoder::new_gpu(&config, gpu.clone(), &options, None)
        .context("AV1 with amd_peak_bitrate_ratio=2 above the ceiling")?;
    let peak = encoder.read("Av1PeakBitrate");
    println!(
        "AV1 peak ratio 2: Av1PeakBitrate={peak:?} range={:?}",
        encoder.range_maximum("Av1PeakBitrate")
    );
    let limit = rate_control::ceiling(
        encoder.range_maximum("Av1PeakBitrate"),
        encoder.bitrate_ceiling(),
    );
    assert!(
        peak.is_some() && limit.is_some(),
        "peak {peak:?}, limit {limit:?}"
    );
    assert!(peak <= limit, "peak {peak:?} above {limit:?}");
    Ok(())
}

#[test]
fn rate_control_holds_runtime_targets_to_the_driver_ceiling() {
    // #80: AMF refuses Av1TargetBitrate above its range with
    // AMF_OUT_OF_RANGE, so a request above the ceiling runs at the ceiling.
    let mut driver = FakeRateProperties::new(2);
    let warnings = butterpollo_media_policy::RecordedWarnings::default();
    let mut state = rate_control::State::new(1_000_000, true).with_ceiling(Some(800_000_000));
    // The stream started held at 800 Mbps: the same request writes nothing.
    driver.apply(&mut state, 2, 1_000_000, &warnings);
    assert!(driver.writes.is_empty());
    driver.apply(&mut state, 2, 400_000, &warnings);
    assert_eq!(driver.values["Av1PeakBitrate"].value, 48_000_000);
    assert!(warnings.snapshot().is_empty(), "under the ceiling");
    driver.writes.clear();
    driver.apply(&mut state, 2, 1_200_000, &warnings);
    assert_eq!(
        driver.writes.first(),
        Some(&("Av1TargetBitrate".to_string(), 800_000_000))
    );
    assert_eq!(driver.values["Av1PeakBitrate"].value, 96_000_000);
    // Another request above the ceiling changes nothing at the driver.
    driver.writes.clear();
    driver.apply(&mut state, 2, 1_500_000, &warnings);
    assert!(driver.writes.is_empty());
    let snapshot = warnings.snapshot();
    assert_eq!(snapshot.len(), 1, "{snapshot:?}");
    assert_eq!(snapshot[0].code, "encoder_bitrate_ceiling");
    assert!(
        snapshot[0]
            .message
            .starts_with("The encoder runs at 800000 Kbps, not the requested 1500000 Kbps"),
        "{}",
        snapshot[0].message
    );
    // A peak at twice the target is held to the ceiling, which it may not pass.
    driver.values.get_mut("Av1PeakBitrate").unwrap().value = 1_600_000_000;
    driver.apply(&mut state, 2, 700_000, &warnings);
    assert_eq!(driver.values["Av1PeakBitrate"].value, 800_000_000);
}

/// An explicit peak ratio held to the ceiling at start comes back once the
/// target drops below it: the hold is never the basis of the next peak.
#[test]
fn a_peak_held_to_the_ceiling_scales_from_its_asked_value() {
    let mut driver = FakeRateProperties::new(2);
    let warnings = butterpollo_media_policy::RecordedWarnings::default();
    // amd_peak_bitrate_ratio=2 at 1.9 Gbps: target and peak start at 1 Gbps.
    let mut state = rate_control::State::new(1_900_000, true).with_ceiling(Some(1_000_000_000));
    state.held_at_start(&[("Av1PeakBitrate".to_string(), 2_000_000_000, 1_000_000_000)]);
    driver.values.get_mut("Av1PeakBitrate").unwrap().value = 1_000_000_000;
    driver.apply(&mut state, 2, 400_000, &warnings);
    assert_eq!(driver.values["Av1PeakBitrate"].value, 800_000_000);
    // Up past the ceiling the peak is held again, and back down it is twice
    // the target again.
    driver.apply(&mut state, 2, 1_900_000, &warnings);
    assert_eq!(driver.values["Av1PeakBitrate"].value, 1_000_000_000);
    driver.apply(&mut state, 2, 400_000, &warnings);
    assert_eq!(driver.values["Av1PeakBitrate"].value, 800_000_000);
}
