//! WebRTC codec negotiation (H.264/HEVC/AV1), surround audio, and HDR display tests.
use super::*;

async fn stream_and_stop(case: Case) -> Result<()> {
    let web = web_with_every_codec().await?;
    let mut ws = web.device("TV", u32::MAX).await;
    ws.send(web.request("start-session", case.name, case.hdr, case.channels))
        .await;
    status(&mut ws, "session-status", "starting").await;
    let (id, peer) = answer_case(&mut ws, &case).await;
    streaming(&mut ws, id).await;

    // Video arrives as the encoder produced it, starting with a keyframe.
    until("video received", WAIT, || peer.count() >= 5).await;
    let codec = case.codec;
    let (key, delta) = (received(codec, true), received(codec, false));
    {
        let units = peer.units.lock().unwrap();
        assert_eq!(units[0], key);
        assert!(units.iter().all(|u| *u == key || *u == delta));
    }
    // So does the audio, in the layout the answer agreed to.
    until("audio received", WAIT, || {
        peer.audio.lock().unwrap().len() >= 10
    })
    .await;
    {
        let audio = peer.audio.lock().unwrap();
        let want = case.audio();
        assert!(
            audio.iter().all(|p| *p == want),
            "{} channels: {:?} but expected {want:?}",
            case.channels,
            audio.iter().find(|p| **p != want)
        );
    }
    let session = web.session();
    assert_eq!(
        session.launch.transport,
        butterpollo_session::session::Transport::WebRtc
    );
    // The stream is configured as the GameStream path would for the same request.
    assert_eq!(session.config.codec, codec);
    assert_eq!(session.config.hdr, case.hdr);
    assert_eq!(session.config.audio_channels as u32, case.channels);
    assert_eq!(session.config.ten_bit(), case.hdr);
    if case.hdr {
        // 10-bit BT.2020 PQ in limited range: what the HEVC VUI and the AV1 sequence header carry.
        assert_eq!(session.config.color_matrix(), 2);
        assert!(!session.config.full_range());
    } else {
        assert_eq!(session.config.color_matrix(), 1);
    }
    assert!(session.stats.frames.load(Ordering::Acquire) >= 5);
    assert!(session.stats.bytes.load(Ordering::Acquire) > 0);

    // A PLI is a keyframe request, as Moonlight's 0x0302.
    let requests = session.stats.idr_requests.load(Ordering::Acquire);
    let keyframes = peer.keyframes(codec);
    // RTCP is unreliable: a busy host's receive queue can drop the datagram
    // with the PLI, so the peer asks again until one arrives, as a browser
    // that still has no keyframe does.
    let mut asked = None::<Instant>;
    until("the PLI reached the session", WAIT, || {
        if session.stats.idr_requests.load(Ordering::Acquire) > requests {
            return true;
        }
        if asked.is_none_or(|at| at.elapsed() >= Duration::from_millis(250)) {
            peer.pli.store(1, Ordering::Release);
            asked = Some(Instant::now());
        }
        false
    })
    .await;
    until("a keyframe followed the PLI", WAIT, || {
        peer.keyframes(codec) > keyframes
    })
    .await;

    // So is the control channel's request-keyframe.
    let requests = session.stats.idr_requests.load(Ordering::Acquire);
    peer.control
        .lock()
        .unwrap()
        .push(r#"{"type":"request-keyframe"}"#.into());
    until("the control request reached the session", WAIT, || {
        session.stats.idr_requests.load(Ordering::Acquire) > requests
    })
    .await;

    // stop-session ends the stream and frees the port; the app keeps running.
    ws.send(json!({"version":2,"type":"stop-session"})).await;
    status(&mut ws, "session-status", "stopping").await;
    status(&mut ws, "session-status", "idle").await;
    web.idle().await;
    assert!(web.harness.h.current_app.lock().unwrap().is_some());
    assert!(UdpSocket::bind(address(web.media_port)).is_ok());
    drop(peer);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn h264_sdr_streams_to_a_webrtc_client_and_stops() -> Result<()> {
    stream_and_stop(H264).await
}

#[tokio::test(flavor = "multi_thread")]
async fn hevc_sdr_streams_to_a_webrtc_client_and_stops() -> Result<()> {
    stream_and_stop(HEVC).await
}

#[tokio::test(flavor = "multi_thread")]
async fn av1_sdr_streams_to_a_webrtc_client_without_temporal_delimiters() -> Result<()> {
    // `received` is the access unit minus its temporal delimiter, so the whole run is checked.
    assert_eq!(
        access_unit(2, false)[..2],
        [0x12, 0],
        "the encoder's AV1 starts with a temporal delimiter"
    );
    stream_and_stop(AV1).await
}

#[tokio::test(flavor = "multi_thread")]
async fn hevc_main10_hdr_streams_to_a_webrtc_client() -> Result<()> {
    stream_and_stop(HEVC.hdr()).await
}

#[tokio::test(flavor = "multi_thread")]
async fn av1_10_bit_hdr_streams_to_a_webrtc_client() -> Result<()> {
    stream_and_stop(AV1.hdr()).await
}

#[tokio::test(flavor = "multi_thread")]
async fn five_one_surround_streams_to_a_webrtc_client() -> Result<()> {
    stream_and_stop(HEVC.surround(6)).await
}

#[tokio::test(flavor = "multi_thread")]
async fn seven_one_surround_streams_to_a_webrtc_client() -> Result<()> {
    stream_and_stop(H264.surround(8)).await
}

#[tokio::test(flavor = "multi_thread")]
async fn hdr_surround_and_av1_together_stream_to_a_webrtc_client() -> Result<()> {
    stream_and_stop(AV1.hdr().surround(8)).await
}

#[tokio::test(flavor = "multi_thread")]
async fn av1_and_hdr_are_refused_without_the_encoder_probe() -> Result<()> {
    // No AV1, no Main10: the base table (H.264, HEVC) only.
    let web = Web::new().await?;
    web.harness.h.codecs.store(1 | 0x100, Ordering::Release);
    let mut ws = web.device("TV", u32::MAX).await;
    for (codec, hdr) in [
        ("av1", false),
        ("av1", true),
        ("hevc", true),
        ("h264", true),
    ] {
        ws.send(web.request("start-session", codec, hdr, 2)).await;
        error(&mut ws, "start-session", "unsupported-settings").await;
    }
    // A switch asks the same, and nothing was started or stopped.
    ws.send(web.request("switch-session", "av1", false, 2))
        .await;
    error(&mut ws, "switch-session", "unsupported-settings").await;
    // AV1 8-bit alone does not make AV1 HDR; HEVC Main10 does not either.
    web.harness.h.codecs.fetch_or(0x10000, Ordering::AcqRel);
    ws.send(web.request("start-session", "av1", true, 2)).await;
    error(&mut ws, "start-session", "unsupported-settings").await;
    web.harness.h.codecs.fetch_or(0x200, Ordering::AcqRel);
    ws.send(web.request("start-session", "av1", true, 2)).await;
    error(&mut ws, "start-session", "unsupported-settings").await;
    let sessions = web.harness.h.sessions.lock().unwrap();
    assert!(sessions.idle() && sessions.pending.is_empty());
    drop(sessions);
    assert_eq!(web.harness.h.webrtc_links.len(), 0);
    assert!(web.harness.h.current_app.lock().unwrap().is_some());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn hdr_on_an_sdr_display_ends_the_session_with_an_error() -> Result<()> {
    let web = web_with_every_codec().await?;
    web.harness
        .fixture
        .display_in_sdr
        .store(true, Ordering::Release);
    let mut ws = web.device("TV", u32::MAX).await;
    ws.send(web.request("start-session", "hevc", true, 2)).await;
    status(&mut ws, "session-status", "starting").await;
    let (id, peer) = answer_case(&mut ws, &HEVC.hdr()).await;
    // The worker refuses before it sends a frame, and "streaming" is not announced before the
    // refusal whenever the transport connects: the client hears an error, not SDR as HDR.
    let message = next(&mut ws).await;
    assert_eq!(message["type"], "session-status", "{message}");
    assert_eq!(message["state"], "error", "{message}");
    assert_eq!(message["sessionId"], id);
    web.idle().await;
    assert_eq!(peer.count(), 0, "no frame went out");
    assert!(UdpSocket::bind(address(web.media_port)).is_ok());
    drop(peer);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn streaming_is_not_announced_for_hdr_until_the_source_is_checked() -> Result<()> {
    let web = web_with_every_codec().await?;
    web.harness
        .fixture
        .display_in_sdr
        .store(true, Ordering::Release);
    // The capture's first frame is held back, so the transport is up before the check runs.
    web.harness
        .fixture
        .hold_first_frame
        .store(true, Ordering::Release);
    let mut ws = web.device("TV", u32::MAX).await;
    ws.send(web.request("start-session", "hevc", true, 2)).await;
    status(&mut ws, "session-status", "starting").await;
    let (id, peer) = answer_case(&mut ws, &HEVC.hdr()).await;
    until("transport connected", WAIT, || peer.connected()).await;
    // Well past the 100 ms the session watches at: nothing is announced while the check waits.
    nothing_for(&mut ws, Duration::from_millis(600)).await;
    web.harness
        .fixture
        .hold_first_frame
        .store(false, Ordering::Release);
    let message = next(&mut ws).await;
    assert_eq!(message["type"], "session-status", "{message}");
    assert_eq!(message["state"], "error", "{message}");
    assert_eq!(message["sessionId"], id);
    web.idle().await;
    assert_eq!(peer.count(), 0, "no frame went out");
    drop(peer);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_held_streaming_status_goes_out_once_the_source_is_checked() -> Result<()> {
    let web = web_with_every_codec().await?;
    web.harness
        .fixture
        .hold_first_frame
        .store(true, Ordering::Release);
    let mut ws = web.device("TV", u32::MAX).await;
    ws.send(web.request("start-session", "hevc", true, 2)).await;
    status(&mut ws, "session-status", "starting").await;
    let (id, peer) = answer_case(&mut ws, &HEVC.hdr()).await;
    until("transport connected", WAIT, || peer.connected()).await;
    nothing_for(&mut ws, Duration::from_millis(400)).await;
    web.harness
        .fixture
        .hold_first_frame
        .store(false, Ordering::Release);
    streaming(&mut ws, id).await;
    until("video received", WAIT, || peer.count() >= 3).await;
    drop(peer);
    Ok(())
}

/// The worker that waits for its first frame stops waiting when the session is stopped, as the
/// real one does; the client sees the stop, not an error.
#[tokio::test(flavor = "multi_thread")]
async fn stopping_a_held_session_ends_it_without_an_error() -> Result<()> {
    let web = web_with_every_codec().await?;
    web.harness
        .fixture
        .hold_first_frame
        .store(true, Ordering::Release);
    let mut ws = web.device("TV", u32::MAX).await;
    ws.send(web.request("start-session", "hevc", true, 2)).await;
    status(&mut ws, "session-status", "starting").await;
    let (_, peer) = answer_case(&mut ws, &HEVC.hdr()).await;
    until("transport connected", WAIT, || peer.connected()).await;
    nothing_for(&mut ws, Duration::from_millis(300)).await;

    ws.send(json!({"version":2,"type":"stop-session"})).await;
    status(&mut ws, "session-status", "stopping").await;
    status(&mut ws, "session-status", "idle").await;
    // The held worker returned on the stop: nothing is left running.
    web.idle().await;
    drop(peer);
    Ok(())
}

/// A worker stuck before its first frame (in `prepare_stream` or `open_session_capture`) never
/// vouches for the HDR source: the held "streaming" status must end in an error, not wait for good.
#[tokio::test(flavor = "multi_thread")]
async fn a_worker_that_never_checks_the_source_ends_the_held_session_with_an_error() -> Result<()> {
    let web = web_with_every_codec().await?;
    web.harness
        .h
        .webrtc_links
        .limit_held(Duration::from_millis(600));
    web.harness
        .fixture
        .hold_first_frame
        .store(true, Ordering::Release);
    let mut ws = web.device("TV", u32::MAX).await;
    ws.send(web.request("start-session", "hevc", true, 2)).await;
    status(&mut ws, "session-status", "starting").await;
    let (id, peer) = answer_case(&mut ws, &HEVC.hdr()).await;
    until("transport connected", WAIT, || peer.connected()).await;
    let message = next(&mut ws).await;
    assert_eq!(message["type"], "session-status", "{message}");
    assert_eq!(message["state"], "error", "{message}");
    assert_eq!(message["sessionId"], id);
    web.harness
        .fixture
        .hold_first_frame
        .store(false, Ordering::Release);
    drop(peer);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_display_that_leaves_hdr_ends_the_stream_with_an_error() -> Result<()> {
    let web = web_with_every_codec().await?;
    let mut ws = web.device("TV", u32::MAX).await;
    ws.send(web.request("start-session", "av1", true, 2)).await;
    status(&mut ws, "session-status", "starting").await;
    let (id, peer) = answer_case(&mut ws, &AV1.hdr()).await;
    streaming(&mut ws, id).await;
    until("video received", WAIT, || peer.count() >= 3).await;

    // Windows drops out of HDR while the frames stay FP16: the stream ends, it does not carry on
    // as SDR labelled HDR.
    web.harness
        .fixture
        .display_in_sdr
        .store(true, Ordering::Release);
    let message = next(&mut ws).await;
    assert_eq!(message["type"], "session-status", "{message}");
    assert_eq!(message["state"], "error", "{message}");
    web.idle().await;
    drop(peer);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn hdr_needs_a_display_colour_space_that_windows_reports() -> Result<()> {
    // The frames are FP16 either way, so without a reading there is nothing to tell HDR from
    // SDR content in an HDR container: the session ends instead of guessing.
    let web = web_with_every_codec().await?;
    web.harness
        .fixture
        .display_unreadable
        .store(true, Ordering::Release);
    let mut ws = web.device("TV", u32::MAX).await;
    ws.send(web.request("start-session", "hevc", true, 2)).await;
    status(&mut ws, "session-status", "starting").await;
    let (id, peer) = answer_case(&mut ws, &HEVC.hdr()).await;
    let message = next(&mut ws).await;
    assert_eq!(message["state"], "error", "{message}");
    assert_eq!(message["sessionId"], id);
    web.idle().await;
    assert_eq!(peer.count(), 0, "no frame went out");
    drop(peer);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failing_colour_space_reading_mid_stream_is_the_captures_to_report() -> Result<()> {
    let web = web_with_every_codec().await?;
    let mut ws = web.device("TV", u32::MAX).await;
    ws.send(web.request("start-session", "hevc", true, 2)).await;
    status(&mut ws, "session-status", "starting").await;
    let (id, peer) = answer_case(&mut ws, &HEVC.hdr()).await;
    streaming(&mut ws, id).await;
    until("video received", WAIT, || peer.count() >= 3).await;
    web.harness
        .fixture
        .display_unreadable
        .store(true, Ordering::Release);
    let seen = peer.count();
    until("video continues", WAIT, || peer.count() >= seen + 5).await;
    drop(peer);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_sdr_stream_does_not_care_about_the_display_mode() -> Result<()> {
    let web = web_with_every_codec().await?;
    web.harness
        .fixture
        .display_in_sdr
        .store(true, Ordering::Release);
    let mut ws = web.device("TV", u32::MAX).await;
    ws.send(web.request("start-session", "hevc", false, 2))
        .await;
    status(&mut ws, "session-status", "starting").await;
    let (id, peer) = answer_case(&mut ws, &HEVC).await;
    streaming(&mut ws, id).await;
    until("video received", WAIT, || peer.count() >= 3).await;
    drop(peer);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_whose_launch_was_hdr_asks_for_hdr_display_mode() -> Result<()> {
    let web = web_with_every_codec().await?;
    let mut ws = web.device("TV", u32::MAX).await;
    ws.send(web.request("start-session", "hevc", true, 2)).await;
    status(&mut ws, "session-status", "starting").await;
    let (id, peer) = answer_case(&mut ws, &HEVC.hdr()).await;
    streaming(&mut ws, id).await;
    let session = web.session();
    assert_eq!(
        session.launch.options.get("hdrMode").map(String::as_str),
        Some("1")
    );
    drop(peer);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unsupported_channel_count_is_still_refused() -> Result<()> {
    let web = Web::new().await?;
    let mut ws = web.device("TV", u32::MAX).await;
    for channels in [1, 4, 7] {
        ws.send(web.request("start-session", "h264", false, channels))
            .await;
        error(&mut ws, "start-session", "unsupported-settings").await;
    }
    assert!(web.harness.h.sessions.lock().unwrap().idle());
    Ok(())
}
