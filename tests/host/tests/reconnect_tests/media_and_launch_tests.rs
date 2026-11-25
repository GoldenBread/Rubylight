//! Microphone routing, Pyrowave probe, HDR/SDR, launch expiry, and RTSP deadline tests.
use super::*;

/// A microphone client as Apollo's: offered in DESCRIBE, set up and played
/// as `streamid=mic`, its encrypted packets taken for its own session only.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn microphone_packets_reach_only_the_session_that_set_them_up() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    let describe = h.rtsp(&launch, 10, "DESCRIBE", "").await?;
    assert!(
        describe.contains(&format!(
            "m=audio {} RTP/AVP 96\r\na=rtpmap:96 opus/48000/1\r\n",
            h.ports.mic
        )),
        "{describe}"
    );
    // Loopback is a LAN client: encryption optional, the microphone requested.
    assert!(
        describe.contains("encryptionSupported:13\r\n"),
        "{describe}"
    );
    assert!(describe.contains("encryptionRequested:9\r\n"), "{describe}");
    let setup = h
        .rtsp_at(&launch, 11, "SETUP", "streamid=mic/0/0", "")
        .await?;
    assert!(
        setup.contains(&format!("Transport: server_port={}\r\n", h.ports.mic)),
        "{setup}"
    );
    let (session, client) = h.play_encrypted(&launch, mic::ENCRYPTION).await?;
    assert!(session.config.mic);
    // A client that plays each stream on its own plays the microphone last.
    h.rtsp_at(&launch, 12, "PLAY", "streamid=mic", "").await?;
    let counters = h
        .media
        .as_ref()
        .unwrap()
        .mic
        .clone()
        .expect("microphone port bound");
    let socket = UdpSocket::bind("127.0.0.1:0")?;
    let send = |datagram: Vec<u8>| {
        socket
            .send_to(&datagram, address(h.ports.mic))
            .expect("microphone packet sent");
    };
    // A loaded PC loses the odd loopback datagram: what has not arrived by the next
    // attempt is sent again, under a new sequence number.
    let attempt = Duration::from_millis(200);
    let mut sequence = 0;
    let mut sent = Instant::now() - attempt;
    until("microphone packets accepted", WAIT, || {
        let accepted = counters.accepted.load(Ordering::Acquire);
        if accepted < 5 && sent.elapsed() >= attempt {
            for _ in accepted..5 {
                send(mic::seal(
                    &launch.key,
                    launch.key_id,
                    sequence,
                    0,
                    &[0xf8, 0xff, 0xfe],
                ));
                sequence += 1;
            }
            sent = Instant::now();
        }
        accepted >= 5
    })
    .await;
    assert_eq!(counters.refused.load(Ordering::Acquire), 0);
    let mut damaged = mic::seal(&launch.key, launch.key_id, sequence, 0, &[0xf8, 0xff, 0xfe]);
    // CBC has no authentication tag: a wrong key can randomly produce valid
    // padding. Keep the microphone header but truncate the ciphertext so this
    // rejection is deterministic for every randomly generated launch key.
    damaged.pop();
    let mut sent = Instant::now() - attempt;
    until("a damaged microphone packet refused", WAIT, || {
        if sent.elapsed() >= attempt {
            send(damaged.clone());
            sent = Instant::now();
        }
        counters.refused.load(Ordering::Acquire) >= 1
    })
    .await;
    h.end(&session, client).await?;
    // No session takes microphone packets from here on.
    let accepted = counters.accepted.load(Ordering::Acquire);
    // Without a microphone SETUP, the next session's packets are refused.
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play_encrypted(&launch, mic::ENCRYPTION).await?;
    assert!(!session.config.mic);
    let refused = counters.refused.load(Ordering::Acquire);
    let mut sent = Instant::now() - attempt;
    until(
        "microphone packet of a session without one refused",
        WAIT,
        || {
            if sent.elapsed() >= attempt {
                send(mic::seal(
                    &launch.key,
                    launch.key_id,
                    0,
                    0,
                    &[0xf8, 0xff, 0xfe],
                ));
                sent = Instant::now();
            }
            counters.refused.load(Ordering::Acquire) > refused
        },
    )
    .await;
    assert_eq!(counters.accepted.load(Ordering::Acquire), accepted);
    h.end(&session, client).await?;
    // A client that would send it in the clear is not taken.
    let launch = h.launch(0, false).await?;
    h.rtsp_at(&launch, 11, "SETUP", "streamid=mic", "").await?;
    let (session, client) = h.play_encrypted(&launch, 0).await?;
    assert!(!session.config.mic);
    assert!(
        session
            .launch
            .warnings
            .snapshot()
            .iter()
            .any(|w| w.code == "microphone"),
    );
    h.end(&session, client).await?;
    h.finish().await
}

