//! Tests for client FEC telemetry, statistical FEC, budget splitting, and adaptive FEC.
use super::*;

#[test]
fn stream_doctors_step_keeps_the_launch_share_so_its_undo_restores_the_encoder() {
    use butterpollo_media_policy::extensions::FecChoice;
    let mut stream = Negotiated {
        configured_bitrate_kbps: 20_000,
        audio_quality: true,
        ..Default::default()
    };
    butterpollo_media_policy::stream_policy::apply(
        &mut stream,
        60_000,
        &butterpollo_config::config::Config::default(),
    );
    let s = Session::new(launch("doctor-fec", Role::Stream), stream);
    s.configure_fec_choice(FecChoice::Fixed(20), false, 20, 80);
    let before = s.encoder_bitrate(true);
    let (live, share) = s.client_bitrate();
    assert!(
        share > 0,
        "the launch took the host's share off the user's bitrate"
    );
    let step = live * 4 / 5;
    assert!(s.set_client_bitrate_from(live, step, share));
    let after = i64::from(s.encoder_bitrate(false));
    assert!(
        (after - i64::from(before) * 4 / 5).abs() <= 1,
        "{after} is a fifth below {before}"
    );
    // A bitrate that moved meanwhile is left as it is.
    assert!(!s.set_client_bitrate_from(live, live, share));
    assert_eq!(s.client_bitrate(), (step, share));
    assert!(s.set_client_bitrate_from(step, live, share));
    assert_eq!(s.encoder_bitrate(false), before);
}

