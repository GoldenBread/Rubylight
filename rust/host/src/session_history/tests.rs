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
    session.bitrate.store(40_000, Ordering::Relaxed);
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
    assert_eq!(entry.bitrate_kbps, 40_000);
    assert_eq!(entry.end_reason, "ended");
    assert_eq!(entry.end_category, None);
    assert_eq!((entry.mean_fps, entry.p95_fps), (120., 120.));
    assert_eq!(entry.loss_percent, 0.);
    assert!(entry.duration_ms < 1_000, "{}", entry.duration_ms);
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
fn the_sampled_frame_rate_reports_its_slow_tail() {
    let mut samples = Samples::default();
    let start = Instant::now();
    samples.sample(0, start);
    // Twenty seconds at 60 fps, one of them at 30.
    for second in 1..=20 {
        let frames = 60 * second;
        let frames = if second == 7 { frames - 30 } else { frames };
        samples.sample(frames, start + Duration::from_secs(second));
    }
    assert_eq!(samples.fps.len(), 20);
    // The slowest 5% of seconds, rounded up, is one second.
    assert_eq!(samples.p95(), Some(30.));
    assert_eq!(samples.mean(), Some((19. * 60. + 30.) / 20.));
    // Within a second, nothing new is measured.
    samples.sample(1_260, start + Duration::from_millis(20_500));
    assert_eq!(samples.fps.len(), 20);
    // A stream too short for one sample has no p95 of its own.
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
        ..Default::default()
    });
    let reloaded = History::load(&f.host.directory).entries();
    assert_eq!(reloaded.len(), 1);
    assert_eq!(reloaded[0].id, "after-damage");
}

#[test]
fn a_crashed_stream_becomes_an_interrupted_entry() {
    let f = Fixture::new();
    let client = f.client(0x071f1f00);
    let journal = Journal {
        version: 1,
        state: State::Streaming,
        owner: Some(client.uuid.clone()),
        app: Some(SessionApp {
            install_id: "install".into(),
            app_id: 4,
            title: "Portal 2".into(),
        }),
        since_unix_ms: now_ms() - 60_000,
    };
    f.host.record_interrupted_session(&journal);
    let entries = f.host.session_history.entries();
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry.end_reason, "interrupted");
    assert_eq!(entry.device_name, "Parity device");
    assert_eq!(entry.app, "Portal 2");
    assert!(entry.duration_ms >= 60_000, "{}", entry.duration_ms);
    // A session that was not streaming has no stream to record.
    f.host.record_interrupted_session(&Journal {
        state: State::Suspended,
        ..journal
    });
    assert_eq!(f.host.session_history.entries().len(), 1);
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
