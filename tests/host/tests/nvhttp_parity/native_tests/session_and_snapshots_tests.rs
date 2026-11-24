//! Native API session actions, error code rendering, and Moonlight XML byte-for-byte snapshots.
use super::*;

#[tokio::test]
async fn session_shows_idle_to_everyone_and_the_owner_alone_the_details() {
    let s = Setup::new();
    let idle = s.get("/session").await.session();
    assert_eq!((idle.state, idle.owner), (SessionState::Idle, Owner::None));
    assert!(idle.app.is_none() && !idle.game_running);
    assert_eq!(idle.actions.len(), 4);

    let _stream = s.streaming();
    let mine = s.get("/session").await.session();
    assert_eq!(
        (mine.state, mine.owner),
        (SessionState::Streaming, Owner::You)
    );
    assert!(mine.game_running && mine.since_unix_ms.is_some());
    assert_eq!(
        mine.app.unwrap().title,
        s.host().apps.read().unwrap()[0].name
    );

    // Another device sees the state and the owner, never the app.
    let other = other_device(&s.f, ALL, true);
    let theirs = api(&s.f, &other, "GET", "/session").await.session();
    assert_eq!(
        (theirs.state, theirs.owner),
        (SessionState::Streaming, Owner::OtherDevice)
    );
    assert!(theirs.app.is_none() && theirs.since_unix_ms.is_none());
    assert!(theirs.actions.iter().all(|action| !action.allowed));
}

#[tokio::test]
async fn disconnect_suspends_the_session_and_never_stops_the_game() {
    let s = Setup::new();
    let stream = s.streaming();
    // The three body forms of an empty POST.
    for (content_type, body) in [
        (None, ""),
        (Some("application/json"), "{}"),
        (Some("text/plain"), " {} "),
    ] {
        let reply = send(
            &s.f,
            true,
            &s.connection,
            "POST",
            &format!("{PREFIX}/session/disconnect"),
            content_type,
            body,
        )
        .await;
        let session = reply.session();
        assert_eq!(reply.headers[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            (session.state, session.owner),
            (SessionState::Suspended, Owner::You)
        );
        assert!(session.game_running && session.suspended_until_unix_ms.is_some());
        assert!(stream.stopping());
    }
    // The game and its display stay for the grace period.
    assert!(s.game_running());
    assert_eq!(s.displays(), 1);
    // Repeating is safe.
    let repeat = s.post("/session/disconnect").await.session();
    assert_eq!(repeat.state, SessionState::Suspended);
    assert!(s.game_running());
}

#[tokio::test]
async fn end_keeps_the_game_running_and_repeats_safely() {
    let s = Setup::new();
    let stream = s.streaming();
    let ended = s.post("/session/end").await.session();
    assert_eq!(
        (ended.state, ended.owner),
        (SessionState::Idle, Owner::None)
    );
    assert!(ended.game_running, "End session leaves the game running");
    assert!(stream.stopping());
    assert!(s.game_running());
    assert_eq!(s.displays(), 0);
    // Nothing is left to end: a refusal, and nothing changes.
    s.post("/session/end").await.assert_error(409, "no_session");
    s.post("/session/disconnect")
        .await
        .assert_error(409, "no_session");
    assert!(s.game_running());
    // While a teardown runs, repeating End is safe and answers `stopping`.
    let _stream = s.streaming();
    let teardown = s
        .host()
        .session_teardown(butterpollo_stream::session_control::Scope::Session);
    let repeat = s.post("/session/end").await.session();
    assert_eq!(repeat.state, SessionState::Stopping);
    drop(teardown);
}

#[tokio::test]
async fn close_game_needs_host_control_and_ends_the_game() {
    let s = Setup::new();
    let _stream = s.streaming();
    // Without Host control, nothing happens.
    s.post("/session/close-game")
        .await
        .assert_error(403, "host_control_required");
    assert!(s.game_running());
    assert_eq!(s.state(), SessionState::Streaming);
    s.host_control(true);
    let closed = s.post("/session/close-game").await.session();
    assert_eq!(closed.state, SessionState::Idle);
    assert!(!closed.game_running);
    assert!(!s.game_running());
    assert_eq!(s.displays(), 0);
    // A repeat has no game and no session.
    s.post("/session/close-game")
        .await
        .assert_error(409, "no_session");
}

#[tokio::test]
async fn close_game_also_closes_a_game_that_end_left_running() {
    let s = Setup::new();
    s.host_control(true);
    let _stream = s.streaming();
    assert_eq!(
        s.post("/session/end").await.session().state,
        SessionState::Idle
    );
    assert!(s.game_running());
    // Another device with Host control may close it: nobody owns a session.
    let other = other_device(&s.f, ALL, true);
    let closed = api(&s.f, &other, "POST", "/session/close-game")
        .await
        .session();
    assert_eq!(closed.state, SessionState::Idle);
    assert!(!closed.game_running && !s.game_running());
}

#[tokio::test]
async fn host_control_is_read_from_the_paired_state_on_every_request() {
    let s = Setup::new();
    // The launch holds a copy of the device from the time it started. The
    // console's grant and revoke apply to the next request, not to that copy.
    let mut granted_at_launch = s.client.clone();
    granted_at_launch
        .extra
        .insert("host_control".into(), true.into());
    s.run_game();
    s.retain_display();
    let _stream = s.stream_as(granted_at_launch);
    s.post("/session/close-game")
        .await
        .assert_error(403, "host_control_required");
    assert!(s.game_running(), "the stale copy did not close the game");
    // A grant applies to the very next request, and so does its revoke.
    s.host_control(true);
    s.host_control(false);
    s.post("/session/close-game")
        .await
        .assert_error(403, "host_control_required");
    s.host_control(true);
    assert_eq!(
        s.post("/session/close-game").await.session().state,
        SessionState::Idle
    );
}

#[tokio::test]
async fn an_action_from_another_device_is_409_not_session_owner_and_changes_nothing() {
    let s = Setup::new();
    let _stream = s.streaming();
    let other = other_device(&s.f, ALL, true);
    for path in ["/session/disconnect", "/session/end", "/session/close-game"] {
        api(&s.f, &other, "POST", path)
            .await
            .assert_error(409, "not_session_owner");
    }
    assert_eq!(s.state(), SessionState::Streaming);
    assert!(s.game_running());
    assert_eq!(s.displays(), 1);
}

#[tokio::test]
#[allow(
    clippy::await_holding_lock,
    reason = "the launch lock is held on purpose while the handler tries it from the blocking pool"
)]
async fn an_action_during_a_launch_is_409_session_busy_with_a_retry_hint() {
    let s = Setup::new();
    let _stream = s.streaming();
    let launch = s.host().launch_transition.lock().unwrap();
    for path in ["/session/disconnect", "/session/end", "/session/close-game"] {
        s.host_control(true);
        let reply = s.post(path).await;
        reply.assert_error(409, "session_busy");
        assert_eq!(reply.json()["error"]["retry_after_ms"], 1000);
    }
    drop(launch);
    assert_eq!(s.state(), SessionState::Streaming);
    assert!(s.game_running());
}

