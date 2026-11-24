//! Native API per-app Radeon profile application, takeover, and restoration tests.
use super::*;

fn smooth_driver() -> Fake {
    only(vec![
        toggle("frame_rate_target.enabled", State::Configurable, false),
        integer("frame_rate_target.fps", State::Configurable, 60),
        toggle("anti_lag.enabled", State::Configurable, false),
    ])
}

#[tokio::test]
async fn a_launched_profile_is_applied_shown_to_the_owner_and_restored_with_the_session() {
    let rig = Rig::new();
    let _stream = rig.owner();
    let app = rig.app_with(json!({"radeon-profile": "competitive"}));
    rig.launched(&app, Some(60));
    let status = rig.settled();
    assert_eq!(status.state, ProfileState::Applied, "{status:?}");
    assert_eq!(status.profile, "competitive");
    assert_eq!(status.settings, ["anti_lag.enabled"]);
    assert_eq!(rig.fake.value("anti_lag.enabled"), Raw::Bool(true));
    assert_eq!(rig.journal().len(), 1);

    // The session of the owner says so, and its revision moved with it.
    let session = rig.get("/session").await.session();
    assert_eq!(session.radeon_profile, Some(status));
    assert!(session.revision > 1);

    let ended = rig.post("/session/end").await.session();
    assert_eq!(ended.state, SessionState::Idle);
    assert!(ended.radeon_profile.is_none());
    rig.flush();
    assert_eq!(rig.fake.value("anti_lag.enabled"), Raw::Bool(false));
    assert_eq!(rig.journal(), vec![]);
}

#[tokio::test]
async fn smoothness_caps_the_game_at_the_stream_rate() {
    let rig = Rig::with(smooth_driver());
    let _stream = rig.owner();
    let app = rig.app_with(json!({"radeon-profile": "smoothness"}));
    rig.launched(&app, Some(120));
    assert_eq!(rig.settled().state, ProfileState::Applied);
    assert_eq!(rig.fake.value("frame_rate_target.enabled"), Raw::Bool(true));
    assert_eq!(rig.fake.value("frame_rate_target.fps"), Raw::Int(120));
    // The toggle was written first, as the driver needs.
    let writes = rig.fake.writes();
    assert!(
        writes
            .iter()
            .position(|w| w.starts_with("frame_rate_target.enabled"))
            < writes
                .iter()
                .position(|w| w.starts_with("frame_rate_target.fps"))
    );
    rig.post("/session/end").await;
    rig.flush();
    assert_eq!(rig.fake.value("frame_rate_target.fps"), Raw::Int(60));
    assert_eq!(
        rig.fake.value("frame_rate_target.enabled"),
        Raw::Bool(false)
    );
}

#[tokio::test]
async fn a_custom_profile_writes_its_own_settings_and_nothing_else() {
    let rig = Rig::with(smooth_driver());
    let _stream = rig.owner();
    let app = rig.app_with(json!({
        "radeon-profile": "custom",
        "radeon-settings": {"frame_rate_target.fps": 90, "frame_rate_target.enabled": true}
    }));
    rig.launched(&app, Some(60));
    assert_eq!(rig.settled().state, ProfileState::Applied);
    assert_eq!(rig.fake.value("frame_rate_target.fps"), Raw::Int(90));
    assert_eq!(rig.fake.value("anti_lag.enabled"), Raw::Bool(false));
}

#[tokio::test]
async fn a_profile_the_driver_refuses_is_reported_loudly_and_the_game_goes_on() {
    let rig = Rig::new();
    rig.fake.with(|d| d.refuse = Some("anti_lag.enabled"));
    let _stream = rig.owner();
    let app = rig.app_with(json!({"radeon-profile": "competitive"}));
    rig.launched(&app, Some(60));
    let status = rig.settled();
    assert_eq!(status.state, ProfileState::Failed);
    assert!(
        status
            .message
            .as_deref()
            .is_some_and(|m| m.contains("anti_lag.enabled")),
        "{status:?}"
    );
    assert!(status.settings.is_empty());
    assert_eq!(rig.fake.value("anti_lag.enabled"), Raw::Bool(false));
    // The client sees it; the session itself is untouched.
    let session = rig.get("/session").await.session();
    assert_eq!(session.state, SessionState::Streaming);
    assert_eq!(session.radeon_profile.unwrap().state, ProfileState::Failed);
}

