//! Background stream, parked controller, and Nucleus launch integration tests.
use super::*;

/// A device's `background_stream` launch of an app runs in Nucleus Co-op, a
/// private session of one player: the host's own app is neither replaced,
/// started nor kept, the stream is a Nucleus player's, its owner returns to
/// it and another device is refused, by the app and by its co-op entry.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn background_launches_run_in_nucleus_and_only_their_owner_returns() -> Result<()> {
    let (h, pc, entry) = coop_harness().await?;
    let portal = h.h.apps.read().unwrap().last().unwrap().id();
    for client in &mut h.h.paired.write().unwrap().clients {
        client.extra.insert("background_stream".into(), true.into());
    }
    let generation =
        h.h.current_app
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .generation
            .clone();
    let first = launch_app(&h, 0, portal).await?;
    assert!(first.contains("status_code=\"200\""), "{first}");
    let launch = pending_of(&h, 0).unwrap();
    let seat = launch.nucleus.unwrap();
    assert_eq!(seat.slot, 0);
    assert_eq!((launch.background, launch.app_id), (Some(true), portal));
    let listed = h.h.nucleus.sessions();
    assert_eq!(
        listed[0].session.kind,
        butterpollo_session::nucleus::coop::Kind::Background
    );
    assert_eq!(
        (listed[0].session.slots.len(), listed[0].app_id),
        (1, portal)
    );
    assert_eq!(h.fixture.apps_prepared.load(Ordering::Acquire), 0);
    assert_eq!(h.fixture.resources_retained.load(Ordering::Acquire), 0);
    assert_eq!(
        h.h.current_app.lock().unwrap().as_ref().unwrap().generation,
        generation
    );
    let config = butterpollo_stream::stream::effective_config(&h.h, &launch)?;
    assert!(butterpollo_stream::nucleus::player(&config));
    assert!(!butterpollo_session::background::enabled(&config));
    let refused = launch_app(&h, 1, portal).await?;
    assert!(refused.contains("status_code=\"409\""), "{refused}");
    assert!(
        refused.contains("Portal 2 can&apos;t stream in the background")
            || refused.contains("Portal 2 can't stream in the background"),
        "{refused}"
    );
    assert!(refused.contains("for a background stream"), "{refused}");
    assert!(pending_of(&h, 1).is_none());
    let by_entry = launch_app(&h, 1, entry).await?;
    assert!(by_entry.contains("status_code=\"409\""), "{by_entry}");
    // The owner's next launch returns to its own slot of the same session.
    let again = launch_app(&h, 0, portal).await?;
    assert!(again.contains("status_code=\"200\""), "{again}");
    let relaunch = pending_of(&h, 0).unwrap();
    assert_eq!(relaunch.nucleus, Some(seat));
    assert_eq!(pc.started.lock().unwrap().len(), 1);
    drop(launch);
    let canceled = h.h.sessions.lock().unwrap().pending.remove(&relaunch.id);
    drop(canceled);
    h.h.nucleus.leave(seat, &relaunch.id);
    drop(relaunch);
    // The harness's last launch is a normal one.
    for client in &mut h.h.paired.write().unwrap().clients {
        client.extra.remove("background_stream");
    }
    h.finish().await
}

#[cfg(windows)]
/// A stream's launch for client `index` of the harness, under `uuid`.
fn parked_launch(h: &Harness, index: usize, uuid: &str) -> Launch {
    let mut client = h.h.paired.read().unwrap().clients[index].clone();
    client.uuid = uuid.into();
    butterpollo_stream::state::test_support::Fixture::new().launch(client, Role::Stream)
}

/// A stream's controllers carry whether its app starts through Steam, which Automatic needs
/// before it gives Nightjar's on-screen Steam Controller controls a Steam Controller. Every
/// path that gives a stream or a launch its controllers (the control loop and its retry, a
/// WebRTC stream and its retry, the launch's preparation) takes them through
/// `controller_sessions::acquire_for` with the stream's own launch and settings, as here: its
/// app with a Steam game's mark says yes, the Desktop no. The harness's control loop takes
/// none itself: it makes no injector before input comes (`ControlPeer::injector_tried`), and
/// input would reach this PC's desktop.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_streams_controllers_know_whether_its_app_starts_through_steam() -> Result<()> {
    use butterpollo_stream::controller_sessions::{acquire_for, key};
    let h = Harness::new(10_000, 1).await?;
    let pool = &h.h.controller_sessions;
    // The harness's app, already running, with or without a Steam game's mark: its launch
    // neither switches apps nor starts anything.
    let mark = |marked: bool| {
        let mut apps = h.h.apps.write().unwrap();
        if marked {
            apps[0].extra.insert("steam-id".into(), "570".into());
        } else {
            apps[0].extra.remove("steam-id");
        }
    };
    let steam_app = |session: &Session| -> Result<Option<bool>> {
        let config = effective_config(&h.h, &session.launch)?;
        drop(acquire_for(&h.h, &session.launch, &config)?);
        Ok(pool
            .policy(&key(&session.launch))
            .map(|policy| policy.steam_app))
    };
    for marked in [true, false] {
        mark(marked);
        let launch = h.launch(0, false).await?;
        let (session, client) = h.play(&launch).await?;
        assert_eq!(steam_app(&session)?, Some(marked), "marked: {marked}");
        h.end(&session, client).await?;
    }
    pool.release_parked(WAIT, &Default::default())?;
    h.finish().await
}

