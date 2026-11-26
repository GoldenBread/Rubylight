//! Tests for route-unavailable recovery, congestion-warning separation, and adaptive pacing.

use super::*;

fn test_fec_report(frame: u32, received_data: u16, received_parity: u16) -> [u8; 21] {
    let mut report = [0u8; 21];
    report[..4].copy_from_slice(&frame.to_be_bytes());
    for (offset, word) in [
        (4, 11u16),
        (10, 10),
        (12, 2),
        (14, received_data),
        (16, received_parity),
    ] {
        report[offset..offset + 2].copy_from_slice(&word.to_be_bytes());
    }
    report[18] = 20;
    report[20] = 1;
    report
}

fn route_drop_frame(
    wire_frame: u32,
    idr: bool,
    non_route_unrecoverable: bool,
    reached_socket: bool,
    abandoned: usize,
    drops: butterpollo_win_io::net::BatchDropDelta,
) -> FrameDrop {
    FrameDrop {
        wire_frame,
        idr,
        non_route_unrecoverable,
        reached_socket,
        abandoned,
        drops,
    }
}

#[test]
fn route_unavailable_drops_avoid_keyframe_storms_and_buffer_congestion_warnings() {
    use butterpollo_win_io::net::BatchDropDelta;
    use std::sync::atomic::Ordering;

    let fixture = crate::state::test_support::Fixture::new();
    let session = Session::new(
        fixture.launch(fixture.client(0), Role::Stream),
        butterpollo_media_policy::rtsp::Negotiated {
            codec: 1,
            bitrate_kbps: 20_000,
            configured_bitrate_kbps: 20_000,
            ..Default::default()
        },
    );
    session.configure_keyframe_loss(true);
    session.keyframe_loss.lock().unwrap().sending_at(
        1,
        true,
        Instant::now()
            - butterpollo_media_policy::keyframe_loss::STARTUP_WINDOW
            - Duration::from_millis(50),
    );
    let period = Duration::from_micros(16_667);
    let mut send_loss = SendLossRecovery::default();
    let route_only = BatchDropDelta {
        dropped: 20,
        route_unavailable: 20,
        ..Default::default()
    };

    let route = Link::default();
    let packetizer = VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
        packet_size: 1392,
        fec_percent: 20,
        min_fec: 2,
        key: Some([0; 16]),
    };
    let peer: SocketAddr = "192.0.2.9:12345".parse().unwrap();
    let _ = pace(
        &session,
        &Config::default(),
        &packetizer,
        period,
        peer,
        route,
    );
    // Prime LinkRate with delivered frames so E4 keyframe requests would trigger a cut if counted.
    session.configure_link_rate(true, 0);
    session.record_link_sent(1, 100, 112_600);
    std::thread::sleep(Duration::from_millis(20));
    for frame in 2..=10u32 {
        session.record_link_sent(frame, 100, 112_600);
    }
    session.stats.video_frame.store(100, Ordering::Release);

    // Stock Moonlight (no FEC status) sends 0x0302; batch 0 of keyframe 11 reaches the socket
    // before route loss, then >8 (15) completely-unsent P-frames drop as well: repeated
    // 0x0302 requests (even reaching KEYFRAME_REQUESTS) and delayed FEC reports must neither
    // latch KeyframeLoss nor step down LinkRate.
    session.request_client_keyframe();
    assert_eq!(session.stats.idr_requests.load(Ordering::Relaxed), 1);
    session.keyframe_loss_bps(800_000_000, 108_000_000, 11, true);
    on_block_unrecoverable(
        &session,
        &mut send_loss,
        Instant::now(),
        period,
        true,
        false,
        route_only,
    );
    finish_frame_drops(
        &session,
        &mut send_loss,
        route_drop_frame(11, true, false, true, 10, route_only),
    );
    for frame in 12..=26u32 {
        session.keyframe_loss_bps(800_000_000, 108_000_000, frame, false);
        on_block_unrecoverable(
            &session,
            &mut send_loss,
            Instant::now(),
            period,
            false,
            false,
            route_only,
        );
        finish_frame_drops(
            &session,
            &mut send_loss,
            route_drop_frame(frame, false, false, false, 20, route_only),
        );
    }
    assert!(!session.keyframe_loss_active());
    assert!(session.keyframe_loss.lock().unwrap().is_abandoned(11));
    assert!(
        !session
            .keyframe_loss
            .lock()
            .unwrap()
            .network_request(Instant::now())
    );
    for _ in 0..3 {
        session.request_client_keyframe();
    }
    assert_eq!(
        session.tick_link_rate(
            Instant::now() + Duration::from_millis(50),
            Duration::from_millis(10)
        ),
        None
    );
    let snap = session.link_rate_snapshot();
    assert_eq!(snap["state"], "full");
    assert_eq!(snap["steps_down"], 0);
    assert!(!session.keyframe_loss_active());
    assert!(!session.keyframe_loss_ever_latched());
    assert_eq!(session.stats.idr_requests.load(Ordering::Relaxed), 4);
    assert_eq!(
        session.stats.send_loss_recoveries.load(Ordering::Relaxed),
        0
    );
    let warnings = serde_json::to_string(&session.launch.warnings.snapshot()).unwrap();
    assert!(
        warnings.contains(ROUTE_UNAVAILABLE_WARNING),
        "warnings={warnings}"
    );
    assert!(
        !warnings.contains(SEND_CONGESTION_WARNING),
        "warnings={warnings}"
    );
    assert!(
        !warnings.contains("could not send video packets fast enough"),
        "warnings={warnings}"
    );

    // Route returns on P-frame 27: request one host keyframe; another 0x0302 before keyframe 28
    // enters sending_at stays suppressed via host_reason.
    let clean = BatchDropDelta::default();
    finish_frame_drops(
        &session,
        &mut send_loss,
        route_drop_frame(27, false, false, true, 0, clean),
    );
    assert_eq!(session.stats.idr_requests.load(Ordering::Relaxed), 5);
    session.request_client_keyframe();
    assert_eq!(
        session.tick_link_rate(
            Instant::now() + Duration::from_millis(60),
            Duration::from_millis(10)
        ),
        None
    );
    assert_eq!(session.link_rate_snapshot()["steps_down"], 0);
    assert_eq!(session.stats.idr_requests.load(Ordering::Relaxed), 6);

    // Recovery keyframe 28 starts, then delayed 0x5502 Unrecoverable arrives for frame 11.
    session.keyframe_loss_bps(800_000_000, 108_000_000, 28, true);
    session.record_fec_status(&test_fec_report(11, 3, 1));
    assert!(!session.keyframe_loss_active());
    assert!(!session.keyframe_loss_ever_latched());
    assert_eq!(
        session.tick_link_rate(
            Instant::now() + Duration::from_millis(70),
            Duration::from_millis(10)
        ),
        None
    );
    assert_eq!(session.link_rate_snapshot()["state"], "full");
    assert_eq!(session.link_rate_snapshot()["steps_down"], 0);
    assert_eq!(
        session.stats.send_loss_recoveries.load(Ordering::Relaxed),
        0
    );

    finish_frame_drops(
        &session,
        &mut send_loss,
        route_drop_frame(29, false, false, true, 0, clean),
    );
    assert_eq!(session.stats.idr_requests.load(Ordering::Relaxed), 6);
    assert_eq!(
        session.stats.send_loss_recoveries.load(Ordering::Relaxed),
        0
    );

    // Real buffer refusal (WSAEWOULDBLOCK) still latches keyframe_loss after three lost frames.
    let congested = BatchDropDelta {
        dropped: 8,
        refused: 8,
        ..Default::default()
    };
    on_block_unrecoverable(
        &session,
        &mut send_loss,
        Instant::now(),
        period,
        false,
        true,
        congested,
    );
    for frame in 30..=32u32 {
        finish_frame_drops(
            &session,
            &mut send_loss,
            route_drop_frame(frame, false, true, false, 8, congested),
        );
    }
    assert_eq!(
        session.stats.send_loss_recoveries.load(Ordering::Relaxed),
        1
    );
    assert!(session.keyframe_loss_active());
    let warnings = serde_json::to_string(&session.launch.warnings.snapshot()).unwrap();
    assert!(
        warnings.contains(SEND_CONGESTION_WARNING),
        "warnings={warnings}"
    );
}

