//! Non-owner resume and join behavior across WebRTC and Moonlight streams.
use super::*;

/// "Stream here too" on a device that does not own the game: `/resume` joins it beside the owner,
/// and takes nothing from it. With no game there is nothing to join, and nothing starts one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resume_by_a_device_that_does_not_own_the_game_joins_and_never_starts_one() -> Result<()>
{
    let h = Harness::new(10_000, 2).await?;
    let owner = "0".to_owned();
    h.h.current_app.lock().unwrap().as_mut().unwrap().owner = owner.clone();
    let launch = h.launch(1, true).await?;
    assert_eq!(launch.client.uuid, "1");
    h.counts(0, 1).await?;
    assert_eq!(
        h.h.current_app.lock().unwrap().as_ref().unwrap().owner,
        owner
    );
    assert!(!h.h.owns_game("1"));
    let (session, client) = h.play(&launch).await?;
    assert_eq!(
        h.h.current_app.lock().unwrap().as_ref().unwrap().owner,
        owner
    );
    h.end(&session, client).await?;

    *h.h.current_app.lock().unwrap() = None;
    let refused = h.launch_response(1, true).await;
    assert!(
        refused
            .as_ref()
            .map_or(true, |reply| !reply.contains("status_code=\"200\"")),
        "{refused:?}"
    );
    assert!(h.h.current_app.lock().unwrap().is_none(), "nothing started");
    h.finish().await
}

/// The stream of `client`'s launch, running over `transport` as far as the registry can tell.
async fn running_over(h: &Harness, client: usize, transport: Transport) -> Result<Arc<Session>> {
    let mut launch = h.launch(client, true).await?;
    let queued = h.h.sessions.lock().unwrap().pending.remove(&launch.id);
    drop(queued);
    launch.transport = transport;
    let session = Session::new(launch, Default::default());
    h.h.sessions
        .lock()
        .unwrap()
        .active
        .insert(session.launch.id.clone(), session.clone());
    Ok(session)
}

/// Ends `session` a moment after it is told to stop, as its worker would: a launch waits for that.
fn ends_when_stopped(h: &Harness, session: &Arc<Session>) -> thread::JoinHandle<()> {
    let (host, session) = (h.h.clone(), session.clone());
    thread::spawn(move || {
        let deadline = Instant::now() + WAIT;
        while !session.stopping() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(2));
        }
        if session.stopping() {
            thread::sleep(Duration::from_millis(100));
            let removed = host
                .sessions
                .lock()
                .unwrap()
                .active
                .remove(&session.launch.id);
            drop(removed);
        }
    })
}

fn forget_stream(h: &Harness, session: Arc<Session>) {
    let removed =
        h.h.sessions
            .lock()
            .unwrap()
            .active
            .remove(&session.launch.id);
    drop((removed, session));
}

