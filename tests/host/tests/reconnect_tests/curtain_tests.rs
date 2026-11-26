//! The launch curtain (0x5548) through the real HTTP, RTSP and control loop: only a launch that
//! asks, on a host with `launch_curtain` on, gets it; the state reaches the device as it
//! changes, and a stock launch never sees a frame of it.
use super::*;
use rubylight_protocol::launch_curtain::{CurtainState, LAUNCH_PARAMETER, Phase, flags};

const ON: &str = "launch_curtain=true\n";

/// The DESCRIBE's `a=x-rl-control` lines, joined.
async fn control_ids(h: &Harness, launch: &Launch, sequence: u32) -> Result<String> {
    let describe = h.rtsp(launch, sequence, "DESCRIBE", "").await?;
    Ok(describe
        .lines()
        .filter(|line| line.starts_with("a=x-rl-control:"))
        .collect::<Vec<_>>()
        .join("|"))
}

/// The curtain states the client got so far.
fn states(client: &Client) -> Vec<CurtainState> {
    client
        .curtain
        .lock()
        .unwrap()
        .iter()
        .map(|(_, payload)| CurtainState::decode(payload).expect("a valid curtain state"))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn launch_curtain_is_offered_only_to_a_launch_that_asks_on_a_host_with_it_on() -> Result<()> {
    let asked = [(LAUNCH_PARAMETER, "1")];
    let off = Harness::new(10_000, 1).await?;
    let launch = off.launch_with(0, false, None, &asked).await?;
    assert!(!control_ids(&off, &launch, 10).await?.contains("0x5548"));
    let (session, client) = off.play(&launch).await?;
    off.end(&session, client).await?;
    off.finish().await?;

    let h = Harness::configured(10_000, 1, ON).await?;
    let stock = h.launch(0, false).await?;
    assert!(!control_ids(&h, &stock, 10).await?.contains("0x5548"));
    let (session, client) = h.play(&stock).await?;
    h.end(&session, client).await?;
    let launch = h.launch_with(0, false, None, &asked).await?;
    assert!(control_ids(&h, &launch, 11).await?.contains("0x5548"));
    let (session, client) = h.play(&launch).await?;
    h.end(&session, client).await?;
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn launch_curtain_state_reaches_the_device_and_lifts() -> Result<()> {
    let h = Harness::configured(10_000, 1, ON).await?;
    let launch = h
        .launch_with(0, false, None, &[(LAUNCH_PARAMETER, "1")])
        .await?;
    // The fixture's app has no game window to wait for: the host begins a curtain for this
    // launch as a real start would, then lifts it.
    h.h.curtain
        .begin(&launch.id, &launch.client.uuid, "fixture".into(), false);
    let (session, client) = h.play(&launch).await?;
    until("the curtain's first state arrives", WAIT, || {
        !client.curtain.lock().unwrap().is_empty()
    })
    .await;
    let first = states(&client)[0].clone();
    assert_eq!(first.phase, Phase::StartingApp);
    assert!(first.has(flags::DISMISSIBLE));
    h.h.curtain
        .dismiss(&launch.client.uuid)
        .expect("the owner lifts it");
    until("the lifted state arrives", WAIT, || {
        states(&client)
            .last()
            .is_some_and(|s| s.phase == Phase::Dismissed)
    })
    .await;
    let all = states(&client);
    assert!(
        all.windows(2)
            .all(|pair| pair[1].sequence > pair[0].sequence)
    );
    // Lifted for good: nothing more is sent.
    let sent = all.len();
    h.h.curtain
        .begin("another-launch", "someone-else", "g".into(), false);
    client.quiet_heartbeat().await;
    assert_eq!(states(&client).len(), sent);
    h.end(&session, client).await?;
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn launch_curtain_of_a_launch_without_a_game_window_is_lifted_at_once() -> Result<()> {
    let h = Harness::configured(10_000, 1, ON).await?;
    let launch = h
        .launch_with(0, false, None, &[(LAUNCH_PARAMETER, "1")])
        .await?;
    let (session, client) = h.play(&launch).await?;
    until("the lifted state arrives", WAIT, || {
        !client.curtain.lock().unwrap().is_empty()
    })
    .await;
    let state = states(&client)[0].clone();
    assert_eq!(state.phase, Phase::Ready);
    assert!(state.has(flags::DESKTOP_OR_LAUNCHER));
    h.end(&session, client).await?;
    h.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn launch_curtain_never_reaches_a_stock_launch() -> Result<()> {
    let h = Harness::configured(10_000, 1, ON).await?;
    let launch = h.launch(0, false).await?;
    h.h.curtain
        .begin(&launch.id, &launch.client.uuid, "fixture".into(), false);
    let (session, client) = h.play(&launch).await?;
    client.quiet_heartbeat().await;
    client.quiet_heartbeat().await;
    assert!(client.curtain.lock().unwrap().is_empty());
    assert!(
        !client
            .kinds
            .lock()
            .unwrap()
            .contains(&rubylight_protocol::launch_curtain::LAUNCH_CURTAIN_STATE_MESSAGE_TYPE)
    );
    h.end(&session, client).await?;
    h.finish().await
}