#[tokio::test]
async fn a_profile_naming_a_setting_this_gpu_lacks_fails_without_writing_anything() {
    let rig = Rig::new();
    let _stream = rig.owner();
    // `driver()` has no frame rate target.
    let app = rig.app_with(json!({"radeon-profile": "smoothness"}));
    rig.launched(&app, Some(60));
    let status = rig.settled();
    assert_eq!(status.state, ProfileState::Failed);
    let message = status.message.unwrap();
    assert!(
        message.contains("this GPU has no Radeon setting"),
        "{message}"
    );
    assert!(message.contains("frame_rate_target.enabled"), "{message}");
    assert_eq!(rig.fake.writes(), Vec::<String>::new());
    assert_eq!(rig.journal(), vec![]);
}

#[tokio::test]
async fn a_resume_of_the_same_app_does_not_write_again_but_a_new_session_does() {
    let rig = Rig::new();
    let _stream = rig.owner();
    let app = rig.app_with(json!({"radeon-profile": "competitive"}));
    rig.launched(&app, Some(60));
    assert_eq!(rig.settled().state, ProfileState::Applied);
    let writes = rig.fake.writes().len();
    rig.launched(&app, Some(60));
    rig.launched(&app, Some(60));
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(rig.fake.writes().len(), writes, "a resume writes nothing");

    // After the session ended the settings went back, and the next launch applies them again.
    rig.post("/session/end").await;
    rig.flush();
    assert_eq!(rig.fake.value("anti_lag.enabled"), Raw::Bool(false));
    let _again = rig.streaming();
    rig.launched(&app, Some(60));
    assert_eq!(rig.settled().state, ProfileState::Applied);
    assert_eq!(rig.fake.value("anti_lag.enabled"), Raw::Bool(true));
}

#[tokio::test]
async fn launching_an_app_without_a_profile_puts_the_earlier_game_settings_back() {
    let rig = Rig::new();
    let _stream = rig.owner();
    let app = rig.app_with(json!({"radeon-profile": "competitive"}));
    rig.launched(&app, Some(60));
    assert_eq!(rig.settled().state, ProfileState::Applied);
    assert_eq!(rig.fake.value("anti_lag.enabled"), Raw::Bool(true));

    let mut plain = app.clone();
    plain.extra.remove("radeon-profile");
    // The launch returns at once; the earlier settings go back on the profile thread.
    rig.launched(&plain, Some(60));
    until("the earlier settings never went back", || {
        rig.fake.value("anti_lag.enabled") == Raw::Bool(false) && rig.journal().is_empty()
    });
    assert!(rig.host().notes.profile(&rig.client.uuid).is_none());
    assert!(rig.get("/session").await.session().radeon_profile.is_none());
}

/// Profile threads run one at a time per host, and no other host's wait for them. The tests run
/// many hosts in one process: with one lock for the process, a profile held up behind a slow
/// driver held up every other test's.
#[tokio::test]
async fn profile_threads_wait_only_for_their_own_hosts() {
    let busy = Rig::new();
    let rig = Rig::new();
    let (_busy_stream, _stream) = (busy.owner(), rig.owner());
    let busy_app = busy.app_with(json!({"radeon-profile": "competitive"}));
    let app = rig.app_with(json!({"radeon-profile": "competitive"}));
    // As a profile thread of the busy host that is still writing.
    let writing = busy.host().radeon.profiles.lock().unwrap();
    busy.launched(&busy_app, Some(60));
    rig.launched(&app, Some(60));
    assert_eq!(rig.settled().state, ProfileState::Applied);
    assert_eq!(rig.fake.value("anti_lag.enabled"), Raw::Bool(true));
    let held = busy.host().notes.profile(&busy.client.uuid).unwrap();
    assert_eq!(held.status.state, ProfileState::Applying);
    assert_eq!(busy.fake.value("anti_lag.enabled"), Raw::Bool(false));
    drop(writing);
    assert_eq!(busy.settled().state, ProfileState::Applied);
    assert_eq!(busy.fake.value("anti_lag.enabled"), Raw::Bool(true));
}

