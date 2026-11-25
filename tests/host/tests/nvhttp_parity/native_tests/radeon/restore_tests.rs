//! Native API Radeon session-end restore, journal recovery, and write/restore ordering tests.
use super::*;

#[tokio::test]
async fn end_session_puts_the_changes_back() {
    let rig = Rig::new();
    let stream = rig.changed().await;
    let ended = rig.post("/session/end").await.session();
    assert_eq!(ended.state, SessionState::Idle);
    rig.flush();
    rig.assert_restored();
    assert!(stream.stopping());
    // The panel no longer shows a pending change.
    rig.host().radeon.forget();
    assert_eq!(setting(&rig.panel().await, "pacing.limit").restore, None);
}

#[tokio::test]
async fn close_game_puts_the_changes_back() {
    let rig = Rig::new();
    let _stream = rig.changed().await;
    let closed = rig.post("/session/close-game").await.session();
    assert_eq!(closed.state, SessionState::Idle);
    rig.flush();
    rig.assert_restored();
}

#[tokio::test]
async fn disconnect_keeps_the_changes_through_the_grace_period() {
    let rig = Rig::new();
    let _stream = rig.changed().await;
    let suspended = rig.post("/session/disconnect").await.session();
    assert_eq!(suspended.state, SessionState::Suspended);
    rig.flush();
    rig.assert_kept();
    // Still the owner's, still shown, still writable.
    let panel = rig.panel().await;
    assert!(panel.writable_now);
    assert_eq!(
        setting(&panel, "pacing.limit").restore,
        Some(ApiRestore {
            original: Api::Integer(140),
            written: Api::Integer(200)
        })
    );
    // The same for a repeated Disconnect and a session whose stream ended by itself.
    rig.post("/session/disconnect").await.session();
    rig.flush();
    rig.assert_kept();
    // The grace period then ends the session and the change goes back (next test).
}

#[tokio::test]
async fn the_grace_period_running_out_puts_the_changes_back() {
    let rig = Rig::new();
    *rig.host().config.write().unwrap() = Config::parse("dd_config_revert_delay=0").unwrap();
    let stream = rig.changed().await;
    stream.stop();
    assert_eq!(rig.state(), SessionState::Suspended);
    rig.flush();
    rig.assert_kept();
    rig.host().reap_paused_display();
    assert_eq!(rig.state(), SessionState::Idle);
    rig.flush();
    rig.assert_restored();
}

#[tokio::test]
async fn unpairing_the_owner_puts_the_changes_back() {
    let rig = Rig::new();
    let _stream = rig.changed().await;
    let reply = send(
        &rig.f,
        true,
        &rig.connection,
        "POST",
        "/unpair",
        None,
        vec![],
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK);
    rig.flush();
    rig.assert_restored();
}

#[tokio::test]
async fn a_host_shutdown_puts_the_changes_back_and_waits_for_it() {
    let rig = Rig::new();
    let _stream = rig.changed().await;
    // The stream is still running: a shutdown ends it.
    rig.host().radeon.shutdown(Duration::from_secs(15));
    rig.assert_restored();
}

#[tokio::test]
async fn a_new_stream_of_the_same_device_after_end_starts_clean() {
    let rig = Rig::new();
    let _first = rig.changed().await;
    rig.post("/session/end").await.session();
    rig.flush();
    rig.assert_restored();
    // The next session writes again, and its end restores again.
    let _second = rig.streaming();
    radeon(
        &rig.write(json!([{"id": "pacing.limit", "value": 210}]))
            .await,
    );
    assert_eq!(rig.fake.value("pacing.limit"), Raw::Int(210));
    rig.post("/session/end").await.session();
    rig.flush();
    rig.assert_restored();
}

#[tokio::test]
async fn a_setting_changed_on_the_pc_is_not_put_back() {
    let rig = Rig::new();
    let _stream = rig.changed().await;
    // Someone moved the slider in Adrenalin during the session.
    rig.fake.set("pacing.limit", Raw::Int(150));
    rig.post("/session/end").await.session();
    rig.flush();
    assert_eq!(rig.fake.value("pacing.limit"), Raw::Int(150));
    assert_eq!(rig.journal(), vec![]);
    assert_eq!(rig.fake.writes(), vec!["pacing.limit=200".to_string()]);
}

