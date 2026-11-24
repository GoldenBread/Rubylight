//! Tests for automatic packet pacing, wireless startup seeds, and client bitrate handoff.
use super::*;

#[test]
fn pyrowave_loss_controls_adapt_without_forcing_extra_frames() {
    for records in [false, true] {
        let s = Session::new(
            launch("pyrowave-loss", Role::Stream),
            Negotiated {
                codec: 3,
                pyrowave_records: records,
                bitrate_kbps: 200_000,
                configured_bitrate_kbps: 250_000,
                pyrowave_client_pace_mbps: 150,
                ..Default::default()
            },
        );
        s.idr.store(false, Ordering::Release);
        s.configure_keyframe_loss(true);
        s.configure_adaptive_bitrate(true, 0);
        let start = Instant::now();
        s.tick_adaptive_bitrate(start, Duration::from_millis(5));
        s.keyframe_loss_bps(1_000_000_000, 250_000_000, 1, true);
        s.stats.video_frame.store(2, Ordering::Release);
        s.record_fec_status(&fec_report(1, 7, 1));
        assert_eq!(
            s.keyframe_loss_bps(1_000_000_000, 250_000_000, 2, true),
            250_000_000
        );
        let (_, target) = s
            .tick_adaptive_bitrate(start + Duration::from_secs(1), Duration::from_millis(5))
            .unwrap();
        assert!(target < 150_000);
        assert!(s.adaptive_pacing_bps(u64::MAX) <= 150_000_000);
        assert!(!s.idr.load(Ordering::Acquire));
        assert!(s.invalidation.lock().unwrap().is_none());
        s.set_client_bitrate(180_000);
        s.record_fec_status(&fec_report(2, 7, 1));
        s.tick_adaptive_bitrate(start + Duration::from_secs(3), Duration::from_millis(5));
        assert_eq!(s.bitrate.load(Ordering::Acquire), 180_000);
    }
}

#[test]
fn pyrowave_packet_adaptation_responds_to_distinct_lost_frames() {
    let s = Session::new(
        launch("pyrowave-pacing", Role::Stream),
        Negotiated {
            codec: 3,
            bitrate_kbps: 40_000,
            ..Default::default()
        },
    );
    s.configure_automatic_pacing(true);
    let start = Instant::now();
    assert_eq!(s.video_pacing_bps(150_000_000, 50_000_000), 80_000_000);
    s.tick_automatic_pacing(start, Duration::from_millis(2));
    pacing_traffic(&s, 60);
    s.record_fec_status(&fec_report(59, 7, 1));
    s.record_fec_status(&fec_report(60, 7, 1));
    assert_eq!(
        s.tick_automatic_pacing(start + Duration::from_secs(1), Duration::from_millis(2)),
        Some((80_000_000, 62_500_000))
    );
    assert_eq!(s.bitrate.load(Ordering::Acquire), 40_000);
    // A measured path below demand remains the hard ceiling.
    assert_eq!(s.video_pacing_bps(30_000_000, 50_000_000), 30_000_000);
}