#[test]
fn wireless_hotspot_route_outage_skips_adaptive_pacing_bitrate_and_fec() {
    use butterpollo_win_io::net::BatchDropDelta;
    use std::sync::atomic::Ordering;

    let fixture = crate::state::test_support::Fixture::new();
    let session = Session::new(
        fixture.launch(fixture.client(0), Role::Stream),
        butterpollo_media_policy::rtsp::Negotiated {
            codec: 1,
            client_wireless: true,
            bitrate_kbps: 40_000,
            configured_bitrate_kbps: 0,
            ..Default::default()
        },
    );
    let config = Config::default();
    assert!(!configure(&session, &config));
    assert!(session.automatic_pacing_enabled());
    session.configure_adaptive_bitrate(true, 0);
    assert_eq!(session.configure_fec(true, 20), 20);

    let start = Instant::now();
    assert_eq!(
        session.video_pacing_bps(800_000_000, 50_000_000),
        80_000_000
    );
    session.tick_automatic_pacing(start, Duration::from_millis(2));
    session.tick_adaptive_bitrate(start, Duration::from_millis(2));

    // Clean link decays adaptive FEC below ceiling before the hotspot outage hits.
    session.stats.frames.store(60, Ordering::Relaxed);
    session.stats.packets.store(60 * 70, Ordering::Relaxed);
    session.stats.bytes.store(60 * 83_334, Ordering::Relaxed);
    session.stats.video_frame.store(61, Ordering::Release);
    session.record_fec_status(&test_fec_report(10, 10, 0));
    session.tick_fec(start + Duration::from_secs(60));
    assert_eq!(
        session.tick_adaptive_bitrate(start + Duration::from_secs(60), Duration::from_millis(2)),
        None
    );
    assert!(session.adaptive_bitrate.lock().unwrap().active());
    let decayed = session.fec_percent();
    assert!(decayed < 20, "decayed={decayed}");
    let before_bps = session.video_pacing_bps(800_000_000, 50_000_000);

    let mut send_loss = SendLossRecovery::default();
    let route_only = BatchDropDelta {
        dropped: 20,
        route_unavailable: 20,
        ..Default::default()
    };
    finish_frame_drops(
        &session,
        &mut send_loss,
        route_drop_frame(30, false, false, true, 10, route_only),
    );
    finish_frame_drops(
        &session,
        &mut send_loss,
        route_drop_frame(31, false, false, true, 10, route_only),
    );
    for frame in 32..=41u32 {
        finish_frame_drops(
            &session,
            &mut send_loss,
            route_drop_frame(frame, false, false, false, 20, route_only),
        );
    }

    // Delayed 0x5502 Unrecoverable reports for route-abandoned frames 30..=31 must not cut
    // AdaptivePacing, trigger AdaptiveBitrate, or reset AdaptiveFec back to its ceiling.
    session.record_fec_status(&test_fec_report(30, 3, 1));
    session.record_fec_status(&test_fec_report(31, 3, 1));
    assert_eq!(
        session.tick_automatic_pacing(start + Duration::from_secs(61), Duration::from_millis(2)),
        None
    );
    assert_eq!(
        session
            .automatic_pacing
            .lock()
            .unwrap()
            .preview_rate(80_000_000, 50_000_000, 40_000),
        80_000_000
    );
    assert_eq!(
        session.video_pacing_bps(800_000_000, 50_000_000),
        before_bps
    );
    assert_eq!(
        session.tick_adaptive_bitrate(start + Duration::from_secs(61), Duration::from_millis(2)),
        None
    );
    assert!(session.adaptive_bitrate.lock().unwrap().active());
    assert_eq!(session.bitrate.load(Ordering::Acquire), 40_000);
    assert_eq!(session.fec_percent(), decayed);
}