#[tokio::test]
async fn an_app_without_a_profile_never_touches_the_driver() {
    let rig = Rig::new();
    let _stream = rig.owner();
    let reads = rig.fake.reads();
    let app = rig.app_with(json!({}));
    rig.launched(&app, Some(60));
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(rig.fake.writes(), Vec::<String>::new());
    assert_eq!(rig.fake.reads(), reads);
    assert!(rig.get("/session").await.session().radeon_profile.is_none());
}

#[tokio::test]
async fn an_unreadable_profile_in_a_hand_edited_app_fails_loudly() {
    let rig = Rig::new();
    let _stream = rig.owner();
    let app = rig.app_with(json!({"radeon-profile": "fastest"}));
    rig.launched(&app, Some(60));
    let status = rig.settled();
    assert_eq!(status.state, ProfileState::Failed);
    assert!(status.message.unwrap().contains("radeon-profile"));
    assert_eq!(rig.fake.writes(), Vec::<String>::new());
}

#[tokio::test]
async fn another_device_sees_no_profile() {
    let rig = Rig::new();
    let _stream = rig.owner();
    let app = rig.app_with(json!({"radeon-profile": "competitive"}));
    rig.launched(&app, Some(60));
    assert_eq!(rig.settled().state, ProfileState::Applied);
    let snapshot = rig.host().session_snapshot();
    assert!(snapshot.radeon_profile.is_some());
    let mut other = rig.client.clone();
    other.uuid = "someone-else".into();
    let seen = butterpollo_session::native::session_view(&snapshot, &other);
    assert!(seen.radeon_profile.is_none());
}

// Review fixes: who is released before a profile is written, what a profile's restore touches,
// and how a profile ends with the session.

fn custom_pacing(rig: &Rig, value: i64) -> butterpollo_session::state::App {
    rig.app_with(json!({
        "radeon-profile": "custom",
        "radeon-settings": {"pacing.limit": value}
    }))
}

#[tokio::test]
async fn a_takeover_puts_the_departed_devices_settings_back_before_the_new_profile_is_written() {
    let rig = Rig::new();
    let first = rig.changed().await;
    // The other device's launch is accepted: the first device's stream and kept display are gone
    // and the session is still launching, so no sync has released it yet.
    let launch = rig.host().session_begin("other-device", None);
    first.stop();
    drop(first);
    drop(
        rig.host()
            .app_display
            .lock()
            .unwrap()
            .remove("parity-device"),
    );
    rig.game_owned_by("other-device");
    assert_eq!(rig.state(), SessionState::Preparing);
    assert!(rig.host().radeon.holds("parity-device"));

    let app = custom_pacing(&rig, 210);
    rig.launched_by("other-device", &app, Some(60));
    assert_eq!(rig.settled_for("other-device").state, ProfileState::Applied);
    // The first device's value went back first, so the journal's original is the PC's own and
    // nothing of the first device can undo the new write later.
    assert_eq!(
        rig.fake.writes(),
        ["pacing.limit=200", "pacing.limit=140", "pacing.limit=210"]
    );
    let journal = rig.journal();
    assert_eq!(journal.len(), 1, "{journal:?}");
    assert_eq!(journal[0].session, "other-device");
    assert_eq!(journal[0].original, Raw::Int(140));
    assert_eq!(journal[0].written, Raw::Int(210));
    assert!(!rig.host().radeon.holds("parity-device"));
    rig.flush();
    assert_eq!(rig.fake.value("pacing.limit"), Raw::Int(210));
    drop(launch);
    rig.put_back("other-device");
    rig.assert_restored();
}

#[tokio::test]
async fn a_takeover_waits_for_a_restore_that_is_already_queued() {
    let rig = Rig::new();
    let _first = rig.changed().await;
    // The first session ends and its restore is queued, but the restore thread is busy.
    let pause = rig.host().radeon.pause_restores();
    rig.post("/session/end").await.session();
    // The other device's launch is accepted and runs while the restore waits.
    let _launch = rig.host().session_begin("other-device", None);
    rig.game_owned_by("other-device");
    let app = custom_pacing(&rig, 210);
    rig.launched_by("other-device", &app, Some(60));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        rig.host()
            .notes
            .profile("other-device")
            .unwrap()
            .status
            .state,
        ProfileState::Applying
    );
    assert_eq!(
        rig.fake.writes(),
        ["pacing.limit=200"],
        "nothing is written yet"
    );
    drop(pause);
    assert_eq!(rig.settled_for("other-device").state, ProfileState::Applied);
    assert_eq!(
        rig.fake.writes(),
        ["pacing.limit=200", "pacing.limit=140", "pacing.limit=210"]
    );
    let journal = rig.journal();
    assert_eq!(journal.len(), 1, "{journal:?}");
    assert_eq!(journal[0].original, Raw::Int(140));
    rig.put_back("other-device");
    rig.assert_restored();
}

