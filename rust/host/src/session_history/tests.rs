use super::*;
use crate::state::test_support::Fixture;
use butterpollo_core::native_api::session::SessionApp;
use std::sync::atomic::Ordering;

#[test]
fn a_stream_is_recorded_with_what_it_ran_and_how_it_went() {
    let f = Fixture::new();
    let client = f.client(0x071f1f00);
    let session = f.session(client, Role::Stream);
    let app = f.host.apps.read().unwrap()[0].name.clone();
    session.stats.frames.store(3_600, Ordering::Relaxed);
    session.stats.packets.store(100_000, Ordering::Relaxed);
    session.stats.bytes.store(18_000_000, Ordering::Relaxed);
    session.bitrate.store(12_000, Ordering::Relaxed);
    // Two full seconds of 120 fps, sampled as the housekeeping tick does.
    let start = Instant::now();
    f.host.session_history.sample(&session.launch.id, 0, start);
    let one = start + Duration::from_secs(1);
    let two = start + Duration::from_secs(2);
    f.host.session_history.sample(&session.launch.id, 120, one);
    f.host.session_history.sample(&session.launch.id, 240, two);

    f.host.record_session(&session, history::REASON_ENDED);

    let entries = f.host.session_history.entries();
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry.id, session.launch.id);
    assert_eq!(entry.device, session.launch.client.uuid);
    assert_eq!(entry.device_name, "Parity device");
    assert_eq!(entry.app_id, session.launch.app_id);
    assert_eq!(entry.app, app);
    assert_eq!(entry.role, "stream");
    assert_eq!(entry.transport, "moonlight");
    assert_eq!(entry.codec, "h264");
    assert_eq!((entry.width, entry.height, entry.fps), (1920, 1080, 60));
    // The encoder ran below the rate the stream asked for, and both are
    // recorded: the effective one is the old field's meaning, unchanged.
    assert_eq!(entry.requested_bitrate_kbps, 20_000);
    assert_eq!(entry.bitrate_kbps, 12_000);
    assert_eq!(entry.end_reason, "ended");
    assert_eq!(entry.end_category, None);
    // Two sampled seconds are not a 5% tail: the mean stands alone.
    assert_eq!((entry.mean_fps, entry.p95_fps), (120., 0.));
    // The client reported no FEC blocks, which is not "no loss".
    assert_eq!(entry.loss_percent, None);
    assert!(entry.duration_ms < 1_000, "{}", entry.duration_ms);
}

#[test]
fn a_client_that_reported_no_loss_is_not_the_same_as_one_that_did_not_report() {
    let f = Fixture::new();
    let client = f.client(0x071f1f00);
    let session = f.session(client, Role::Stream);
    session.stats.packets.store(10_000, Ordering::Relaxed);
    // A malformed report still counts as a report: the client speaks FEC status.
    let _ = session
        .stats
        .performance
        .lock()
        .unwrap()
        .record_fec_status(&[], None);
    f.host.record_session(&session, history::REASON_ENDED);
    assert_eq!(f.host.session_history.entries()[0].loss_percent, Some(0.));
}

#[test]
fn a_failed_stream_records_its_reason_and_troubleshooting_category() {
    let f = Fixture::new();
    let client = f.client(0x071f1f00);
    let session = f.session(client, Role::Stream);
    session.fail("GPU device lost: DXGI_ERROR_DEVICE_REMOVED (0x887A0005)");
    f.host.record_session(&session, history::REASON_FAILED);
    let entries = f.host.session_history.entries();
    assert_eq!(entries[0].end_reason, "failed");
    assert_eq!(entries[0].end_category.as_deref(), Some("driver"));
    assert!(
        entries[0].end_detail.contains("DXGI_ERROR_DEVICE_REMOVED"),
        "{}",
        entries[0].end_detail
    );
}

#[test]
fn a_start_that_never_ran_has_no_figures() {
    let f = Fixture::new();
    let client = f.client(0x071f1f00);
    let session = f.session(client, Role::Stream);
    session.fail("the stream could not start: the app exited");
    f.host
        .record_session(&session, history::REASON_START_FAILED);
    let entry = &f.host.session_history.entries()[0];
    // The negotiated request is not a measurement; neither bitrate is set.
    assert_eq!(
        (
            entry.width,
            entry.height,
            entry.fps,
            entry.bitrate_kbps,
            entry.requested_bitrate_kbps
        ),
        (0, 0, 0, 0, 0)
    );
    assert_eq!(entry.mean_fps, 0.);
    assert_eq!(entry.p95_fps, 0.);
    assert_eq!(entry.mean_bitrate_kbps, 0.);
    assert_eq!(entry.loss_percent, None);
    assert_eq!(entry.end_reason, "start_failed");
    assert!(entry.duration_ms < 1_000, "{}", entry.duration_ms);
}

#[test]
fn the_sampled_frame_rate_reports_its_slow_tail_only_with_enough_samples() {
    let mut samples = Samples::default();
    let start = Instant::now();
    samples.sample(0, start);
    // Nineteen seconds: 5% is not yet one second, so no percentile.
    for second in 1..=19 {
        samples.sample(60 * second, start + Duration::from_secs(second));
    }
    assert_eq!(samples.p95(), None);
    // The twentieth, at 30 fps: the slowest 5% of seconds, rounded up, is one.
    samples.sample(1_170, start + Duration::from_secs(20));
    assert_eq!(samples.fps.len(), 20);
    assert_eq!(samples.p95(), Some(30.));
    assert_eq!(samples.mean(), Some((19. * 60. + 30.) / 20.));
    // Within a second, nothing new is measured.
    samples.sample(1_200, start + Duration::from_millis(20_500));
    assert_eq!(samples.fps.len(), 20);
    // A stream too short for one sample has no figures at all.
    let mut short = Samples::default();
    short.sample(0, start);
    assert_eq!(short.p95(), None);
    assert_eq!(short.mean(), None);
}