// Nucleus Co-op parks controllers through XInput, on Windows only.
#[cfg(windows)]
/// #56: a desktop stream that ended parks its controller, which keeps the
/// first XInput slot, and the background game started next read that pad
/// instead of the player's. A new background session releases parked
/// controllers before it plugs its player's pad, but not a pending launch's,
/// which Nightjar prepares before its stream takes them, nor a running
/// stream's, which its control stream takes at its first input. A launch
/// the registry refuses, and the owner returning to its session, plug
/// nothing and leave them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_background_stream_releases_parked_controllers_first() -> Result<()> {
    use butterpollo_stream::controller_sessions::key;
    use std::collections::BTreeSet;
    let (h, _pc, _) = coop_harness().await?;
    let portal = h.h.apps.read().unwrap().last().unwrap().id();
    let none = BTreeSet::new();
    let config = butterpollo_config::config::Config::default();
    let pool = &h.h.controller_sessions;
    // Client 1 streams the desktop; its controllers wait for its first
    // input.
    let streaming = h.launch(1, false).await?;
    let (session, client) = h.play(&streaming).await?;
    for client in &mut h.h.paired.write().unwrap().clients {
        client.extra.insert("background_stream".into(), true.into());
    }
    // A desktop stream that ended, waiting for its reconnect, and a launch
    // whose controllers Nightjar prepared.
    let (gap, prepared) = (
        parked_launch(&h, 1, "gap"),
        parked_launch(&h, 1, "nightjar"),
    );
    // Parked together: a client's acquire evicts the others' parked ones.
    let held: Vec<_> = [&streaming, &gap, &prepared]
        .into_iter()
        .map(|launch| pool.acquire(launch, &config, false))
        .collect::<Result<_>>()?;
    drop(held);
    h.h.sessions
        .lock()
        .unwrap()
        .pending
        .insert(prepared.id.clone(), prepared.clone());
    let kept = BTreeSet::from([key(&prepared), key(&streaming)]);
    assert!(pool.parked(&kept), "the gap's controller is parked");
    let first = launch_app(&h, 0, portal).await?;
    assert!(first.contains("status_code=\"200\""), "{first}");
    assert!(!pool.parked(&kept), "the gap's controller is gone");
    assert!(
        pool.parked(&BTreeSet::from([key(&prepared)])),
        "the running stream's stays"
    );
    assert!(
        pool.parked(&BTreeSet::from([key(&streaming)])),
        "the prepared one stays"
    );
    let canceled = h.h.sessions.lock().unwrap().pending.remove(&prepared.id);
    drop((canceled, streaming));
    let launch = pending_of(&h, 0).unwrap();
    let seat = launch.nucleus.unwrap();
    // Client 1 is refused: the session is client 0's. Its launch ends its
    // desktop stream first.
    let refused = launch_app(&h, 1, portal).await?;
    assert!(refused.contains("status_code=\"409\""), "{refused}");
    assert!(pool.parked(&none));
    let again = launch_app(&h, 0, portal).await?;
    assert!(again.contains("status_code=\"200\""), "{again}");
    assert!(pool.parked(&none));
    let relaunch = pending_of(&h, 0).unwrap();
    drop(launch);
    let canceled = h.h.sessions.lock().unwrap().pending.remove(&relaunch.id);
    drop(canceled);
    h.h.nucleus.leave(seat, &relaunch.id);
    drop((relaunch, session, client));
    pool.release_parked(Duration::from_secs(5), &none)?;
    // The harness's last launch is a normal one.
    for client in &mut h.h.paired.write().unwrap().clients {
        client.extra.remove("background_stream");
    }
    h.finish().await
}