/// "Stream here too" promises a stream beside the owner, and a Moonlight stream ends a WebRTC one:
/// so another device's WebRTC stream refuses the join and keeps running, while a Moonlight stream
/// has the join beside it. The owner's own resume moves its stream between transports as ever.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_join_never_ends_the_owners_webrtc_stream_and_runs_beside_its_moonlight_one() -> Result<()>
{
    let h = Harness::new(10_000, 2).await?;
    let owner = "0".to_owned();
    h.h.current_app.lock().unwrap().as_mut().unwrap().owner = owner.clone();

    let webrtc = running_over(&h, 0, Transport::WebRtc).await?;
    let refused = h.launch_response(1, true).await?;
    assert!(refused.contains("status_code=\"409\""), "{refused}");
    assert!(
        refused.contains(butterpollo_stream::nvhttp::WEBRTC_HELD_BY_OTHER),
        "{refused}"
    );
    assert!(!webrtc.stopping(), "the owner's stream goes on");
    assert!(pending_of(&h, 1).is_none());
    assert_eq!(
        h.h.current_app.lock().unwrap().as_ref().unwrap().owner,
        owner
    );

    // The same join as stock Moonlight makes it: a `/launch` of the Running Game tile.
    let game = h.h.current_app.lock().unwrap().as_ref().unwrap().id;
    let tile = butterpollo_session::remote::running_game_id(game).to_string();
    let refused = h
        .launch_response_with(1, false, None, &[("appid", &tile)])
        .await?;
    assert!(refused.contains("status_code=\"409\""), "{refused}");
    assert!(
        refused.contains(butterpollo_stream::nvhttp::WEBRTC_HELD_BY_OTHER),
        "{refused}"
    );
    assert!(!webrtc.stopping(), "the owner's stream goes on");
    assert!(pending_of(&h, 1).is_none());

    // The same refusal for a launch of the owner's on its way.
    forget_stream(&h, webrtc);
    let mut launch = h.launch(0, true).await?;
    launch.transport = Transport::WebRtc;
    h.h.sessions
        .lock()
        .unwrap()
        .pending
        .insert(launch.id.clone(), launch.clone());
    let refused = h.launch_response(1, true).await?;
    assert!(refused.contains("status_code=\"409\""), "{refused}");
    assert!(pending_of(&h, 0).is_some(), "the owner's launch stays");
    let queued = h.h.sessions.lock().unwrap().pending.remove(&launch.id);
    drop((queued, launch));

    // A stream that is ending, as after a press, holds nothing.
    let webrtc = running_over(&h, 0, Transport::WebRtc).await?;
    webrtc.stop();
    let ended = ends_when_stopped(&h, &webrtc);
    let joined = h.launch(1, true).await?;
    let queued = h.h.sessions.lock().unwrap().pending.remove(&joined.id);
    drop((queued, joined));
    ended.join().unwrap();
    forget_stream(&h, webrtc);

    // A background stream is no plain one: it has a Nucleus seat, and a Moonlight stream never ends it.
    let mut background = h.launch(0, true).await?;
    let queued = h.h.sessions.lock().unwrap().pending.remove(&background.id);
    drop(queued);
    background.transport = Transport::WebRtc;
    background.background = Some(true);
    background.nucleus = Some(butterpollo_session::session::Seat {
        session: 1,
        slot: 0,
    });
    let session = Session::new(background, Default::default());
    h.h.sessions
        .lock()
        .unwrap()
        .active
        .insert(session.launch.id.clone(), session.clone());
    let joined = h.launch(1, true).await?;
    assert!(!session.stopping());
    let queued = h.h.sessions.lock().unwrap().pending.remove(&joined.id);
    drop((queued, joined));
    forget_stream(&h, session);

    // The owner moves its own stream, as before.
    let webrtc = running_over(&h, 0, Transport::WebRtc).await?;
    let ended = ends_when_stopped(&h, &webrtc);
    let moved = h.launch(0, true).await?;
    assert!(webrtc.stopping(), "its Moonlight stream takes over");
    let queued = h.h.sessions.lock().unwrap().pending.remove(&moved.id);
    drop((queued, moved));
    ended.join().unwrap();
    forget_stream(&h, webrtc);

    // Beside a Moonlight stream the join runs, and both go on.
    let first = h.launch(0, true).await?;
    let (stream, client) = h.play(&first).await?;
    let second = h.launch(1, true).await?;
    assert!(!stream.stopping());
    let (joined, joined_client) = h.play(&second).await?;
    h.counts(2, 0).await?;
    assert!(!stream.stopping());
    assert!(!joined.stopping());
    assert_eq!(
        h.h.current_app.lock().unwrap().as_ref().unwrap().owner,
        owner
    );
    // The join leaving takes nothing from the owner either.
    h.rtsp(&joined.launch, 3, "TEARDOWN", "").await?;
    until("the join ended", WAIT * 3, || {
        joined_client.reason.load(Ordering::Acquire) != 0
    })
    .await;
    assert!(!stream.stopping());
    drop((joined, joined_client));
    h.end(&stream, client).await?;
    h.finish().await
}

