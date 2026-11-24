//! WebRTC session switching, Moonlight replacement, remote sessions, and cleanup tests.
use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn a_device_that_may_not_launch_or_names_no_app_starts_nothing() -> Result<()> {
    let web = Web::new().await?;
    let mut viewer = web.device("Viewer", VIEW | LIST).await;
    viewer
        .send(web.request("start-session", "h264", false, 2))
        .await;
    error(&mut viewer, "start-session", "not-authenticated").await;
    viewer
        .send(json!({"version":2,"type":"stop-host-session"}))
        .await;
    error(&mut viewer, "stop-host-session", "not-authenticated").await;

    let mut ws = web.device("TV", u32::MAX).await;
    let mut request = web.request("start-session", "h264", false, 2);
    request["appId"] = json!("99999999");
    ws.send(request).await;
    status(&mut ws, "session-status", "starting").await;
    error(&mut ws, "start-session", "invalid-application").await;
    let mut request = web.request("start-session", "h264", false, 2);
    request["appId"] = json!("not a number");
    ws.send(request).await;
    error(&mut ws, "start-session", "invalid-application").await;
    web.idle().await;
    assert!(web.harness.h.sessions.lock().unwrap().pending.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn switch_session_resumes_the_running_app_and_stop_host_ends_it() -> Result<()> {
    let web = Web::new().await?;
    let mut ws = web.device("TV", u32::MAX).await;
    ws.send(web.request("start-session", "h264", false, 2))
        .await;
    status(&mut ws, "session-status", "starting").await;
    let (first, peer) = answer(&mut ws, VideoCodec::H264).await;
    streaming(&mut ws, first).await;
    until("video received", WAIT, || peer.count() >= 2).await;

    // The target is the running app: the stream restarts and the app stays.
    ws.send(web.request("switch-session", "hevc", false, 2))
        .await;
    let resuming = status(&mut ws, "host-session-status", "resuming").await;
    assert_eq!(resuming["runningAppId"], web.app());
    assert_eq!(resuming["targetAppId"], web.app());
    status(&mut ws, "session-status", "stopping").await;
    status(&mut ws, "session-status", "idle").await;
    status(&mut ws, "host-session-status", "starting").await;
    drop(peer);
    let (second, peer) = answer(&mut ws, VideoCodec::Hevc).await;
    assert_ne!(first, second);
    streaming(&mut ws, second).await;
    until("video of the new session", WAIT, || peer.keyframes(1) >= 1).await;
    assert!(web.harness.h.current_app.lock().unwrap().is_some());

    // stop-host-session ends the stream and the app.
    ws.send(json!({"version":2,"type":"stop-host-session"}))
        .await;
    let stopping = status(&mut ws, "host-session-status", "stopping").await;
    assert_eq!(stopping["runningAppId"], web.app());
    status(&mut ws, "session-status", "stopping").await;
    status(&mut ws, "session-status", "idle").await;
    status(&mut ws, "host-session-status", "stopped").await;
    web.idle().await;
    assert!(web.harness.h.current_app.lock().unwrap().is_none());
    // With nothing running it says so at once.
    ws.send(json!({"version":2,"type":"stop-host-session"}))
        .await;
    status(&mut ws, "host-session-status", "stopped").await;
    drop(peer);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_webrtc_device_is_refused_while_one_streams() -> Result<()> {
    let web = Web::new().await?;
    let mut first = web.device("TV", u32::MAX).await;
    first
        .send(web.request("start-session", "h264", false, 2))
        .await;
    status(&mut first, "session-status", "starting").await;
    let (id, peer) = answer(&mut first, VideoCodec::H264).await;
    streaming(&mut first, id).await;

    let mut second = web.device("Phone", u32::MAX).await;
    second
        .send(web.request("start-session", "h264", false, 2))
        .await;
    status(&mut second, "session-status", "starting").await;
    error(&mut second, "start-session", "session-active").await;
    // The first stream did not notice.
    assert!(!web.session().stopping());
    // The same device cannot start a second one on its own connection either.
    first
        .send(web.request("start-session", "h264", false, 2))
        .await;
    error(&mut first, "start-session", "session-active").await;
    drop(peer);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_moonlight_launch_replaces_a_webrtc_stream() -> Result<()> {
    let web = Web::new().await?;
    let mut ws = web.device("TV", u32::MAX).await;
    ws.send(web.request("start-session", "h264", false, 2))
        .await;
    status(&mut ws, "session-status", "starting").await;
    let (id, peer) = answer(&mut ws, VideoCodec::H264).await;
    streaming(&mut ws, id).await;
    until("video received", WAIT, || peer.count() >= 2).await;
    let webrtc = web.session();

    let launch = web.harness.launch(0, false).await?;
    let idle = status(&mut ws, "session-status", "idle").await;
    assert_eq!(idle["sessionId"], id);
    assert!(webrtc.stopping());
    let (session, client) = web.harness.play(&launch).await?;
    assert_eq!(
        session.launch.transport,
        butterpollo_session::session::Transport::GameStream
    );
    web.harness.counts(1, 0).await?;
    web.harness.end(&session, client).await?;
    drop(peer);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_webrtc_launch_replaces_a_moonlight_stream() -> Result<()> {
    let web = Web::new().await?;
    let launch = web.harness.launch(0, false).await?;
    let (moonlight, client) = web.harness.play(&launch).await?;

    let mut ws = web.device("TV", u32::MAX).await;
    ws.send(web.request("start-session", "h264", false, 2))
        .await;
    status(&mut ws, "session-status", "starting").await;
    let (id, peer) = answer(&mut ws, VideoCodec::H264).await;
    streaming(&mut ws, id).await;
    until("the Moonlight stream ended", WAIT, || moonlight.stopping()).await;
    until("video received", WAIT, || peer.count() >= 2).await;
    assert_eq!(
        web.session().launch.transport,
        butterpollo_session::session::Transport::WebRtc
    );
    drop(client);

    ws.send(json!({"version":2,"type":"stop-session"})).await;
    status(&mut ws, "session-status", "stopping").await;
    status(&mut ws, "session-status", "idle").await;
    web.idle().await;
    drop(peer);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_the_signalling_socket_ends_the_stream() -> Result<()> {
    let web = Web::new().await?;
    let mut ws = web.device("TV", u32::MAX).await;
    ws.send(web.request("start-session", "h264", false, 2))
        .await;
    status(&mut ws, "session-status", "starting").await;
    let (id, peer) = answer(&mut ws, VideoCodec::H264).await;
    streaming(&mut ws, id).await;
    let session = web.session();
    drop(ws);
    until("the session stopped", WAIT, || session.stopping()).await;
    web.idle().await;
    drop(peer);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unanswered_offer_is_dropped_when_the_client_stops() -> Result<()> {
    let web = Web::new().await?;
    let mut ws = web.device("TV", u32::MAX).await;
    ws.send(web.request("start-session", "h264", false, 2))
        .await;
    status(&mut ws, "session-status", "starting").await;
    status(&mut ws, "session-status", "starting-webrtc").await;
    assert_eq!(next(&mut ws).await["type"], "offer");
    assert_eq!(web.harness.h.sessions.lock().unwrap().pending.len(), 1);
    // Nothing was streaming, so there is nothing to stop: straight to idle.
    ws.send(json!({"version":2,"type":"stop-session"})).await;
    status(&mut ws, "session-status", "idle").await;
    web.idle().await;
    assert!(web.harness.h.sessions.lock().unwrap().pending.is_empty());
    // The port is free for the next offer, and the app was left running.
    assert!(UdpSocket::bind(address(web.media_port)).is_ok());
    assert!(web.harness.h.current_app.lock().unwrap().is_some());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn with_the_setting_off_no_media_socket_opens() -> Result<()> {
    let mut web = Web::new().await?;
    // The harness host has the setting off: `start` opens nothing, not even the media port.
    web.server = None;
    assert!(!web.harness.h.config.read().unwrap().webrtc_server());
    assert!(
        server::start(
            web.harness.h.clone(),
            [127, 0, 0, 1].into(),
            web.harness.media.clone()
        )
        .is_none()
    );
    assert!(UdpSocket::bind(address(web.media_port)).is_ok());
    assert_eq!(web.harness.h.webrtc_links.len(), 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn remote_sessions_do_not_hold_up_a_switch_or_stop_host_session() -> Result<()> {
    let web = Web::new().await?;
    let mut ws = web.device("TV", u32::MAX).await;
    web.clear_app();
    let peer = stream(&web, &mut ws).await;
    assert_eq!(web.owner(), web.uuid("TV"));
    let monitor = web.add_monitor();

    // Resuming the app stops the stream only: waiting for the monitor would time out.
    let started = Instant::now();
    ws.send(web.request("switch-session", "h264", false, 2))
        .await;
    status(&mut ws, "host-session-status", "resuming").await;
    status(&mut ws, "session-status", "stopping").await;
    status(&mut ws, "session-status", "idle").await;
    status(&mut ws, "host-session-status", "starting").await;
    assert!(started.elapsed() < WAIT);
    drop(peer);
    let (id, peer) = answer(&mut ws, VideoCodec::H264).await;
    streaming(&mut ws, id).await;

    // So does ending the app, which its owner may do whoever else is connected.
    ws.send(json!({"version":2,"type":"stop-host-session"}))
        .await;
    status(&mut ws, "host-session-status", "stopping").await;
    status(&mut ws, "session-status", "stopping").await;
    status(&mut ws, "session-status", "idle").await;
    status(&mut ws, "host-session-status", "stopped").await;
    assert!(web.harness.h.current_app.lock().unwrap().is_none());
    assert!(!monitor.stopping());
    assert_eq!(web.harness.h.sessions.lock().unwrap().active.len(), 1);
    drop(peer);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_owner_may_end_the_app_while_remote_sessions_are_active() -> Result<()> {
    let web = Web::new().await?;
    let mut tv = web.device("TV", u32::MAX).await;
    web.clear_app();
    let peer = stream(&web, &mut tv).await;
    assert_eq!(web.owner(), web.uuid("TV"));
    let session = web.session();
    let monitor = web.add_monitor();

    // Another device with every permission is refused, as Moonlight's cancel answers a 403,
    // before anything is announced or stopped.
    let mut phone = web.device("Phone", u32::MAX).await;
    phone
        .send(json!({"version":2,"type":"stop-host-session"}))
        .await;
    error(&mut phone, "stop-host-session", "session-active").await;
    let mut other_app = web.request("switch-session", "h264", false, 2);
    other_app["appId"] = json!("12345");
    phone.send(other_app).await;
    error(&mut phone, "switch-session", "session-active").await;
    assert!(!session.stopping());
    assert!(web.harness.h.current_app.lock().unwrap().is_some());

    // With no remote session left anyone may end it.
    web.harness
        .h
        .sessions
        .lock()
        .unwrap()
        .active
        .remove(&monitor.launch.id);
    phone
        .send(json!({"version":2,"type":"stop-host-session"}))
        .await;
    status(&mut phone, "host-session-status", "stopping").await;
    status(&mut phone, "host-session-status", "stopped").await;
    assert!(session.stopping());
    assert!(web.harness.h.current_app.lock().unwrap().is_none());
    drop(peer);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_worker_that_fails_before_taking_its_link_still_frees_the_media_port() -> Result<()> {
    let web = Web::new().await?;
    web.harness
        .fixture
        .webrtc_fails_before_its_link
        .store(true, Ordering::Release);
    let mut ws = web.device("TV", u32::MAX).await;
    ws.send(web.request("start-session", "h264", false, 2))
        .await;
    status(&mut ws, "session-status", "starting").await;
    let (_, peer) = answer(&mut ws, VideoCodec::H264).await;
    // The link was handed over and the worker never took it: the teardown releases it.
    web.idle().await;
    assert!(UdpSocket::bind(address(web.media_port)).is_ok());
    drop(peer);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_that_starts_again_gets_the_media_socket_from_its_unanswered_start() -> Result<()>
{
    let web = Web::new().await?;
    let credentials = web.pair("TV", u32::MAX);
    let mut old = web.sign_in(&credentials).await;
    old.send(web.request("start-session", "h264", false, 2))
        .await;
    status(&mut old, "session-status", "starting").await;
    status(&mut old, "session-status", "starting-webrtc").await;
    assert_eq!(next(&mut old).await["type"], "offer");

    // The offer is still waiting for its answer, on the one media port, when the device returns
    // on a new connection, which closes the old one. Releasing the old start's launch is slow, as
    // a virtual display that takes seconds to go is, and the new start must not wait for it.
    web.harness
        .fixture
        .audio_release_ms
        .store(4_000, Ordering::Release);
    let mut new = web.sign_in(&credentials).await;
    while old.next().await.is_some() {}
    new.send(web.request("start-session", "h264", false, 2))
        .await;
    status(&mut new, "session-status", "starting").await;
    let (id, peer) = answer(&mut new, VideoCodec::H264).await;
    streaming(&mut new, id).await;
    until("video received", WAIT, || peer.count() >= 2).await;
    assert_eq!(web.harness.h.sessions.lock().unwrap().pending.len(), 0);
    web.harness
        .fixture
        .audio_release_ms
        .store(0, Ordering::Release);
    drop(peer);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn cancelling_a_start_releases_its_resources_off_the_async_tasks() -> Result<()> {
    let web = Web::new().await?;
    let mut ws = web.device("TV", u32::MAX).await;
    ws.send(web.request("start-session", "h264", false, 2))
        .await;
    status(&mut ws, "session-status", "starting").await;
    status(&mut ws, "session-status", "starting-webrtc").await;
    assert_eq!(next(&mut ws).await["type"], "offer");

    // Dropping the launch takes this long, as a virtual display or audio route that blocks does.
    // The one thread of this runtime runs the session actor, the signalling server and this
    // test, so a release on it shows as a gap between ticks.
    web.harness
        .fixture
        .audio_release_ms
        .store(600, Ordering::Release);
    let gap = Arc::new(AtomicU64::new(0));
    let ticker = tokio::spawn({
        let gap = gap.clone();
        async move {
            loop {
                let before = Instant::now();
                tokio::time::sleep(Duration::from_millis(10)).await;
                gap.fetch_max(before.elapsed().as_millis() as u64, Ordering::AcqRel);
            }
        }
    });
    ws.send(json!({"version":2,"type":"stop-session"})).await;
    status(&mut ws, "session-status", "idle").await;
    web.idle().await;
    // Let the ticker report a stall that ended just before.
    tokio::time::sleep(Duration::from_millis(50)).await;
    ticker.abort();
    web.harness
        .fixture
        .audio_release_ms
        .store(0, Ordering::Release);
    let longest = gap.load(Ordering::Acquire);
    assert!(longest < 300, "the runtime stalled for {longest} ms");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_webrtc_launch_records_its_device_as_the_apps_owner() -> Result<()> {
    let web = Web::new().await?;
    web.clear_app();
    let mut ws = web.device("TV", u32::MAX).await;
    let peer = stream(&web, &mut ws).await;
    // As a Moonlight launch does, so the owner guard of cancel applies to this app.
    assert_eq!(web.owner(), web.uuid("TV"));
    assert_eq!(web.session().launch.client.uuid, web.uuid("TV"));
    drop(peer);
    Ok(())
}
