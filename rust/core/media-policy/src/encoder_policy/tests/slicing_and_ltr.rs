//! AMF long-term reference, intra-refresh, and slice/tile delivery policy tests.

use super::*;

#[test]
fn amf_ltr_anchors_share_the_negotiated_reference_budget() {
    let config = Config::parse("amd_ltr_frames=4\n").unwrap();
    for codec in 0..=2 {
        for (references, expected_ltr) in [(1, 0), (2, 1), (3, 2), (4, 3), (5, 4), (16, 4)] {
            let stream = Negotiated {
                codec,
                references,
                ..Default::default()
            };
            let count = amf_ltr_frames(&config, &stream);
            assert_eq!(count, expected_ltr);
            assert!(count < references as usize);
            let properties = amf(&config, &stream).unwrap();
            assert!(properties.iter().any(|property| {
                property.name.ends_with("MaxNumRefFrames")
                    && property.value == Value::Integer(i64::from(references))
                    && property.required
            }));
        }
    }
}

#[test]
fn amf_keeps_long_term_references_on_every_codec_for_every_client() {
    let one = Config::parse("amd_ltr_frames=1\n").unwrap();
    let off = Config::parse("amd_ltr_frames=0\n").unwrap();
    for codec in [0, 1, 2] {
        // A stock client, and Nightjar's launch parameter, which no
        // longer changes anything: the same setting and budget.
        for ltr_recovery in [false, true] {
            let stream = Negotiated {
                codec,
                ltr_recovery,
                ..Default::default()
            };
            assert_eq!(amf_ltr_requested(&Config::default(), &stream), 4);
            assert_eq!(amf_ltr_frames(&Config::default(), &stream), 4);
            assert_eq!(amf_ltr_requested(&one, &stream), 1);
            assert_eq!(amf_ltr_requested(&off, &stream), 0);
            assert_eq!(amf_ltr_frames(&off, &stream), 0);
            let budget = Negotiated {
                references: 3,
                ..stream.clone()
            };
            assert_eq!(amf_ltr_frames(&Config::default(), &budget), 2);
        }
    }
    // PyroWave keeps no long-term references.
    let pyrowave = Negotiated {
        codec: 3,
        ltr_recovery: true,
        ..Default::default()
    };
    assert_eq!(amf_ltr_requested(&Config::default(), &pyrowave), 0);
}

#[test]
fn amf_offers_invalidation_with_four_anchors_unless_turned_off() {
    let unset = Config::default();
    assert!(amf_offers_invalidation(&unset));
    let av1 = Negotiated {
        codec: 2,
        ..Default::default()
    };
    assert_eq!(amf_ltr_requested(&unset, &av1), 4);
    assert_eq!(amf_ltr_frames(&unset, &av1), 4);
    let off = Config::parse("amd_ltr_frames=0\n").unwrap();
    assert!(!amf_offers_invalidation(&off));
    assert_eq!(amf_ltr_requested(&off, &av1), 0);
}

#[test]
fn one_reference_avc_intra_refresh_falls_back_without_expanding_the_budget() {
    let config = Config::parse("amd_ltr_frames=4\n").unwrap();
    for references in [0, 1, 2, 4] {
        let stream = Negotiated {
            references,
            intra_refresh: true,
            ..Default::default()
        };
        let enabled = references != 1;
        assert_eq!(amf_intra_refresh(&stream), enabled);
        let properties = amf(&config, &stream).unwrap();
        assert!(properties.iter().any(|property| {
            property.name == "MaxNumRefFrames"
                && property.value
                    == Value::Integer(if references == 0 {
                        2
                    } else {
                        i64::from(references)
                    })
        }));
        assert_eq!(
            properties
                .iter()
                .any(|property| { property.name == "IntraRefreshMBsNumberPerSlot" }),
            enabled
        );
        assert_eq!(amf_ltr_frames(&config, &stream), 0);
    }
}

#[test]
fn hevc_intra_refresh_uses_actual_ctb_rows_and_preserves_decoder_budget() {
    for (width, height, ctbs, fallback) in [
        (1280, 720, 20, 1),
        (1920, 1080, 30, 2),
        (2560, 1440, 40, 4),
        (3840, 2160, 60, 7),
        (1921, 1081, 31, 2),
        (1, 1, 1, 1),
        (64, 7680, 1, 1),
        (64, 7681, 2, 1),
    ] {
        for hdr in [false, true] {
            let stream = Negotiated {
                width,
                height,
                codec: 1,
                intra_refresh: true,
                references: 1,
                hdr,
                ..Default::default()
            };
            assert_eq!(
                amf_hevc_intra_refresh(&stream),
                Some(AmfHevcIntraRefresh {
                    gop_frames: 120,
                    ctbs_per_slot: ctbs,
                    fallback_ctbs_per_slot: fallback,
                })
            );
            let properties = amf(&Config::default(), &stream).unwrap();
            assert!(properties.iter().any(|p| {
                p.name == "HevcIntraRefreshCTBsNumberPerSlot" && p.value == Value::Integer(fallback)
            }));
            assert!(
                properties
                    .iter()
                    .any(|p| { p.name == "HevcMaxNumRefFrames" && p.value == Value::Integer(1) })
            );
            assert!(!properties.iter().any(|p| p.name == "HevcGOPSPerIDR"));
            assert_eq!(amf_ltr_frames(&Config::default(), &stream), 0);
        }
    }
}

