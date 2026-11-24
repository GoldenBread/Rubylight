//! Unit tests for Host config snapshots, focus checks, shutdown audio release, and probe submodules.

use super::*;

mod packed_reprobe;
mod persistence_and_retry;
mod pyrowave_retry;

#[test]
fn the_config_snapshot_is_shared_while_unchanged_and_follows_every_change() {
    use std::sync::Arc;
    let f = super::test_support::Fixture::new();
    let first = f.host.config_snapshot();
    let name = first.get("sunshine_name", "").to_owned();
    assert!(Arc::ptr_eq(&first, &f.host.config_snapshot()));
    assert_eq!(first.values, f.host.config.read().unwrap().values);
    // Any change, also one written straight into the lock, shows at once.
    f.host
        .config
        .write()
        .unwrap()
        .values
        .insert("sunshine_name".into(), "Den".into());
    let changed = f.host.config_snapshot();
    assert!(!Arc::ptr_eq(&first, &changed));
    assert_eq!(crate::network::host_name(&changed), "Den");
    assert_eq!(changed.values, f.host.config.read().unwrap().values);
    assert!(Arc::ptr_eq(&changed, &f.host.config_snapshot()));
    // What a request still holds stays as it was.
    assert_eq!(first.get("sunshine_name", ""), name);
}
#[test]
fn a_focus_check_error_marks_only_that_codec_unsupported() {
    use butterpollo_media_policy::encoder_focus::{Reason, Support};
    let mut support: [Support; 5] = std::array::from_fn(|_| Support::NotProbed);
    support[0] = focus_verdict(Ok(Support::Verified { delta: 10.0 }));
    support[1] = focus_verdict(Err(anyhow::anyhow!("frame 3 was not encoded")));
    support[3] = focus_verdict(Ok(Support::Verified { delta: 50.0 }));
    assert_eq!(
        support[1],
        Support::Unsupported {
            reason: Reason::CodecUnsupported,
            message: "the focus check failed: frame 3 was not encoded".into(),
        }
    );
    assert_eq!(
        focus_summary(&support),
        "h264=verified(+10.0) hevc=unsupported(the focus check failed: frame 3 was not encoded) hevc10=not_probed av1=verified(+50.0) av1_10=not_probed"
    );
}
#[test]
fn a_timed_out_early_or_crashed_pyrowave_probe_is_probed_again() {
    use butterpollo_win_graphics::codec_probe::{Crashed, SessionNotReady, TimedOut};
    use std::time::Duration;
    // A probe that stalled under a busy CPU left PyroWave unoffered
    // until the host restarted. The load that stalled it, such as a
    // game, is given a minute to pass.
    assert_eq!(
        super::pyrowave_retry_delay(&TimedOut.into()),
        Some(Duration::from_secs(60))
    );
    assert_eq!(
        super::pyrowave_retry_delay(&SessionNotReady::new("no user").into()),
        Some(Duration::from_secs(5))
    );
    // Issue #94: a worker that crashed on half of one PC's starts.
    assert_eq!(
        super::pyrowave_retry_delay(&Crashed::new("exited before replying (0xc0000005)").into()),
        Some(Duration::from_secs(5))
    );
    assert_eq!(
        super::pyrowave_retry_delay(&anyhow::anyhow!("loading libpyrowave-shared-1.dll")),
        None
    );
}
/// The shutdown audio wait: a game that ignores close loses the route at the
/// deadline, while a fast close releases it by its notice without a warning.
mod shutdown_audio {
    use super::super::{AppStop, test_support::Fixture};
    use butterpollo_win_io::audio_route::Route;
    use std::{
        sync::Arc,
        time::{Duration, Instant},
    };

    fn route(f: &Fixture) {
        *f.host.app_audio.lock().unwrap() = Some(Arc::new(Route::detached(&f.host.directory)));
    }

    /// What `run` logged at WARN, to this fixture's own file.
    fn warnings(f: &Fixture, run: impl FnOnce()) -> String {
        let path = f.host.directory.join("shutdown-audio.log");
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::WARN)
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .with_writer(std::sync::Mutex::new(std::fs::File::create(&path).unwrap()))
            .finish();
        crate::test_log::capture_during(subscriber, run);
        std::fs::read_to_string(path).unwrap()
    }

    /// The passed deadline is inside main's `stop_app` wait: past it the route
    /// must be back on the PC's default device, whatever the game does.
    #[test]
    fn a_game_that_ignores_close_loses_its_audio_route_at_the_deadline() {
        let f = Fixture::new();
        route(&f);
        // The game keeps the sender: its processes never report that they stopped.
        let (_game, stopped) = std::sync::mpsc::channel();
        let wait = Duration::from_millis(100);
        let began = Instant::now();
        let log = warnings(&f, || {
            f.host.release_audio_by(stopped, began + wait);
        });
        assert!(
            began.elapsed() >= wait,
            "the route was released before the passed deadline"
        );
        assert!(f.host.app_audio.lock().unwrap().is_none());
        assert!(log.contains("had not stopped"), "{log}");
    }

    #[test]
    fn a_fast_close_releases_the_audio_route_without_a_warning() {
        let f = Fixture::new();
        route(&f);
        let (game, stopped) = std::sync::mpsc::channel();
        game.send(()).unwrap();
        let log = warnings(&f, || {
            f.host
                .release_audio_by(stopped, Instant::now() + Duration::from_secs(5));
        });
        assert!(f.host.app_audio.lock().unwrap().is_none());
        assert!(!log.contains("had not stopped"), "{log}");
    }

    /// `stop_app` returns only after the app's stop; a game whose stop outlasts
    /// the passed audio deadline must lose the route before that return.
    #[test]
    fn stop_app_releases_the_audio_route_before_the_game_stops() {
        let f = Fixture::new();
        route(&f);
        let mut running = crate::process::RunningApp::with_environment(
            &butterpollo_session::state::App::desktop(),
            Default::default(),
        )
        .expect("the test app starts");
        running.owner = "test-owner".into();
        running.stop_delay = Some(Duration::from_secs(2));
        *f.host.current_app.lock().unwrap() = Some(running);
        let stopping = f.host.clone();
        let deadline = Instant::now() + Duration::from_millis(100);
        let stopped = std::thread::spawn(move || stopping.stop_app(AppStop::Shutdown { deadline }));
        let gone = (0..200).any(|_| {
            std::thread::sleep(Duration::from_millis(10));
            f.host.app_audio.lock().unwrap().is_none()
        });
        assert!(gone, "the audio route outlived the passed deadline");
        assert!(
            !stopped.is_finished(),
            "the audio route was not released until the game had stopped"
        );
        stopped.join().unwrap();
        assert!(f.host.app_audio.lock().unwrap().is_none());
    }
}