#[test]
fn entries_survive_a_restart_and_a_damaged_file_starts_from_none() {
    let f = Fixture::new();
    let client = f.client(0x071f1f00);
    let session = f.session(client, Role::Stream);
    f.host.record_session(&session, history::REASON_ENDED);
    f.host.session_history.flush();
    let saved = f.host.session_history.entries();
    assert_eq!(saved.len(), 1);
    // A restarted host reads the entry from its profile directory.
    assert_eq!(History::load(&f.host.directory).entries(), saved);

    // A damaged file reads as no history; the next save replaces it.
    std::fs::write(
        f.host.directory.join(FILE),
        b"{\"version\":1,\"entries\":[tru",
    )
    .unwrap();
    assert!(History::load(&f.host.directory).entries().is_empty());
    f.host.session_history.push(Entry {
        id: "after-damage".into(),
        ended_unix_ms: now_ms(),
        ..Default::default()
    });
    f.host.session_history.flush();
    let reloaded = History::load(&f.host.directory).entries();
    // The host's memory still held the first session; the next save replaced
    // the damaged file with both, newest first.
    assert_eq!(reloaded.len(), 2);
    assert_eq!(reloaded[0].id, "after-damage");
    assert_eq!(reloaded[1].id, session.launch.id);
}

#[test]
fn three_reasons_survive_a_restart_in_order() {
    let f = Fixture::new();
    let client = f.client(0x071f1f00);
    let ended = f.session(client.clone(), Role::Stream);
    f.host.record_session(&ended, history::REASON_ENDED);
    let failed = f.session(client.clone(), Role::Stream);
    failed.fail("the encoder failed");
    f.host.record_session(&failed, history::REASON_FAILED);
    let now = now_ms();
    f.host.record_interrupted_session(&Journal {
        version: 1,
        state: State::Streaming,
        owner: Some(client.uuid.clone()),
        app: None,
        since_unix_ms: now - 5_000,
        last_seen_unix_ms: now - 1_000,
    });
    f.host.session_history.flush();
    let reasons: Vec<_> = History::load(&f.host.directory)
        .entries()
        .into_iter()
        .map(|entry| entry.end_reason)
        .collect();
    assert_eq!(reasons, ["interrupted", "failed", "ended"]);
}

#[test]
fn a_crashed_stream_becomes_an_interrupted_entry_that_ends_at_its_last_heartbeat() {
    let f = Fixture::new();
    let client = f.client(0x071f1f00);
    let now = now_ms();
    // The stream began two minutes ago; the host last said it was alive a
    // minute ago and then stopped for eight hours, say.
    let journal = Journal {
        version: 1,
        state: State::Streaming,
        owner: Some(client.uuid.clone()),
        app: Some(SessionApp {
            install_id: "install".into(),
            app_id: 4,
            title: "Portal 2".into(),
        }),
        since_unix_ms: now - 120_000,
        last_seen_unix_ms: now - 60_000,
    };
    f.host.record_interrupted_session(&journal);
    let entries = f.host.session_history.entries();
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry.end_reason, "interrupted");
    assert_eq!(entry.device_name, "Parity device");
    assert_eq!(entry.app, "Portal 2");
    // The minute between the first and last beat, not the downtime after.
    assert_eq!(entry.duration_ms, 60_000);
    assert_eq!(entry.ended_unix_ms, now - 60_000);
    // The page keys its list by id: a restart's entry needs its own.
    assert!(!entry.id.is_empty());
    // A second crash inside the window gets a different id.
    f.host.record_interrupted_session(&Journal {
        since_unix_ms: now - 300_000,
        last_seen_unix_ms: now - 240_000,
        ..journal.clone()
    });
    let entries = f.host.session_history.entries();
    assert_eq!(entries.len(), 2);
    assert_ne!(entries[0].id, entries[1].id);
    // A session that was not streaming has no stream to record.
    f.host.record_interrupted_session(&Journal {
        state: State::Suspended,
        ..journal
    });
    assert_eq!(f.host.session_history.entries().len(), 2);
}

#[test]
fn a_journal_without_a_heartbeat_has_no_duration_rather_than_the_host_downtime() {
    let f = Fixture::new();
    let client = f.client(0x071f1f00);
    let now = now_ms();
    f.host.record_interrupted_session(&Journal {
        version: 1,
        state: State::Streaming,
        owner: Some(client.uuid),
        app: None,
        since_unix_ms: now - 8 * 60 * 60 * 1000,
        last_seen_unix_ms: 0,
    });
    let entry = &f.host.session_history.entries()[0];
    assert_eq!(
        entry.duration_ms, 0,
        "an old journal's downtime is not a duration"
    );
    assert_eq!(entry.ended_unix_ms, now - 8 * 60 * 60 * 1000);
}

#[test]
fn only_streams_enter_the_history() {
    let f = Fixture::new();
    let client = f.client(0x071f1f00);
    for role in [Role::RemoteMonitor, Role::InputOnly] {
        let session = f.session(client.clone(), role);
        f.host.record_session(&session, history::REASON_ENDED);
    }
    assert!(f.host.session_history.entries().is_empty());
}