#[test]
fn p_frame_route_outage_with_client_idr_does_not_latch_or_step_down_link_rate() {
    use butterpollo_win_io::net::BatchDropDelta;
    use std::sync::atomic::Ordering;

    let fixture = crate::state::test_support::Fixture::new();
    let session = Session::new(
        fixture.launch(fixture.client(0), Role::Stream),
        butterpollo_media_policy::rtsp::Negotiated {
            codec: 1,
            bitrate_kbps: 20_000,
            configured_bitrate_kbps: 20_000,
            ..Default::default()
        },
    );
    session.configure_keyframe_loss(true);
    session.keyframe_loss.lock().unwrap().sending_at(
        1,
        true,
        Instant::now()
            - butterpollo_media_policy::keyframe_loss::STARTUP_WINDOW
            - Duration::from_millis(50),
    );
    session.configure_link_rate(true, 0);
    session.record_link_sent(1, 100, 112_600);
    std::thread::sleep(Duration::from_millis(20));
    for frame in 2..=10u32 {
        session.record_link_sent(frame, 100, 112_600);
    }
    session.stats.video_frame.store(100, Ordering::Release);
    session.idr.store(false, Ordering::Release);

    let period = Duration::from_micros(16_667);
    let mut send_loss = SendLossRecovery::default();
    let route_only = BatchDropDelta {
        dropped: 20,
        route_unavailable: 20,
        ..Default::default()
    };

    // Route outage hits only ordinary P-frames 11..=26 (batch 0 of 11 leaves the socket).
    for frame in 11..=26u32 {
        session.keyframe_loss_bps(800_000_000, 108_000_000, frame, false);
        on_block_unrecoverable(
            &session,
            &mut send_loss,
            Instant::now(),
            period,
            frame == 11,
            false,
            route_only,
        );
        finish_frame_drops(
            &session,
            &mut send_loss,
            route_drop_frame(
                frame,
                false,
                false,
                frame == 11,
                if frame == 11 { 10 } else { 20 },
                route_only,
            ),
        );
    }

    // Three client 0x0302 requests arrive during the outage and set `session.idr = true`.
    for _ in 0..3 {
        session.request_client_keyframe();
    }
    assert!(session.idr.load(Ordering::Acquire));
    assert_eq!(
        session.tick_link_rate(
            Instant::now() + Duration::from_millis(50),
            Duration::from_millis(10)
        ),
        None
    );
    assert_eq!(session.link_rate_snapshot()["state"], "full");
    assert_eq!(session.link_rate_snapshot()["steps_down"], 0);

    // Post-outage frame consumes `session.idr` without `request_host_keyframe()`, so `sending_at`
    // takes the unresolved route-outage flag and opens HOST_CHANGE_WINDOW.
    let idr = session.idr.swap(false, Ordering::AcqRel);
    assert!(idr);
    session.keyframe_loss_bps(800_000_000, 108_000_000, 27, idr);
    session.record_link_sent(27, 100, 112_600);
    finish_frame_drops(
        &session,
        &mut send_loss,
        route_drop_frame(27, idr, false, true, 0, BatchDropDelta::default()),
    );
    session.keyframe_loss_bps(800_000_000, 108_000_000, 28, false);

    // Immediate follow-up client 0x0302 and delayed FEC report for P-frame 11 must stay ignored.
    session.request_client_keyframe();
    session.record_fec_status(&test_fec_report(11, 3, 1));
    assert!(!session.keyframe_loss_active());
    assert!(!session.keyframe_loss_ever_latched());
    assert_eq!(
        session.tick_link_rate(
            Instant::now() + Duration::from_millis(60),
            Duration::from_millis(10)
        ),
        None
    );
    assert_eq!(session.link_rate_snapshot()["state"], "full");
    assert_eq!(session.link_rate_snapshot()["steps_down"], 0);
}

