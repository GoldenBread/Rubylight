//! Tests for 10-bit SDR/HDR color negotiation, DSCP tagging, and still-picture resends.

use super::*;

/// A client's ten-bit request after RTSP, as `from_sdp` leaves it.
fn ten_bit_request() -> Negotiated {
    Negotiated {
        hdr: true,
        ..Default::default()
    }
}
/// What the launch asked for, then what RTSP negotiated, as the host applies them.
fn negotiate_color(mut stream: Negotiated, sdr_requested: bool, config: &Config) -> Negotiated {
    infer_sdr_10bit(&mut stream, sdr_requested);
    apply_color(&mut stream, config);
    stream
}
#[test]
fn a_ten_bit_request_with_hdr_mode_zero_is_ten_bit_sdr() {
    let mut stream = ten_bit_request();
    assert!(infer_sdr_10bit(&mut stream, true));
    assert!(stream.sdr_10bit && !stream.hdr && stream.ten_bit());
    // `apply_color` after the inference keeps what it inferred, under the
    // default config and with the host option on.
    for config in [
        Config::default(),
        Config::parse("prefer_sdr_10bit=true\n").unwrap(),
    ] {
        let stream = negotiate_color(ten_bit_request(), true, &config);
        assert!(stream.sdr_10bit && !stream.hdr && stream.ten_bit());
    }
    // And the inference is idempotent.
    let mut again = stream.clone();
    assert!(!infer_sdr_10bit(&mut again, true));
    assert!(again.sdr_10bit && !again.hdr);
}
#[test]
fn a_ten_bit_request_without_hdr_mode_zero_stays_hdr() {
    let stream = negotiate_color(ten_bit_request(), false, &Config::default());
    assert!(stream.hdr && !stream.sdr_10bit);
    assert!(!infer_sdr_10bit(&mut ten_bit_request(), false));
}
#[test]
fn prefer_sdr_10bit_still_turns_an_hdr_launch_into_ten_bit_sdr() {
    let config = Config::parse("prefer_sdr_10bit=true\n").unwrap();
    let stream = negotiate_color(ten_bit_request(), false, &config);
    assert!(stream.sdr_10bit && !stream.hdr);
}
#[test]
fn an_hdr_launch_stays_hdr_while_rtx_hdr_converts_it() {
    let mut config = Config::parse(
        "prefer_sdr_10bit=true
rtx_hdr=true
",
    )
    .unwrap();
    config
        .values
        .insert(crate::nvidia_enhance::marker("rtx_hdr"), "true".into());
    // Chosen on a host that does not offer it: ten-bit SDR as before.
    let stream = negotiate_color(ten_bit_request(), false, &config);
    assert!(stream.sdr_10bit && !stream.hdr);
    config.values.insert(
        crate::nvidia_enhance::OFFERED_KEY.into(),
        crate::nvidia_enhance::RTX_HDR.to_string(),
    );
    let stream = negotiate_color(ten_bit_request(), false, &config);
    assert!(stream.hdr && !stream.sdr_10bit);
}
#[test]
fn a_stream_turned_into_ten_bit_sdr_drops_its_hdr_format_pick() {
    // An HDR10+ pick the host offered, from a client the host streams as
    // ten-bit SDR: by `prefer_sdr_10bit`, or by `hdrMode=0` at launch.
    let request = Negotiated::from_sdp(
        b"a=x-nv-video[0].dynamicRangeMode:1\na=x-nv-vqos[0].bitStreamFormat:1\na=x-rl-video[0].hdrFormat:2\n",
    )
    .unwrap();
    let offered = crate::extensions::Offered {
        hdr_formats: rubylight_protocol::hdr::ALL,
        ..crate::extensions::Offered::NONE
    };
    offered.check_announce(&request).unwrap();
    let prefer = Config::parse("prefer_sdr_10bit=true\n").unwrap();
    for (sdr_requested, config) in [(true, Config::default()), (false, prefer)] {
        let stream = negotiate_color(request.clone(), sdr_requested, &config);
        assert!(stream.sdr_10bit && !stream.hdr);
        assert_eq!(stream.hdr_format, 0);
        // What is streamed is still something the host offered.
        offered.check_announce(&stream).unwrap();
        stream.validate().unwrap();
    }
    // An HDR stream keeps its pick.
    let stream = negotiate_color(request.clone(), false, &Config::default());
    assert!(stream.hdr && stream.hdr_format == 2);
}
#[test]
fn a_packed_stream_drops_dolby_vision_loudly_and_streams_hdr10() {
    use rubylight_protocol::hdr;
    let offered = crate::extensions::Offered {
        hdr_formats: crate::extensions::hdr_offer(0x200, true, true),
        ..crate::extensions::Offered::NONE
    };
    let request = |packed: bool, format: u8| {
        let packing = if packed {
            "a=x-rl-video[0].chromaPacking:1\n"
        } else {
            ""
        };
        Negotiated::from_sdp(
            format!(
                "a=x-nv-video[0].dynamicRangeMode:1\na=x-nv-vqos[0].bitStreamFormat:1\n{packing}a=x-rl-video[0].hdrFormat:{format}\n"
            )
            .as_bytes(),
        )
        .unwrap()
    };
    let warnings = crate::TestWarnings::default();
    let mut packed = request(true, hdr::DV_81);
    offered.check_announce(&packed).unwrap();
    assert!(packed_drops_dolby_vision(&mut packed, &warnings));
    // HDR10, which HDR10+ rides on, and what is streamed was offered.
    assert!(packed.hdr && packed.hdr_format == 0);
    assert_eq!(crate::dovi::negotiated(&packed), None);
    offered.check_announce(&packed).unwrap();
    let shown = warnings.snapshot();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].code, "hdr_dolby_vision");
    assert!(
        shown[0]
            .message
            .contains("unpacks the packed picture itself")
    );
    assert!(shown[0].message.contains("static HDR10 instead"));
    // A 4:2:0 stream keeps its pick, and the next stream clears the warning.
    let mut plain = request(false, hdr::DV_81);
    assert!(!packed_drops_dolby_vision(&mut plain, &warnings));
    assert_eq!(plain.hdr_format, hdr::DV_81);
    assert!(crate::dovi::negotiated(&plain).is_some());
    assert!(warnings.snapshot().is_empty());
    // A packed PQ pick is not Dolby Vision and stays.
    let mut pq = request(true, hdr::PQ);
    assert!(!packed_drops_dolby_vision(&mut pq, &warnings));
    assert_eq!(pq.hdr_format, hdr::PQ);
    // Packed 8.4 goes as Dolby Vision, to HDR10 rather than HLG.
    let mut p84 = request(true, hdr::DV_84);
    assert!(packed_drops_dolby_vision(&mut p84, &warnings));
    assert!(!packed_drops_hlg(&mut p84, &warnings));
    assert!(p84.hdr && p84.hdr_format == 0 && !p84.hlg());
}
#[test]
fn a_native_444_stream_drops_dolby_vision_loudly_and_streams_hdr10() {
    use rubylight_protocol::hdr;
    let offered = crate::extensions::Offered {
        hdr_formats: crate::extensions::hdr_offer(0x200, true, true),
        ..crate::extensions::Offered::NONE
    };
    let warnings = crate::TestWarnings::default();
    for format in [hdr::DV_81, hdr::DV_84] {
        let mut native = Negotiated::from_sdp(
            format!(
                "a=x-nv-video[0].dynamicRangeMode:1\na=x-nv-vqos[0].bitStreamFormat:1\na=x-ss-video[0].chromaSamplingType:1\na=x-rl-video[0].hdrFormat:{format}\n"
            )
            .as_bytes(),
        )
        .unwrap();
        assert!(native.yuv444 && native.chroma_packing == 0);
        offered.check_announce(&native).unwrap();
        assert!(packed_drops_dolby_vision(&mut native, &warnings));
        // Dolby Vision profile 8 is HEVC Main10 4:2:0; the HEVC Rext stream
        // carries static HDR10, which was offered.
        assert!(native.hdr && native.hdr_format == 0);
        assert_eq!(crate::dovi::negotiated(&native), None);
        offered.check_announce(&native).unwrap();
        let shown = warnings.snapshot();
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].code, "hdr_dolby_vision");
        assert_eq!(shown[0].message, NATIVE_444_DOLBY_VISION);
    }
}
#[test]
fn a_full_range_stream_drops_dolby_vision_loudly_and_streams_hdr10() {
    use rubylight_protocol::hdr;
    let warnings = crate::TestWarnings::default();
    for (codec, format) in [(1, hdr::DV_81), (1, hdr::DV_84), (2, hdr::DV_101)] {
        let mut full = Negotiated::from_sdp(
            format!(
                "a=x-nv-video[0].dynamicRangeMode:1\na=x-nv-video[0].encoderCscMode:5\na=x-nv-vqos[0].bitStreamFormat:{codec}\na=x-rl-video[0].hdrFormat:{format}\n"
            )
            .as_bytes(),
        )
        .unwrap();
        assert!(full.full_range());
        assert!(packed_drops_dolby_vision(&mut full, &warnings));
        assert!(full.hdr && full.hdr_format == 0 && !full.hlg());
        assert_eq!(crate::dovi::negotiated(&full), None);
        let shown = warnings.snapshot();
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].code, "hdr_dolby_vision");
        assert_eq!(shown[0].message, FULL_RANGE_DOLBY_VISION);
    }
}
#[test]
fn a_packed_stream_drops_hlg_loudly_and_streams_hdr10() {
    let offered = crate::extensions::Offered {
        hdr_formats: crate::extensions::hdr_offer(0x200, true, true),
        ..crate::extensions::Offered::NONE
    };
    let request = |packing: &str, codec: u8| {
        Negotiated::from_sdp(
            format!(
                "a=x-nv-video[0].dynamicRangeMode:1\na=x-nv-vqos[0].bitStreamFormat:{codec}\n{packing}a=x-rl-video[0].hdrFormat:2\n"
            )
            .as_bytes(),
        )
        .unwrap()
    };
    let warnings = crate::TestWarnings::default();
    for codec in [1, 2] {
        let mut packed = request("a=x-rl-video[0].chromaPacking:1\n", codec);
        offered.check_announce(&packed).unwrap();
        assert!(packed.hlg());
        assert!(!packed_drops_dolby_vision(&mut packed, &warnings));
        assert!(packed_drops_hlg(&mut packed, &warnings));
        assert!(packed.hdr && packed.hdr_format == 0 && !packed.hlg());
        offered.check_announce(&packed).unwrap();
        let shown = warnings.snapshot();
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].code, "hdr_hlg");
        assert!(shown[0].message.contains("static HDR10 instead"));
        // A 4:2:0 stream keeps HLG, and clears the warning.
        let mut plain = request("", codec);
        assert!(!packed_drops_hlg(&mut plain, &warnings));
        assert!(plain.hlg());
        assert!(warnings.snapshot().is_empty());
    }
}
#[test]
fn an_eight_bit_request_is_neither_hdr_nor_ten_bit() {
    let config = Config::parse("prefer_sdr_10bit=true\n").unwrap();
    for sdr_requested in [false, true] {
        let mut stream = Negotiated::default();
        assert!(!infer_sdr_10bit(&mut stream, sdr_requested));
        let stream = negotiate_color(stream, sdr_requested, &config);
        assert!(!stream.hdr && !stream.sdr_10bit && !stream.ten_bit());
    }
}