#[tokio::test]
async fn an_action_with_no_session_is_409_no_session() {
    let s = Setup::new();
    s.host_control(true);
    for path in ["/session/disconnect", "/session/end", "/session/close-game"] {
        s.post(path).await.assert_error(409, "no_session");
    }
}

#[tokio::test]
async fn a_bad_body_is_400_invalid_request() {
    let s = Setup::new();
    let url = format!("{PREFIX}/session/end");
    let post = |content_type: Option<&'static str>, body: Vec<u8>| {
        send(&s.f, true, &s.connection, "POST", &url, content_type, body)
    };
    post(Some("application/json"), b"{".to_vec())
        .await
        .assert_error(400, "invalid_request");
    post(None, b"[]".to_vec())
        .await
        .assert_error(400, "invalid_request");
    post(Some("text/xml"), b"{}".to_vec())
        .await
        .assert_error(400, "invalid_request");
    let huge = format!("{{\"padding\":\"{}\"}}", "x".repeat(16 * 1024));
    post(Some("application/json"), huge.into_bytes())
        .await
        .assert_error(400, "invalid_request");
    // Valid bodies reach the action, which refuses on the state alone.
    post(
        Some("application/json; charset=utf-8"),
        b"{\"a\":1}".to_vec(),
    )
    .await
    .assert_error(409, "no_session");
    post(Some("text/plain"), b"".to_vec())
        .await
        .assert_error(409, "no_session");
}