#[test]
fn mixed_recoverable_refusal_and_route_unavailable_idr_stays_route_abandoned() {
    use butterpollo_media_policy::stream_policy::SendLoss;
    use butterpollo_win_io::net::BatchDropDelta;
    use std::sync::atomic::Ordering;

    let fixture = crate::state::test_support::Fixture::new();
    let session = Session::new(
        fixture.launch(fixture.client(0), Role::Stream),
        butterpollo_media_policy::rtsp::Negotiated {
            codec: 1,
            bitrate_kbps: 20_000,
            configured_bitrate_kbps: 20_000,
            ..Default::default()
        },
    );
    session.configure_keyframe_loss(true);
    session.keyframe_loss.lock().unwrap().sending_at(
        1,
        true,
        Instant::now()
            - butterpollo_media_policy::keyframe_loss::STARTUP_WINDOW
            - Duration::from_millis(50),
    );
    let period = Duration::from_micros(16_667);
    let mut send_loss = SendLossRecovery::default();

    // Batch 0 of IDR frame 2 suffers 1 recoverable refused packet (within FEC parity 2), then
    // Batch 1 fails with WSAENETUNREACH (route_unavailable).
    session.keyframe_loss_bps(800_000_000, 108_000_000, 2, true);
    let mut lost = SendLoss::new(vec![(10, 2), (10, 2)]);
    let batch0 = BatchDropDelta {
        dropped: 1,
        refused: 1,
        ..Default::default()
    };
    assert!(!lost.sent_with_route(
        12,
        batch0.dropped as usize,
        batch0.route_unavailable as usize
    ));
    let batch1 = BatchDropDelta {
        dropped: 10,
        route_unavailable: 10,
        ..Default::default()
    };
    let non_route = lost.sent_with_route(
        12,
        batch1.dropped as usize,
        batch1.route_unavailable as usize,
    );
    assert!(!non_route);

    on_block_unrecoverable(
        &session,
        &mut send_loss,
        Instant::now(),
        period,
        lost.reached_socket(),
        non_route,
        batch1,
    );
    let frame_drops = BatchDropDelta {
        dropped: 11,
        refused: 1,
        route_unavailable: 10,
        ..Default::default()
    };
    finish_frame_drops(
        &session,
        &mut send_loss,
        FrameDrop {
            wire_frame: 2,
            idr: true,
            non_route_unrecoverable: lost.unrecoverable(),
            reached_socket: lost.reached_socket(),
            abandoned: 0,
            drops: frame_drops,
        },
    );

    assert!(!session.keyframe_loss_active());
    assert!(!session.keyframe_loss_ever_latched());
    assert!(session.keyframe_loss.lock().unwrap().is_route_abandoned(2));
    assert!(
        !session
            .keyframe_loss
            .lock()
            .unwrap()
            .network_request(Instant::now())
    );
    assert_eq!(
        session.stats.send_loss_recoveries.load(Ordering::Relaxed),
        0
    );
    let warnings = serde_json::to_string(&session.launch.warnings.snapshot()).unwrap();
    assert!(
        warnings.contains(ROUTE_UNAVAILABLE_WARNING),
        "warnings={warnings}"
    );
    assert!(
        !warnings.contains(SEND_CONGESTION_WARNING),
        "warnings={warnings}"
    );
}