#[test]
fn hevc_intra_refresh_override_requires_a_valid_negotiated_hevc_stream() {
    for codec in 0..=3 {
        for requested in [false, true] {
            let stream = Negotiated {
                codec,
                intra_refresh: requested,
                ..Default::default()
            };
            assert_eq!(
                amf_hevc_intra_refresh(&stream).is_some(),
                codec == 1 && requested
            );
        }
    }
    for (width, height) in [(0, 2160), (3840, 0), (0, 0)] {
        assert!(
            amf_hevc_intra_refresh(&Negotiated {
                width,
                height,
                codec: 1,
                intra_refresh: true,
                ..Default::default()
            })
            .is_none()
        );
    }
    let stream = Negotiated {
        width: 3840,
        height: 2160,
        intra_refresh: true,
        ..Default::default()
    };
    let properties = amf(&Config::default(), &stream).unwrap();
    assert!(
        properties.iter().any(|p| {
            p.name == "IntraRefreshMBsNumberPerSlot" && p.value == Value::Integer(109)
        })
    );
    assert!(!properties.iter().any(|p| p.name.starts_with("Hevc")));
}

#[test]
fn av1_that_asked_for_intra_refresh_is_configured_as_a_stream_that_never_asked() {
    for references in [0, 1, 2, 5] {
        let asked = Negotiated {
            codec: 2,
            width: 2560,
            height: 1440,
            references,
            intra_refresh: true,
            ..Default::default()
        };
        let never_asked = Negotiated {
            intra_refresh: false,
            ..asked.clone()
        };
        assert!(!amf_intra_refresh(&asked));
        assert!(amf_hevc_intra_refresh(&asked).is_none());
        let config = Config::default();
        let properties = |stream| {
            amf(&config, stream)
                .unwrap()
                .into_iter()
                .map(|p| (p.name, p.value, p.required))
                .collect::<Vec<_>>()
        };
        assert!(
            !properties(&asked)
                .iter()
                .any(|p| p.0.contains("IntraRefresh"))
        );
        assert_eq!(properties(&asked), properties(&never_asked));
        // Loss recovery keeps its long-term references for invalidation.
        assert_eq!(
            amf_ltr_frames(&config, &asked),
            amf_ltr_frames(&config, &never_asked)
        );
        assert_eq!(
            amf_ltr_frames(&config, &asked) > 0,
            references != 1,
            "{references}"
        );
    }
}

#[test]
fn slice_delivery_follows_the_setting_on_avc_or_hevc() {
    let set = |value: &str| Config::parse(&format!("video_slice_delivery={value}\n")).unwrap();
    let stream = |codec, width, height| Negotiated {
        codec,
        width,
        height,
        ..Default::default()
    };
    for (value, small, large) in [
        ("off", 0, 0),
        ("0", 0, 0),
        ("2", 2, 2),
        ("4", 4, 4),
        ("auto", 0, 4),
        // Unknown values take the default.
        ("3", 0, 4),
        ("on", 0, 4),
    ] {
        for codec in [0, 1] {
            assert_eq!(
                slice_delivery(&set(value), &stream(codec, 1920, 1080)),
                small,
                "{value}"
            );
            assert_eq!(
                slice_delivery(&set(value), &stream(codec, 3840, 2160)),
                large,
                "{value}"
            );
        }
        assert_eq!(
            slice_delivery(&set(value), &stream(2, 3840, 2160)),
            0,
            "AV1 is never sliced"
        );
    }
}

#[test]
fn auto_slice_delivery_is_on_for_4k_class_pictures_only() {
    let blocks = |width, height| {
        let stream = Negotiated {
            codec: 1,
            width,
            height,
            ..Default::default()
        };
        slice_delivery(&Config::default(), &stream)
    };
    // The 1080p and 1440p encodes leave too little slice lead to gain from.
    for (width, height) in [
        (1920, 1080),
        (2560, 1440),
        (2880, 1800),
        (3440, 1440),
        (0, 0),
    ] {
        assert_eq!(blocks(width, height), 0, "{width}x{height}");
    }
    for (width, height) in [
        (3840, 2160),
        (4096, 2160),
        (3840, 2400),
        (5120, 1440),
        (7680, 4320),
    ] {
        assert_eq!(blocks(width, height), 4, "{width}x{height}");
    }
}

#[test]
fn auto_slice_delivery_keeps_the_intra_refresh_the_stream_would_use() {
    let stream = |codec, references| Negotiated {
        codec,
        width: 3840,
        height: 2160,
        intra_refresh: true,
        references,
        ..Default::default()
    };
    let auto = Config::default();
    assert_eq!(
        slice_delivery(&auto, &stream(1, 2)),
        0,
        "HEVC refresh stays"
    );
    assert_eq!(
        slice_delivery(&auto, &stream(0, 2)),
        0,
        "H.264 refresh stays"
    );
    // H.264 with one reference recovers with keyframes anyway.
    assert_eq!(slice_delivery(&auto, &stream(0, 1)), 4);
    // An explicit setting still wins over refresh.
    let four = Config::parse("video_slice_delivery=4\n").unwrap();
    assert_eq!(slice_delivery(&four, &stream(1, 2)), 4);
}