// Nucleus Co-op parks controllers through XInput, on Windows only.
#[cfg(windows)]
/// #56: a new co-op session plugs its players' pads too, so it releases the
/// controllers that streams which ended still park first.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_co_op_session_releases_parked_controllers_first() -> Result<()> {
    let (h, _pc, id) = coop_harness().await?;
    let none = std::collections::BTreeSet::new();
    let gap = parked_launch(&h, 0, "gap");
    drop(h.h.controller_sessions.acquire(
        &gap,
        &butterpollo_config::config::Config::default(),
        false,
    )?);
    assert!(h.h.controller_sessions.parked(&none));
    let started = launch_app(&h, 1, id).await?;
    assert!(started.contains("status_code=\"200\""), "{started}");
    assert!(!h.h.controller_sessions.parked(&none));
    let launch = pending_of(&h, 1).unwrap();
    let canceled = h.h.sessions.lock().unwrap().pending.remove(&launch.id);
    drop((canceled, launch));
    h.finish().await
}

/// Moonlight and Nightjar end the stream first and then quit: a quit from
/// the device whose background stream already ended still ends that
/// session, by Moonlight's /cancel or Nightjar's Close game, instead of
/// leaving Nucleus Co-op running it for the two minutes its slot is kept
/// (#56). The host's own app stays.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_quit_after_the_stream_ended_ends_the_background_stream() -> Result<()> {
    let (h, pc, _) = coop_harness().await?;
    let portal = h.h.apps.read().unwrap().last().unwrap().id();
    for client in &mut h.h.paired.write().unwrap().clients {
        client.extra.insert("background_stream".into(), true.into());
        client.extra.insert("host_control".into(), true.into());
    }
    let generation = || {
        h.h.current_app
            .lock()
            .unwrap()
            .as_ref()
            .map(|app| app.generation.clone())
    };
    let before = generation();
    // The stream ends: its launch is gone, its slot kept for the device.
    let vacate = |h: &Harness| {
        let launch = pending_of(h, 0).unwrap();
        let seat = launch.nucleus.unwrap();
        let canceled = h.h.sessions.lock().unwrap().pending.remove(&launch.id);
        drop(canceled);
        h.h.nucleus.leave(seat, &launch.id);
        seat
    };
    let ended = |seat: butterpollo_session::session::Seat| {
        h.h.nucleus
            .sessions()
            .iter()
            .find(|listed| listed.session.id == seat.session)
            .is_none_or(|listed| listed.session.ended())
    };
    assert!(
        launch_app(&h, 0, portal)
            .await?
            .contains("status_code=\"200\"")
    );
    let seat = vacate(&h);
    assert!(!ended(seat));
    let response = h.clients[0]
        .get(format!("https://127.0.0.1:{}/cancel", h.ports.https))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    assert_eq!(xml(&response, "cancel"), "1", "{response}");
    assert!(ended(seat));
    assert!(pc.started.lock().unwrap()[0].stopped());
    assert_eq!(generation(), before);

    // Nightjar quits with Close game where the device has host control.
    assert!(
        launch_app(&h, 0, portal)
            .await?
            .contains("status_code=\"200\"")
    );
    let seat = vacate(&h);
    assert_eq!(pc.started.lock().unwrap().len(), 2);
    let host = h.h.clone();
    let client = h.h.paired.read().unwrap().clients[0].clone();
    tokio::task::spawn_blocking(move || {
        butterpollo_stream::session_actions::session_action(
            &host,
            &client,
            butterpollo_stream::session_actions::Action::CloseGame,
        )
    })
    .await?
    .map_err(|failure| anyhow::anyhow!("{failure:?}"))?;
    assert!(ended(seat));
    assert!(pc.started.lock().unwrap()[1].stopped());
    assert_eq!(generation(), before);

    // Native End too. A device that owns the host's app quits that app, as
    // Moonlight shows it running, and its background stream stays.
    assert!(
        launch_app(&h, 0, portal)
            .await?
            .contains("status_code=\"200\"")
    );
    let seat = vacate(&h);
    let owner = |who: &str| {
        let mut app = h.h.current_app.lock().unwrap();
        std::mem::replace(&mut app.as_mut().unwrap().owner, who.into())
    };
    let previous = owner("0");
    let host = h.h.clone();
    let quit = tokio::task::spawn_blocking(move || {
        butterpollo_stream::nvhttp::quit_nucleus_stream(&host, "0", false)
    })
    .await?
    .map_err(|failure| anyhow::anyhow!("{failure:?}"))?;
    assert!(!quit);
    assert!(!ended(seat));
    owner(&previous);
    let host = h.h.clone();
    let client = h.h.paired.read().unwrap().clients[0].clone();
    tokio::task::spawn_blocking(move || {
        butterpollo_stream::session_actions::session_action(
            &host,
            &client,
            butterpollo_stream::session_actions::Action::End,
        )
    })
    .await?
    .map_err(|failure| anyhow::anyhow!("{failure:?}"))?;
    assert!(ended(seat));
    assert!(pc.started.lock().unwrap()[2].stopped());
    assert_eq!(generation(), before);
    for client in &mut h.h.paired.write().unwrap().clients {
        client.extra.remove("background_stream");
        client.extra.remove("host_control");
    }
    h.finish().await
}

