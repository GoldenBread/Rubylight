//! Tests for keyframe-loss pacing guard, send-loss handling, and inter-frame recovery.
use super::*;

#[test]
fn pacing_counts_as_fast_only_when_the_applied_rate_is_above_the_wifi_rate() {
    let s = Session::new(launch("link-pace", Role::Stream), Negotiated::default());
    s.configure_keyframe_loss(true);
    assert_eq!(
        s.keyframe_loss_bps(800_000_000, 62_000_000, 1, false),
        800_000_000
    );
    assert!(s.pace_was_fast.load(Ordering::Relaxed));
    // Once slowed to the Wi-Fi rate the route's own rate is not what ran.
    s.configure_remembered_keyframe_loss(true, true);
    assert_eq!(
        s.keyframe_loss_bps(800_000_000, 62_000_000, 2, false),
        62_000_000
    );
    assert!(!s.pace_was_fast.load(Ordering::Relaxed));
}
#[test]
fn a_latch_confirmed_by_more_evidence_does_not_start_a_second_fence() {
    let t0 = Instant::now();
    let s = first_keyframe_lost(FAST, t0);
    let first = fence_at(&s).expect("the first latch is a reaction to fast pacing");
    // Three more distinct frames confirm the same latch; it is not new.
    let at = t0 + Duration::from_secs(1);
    let confirmed = {
        let mut loss = s.keyframe_loss.lock().unwrap();
        // More reports about the same keyframe are not new evidence.
        assert!(loss.unrecoverable(1, 2, FRAME, at).is_none());
        assert!(loss.abandoned(1, FRAME, at).is_none());
        for frame in 3..=4 {
            loss.sending_at(frame, false, at);
            assert!(loss.unrecoverable(frame, 4, FRAME, at).is_none());
        }
        loss.sending_at(5, false, at);
        loss.unrecoverable(5, 5, FRAME, at)
    }
    .expect("more evidence confirms the latch");
    assert!(confirmed.confirmed);
    std::thread::sleep(Duration::from_millis(5));
    s.report_keyframe_loss(Some(confirmed), None);
    assert_eq!(fence_at(&s), Some(first));
}
#[test]
fn a_latch_is_a_reaction_to_fast_pacing_only_if_fast_pacing_was_applied() {
    // The route allows 800 Mbps, but the frame went out at 50 Mbps (the
    // client's own link): the latch cannot be the whole fix.
    let s = first_keyframe_lost(50_000_000, Instant::now());
    assert!(!s.pace_was_fast.load(Ordering::Relaxed));
    assert_eq!(fence_at(&s), None);
    // The same loss after frames that really went out fast starts a fence.
    let s = first_keyframe_lost(FAST, Instant::now());
    assert!(fence_at(&s).is_some());
    // A frame paced at the Wi-Fi rate because of the latch is not fast,
    // whatever the route would have allowed.
    assert_eq!(
        s.keyframe_loss_bps_at(FAST, SAFE, 3, false, Instant::now()),
        SAFE
    );
    assert!(!s.pace_was_fast.load(Ordering::Relaxed));
}
#[test]
fn a_lifted_start_up_latch_gives_link_rate_one_fence_for_the_faster_pace() {
    for (base, fenced) in [(FAST, true), (50_000_000, false)] {
        let t0 = Instant::now();
        let s = first_keyframe_lost(base, t0);
        let held = fence_at(&s);
        assert_eq!(held.is_some(), fenced);
        let mut frame = 3;
        while s.keyframe_loss_active() {
            let at = t0 + FRAME * frame;
            s.keyframe_loss_bps_at(base, SAFE, frame, false, at);
            frame += 1;
            assert!(frame < 2_000, "the latch never lifted");
        }
        assert_eq!(s.pace_was_fast.load(Ordering::Relaxed), base > SAFE);
        if fenced {
            let lifted = fence_at(&s).unwrap();
            assert!(Some(lifted) > held, "the lift starts a fence");
        } else {
            assert_eq!(fence_at(&s), None, "the pace did not change");
        }
        assert!(s.launch.warnings.snapshot().is_empty());
    }
}
#[test]
fn host_only_loss_on_a_remembered_route_names_the_pc_not_the_path() {
    const FAST: u64 = 800_000_000;
    const SAFE: u64 = 62_000_000;
    let s = Session::new(launch("onn", Role::Stream), Negotiated::default());
    s.configure_remembered_keyframe_loss(true, true);
    for frame in 1..=20 {
        assert_eq!(s.keyframe_loss_bps(FAST, SAFE, frame, frame == 20), SAFE);
    }
    // The socket refuses keyframe 20: held for the stream, never saved,
    // and the warning points at the PC, not the network path.
    s.keyframe_abandoned(20);
    assert!(s.keyframe_loss_active());
    assert!(s.keyframe_loss_sustained_at().is_none());
    let message = s.info()["warnings"][0]["message"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        message.contains("PC could not send video packets fast enough"),
        "{message}"
    );
    assert!(message.contains("rest of the stream"), "{message}");
    assert!(!message.contains("network path"), "{message}");
}
#[test]
fn a_lost_keyframe_paces_the_rest_of_the_stream_like_wifi() {
    const FAST: u64 = 800_000_000;
    const SAFE: u64 = 62_000_000;
    let s = Session::new(launch("onn", Role::Stream), Negotiated::default());
    s.configure_keyframe_loss(true);
    assert_eq!(s.keyframe_loss_bps(FAST, SAFE, 1, true), FAST);
    for frame in 2..=8 {
        assert_eq!(s.keyframe_loss_bps(FAST, SAFE, frame, false), FAST);
    }
    s.stats.video_frame.store(9, Ordering::Release);
    // A lost ordinary frame is left to recovery frames.
    s.record_fec_status(&fec_report(5, 7, 1));
    assert_eq!(s.keyframe_loss_bps(FAST, SAFE, 9, false), FAST);
    assert!(s.info()["warnings"].as_array().unwrap().is_empty());
    // The keyframe arrived short of what FEC repairs.
    s.record_fec_status(&fec_report(1, 7, 1));
    assert_eq!(s.keyframe_loss_bps(FAST, SAFE, 10, false), SAFE);
    assert_eq!(s.keyframe_loss_bps(FAST, SAFE, 11, true), SAFE);
    assert_eq!(s.info()["warnings"][0]["code"], "network_keyframe_loss");
    // The stream's first keyframe alone: held for now, not remembered.
    assert!(s.keyframe_loss_ever_latched());
    assert!(s.keyframe_loss_observed_at().is_some());
    assert!(s.keyframe_loss_sustained_at().is_none());

    // Without reports: the client asks again for the keyframe it was sent.
    let s = Session::new(launch("old", Role::Stream), Negotiated::default());
    s.configure_keyframe_loss(true);
    s.keyframe_loss_bps(FAST, SAFE, 1, true);
    s.keyframe_loss.lock().unwrap().settle();
    s.request_recovery_keyframe();
    assert_eq!(s.keyframe_loss_bps(FAST, SAFE, 2, true), FAST);
    s.request_recovery_keyframe();
    assert_eq!(s.keyframe_loss_bps(FAST, SAFE, 3, true), SAFE);
    assert_eq!(s.stats.idr_requests.load(Ordering::Relaxed), 2);

    // Explicitly disabled policies preserve the base rate for every codec.
    for (enabled, codec) in [(false, 1), (false, 3)] {
        let s = Session::new(
            launch("off", Role::Stream),
            Negotiated {
                codec,
                ..Default::default()
            },
        );
        s.configure_keyframe_loss(enabled);
        s.keyframe_loss_bps(FAST, SAFE, 1, true);
        s.stats.video_frame.store(2, Ordering::Release);
        s.record_fec_status(&fec_report(1, 0, 0));
        s.request_recovery_keyframe();
        s.keyframe_loss_bps(FAST, SAFE, 2, true);
        s.request_recovery_keyframe();
        assert_eq!(s.keyframe_loss_bps(FAST, SAFE, 3, false), FAST);
    }
}
#[test]
fn repeated_distinct_unrecoverable_frames_reduce_pacing_without_changing_bitrate() {
    const FAST: u64 = 800_000_000;
    const SAFE: u64 = 62_000_000;
    let s = Session::new(launch("loss", Role::Stream), Negotiated::default());
    s.configure_keyframe_loss(true);
    s.stats.video_frame.store(7, Ordering::Release);
    let bitrate = s.bitrate.load(Ordering::Relaxed);
    s.record_fec_status(&fec_report(1, 10, 0)); // Clean.
    s.record_fec_status(&fec_report(2, 9, 1)); // Recovered.
    // Interleaved failed blocks from two frames are still only two losses.
    for (frame, block) in [(4, 0), (5, 0), (4, 1), (5, 1), (4, 2), (5, 2)] {
        let mut report = fec_report(frame, 7, 1);
        report[19] = block;
        report[20] = 4;
        s.record_fec_status(&report);
        s.record_fec_status(&report); // Duplicate block.
        assert!(!s.keyframe_loss_active());
        assert_eq!(s.keyframe_loss_rate_bps(FAST, SAFE), FAST);
    }
    s.record_fec_status(&fec_report(6, 7, 1));
    assert!(s.keyframe_loss_active());
    assert_eq!(s.keyframe_loss_rate_bps(FAST, SAFE), SAFE);
    assert_eq!(s.bitrate.load(Ordering::Relaxed), bitrate);
    assert_eq!(s.stats.idr_requests.load(Ordering::Relaxed), 0);
    assert!(
        s.info()["warnings"][0]["message"]
            .as_str()
            .unwrap()
            .contains("three recent")
    );
}
#[test]
fn repeated_loss_guard_preserves_disabled_manual_and_adaptive_owners() {
    for text in [
        "keyframe_loss_pacing=false",
        "pacing_max_bitrate_kbps=80000",
        "host_adaptive_pacing=true",
    ] {
        let config = butterpollo_config::config::Config::parse(text).unwrap();
        let s = Session::new(launch("owner", Role::Stream), Negotiated::default());
        s.configure_keyframe_loss(config.keyframe_loss_pacing(0, false));
        s.stats.video_frame.store(4, Ordering::Release);
        for frame in 1..=3 {
            s.record_fec_status(&fec_report(frame, 7, 1));
        }
        assert!(!s.keyframe_loss_active(), "{text}");
        assert!(
            s.info()["warnings"].as_array().unwrap().is_empty(),
            "{text}"
        );
    }
}
#[test]
fn a_fresh_probe_or_disabled_policy_clears_the_previous_launch_warning() {
    let s = Session::new(launch("remembered", Role::Stream), Negotiated::default());
    for enabled in [true, false] {
        s.configure_remembered_keyframe_loss(true, true);
        assert_eq!(
            s.keyframe_loss_reason(),
            Some("remembered from the previous session")
        );
        assert!(!s.info()["warnings"].as_array().unwrap().is_empty());
        s.configure_remembered_keyframe_loss(enabled, false);
        assert!(!s.keyframe_loss_active());
        assert!(s.info()["warnings"].as_array().unwrap().is_empty());
    }
    s.configure_remembered_keyframe_loss(false, true);
    assert!(!s.keyframe_loss_active());
}
#[test]
fn sender_pacing_queries_do_not_add_sent_frames_to_keyframe_history() {
    const FAST: u64 = 800_000_000;
    const SAFE: u64 = 62_000_000;
    let s = Session::new(launch("query", Role::Stream), Negotiated::default());
    s.configure_keyframe_loss(true);
    s.keyframe_loss_bps(FAST, SAFE, 1, true);
    s.keyframe_loss.lock().unwrap().settle();
    s.request_recovery_keyframe();
    s.keyframe_loss_bps(FAST, SAFE, 2, true);
    for _ in 0..butterpollo_media_policy::keyframe_loss::DECODE_QUEUE + 1 {
        assert!(!s.keyframe_loss_active());
        assert_eq!(s.keyframe_loss_rate_bps(FAST, SAFE), FAST);
    }
    // No ordinary frame followed that keyframe: repeated queries cannot
    // turn a lost-keyframe request into an apparent decode queue overflow.
    s.request_recovery_keyframe();
    assert!(s.keyframe_loss_active());
    assert_eq!(s.keyframe_loss_rate_bps(FAST, SAFE), SAFE);
}
#[test]
fn the_hosts_own_keyframe_and_the_clients_reaction_to_it_do_not_slow_the_stream() {
    // Alt+Tab, a bitrate change or an encoder restart: the host sends a
    // keyframe nobody asked for, the client resets and asks for one,
    // twice. No packet was lost (the issue behind Wi-Fi pacing for 24 h).
    const FAST: u64 = 800_000_000;
    const SAFE: u64 = 62_000_000;
    let s = steady_stream("alt-tab");
    s.keyframe_loss_bps(FAST, SAFE, 41, true);
    s.request_recovery_keyframe();
    s.keyframe_loss_bps(FAST, SAFE, 42, true);
    s.request_recovery_keyframe();
    for frame in 43..=45 {
        s.keyframe_loss_bps(FAST, SAFE, frame, false);
    }
    s.stats.video_frame.store(46, Ordering::Release);
    for frame in 43..=45 {
        s.record_fec_status(&fec_report(frame, 7, 1));
    }
    assert!(!s.keyframe_loss_active());
    assert!(s.keyframe_loss_observed_at().is_none());
    assert_eq!(s.keyframe_loss_rate_bps(FAST, SAFE), FAST);
    assert!(s.info()["warnings"].as_array().unwrap().is_empty());
    // The keyframe itself arriving incomplete is still network loss.
    s.record_fec_status(&fec_report(41, 7, 1));
    assert!(s.keyframe_loss_active());
    assert!(s.keyframe_loss_observed_at().is_some());
}
#[test]
fn the_start_of_a_stream_does_not_slow_it_but_a_lost_first_keyframe_does() {
    // Logged at the start of real streams: the client asks for a
    // keyframe, gets frame 1, and asks again ~100 ms later before it has
    // shown anything. Nothing was lost.
    const FAST: u64 = 800_000_000;
    const SAFE: u64 = 62_000_000;
    let start = |name: &str| {
        let s = Session::new(launch(name, Role::Stream), Negotiated::default());
        s.configure_keyframe_loss(true);
        s.request_recovery_keyframe();
        s.keyframe_loss_bps(FAST, SAFE, 1, true);
        s.request_recovery_keyframe();
        s.keyframe_loss_bps(FAST, SAFE, 2, true);
        s.request_recovery_keyframe();
        for frame in 3..=10 {
            s.keyframe_loss_bps(FAST, SAFE, frame, false);
        }
        s.stats.video_frame.store(11, Ordering::Release);
        s
    };
    let s = start("startup");
    assert!(!s.keyframe_loss_active());
    assert!(s.keyframe_loss_observed_at().is_none());
    assert_eq!(s.keyframe_loss_rate_bps(FAST, SAFE), FAST);
    assert!(s.info()["warnings"].as_array().unwrap().is_empty());
    // Frames reported after the second keyframe, inside the window, are
    // the same start; the first keyframe reported incomplete is a loss.
    s.record_fec_status(&fec_report(5, 7, 1));
    assert!(!s.keyframe_loss_active());
    s.record_fec_status(&fec_report(2, 7, 1));
    assert!(s.keyframe_loss_active());
}
#[test]
fn frames_the_host_could_not_send_slow_the_stream_and_the_clients_report_adds_nothing() {
    // Send-buffer pressure at 800 Mbps (error 10055) is loss: three frames
    // within two seconds slow the stream, named as lost on send.
    const FAST: u64 = 800_000_000;
    const SAFE: u64 = 62_000_000;
    let s = steady_stream("lost-on-send");
    for frame in 41..=43 {
        s.keyframe_loss_bps(FAST, SAFE, frame, false);
        s.keyframe_abandoned(frame);
    }
    assert!(s.keyframe_loss_active());
    assert_eq!(
        s.keyframe_loss_reason(),
        Some("three recent video frames were lost on send within two seconds")
    );
    assert_eq!(s.keyframe_loss_bps(FAST, SAFE, 44, false), SAFE);
    // Only the host saw it: for now, and nothing for the next session.
    assert!(s.keyframe_loss_sustained_at().is_none());
    assert_eq!(s.info()["frames_lost_on_send"], 3);
    assert!(
        s.info()["warnings"][0]["message"]
            .as_str()
            .unwrap()
            .contains("The PC could not send video packets fast enough")
    );
    // Two such frames are not enough, and the client reporting them is the
    // same loss, not more of it.
    let s = steady_stream("lost-on-send-twice");
    for frame in 41..=42 {
        s.keyframe_loss_bps(FAST, SAFE, frame, false);
        s.keyframe_abandoned(frame);
    }
    s.stats.video_frame.store(44, Ordering::Release);
    for frame in 41..=42 {
        s.record_fec_status(&fec_report(frame, 7, 1));
    }
    assert!(!s.keyframe_loss_active());
}