#[tokio::test]
async fn changing_games_puts_back_only_what_the_profile_wrote_not_the_clients_panel_writes() {
    let rig = Rig::new();
    let _stream = rig.changed().await;
    let app = rig.app_with(json!({"radeon-profile": "competitive"}));
    rig.launched(&app, Some(60));
    assert_eq!(rig.settled().state, ProfileState::Applied);
    assert_eq!(rig.journal().len(), 2);

    let mut plain = app.clone();
    plain.extra.remove("radeon-profile");
    rig.launched(&plain, Some(60));
    until("the profile never went back", || {
        rig.fake.value("anti_lag.enabled") == Raw::Bool(false) && rig.journal().len() == 1
    });
    // The client's own write stays until its session ends.
    assert_eq!(rig.fake.value("pacing.limit"), Raw::Int(200));
    assert_eq!(rig.journal()[0].id, "pacing.limit");
    assert!(rig.host().radeon.holds("parity-device"));
    rig.post("/session/end").await.session();
    rig.flush();
    rig.assert_restored();
}

#[tokio::test]
async fn a_launch_returns_before_the_earlier_settings_are_back() {
    let rig = Rig::new();
    let _stream = rig.owner();
    let app = rig.app_with(json!({"radeon-profile": "competitive"}));
    rig.launched(&app, Some(60));
    assert_eq!(rig.settled().state, ProfileState::Applied);
    // The driver is slow; the launch must not wait for it.
    rig.fake.with(|d| d.hold = true);
    let mut plain = app.clone();
    plain.extra.remove("radeon-profile");
    let started = Clock::now();
    rig.launched(&plain, Some(60));
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the launch waited for the driver"
    );
    assert!(
        rig.host().notes.profile(&rig.client.uuid).is_none(),
        "the note goes at once"
    );
    rig.fake.with(|d| d.hold = false);
    until("the earlier settings never went back", || {
        rig.fake.value("anti_lag.enabled") == Raw::Bool(false) && rig.journal().is_empty()
    });
}

#[tokio::test]
async fn a_newer_launch_supersedes_an_older_profile_that_was_still_applying() {
    let rig = Rig::new();
    let _stream = rig.owner();
    let first = rig.app_with(json!({"radeon-profile": "competitive"}));
    // The first profile's thread is stuck behind a slow driver when the next launch comes.
    rig.fake.with(|d| d.hold = true);
    rig.launched(&first, Some(60));
    std::thread::sleep(Duration::from_millis(100));
    let second = custom_pacing(&rig, 210);
    rig.launched(&second, Some(60));
    rig.fake.with(|d| d.hold = false);
    let status = rig.settled();
    assert_eq!(status.profile, "custom");
    assert_eq!(status.state, ProfileState::Applied, "{status:?}");
    // Only the second profile's settings are in place; the first one's are back.
    until("the first profile never went back", || {
        rig.fake.value("anti_lag.enabled") == Raw::Bool(false)
            && rig
                .journal()
                .iter()
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>()
                == ["pacing.limit"]
    });
    assert_eq!(rig.fake.value("pacing.limit"), Raw::Int(210));
    assert_eq!(
        rig.host()
            .notes
            .profile(&rig.client.uuid)
            .unwrap()
            .status
            .profile,
        "custom"
    );
}

#[tokio::test]
async fn a_profile_still_writing_when_the_session_ends_is_put_back_and_its_note_cleared() {
    let rig = Rig::new();
    let _stream = rig.streaming();
    let app = rig.app_with(json!({"radeon-profile": "competitive"}));
    rig.fake.with(|d| d.hold = true);
    rig.launched(&app, Some(60));
    std::thread::sleep(Duration::from_millis(150));
    // The session ends while the driver is still busy with the first call.
    rig.post("/session/end").await.session();
    rig.fake.with(|d| d.hold = false);
    until("the profile was left on the driver", || {
        rig.fake.value("anti_lag.enabled") == Raw::Bool(false)
            && rig.journal().is_empty()
            && rig.host().notes.profile(&rig.client.uuid).is_none()
    });
    assert!(rig.get("/session").await.session().radeon_profile.is_none());
}