#[test]
fn slice_delivery_asks_for_slice_output_with_at_least_one_slice_per_block() {
    let find = |properties: &[Property], name: &str| {
        properties
            .iter()
            .find(|p| p.name == name)
            .map(|p| (p.value, p.required))
    };
    let on = Config::parse("video_slice_delivery=4\n").unwrap();
    for (codec, prefix) in [(0, ""), (1, "Hevc")] {
        let slices = format!("{prefix}SlicesPerFrame");
        let mode = format!("{prefix}OutputMode");
        for (client, expected) in [(1, 4), (2, 4), (4, 4), (8, 8)] {
            let stream = Negotiated {
                codec,
                slices: client,
                ..Default::default()
            };
            let properties = amf(&on, &stream).unwrap();
            assert_eq!(find(&properties, &mode), Some((Value::Integer(1), true)));
            assert_eq!(
                find(&properties, &slices),
                Some((Value::Integer(expected), true))
            );
            let off = amf(&Config::default(), &stream).unwrap();
            assert_eq!(find(&off, &mode), None, "frame output is the default");
        }
    }
    let av1 = Negotiated {
        codec: 2,
        slices: 4,
        ..Default::default()
    };
    let properties = amf(&on, &av1).unwrap();
    assert!(properties.iter().all(|p| !p.name.contains("OutputMode")));
}

#[test]
fn slice_delivery_turns_intra_refresh_off() {
    let on = Config::parse("video_slice_delivery=2\n").unwrap();
    for codec in [0, 1] {
        let stream = Negotiated {
            codec,
            intra_refresh: true,
            references: 2,
            ..Default::default()
        };
        let refresh =
            |properties: &[Property]| properties.iter().any(|p| p.name.contains("IntraRefresh"));
        assert!(refresh(&amf(&Config::default(), &stream).unwrap()));
        assert!(!refresh(&amf(&on, &stream).unwrap()));
    }
}

#[test]
fn client_slice_requests_are_best_effort_but_configured_tiles_are_required() {
    let find = |properties: &[Property], name: &str| {
        properties
            .iter()
            .find(|p| p.name == name)
            .map(|p| (p.value, p.required))
    };
    for (codec, name) in [(0, "SlicesPerFrame"), (1, "HevcSlicesPerFrame")] {
        let stream = Negotiated {
            codec,
            slices: 4,
            ..Default::default()
        };
        let properties = amf(&Config::default(), &stream).unwrap();
        assert_eq!(find(&properties, name), Some((Value::Integer(4), false)));
        let single = Negotiated {
            codec,
            slices: 1,
            ..Default::default()
        };
        let properties = amf(&Config::default(), &single).unwrap();
        assert_eq!(find(&properties, name), None);
    }
    let av1 = Negotiated {
        codec: 2,
        slices: 4,
        ..Default::default()
    };
    let properties = amf(&Config::default(), &av1).unwrap();
    assert_eq!(
        find(&properties, "Av1NumTilesPerFrame"),
        Some((Value::Integer(4), false))
    );
    let configured = Config::parse("amd_av1_tiles=2\n").unwrap();
    let properties = amf(&configured, &av1).unwrap();
    assert_eq!(
        find(&properties, "Av1NumTilesPerFrame"),
        Some((Value::Integer(2), true))
    );
}

#[test]
fn one_reference_avc_uses_idr_recovery_without_ltr_marks() {
    let config = Config::parse("amd_ltr_frames=4\n").unwrap();
    let stream = Negotiated {
        references: 1,
        ..Default::default()
    };
    let mut recovery = crate::ltr::References::new(
        amf_ltr_frames(&config, &stream),
        crate::ltr::Counter::for_codec(stream.codec),
    );
    for frame in 1..=64 {
        let plan = recovery.plan(frame, frame == 1);
        assert!(plan.mark.is_none());
        assert!(plan.reference.is_none());
        recovery.accepted(frame, &plan);
    }
    assert!(!recovery.enabled());
    assert!(!recovery.invalidate(60, 61));
}

#[test]
fn amf_ltr_preserves_unrestricted_clients_and_explicit_disable() {
    let stream = Negotiated {
        codec: 2,
        ..Default::default()
    };
    for (requested, expected) in [(-1, 0), (0, 0), (1, 1), (4, 4), (99, 4)] {
        let config = Config::parse(&format!("amd_ltr_frames={requested}\n")).unwrap();
        assert_eq!(amf_ltr_frames(&config, &stream), expected);
        // AV1 does not refresh, so asking for it keeps the anchors.
        assert_eq!(
            amf_ltr_frames(
                &config,
                &Negotiated {
                    intra_refresh: true,
                    ..stream.clone()
                }
            ),
            expected
        );
    }
}