/// Nonary's calibration and Nightjar read the host's UDP port from the
/// headers before the probe's result, so the host must send them at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_paired_client_runs_the_pyrowave_udp_probe_with_a_handshake() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let token = "0123456789abcdef0123456789abcdef";
    let client = std::net::UdpSocket::bind("127.0.0.1:0")?;
    // Buffer the complete two-second probe if parallel tests deschedule this receiver.
    socket2::SockRef::from(&client).set_recv_buffer_size(8 * 1024 * 1024)?;
    let named = client.local_addr()?.port();
    let url = |handshake: u8| {
        format!(
            "https://127.0.0.1:{}/pyrowave-udp-probe?kbps=20000&port={named}&packetsize=1024&token={token}&handshake={handshake}",
            h.ports.https
        )
    };
    // Not over plain HTTP, where no client is identified.
    let plain = format!("http://127.0.0.1:{}/pyrowave-udp-probe", h.ports.http);
    assert_eq!(
        h.http.get(plain).send().await?.status(),
        reqwest::StatusCode::NOT_FOUND
    );
    let response = h.clients[0].get(url(1)).send().await?;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let port: u16 = response.headers()["x-pyrowave-udp-port"]
        .to_str()?
        .parse()?;
    let counted = tokio::task::spawn_blocking(move || -> Result<usize> {
        client.send_to(token.as_bytes(), ("127.0.0.1", port))?;
        client.set_read_timeout(Some(Duration::from_millis(400)))?;
        let mut seen = std::collections::HashSet::new();
        let mut buffer = [0u8; 2048];
        while let Ok((bytes, _)) = client.recv_from(&mut buffer) {
            if bytes == 1024 + 48 && &buffer[..32] == token.as_bytes() {
                seen.insert(u32::from_be_bytes(buffer[32..36].try_into()?));
            }
        }
        Ok(seen.len())
    });
    let body = response.text().await?;
    let counted = counted.await??;
    let field = |name: &str| -> u32 {
        body.split(&format!("<{name}>"))
            .nth(1)
            .and_then(|rest| rest.split('<').next())
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    };
    assert!(body.contains("status_code=\"200\""), "{body}");
    assert_eq!(
        field("expected"),
        butterpollo_media_policy::pyrowave_probe::expected(20_000, 1024)
    );
    // A busy machine (a CI runner) can fall behind the schedule and stop at
    // the overload limit (2128 of 4317 on a windows-2022 runner); what the
    // host reports sent must still arrive.
    assert!(field("sent") > field("expected") / 4, "{body}");
    assert!(field("sent") <= field("expected"), "{body}");
    assert_eq!(counted as u32, field("sent"));
    // A bad query is refused before anything is sent.
    let bad = h.clients[0].get(url(2)).send().await?;
    assert_eq!(bad.status(), reqwest::StatusCode::BAD_REQUEST);
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_session_state_follows_a_launch_a_resume_and_the_end_of_a_stream() -> Result<()> {
    use rubylight_native_api::session::State;
    let h = Harness::new(10_000, 1).await?;
    let journal = h
        .directory
        .join(butterpollo_stream::session_control::JOURNAL);
    // A game runs from before and nobody streams.
    let snapshot = h.h.session_snapshot();
    assert_eq!(snapshot.state, State::Idle);
    assert!(snapshot.game_running);
    assert!(!journal.exists());

    // /launch replies and waits for the client.
    let launch = h.launch(0, false).await?;
    let snapshot = h.h.session_snapshot();
    assert_eq!(snapshot.state, State::Launching);
    assert_eq!(snapshot.owner.as_deref(), Some("0"));
    h.h.session_flush();
    assert!(journal.exists());
    let (first, first_client) = h.play(&launch).await?;
    assert_eq!(h.h.session_snapshot().state, State::Streaming);

    // The same device's /resume supersedes its running stream.
    let resume = h.launch(0, true).await?;
    assert!(first.stopping());
    assert_eq!(h.h.session_snapshot().state, State::Launching);
    drop(first_client);
    until("the superseded stream ended", WAIT, || {
        !h.h.sessions
            .lock()
            .unwrap()
            .active
            .contains_key(&first.launch.id)
    })
    .await;
    let (second, client) = h.play(&resume).await?;
    let snapshot = h.h.session_snapshot();
    assert_eq!(snapshot.state, State::Streaming);
    assert_eq!(snapshot.owner.as_deref(), Some("0"));

    // The client ends the stream: no display is kept, so the session is
    // idle and the game keeps running.
    h.end(&second, client).await?;
    let snapshot = h.h.session_snapshot();
    assert_eq!(snapshot.state, State::Idle);
    assert!(snapshot.game_running);
    assert_eq!(snapshot.owner, None);
    h.h.session_flush();
    assert!(!journal.exists());
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancel_while_a_launch_waits_for_its_client_ends_the_session() -> Result<()> {
    use rubylight_native_api::session::State;
    let h = Harness::new(10_000, 1).await?;
    let journal = h
        .directory
        .join(butterpollo_stream::session_control::JOURNAL);
    // The copy holds the launch's resources: keep only the id.
    let id = h.launch(0, false).await?.id;
    assert_eq!(h.h.session_snapshot().state, State::Launching);
    h.h.session_flush();
    assert!(journal.exists());

    // Stock /cancel, from the launching device, stops the pending launch and
    // the game.
    let response = h.clients[0]
        .get(format!("https://127.0.0.1:{}/cancel", h.ports.https))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    assert_eq!(xml(&response, "cancel"), "1", "{response}");
    assert!(!h.h.sessions.lock().unwrap().pending.contains_key(&id));
    let snapshot = h.h.session_snapshot();
    assert_eq!(snapshot.state, State::Idle);
    assert!(!snapshot.game_running);
    assert_eq!(snapshot.owner, None);
    h.h.session_flush();
    assert!(!journal.exists());
    h.finish().await
}

/// A launch whose app fails to start after its display was prepared answers
/// with an error, so the client never connects. Its pending slot and display
/// lease go at once rather than at the 30 s expiry: /serverinfo must not keep
/// counting a client that is gone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_launch_whose_app_fails_to_start_is_not_left_pending() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    h.fixture
        .fail_app_preparation
        .store(true, Ordering::Release);
    let response = h.launch_response(0, false).await?;
    assert!(!response.contains("status_code=\"200\""), "{response}");
    assert_eq!(xml(&response, "gamesession"), "0", "{response}");
    assert!(h.h.sessions.lock().unwrap().pending.is_empty());
    h.counts(0, 0).await?;
    assert_eq!(h.fixture.displays.load(Ordering::Acquire), 0);
    h.fixture
        .fail_app_preparation
        .store(false, Ordering::Release);
    h.finish().await
}