#[tokio::test]
async fn a_launch_that_never_connects_gives_the_profile_back_and_clears_its_note() {
    let rig = Rig::new();
    // The launch is accepted and the profile applied, but no stream ever starts.
    let launch = rig.host().session_begin(&rig.client.uuid, None);
    let app = rig.app_with(json!({"radeon-profile": "competitive"}));
    rig.launched(&app, Some(60));
    assert_eq!(rig.settled().state, ProfileState::Applied);
    assert_eq!(rig.fake.value("anti_lag.enabled"), Raw::Bool(true));
    // Nothing is taken back while the launch is still running.
    rig.host().session_tick();
    rig.flush();
    assert_eq!(rig.fake.value("anti_lag.enabled"), Raw::Bool(true));
    // The launch fails and gives the session back: nobody is part of it.
    drop(launch);
    rig.host().session_tick();
    until(
        "the profile of a launch that never connected stayed",
        || {
            rig.fake.value("anti_lag.enabled") == Raw::Bool(false)
                && rig.journal().is_empty()
                && rig.host().notes.profile(&rig.client.uuid).is_none()
        },
    );
    assert_eq!(rig.state(), SessionState::Idle);
}

#[tokio::test]
async fn disconnect_keeps_the_profile_and_its_note_until_the_session_ends() {
    let rig = Rig::new();
    let _stream = rig.streaming();
    let app = rig.app_with(json!({"radeon-profile": "competitive"}));
    rig.launched(&app, Some(60));
    assert_eq!(rig.settled().state, ProfileState::Applied);
    let suspended = rig.post("/session/disconnect").await.session();
    assert_eq!(suspended.state, SessionState::Suspended);
    rig.flush();
    assert_eq!(rig.fake.value("anti_lag.enabled"), Raw::Bool(true));
    assert_eq!(rig.journal().len(), 1);
    assert_eq!(
        suspended.radeon_profile.map(|p| p.state),
        Some(ProfileState::Applied)
    );
    assert_eq!(
        rig.host()
            .notes
            .profile(&rig.client.uuid)
            .unwrap()
            .status
            .state,
        ProfileState::Applied
    );
    rig.post("/session/end").await.session();
    rig.flush();
    assert_eq!(rig.fake.value("anti_lag.enabled"), Raw::Bool(false));
    assert!(rig.host().notes.profile(&rig.client.uuid).is_none());
}

#[tokio::test]
async fn a_profile_that_needs_the_stream_rate_is_refused_when_the_launch_states_none() {
    let rig = Rig::with(smooth_driver());
    let _stream = rig.owner();
    let app = rig.app_with(json!({"radeon-profile": "smoothness"}));
    rig.launched(&app, None);
    let status = rig.settled();
    assert_eq!(status.state, ProfileState::Failed);
    let message = status.message.unwrap();
    assert!(
        message.contains("frame_rate_target.fps") && message.contains("frame rate"),
        "{message}"
    );
    assert_eq!(rig.fake.writes(), Vec::<String>::new());
    assert_eq!(rig.journal(), vec![]);
    // A profile that does not follow the rate is still applied without one.
    let competitive = rig.app_with(json!({"radeon-profile": "competitive"}));
    rig.launched(&competitive, None);
    assert_eq!(rig.settled().profile, "competitive");
}

#[tokio::test]
async fn an_apps_profile_needs_no_host_control_on_the_device() {
    let rig = Rig::new();
    // The admin's app config is the consent: the device was never granted host control.
    let _stream = rig.streaming();
    let paired = rig.host().paired.read().unwrap().clients[0].extra.clone();
    assert!(!paired.contains_key("host_control"));
    let app = rig.app_with(json!({"radeon-profile": "competitive"}));
    rig.launched(&app, Some(60));
    assert_eq!(rig.settled().state, ProfileState::Applied);
    assert_eq!(rig.fake.value("anti_lag.enabled"), Raw::Bool(true));
}