#[test]
fn dscp_tagging_off_turns_off_what_the_device_asked_for() {
    let mut stream = artemis_announce(60);
    stream.video_qos = true;
    stream.audio_qos = true;
    apply(&mut stream, 60_000, &Config::parse("").unwrap());
    assert!(stream.video_qos && stream.audio_qos, "on by default");
    apply(
        &mut stream,
        60_000,
        &Config::parse(
            "dscp_tagging=true
",
        )
        .unwrap(),
    );
    assert!(stream.video_qos && stream.audio_qos);
    apply(
        &mut stream,
        60_000,
        &Config::parse(
            "dscp_tagging=false
",
        )
        .unwrap(),
    );
    assert!(!stream.video_qos && !stream.audio_qos);
    // On never turns on what a remote device did not ask for.
    let mut remote = artemis_announce(60);
    remote.video_qos = false;
    remote.audio_qos = false;
    apply_dscp(
        &mut remote,
        &Config::parse(
            "dscp_tagging=true
",
        )
        .unwrap(),
    );
    assert!(!remote.video_qos && !remote.audio_qos);
}
#[test]
fn the_source_phase_learns_only_the_focus_pictures() {
    // Another window's update next to the focus window's picture is no
    // update of the source. Learned as one, a window composed at 240 Hz
    // beside the 60 Hz game could move the learned phase onto its own
    // compositions: for seconds each game frame then waited about 4 ms for
    // the next of them, which a change of the warm-up alone (how soon the
    // focus window counts as animating) brought out in one seed of four.
    for other in [1000. / 245., 1000. / 240., 1000. / 144.] {
        for seed in 0..16 {
            let (compositions, frames) = mixed_desktop(25., other, seed * 10);
            for duplicate_other in [false, true] {
                let claims = simulate_focus(
                    1000. / 60.,
                    &focus_labels(&compositions, true, duplicate_other),
                    |i| 0.08 + 0.12 * noise(i, seed + 99),
                    vrr_desktop,
                );
                let steady = &claims[300..];
                let case =
                    format!("other {other:.2} ms seed {seed} duplicate other {duplicate_other}");
                assert_eq!(game_frames_shown(steady, &frames), (0, 0, 0), "{case}");
                let shown = |at: f64| frames.partition_point(|f| *f <= at + 1e-9);
                let mut ages: Vec<f64> = steady
                    .iter()
                    .map(|(claim, presented)| claim - frames[shown(*presented).max(1) - 1])
                    .collect();
                ages.sort_by(f64::total_cmp);
                let p95 = ages[ages.len() * 95 / 100];
                assert!(p95 < 1., "{case}: game age p95 {p95:.2} ms");
            }
        }
    }
}