/// A client that negotiates a ten-bit format (`dynamicRangeMode:1`) after a
/// launch or resume: it is ten-bit SDR only when the query said `hdrMode=0`,
/// and HDR otherwise (no `hdrMode`, or `hdrMode=1`). SDR keeps `hdr` off, the
/// part the prepared display compares, so the display prepared for the launch
/// is kept, and the client is told HDR is off. The three launches run first,
/// each ended by TEARDOWN, which leaves the app for the three resumes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_ten_bit_format_streams_sdr_only_with_hdr_mode_zero() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let sdp = "a=x-nv-vqos[0].bitStreamFormat:1\r\na=x-nv-video[0].dynamicRangeMode:1\r\n";
    // (resume, hdrMode, streams ten-bit SDR rather than HDR)
    for (resume, hdr_mode, sdr) in [
        (false, None, false),
        (false, Some("0"), true),
        (false, Some("1"), false),
        (true, None, false),
        (true, Some("0"), true),
        (true, Some("1"), false),
    ] {
        let case = format!("resume={resume} hdrMode={hdr_mode:?}");
        let launch = h.launch_hdr(0, resume, hdr_mode).await?;
        let (session, client) = h.connect_announcing(&launch, 0, sdp).await?;
        let config = &session.config;
        assert!(config.ten_bit(), "{case}");
        assert_eq!((config.hdr, config.sdr_10bit), (!sdr, sdr), "{case}");
        // The message needs the control connection only.
        until("HDR mode message", WAIT, || {
            !client.hdr_modes.lock().unwrap().is_empty()
        })
        .await;
        assert_eq!(
            client.hdr_modes.lock().unwrap()[0].1[0],
            u8::from(!sdr),
            "{case}"
        );
        h.end(&session, client).await?;
    }
    h.finish().await
}