#[test]
fn a_wireless_client_stream_starts_at_the_controllers_seed_and_climbs_toward_the_route_ceiling() {
    let config = butterpollo_config::config::Config::default();
    for (wireless, expected) in [(true, 126_000_000), (false, 800_000_000)] {
        let negotiated = Negotiated {
            codec: 1,
            bitrate_kbps: 63_000,
            client_wireless: wireless,
            ..Default::default()
        };
        let s = Session::new(launch("wireless-start", Role::Stream), negotiated);
        s.configure_automatic_pacing(
            config.adaptive_packet_pacing(s.config.codec, s.config.client_wireless),
        );
        // The sender passes the host route's base (800 Mbps on a wired 1 GbE
        // host) to the controller: its seed gives the conservative start.
        assert_eq!(s.automatic_pacing_enabled(), wireless);
        assert!(!s.client_wireless_sets_route_pacing());
        assert_eq!(s.video_pacing_bps(800_000_000, 75_000_000), expected);
        if !wireless {
            continue;
        }
        // Clean feedback lets the rate climb above twice the bitrate, in
        // steps of at most 12.5%, and never past the route ceiling.
        let start = Instant::now();
        s.tick_automatic_pacing(start, Duration::from_millis(2));
        let mut rate = expected;
        for step in 1..=40u64 {
            let now = start + Duration::from_millis(500 * step);
            pacing_traffic(&s, 30 * step);
            s.record_control_ping(now, &[4, 0, 0, 0, 0, 0, 0, 0]);
            s.tick_automatic_pacing(now, Duration::from_millis(2));
            let next = s.video_pacing_bps(800_000_000, 75_000_000);
            assert!(next >= rate && next <= rate + rate / 8, "{rate} -> {next}");
            assert!(next <= 800_000_000);
            rate = next;
        }
        assert!(rate > 2 * 63_000_000, "never climbed: {rate}");
        assert_eq!(rate, 4 * 63_000_000);
        // When the client reports a 1 Gbps link speed, known-link behavior stays unchanged.
        let known = Session::new(
            launch("wireless-known-link", Role::Stream),
            Negotiated {
                codec: 1,
                bitrate_kbps: 63_000,
                client_wireless: true,
                pyrowave_client_link_mbps: 1000,
                ..Default::default()
            },
        );
        known.configure_automatic_pacing(config.adaptive_packet_pacing(1, true));
        known.video_pacing_bps(800_000_000, 75_000_000);
        known.tick_automatic_pacing(start, Duration::from_millis(2));
        for step in 1..=40u64 {
            let now = start + Duration::from_millis(500 * step);
            pacing_traffic(&known, 30 * step);
            known.record_control_ping(now, &[4, 0, 0, 0, 0, 0, 0, 0]);
            known.tick_automatic_pacing(now, Duration::from_millis(2));
        }
        assert_eq!(known.video_pacing_bps(800_000_000, 75_000_000), 800_000_000);
    }
    // Without the controller the hint sets the route pacing instead.
    for (setting, codec) in [
        ("wireless_adaptive_pacing=false", 1),
        ("", 3),
        ("pacing_max_bitrate_kbps=80000", 1),
    ] {
        let config = butterpollo_config::config::Config::parse(setting).unwrap();
        let s = Session::new(
            launch("wireless-hint", Role::Stream),
            Negotiated {
                codec,
                client_wireless: true,
                ..Default::default()
            },
        );
        s.configure_automatic_pacing(config.adaptive_packet_pacing(codec, true));
        assert!(
            s.client_wireless_sets_route_pacing(),
            "{setting} codec {codec}"
        );
    }
    let wired = Session::new(launch("wired", Role::Stream), Negotiated::default());
    assert!(!wired.client_wireless_sets_route_pacing());
}
#[test]
fn automatic_pacing_uses_validated_new_losses_without_changing_stream_quality() {
    let s = Session::new(
        launch("automatic-pacing", Role::Stream),
        Negotiated {
            bitrate_kbps: 40_000,
            ..Default::default()
        },
    );
    s.configure_automatic_pacing(true);
    s.idr.store(false, Ordering::Relaxed);
    let mode = s.stream_mode();
    let start = Instant::now();
    assert_eq!(s.video_pacing_bps(800_000_000, 50_000_000), 80_000_000);
    s.tick_automatic_pacing(start, Duration::from_millis(2));
    s.record_fec_status(&fec_report(1, 7, 1)); // No sent frames yet.
    s.record_fec_status(&fec_report(2, 7, 1));
    pacing_traffic(&s, 30);
    s.record_fec_status(&fec_report(29, 7, 1)[..20]); // Bad length.
    s.record_fec_status(&fec_report(30, 11, 1)); // Impossible count.
    s.record_fec_status(&fec_report(31, 7, 1)); // Future frames.
    s.record_fec_status(&fec_report(32, 7, 1));
    s.record_fec_status(&fec_report(29, 7, 1));
    s.record_fec_status(&fec_report(29, 7, 1)); // Duplicate block.
    assert_eq!(
        s.tick_automatic_pacing(start + Duration::from_millis(500), Duration::from_millis(2)),
        None
    );
    // The invalid reports did not reserve these frame/block identities.
    pacing_traffic(&s, 60);
    s.record_fec_status(&fec_report(30, 7, 1));
    s.record_fec_status(&fec_report(31, 7, 1));
    assert_eq!(
        s.tick_automatic_pacing(start + Duration::from_secs(1), Duration::from_millis(2)),
        Some((80_000_000, 62_500_000))
    );
    pacing_traffic(&s, 90);
    // These unseen blocks are valid telemetry, but describe the old rate.
    s.record_fec_status(&fec_report(58, 7, 1));
    s.record_fec_status(&fec_report(60, 7, 1));
    assert_eq!(
        s.tick_automatic_pacing(
            start + Duration::from_millis(1500),
            Duration::from_millis(2)
        ),
        None
    );
    pacing_traffic(&s, 120);
    s.record_fec_status(&fec_report(119, 7, 1));
    s.record_fec_status(&fec_report(120, 7, 1));
    assert_eq!(
        s.tick_automatic_pacing(start + Duration::from_secs(2), Duration::from_millis(2)),
        None // Already at the live wire-demand floor.
    );
    assert_eq!(s.video_pacing_bps(800_000_000, 50_000_000), 62_500_000);
    assert_eq!(s.bitrate.load(Ordering::Acquire), 40_000);
    assert_eq!(s.stream_mode(), mode);
    assert!(!s.idr.load(Ordering::Relaxed));
    assert!(s.invalidation.lock().unwrap().is_none());
    assert_eq!(s.stats.idr_requests.load(Ordering::Relaxed), 0);
    assert_eq!(s.stats.send_loss_recoveries.load(Ordering::Relaxed), 0);
    assert!(!s.stopping());
    let performance = &s.info()["performance"];
    assert_eq!(performance["fec_invalid_reports"], 6);
    assert_eq!(performance["fec_duplicate_reports"], 1);
}
#[test]
fn automatic_pacing_respects_client_bitrate_changes_and_updated_fec_wire_demand() {
    let s = Session::new(
        launch("pacing-client-bitrate", Role::Stream),
        Negotiated {
            bitrate_kbps: 40_000,
            configured_bitrate_kbps: 60_000,
            packet_size: 1392,
            min_fec: 2,
            ..Default::default()
        },
    );
    s.configure_automatic_pacing(true);
    s.configure_adaptive_bitrate(true, 0);
    s.configure_fec(false, 20);
    s.set_client_bitrate(40_000);
    s.idr.store(false, Ordering::Relaxed);
    let mode = s.stream_mode();
    let start = Instant::now();
    let mut packetizer = butterpollo_media_policy::packet::VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
        packet_size: s.config.packet_size,
        fec_percent: s.fec_percent(),
        min_fec: s.config.min_fec,
        key: Some([0; 16]),
    };
    let needed = butterpollo_media_policy::network_pacing::video_wire_rate_bps(
        &packetizer,
        s.bitrate.load(Ordering::Acquire),
        mode.period(),
        false,
    );
    s.video_pacing_bps(800_000_000, needed);
    s.tick_automatic_pacing(start, Duration::from_millis(2));
    for interval in 1..=4 {
        let frame = interval * 30;
        pacing_traffic(&s, frame);
        s.record_fec_status(&fec_report(frame as u32 - 1, 7, 1));
        s.record_fec_status(&fec_report(frame as u32, 7, 1));
        let now = start + Duration::from_millis(interval * 500);
        let change = s.tick_automatic_pacing(now, Duration::from_millis(2));
        // Startup is already close to wire demand, so the first cut
        // reaches the floor; repeated loss must not push below it.
        assert_eq!(change.is_some(), interval == 1);
        assert_eq!(s.video_pacing_bps(800_000_000, needed), needed + needed / 4);
        assert_eq!(s.tick_adaptive_bitrate(now, Duration::from_millis(2)), None);
        assert_eq!(s.bitrate.load(Ordering::Acquire), 40_000);
    }
    let previous = s.video_pacing_bps(800_000_000, needed);
    assert_eq!(previous, needed + needed / 4);
    // The real /bitrate writer retains ownership. The sender supplies a
    // refreshed wire budget when bitrate or FEC changes between frames.
    s.set_client_bitrate(80_000);
    packetizer.fec_percent = s.configure_fec(false, 50);
    let needed = butterpollo_media_policy::network_pacing::video_wire_rate_bps(
        &packetizer,
        s.bitrate.load(Ordering::Acquire),
        mode.period(),
        false,
    );
    let raised = s.video_pacing_bps(800_000_000, needed);
    assert!(raised > previous);
    assert_eq!(raised, needed + needed / 4);
    pacing_traffic(&s, 150);
    s.record_fec_status(&fec_report(149, 7, 1));
    s.record_fec_status(&fec_report(150, 7, 1));
    let now = start + Duration::from_millis(2500);
    assert_eq!(
        s.tick_automatic_pacing(now, Duration::from_millis(200)),
        None
    );
    assert_eq!(
        s.tick_adaptive_bitrate(now, Duration::from_millis(200)),
        None
    );
    assert_eq!(s.bitrate.load(Ordering::Acquire), 80_000);
    assert_eq!(s.info()["adaptive_bitrate"]["owner"], "client");
    assert_eq!(s.stream_mode(), mode);
    assert!(!s.idr.load(Ordering::Relaxed));
}
#[test]
fn disabled_automatic_pacing_preserves_the_base_rate() {
    for (enabled, codec) in [(false, 0), (false, 1), (false, 3)] {
        let s = Session::new(
            launch("pacing-bypass", Role::Stream),
            Negotiated {
                codec,
                ..Default::default()
            },
        );
        s.configure_automatic_pacing(enabled);
        assert_eq!(s.video_pacing_bps(950_000_000, 50_000_000), 950_000_000);
        let start = Instant::now();
        s.tick_automatic_pacing(start, Duration::from_millis(2));
        pacing_traffic(&s, 30);
        s.record_fec_status(&fec_report(29, 7, 1));
        s.record_fec_status(&fec_report(30, 7, 1));
        assert_eq!(
            s.tick_automatic_pacing(start + Duration::from_secs(1), Duration::from_millis(2)),
            None
        );
        assert_eq!(s.video_pacing_bps(950_000_000, 50_000_000), 950_000_000);
        let info = s.info();
        assert_eq!(info["packet_pacing"]["automatic"], false);
        assert_eq!(info["performance"]["fec_unrecoverable_frames"], 2);
    }
}
#[test]
fn automatic_pacing_guards_first_frame_and_each_reconnect() {
    let start = Instant::now();
    let launch = launch("pacing-reconnect", Role::Stream);
    let negotiated = Negotiated {
        bitrate_kbps: 40_000,
        ..Default::default()
    };
    let first = Session::new(launch.clone(), negotiated.clone());
    first.configure_automatic_pacing(true);
    assert_eq!(first.stats.frames.load(Ordering::Relaxed), 0);
    assert_eq!(first.video_pacing_bps(800_000_000, 50_000_000), 80_000_000);
    first.tick_automatic_pacing(start, Duration::from_millis(2));
    let ping = [4, 0, 0, 0, 0, 0, 0, 0];
    for interval in 1..=30 {
        let now = start + Duration::from_millis(interval * 500);
        pacing_traffic(&first, interval * 30);
        first.record_control_ping(now, &ping);
        first.tick_automatic_pacing(now, Duration::from_millis(2));
        first.video_pacing_bps(800_000_000, 50_000_000);
    }
    assert_eq!(first.video_pacing_bps(800_000_000, 50_000_000), 800_000_000);
    assert_eq!(first.bitrate.load(Ordering::Acquire), 40_000);
    // A client can change its downstream connection without changing
    // its identity or host route. Never reuse an old fast rate at startup.
    let reconnect = Session::new(launch, negotiated);
    reconnect.configure_automatic_pacing(true);
    assert_eq!(reconnect.stats.frames.load(Ordering::Relaxed), 0);
    assert_eq!(
        reconnect.video_pacing_bps(800_000_000, 50_000_000),
        80_000_000
    );
    assert_eq!(first.video_pacing_bps(800_000_000, 50_000_000), 800_000_000);
    assert_eq!(reconnect.bitrate.load(Ordering::Acquire), 40_000);
}