#[tokio::test]
async fn every_error_code_renders_with_its_status_and_the_one_body_shape() {
    for code in [
        ErrorCode::NotPaired,
        ErrorCode::PermissionRequired,
        ErrorCode::HostControlRequired,
        ErrorCode::NotFound,
        ErrorCode::InvalidRequest,
        ErrorCode::InvalidValue,
        ErrorCode::NotWritable,
        ErrorCode::NoSession,
        ErrorCode::NotSessionOwner,
        ErrorCode::SessionBusy,
        ErrorCode::DriverRejected,
        ErrorCode::TemporarilyUnavailable,
        ErrorCode::Forbidden,
        ErrorCode::FileTransferDisabled,
        ErrorCode::InvalidName,
        ErrorCode::IncompleteUpload,
        ErrorCode::LengthRequired,
        ErrorCode::TooLarge,
        ErrorCode::InsufficientStorage,
        ErrorCode::NoConsoleUser,
        ErrorCode::TransferBusy,
        ErrorCode::Internal,
    ] {
        let response = axum::response::IntoResponse::into_response(ApiFailure(Failure::new(
            code,
            "Something to read.",
        )));
        let (parts, body) = response.into_parts();
        let reply = Reply {
            status: parts.status,
            headers: parts.headers,
            body: to_bytes(body, 4096).await.unwrap().to_vec(),
        };
        let name = serde_json::to_value(code).unwrap();
        reply.assert_error(code.status(), name.as_str().unwrap());
    }
    let internal = axum::response::IntoResponse::into_response(ApiFailure(Failure::internal(
        "do a thing",
        "disk on fire",
    )));
    assert_eq!(internal.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn serverinfo_only_gains_native_api_version_and_the_rest_is_byte_identical() {
    let f = Fixture::new();
    let field = |xml: &str| -> String {
        let start = xml.find("<NativeApiVersion>").expect("the field is there");
        let end = start + xml[start..].find("</NativeApiVersion>").unwrap();
        xml[start + "<NativeApiVersion>".len()..end].to_owned()
    };
    let strip = |xml: &str| -> String {
        let start = xml.find("<NativeApiVersion>").unwrap();
        let end =
            start + xml[start..].find("</NativeApiVersion>").unwrap() + "</NativeApiVersion>".len();
        format!("{}{}", &xml[..start], &xml[end..])
    };
    // Unpaired HTTPS, and plain HTTP (which can never identify a client).
    for tls in [true, false] {
        let xml = serverinfo_xml(&f, tls).await;
        assert_eq!(field(&xml), "0");
        assert_eq!(strip(&xml), golden(SERVERINFO_UNPAIRED, &f));
    }
    f.client(0x071f1f00);
    let xml = serverinfo_xml(&f, true).await;
    assert_eq!(field(&xml), "1");
    assert_eq!(strip(&xml), golden(SERVERINFO_PAIRED, &f));
    // Paired state without TLS is still version 0.
    assert_eq!(field(&serverinfo_xml(&f, false).await), "0");
    // A stock Moonlight client ignores the field: it sits among the others.
    assert!(xml.contains("<HardwareMetricsVersion>1</HardwareMetricsVersion><NativeApiVersion>1</NativeApiVersion><hostname>"));
}

#[tokio::test]
async fn applist_and_cancel_are_byte_identical_to_before_the_native_api() {
    let f = Fixture::new();
    let client = f.client(0x071f1f00);
    *f.host.apps.write().unwrap() = [
        json!({"name": "Desktop", "uuid": "11111111-1111-4111-8111-111111111111"}),
        json!({"name": "Dota 2", "uuid": "22222222-2222-4222-8222-222222222222", "steam-id": "570"}),
    ]
    .into_iter()
    .map(|app| serde_json::from_value(app).unwrap())
    .collect();
    let connection = f.connection(true);
    let applist = send(&f, true, &connection, "GET", "/applist", None, vec![]).await;
    assert_eq!(applist.text(), APPLIST);
    let cancel = send(&f, true, &connection, "GET", "/cancel", None, vec![]).await;
    assert_eq!(cancel.text(), CANCEL);
    // The native API does not need the Moonlight endpoints' permissions
    // changed: the same device reads both.
    assert_eq!(client.perm, 0x071f1f00);
    assert_eq!(
        send(
            &f,
            true,
            &connection,
            "GET",
            &format!("{PREFIX}/library"),
            None,
            vec![]
        )
        .await
        .status,
        StatusCode::OK
    );
}