/// A client may not use its launch before the host has prepared it and
/// replied: an RTSP request then gets no answer, and the launch, once
/// prepared, streams as usual.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_launch_still_being_prepared_takes_no_rtsp_request() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    h.fixture
        .hold_app_preparation
        .store(true, Ordering::Release);
    let early = async {
        until("launch preparing its app", WAIT, || {
            h.fixture.app_preparation_waiting.load(Ordering::Acquire)
        })
        .await;
        let early =
            h.h.sessions
                .lock()
                .unwrap()
                .pending
                .values()
                .next()
                .cloned();
        let early = early.expect("a launch is pending while it prepares");
        let options = h.rtsp(&early, 1, "OPTIONS", "").await;
        let announce = h.rtsp(&early, 2, "ANNOUNCE", VIEWPORT).await;
        h.fixture
            .resume_app_preparation
            .store(true, Ordering::Release);
        (early, options, announce)
    };
    let (launch, (early, options, announce)) = tokio::join!(h.launch(0, false), early);
    let launch = launch?;
    assert_eq!(launch.id, early.id);
    for refused in [options, announce] {
        let refused = refused.unwrap_err().to_string();
        assert!(refused.contains("closed without a response"), "{refused}");
    }
    let (session, client) = h.play(&launch).await?;
    h.end(&session, client).await?;
    h.finish().await
}

/// A handshake under way when the launch's 30 s pass keeps it for its lease;
/// a launch nobody claimed in its 30 s is refused and pruned.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_claimed_launch_outlives_its_30_s_and_an_unclaimed_one_expires() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    h.rtsp(&launch, 1, "ANNOUNCE", VIEWPORT).await?;
    age_launch(&h, &launch, Duration::from_secs(31));
    h.rtsp(&launch, 2, "PLAY", "").await?;
    let session = h.h.sessions.lock().unwrap().active[&launch.id].clone();
    let client = Client::new(h.ports, launch.clone())?;
    assert!(
        established(&session, &client).await,
        "claimed launch did not stream"
    );
    h.end(&session, client).await?;

    let stale = h.launch(0, false).await?;
    age_launch(&h, &stale, Duration::from_secs(31));
    let refused = h
        .rtsp(&stale, 1, "OPTIONS", "")
        .await
        .unwrap_err()
        .to_string();
    assert!(refused.contains("closed without a response"), "{refused}");
    h.counts(0, 0).await?;
    // The pruned launch holds its display until the last copy goes.
    drop(stale);
    h.finish().await
}