#[test]
fn client_fec_is_exposed_as_telemetry_without_changing_stream_decisions() {
    let session = Session::new(launch("fec", Role::Stream), Negotiated::default());
    session.idr.store(false, Ordering::Relaxed);
    session.stats.video_frame.store(8, Ordering::Release);
    let bitrate = session.bitrate.load(Ordering::Relaxed);
    let report = [
        0, 0, 0, 7, 0, 11, 0, 0, 0, 1, 0, 10, 0, 2, 0, 9, 0, 1, 20, 0, 1,
    ];
    session.record_fec_status(&report);
    session.record_fec_status(&report[..20]);
    let info = session.info();
    assert_eq!(info["performance"]["fec_reports"], 2);
    assert_eq!(info["performance"]["fec_recovered_frames"], 1);
    assert_eq!(info["performance"]["fec_invalid_reports"], 1);
    assert_eq!(info["performance"]["fec_loss_reports"], 0);
    assert_eq!(session.bitrate.load(Ordering::Relaxed), bitrate);
    assert!(!session.idr.load(Ordering::Relaxed));
    assert!(session.invalidation.lock().unwrap().is_none());
    assert_eq!(info["idr_requests"], 0);
    assert_eq!(info["reference_invalidations"], 0);
    assert!(!session.stopping());
}
#[test]
fn statistical_fec_reports_show_in_the_stream_timings() {
    use butterpollo_media_policy::extensions::FecChoice;
    use butterpollo_media_policy::statistical_fec::FecReport;
    let report = |seq| {
        FecReport {
            seq,
            data_shards: 600,
            interval_ms: 1000,
            ..Default::default()
        }
        .encode()
    };
    let start = Instant::now();
    // The Retroid run: B3 reports moved statistical FEC 20 → 0 while
    // `fec_reports`, which counts only 0x5502, stayed 0.
    for choice in [FecChoice::Auto, FecChoice::Host] {
        let session = Session::new(launch("fec", Role::Stream), Negotiated::default());
        session.configure_fec_choice(choice, true, 20, 50);
        let used = choice == FecChoice::Auto;
        assert_eq!(
            session.record_fec_report_at(&report(0), start).unwrap().1,
            used
        );
        session
            .record_fec_report_at(&report(1), start + Duration::from_secs(1))
            .unwrap();
        assert!(session.record_fec_report_at(&[1; 3], start).is_err());
        let performance = &session.info()["performance"];
        assert_eq!(performance["fec_loss_reports"], 2, "{choice:?}");
        assert_eq!(performance["fec_loss_refused_reports"], 1, "{choice:?}");
        assert_eq!(performance["fec_reports"], 0, "{choice:?}");
    }
}
#[test]
fn pyrowave_fec_adapts_for_records_and_legacy_clients() {
    for records in [false, true] {
        let s = Session::new(
            launch("pyrowave-fec", Role::Stream),
            Negotiated {
                codec: 3,
                pyrowave_records: records,
                ..Default::default()
            },
        );
        assert_eq!(s.configure_fec(true, 200), 200);
        s.stats.video_frame.store(3, Ordering::Release);
        s.record_fec_status(&fec_report(1, 9, 1));
        s.tick_fec(
            Instant::now()
                + butterpollo_media_policy::adaptive_fec::AdaptiveFec::CLEAN_HOLD
                + Duration::from_secs(1),
        );
        assert_eq!(s.fec_percent(), 199);
        s.record_fec_status(&fec_report(2, 7, 1));
        assert_eq!(s.fec_percent(), 200);
    }
}
#[test]
fn a_client_fec_share_splits_the_encoder_budget_at_a_fence_or_a_report() {
    use butterpollo_media_policy::extensions::FecChoice;
    use butterpollo_media_policy::statistical_fec::FecReport;
    let report = |seq, missing| {
        FecReport {
            seq,
            data_shards: 900,
            parity_shards: 180,
            missing_shards: missing,
            unrecovered_blocks: 0,
            interval_ms: 1000,
        }
        .encode()
    };
    // The host's own FEC: the encoder gets the whole bitrate, as before.
    let host = Session::new(launch("host", Role::Stream), Negotiated::default());
    host.configure_fec_choice(FecChoice::Host, true, 20, 50);
    host.bitrate.store(12_000, Ordering::Release);
    assert_eq!(host.encoder_bitrate(true), 12_000);

    // 12 Mbps at 60 fps: about 25 data shards a frame.
    let stream = Negotiated {
        packet_size: 1024,
        fps: 60,
        ..Default::default()
    };
    let auto = Session::new(launch("auto", Role::Stream), stream);
    assert_eq!(auto.configure_fec_choice(FecChoice::Auto, true, 20, 40), 20);
    let start = Instant::now();
    auto.bitrate.store(12_000, Ordering::Release);
    assert_eq!(auto.encoder_bitrate(false), 10_000, "12 Mbps at 20 % FEC");
    auto.record_fec_report_at(&report(0, 0), start).unwrap();
    assert_eq!(
        auto.fec_percent(),
        usize::from(butterpollo_media_policy::statistical_fec::MINIMUM_PERCENT)
    );
    assert_eq!(
        auto.encoder_bitrate(false),
        10_909,
        "the first report replaces the fallback share without an IDR: 12 Mbps × 100 / 110"
    );
    assert_eq!(
        auto.encoder_bitrate(true),
        10_909,
        "a clean link: video and the floor's parity"
    );
    auto.record_fec_report_at(&report(1, 540), start + Duration::from_secs(1))
        .unwrap();
    assert_eq!(auto.fec_percent(), 40);
    assert_eq!(
        auto.encoder_bitrate(false),
        8_571,
        "rising loss moves the split at once: 12 Mbps × 100 / 140"
    );
    let within = |s: &Session| {
        u64::from(s.encoder_bitrate(false)) * (100 + s.fec_percent() as u64) / 100 <= 12_000
    };
    assert!(within(&auto), "video and parity fit the budget");
    assert_eq!(auto.encoder_bitrate(true), 8_571);
    // A small fall keeps the split until a fence, a larger one moves it.
    auto.fec_percent.store(36, Ordering::Release);
    auto.follow_fec_report(false);
    assert_eq!(auto.encoder_bitrate(false), 8_571, "within the hysteresis");
    auto.fec_percent.store(30, Ordering::Release);
    auto.follow_fec_report(false);
    assert_eq!(auto.encoder_bitrate(false), 9_230, "12 Mbps × 100 / 130");
    auto.fec_percent.store(31, Ordering::Release);
    auto.follow_fec_report(false);
    assert_eq!(auto.encoder_bitrate(false), 9_160, "any rise moves it");
    assert!(within(&auto));
    // A new budget (a client /bitrate) applies at once, at the latched share.
    auto.bitrate.store(7_000, Ordering::Release);
    assert_eq!(auto.encoder_bitrate(false), 5_343);
    // A host-FEC session never follows reports.
    host.follow_fec_report(true);
    assert_eq!(host.encoder_bitrate(false), 12_000);

    let fixed = Session::new(launch("fixed", Role::Stream), Negotiated::default());
    assert_eq!(
        fixed.configure_fec_choice(FecChoice::Fixed(90), true, 20, 50),
        50
    );
    fixed.bitrate.store(12_000, Ordering::Release);
    assert_eq!(fixed.encoder_bitrate(true), 8_000);
    // Back to the host's FEC on a new configuration: no split.
    fixed.configure_fec_choice(FecChoice::Host, true, 20, 50);
    assert_eq!(fixed.encoder_bitrate(true), 12_000);
    assert_eq!(fec_split_kbps(0, 50), 0);
    assert_eq!(fec_split_kbps(1, 255), 1);
}
/// A client that sends the bitrate its user set launches with the host's FEC share already
/// taken off it (20 Mbps: 14988 Kbps). Its own FEC share then came off that again: the
/// host's own 20 % chosen by the client gave the picture 12490 Kbps. The share comes off
/// once: the client's share takes the place of the host's, the picture never gets less than
/// with the host's own FEC unless the client asks for more parity than the host's share
/// leaves room for, and never more than the stream's bitrate, which pacing and the bitrate
/// controllers take as the encoder's most.
#[test]
fn a_client_fec_share_comes_off_the_bitrate_its_user_set_once() {
    use butterpollo_media_policy::extensions::FecChoice;
    let launched = |configured: u32| {
        let mut stream = Negotiated {
            configured_bitrate_kbps: configured,
            audio_quality: true,
            ..Default::default()
        };
        butterpollo_media_policy::stream_policy::apply(
            &mut stream,
            60_000,
            &butterpollo_config::config::Config::default(),
        );
        stream
    };
    let session = |percent: u8| {
        let s = Session::new(launch("client-fec", Role::Stream), launched(20_000));
        assert_eq!(
            s.configure_fec_choice(FecChoice::Fixed(percent), false, 20, 80),
            usize::from(percent)
        );
        s
    };
    assert_eq!(launched(20_000).bitrate_kbps, 14_988);
    let host = Session::new(launch("host-fec", Role::Stream), launched(20_000));
    host.configure_fec_choice(FecChoice::Host, false, 20, 80);
    assert_eq!(host.encoder_bitrate(true), 14_988);
    for (percent, encoder) in [
        (0, 14_988),
        (10, 14_988),
        (20, 14_988),
        (25, 14_988),
        (30, 14_411),
        (50, 12_490),
        (80, 10_408),
    ] {
        let s = session(percent);
        assert_eq!(s.encoder_bitrate(true), encoder, "{percent} %");
        assert_eq!(s.encoder_bitrate(false), encoder, "{percent} %");
        assert!(
            u64::from(encoder) * (100 + u64::from(percent)) / 100 + 512 + 500 <= 20_000,
            "{percent} %: video, parity, audio and overhead fit the bitrate the user set"
        );
        assert!(encoder <= s.bitrate.load(Ordering::Acquire));
    }
    // A bitrate asked for mid-stream in the launch's unit (0x553C) carries the same share.
    let s = session(50);
    let cut = butterpollo_media_policy::stream_policy::runtime_encoder_kbps(
        &s.config,
        18_000,
        &butterpollo_config::config::Config::default(),
    );
    assert_eq!(cut, 13_388);
    s.set_client_bitrate(cut);
    assert_eq!(
        s.encoder_bitrate(false),
        11_156,
        "13388 × 100 / 80 × 100 / 150"
    );
    // The HTTP route's bitrate is the encoder's own: nothing was taken off it.
    s.set_client_encoder_bitrate(12_000);
    assert_eq!(s.encoder_bitrate(false), 8_000);
    s.set_client_bitrate(cut);
    assert_eq!(s.encoder_bitrate(false), 11_156);
    // Nor is one inside such a bitrate set before the stream's FEC was chosen.
    let early = Session::new(launch("early-fec", Role::Stream), launched(20_000));
    early.set_client_encoder_bitrate(12_000);
    early.configure_fec_choice(FecChoice::Fixed(20), false, 20, 80);
    assert_eq!(early.encoder_bitrate(true), 10_000);
    // While host bitrate adaptation budgets FEC itself, the encoder gets the bitrate as it
    // is; the share is latched again when it stops.
    s.fec_split.store(FEC_SPLIT_NONE, Ordering::Release);
    assert_eq!(s.encoder_bitrate(false), cut);
    s.follow_fec_report(false);
    assert_eq!(s.encoder_bitrate(false), 11_156);
    // A launch without the user's bitrate took no share off: the split is as it was.
    let stock = Session::new(launch("stock-fec", Role::Stream), Negotiated::default());
    stock.configure_fec_choice(FecChoice::Fixed(50), false, 20, 80);
    stock.bitrate.store(12_000, Ordering::Release);
    assert_eq!(stock.encoder_bitrate(true), 8_000);
    // PyroWave keeps its own protection and bitrate.
    let mut wave = launched(100_000);
    wave.codec = 3;
    let wave = Session::new(launch("wave-fec", Role::Stream), wave);
    wave.configure_fec_choice(FecChoice::Fixed(50), false, 20, 80);
    assert_eq!(
        wave.encoder_bitrate(true),
        wave.bitrate.load(Ordering::Acquire)
    );
}
/// The link-rate and congestion controllers size a cut as the encoder's own bitrate, the FEC
/// share in use counted as parity (`WireBudget::encoder_kbps`). Split for the client's share
/// again, a cut for 50 % parity gave the encoder five sixths of it.
#[test]
fn a_controllers_cut_is_not_split_for_the_clients_fec_share_again() {
    use butterpollo_media_policy::extensions::FecChoice;
    let session = |name: &str| {
        let mut stream = Negotiated {
            configured_bitrate_kbps: 20_000,
            audio_quality: true,
            ..Default::default()
        };
        butterpollo_media_policy::stream_policy::apply(
            &mut stream,
            60_000,
            &butterpollo_config::config::Config::default(),
        );
        let s = Session::new(launch(name, Role::Stream), stream);
        s.configure_fec_choice(FecChoice::Fixed(50), false, 20, 80);
        assert_eq!(s.bitrate.load(Ordering::Acquire), 14_988);
        assert_eq!(s.encoder_bitrate(true), 12_490);
        s
    };
    let lossy = |s: &Session| {
        let end = Instant::now();
        for frame in 1..=60u32 {
            let at = end - Duration::from_millis(17 * u64::from(60 - frame));
            s.record_link_sent_at(frame, 100, 112_600, at);
        }
        s.stats.video_frame.store(70, Ordering::Release);
        lose_frames(s, 50..=54);
        s.tick_link_rate(end + Duration::from_millis(200), Duration::from_millis(10))
            .expect("the controller lowers the bitrate")
    };

    // The link-rate controller on its own.
    let s = session("cut-link");
    s.configure_link_rate(true, 0);
    let (from, cut) = lossy(&s);
    assert_eq!(from, 14_988);
    assert!(
        cut < 12_490,
        "{cut}: a cut below the encoder's share of 14988"
    );
    assert_eq!(s.encoder_bitrate(false), cut, "the cut is the encoder's");
    assert_eq!(s.encoder_bitrate(true), cut, "at a fence too");
    // A stream whose mode changed starts from the bitrate from before the cut, split once.
    s.rebuild_link_rate(&s.config, true, 0);
    assert_eq!(s.bitrate.load(Ordering::Acquire), 14_988);
    assert_eq!(s.encoder_bitrate(false), 12_490);

    // A bitrate the client sets ends the cut: it is in the launch's unit, split as one.
    let s = session("cut-client");
    s.configure_link_rate(true, 0);
    let (_, cut) = lossy(&s);
    assert_eq!(s.encoder_bitrate(false), cut);
    s.set_client_bitrate(13_388);
    assert_eq!(s.encoder_bitrate(false), 11_156);

    // Through congestion control, which writes the lower of both controllers' limits.
    let s = session("cut-congestion");
    s.configure_link_rate(true, 0);
    s.configure_congestion(true, 0, false);
    let now = Instant::now();
    let tick = |wanted: Option<u32>| {
        s.congestion.lock().unwrap().link_wanted = wanted;
        s.tick_congestion(now, Duration::from_millis(10));
        (s.bitrate.load(Ordering::Acquire), s.encoder_bitrate(false))
    };
    assert_eq!(tick(Some(9_000)), (9_000, 9_000));
    assert_eq!(tick(Some(6_000)), (6_000, 6_000));
    // A cut above the encoder's share of the bitrate from before it raises nothing: at
    // 14000 with 50 % parity the stream would send more than at its full bitrate.
    assert_eq!(tick(Some(14_000)), (14_000, 12_490));
    assert_eq!(tick(None), (14_988, 12_490));
    // A rebuilt controller gives the bitrate from before its limit back, split once.
    assert_eq!(tick(Some(9_000)), (9_000, 9_000));
    s.rebuild_congestion(&s.config, true, 0, false);
    assert_eq!(s.bitrate.load(Ordering::Acquire), 14_988);
    assert_eq!(s.encoder_bitrate(false), 12_490);
    // So does the link-rate controller's rebuild, which takes congestion control's limit too.
    assert_eq!(tick(Some(9_000)), (9_000, 9_000));
    s.rebuild_link_rate(&s.config, true, 0);
    assert_eq!(s.bitrate.load(Ordering::Acquire), 14_988);
    assert_eq!(s.encoder_bitrate(false), 12_490);

    // The host's own share was never split: a cut is the encoder's bitrate there as before.
    let mut stream = Negotiated {
        configured_bitrate_kbps: 20_000,
        audio_quality: true,
        ..Default::default()
    };
    butterpollo_media_policy::stream_policy::apply(
        &mut stream,
        60_000,
        &butterpollo_config::config::Config::default(),
    );
    let host = Session::new(launch("cut-host", Role::Stream), stream);
    host.configure_fec_choice(FecChoice::Host, false, 20, 80);
    host.configure_link_rate(true, 0);
    let (_, cut) = lossy(&host);
    assert_eq!(host.encoder_bitrate(false), cut);
}
#[test]
fn auto_fec_keeps_the_configured_share_for_recovery_frames() {
    use butterpollo_media_policy::extensions::FecChoice;
    use butterpollo_media_policy::statistical_fec::FecReport;
    let report = |seq, missing| {
        FecReport {
            seq,
            data_shards: 900,
            parity_shards: 180,
            missing_shards: missing,
            unrecovered_blocks: 0,
            interval_ms: 1000,
        }
        .encode()
    };
    let stream = Negotiated {
        packet_size: 1024,
        fps: 60,
        ..Default::default()
    };
    let auto = Session::new(launch("auto-recovery", Role::Stream), stream);
    auto.configure_fec_choice(FecChoice::Auto, true, 20, 40);
    auto.bitrate.store(12_000, Ordering::Release);
    let start = Instant::now();
    auto.record_fec_report_at(&report(0, 0), start).unwrap();
    // A clean link: ordinary frames get the floor's parity, but a keyframe or
    // the frame after an invalidation keeps the configured 20 %, and the
    // recovery budget is planned with the same share.
    assert_eq!(
        auto.video_fec_percent(false, 30_000),
        usize::from(butterpollo_media_policy::statistical_fec::MINIMUM_PERCENT)
    );
    assert_eq!(auto.video_fec_percent(true, 30_000), 20);
    assert_eq!(auto.video_fec_percent(true, 3_000), 20);
    assert_eq!(auto.recovery_fec_percent(), 20);
    // Above the configured share the statistical choice wins.
    auto.record_fec_report_at(&report(1, 540), start + Duration::from_secs(1))
        .unwrap();
    assert_eq!(auto.video_fec_percent(false, 30_000), 40);
    assert_eq!(auto.video_fec_percent(true, 30_000), 40);
    assert_eq!(auto.recovery_fec_percent(), 40);

    // Without Auto the plan is the host's recovery share, as before.
    let host = Session::new(launch("host-recovery", Role::Stream), Negotiated::default());
    host.configure_fec_choice(FecChoice::Host, true, 20, 50);
    assert_eq!(host.recovery_fec_percent(), host.frame_fec_percent(true));
    let fixed = Session::new(
        launch("fixed-recovery", Role::Stream),
        Negotiated::default(),
    );
    fixed.configure_fec_choice(FecChoice::Fixed(10), true, 20, 50);
    assert_eq!(fixed.video_fec_percent(true, 30_000), 10);
    assert_eq!(fixed.recovery_fec_percent(), 10);
}
#[test]
fn auto_fec_finds_the_largest_recovery_frame_its_own_percentage_covers_in_full() {
    use butterpollo_media_policy::extensions::FecChoice;
    use butterpollo_media_policy::statistical_fec::FecReport;
    let report = |seq, missing| {
        FecReport {
            seq,
            data_shards: 900,
            parity_shards: 180,
            missing_shards: missing,
            unrecovered_blocks: 0,
            interval_ms: 1000,
        }
        .encode()
    };
    let stream = Negotiated {
        packet_size: 1392,
        fps: 60,
        ..Default::default()
    };
    let packetizer = |percent| butterpollo_media_policy::packet::VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
        packet_size: 1392,
        fec_percent: percent,
        min_fec: stream.min_fec,
        key: Some([0; 16]),
    };
    let auto = Session::new(launch("auto-whole", Role::Stream), stream.clone());
    auto.configure_fec_choice(FecChoice::Auto, true, 20, 40);
    auto.bitrate.store(200_000, Ordering::Release);
    let start = Instant::now();
    // Only a fixed choice: the packetizer's own limit applies.
    let fixed = Session::new(launch("fixed-whole", Role::Stream), stream.clone());
    fixed.configure_fec_choice(FecChoice::Fixed(20), true, 20, 40);
    assert_eq!(fixed.statistical_fec_whole_payload(start), None);
    // A clean link: recovery frames keep the configured 20 %, so the limit
    // is the fixed one.
    auto.record_fec_report_at(&report(0, 0), start).unwrap();
    let clean = auto.statistical_fec_whole_payload(start).unwrap();
    assert_eq!(clean, packetizer(20).fec_whole_payload_limit());
    // A lossy link raises an average frame's share to 40 %, but a large
    // frame gets the most parity it can carry in full, and a recovery
    // frame at least the configured 20 %: the limit is the 20 % one, not
    // the smaller 40 % limit the average frame's share would give.
    let later = start + Duration::from_secs(1);
    auto.record_fec_report_at(&report(1, 540), later).unwrap();
    assert_eq!(auto.recovery_fec_percent(), 40);
    let lossy = auto.statistical_fec_whole_payload(later).unwrap();
    assert_eq!(lossy, clean);
    assert!(lossy > packetizer(40).fec_whole_payload_limit());
    let percent = |payload| auto.video_fec_percent(true, payload);
    assert!(!packetizer(percent(lossy)).fec_limited(lossy));
    // The next shard is a frame its own percentage no longer covers.
    let next = lossy + 1376;
    assert!(packetizer(percent(next)).fec_limited(next));
}
#[test]
fn adaptive_fec_follows_reports_and_stays_fixed_without_them_or_when_off() {
    let start = Instant::now();
    // A client that never reports: the configured percentage, for minutes.
    let quiet = Session::new(launch("quiet", Role::Stream), Negotiated::default());
    assert_eq!(quiet.configure_fec(true, 30), 30);
    for second in 0..300 {
        assert_eq!(quiet.tick_fec(start + Duration::from_secs(second)), None);
    }
    assert_eq!(quiet.fec_percent(), 30);
    assert_eq!(quiet.info()["fec_percent"], 30);
    // Off: reports are still counted, the percentage never moves.
    let off = Session::new(launch("off", Role::Stream), Negotiated::default());
    assert_eq!(off.configure_fec(false, 20), 20);
    off.stats.video_frame.store(100, Ordering::Release);
    off.record_fec_status(&fec_report(10, 10, 0));
    off.record_fec_status(&fec_report(11, 7, 1));
    for second in 0..120 {
        assert_eq!(off.tick_fec(start + Duration::from_secs(second)), None);
    }
    assert_eq!(off.fec_percent(), 20);
    assert_eq!(off.info()["performance"]["fec_unrecoverable_frames"], 1);
    // On: a clean link decays, a rebuilt frame raises, a lost one restores.
    let on = Session::new(launch("on", Role::Stream), Negotiated::default());
    assert_eq!(on.configure_fec(true, 20), 20);
    on.stats.video_frame.store(100, Ordering::Release);
    on.record_fec_status(&fec_report(10, 10, 0));
    let now = Instant::now();
    assert_eq!(on.tick_fec(now), None);
    let later = now + Duration::from_secs(60);
    on.tick_fec(later);
    assert!(on.fec_percent() < 20);
    let decayed = on.fec_percent();
    on.record_fec_status(&fec_report(20, 9, 1));
    assert!(on.fec_percent() > decayed);
    on.record_fec_status(&fec_report(21, 7, 1));
    assert_eq!(on.fec_percent(), 20);
    // Recovery frames keep the configured percentage on a decayed link.
    on.tick_fec(later + Duration::from_secs(120));
    let decayed = on.fec_percent();
    assert!(decayed < 20);
    assert_eq!(on.frame_fec_percent(false), decayed);
    assert_eq!(on.frame_fec_percent(true), 20);
    assert_eq!(off.frame_fec_percent(true), 20);
    assert_eq!(quiet.frame_fec_percent(false), 30);
    // PyroWave reports its own configured and adapted protection.
    let pyrowave = Session::new(
        launch("pyrowave-fec", Role::Stream),
        Negotiated {
            codec: 3,
            ..Default::default()
        },
    );
    pyrowave.configure_fec(true, 200);
    assert_eq!(pyrowave.info()["fec_percent"], 200);
}