/// Resends of a still picture at 180 fps whose loop wakes `late` after each
/// due slot: how many go out in one second.
fn still_resends_in_a_second(late: impl Fn(u32) -> Duration) -> u32 {
    let period = Duration::from_secs_f64(1. / 180.);
    let start = Instant::now();
    let mut anchor = start;
    let mut sent = 0;
    loop {
        let begin = anchor + period + late(sent);
        if begin > start + Duration::from_secs(1) {
            return sent;
        }
        sent += 1;
        anchor = resend_anchor(anchor, begin, period, false, false);
    }
}

#[test]
fn resends_anchored_on_their_grid_hold_the_stream_rate() {
    // `resend_anchor` alone, without the pacer's `resend_at` (see
    // `resends_on_their_grid_never_come_closer_than_a_claim_nor_hold_back_a_new_picture`).
    // Issue #110: each resend counted from when the previous one began, so
    // 0.3 ms of wake-up latency a resend sent about 170 of 180 fps.
    let steady = still_resends_in_a_second(|_| Duration::from_micros(300));
    assert!((179..=180).contains(&steady), "{steady} resends a second");
    // Uneven wake-ups up to most of a period still keep the rate.
    let uneven = still_resends_in_a_second(|n| {
        Duration::from_micros([200, 1_500, 4_000, 600][n as usize % 4])
    });
    assert!((179..=180).contains(&uneven), "{uneven} resends a second");
}

