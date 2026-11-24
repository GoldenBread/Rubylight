//! RTSP launch, resume, cleanup, teardown, network drop, and control loss tests.
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_change_during_launch_or_resume_releases_unaccepted_preparation() -> Result<()> {
    for resume in [false, true] {
        let h = Harness::new(10_000, 1).await?;
        if !resume {
            let previous = h.h.current_app.lock().unwrap().take();
            drop(previous);
        }
        let undo_marker = h.directory.join("rejected-launch-prep.txt");
        if !resume {
            // with_environment runs the real prep/undo lifecycle. These are
            // hidden cmd.exe built-ins writing only inside this test's TMP
            // directory; no app window, input or user process is involved.
            h.h.apps.write().unwrap()[0]
                .prep
                .push(butterpollo_session::state::PrepCommand {
                    r#do: format!("echo prepared>\"{}\"", undo_marker.display()),
                    undo: format!("echo undone>>\"{}\"", undo_marker.display()),
                    // CI administrators may have no linked non-elevated
                    // token. Use the existing runner context, as the process
                    // lifecycle fixtures do; never request UAC elevation.
                    elevated: butterpollo_win_io::process::user_environment().is_err(),
                    extra: Default::default(),
                });
        }
        let previous_generation =
            h.h.current_app
                .lock()
                .unwrap()
                .as_ref()
                .map(|app| app.generation.clone());
        h.fixture
            .disconnect_during_preparation
            .store(true, Ordering::Release);
        let response = h.launch_response(0, resume).await?;
        assert!(response.contains("status_code=\"503\""), "{response}");
        assert!(response.contains("disconnected or changing"), "{response}");
        assert_eq!(
            xml(&response, if resume { "resume" } else { "gamesession" }),
            "0"
        );
        assert!(!response.contains("sessionUrl0"));
        assert_eq!(h.fixture.session_checks.load(Ordering::Acquire), 2);
        assert_eq!(h.fixture.resources_retained.load(Ordering::Acquire), 0);
        assert_eq!(
            h.fixture.apps_prepared.load(Ordering::Acquire),
            usize::from(!resume)
        );
        assert_eq!(
            h.h.current_app
                .lock()
                .unwrap()
                .as_ref()
                .map(|app| app.generation.clone()),
            previous_generation
        );
        assert!(h.h.app_audio.lock().unwrap().is_none());
        if !resume {
            assert_eq!(
                std::fs::read_to_string(&undo_marker)?
                    .lines()
                    .collect::<Vec<_>>(),
                ["prepared", "undone"],
                "rejecting a newly prepared app must execute its real RAII undo"
            );
        }
        h.empty().await?;

        h.fixture
            .disconnect_during_preparation
            .store(false, Ordering::Release);
        h.fixture
            .session_disconnected
            .store(false, Ordering::Release);
        let retry = h.launch(0, resume).await?;
        let (session, client) = h.play(&retry).await?;
        h.end(&session, client).await?;
        h.finish().await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_change_before_play_rejects_only_the_pending_attempt() -> Result<()> {
    let h = Harness::new(10_000, 2).await?;
    let live_launch = h.launch(0, false).await?;
    let (session, client) = h.play(&live_launch).await?;
    let generation =
        h.h.current_app
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .generation
            .clone();
    let pending = h.launch(1, false).await?;
    h.rtsp(
        &pending,
        1,
        "ANNOUNCE",
        "a=x-nv-video[0].clientViewportWd:640\r\na=x-nv-video[0].clientViewportHt:480\r\n",
    )
    .await?;
    h.fixture
        .session_disconnected
        .store(true, Ordering::Release);
    let error = h.rtsp(&pending, 2, "PLAY", "").await.unwrap_err();
    assert!(
        error.to_string().contains("disconnected or changing"),
        "{error:#}"
    );
    // This test keeps an extra launch identity clone; real RTSP snapshots
    // deliberately do not own its preparation. Release the test's clone too.
    drop(pending);
    h.counts(1, 0).await?;
    assert!(!session.stopping());
    assert_eq!(h.fixture.displays.load(Ordering::Acquire), 1);
    assert_eq!(h.fixture.audios.load(Ordering::Acquire), 1);
    assert_eq!(
        h.h.current_app.lock().unwrap().as_ref().unwrap().generation,
        generation
    );
    // HTTP already accepted this app; a later PLAY rejection must preserve
    // its normal resumable lifecycle and must not stop another client's stream.
    h.fixture
        .session_disconnected
        .store(false, Ordering::Release);
    h.end(&session, client).await?;
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn abandoned_rtsp_socket_releases_preparation_before_replacement() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    drop(launch);
    h.fixture.hold_rtsp_cleanup.store(true, Ordering::Release);
    let abandoned = tokio::net::TcpStream::connect(address(h.ports.rtsp)).await?;
    until("abandoned RTSP connection authorized", WAIT, || {
        h.fixture.rtsp_cleanup_waiting.load(Ordering::Acquire)
    })
    .await;

    let replacement = h.launch(0, true).await?;
    assert_eq!(
        h.fixture.displays.load(Ordering::Acquire),
        1,
        "canceled preparation still owned while the replacement prepares its display"
    );
    h.fixture.resume_rtsp_cleanup.store(true, Ordering::Release);
    let (session, client) = h.play(&replacement).await?;
    drop(abandoned);
    h.end(&session, client).await?;
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_rtsp_cleanup_preserves_a_new_launch_announcement() -> Result<()> {
    let h = Harness::new(10_000, 2).await?;
    let older_launch = h.launch(0, false).await?;
    h.fixture.hold_rtsp_cleanup.store(true, Ordering::Release);
    let older_connection = tokio::net::TcpStream::connect(address(h.ports.rtsp)).await?;
    until("older RTSP connection reached cleanup", WAIT, || {
        h.fixture.rtsp_cleanup_waiting.load(Ordering::Acquire)
    })
    .await;

    let launch = h.launch(1, false).await?;
    h.rtsp(
        &launch,
        1,
        "ANNOUNCE",
        "a=x-nv-video[0].clientViewportWd:640\r\na=x-nv-video[0].clientViewportHt:480\r\n",
    )
    .await?;
    h.fixture.resume_rtsp_cleanup.store(true, Ordering::Release);
    until("older RTSP cleanup resumed", WAIT, || {
        !h.fixture.rtsp_cleanup_waiting.load(Ordering::Acquire)
    })
    .await;
    // The old connection can now reach its read only after pruning. Its EOF
    // response establishes that cleanup completed before this PLAY.
    let mut older_connection = older_connection;
    older_connection.shutdown().await?;
    let mut response = Vec::new();
    older_connection.read_to_end(&mut response).await?;

    h.rtsp(&launch, 2, "PLAY", "").await?;
    let session = h.h.sessions.lock().unwrap().active[&launch.id].clone();
    let client = Client::new(h.ports, launch)?;
    until("newly announced session is streaming", WAIT, || {
        client.frames.load(Ordering::Acquire) > 2
            && session.stats.idr_requests.load(Ordering::Acquire) > 0
    })
    .await;
    let canceled =
        h.h.sessions
            .lock()
            .unwrap()
            .request_stop(Some(&older_launch.id));
    drop((canceled, older_launch));
    h.end(&session, client).await?;
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failure_during_control_connect_preserves_the_host_termination_reason() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    h.rtsp(
        &launch,
        1,
        "ANNOUNCE",
        "a=x-nv-video[0].clientViewportWd:640\r\na=x-nv-video[0].clientViewportHt:480\r\n",
    )
    .await?;
    h.rtsp(&launch, 2, "PLAY", "").await?;
    h.fixture
        .fail_on_control_connect
        .store(true, Ordering::Release);
    let client = Client::new(h.ports, launch)?;
    until("early failure reason delivered", WAIT, || {
        client.reason.load(Ordering::Acquire) != 0
    })
    .await;
    assert_eq!(client.reason.load(Ordering::Acquire), FAILED);
    drop(client);
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_media_pings_wake_idle_sockets() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play(&launch).await?;
    // A ping the client's worker sent after this one would take the video peer back.
    client.stop_video_pings().await;
    let socket = UdpSocket::bind("127.0.0.1:0")?;
    for port in [h.ports.video, h.ports.audio] {
        socket.send_to(b"PING", address(port))?;
    }
    let peer = socket.local_addr()?;
    until("legacy audio and video pings registered", WAIT, || {
        let peers = h.media.as_ref().unwrap().peers.lock().unwrap();
        [false, true]
            .into_iter()
            .all(|audio| peers.get(&launch.id, audio) == Some(peer))
    })
    .await;
    h.end(&session, client).await?;
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_rtsp_requests_receive_an_authenticated_error_response() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    for (sequence, method, target, body, message) in [
        (
            1,
            "PLAY",
            "streamid=video",
            "",
            "ANNOUNCE required before PLAY",
        ),
        (
            2,
            "SETUP",
            "streamid=unknown",
            "",
            "unknown stream setup target",
        ),
        (
            3,
            "ANNOUNCE",
            "streamid=video",
            "a=x-nv-video[0].clientViewportWd:1\r\n",
            "",
        ),
    ] {
        let response = h
            .rtsp_at(&launch, sequence, method, target, body)
            .await
            .unwrap_err()
            .to_string();
        assert!(response.starts_with("RTSP/1.0 400"), "{response}");
        assert!(response.contains(message), "{response}");
    }
    let canceled = h.h.sessions.lock().unwrap().request_stop(Some(&launch.id));
    drop(canceled);
    drop(launch);
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_a_resume_says_it_came_for_the_running_game() -> Result<()> {
    use butterpollo_stream::display_session::RESUMES_GAME;

    let h = Harness::new(10_000, 1).await?;
    // A client that sends the name itself is not asking for anything: a plain
    // launch keeps a display of its own.
    let launch = h
        .launch_with(0, false, None, &[(RESUMES_GAME, "1")])
        .await?;
    assert_eq!(launch.options.get(RESUMES_GAME), None);
    let resumed = h.launch(0, true).await?;
    assert_eq!(
        resumed.options.get(RESUMES_GAME).map(String::as_str),
        Some("1")
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn vanishes_then_resumes_within_ping_timeout() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    let (old, client) = h.play(&launch).await?;
    drop(client);
    assert!(!old.stopping());
    let resumed = h.launch(0, true).await?;
    assert_ne!(resumed.id, old.launch.id);
    assert!(old.stopping());
    assert_eq!(old.termination_reason(), CLOSED);
    h.counts(0, 1).await?;
    assert_eq!(h.fixture.displays.load(Ordering::Acquire), 1);
    let (session, client) = h.play(&resumed).await?;
    h.end(&session, client).await?;
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn vanishes_then_resumes_after_ping_timeout() -> Result<()> {
    let h = Harness::new(1000, 1).await?;
    let launch = h.launch(0, false).await?;
    let (old, client) = h.play_short_timeout(&launch).await?;
    drop(client);
    until("vanished client times out", WAIT, || old.stopping()).await;
    assert_eq!(old.termination_reason(), FAILED);
    h.empty().await?;
    let resumed = h.launch(0, true).await?;
    let (session, client) = h.play_short_timeout(&resumed).await?;
    h.end(&session, client).await?;
    h.finish().await
}

async fn network_drop(seconds: u64, timeout_ms: u32, survives: bool) -> Result<()> {
    let h = Harness::new(timeout_ms, 1).await?;
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play(&launch).await?;
    let lost_at = Instant::now();
    client.network(true);
    if !survives {
        until(
            "outage ends the session within ping_timeout plus worker scheduling allowance",
            Duration::from_millis(u64::from(timeout_ms)) + WAIT,
            || session.stopping(),
        )
        .await;
        assert_eq!(session.termination_reason(), FAILED);
        h.empty().await?;
    }
    tokio::time::sleep(Duration::from_secs(seconds).saturating_sub(lost_at.elapsed())).await;
    assert!(h.fixture.input_releases.load(Ordering::Acquire) > 0);
    if survives {
        assert!(
            !session.stopping(),
            "{seconds}s outage ended the session within ping_timeout"
        );
        let before = session.stats.idr_requests.load(Ordering::Acquire);
        let frames = client.frames.load(Ordering::Acquire);
        client.network(false);
        until("control and video resume after packet loss", WAIT, || {
            session.stats.idr_requests.load(Ordering::Acquire) > before
                && client.frames.load(Ordering::Acquire) > frames
        })
        .await;
        h.counts(1, 0).await?;
        h.end(&session, client).await?;
    } else {
        assert!(
            session.stopping(),
            "{seconds}s outage exceeded ping_timeout without ending"
        );
        assert_eq!(session.termination_reason(), FAILED);
        drop(client);
        h.empty().await?;
        let resumed = h.launch(0, true).await?;
        let (session, client) = h.play(&resumed).await?;
        h.end(&session, client).await?;
    }
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn network_drop_2s_survives_default_timeout() -> Result<()> {
    network_drop(2, 10_000, true).await
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn network_drop_10s_survives_15s_timeout() -> Result<()> {
    network_drop(10, 15_000, true).await
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn network_drop_40s_times_out_then_resumes() -> Result<()> {
    network_drop(40, 10_000, false).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_clients_race_to_resume_the_same_app() -> Result<()> {
    let h = Harness::new(10_000, 2).await?;
    let generation =
        h.h.current_app
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .generation
            .clone();
    let (a, b) = tokio::join!(h.launch(0, true), h.launch(1, true));
    let (a, b) = (a?, b?);
    assert_ne!(a.id, b.id);
    h.counts(0, 2).await?;
    let (a, b) = tokio::join!(h.play(&a), h.play(&b));
    let ((a, ac), (b, bc)) = (a?, b?);
    h.counts(2, 0).await?;
    assert_eq!(h.fixture.displays.load(Ordering::Acquire), 2);
    assert_eq!(h.fixture.inputs.load(Ordering::Acquire), 2);
    assert_eq!(
        h.h.current_app.lock().unwrap().as_ref().unwrap().generation,
        generation
    );
    h.rtsp(&a.launch, 3, "TEARDOWN", "").await?;
    until("first client's resources released", WAIT, || {
        h.fixture.displays.load(Ordering::Acquire) == 1
            && h.fixture.inputs.load(Ordering::Acquire) == 1
    })
    .await;
    assert_eq!(a.termination_reason(), CLOSED);
    assert!(!b.stopping());
    let before = bc.frames.load(Ordering::Acquire);
    until(
        "second client's video survives first client's teardown",
        WAIT,
        || bc.frames.load(Ordering::Acquire) > before,
    )
    .await;
    h.counts(1, 0).await?;
    drop(ac);
    h.end(&b, bc).await?;
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rtsp_reconnect_waits_for_previous_teardown() -> Result<()> {
    for (client, resume) in [(0, false), (0, true), (1, false), (1, true)] {
        reconnect_waits_for_previous_teardown(client, resume).await?;
    }
    Ok(())
}

async fn reconnect_waits_for_previous_teardown(client_index: usize, resume: bool) -> Result<()> {
    let h = Harness::new(10_000, 2).await?;
    let launch = h.launch(0, false).await?;
    let (old, client) = h.play(&launch).await?;
    h.fixture.hold_teardown.store(true, Ordering::Release);
    h.rtsp(&launch, 3, "TEARDOWN", "").await?;
    until("previous session enters resource teardown", WAIT, || {
        h.fixture.in_teardown.load(Ordering::Acquire)
    })
    .await;
    assert!(old.stopping());
    assert_eq!(old.termination_reason(), CLOSED);
    h.counts(0, 0).await?;
    assert!(
        h.h.sessions
            .lock()
            .unwrap()
            .teardown
            .contains_key(&launch.id)
    );
    let (session, resumed_client, overlapped) = {
        let resume = h.launch(client_index, resume);
        tokio::pin!(resume);
        let launch = match tokio::time::timeout(Duration::from_millis(500), &mut resume).await {
            Ok(result) => result?,
            Err(_) => {
                h.counts(0, 0).await?;
                assert_eq!(h.fixture.displays.load(Ordering::Acquire), 1);
                h.fixture.hold_teardown.store(false, Ordering::Release);
                resume.await?
            }
        };
        let (session, resumed_client) = h.play(&launch).await?;
        let overlapped = h.fixture.displays.load(Ordering::Acquire) > 1;
        h.fixture.hold_teardown.store(false, Ordering::Release);
        (session, resumed_client, overlapped)
    };
    drop(client);
    h.end(&session, resumed_client).await?;
    h.finish().await?;
    assert!(
        !overlapped,
        "RTSP PLAY started a replacement while the previous session still owned display resources in teardown"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rtsp_reconnect_reports_busy_when_previous_teardown_times_out() -> Result<()> {
    for (client_index, resume) in [(0, false), (0, true), (1, false), (1, true)] {
        let h = Harness::new(10_000, 2).await?;
        let launch = h.launch(0, false).await?;
        let (old, client) = h.play(&launch).await?;
        h.fixture.hold_teardown.store(true, Ordering::Release);
        h.rtsp(&launch, 3, "TEARDOWN", "").await?;
        until("previous session enters resource teardown", WAIT, || {
            h.fixture.in_teardown.load(Ordering::Acquire)
        })
        .await;

        let started = Instant::now();
        let response = h.launch_response(client_index, resume).await?;
        assert!(started.elapsed() >= Duration::from_secs(5));
        assert!(response.contains("status_code=\"503\""), "{response}");
        assert!(
            response.contains(&format!(
                "status_message=\"{}\"",
                butterpollo_stream::nvhttp::STILL_CLOSING
            )),
            "{response}"
        );
        assert_eq!(
            xml(&response, if resume { "resume" } else { "gamesession" }),
            "0"
        );
        assert!(!response.contains("sessionUrl0"), "{response}");
        h.counts(0, 0).await?;
        assert_eq!(h.fixture.displays.load(Ordering::Acquire), 1);
        assert!(
            h.h.sessions
                .lock()
                .unwrap()
                .teardown
                .contains_key(&launch.id)
        );
        assert_eq!(old.termination_reason(), CLOSED);

        h.fixture.hold_teardown.store(false, Ordering::Release);
        drop(client);
        h.empty().await?;
        let launch = h.launch(client_index, resume).await?;
        let (session, client) = h.play(&launch).await?;
        h.end(&session, client).await?;
        h.finish().await?;
    }
    Ok(())
}

/// A control worker that runs late finds ENet's own timeout for a lost client before its ping
/// check. The stream ended as closed by the host (0x80030023); it was lost.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_control_stream_found_late_by_enet_still_fails() -> Result<()> {
    let h = Harness::new(2000, 1).await?;
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play_short_timeout(&launch).await?;
    let lost = Instant::now();
    // From before the ping check is due, once ENet has resent its unanswered ping often enough,
    // to after ENet's own timeout.
    *h.fixture.control_held.lock().unwrap() = Some((
        lost + Duration::from_millis(1800),
        lost + Duration::from_millis(4000),
    ));
    client.control_down.store(true, Ordering::Release);
    until("the lost client's stream ends", WAIT, || session.stopping()).await;
    h.empty().await?;
    assert_eq!(session.termination_reason(), FAILED);
    drop(client);
    h.finish().await
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn control_lost_while_video_continues_times_out() -> Result<()> {
    let h = Harness::new(2000, 1).await?;
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play_short_timeout(&launch).await?;
    let before = client.frames.load(Ordering::Acquire);
    let released = h.fixture.input_releases.load(Ordering::Acquire);
    client.control_down.store(true, Ordering::Release);
    assert!(!session.stopping());
    until(
        "video traffic cannot keep a lost control stream alive",
        WAIT,
        || session.stopping(),
    )
    .await;
    h.empty().await?;
    let released_after = h.fixture.input_releases.load(Ordering::Acquire);
    let video_after_release = h.fixture.video_after_input_release.load(Ordering::Acquire);
    let frames_after = client.frames.load(Ordering::Acquire);
    assert!(
        released_after > released,
        "control loss must release input: {released} -> {released_after}"
    );
    assert!(
        video_after_release > released,
        "video must continue after control loss and before timeout: input {released} -> {released_after}, video checkpoint {video_after_release}, frames {before} -> {frames_after}"
    );
    assert!(
        frames_after > before,
        "client must still receive video while control is blocked: {before} -> {frames_after}"
    );
    assert_eq!(session.termination_reason(), FAILED);
    drop(client);
    let launch = h.launch(0, true).await?;
    let (session, client) = h.play_short_timeout(&launch).await?;
    h.end(&session, client).await?;
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_enet_peer_reset_mid_stream_times_out_then_resumes() -> Result<()> {
    let h = Harness::new(1000, 1).await?;
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play_short_timeout(&launch).await?;
    *h.fixture.reset.lock().unwrap() = Some(session.launch.id.clone());
    until("host ENet peer was reset", WAIT, || {
        h.fixture.reset_done.load(Ordering::Acquire)
    })
    .await;
    until("reset peer's session times out", WAIT, || {
        session.stopping()
    })
    .await;
    assert_eq!(session.termination_reason(), FAILED);
    drop(client);
    h.empty().await?;
    let launch = h.launch(0, true).await?;
    let (session, client) = h.play_short_timeout(&launch).await?;
    h.end(&session, client).await?;
    h.finish().await
}
