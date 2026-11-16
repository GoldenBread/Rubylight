//! Every way a stream ends releases the same things: /serverinfo counts no session and no
//! pending launch, and the display lease, audio route and input slots are back
//! (`Harness::empty`).

use super::*;

async fn streaming(h: &Harness) -> Result<(Arc<Session>, Client)> {
    let launch = h.launch(0, false).await?;
    let streaming = if h.short_timeout {
        h.play_short_timeout(&launch).await?
    } else {
        h.play(&launch).await?
    };
    h.counts(1, 0).await?;
    Ok(streaming)
}

/// Moonlight's stop streaming (RTSP TEARDOWN) and quit app (/cancel).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_that_stops_or_quits_leaves_nothing_behind() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let (session, client) = streaming(&h).await?;
    h.end(&session, client).await?;

    let (session, client) = streaming(&h).await?;
    let response = h.clients[0]
        .get(format!("https://127.0.0.1:{}/cancel", h.ports.https))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    assert_eq!(xml(&response, "cancel"), "1", "{response}");
    until("quit stops the stream", WAIT, || session.stopping()).await;
    drop(client);
    h.empty().await?;
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_network_drop_past_the_ping_timeout_leaves_nothing_behind() -> Result<()> {
    let h = Harness::new(2_000, 1).await?;
    let (session, client) = streaming(&h).await?;
    client.network(true);
    until(
        "the outage ends the session",
        Duration::from_millis(2_000) + WAIT,
        || session.stopping(),
    )
    .await;
    assert_eq!(session.termination_reason(), FAILED);
    drop(client);
    h.empty().await?;
    h.finish().await
}

/// Capture or the encoder failing mid-stream fails the session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_encoder_failure_leaves_nothing_behind() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let (session, client) = streaming(&h).await?;
    session.fail("the capture failed");
    until("the failure reaches the client", WAIT, || {
        client.reason.load(Ordering::Acquire) != 0
    })
    .await;
    assert_eq!(client.reason.load(Ordering::Acquire), FAILED);
    drop(client);
    h.empty().await?;
    h.finish().await
}

/// The `RustHostLastFailure` the paired device reads from `/serverinfo`.
async fn last_failure(h: &Harness) -> Result<String> {
    let response = h.clients[0]
        .get(format!("https://127.0.0.1:{}/serverinfo", h.ports.https))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    Ok(xml(&response, "RustHostLastFailure").to_owned())
}

/// A failure the stream worker never sees as an error (the capture, the control stream, a
/// Nucleus Co-op game) still gives the client the host's reason by the time it hears the
/// stream failed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failure_outside_the_stream_worker_reaches_the_client_with_its_reason() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let (session, client) = streaming(&h).await?;
    session.fail("the capture failed: the display went away");
    until("the failure reaches the client", WAIT, || {
        client.reason.load(Ordering::Acquire) != 0
    })
    .await;
    assert_eq!(client.reason.load(Ordering::Acquire), FAILED);
    assert_eq!(
        last_failure(&h).await?,
        "the capture failed: the display went away"
    );
    drop(client);
    h.empty().await?;
    h.finish().await
}

/// A stream refused at ANNOUNCE for a codec this PC cannot encode leaves the host's words in
/// the device's `/serverinfo`, as a refused launch does; the retry that asks for one it has
/// clears them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_codec_refused_at_announce_reaches_the_device_with_its_reason() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    assert_eq!(last_failure(&h).await?, "");
    // The test host encodes H.264 and HEVC only: AV1 (format 2) is refused.
    let refused = h
        .rtsp(
            &launch,
            1,
            "ANNOUNCE",
            &format!("{VIEWPORT}a=x-nv-vqos[0].bitStreamFormat:2\r\n"),
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(refused.contains("RTSP/1.0 406"), "{refused}");
    let reason = last_failure(&h).await?;
    assert!(reason.contains("AV1"), "{reason}");
    h.rtsp(&launch, 2, "ANNOUNCE", VIEWPORT).await?;
    assert_eq!(last_failure(&h).await?, "");
    h.rtsp(&launch, 3, "PLAY", "").await?;
    let session = h.h.sessions.lock().unwrap().active[&launch.id].clone();
    let client = Client::new(h.ports, launch.clone())?;
    assert!(
        established(&session, &client).await,
        "the retry did not stream"
    );
    // The harness's own end sends its TEARDOWN as message 3, which the PLAY above took.
    h.rtsp(&launch, 4, "TEARDOWN", "").await?;
    until("normal termination delivered", WAIT * 3, || {
        client.reason.load(Ordering::Acquire) != 0
    })
    .await;
    assert_eq!(session.termination_reason(), CLOSED);
    drop(client);
    h.empty().await?;
    h.finish().await
}