#[tokio::test]
async fn a_failed_restore_is_reported_stays_journaled_and_the_restore_endpoint_retries_it() {
    let rig = Rig::new();
    let _stream = rig.changed().await;
    rig.fake.with(|d| d.refuse_all = true);
    let reply = rig.post("/radeon/restore").await;
    reply.assert_error(503, "temporarily_unavailable");
    assert_eq!(reply.json()["error"]["retry_after_ms"], 60_000);
    assert!(
        reply.json()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("pacing.limit")
    );
    rig.assert_kept();
    // The panel tells a client the change is still there and why.
    let panel = rig.panel().await;
    assert_eq!(panel.reason.as_deref(), Some("restore_failed"));
    assert!(panel.message.is_some());
    let pacing = setting(&panel, "pacing.limit");
    assert_eq!(pacing.reason.as_deref(), Some("restore_failed"));
    assert!(pacing.message.as_deref().unwrap().contains("140"));
    assert_eq!(
        pacing.restore,
        Some(ApiRestore {
            original: Api::Integer(140),
            written: Api::Integer(200)
        })
    );
    // The driver recovers; the owner asks again.
    rig.fake.with(|d| d.refuse_all = false);
    let panel = radeon(&rig.post("/radeon/restore").await);
    rig.assert_restored();
    assert_eq!(panel.reason, None);
    assert_eq!(panel.message, None);
    assert_eq!(setting(&panel, "pacing.limit").restore, None);
    assert_eq!(
        setting(&panel, "pacing.limit").value,
        Some(Api::Integer(140))
    );
    assert!(panel.writable_now, "the session goes on");
}

#[tokio::test]
async fn a_restore_that_fails_at_the_end_of_a_session_stays_journaled() {
    let rig = Rig::new();
    let _stream = rig.changed().await;
    rig.fake.with(|d| d.refuse_all = true);
    rig.post("/session/end").await.session();
    rig.flush();
    rig.assert_kept();
    let panel = rig.panel().await;
    assert_eq!(panel.reason.as_deref(), Some("restore_failed"));
    assert_eq!(
        setting(&panel, "pacing.limit")
            .restore
            .as_ref()
            .map(|r| &r.original),
        Some(&Api::Integer(140))
    );
    // Nobody owns a session now, so nothing is writable, and the host still holds the entry for
    // its next attempt.
    assert!(!panel.writable_now);
    rig.fake.with(|d| d.refuse_all = false);
    let worker = rig.host().radeon.worker().unwrap();
    assert!(worker.restore_session("parity-device").unwrap().is_clean());
    rig.assert_restored();
}

#[tokio::test]
async fn the_restore_endpoint_puts_the_changes_back_and_the_session_goes_on() {
    let rig = Rig::new();
    let stream = rig.changed().await;
    let panel = radeon(&rig.post("/radeon/restore").await);
    rig.assert_restored();
    assert_eq!(setting(&panel, "pacing.limit").restore, None);
    assert!(panel.writable_now);
    assert!(!stream.stopping());
    assert_eq!(rig.state(), SessionState::Streaming);
    // Nothing to put back is fine.
    radeon(&rig.post("/radeon/restore").await);
    // A write after a restore is put back at the end of the session again.
    radeon(
        &rig.write(json!([{"id": "pacing.limit", "value": 220}]))
            .await,
    );
    rig.post("/session/end").await.session();
    rig.flush();
    rig.assert_restored();
}