#[test]
fn a_keyframe_the_host_could_not_send_whole_slows_the_stream() {
    const FAST: u64 = 800_000_000;
    const SAFE: u64 = 62_000_000;
    let s = steady_stream("keyframe-lost-on-send");
    s.keyframe_loss_bps(FAST, SAFE, 41, true);
    s.keyframe_abandoned(41);
    assert_eq!(
        s.keyframe_loss_reason(),
        Some("a keyframe was lost on send")
    );
}

#[test]
fn the_keyframe_after_a_loss_on_send_does_not_hide_the_losses_around_it() {
    // Not a change the host made on its own, so no window follows it and
    // the client's reports of later frames still count.
    const FAST: u64 = 800_000_000;
    const SAFE: u64 = 62_000_000;
    let s = steady_stream("send-loss-recovery");
    s.keyframe_loss.lock().unwrap().settle();
    s.request_send_loss_recovery();
    s.keyframe_loss_bps(FAST, SAFE, 41, true);
    s.stats.video_frame.store(60, Ordering::Release);
    for frame in [45, 50, 55] {
        s.record_fec_status(&fec_report(frame, 7, 1));
    }
    assert!(s.keyframe_loss_active());
}

#[test]
fn a_request_just_before_the_hosts_keyframe_is_not_the_one_asked_for_again() {
    const FAST: u64 = 800_000_000;
    const SAFE: u64 = 62_000_000;
    let s = steady_stream("request-then-host-keyframe");
    s.request_recovery_keyframe();
    s.request_host_keyframe();
    s.keyframe_loss_bps(FAST, SAFE, 41, true);
    s.keyframe_loss_bps(FAST, SAFE, 42, false);
    s.request_recovery_keyframe();
    assert!(!s.keyframe_loss_active());
}
#[test]
fn pyrowave_feedback_cannot_force_frames_or_turn_invalidation_into_idr() {
    let session = Session::new(
        launch("pyrowave", Role::Stream),
        Negotiated {
            codec: 3,
            ..Default::default()
        },
    );
    session.idr.store(false, Ordering::Release);
    *session.recovery_wake.lock().unwrap() = Some(Box::new(|| {
        panic!("intra-only feedback must not wake the session");
    }));
    for (first, last) in [(1, 2), (0, 0), (4, 3)] {
        session.request_invalidation(first, last);
    }
    assert_eq!(
        session
            .stats
            .reference_invalidations
            .load(Ordering::Relaxed),
        3
    );
    assert_eq!(session.stats.idr_requests.load(Ordering::Relaxed), 0);
    assert!(session.invalidation.lock().unwrap().is_none());
    session.request_idr();
    assert_eq!(session.stats.idr_requests.load(Ordering::Relaxed), 1);
    assert!(!session.idr.load(Ordering::Acquire));
}
#[test]
fn send_loss_recovery_forces_a_keyframe_without_counting_a_client_request() {
    for codec in 0..=3 {
        let session = Session::new(
            launch("send-loss", Role::Stream),
            Negotiated {
                codec,
                ..Default::default()
            },
        );
        session.idr.store(false, Ordering::Release);
        session.request_send_loss_recovery();
        // PyroWave frames are all intra; nothing to force.
        assert_eq!(session.idr.load(Ordering::Acquire), codec != 3);
        assert_eq!(session.stats.idr_requests.load(Ordering::Relaxed), 0);
        assert_eq!(session.info()["send_loss_recoveries"], 1);
    }
}
#[test]
fn inter_frame_recovery_keeps_invalidation_ranges_and_idr_fallback() {
    for codec in 0..=2 {
        let session = Session::new(
            launch("inter-frame", Role::Stream),
            Negotiated {
                codec,
                ..Default::default()
            },
        );
        session.idr.store(false, Ordering::Release);
        session.request_invalidation(4, 6);
        session.request_invalidation(2, 3);
        assert_eq!(*session.invalidation.lock().unwrap(), Some((2, 6)));
        assert!(!session.idr.load(Ordering::Acquire));
        session.request_invalidation(0, 0);
        assert!(session.idr.load(Ordering::Acquire));
        assert_eq!(session.stats.idr_requests.load(Ordering::Relaxed), 1);
        assert_eq!(
            session
                .stats
                .reference_invalidations
                .load(Ordering::Relaxed),
            3
        );
    }
}