#[test]
fn a_resend_restarts_its_grid_after_a_stall_and_a_new_picture_anchors_at_its_claim() {
    let period = Duration::from_secs_f64(1. / 180.);
    let previous = Instant::now();
    let due = previous + period;
    // A little late: the next resend stays on the grid.
    assert_eq!(
        resend_anchor(
            previous,
            due + Duration::from_millis(1),
            period,
            false,
            false
        ),
        due
    );
    // A period or more late (a stall): restart at the claim, no burst.
    let stalled = due + period * 3;
    assert_eq!(
        resend_anchor(previous, stalled, period, false, false),
        stalled
    );
    // Early (the due slot moved, e.g. a shorter resend period): no slot in the past is owed.
    let early = previous + period / 2;
    assert_eq!(resend_anchor(previous, early, period, false, false), early);
    // A new picture and a recovery re-encode count from their own claim.
    let late = due + Duration::from_millis(2);
    assert_eq!(resend_anchor(previous, late, period, true, false), late);
    assert_eq!(resend_anchor(previous, late, period, false, true), late);
}

/// Resends of a still picture on their grid under arrival pacing, whose loop
/// wakes `late` after each time the pacer allows: after every resend, how
/// long a new picture arriving half a period later waits past the spacing
/// bound (the longest such wait), the closest two resends came, and how many
/// resends went out in the first second.
fn paced_resends(fps: f64, spacing: f64, late: [u64; 4]) -> (Duration, Duration, u32) {
    let period = Duration::from_secs_f64(1. / fps);
    let start = Instant::now();
    let mut pacer = Pacer::new(start, period).with_spacing(spacing);
    let mut anchor = start;
    let mut previous: Option<Instant> = None;
    let (mut extra, mut closest, mut in_first_second) = (Duration::ZERO, Duration::MAX, 0);
    for n in 0..(fps as usize * 2) {
        let due = anchor + period;
        let begin = pacer.resend_at(due, due) + Duration::from_micros(late[n % 4]);
        if let Some(previous) = previous {
            closest = closest.min(begin - previous);
        }
        if begin <= start + Duration::from_secs(1) {
            in_first_second += 1;
        }
        pacer.claimed(begin);
        anchor = resend_anchor(anchor, begin, period, false, false);
        previous = Some(begin);
        let arrives = begin + period / 2;
        let bound = arrives.max(begin + period.mul_f64(spacing));
        extra = extra.max(pacer.allowed_at(arrives).saturating_duration_since(bound));
    }
    (extra, closest, in_first_second)
}

