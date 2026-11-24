//! Tests for session warnings, roaming, HDR metadata, failures, and console info snapshots.
use super::super::warnings::warning_bit;
use super::*;

#[test]
fn events_expire_and_clearing_absent_codes_skips_the_lock() {
    let warnings = Warnings::default();
    warnings.clear("encoder_recovery");
    assert_eq!(warnings.present.load(Ordering::Relaxed), 0);
    warnings.set("display_virtual", "Physical display in use");
    warnings.event("network_send", "Packets dropped", Duration::ZERO);
    // An expired event disappears; a lasting condition stays.
    assert_eq!(
        warnings
            .snapshot()
            .iter()
            .map(|w| w.code.as_str())
            .collect::<Vec<_>>(),
        ["display_virtual"]
    );
    assert_eq!(
        warnings.present.load(Ordering::Relaxed),
        warning_bit("display_virtual")
    );
    warnings.event("network_send", "Packets dropped", Duration::from_secs(60));
    assert_eq!(warnings.snapshot().len(), 2);
    warnings.clear("display_virtual");
    warnings.clear("network_send");
    assert!(warnings.snapshot().is_empty());
    assert_eq!(warnings.present.load(Ordering::Relaxed), 0);
}
#[test]
fn warnings_survive_preparation_and_clear_after_recovery() {
    let launch = launch("warnings", Role::Stream);
    launch.warnings.set("display", "Physical display in use");
    let session = Session::new(launch.clone(), Negotiated::default());
    *session.encoder.write().unwrap() = "amf".into();
    let capture = Arc::new(Warnings::default());
    *session.capture_warnings.write().unwrap() = capture.clone();
    capture.set("capture", "Desktop Duplication in use");
    launch.warnings.set("display", "Physical display in use");
    assert_eq!(session.info()["encoder"], "amf");
    assert_eq!(
        session.info()["warnings"],
        serde_json::json!([
            {"code":"display", "message":"Physical display in use"},
            {"code":"capture", "message":"Desktop Duplication in use"}
        ])
    );
    capture.set("capture", "Capture recovering");
    assert_eq!(
        session.info()["warnings"][1]["message"],
        "Capture recovering"
    );
    capture.clear("capture");
    launch.warnings.clear("display");
    assert_eq!(session.info()["warnings"], serde_json::json!([]));
    let other = Session::new(super::launch("other", Role::Stream), Negotiated::default());
    launch.warnings.set("audio", "Audio interrupted");
    assert_eq!(other.info()["warnings"], serde_json::json!([]));
}
#[test]
fn a_capped_stream_reports_its_display_size_beside_the_coded_picture() {
    let mut config = Negotiated {
        width: 2560,
        height: 1440,
        ..Default::default()
    };
    config.set_display_size((3840, 2160), Some((2560, 1440)));
    let session = Session::new(launch("capped", Role::Stream), config);
    let info = session.info();
    assert_eq!(
        (info["width"].as_u64(), info["height"].as_u64()),
        (Some(2560), Some(1440))
    );
    assert_eq!(
        (
            info["display_width"].as_u64(),
            info["display_height"].as_u64()
        ),
        (Some(3840), Some(2160))
    );
    // Without a cap both pairs are the negotiated size.
    let plain = Session::new(launch("plain", Role::Stream), Negotiated::default());
    let info = plain.info();
    assert_eq!(
        (
            info["display_width"].as_u64(),
            info["display_height"].as_u64()
        ),
        (Some(1920), Some(1080))
    );
}
#[test]
fn a_roam_moves_the_client_and_restarts_the_media_ping_wait() {
    let mut launch = launch("roam", Role::Stream);
    launch.peer = "::ffff:192.168.1.20".parse().unwrap();
    let session = Session::new(launch, Negotiated::default());
    let start = Instant::now();
    assert_eq!(
        session.client_ip(),
        "192.168.1.20".parse::<IpAddr>().unwrap()
    );
    assert_eq!(session.media_wait_start(start), start);
    assert!(!session.take_new_path());
    let at = start + Duration::from_secs(60);
    session.roamed("::ffff:10.0.0.7".parse().unwrap(), at);
    assert_eq!(session.client_ip(), "10.0.0.7".parse::<IpAddr>().unwrap());
    assert_eq!(session.roams(), 1);
    assert_eq!(session.media_wait_start(start), at);
    assert!(session.take_new_path(), "the first ping from the new path");
    assert!(!session.take_new_path(), "once");
}
#[test]
fn a_new_path_forgets_the_old_paths_pacing_estimate_and_keyframe_loss() {
    let session = Session::new(launch("new-path", Role::Stream), Negotiated::default());
    session.configure_automatic_pacing(true);
    session.configure_keyframe_loss(true);
    let lose = |session: &Session| {
        for frame in 10..=12 {
            session.keyframe_loss.lock().unwrap().unrecoverable(
                frame,
                frame,
                Duration::from_millis(16),
                Instant::now(),
            );
        }
    };
    lose(&session);
    assert!(session.keyframe_loss_active());
    session.video_pacing_bps(100_000_000, 10_000_000);
    let ceiling = |session: &Session| {
        session.automatic_pacing.lock().unwrap().snapshot()["ceiling_kbps"].clone()
    };
    assert_ne!(ceiling(&session), 0);
    session.configure_adaptive_bitrate(true, 0);
    {
        let mut bitrate = session.adaptive_bitrate.lock().unwrap();
        bitrate.on_block(butterpollo_media_policy::adaptive_fec::Block::Clean);
        let start = Instant::now();
        for i in 0..2 {
            let now = start + Duration::from_secs(i);
            bitrate.on_ping(now, &[4, 0, 0, 0, 0, 0, 0, 0]);
            bitrate.tick(
                now,
                Default::default(),
                Duration::from_millis(2),
                20,
                Duration::from_millis(16),
                20_000,
            );
        }
        assert_eq!(bitrate.minimum_rtt(), Some(Duration::from_millis(2)));
    }
    session.new_network_path();
    assert!(!session.keyframe_loss_active());
    assert_eq!(ceiling(&session), 0);
    assert_eq!(session.adaptive_bitrate.lock().unwrap().minimum_rtt(), None);
    assert!(session.automatic_pacing_enabled());
    // The guard still watches the new path.
    lose(&session);
    assert!(session.keyframe_loss_active());
}
#[test]
fn fec_status_clients_do_not_latch_on_requests_even_after_a_roam() {
    const FAST: u64 = 800_000_000;
    const SAFE: u64 = 62_000_000;
    for features in [1, 0] {
        let s = Session::new(
            launch("fec-status", Role::Stream),
            Negotiated {
                moonlight_features: features,
                ..Default::default()
            },
        );
        s.configure_keyframe_loss(true);
        // Asked, sent keyframe 41, asked again right after it.
        let ask_twice = |s: &Session, first: u32| {
            for frame in first..first + 40 {
                s.keyframe_loss_bps(FAST, SAFE, frame, frame == first);
            }
            s.keyframe_loss.lock().unwrap().settle();
            s.request_recovery_keyframe();
            s.keyframe_loss_bps(FAST, SAFE, first + 40, true);
            s.keyframe_loss_bps(FAST, SAFE, first + 41, false);
            s.request_recovery_keyframe();
        };
        ask_twice(&s, 1);
        assert_eq!(s.keyframe_loss_active(), features == 0, "{features}");
        s.new_network_path();
        assert!(!s.keyframe_loss_active());
        ask_twice(&s, 100);
        assert_eq!(
            s.keyframe_loss_active(),
            features == 0,
            "after a roam, {features}"
        );
    }
}
#[test]
fn a_roam_drops_the_clients_hints_about_the_old_path() {
    let session = Session::new(
        launch("hints", Role::Stream),
        Negotiated {
            codec: 3,
            bitrate_kbps: 200_000,
            configured_bitrate_kbps: 250_000,
            pyrowave_client_link_mbps: 100,
            pyrowave_client_pace_mbps: 150,
            ..Default::default()
        },
    );
    let ceiling = |session: &Session| {
        session.configure_adaptive_bitrate(true, 0);
        session.adaptive_bitrate.lock().unwrap().snapshot()["wire_ceiling_kbps"].clone()
    };
    assert_eq!(
        (session.client_link_mbps(), session.client_pace_mbps()),
        (100, 150)
    );
    assert_eq!(ceiling(&session), 95_000);
    session.roamed("10.0.0.7".parse().unwrap(), Instant::now());
    assert_eq!(
        (session.client_link_mbps(), session.client_pace_mbps()),
        (0, 0)
    );
    assert_eq!(ceiling(&session), 250_000);
}
#[test]
fn the_console_gets_pipeline_rates_once_a_window_is_complete() {
    let session = Session::new(launch("rates", Role::Stream), Negotiated::default());
    assert!(session.info()["pipeline_rates"].is_null());
    session.stats.pipeline.captured(1, 2);
    session.stats.pipeline.submitted(true);
    session.stats.frames.fetch_add(1, Ordering::Relaxed);
    assert_eq!(
        session.pipeline_counts(),
        butterpollo_media_policy::pipeline_rates::Counts {
            source: 1,
            capture: 2,
            submit: 1,
            output: 1,
            repeat: 1,
        }
    );
    std::thread::sleep(butterpollo_media_policy::pipeline_rates::CONSOLE_WINDOW);
    let rates = &session.info()["pipeline_rates"];
    assert!(rates["window_ms"].as_u64().unwrap() >= 1000, "{rates}");
    assert!(rates["capture_mhz"].as_u64().unwrap() > 0, "{rates}");
    assert!(rates["repeat_mhz"].as_u64().unwrap() > 0, "{rates}");
}
#[test]
fn client_luminance_changes_never_relabel_hdr_content() {
    use rubylight_protocol::control::DisplayCaps;
    let display = butterpollo_media_policy::hdr::Metadata::display(1000., 0.01, 600.);
    for hdr in [false, true] {
        let s = Session::new(
            launch("hdr", Role::Stream),
            Negotiated {
                hdr,
                ..Default::default()
            },
        );
        let mut caps = DisplayCaps {
            hdr: true,
            max_centinits: 260_000,
            max_average_centinits: 120_000,
            min_decimillinits: 5,
        };
        // Early reports must not start HDR with the receiver's luminance.
        assert!(s.record_display_caps(&caps.encode()).unwrap().changed);
        assert!(s.hdr_metadata.read().unwrap().is_none());
        assert_eq!(s.set_display_hdr_metadata(display), display);
        // Brighter, dimmer, unknown and SDR receiver reports all preserve
        // the source metadata, preventing HDR control updates/codec restarts.
        for peak in [40_000, 100_000, 260_000, 0] {
            caps.max_centinits = peak;
            caps.max_average_centinits = 0;
            caps.hdr = peak != 0;
            s.record_display_caps(&caps.encode()).unwrap();
            assert_eq!(*s.hdr_metadata.read().unwrap(), Some(display));
            assert_eq!(s.set_display_hdr_metadata(display), display);
        }
        assert!(s.record_display_caps(&caps.encode()[..8]).is_none());
        assert_eq!(*s.hdr_metadata.read().unwrap(), Some(display));
        // A real source change must still reach the encoder and client.
        let new_source = butterpollo_media_policy::hdr::Metadata::display(1500., 0.001, 800.);
        assert_eq!(s.set_display_hdr_metadata(new_source), new_source);
        assert_eq!(*s.hdr_metadata.read().unwrap(), Some(new_source));
    }
}
#[test]
fn what_a_launch_holds_goes_with_its_last_copy_and_never_with_an_rtsp_snapshot() {
    let held: Arc<dyn std::any::Any + Send + Sync> = Arc::new(());
    let given_back = Arc::downgrade(&held);
    let mut first = launch("held", Role::Stream);
    first.held = Some(held);
    let copy = first.clone();
    assert!(first.rtsp_snapshot().held.is_none());
    drop(first);
    assert!(given_back.upgrade().is_some());
    drop(copy);
    assert!(given_back.upgrade().is_none());
}
#[test]
fn a_ten_bit_format_is_sdr_only_when_the_query_says_hdr_mode_zero() {
    // Launch and resume carry the same query, so one table covers both.
    for (hdr_mode, sdr) in [(None, false), (Some("0"), true), (Some("1"), false)] {
        let mut launch = launch("color", Role::Stream);
        if let Some(mode) = hdr_mode {
            launch.options.insert("hdrMode".into(), mode.into());
        }
        assert_eq!(launch.ten_bit_sdr_requested(), sdr, "hdrMode={hdr_mode:?}");
    }
}
#[test]
fn a_failed_stream_is_not_reported_as_a_normal_close() {
    let closed = Session::new(launch("closed", Role::Stream), Negotiated::default());
    closed.stop();
    assert_eq!(closed.termination_reason(), 0x8003_0023);
    let failed = Session::new(launch("failed", Role::Stream), Negotiated::default());
    failed.fail("the capture stopped");
    assert!(failed.stopping());
    assert_eq!(failed.termination_reason(), 0x8000_4005);
    // The first reason is the one the client reads; what follows is its consequence.
    failed.fail("the control stream timed out");
    assert_eq!(failed.failure().as_deref(), Some("the capture stopped"));
    assert_eq!(closed.failure(), None);
}
#[test]
fn a_failure_reason_is_safe_to_send_in_serverinfo() {
    // Driver and FFI strings can carry NUL and other control characters, which
    // XML cannot hold; tab and line breaks stay.
    assert_eq!(
        failure_text("AMF\0 error\u{1b}[0m\tline\r\nnext"),
        "AMF error[0m\tline\r\nnext"
    );
    // At most FAILURE_TEXT_MAX bytes, cut between characters.
    let long = "\u{e9}".repeat(FAILURE_TEXT_MAX);
    let text = failure_text(&long);
    assert_eq!(text.len(), FAILURE_TEXT_MAX);
    assert!(text.chars().all(|c| c == '\u{e9}'));
    assert_eq!(
        failure_text(&format!("x{long}")).len(),
        FAILURE_TEXT_MAX - 1
    );
    // Through `fail_with`: only the outer two contexts, which name what failed.
    let session = Session::new(launch("chain", Role::Stream), Negotiated::default());
    session.fail_with(
        &anyhow::anyhow!("C:\\Users\\someone\\driver.dll: bad")
            .context("AMF error AMF_FAIL (1)")
            .context("the encoder kept failing"),
    );
    assert_eq!(
        session.failure().as_deref(),
        Some("the encoder kept failing: AMF error AMF_FAIL (1)")
    );
    // Through `fail`: cleaned and capped as well.
    let session = Session::new(launch("nul", Role::Stream), Negotiated::default());
    session.fail(format!("capture\0 {long}"));
    let failure = session.failure().unwrap();
    assert!(
        failure.starts_with("capture ") && failure.len() <= FAILURE_TEXT_MAX,
        "{failure}"
    );
}