// Nucleus Co-op parks controllers through XInput, on Windows only.
#[cfg(windows)]
/// A one-player game without a hub handler streams in the background with
/// Rubylight's own handler, written once at the launch: the stream is a
/// Nucleus player's like any other, with nothing added on its way.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_background_launch_without_a_hub_handler_runs_with_ours() -> Result<()> {
    let (h, pc, _) = coop_harness().await?;
    let mut ice = h.h.apps.read().unwrap().last().unwrap().clone();
    ice.name = "Ice".into();
    ice.cmd = r"C:\NoSuchGames\Ice\Ice.exe".into();
    ice.extra.insert("background-stream".into(), true.into());
    let id = ice.id();
    h.h.apps.write().unwrap().push(ice);
    let started = launch_app(&h, 0, id).await?;
    assert!(started.contains("status_code=\"200\""), "{started}");
    let launch = pending_of(&h, 0).unwrap();
    let seat = launch.nucleus.unwrap();
    let own = pc.own.lock().unwrap().clone();
    assert_eq!(own.len(), 1);
    let listed = h.h.nucleus.sessions();
    assert_eq!(listed[0].session.game, own[0].guid());
    assert_eq!((listed[0].session.slots.len(), listed[0].app_id), (1, id));
    assert_eq!(h.fixture.apps_prepared.load(Ordering::Acquire), 0);
    let config = butterpollo_stream::stream::effective_config(&h.h, &launch)?;
    assert!(butterpollo_stream::nucleus::player(&config));
    let canceled = h.h.sessions.lock().unwrap().pending.remove(&launch.id);
    drop(canceled);
    h.h.nucleus.leave(seat, &launch.id);
    drop(launch);
    h.finish().await
}

/// A background launch Nucleus Co-op can't run is refused with the reason
/// and streams nothing: no display, no app, nothing pending. An app's own
/// `background-stream` asks for one as the device's setting does.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_background_launch_nucleus_cannot_run_streams_nothing() -> Result<()> {
    let (h, pc, _) = coop_harness().await?;
    // A launcher's link names no game for Nucleus Co-op to start.
    let mut store = h.h.apps.read().unwrap().last().unwrap().clone();
    store.name = "Store Game".into();
    store.cmd = "com.epicgames.launcher://apps/x?action=launch".into();
    store.extra.insert("background-stream".into(), true.into());
    let id = store.id();
    h.h.apps.write().unwrap().push(store);
    let missing = launch_app(&h, 0, id).await?;
    assert!(missing.contains("status_code=\"404\""), "{missing}");
    assert!(
        missing.contains("Store Game does not name one"),
        "{missing}"
    );
    assert!(missing.contains("stream in the background"), "{missing}");
    assert!(pending_of(&h, 0).is_none());
    // Where Nucleus Co-op can't run at all, 503 and why.
    h.h.nucleus.use_pc(Arc::new(
        butterpollo_stream::nucleus::fake::Pc::unavailable(),
    ));
    let unavailable = launch_app(&h, 0, id).await?;
    assert!(unavailable.contains("status_code=\"503\""), "{unavailable}");
    assert!(unavailable.contains("not on this test PC"), "{unavailable}");
    h.counts(0, 0).await?;
    assert_eq!(h.fixture.displays.load(Ordering::Acquire), 0);
    assert_eq!(h.fixture.apps_prepared.load(Ordering::Acquire), 0);
    assert!(pc.started.lock().unwrap().is_empty());
    h.finish().await
}

/// #56: the launch that reaches a background stream's session no longer asks
/// to play on the PC, whatever the device does.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_background_launch_never_plays_its_game_on_the_pc() -> Result<()> {
    let (h, _pc, _entry) = coop_harness().await?;
    let portal = h.h.apps.read().unwrap().last().unwrap().id();
    let launch = || {
        h.clients[0]
            .get(format!("https://127.0.0.1:{}/launch", h.ports.https))
            .query(&[
                ("appid", portal.to_string()),
                ("rikey", hex::encode(crypto::random::<16>())),
                ("rikeyid", "1".into()),
                ("corever", "1".into()),
                ("mode", "1281x721x60".into()),
                ("localAudioPlayMode", "1".into()),
            ])
            .send()
    };
    h.h.paired.write().unwrap().clients[0]
        .extra
        .insert("background_stream".into(), true.into());
    let reply = launch().await?.text().await?;
    assert!(reply.contains("status_code=\"200\""), "{reply}");
    let pending = pending_of(&h, 0).unwrap();
    assert_eq!(pending.background, Some(true));
    assert!(!pending.host_audio);
    Ok(())
}