/// The device launches again while an RTSP connection still holds its older
/// launch: that connection's request is refused, and only the new launch streams.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reconnect_racing_a_new_launch_streams_only_the_new_one() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let first = h.launch(0, false).await?;
    h.rtsp(&first, 1, "OPTIONS", "").await?;
    // An RTSP connection authorized for the first launch, before it reads.
    h.fixture.hold_rtsp_cleanup.store(true, Ordering::Release);
    let mut held = tokio::net::TcpStream::connect(address(h.ports.rtsp)).await?;
    until(
        "RTSP connection authorized for the first launch",
        WAIT,
        || h.fixture.rtsp_cleanup_waiting.load(Ordering::Acquire),
    )
    .await;
    let second = h.launch(0, false).await?;
    assert_ne!(first.id, second.id);
    h.fixture.resume_rtsp_cleanup.store(true, Ordering::Release);
    held.write_all(&sealed_rtsp(
        &first,
        2,
        "ANNOUNCE",
        "rtsp://127.0.0.1/stream",
        VIEWPORT,
    )?)
    .await?;
    let mut response = Vec::new();
    tokio::time::timeout(WAIT, held.read_to_end(&mut response)).await??;
    let refused = opened_rtsp(&first, "ANNOUNCE", &response)
        .unwrap_err()
        .to_string();
    assert!(refused.starts_with("RTSP/1.0 400"), "{refused}");
    assert!(!refused.contains("replaced"), "{refused}");
    let failure = h.h.last_failure(&first.client.uuid).unwrap_or_default();
    assert!(failure.contains("replaced"), "{failure}");
    let refused = h.rtsp(&first, 3, "PLAY", "").await.unwrap_err().to_string();
    assert!(refused.contains("closed without a response"), "{refused}");
    // The replaced launch holds its display until the last copy goes.
    drop(first);
    h.counts(0, 1).await?;
    let (session, client) = h.play(&second).await?;
    assert_eq!(
        h.h.sessions
            .lock()
            .unwrap()
            .active
            .keys()
            .collect::<Vec<_>>(),
        [&second.id]
    );
    h.end(&session, client).await?;
    h.finish().await
}

/// Two devices launch at once. Two PLAYs of one launch start one session,
/// and the other device's launch stays its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_clients_launching_at_once_each_keep_their_launch_and_play_once() -> Result<()> {
    let h = Harness::new(10_000, 2).await?;
    let (other, launch) = tokio::join!(h.launch(0, false), h.launch(1, false));
    let (other, launch) = (other?, launch?);
    h.counts(0, 2).await?;
    h.rtsp(&launch, 1, "ANNOUNCE", VIEWPORT).await?;
    let (a, b) = tokio::join!(
        h.rtsp(&launch, 2, "PLAY", ""),
        // Not 3: that is the TEARDOWN's.
        h.rtsp(&launch, 4, "PLAY", "")
    );
    assert_eq!(
        [a.is_ok(), b.is_ok()].iter().filter(|ok| **ok).count(),
        1,
        "{a:?} {b:?}"
    );
    let session = {
        let sessions = h.h.sessions.lock().unwrap();
        assert_eq!(sessions.active.len(), 1);
        assert!(sessions.pending.contains_key(&other.id));
        sessions.active[&launch.id].clone()
    };
    h.counts(1, 1).await?;
    let client = Client::new(h.ports, launch.clone())?;
    assert!(
        established(&session, &client).await,
        "the one session did not stream"
    );
    let canceled = h.h.sessions.lock().unwrap().request_stop(Some(&other.id));
    drop((canceled, other));
    h.end(&session, client).await?;
    h.finish().await
}

/// A client that connects and stalls is cut off at the handshake deadline,
/// without costing its launch.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stalled_rtsp_connection_is_closed_at_its_deadline() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    h.fixture.rtsp_deadline_ms.store(300, Ordering::Release);
    let launch = h.launch(0, false).await?;
    let started = Instant::now();
    let mut stalled = tokio::net::TcpStream::connect(address(h.ports.rtsp)).await?;
    // Half of an encrypted request's header, then nothing.
    stalled
        .write_all(&sealed_rtsp(&launch, 1, "OPTIONS", "rtsp://127.0.0.1/stream", "")?[..12])
        .await?;
    let mut response = Vec::new();
    tokio::time::timeout(WAIT, stalled.read_to_end(&mut response)).await??;
    assert!(response.is_empty());
    assert!(started.elapsed() >= Duration::from_millis(300));
    h.counts(0, 1).await?;
    let (session, client) = h.play(&launch).await?;
    h.end(&session, client).await?;
    h.finish().await
}