#[test]
fn incomplete_streamed_frame_uses_route_unavailable_classification() {
    use butterpollo_win_io::net::BatchDropDelta;
    use std::sync::atomic::Ordering;

    let fixture = crate::state::test_support::Fixture::new();
    let session = Session::new(
        fixture.launch(fixture.client(0), Role::Stream),
        butterpollo_media_policy::rtsp::Negotiated {
            codec: 1,
            bitrate_kbps: 20_000,
            configured_bitrate_kbps: 20_000,
            ..Default::default()
        },
    );
    session.configure_keyframe_loss(true);
    session.keyframe_loss.lock().unwrap().sending_at(
        1,
        true,
        Instant::now()
            - butterpollo_media_policy::keyframe_loss::STARTUP_WINDOW
            - Duration::from_millis(50),
    );
    let mut send_loss = SendLossRecovery::default();

    // Case A: route-only drop on an interrupted/failed streamed frame keeps host send-loss clear
    // and arms route-recovery keyframe emission for the next delivered P-frame.
    session.keyframe_loss_bps(800_000_000, 108_000_000, 2, false);
    let route_drops = BatchDropDelta {
        dropped: 6,
        route_unavailable: 6,
        ..Default::default()
    };
    abandon_incomplete_streamed(&session, &mut send_loss, 2, false, false, route_drops);
    assert_eq!(session.stats.frames_lost_on_send.load(Ordering::Relaxed), 0);
    assert!(!session.keyframe_loss_active());
    assert!(!session.keyframe_loss_ever_latched());
    assert!(session.keyframe_loss.lock().unwrap().is_route_abandoned(2));
    assert!(
        !session
            .keyframe_loss
            .lock()
            .unwrap()
            .network_request(Instant::now())
    );
    assert!(send_loss.delivered(false));

    // Case B: non-route incomplete streamed drop counts as a host send loss and does not arm route recovery.
    let mut non_route_send_loss = SendLossRecovery::default();
    session.keyframe_loss_bps(800_000_000, 108_000_000, 3, false);
    abandon_incomplete_streamed(
        &session,
        &mut non_route_send_loss,
        3,
        false,
        true,
        BatchDropDelta::default(),
    );
    assert_eq!(session.stats.frames_lost_on_send.load(Ordering::Relaxed), 1);
    assert!(!non_route_send_loss.delivered(false));
}
