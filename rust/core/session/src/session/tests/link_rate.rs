//! Tests for host ABR, link-rate control, link notices, warnings, and congestion control.
use super::super::link::{LinkReport, link_notice, mbps, note_link_change};
use super::*;

#[test]
fn host_bitrate_requires_valid_video_feedback_and_yields_to_the_client() {
    let s = Session::new(
        launch("host-abr", Role::Stream),
        Negotiated {
            bitrate_kbps: 20_000,
            configured_bitrate_kbps: 30_000,
            ..Default::default()
        },
    );
    s.configure_adaptive_bitrate(true, 0);
    let start = Instant::now();
    s.tick_adaptive_bitrate(start, Duration::from_millis(8));
    s.record_fec_status(&fec_report(1, 7, 1)); // no frame has been sent
    assert!(!s.adaptive_bitrate.lock().unwrap().active());
    s.stats.video_frame.store(10, Ordering::Release);
    s.record_fec_status(&fec_report(1, 7, 1));
    s.tick_adaptive_bitrate(start + Duration::from_secs(1), Duration::from_millis(8));
    assert!(s.bitrate.load(Ordering::Acquire) < 20_000);
    assert!(s.adaptive_pacing_bps(u64::MAX) < 30_000_000);
    let after_loss = s.info()["adaptive_bitrate"]["wire_budget_kbps"].clone();
    s.record_fec_status(&fec_report(1, 7, 1)); // same block is ignored
    s.tick_adaptive_bitrate(start + Duration::from_secs(2), Duration::from_millis(8));
    assert_eq!(s.info()["adaptive_bitrate"]["wire_budget_kbps"], after_loss);
    s.set_client_bitrate(17_000);
    s.record_fec_status(&fec_report(2, 7, 1));
    s.tick_adaptive_bitrate(start + Duration::from_secs(4), Duration::from_millis(200));
    assert_eq!(s.bitrate.load(Ordering::Acquire), 17_000);
    assert_eq!(s.adaptive_pacing_bps(800_000_000), 800_000_000);
    assert_eq!(s.info()["adaptive_bitrate"]["owner"], "client");
}
#[test]
fn a_burst_of_lost_frames_lowers_the_bitrate_and_says_so() {
    let (s, end) = link_session(60, true);
    assert_eq!(s.info()["link_rate"]["state"], "full");
    lose_frames(&s, 50..=54);
    let (from, to) = s
        .tick_link_rate(end + Duration::from_millis(200), Duration::from_millis(10))
        .expect("the controller lowers the bitrate");
    assert_eq!(from, 20_000);
    assert!((20_000 * 40 / 100..20_000 * 85 / 100).contains(&to), "{to}");
    assert_eq!(s.bitrate.load(Ordering::Acquire), to);
    let info = s.info();
    assert_eq!(info["link_rate"]["state"], "limited");
    let warning = info["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["code"] == LINK_WARNING)
        .expect("the console gets a warning")
        .clone();
    assert!(
        warning["message"].as_str().unwrap().contains("Mbps"),
        "{warning}"
    );
    // Later ticks do not change the warning or the bitrate while it holds.
    assert_eq!(
        s.tick_link_rate(end + Duration::from_millis(300), Duration::from_millis(10)),
        None
    );
}
/// 2026-10-29 sweep (r-def): eleven invalidation requests for one lost
/// frame cut the bitrate for 26 s; three separate losses still do.
#[test]
fn a_clients_repeated_invalidations_for_one_loss_count_once() {
    let (s, end) = link_session(60, true);
    for last in 45..=55 {
        s.request_invalidation(45, last);
    }
    assert_eq!(
        s.tick_link_rate(end + Duration::from_millis(200), Duration::from_millis(10)),
        None
    );
    assert_eq!(s.info()["link_rate"]["state"], "full");
    let (s, end) = link_session(60, true);
    for first in [20, 35, 50] {
        s.request_invalidation(first, first + 3);
        s.request_invalidation(first, first + 4);
    }
    let (_, to) = s
        .tick_link_rate(end + Duration::from_millis(200), Duration::from_millis(10))
        .expect("three losses lower the bitrate");
    assert!(to < 20_000, "{to}");
}
/// 2026-10-29: a PyroWave stream to a phone on Wi-Fi lost about a quarter
/// of its packets for the whole stream and nothing lowered its bitrate.
/// Review 2026-10-29: only a lost coarse block (block 0) loses the picture;
/// detail blocks carry no parity while it moves, and one lost packet there
/// only blurs part of it.
#[test]
fn lost_pyrowave_frames_lower_the_bitrate_too() {
    let s = Session::new(
        launch("link-rate-pyrowave", Role::Stream),
        Negotiated {
            codec: 3,
            pyrowave_records: true,
            bitrate_kbps: 397_464,
            configured_bitrate_kbps: 400_000,
            ..Default::default()
        },
    );
    s.configure_link_rate(true, 0);
    assert_eq!(s.info()["link_rate"]["state"], "full");
    let end = Instant::now();
    for frame in 1..=60u32 {
        let at = end - Duration::from_millis(17 * u64::from(60 - frame));
        s.record_link_sent_at(frame, 600, 840_000, at);
    }
    s.stats.video_frame.store(70, Ordering::Release);
    let block = |frame, block| {
        let mut report = fec_report(frame, 7, 1);
        report[19] = block;
        report[20] = 3;
        report
    };
    for frame in 40..=49 {
        s.record_fec_status(&block(frame, 1));
        s.record_fec_status(&block(frame, 2));
    }
    assert_eq!(
        s.tick_link_rate(end + Duration::from_millis(100), Duration::from_millis(10)),
        None,
        "lost detail blocks are not lost pictures"
    );
    for frame in 50..=54 {
        s.record_fec_status(&block(frame, 0));
    }
    let (from, to) = s
        .tick_link_rate(end + Duration::from_millis(200), Duration::from_millis(10))
        .expect("the controller lowers the PyroWave bitrate");
    assert_eq!(from, 397_464);
    assert!(
        (397_464 * 40 / 100..397_464 * 85 / 100).contains(&to),
        "{to}"
    );
    assert_eq!(s.bitrate.load(Ordering::Acquire), to);
    assert_eq!(s.info()["link_rate"]["state"], "limited");
}
/// Review 2026-10-29: 1080p120 at 200 Mbps is under PyroWave's 373 Mbps
/// floor. Its warning must not call the user's bitrate the lowest at which
/// PyroWave keeps a usable picture: this link loses PyroWave at that rate,
/// and HEVC or AV1 will hold up better. Its WARN line comes once.
#[test]
fn a_pyrowave_stream_with_no_room_under_it_is_told_to_use_hevc_or_av1() {
    let s = Session::new(
        launch("link-rate-pyrowave-no-room", Role::Stream),
        Negotiated {
            codec: 3,
            pyrowave_records: true,
            width: 1920,
            height: 1080,
            fps: 120,
            bitrate_kbps: 200_000,
            configured_bitrate_kbps: 200_000,
            ..Default::default()
        },
    );
    s.configure_link_rate(true, 0);
    let end = Instant::now();
    for frame in 1..=60u32 {
        let at = end - Duration::from_millis(8 * u64::from(60 - frame));
        s.record_link_sent_at(frame, 300, 420_000, at);
    }
    s.stats.video_frame.store(70, Ordering::Release);
    lose_frames(&s, 50..=54);
    let (_, cut) = s
        .tick_link_rate(end + Duration::from_millis(200), Duration::from_millis(10))
        .expect("loss cuts a stream with no room under it");
    assert!((100_000..200_000).contains(&cut), "{cut}");
    assert_eq!(s.info()["link_rate"]["state"], "limited");
    // More loss: it stops at half its bitrate, and since PyroWave has no
    // usable picture that low the warning names HEVC and AV1.
    // The reports are stamped with the real clock, so every round stays
    // within the 2 s evidence window of it.
    for round in 0..2u32 {
        let first = 64 + round * 30;
        // Past the last change's fence (500 ms).
        let from = end + Duration::from_millis(650 * u64::from(round) + 800);
        // Frames the size of the bitrate now in effect, 8 ms apart: an
        // encoder that follows the cuts.
        let bytes = s.bitrate.load(Ordering::Acquire);
        for frame in first..=first + 16 {
            let at = from + Duration::from_millis(8 * u64::from(frame - first));
            s.record_link_sent_at(frame, bytes as usize / 1_400, u64::from(bytes), at);
        }
        s.stats
            .video_frame
            .store(u64::from(first + 26), Ordering::Release);
        lose_frames(&s, first + 6..=first + 10);
        s.tick_link_rate(from + Duration::from_millis(150), Duration::from_millis(10));
    }
    assert_eq!(s.info()["link_rate"]["state"], "floor");
    let message = link_warning(&s).expect("the link warning");
    assert!(
        message.contains("loses PyroWave video at 100 Mbps"),
        "{message}"
    );
    assert!(message.contains("HEVC or AV1"), "{message}");
    assert!(!message.contains("lowest bitrate"), "{message}");
    // The floor again after a quiet spell is INFO, not a second WARN.
    let floor = butterpollo_media_policy::link_rate::Change {
        kind: butterpollo_media_policy::link_rate::ChangeKind::Floor,
        reason: "unrecoverable_frames",
        from_kbps: 200_000,
        to_kbps: 200_000,
        delivered_kbps: 0,
        evidence: butterpollo_media_policy::link_rate::Evidence::default(),
    };
    let mut link = s.link_rate.lock().unwrap();
    assert!(!note_link_change(&mut link, end, floor).warned);
    // A PyroWave stream with room under it warns at every floor, as before.
    let mut room = butterpollo_media_policy::link_control::LinkControl::new(
        butterpollo_media_policy::link_rate::LinkRate::new(
            true,
            &Negotiated {
                codec: 3,
                width: 1920,
                height: 1080,
                fps: 60,
                bitrate_kbps: 400_000,
                ..Default::default()
            },
            0,
            20,
        ),
        0,
    );
    for _ in 0..2 {
        let report = note_link_change(&mut room, end, floor);
        assert!(report.warned && !report.no_room);
    }
}
/// The floor warning of a PyroWave stream with room under it names the
/// floor as the lowest rate at which PyroWave keeps a usable picture.
#[test]
fn a_pyrowave_floor_warning_names_its_floor_and_hevc_or_av1() {
    let s = Session::new(
        launch("link-rate-pyrowave-floor", Role::Stream),
        Negotiated {
            codec: 3,
            bitrate_kbps: 400_000,
            ..Default::default()
        },
    );
    s.announce_link_change(LinkReport {
        change: butterpollo_media_policy::link_rate::Change {
            kind: butterpollo_media_policy::link_rate::ChangeKind::Floor,
            reason: "unrecoverable_frames",
            from_kbps: 186_624,
            to_kbps: 186_624,
            delivered_kbps: 0,
            evidence: butterpollo_media_policy::link_rate::Evidence::default(),
        },
        was_limited: true,
        warned: true,
        no_room: false,
        ceiling_kbps: 400_000,
        loss_share: 0.,
        lowest_kbps: 186_624,
        log_info: false,
    });
    let message = link_warning(&s).expect("the link warning");
    assert!(
        message.contains("the lowest bitrate at which PyroWave keeps a usable picture"),
        "{message}"
    );
    assert!(message.contains("Use HEVC or AV1"), "{message}");
}
/// Review 2026-10-29: a PyroWave client's keyframe requests are no
/// evidence of loss, even from a client that sends no FEC status; only
/// lost coarse blocks are.
#[test]
fn pyrowave_keyframe_requests_lower_nothing() {
    let s = Session::new(
        launch("link-rate-pyrowave-requests", Role::Stream),
        Negotiated {
            codec: 3,
            pyrowave_records: true,
            bitrate_kbps: 400_000,
            configured_bitrate_kbps: 400_000,
            ..Default::default()
        },
    );
    s.configure_link_rate(true, 0);
    let end = Instant::now();
    for frame in 1..=60u32 {
        let at = end - Duration::from_millis(17 * u64::from(60 - frame));
        s.record_link_sent_at(frame, 600, 840_000, at);
    }
    s.stats.video_frame.store(70, Ordering::Release);
    for _ in 0..10 {
        s.request_client_keyframe();
    }
    assert_eq!(
        s.tick_link_rate(end + Duration::from_millis(200), Duration::from_millis(10)),
        None
    );
    assert_eq!(s.info()["link_rate"]["state"], "full");
}
#[test]
fn a_client_that_asked_is_told_when_the_bitrate_is_lowered_and_when_it_is_back() {
    use butterpollo_media_policy::link_notice::{LinkReason, LinkState};
    let (s, end) = link_session(60, true);
    ask_for_notices(&s);
    let at = end + Duration::from_millis(200);
    assert_eq!(notice_at(&s, at), None, "nothing to say at the full rate");
    lose_frames(&s, 50..=54);
    let (_, to) = s
        .tick_link_rate(at, Duration::from_millis(10))
        .expect("the controller lowers the bitrate");
    let notice = notice_at(&s, at).expect("the step down is announced");
    assert_eq!(notice.state, LinkState::Limited);
    assert_eq!(notice.reason, LinkReason::Network);
    assert_eq!((notice.ceiling_kbps, notice.current_kbps), (20_000, to));
    assert_eq!(notice_at(&s, at + Duration::from_millis(8)), None, "once");
    // A reconfigure puts the requested bitrate back and the client hears it.
    s.rebuild_link_rate(&s.config, true, 0);
    assert_eq!(s.bitrate.load(Ordering::Acquire), 20_000);
    let notice = notice_at(&s, at + Duration::from_secs(1)).expect("the return is announced");
    assert_eq!(notice.state, LinkState::Full);
    assert_eq!(notice.reason, LinkReason::None);
    assert_eq!((notice.ceiling_kbps, notice.current_kbps), (20_000, 20_000));
}
#[test]
fn a_client_that_did_not_ask_is_told_nothing() {
    let (s, end) = link_session(60, true);
    lose_frames(&s, 50..=54);
    let at = end + Duration::from_millis(200);
    s.tick_link_rate(at, Duration::from_millis(10))
        .expect("the controller lowers the bitrate");
    assert_eq!(notice_at(&s, at), None);
    assert_eq!(notice_at(&s, at + Duration::from_secs(5)), None);
}
#[test]
fn a_client_that_takes_the_bitrate_over_is_told_it_is_no_longer_lowered() {
    use butterpollo_media_policy::link_notice::LinkState;
    let (s, end) = link_session(60, true);
    ask_for_notices(&s);
    lose_frames(&s, 50..=54);
    let at = end + Duration::from_millis(200);
    s.tick_link_rate(at, Duration::from_millis(10)).unwrap();
    assert_eq!(notice_at(&s, at).unwrap().state, LinkState::Limited);
    s.set_client_bitrate(12_000);
    let notice = notice_at(&s, at + Duration::from_secs(1)).expect("cleared");
    assert_eq!(notice.state, LinkState::Full);
    assert_eq!((notice.ceiling_kbps, notice.current_kbps), (12_000, 12_000));
}
#[test]
fn every_controller_change_has_its_notice() {
    use butterpollo_media_policy::link_notice::{LinkReason, LinkState};
    use butterpollo_media_policy::link_rate::{Change, ChangeKind, Evidence};
    for (kind, state, reason) in [
        (
            ChangeKind::StepDown,
            LinkState::Limited,
            LinkReason::Network,
        ),
        (ChangeKind::Probe, LinkState::Limited, LinkReason::Network),
        (
            ChangeKind::ProbeFailed,
            LinkState::Limited,
            LinkReason::Network,
        ),
        (ChangeKind::Floor, LinkState::Floor, LinkReason::Network),
        (
            ChangeKind::EncoderFloor,
            LinkState::Floor,
            LinkReason::Network,
        ),
        (ChangeKind::Full, LinkState::Full, LinkReason::None),
    ] {
        let to_kbps = if kind == ChangeKind::Full {
            20_000
        } else {
            5_000
        };
        let notice = link_notice(&LinkReport {
            change: Change {
                kind,
                reason: "test",
                from_kbps: 10_000,
                to_kbps,
                delivered_kbps: 0,
                evidence: Evidence::default(),
            },
            was_limited: true,
            warned: false,
            no_room: false,
            ceiling_kbps: 20_000,
            loss_share: 0.,
            lowest_kbps: 5_000,
            log_info: false,
        });
        assert_eq!(
            (
                notice.state,
                notice.reason,
                notice.ceiling_kbps,
                notice.current_kbps
            ),
            (state, reason, 20_000, to_kbps),
            "{kind:?}"
        );
    }
}
#[test]
fn a_healthy_session_keeps_its_bitrate_and_has_no_warning() {
    let (s, end) = link_session(120, true);
    // Frames that FEC repaired are not loss the controller acts on.
    for frame in (10..120).step_by(7) {
        s.record_fec_status(&fec_report(frame, 9, 1));
    }
    for ms in (0..3000).step_by(8) {
        assert_eq!(
            s.tick_link_rate(end + Duration::from_millis(ms), Duration::from_millis(10)),
            None
        );
    }
    assert_eq!(s.bitrate.load(Ordering::Acquire), 20_000);
    let info = s.info();
    assert_eq!(info["link_rate"]["state"], "full");
    assert!(
        info["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .all(|w| w["code"] != LINK_WARNING)
    );
}
#[test]
fn the_controller_is_off_when_disabled() {
    let (s, end) = link_session(60, false);
    lose_frames(&s, 50..=54);
    assert_eq!(
        s.tick_link_rate(end + Duration::from_millis(200), Duration::ZERO),
        None
    );
    assert_eq!(s.bitrate.load(Ordering::Acquire), 20_000);
    assert_eq!(s.info()["link_rate"]["state"], "off");
}
#[test]
fn a_client_that_sets_the_bitrate_clears_the_warning_and_restores_nothing() {
    let (s, end) = link_session(60, true);
    lose_frames(&s, 50..=54);
    s.tick_link_rate(end + Duration::from_millis(200), Duration::from_millis(10))
        .unwrap();
    assert!(
        s.launch
            .warnings
            .snapshot()
            .iter()
            .any(|w| w.code == LINK_WARNING)
    );
    s.set_client_bitrate(12_000);
    assert_eq!(s.bitrate.load(Ordering::Acquire), 12_000);
    assert!(
        s.launch
            .warnings
            .snapshot()
            .iter()
            .all(|w| w.code != LINK_WARNING)
    );
    assert_eq!(s.info()["link_rate"]["reason"], "client_controlled");
    lose_frames(&s, 55..=59);
    assert_eq!(
        s.tick_link_rate(end + Duration::from_secs(1), Duration::from_millis(10)),
        None
    );
    assert_eq!(s.bitrate.load(Ordering::Acquire), 12_000);
    // A reconfigure keeps the client in charge and does not bring back the
    // bitrate the host had lowered.
    s.rebuild_link_rate(&s.config, true, 0);
    assert_eq!(s.bitrate.load(Ordering::Acquire), 12_000);
    assert_eq!(s.info()["link_rate"]["reason"], "client_controlled");
}
#[test]
fn a_keyframe_the_host_abandoned_is_not_counted_again_as_network_loss() {
    // Two 300-packet keyframes in two loss windows: every packet of both
    // is missing at the client, 7 % of what was sent in each window.
    for abandoned in [true, false] {
        let s = Session::new(
            launch("link-abandoned", Role::Stream),
            Negotiated {
                bitrate_kbps: 20_000,
                configured_bitrate_kbps: 20_000,
                ..Default::default()
            },
        );
        s.configure_link_rate(true, 0);
        s.configure_keyframe_loss(true);
        s.stats.video_frame.store(500, Ordering::Release);
        let base = Instant::now();
        let rtt = Duration::from_millis(10);
        assert_eq!(s.tick_link_rate(base, rtt), None);
        let mut cut = None;
        for window in 0..2u32 {
            let first = 1 + window * 41;
            for frame in first..first + 40 {
                let at = base
                    + Duration::from_millis(
                        u64::from(window) * 600 + u64::from(frame - first) * 10,
                    );
                s.record_link_sent_at(frame, 100, 112_600, at);
            }
            let keyframe = first + 40;
            s.record_link_sent_at(
                keyframe,
                300,
                337_800,
                base + Duration::from_millis(u64::from(window) * 600 + 400),
            );
            if abandoned {
                s.keyframe_loss.lock().unwrap().sending(keyframe, true);
                s.keyframe_abandoned(keyframe);
            }
            s.record_fec_status(&fec_block(keyframe, 0, 2, 125, 0));
            s.record_fec_status(&fec_block(keyframe, 1, 2, 125, 0));
            cut = s.tick_link_rate(
                base + Duration::from_millis(u64::from(window + 1) * 600),
                rtt,
            );
        }
        assert_eq!(
            cut.is_some(),
            !abandoned,
            "abandoned {abandoned}: {}",
            s.link_rate_snapshot()
        );
        if abandoned {
            assert_eq!(s.link_rate_snapshot()["state"], "full");
        }
    }
}
#[test]
fn the_warning_and_the_console_agree_on_the_bitrate_and_nothing_is_said_under_the_lock() {
    let (s, end) = link_session(60, true);
    lose_frames(&s, 50..=54);
    // The change is worked out under the lock; the warning is not set
    // until the lock is released.
    let now = end + Duration::from_millis(200);
    let mut link = s.link_rate.lock().unwrap();
    link.drain(&s.link_sent);
    link.rate
        .tick(
            now,
            Duration::from_millis(10),
            s.fec_percent(),
            s.stream_mode().period(),
            20_000,
            true,
        )
        .expect("the controller lowers the bitrate");
    let change = link.rate.take_change().unwrap();
    let report = note_link_change(&mut link, now, change);
    drop(link);
    assert_eq!(link_warning(&s), None);
    s.announce_link_change(report);
    let message = link_warning(&s).expect("the console gets a warning");
    let lowest = s.link_rate.lock().unwrap().lowest_kbps;
    // "As low as", never a bare "lowered the bitrate to X", which the
    // badge's current rate would contradict once the host probes upward.
    assert!(
        message.contains(&format!("as low as {} so far", mbps(lowest))),
        "{message}"
    );
    assert!(!message.contains("bitrate to"), "{message}");
    assert!(message.contains("20 Mbps"), "{message}");
}
#[test]
fn an_encoder_floor_says_so_without_claiming_the_lowest_bitrate_is_too_high() {
    let s = Session::new(launch("link-floor", Role::Stream), Negotiated::default());
    let change = butterpollo_media_policy::link_rate::Change {
        kind: butterpollo_media_policy::link_rate::ChangeKind::EncoderFloor,
        reason: "encoder_floor",
        from_kbps: 9_000,
        to_kbps: 14_000,
        delivered_kbps: 17_000,
        evidence: Default::default(),
    };
    let mut link = s.link_rate.lock().unwrap();
    let report = note_link_change(&mut link, Instant::now(), change);
    drop(link);
    s.announce_link_change(report);
    let message = link_warning(&s).unwrap();
    assert!(message.contains("14 Mbps"), "{message}");
    assert!(message.contains("does not go lower"), "{message}");
    assert!(!message.contains("cannot carry even"), "{message}");
}
#[test]
fn a_queue_cut_says_the_network_held_video_back_not_that_packets_were_lost() {
    let s = Session::new(launch("link-queue", Role::Stream), Negotiated::default());
    let change = butterpollo_media_policy::link_rate::Change {
        kind: butterpollo_media_policy::link_rate::ChangeKind::StepDown,
        reason: "queue",
        from_kbps: 36_000,
        to_kbps: 31_700,
        delivered_kbps: 0,
        evidence: Default::default(),
    };
    let mut link = s.link_rate.lock().unwrap();
    let report = note_link_change(&mut link, Instant::now(), change);
    drop(link);
    s.announce_link_change(report);
    let message = link_warning(&s).expect("the console gets a warning");
    assert!(
        message.contains("holding video back in a growing queue"),
        "{message}"
    );
    assert!(!message.contains("lost"), "{message}");
    assert!(message.contains("as low as"), "{message}");
}
#[test]
fn keyframe_requests_count_only_for_a_client_that_sends_no_fec_status() {
    for reports_fec in [false, true] {
        let (s, end) = link_session(60, true);
        if reports_fec {
            s.record_fec_status(&fec_report(40, 10, 2));
        }
        for _ in 0..4 {
            s.request_client_keyframe();
        }
        let cut = s.tick_link_rate(end + Duration::from_millis(200), Duration::from_millis(10));
        assert_eq!(cut.is_some(), !reports_fec, "reports_fec {reports_fec}");
    }
}
#[test]
fn arrival_feedback_turns_the_ledger_on_and_a_client_bitrate_turns_it_off() {
    let s = Session::new(
        launch("congestion", Role::Stream),
        Negotiated {
            bitrate_kbps: 20_000,
            configured_bitrate_kbps: 20_000,
            ..Default::default()
        },
    );
    let report = butterpollo_media_policy::congestion::TransportFeedback {
        base: 0,
        stride: 1,
        entries: vec![0, 10, 20],
        ..Default::default()
    }
    .encode()
    .unwrap();
    // Off: reports are ignored and the sender records nothing.
    assert!(!s.congestion_active());
    assert!(!s.record_transport_feedback(&report));
    assert!(!s.transport_sent.live());
    assert_eq!(s.info()["congestion"]["state"], "off");
    s.configure_congestion(true, 0, false);
    assert!(s.congestion_active());
    assert!(!s.transport_sent.live());
    assert!(s.record_transport_feedback(&report));
    assert!(s.transport_sent.live());
    assert!(!s.record_transport_feedback(&[0; 3]));
    s.record_transport_sent(&[vec![0u8; 100]], 28);
    assert_eq!(s.congestion_snapshot()["reports"], 1);
    // No limit before it learned anything: the bitrate stays.
    let (change, applied) = s.tick_congestion(Instant::now(), Duration::from_millis(10));
    assert!(change.is_none() && applied.is_none());
    assert_eq!(s.bitrate.load(Ordering::Acquire), 20_000);
    assert_eq!(s.adaptive_pacing_bps(50_000_000), 50_000_000);
    s.set_client_bitrate(12_000);
    assert!(!s.congestion_active());
    assert!(!s.transport_sent.live());
    assert!(!s.record_transport_feedback(&report));
    // The takeover outlives a rebuilt stream.
    s.rebuild_congestion(&s.config, true, 0, false);
    assert!(!s.congestion_active());
}