/// A resume is of the running game, whatever it names: a prompt left open while the game changed
/// names the old one. Another device's stale name is refused, since the user did not choose this
/// game; the owner's resume, and one that names nothing as stock Moonlight does, go to the game.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resume_naming_another_game_is_refused_for_a_non_owner_and_gets_the_game_for_its_owner()
-> Result<()> {
    let h = Harness::new(10_000, 2).await?;
    let mut running = App::desktop();
    running.extra.insert("uuid".into(), "game-y".into());
    let mut stale = running.clone();
    stale.name = "Game X".into();
    stale.extra.insert("uuid".into(), "game-x".into());
    let (stale_id, running_id) = (stale.id(), running.id());
    assert_ne!(stale_id, running_id);
    // Earlier in the list: the query's id and uuid both name it.
    *h.h.apps.write().unwrap() = vec![stale, running.clone()];
    *h.h.current_app.lock().unwrap() = Some(
        butterpollo_stream::process::RunningApp::with_environment(&running, Default::default())?,
    );
    h.h.current_app.lock().unwrap().as_mut().unwrap().owner = "0".into();
    let resume = |client: usize, uuid: &'static str| {
        let h = &h;
        async move {
            let extra = [("appuuid", uuid)];
            let extra: &[(&str, &str)] = if uuid.is_empty() { &[] } else { &extra };
            h.launch_response_with(client, true, None, extra).await
        }
    };
    let joins_the_game = |reply: String, client: usize| {
        assert!(reply.contains("status_code=\"200\""), "{reply}");
        let launch = pending_of(&h, client).expect("a launch");
        assert_eq!(launch.app_id, running_id);
        let queued = h.h.sessions.lock().unwrap().pending.remove(&launch.id);
        drop((queued, launch));
    };

    let refused = resume(1, "game-x").await?;
    assert!(refused.contains("status_code=\"409\""), "{refused}");
    assert!(
        refused.contains(butterpollo_stream::nvhttp::RESUME_GAME_CHANGED),
        "{refused}"
    );
    assert!(pending_of(&h, 1).is_none());
    // The owner's name for its own game does not decide which game it is.
    joins_the_game(resume(0, "game-x").await?, 0);
    // Nothing named, the game's own name, and a Resume tile's.
    joins_the_game(resume(1, "").await?, 1);
    joins_the_game(resume(1, "game-y").await?, 1);
    joins_the_game(resume(1, "9a1c5a25-58fe-40e0-b9aa-7d3f00000001").await?, 1);
    // A game listed with a new id since it started is still found, and is the same game.
    h.h.current_app.lock().unwrap().as_mut().unwrap().id = 7777;
    h.h.apps.write().unwrap()[1].aliases.push(7777);
    joins_the_game(resume(1, "game-y").await?, 1);

    *h.h.apps.write().unwrap() = vec![running.clone()];
    h.h.current_app.lock().unwrap().as_mut().unwrap().id = running_id;
    h.finish().await
}

/// A game closed while a resume prepares its display is not started by that resume.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_game_closed_while_a_resume_prepares_stays_closed() -> Result<()> {
    let h = Harness::new(10_000, 2).await?;
    h.h.current_app.lock().unwrap().as_mut().unwrap().owner = "0".into();
    let host = h.h.clone();
    *h.fixture.during_display_preparation.lock().unwrap() = Some(Box::new(move || {
        *host.current_app.lock().unwrap() = None;
    }));
    let refused = h.launch_response(1, true).await?;
    assert!(refused.contains("status_code=\"503\""), "{refused}");
    assert!(
        refused.contains(butterpollo_stream::nvhttp::NOTHING_TO_RESUME),
        "{refused}"
    );
    assert!(h.h.current_app.lock().unwrap().is_none(), "nothing started");
    assert_eq!(h.fixture.apps_prepared.load(Ordering::Acquire), 0);
    assert!(pending_of(&h, 1).is_none());
    h.finish().await
}