/// A stream that ends on an encoder error tells the client so (0x800E9403, Moonlight's
/// encoder hint), and the device's `/serverinfo` carries the host's words until its next
/// launch.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_encoder_error_reaches_the_client_with_its_reason() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let serverinfo = || last_failure(&h);
    let (session, client) = streaming(&h).await?;
    assert_eq!(serverinfo().await?, "");
    h.fixture.fail_encoder.store(true, Ordering::Release);
    until("the failure reaches the client", WAIT, || {
        client.reason.load(Ordering::Acquire) != 0
    })
    .await;
    assert_eq!(client.reason.load(Ordering::Acquire), 0x800E_9403);
    assert_eq!(session.termination_reason(), 0x800E_9403);
    assert_eq!(
        serverinfo().await?,
        "the encoder kept failing: AMF error AMF_FAIL (1)"
    );
    // Over plain HTTP the caller is nobody's device.
    let plain = h
        .http
        .get(format!("http://127.0.0.1:{}/serverinfo", h.ports.http))
        .send()
        .await?
        .text()
        .await?;
    assert_eq!(xml(&plain, "RustHostLastFailure"), "", "{plain}");
    drop(client);
    h.empty().await?;
    h.fixture.fail_encoder.store(false, Ordering::Release);
    let (next, client) = streaming(&h).await?;
    assert_eq!(
        serverinfo().await?,
        "",
        "a new launch starts without the old failure"
    );
    // The old stream's teardown finishing late does not bring it back.
    h.h.record_failure(&session);
    assert_eq!(
        serverinfo().await?,
        "",
        "an old stream's failure is not the new one's"
    );
    h.end(&next, client).await?;
    h.finish().await
}

/// The host stopping, as `main` stops it, ends its streams first.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_host_stop_leaves_nothing_behind() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let (session, client) = streaming(&h).await?;
    h.h.stop.store(true, Ordering::Release);
    let canceled = h.h.sessions.lock().unwrap().request_stop(None);
    drop(canceled);
    until("the stop ends the stream", WAIT, || session.stopping()).await;
    drop(client);
    // A stopping host takes no new launch: no `finish`.
    h.empty().await
}

/// The shutdown notice reaches each live client in order: its forwarded USB devices detach
/// before the 0x0109 termination, the worker counts it as told, and its control peer is
/// disconnected rather than left with a queued disconnect.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_host_shutdown_detaches_usb_before_it_tells_the_client() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let (session, client) = streaming(&h).await?;
    // A forwarded device still waiting for its tunnel: ending it needs no USB/IP driver.
    let (vhci, _connections) = crate::usb_forward::tests::FakeVhci::new(true);
    h.h.usb.set_vhci(vhci);
    let mut paired = h.h.paired.read().unwrap().clients[0].clone();
    paired
        .extra
        .insert("allow_usb".into(), serde_json::Value::Bool(true));
    h.h.usb.on_message(
        &session.launch.id,
        &paired,
        true,
        8,
        butterpollo_core::usb_forward::UsbMessage::AttachRequest {
            request_id: 7,
            vid: 0x0781,
            pid: 0x5581,
            class: 8,
            speed: 3,
            busid: "1-1".into(),
            name: "Flash drive".into(),
        },
    );
    // The answer to the attach is not part of the shutdown notice.
    assert!(!h.h.usb.take(&session.launch.id, Instant::now()).is_empty());

    let told = h
        .media
        .as_ref()
        .unwrap()
        .clients_told_of_shutdown(Duration::from_secs(5))
        .context("the control worker did not finish telling its clients")?;
    assert_eq!(told.0, 1, "one live stream is told");
    until("the termination reaches the client", WAIT, || {
        client.reason.load(Ordering::Acquire) != 0
    })
    .await;
    assert_eq!(client.reason.load(Ordering::Acquire), CLOSED);
    let kinds = client.kinds.lock().unwrap().clone();
    let termination = kinds
        .iter()
        .position(|&kind| kind == 0x0109)
        .context("the client heard no termination")?;
    assert!(
        termination >= 2,
        "the termination came before the USB detaches: {kinds:?}"
    );
    assert_eq!(
        &kinds[termination - 2..termination],
        &[
            butterpollo_core::usb_forward::USB_MESSAGE_TYPE,
            butterpollo_core::usb_forward::USB_MESSAGE_TYPE,
        ],
        "{kinds:?}"
    );
    until("the host disconnects the control peer", WAIT, || {
        client.disconnected.load(Ordering::Acquire)
    })
    .await;
    drop(client);
    // The shutdown ended the stream; no `finish` runs a new launch on a stopping host.
    h.empty().await
}

/// End session from the console or another device's native API.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_kick_leaves_nothing_behind() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let (session, client) = streaming(&h).await?;
    let host = h.h.clone();
    let outcome = tokio::task::spawn_blocking(move || host.end_session(None)).await?;
    assert!(outcome.is_ok(), "{outcome:?}");
    until("the kick ends the stream", WAIT, || session.stopping()).await;
    until("the kick reaches the client", WAIT, || {
        client.reason.load(Ordering::Acquire) != 0
    })
    .await;
    drop(client);
    h.empty().await?;
    h.finish().await
}

/// The same device launching again takes over its stream.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_replaced_stream_leaves_only_its_replacement() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let (old, old_client) = streaming(&h).await?;
    let launch = h.launch(0, true).await?;
    until("the replacement stops the old stream", WAIT, || {
        old.stopping()
    })
    .await;
    let (session, client) = h.play(&launch).await?;
    h.counts(1, 0).await?;
    drop(old_client);
    h.end(&session, client).await?;
    h.finish().await
}

/// A launch whose client never connects expires after 30 s.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_expired_launch_leaves_nothing_behind() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    h.counts(0, 1).await?;
    age_launch(&h, &launch, Duration::from_secs(31));
    // The harness runs no maintenance loop; /serverinfo from this PC expires launches too.
    h.counts(0, 0).await?;
    // The expired launch holds its display until the last copy goes.
    drop(launch);
    h.empty().await?;
    h.finish().await
}