#[test]
fn resends_on_their_grid_never_come_closer_than_a_claim_nor_hold_back_a_new_picture() {
    // One resend 4 ms late and the next on time on the grid went out 1.6 ms
    // apart at 180 fps: two claims' credit spent, and the next new picture
    // waited up to 2.7 ms past the spacing bound for it.
    // The resend rate is what gives way: steady lateness (issue #110) keeps
    // 179 of 180 (119 of 120) resends a second, the 2 ms pattern 179 at
    // spacing 0.75 and 163 at 0.5, and the 4 ms pattern 158 and 150 at
    // 180 fps (116 and 108 at 120). Each floor is two under what this model
    // sends.
    let lates = [
        [200, 1_500, 4_000, 600],
        [300, 300, 300, 300],
        [200, 1_500, 2_000, 600],
    ];
    for (fps, spacing, floors) in [
        (180., 0.75, [156, 177, 177]),
        (180., 0.5, [148, 177, 161]),
        (120., 0.5, [106, 117, 113]),
        (120., 0.75, [114, 117, 117]),
    ] {
        let period = Duration::from_secs_f64(1. / fps);
        for (late, floor) in lates.into_iter().zip(floors) {
            let (extra, closest, sent) = paced_resends(fps, spacing, late);
            assert_eq!(
                extra,
                Duration::ZERO,
                "{fps} fps, spacing {spacing}, {late:?}"
            );
            assert!(
                closest >= period.mul_f64(spacing),
                "{fps} fps, spacing {spacing}, {late:?}: {closest:?}"
            );
            assert!(
                sent >= floor,
                "{fps} fps, spacing {spacing}, {late:?}: {sent} resends"
            );
        }
    }
}