#[tokio::test]
async fn a_journal_from_a_crashed_run_is_restored_when_the_host_starts() {
    let s = Setup::new();
    let fake = driver();
    // The crash left the driver at 200 and the journal holding 140.
    fake.set("pacing.limit", Raw::Int(200));
    let journal = Journal {
        entries: vec![Entry {
            gpu: GPU,
            id: "pacing.limit".into(),
            session: "parity-device".into(),
            original: Raw::Int(140),
            written: Raw::Int(200),
            raw: None,
            side_effect: false,
            user: butterpollo_win_io::process::user_sid().unwrap(),
        }],
        ..Default::default()
    };
    let path = s.host().directory.join(JOURNAL_FILE);
    journal.save(&path).unwrap();
    // The driver is slow to answer, so the recovery is visible.
    fake.with(|d| d.hold = true);
    let rig = Rig::start(s, fake.clone());
    let reply = rig.get("/radeon").await;
    let panel = radeon(&reply);
    assert_eq!(panel.state, ControlState::TemporarilyUnavailable);
    assert_eq!(panel.reason.as_deref(), Some("recovering"));
    assert!(panel.gpus.is_empty());
    rig.host_control(true);
    fake.with(|d| d.hold = false);
    rig.ready();
    assert_eq!(fake.value("pacing.limit"), Raw::Int(140));
    assert_eq!(rig.journal(), vec![]);
    rig.host().radeon.forget();
    let panel = rig.panel().await;
    assert_eq!(panel.state, ControlState::Configurable);
    assert_eq!(
        setting(&panel, "pacing.limit").value,
        Some(Api::Integer(140))
    );
    assert_eq!(setting(&panel, "pacing.limit").restore, None);
}

/// A second paired device that streams as well, and its connection.
fn second_streamer(rig: &Rig) -> (Arc<Session>, Connection) {
    let connection = other_device(&rig.f, ALL, true);
    let client = rig
        .host()
        .paired
        .read()
        .unwrap()
        .clients
        .iter()
        .find(|client| client.uuid == "other-device")
        .cloned()
        .unwrap();
    (rig.stream_as(client), connection)
}

#[tokio::test]
async fn a_launch_by_another_device_that_fails_leaves_the_owners_settings_alone() {
    let rig = Rig::new();
    let _stream = rig.changed().await;
    let launch = rig.host().session_begin("other-device", None);
    // The other device owns the session while it prepares, and the owner's settings stay.
    assert_eq!(rig.state(), SessionState::Preparing);
    rig.flush();
    rig.assert_kept();
    assert!(rig.host().radeon.holds("parity-device"));
    // The launch fails and gives the session back.
    drop(launch);
    assert_eq!(rig.state(), SessionState::Streaming);
    rig.flush();
    rig.assert_kept();
    assert!(rig.host().radeon.holds("parity-device"));
    // The owner's own end of the session still puts them back.
    rig.post("/session/end").await.session();
    rig.flush();
    rig.assert_restored();
}

#[tokio::test]
async fn disconnect_with_two_streamers_restores_nothing() {
    let rig = Rig::new();
    let _first = rig.changed().await;
    let (_second, other) = second_streamer(&rig);
    let after = rig.post("/session/disconnect").await.session();
    // The other device's stream is the session now, and the first one keeps its display.
    assert_eq!(after.state, SessionState::Streaming);
    assert_eq!(
        rig.host().session_snapshot().owner.as_deref(),
        Some("other-device")
    );
    rig.flush();
    rig.assert_kept();
    assert!(rig.host().radeon.holds("parity-device"));
    // The other device ends its session, but the first one still has its display kept for the
    // game, so its settings stay until that display goes too.
    api(&rig.f, &other, "POST", "/session/end").await.session();
    rig.flush();
    rig.assert_kept();
    drop(
        rig.host()
            .app_display
            .lock()
            .unwrap()
            .remove("parity-device"),
    );
    rig.host().session_tick();
    rig.flush();
    rig.assert_restored();
}

#[tokio::test]
async fn a_manual_restore_that_fails_leaves_the_session_to_try_again_at_its_end() {
    let rig = Rig::new();
    let _stream = rig.changed().await;
    rig.fake.with(|d| d.refuse_all = true);
    rig.post("/radeon/restore")
        .await
        .assert_error(503, "temporarily_unavailable");
    rig.assert_kept();
    assert!(rig.host().radeon.holds("parity-device"));
    rig.fake.with(|d| d.refuse_all = false);
    rig.post("/session/end").await.session();
    rig.flush();
    rig.assert_restored();
}