#[test]
fn an_encoder_failure_sends_the_encoder_termination_code() {
    let other = Session::new(launch("other", Role::Stream), Negotiated::default());
    other.fail_with(&anyhow::anyhow!("client video ping timed out"));
    assert!(other.stopping());
    assert_eq!(other.termination_reason(), 0x8000_4005);
    // As the cause or as context below more context, the marker is found.
    for error in [
        anyhow::Error::new(EncoderFailure("the encoder stopped returning frames")),
        anyhow::anyhow!("AMF error")
            .context(EncoderFailure("the encoder kept failing"))
            .context("session"),
    ] {
        let encoder = Session::new(launch("encoder", Role::Stream), Negotiated::default());
        encoder.fail_with(&error);
        assert!(encoder.stopping());
        assert_eq!(encoder.termination_reason(), 0x800E_9403, "{error:#}");
    }
    assert_eq!(
        anyhow::Error::new(EncoderFailure("the encoder stopped returning frames")).to_string(),
        "the encoder stopped returning frames"
    );
}
#[test]
fn only_pyrowave_sessions_report_quality_bitrates() {
    for codec in 0..=3 {
        let config = Negotiated {
            codec,
            rate_millihz: 59_940,
            ..Default::default()
        };
        let session = Session::new(launch("quality", Role::Stream), config.clone());
        let info = session.info();
        if codec == 3 {
            // 59.94 is judged as 60 fps, as the client's minimum and link-rate adaptation are.
            assert_eq!(
                info["pyrowave_minimum_kbps"],
                butterpollo_media_policy::pyrowave::minimum_kbps(
                    config.width,
                    config.height,
                    60_000
                )
            );
            assert_eq!(
                info["pyrowave_recommended_kbps"],
                butterpollo_media_policy::pyrowave::recommended_kbps(
                    config.width,
                    config.height,
                    60_000
                )
            );
        } else {
            assert!(info["pyrowave_minimum_kbps"].is_null());
            assert!(info["pyrowave_recommended_kbps"].is_null());
        }
        session.bitrate.store(100_000, Ordering::Relaxed);
        assert_eq!(session.info()["encoder_bitrate_kbps"], 100_000);
    }
    // A client that asked exactly the floor: the published rates are scaled by configured over
    // negotiated like the encoder's, so the card compares equal numbers and stays green.
    let floor = butterpollo_media_policy::pyrowave::minimum_kbps(1920, 1080, 60_000);
    let config = Negotiated {
        codec: 3,
        rate_millihz: 59_940,
        bitrate_kbps: floor - 752,
        configured_bitrate_kbps: floor,
        ..Default::default()
    };
    let session = Session::new(launch("quality", Role::Stream), config);
    session.bitrate.store(floor - 752, Ordering::Relaxed);
    let info = session.info();
    assert_eq!(info["pyrowave_minimum_kbps"], floor - 752);
    assert_eq!(info["encoder_bitrate_kbps"], floor - 752);
}
#[test]
fn the_console_shows_the_encoded_rate_beside_the_requested_one() {
    // Artemis's Warp launches at 60 fps and asks for 120 in the stream
    // request; with the launch frame rate on, 60 is encoded. The console
    // showed 120 as the stream's rate and target.
    let request = Negotiated {
        fps: 120,
        rate_millihz: 120_000,
        ..Default::default()
    };
    let mut config = request.clone();
    let limited = butterpollo_config::config::Config::parse("limit_framerate=true\n").unwrap();
    butterpollo_media_policy::stream_policy::apply(&mut config, 60_000, &limited);
    let session = Session::new(launch("warp", Role::Stream), config.clone());
    let info = session.info();
    assert_eq!(
        (info["fps"].clone(), info["requested_fps"].clone()),
        (60.into(), 120.into())
    );
    // By default the stream runs at the request's rate.
    let mut following = request.clone();
    butterpollo_media_policy::stream_policy::apply(
        &mut following,
        60_000,
        &butterpollo_config::config::Config::default(),
    );
    let info = Session::new(launch("warp", Role::Stream), following).info();
    assert_eq!(
        (info["fps"].clone(), info["requested_fps"].clone()),
        (120.into(), 120.into())
    );
    // A client's own mode switch is what it asked for.
    let switch = butterpollo_media_policy::reconfigure::Mode {
        width: config.width,
        height: config.height,
        fps_millihz: 90_000,
    };
    let mut reconfigure = session.reconfigure.lock().unwrap();
    let now = Instant::now();
    reconfigure.on_request(now, switch);
    assert_eq!(
        reconfigure.take_due(now + Duration::from_secs(1)),
        Some(switch)
    );
    // The session loop marks the switch applied once the encoder runs at it;
    // the console shows what is encoded.
    reconfigure.applied_now(switch);
    drop(reconfigure);
    let info = session.info();
    assert_eq!(
        (info["fps"].clone(), info["requested_fps"].clone()),
        (90.into(), 90.into())
    );
    let unlimited = Negotiated {
        fps: 120,
        rate_millihz: 119_880,
        ..Default::default()
    };
    let info = Session::new(launch("plain", Role::Stream), unlimited).info();
    assert_eq!(
        (info["fps"].clone(), info["requested_fps"].clone()),
        (120.into(), 120.into())
    );
}
#[test]
fn a_sent_picture_feeds_the_client_report_and_the_record_sink_alike() {
    use butterpollo_media_policy::{
        host_timing::HostTimingRequest, performance::Stamps, timing_record,
    };
    use std::time::Duration;
    let session = Session::new(launch("record", Role::Stream), Negotiated::default());
    let epoch = Instant::now();
    let us = |n: u64| epoch + Duration::from_micros(n);
    let stamps = Stamps {
        presented: us(100),
        acquired: us(200),
        published: us(250),
        claimed: us(300),
        submitted: us(500),
        encoded: us(2_500),
        packetized: us(2_600),
        prepared: us(2_700),
        first_sent: us(2_700),
        last_sent: us(2_900),
        streamed: None,
    };
    // Neither a client report nor a sink: nothing is kept.
    session.record_sent(1, 9_000, &stamps, epoch);
    assert_eq!(session.host_timings.take_report(), None);

    let (sink, rows) = timing_record::channel(8);
    *session.timing_record.write().unwrap() = Some(Arc::new(sink));
    session
        .host_timings
        .on_request(&HostTimingRequest { enabled: true }.encode());
    session.record_sent(2, 9_000, &stamps, epoch);
    let report = session.host_timings.take_report().unwrap();
    let (record, extra) =
        butterpollo_media_policy::host_timing::HostTiming::decode_report_with(&report)
            .unwrap()
            .remove(0);
    assert_eq!((record.frame, record.host_us), (2, 2_800));
    assert_eq!(extra.unwrap().presented_at_us, 100);
    let row = rows.try_recv().unwrap();
    assert_eq!(row.wire_frame, 2);
    assert_eq!(row.rtp_us, 100_000);
    assert_eq!(row.points[0], 100);
    assert_eq!(row.points[9], 2_900);
    assert!(rows.try_recv().is_err(), "the first picture had no sink");
}
#[test]
fn slice_delivery_overflows_are_counted_in_the_session_info() {
    let session = Session::new(launch("overflow", Role::Stream), Negotiated::default());
    assert_eq!(session.info()["slice_delivery_overflows"], 0);
    session
        .stats
        .slice_delivery_overflows
        .fetch_add(1, Ordering::Relaxed);
    assert_eq!(session.info()["slice_delivery_overflows"], 1);
}
#[test]
fn the_session_info_names_the_stage_that_took_the_time_pictures_lost() {
    use butterpollo_media_policy::performance::Stamps;
    use std::time::Duration;
    let session = Session::new(launch("stutter", Role::Stream), Negotiated::default());
    let start = Instant::now();
    let stamped = |stages: [u64; 9], at: Instant| {
        let mut points = [at; 10];
        for (index, us) in stages.into_iter().enumerate() {
            points[index + 1] = points[index] + Duration::from_micros(us);
        }
        Stamps {
            presented: points[0],
            acquired: points[1],
            published: points[2],
            claimed: points[3],
            submitted: points[4],
            encoded: points[5],
            packetized: points[6],
            prepared: points[7],
            first_sent: points[8],
            last_sent: points[9],
            streamed: None,
        }
    };
    {
        let mut performance = session.stats.performance.lock().unwrap();
        for n in 0..60 {
            // capture, hold, claim, convert, encode, queue, packetize, send queue, send
            let mut stages = [1_000, 20, 5, 300, 4_000, 10, 40, 50, 500];
            if n == 20 {
                stages[4] = 24_000;
            }
            performance.record_stages(&stamped(stages, start + Duration::from_millis(n)), 0);
        }
    }
    let info = session.info();
    assert_eq!(info["stutter"]["pictures"], 60);
    assert_eq!(info["stutter"]["late_pictures"], 1);
    assert_eq!(info["stutter"]["dominant"], "encode");
    assert_eq!(info["stutter"]["lost_ms"], 20.0);
    assert_eq!(
        info["stutter"]["stages"],
        serde_json::json!([{ "id": "encode", "lost_ms": 20.0, "percent": 100 }])
    );
    assert_eq!(
        info["stutter"]["not_measured"],
        serde_json::json!(["game", "network", "decode", "present"])
    );
    // A stream that has not sent a picture yet has nothing to show.
    let empty = Session::new(launch("cold", Role::Stream), Negotiated::default());
    assert_eq!(empty.info()["stutter"]["pictures"], 0);
    assert_eq!(empty.info()["stutter"]["dominant"], serde_json::Value::Null);
}
