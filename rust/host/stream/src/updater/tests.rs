//! Unit tests for updater scheduling, queue transitions, and polling.

mod downloads;

use super::*;
struct Fixture {
    host: Shared,
    directory: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let directory =
            std::env::temp_dir().join(format!("butterpollo-update-test-{}", uuid::Uuid::new_v4()));
        let host =
            crate::state::Host::load(directory.clone(), directory.join("assets"), None).unwrap();
        Self { host, directory }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
fn launch(role: butterpollo_session::session::Role) -> crate::state::Launch {
    crate::state::Launch {
        id: "fixture".into(),
        client: butterpollo_session::state::Client {
            name: "fixture".into(),
            cert: String::new(),
            uuid: "fixture".into(),
            perm: u32::MAX,
            enabled: true,
            extra: Default::default(),
        },
        peer: "127.0.0.1".parse().unwrap(),
        app_id: 1,
        background: None,
        key: [0; 16],
        key_id: 1,
        ping: "fixture".into(),
        connect_data: 1,
        role,
        created: std::time::Instant::now(),
        rtsp_encrypted: true,
        rtsp_counter: Default::default(),
        rtsp_received: Default::default(),
        preparation: Default::default(),
        vrr_requested: false,
        host_audio: false,
        requested_rate: 0,
        options: Default::default(),
        audio_preparation: Default::default(),
        preparing: Default::default(),
        transport: Default::default(),
        roaming_offered: Default::default(),
        claimed_until: None,
        nucleus: None,
        held: None,
        warnings: Default::default(),
    }
}
#[test]
fn connections_reset_the_idle_wait_including_during_downloads() {
    use butterpollo_media_policy::rtsp::Negotiated;
    use butterpollo_session::session::Role;
    let f = Fixture::new();
    let h = &f.host;
    for role in [Role::Stream, Role::RemoteMonitor, Role::InputOnly] {
        for phase in ["waiting", "downloading", "ready"] {
            *h.updates.lock().unwrap() = json!({"phase":phase,"idle_since":now()-120});
            h.sessions.lock().unwrap().queue(launch(role)).unwrap();
            poll(h);
            assert_eq!(h.updates.lock().unwrap()["phase"], phase);
            assert_eq!(h.updates.lock().unwrap()["idle_since"], 0);
            h.sessions
                .lock()
                .unwrap()
                .start(launch(role), Negotiated::default())
                .unwrap();
            poll(h);
            assert_eq!(h.updates.lock().unwrap()["idle_since"], 0);
            h.sessions.lock().unwrap().active.clear();
            poll(h);
            assert!(h.updates.lock().unwrap()["idle_since"].as_u64().unwrap() > 0);
            assert_eq!(h.updates.lock().unwrap()["phase"], phase);
        }
    }
}
#[test]
fn a_devices_install_now_waits_for_another_devices_stream_but_not_its_own() {
    let f = Fixture::new();
    let h = &f.host;
    // "fixture" streams; the phone asked, so its install waits, even once
    // the download is done and a stream starts later.
    h.sessions
        .lock()
        .unwrap()
        .queue(launch(butterpollo_session::session::Role::Stream))
        .unwrap();
    for phase in ["waiting", "ready"] {
        *h.updates.lock().unwrap() =
            json!({"phase":phase,"idle_since":0,"now":true,"now_by":"phone"});
        poll(h);
        assert_eq!(h.updates.lock().unwrap()["phase"], phase);
        assert_eq!(
            status(h)["waiting_for"],
            "fixture is starting a stream.",
            "{phase}"
        );
    }
    // The asking device's own stream ends: on to the installer, which
    // this test host, no installed service, refuses.
    *h.updates.lock().unwrap() =
        json!({"phase":"ready","idle_since":0,"now":true,"now_by":"fixture"});
    assert_eq!(status(h)["waiting_for"], Value::Null);
    poll(h);
    assert_eq!(h.updates.lock().unwrap()["phase"], "failed");
    // So does the console's, whoever streams.
    *h.updates.lock().unwrap() = json!({"phase":"ready","idle_since":0,"now":true});
    poll(h);
    assert_eq!(h.updates.lock().unwrap()["phase"], "failed");
}
#[test]
fn install_now_again_says_whether_it_changed_anything() {
    let mut state = json!({"phase":"waiting","now":false});
    assert!(install_now_again(&mut state, Some("phone")));
    assert_eq!(
        (&state["now"], &state["now_by"]),
        (&json!(true), &json!("phone"))
    );
    // The same device again: nothing new, so no second update.started.
    assert!(!install_now_again(&mut state, Some("phone")));
    // The console's ends any stream.
    assert!(install_now_again(&mut state, None));
    assert_eq!(state["now_by"], Value::Null);
    assert!(!install_now_again(&mut state, Some("phone")));
    assert_eq!(state["now_by"], Value::Null);
    let mut installing = json!({"phase":"installing","now":false});
    assert!(!install_now_again(&mut installing, Some("phone")));
    assert_eq!(installing["now"], false);
}
#[test]
fn install_now_does_not_wait_for_an_idle_host() {
    let f = Fixture::new();
    let h = &f.host;
    h.sessions
        .lock()
        .unwrap()
        .queue(launch(butterpollo_session::session::Role::Stream))
        .unwrap();
    *h.updates.lock().unwrap() = json!({"phase":"ready","idle_since":0,"now":true});
    poll(h);
    // Past the idle gate, straight to the installer; this test host is
    // no installed service, so that step fails.
    let state = h.updates.lock().unwrap();
    assert_eq!(state["phase"], "failed");
    assert!(
        state["error"]
            .as_str()
            .unwrap()
            .contains("installed Windows service"),
        "{state}"
    );
    drop(state);
    // Without it the busy host keeps the update waiting.
    *h.updates.lock().unwrap() = json!({"phase":"ready","idle_since":0});
    poll(h);
    assert_eq!(h.updates.lock().unwrap()["phase"], "ready");
    assert_eq!(h.updates.lock().unwrap()["idle_since"], 0);
}
#[test]
fn the_installer_is_hashed_with_no_lock_held_and_checked_again_after() {
    let f = Fixture::new();
    let h = &f.host;
    let ready = json!({"phase":"ready","idle_since":0,"now":true,
                       "download_id":"a","installer_path":"C:\\setup.exe"});
    let hashes = std::cell::Cell::new(0);
    // Unchanged meanwhile: on to the installer, which this test host,
    // no installed service, refuses.
    *h.updates.lock().unwrap() = ready.clone();
    poll_with(h, None, &|path| {
        hashes.set(hashes.get() + 1);
        assert_eq!(path, std::path::Path::new("C:\\setup.exe"));
        assert!(h.launch_transition.try_lock().is_ok());
        assert!(h.updates.try_lock().is_ok());
        Ok((1, "00".into()))
    });
    assert_eq!(hashes.get(), 1);
    let state = h.updates.lock().unwrap().clone();
    assert_eq!(state["phase"], "failed");
    assert!(
        state["error"]
            .as_str()
            .unwrap()
            .contains("installed Windows service")
    );
    // Cancelled and downloaded again while it was hashed: nothing starts,
    // and the next poll hashes the new download.
    *h.updates.lock().unwrap() = ready.clone();
    poll_with(h, None, &|_| {
        hashes.set(hashes.get() + 1);
        h.updates.lock().unwrap()["download_id"] = json!("b");
        Ok((1, "00".into()))
    });
    assert_eq!(hashes.get(), 2);
    let state = h.updates.lock().unwrap().clone();
    assert_eq!(state["phase"], "ready");
    assert_eq!(state["download_id"], "b");
    // A busy host by then keeps the update waiting.
    *h.updates.lock().unwrap() = json!({"phase":"ready","idle_since":now()-3600,
                                        "download_id":"a","installer_path":"C:\\setup.exe"});
    poll_with(h, None, &|_| {
        h.sessions
            .lock()
            .unwrap()
            .queue(launch(butterpollo_session::session::Role::Stream))
            .unwrap();
        Ok((1, "00".into()))
    });
    let state = h.updates.lock().unwrap().clone();
    assert_eq!(state["phase"], "ready");
    assert_eq!(state["idle_since"], 0);
}
#[test]
fn cancellation_invalidates_a_download_and_stops_automatic_requeue() {
    let f = Fixture::new();
    let h = &f.host;
    *h.updates.lock().unwrap() =
        json!({"phase":"downloading","download_id":"old","latest_version":"2.1.0"});
    cancel(h).unwrap();
    let state = h.updates.lock().unwrap();
    assert_eq!(state["phase"], "idle");
    assert!(state["download_id"].is_null());
    assert_eq!(state["attempted_version"], "2.1.0");
    drop(state);
    h.updates.lock().unwrap()["phase"] = json!("installing");
    assert!(cancel(h).is_err());
}
// The tidy stops looking once no download folder is left, which on an installed host is the first
// pass after the start. A download given up after that must start it again, or its folder stays
// until the host restarts. One test for both ways, as they share the process-wide timer.
#[test]
fn giving_up_a_download_starts_the_tidy_again() {
    let f = Fixture::new();
    let h = &f.host;
    NEXT_TIDY.store(u64::MAX, Ordering::Relaxed);
    *h.updates.lock().unwrap() = json!({"phase":"downloading","download_id":"old","latest_version":"2.1.0","partial":"setup.exe"});
    cancel(h).unwrap();
    assert!(
        NEXT_TIDY.load(Ordering::Relaxed) < u64::MAX,
        "a cancelled download's folder is looked for"
    );

    NEXT_TIDY.store(u64::MAX, Ordering::Relaxed);
    *h.updates.lock().unwrap() =
        json!({"phase":"waiting","automatic":true,"idle_since":0,"partial":"setup.exe"});
    poll(h);
    assert!(h.updates.lock().unwrap()["partial"].is_null());
    assert!(
        NEXT_TIDY.load(Ordering::Relaxed) < u64::MAX,
        "so is the one automatic updates left when turned off"
    );
}
#[test]
fn disabling_automatic_updates_cancels_an_automatic_queue_only() {
    let f = Fixture::new();
    let h = &f.host;
    for automatic in [true, false] {
        *h.updates.lock().unwrap() =
            json!({"phase":"waiting","automatic":automatic,"idle_since":0,"partial":"setup.exe"});
        poll(h);
        let state = h.updates.lock().unwrap();
        assert_eq!(state["phase"], if automatic { "idle" } else { "waiting" });
        // Turning automatic updates off is not a pause: its half-finished
        // download goes at the next tidy, not at the next restart.
        assert_eq!(
            state["partial"].is_null(),
            automatic,
            "automatic={automatic}"
        );
    }
    assert!(!status(h)["install_supported"].as_bool().unwrap());
    assert_eq!(status(h)["update_check_interval"], 86400);
    h.config
        .write()
        .unwrap()
        .values
        .insert("update_check_interval".into(), "0".into());
    assert_eq!(status(h)["update_check_interval"], 0);
    assert!(queue(h, false, false, None).is_err());
}
fn set_interval(h: &Shared, seconds: &str) {
    h.config
        .write()
        .unwrap()
        .values
        .insert("update_check_interval".into(), seconds.into());
}
#[test]
fn the_schedule_follows_the_wall_clock_and_the_current_interval() {
    let day = 86400;
    let t = 1_000_000_000;
    // Off, or never checked.
    assert_eq!(next_check_at(0, t, 0, t), None);
    assert_eq!(next_check_at(-1, t, 0, t), None);
    assert_eq!(next_check_at(day, 0, 0, t), Some(0));
    // Not due until a whole interval after the last check; the console's
    // floor of an hour applies to a smaller value.
    assert_eq!(
        next_check_at(day, t - 100, 0, t),
        Some(t - 100 + day as u64)
    );
    assert_eq!(next_check_at(60, t - 100, 0, t), Some(t - 100 + 3600));
    // A PC that slept for days is overdue the moment it wakes.
    assert!(next_check_at(day, t - 3 * 86400, 0, t).unwrap() <= t);
    // A clock set back must not postpone the check by the difference.
    assert_eq!(next_check_at(day, t + 5000, 0, t), Some(0));
    // A step of a second or two (time sync) is not a clock set back.
    assert_eq!(next_check_at(day, t + 1, 0, t), Some(t + 1 + day as u64));
    assert_eq!(
        next_check_at(day, t + 300, 0, t),
        Some(t + 300 + day as u64)
    );
    assert_eq!(next_check_at(day, t + 301, 0, t), Some(0));
    // A failed check (network not back after sleep) is retried within
    // minutes, not after a whole interval.
    let retry = next_check_at(day, t - 10, 1, t).unwrap();
    assert!(retry > t && retry <= t + 300, "{retry}");
}
#[test]
fn the_loop_sleeps_to_the_checks_own_deadline_not_a_seconds_poll() {
    let t = 1_000_000_000;
    // A check already due (a stream or one still running defers it) is
    // asked again after a second, as its poll was.
    assert_eq!(next_wait(t, t), Duration::from_secs(1));
    assert_eq!(next_wait(t - 1, t), Duration::from_secs(1));
    // An interval that has not passed is slept to, however far.
    assert_eq!(next_wait(t + 7, t), Duration::from_secs(7));
    // One longer than a day is looked at again after a day, so a slept
    // clock is caught without polling.
    assert_eq!(next_wait(t + 10 * 86_400, t), Duration::from_secs(86_400));
}
#[test]
fn repeated_failures_back_off_up_to_the_interval() {
    let t = 1_000_000_000;
    let day = 86400;
    let wait = |failures, interval| next_check_at(interval, t, failures, t + 1).unwrap() - t;
    assert_eq!(wait(1, day), 120);
    assert_eq!(wait(2, day), 240);
    assert_eq!(wait(3, day), 480);
    // Capped by the interval, and by the hour floor for a smaller one.
    assert_eq!(wait(20, day), day as u64);
    assert_eq!(wait(20, 7200), 7200);
    assert_eq!(wait(20, 60), 3600);
    // A huge count must not overflow the shift.
    assert_eq!(wait(u64::MAX, day), day as u64);
    // A success resets the count.
    assert_eq!(wait(0, day), day as u64);
}
#[test]
fn a_changed_interval_applies_without_a_restart_and_a_failed_check_is_retried() {
    let f = Fixture::new();
    let h = &f.host;
    // Checked two hours ago with the default daily interval: not due.
    let t = now();
    *h.updates.lock().unwrap() =
        json!({"checking":false,"check_failed":false,"checked_at":t - 7200});
    assert!(!check_due(h));
    assert_eq!(status(h)["next_check_at"], t - 7200 + 86400);
    // The user picks the minimum: due now, same running host.
    set_interval(h, "3600");
    assert!(check_due(h));
    // Turned off.
    set_interval(h, "0");
    assert!(!check_due(h));
    assert!(status(h)["next_check_at"].is_null());
    // A check in flight is not started again.
    set_interval(h, "3600");
    h.updates.lock().unwrap()["checking"] = json!(true);
    assert!(!check_due(h));
    // A check that failed a moment ago waits briefly; one that failed
    // minutes ago is retried even with a daily interval.
    set_interval(h, "86400");
    *h.updates.lock().unwrap() =
        json!({"checking":false,"check_failed":true,"check_failures":1,"checked_at":t - 10});
    assert!(!check_due(h));
    h.updates.lock().unwrap()["checked_at"] = json!(t - 600);
    assert!(check_due(h));
}
#[test]
fn a_stream_defers_the_check() {
    let f = Fixture::new();
    let h = &f.host;
    *h.updates.lock().unwrap() = json!({"checking":false,"checked_at":0});
    assert!(check_due(h));
    h.sessions
        .lock()
        .unwrap()
        .queue(launch(butterpollo_session::session::Role::Stream))
        .unwrap();
    assert!(!check_due(h));
    // Due but held back: the console says so.
    assert_eq!(status(h)["deferred"], true);
}
#[test]
fn a_lingering_game_or_monitor_does_not_defer_the_check() {
    let f = Fixture::new();
    let h = &f.host;
    *h.updates.lock().unwrap() = json!({"checking":false,"checked_at":0});
    // (A held virtual display is `busy` the same way, but a `Retained`
    // cannot be built without a display driver.)
    *h.current_app.lock().unwrap() = Some(
        crate::process::RunningApp::with_environment(
            &butterpollo_session::state::App::desktop(),
            Default::default(),
        )
        .unwrap(),
    );
    assert!(busy(h), "the install path still waits for an idle host");
    assert!(check_due(h));
    assert_eq!(status(h)["deferred"], false);
}
fn set_time(h: &Shared, time: &str) {
    h.config
        .write()
        .unwrap()
        .values
        .insert("update_check_time".into(), time.into());
}
/// The time this PC's clock showed `hours` ago, as the setting takes it.
fn hours_ago(hours: i64) -> String {
    let local = local_clock(now()) - hours * 3600;
    format!(
        "{:02}:{:02}",
        local.rem_euclid(86400) / 3600,
        local.rem_euclid(3600) / 60
    )
}
#[test]
fn a_time_of_day_decides_the_check_in_place_of_the_interval() {
    let f = Fixture::new();
    let h = &f.host;
    let t = now();
    let checked = |seconds_ago: u64| {
        *h.updates.lock().unwrap() =
            json!({"checking":false,"check_failed":false,"checked_at":t - seconds_ago});
    };
    assert!(status(h)["update_check_time"].is_null());
    // The time was four hours ago and the last check eight hours ago: due,
    // although the daily interval has not passed.
    let time = hours_ago(4);
    set_time(h, &time);
    checked(8 * 3600);
    assert!(check_due(h));
    assert_eq!(status(h)["update_check_time"], time);
    assert!(status(h)["next_check_at"].as_u64().unwrap() <= t);
    // Checked since: not again today, however short the interval.
    set_interval(h, "3600");
    checked(2 * 3600);
    assert!(!check_due(h));
    let next = status(h)["next_check_at"].as_u64().unwrap();
    assert!(next > t + 18 * 3600 && next < t + 22 * 3600, "{next}");
    // Without the time the interval decides again, same running host.
    set_time(h, "");
    assert!(check_due(h));
    assert!(status(h)["update_check_time"].is_null());
    // So does it with a time the host cannot read.
    for unreadable in ["3am", "25:00", "0300"] {
        set_time(h, unreadable);
        assert!(check_due(h), "{unreadable}");
        assert!(status(h)["update_check_time"].is_null(), "{unreadable}");
        assert_eq!(status(h)["next_check_at"], t - 2 * 3600 + 3600);
    }
    // An interval of 0 turns automatic checks off with a time as well.
    set_time(h, &time);
    set_interval(h, "0");
    checked(8 * 3600);
    assert!(!check_due(h));
    assert!(status(h)["next_check_at"].is_null());
    // A stream defers the daily check like any other.
    set_interval(h, "86400");
    assert!(check_due(h));
    h.sessions
        .lock()
        .unwrap()
        .queue(launch(butterpollo_session::session::Role::Stream))
        .unwrap();
    assert!(!check_due(h));
    assert_eq!(status(h)["deferred"], true);
}
#[test]
fn the_daily_check_counts_the_last_check_before_a_restart() {
    let f = Fixture::new();
    let h = &f.host;
    let t = now();
    let time = hours_ago(4);
    // Never checked: at once.
    set_time(h, &time);
    assert!(check_due(h));
    // A check two hours ago succeeded; then the host restarts.
    h.updates.lock().unwrap()["checked_at"] = json!(t - 2 * 3600);
    remember_check(h);
    let saved = butterpollo_session::state::load_json(&h.aliases_path, json!(null)).unwrap();
    assert_eq!(saved["root"][LAST_CHECK], t - 2 * 3600);
    let again =
        crate::state::Host::load(f.directory.clone(), f.directory.join("assets"), None).unwrap();
    assert_eq!(again.updates.lock().unwrap()["checked_at"], 0);
    // Today's check is done: the restarted host waits for tomorrow's time.
    set_time(&again, &time);
    assert!(!check_due(&again));
    assert!(status(&again)["next_check_at"].as_u64().unwrap() > t);
    // With an interval it checks at every start, as before.
    set_time(&again, "");
    assert!(check_due(&again));
    // A restart after the time passed catches the check up.
    again.aliases.lock().unwrap()["root"][LAST_CHECK] = json!(t - 8 * 3600);
    set_time(&again, &time);
    assert!(check_due(&again));
    // A check that failed in this run is retried by its own time, not
    // held back by the one remembered.
    again.aliases.lock().unwrap()["root"][LAST_CHECK] = json!(t - 2 * 3600);
    *again.updates.lock().unwrap() =
        json!({"checking":false,"check_failed":true,"check_failures":1,"checked_at":t - 600});
    assert!(check_due(&again));
}
#[tokio::test]
#[ignore = "downloads the official public installer; never executes it"]
async fn official_release_download_verifies_with_github_digest() -> Result<()> {
    let f = Fixture::new();
    let releases: Vec<Value> = super::releases(&client(Duration::from_secs(35))?)
        .await?
        .json()
        .await?;
    let candidate = releases
        .iter()
        .find_map(|r| installer(r).ok())
        .context("no release installer")?;
    f.host.updates.lock().unwrap()["download_id"] = json!("download-test");
    let path = f.directory.join("setup.exe");
    transfer(&f.host, "download-test", &candidate, &path).await?;
    assert_eq!(std::fs::metadata(&path)?.len(), candidate.size);
    eprintln!(
        "Verified {}: {} bytes, SHA256 {}",
        candidate.version, candidate.size, candidate.digest
    );
    Ok(())
}