#[tokio::test]
async fn a_restore_waiting_in_the_queue_does_not_undo_a_newer_write() {
    let rig = Rig::new();
    let _first = rig.changed().await;
    // The restore thread is busy when the session ends, and the device writes again meanwhile.
    let pause = rig.host().radeon.pause_restores();
    rig.post("/session/end").await.session();
    let _second = rig.streaming();
    radeon(
        &rig.write(json!([{"id": "pacing.limit", "value": 210}]))
            .await,
    );
    drop(pause);
    rig.flush();
    assert_eq!(rig.fake.value("pacing.limit"), Raw::Int(210));
    assert_eq!(rig.journal().len(), 1);
    // The new session's end puts back what the new session wrote.
    rig.post("/session/end").await.session();
    rig.flush();
    rig.assert_restored();
}

#[tokio::test]
async fn a_failed_restore_stops_showing_when_the_workers_retry_puts_the_setting_back() {
    let rig = Rig::quick_retry();
    let _stream = rig.changed().await;
    rig.fake.with(|d| d.refuse_all = true);
    rig.post("/session/end").await.session();
    rig.flush();
    rig.assert_kept();
    rig.host().radeon.forget();
    assert_eq!(rig.panel().await.reason.as_deref(), Some("restore_failed"));
    // The driver recovers and the worker's own retry puts the setting back.
    rig.fake.with(|d| d.refuse_all = false);
    let until = Clock::now() + Duration::from_secs(15);
    while !rig.journal().is_empty() {
        assert!(Clock::now() < until, "the worker never retried");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(rig.fake.value("pacing.limit"), Raw::Int(140));
    rig.host().radeon.forget();
    let panel = rig.panel().await;
    assert_eq!((panel.reason, panel.message), (None, None));
    // The same device writes the setting again: the old failure is not about this write.
    let _next = rig.streaming();
    radeon(
        &rig.write(json!([{"id": "pacing.limit", "value": 210}]))
            .await,
    );
    let panel = rig.panel().await;
    assert_eq!((panel.reason.clone(), panel.message.clone()), (None, None));
    let pacing = setting(&panel, "pacing.limit");
    assert_eq!(pacing.reason, None);
    assert!(pacing.restore.is_some());
}

#[tokio::test]
async fn writing_needs_view_and_host_control_launch_alone_is_not_enough() {
    let rig = Rig::start(Setup::with(LIST | LAUNCH), driver());
    rig.ready();
    rig.host_control(true);
    let _stream = rig.streaming();
    // Reading is open to Launch.
    assert!(rig.panel().await.host_control);
    let change = json!([{"id": "pacing.limit", "value": 200}]);
    rig.write(change.clone())
        .await
        .assert_error(403, "permission_required");
    rig.post("/radeon/restore")
        .await
        .assert_error(403, "permission_required");
    assert_eq!(rig.fake.writes(), Vec::<String>::new());
    rig.host().paired.write().unwrap().clients[0].perm = LIST | VIEW_BIT;
    radeon(&rig.write(change).await);
    assert_eq!(rig.fake.value("pacing.limit"), Raw::Int(200));
}

#[tokio::test]
async fn a_write_answers_from_its_own_read_back_without_another_driver_read() {
    // The same first write, once straight through the worker and once through the endpoint.
    let direct = Rig::new();
    let _stream = direct.owner();
    let before = direct.fake.reads();
    direct
        .host()
        .radeon
        .worker()
        .unwrap()
        .apply(
            "parity-device",
            GPU,
            vec![butterpollo_win_graphics::radeon::Change {
                id: "pacing.limit".into(),
                value: Raw::Int(200),
            }],
        )
        .unwrap();
    let apply_reads = direct.fake.reads() - before;

    let rig = Rig::new();
    let _stream = rig.owner();
    rig.host().radeon.forget();
    let before = rig.fake.reads();
    let panel = radeon(
        &rig.write(json!([{"id": "pacing.limit", "value": 200}]))
            .await,
    );
    // One read to validate the request, then the write's own, and nothing after it.
    assert_eq!(rig.fake.reads() - before, 1 + apply_reads);
    assert_eq!(
        setting(&panel, "pacing.limit").restore,
        Some(ApiRestore {
            original: Api::Integer(140),
            written: Api::Integer(200)
        })
    );
    // It is the newest reading: the next panel is the same one.
    let reads = rig.fake.reads();
    assert_eq!(rig.panel().await, panel);
    assert_eq!(rig.fake.reads(), reads);
}

/// A reading the unsigned way would turn into a different number.
const _: () = assert!(GPU < 0);

#[tokio::test]
async fn the_gpu_id_is_the_signed_id_and_one_that_does_not_fit_is_not_found() {
    let rig = Rig::new();
    let reply = rig.get("/radeon").await;
    assert_eq!(reply.json()["gpus"][0]["id"], json!(GPU));
    let _stream = rig.owner();
    for gpu in [i64::from(GPU) + (1_i64 << 32), 1 << 40] {
        let body = json!({"gpu": gpu, "changes": [{"id": "pacing.limit", "value": 200}]});
        rig.write_body(&body.to_string())
            .await
            .assert_error(404, "not_found");
    }
    assert_eq!(rig.fake.writes(), Vec::<String>::new());
}

#[tokio::test]
async fn a_write_cannot_slip_between_a_restores_check_and_its_queueing() {
    let rig = Rig::new();
    let _first = rig.changed().await;
    // The restore thread has checked that the device did not write again, and is about to queue.
    let (reached, release) = rig.host().radeon.hold_restore_after_check();
    rig.post("/session/end").await.session();
    reached
        .recv_timeout(Duration::from_secs(5))
        .expect("the restore thread checked its generation");
    let _second = rig.streaming();
    let before = rig.host().radeon.writes_noted("parity-device");
    std::thread::scope(|scope| {
        let write = scope.spawn(|| {
            rig.host().radeon.apply(
                "parity-device",
                GPU,
                vec![butterpollo_win_graphics::radeon::Change {
                    id: "pacing.limit".into(),
                    value: Raw::Int(210),
                }],
            )
        });
        // The write waits for the restore to be queued: it has not even noted itself.
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(rig.host().radeon.writes_noted("parity-device"), before);
        assert!(!write.is_finished());
        release.send(()).unwrap();
        write.join().unwrap().unwrap();
    });
    rig.flush();
    // The restore ran first and the write came after it: the new value stays.
    assert_eq!(rig.fake.value("pacing.limit"), Raw::Int(210));
    assert_eq!(rig.journal().len(), 1);
    rig.post("/session/end").await.session();
    rig.flush();
    rig.assert_restored();
}

#[tokio::test]
async fn a_new_write_clears_only_the_failures_of_the_settings_it_changes() {
    let rig = Rig::new();
    let _first = rig.changed().await;
    radeon(
        &rig.write(json!([{"id": "tessellation.mode", "value": "c"}]))
            .await,
    );
    rig.fake.with(|d| d.refuse_all = true);
    rig.post("/session/end").await.session();
    rig.flush();
    assert_eq!(rig.journal().len(), 2);
    rig.fake.with(|d| d.refuse_all = false);
    // The same device writes one of the two settings again.
    let _next = rig.streaming();
    let panel = radeon(
        &rig.write(json!([{"id": "tessellation.mode", "value": "on_unless_app_specifies"}]))
            .await,
    );
    assert_eq!(setting(&panel, "tessellation.mode").reason, None);
    // The other one is still journaled and still failed to be put back.
    let pacing = setting(&panel, "pacing.limit");
    assert_eq!(pacing.reason.as_deref(), Some("restore_failed"));
    assert!(pacing.restore.is_some());
}
