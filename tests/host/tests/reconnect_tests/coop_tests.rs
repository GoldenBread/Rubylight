//! Co-op session launch, seat allocation, input routing, and expiry tests.
use super::*;

/// Two devices launching the co-op entry share one Nucleus session, each in
/// its own slot. Neither replaces, starts or keeps the host's app: Nucleus
/// started the game. The pending launches carry the seats their streams use.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn co_op_launches_share_one_session_beside_the_running_app() -> Result<()> {
    let (h, pc, id) = coop_harness().await?;
    let generation =
        h.h.current_app
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .generation
            .clone();
    let first = launch_app(&h, 0, id).await?;
    assert!(first.contains("status_code=\"200\""), "{first}");
    assert_eq!(xml(&first, "gamesession"), "1");
    let second = launch_app(&h, 1, id).await?;
    assert!(second.contains("status_code=\"200\""), "{second}");
    h.counts(0, 2).await?;
    let (a, b) = (pending_of(&h, 0).unwrap(), pending_of(&h, 1).unwrap());
    let session = a.nucleus.unwrap().session;
    assert_eq!(
        a.nucleus,
        Some(butterpollo_session::session::Seat { session, slot: 0 })
    );
    assert_eq!(
        b.nucleus,
        Some(butterpollo_session::session::Seat { session, slot: 1 })
    );
    assert_eq!((a.app_id, b.app_id), (id, id));
    assert_eq!(a.background, Some(false));
    assert_eq!(pc.started.lock().unwrap().len(), 1);
    // The first window was parked at the stream's even size.
    assert_eq!(pc.last().sizes.lock().unwrap().first(), Some(&(1280, 720)));
    assert_eq!(h.fixture.apps_prepared.load(Ordering::Acquire), 0);
    assert_eq!(h.fixture.resources_retained.load(Ordering::Acquire), 0);
    assert_eq!(
        h.h.current_app.lock().unwrap().as_ref().unwrap().generation,
        generation
    );
    assert_eq!(h.fixture.displays.load(Ordering::Acquire), 2);
    let listed = h.h.nucleus.sessions();
    assert_eq!(listed[0].held, [true, true]);
    assert_eq!(listed[0].app_id, id);
    // The launches' last copies give their slots back.
    for launch in [a, b] {
        let canceled = h.h.sessions.lock().unwrap().pending.remove(&launch.id);
        drop(canceled);
    }
    assert_eq!(h.h.nucleus.sessions()[0].held, [false, false]);
    h.finish().await
}

/// A co-op launch Nucleus refuses answers with the refusal's status and text
/// and leaves nothing pending: a device without Join co-op (409), a start
/// that fails (503) and a game whose handler went away (404).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refused_co_op_launches_answer_their_status_and_leave_nothing_pending() -> Result<()> {
    let (h, pc, id) = coop_harness().await?;
    *pc.start_error.lock().unwrap() = Some("The co-op helper did not start".into());
    let failed = launch_app(&h, 0, id).await?;
    assert!(failed.contains("status_code=\"503\""), "{failed}");
    assert!(
        failed.contains("The co-op helper did not start"),
        "{failed}"
    );
    assert_eq!(xml(&failed, "gamesession"), "0");
    *pc.start_error.lock().unwrap() = None;
    assert!(launch_app(&h, 1, id).await?.contains("status_code=\"200\""));
    let seated = pending_of(&h, 1).unwrap();
    // Device 0 may not join the running session.
    let refused = launch_app(&h, 0, id).await?;
    assert!(refused.contains("status_code=\"409\""), "{refused}");
    assert!(refused.contains("Can join co-op games"), "{refused}");
    assert!(pending_of(&h, 0).is_none());
    // The handler is gone by the time the next device launches.
    h.h.paired.write().unwrap().clients[0]
        .extra
        .insert("coop_join".into(), true.into());
    pc.catalog.lock().unwrap().handlers.clear();
    let missing = launch_app(&h, 0, id).await?;
    assert!(missing.contains("status_code=\"404\""), "{missing}");
    assert!(missing.contains("portal2.exe"), "{missing}");
    assert!(pending_of(&h, 0).is_none());
    h.counts(0, 1).await?;
    assert_eq!(h.fixture.displays.load(Ordering::Acquire), 1);
    assert_eq!(h.h.nucleus.sessions()[0].held, [true, false]);
    let canceled = h.h.sessions.lock().unwrap().pending.remove(&seated.id);
    drop(canceled);
    // The copy holds the launch's display lease and its slot.
    drop(seated);
    h.finish().await
}

/// A co-op launch that fails after Nucleus seated it gives the slot back: the
/// next launch of that device is not held off by a slot nobody streams.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_co_op_launch_that_fails_later_gives_its_slot_back() -> Result<()> {
    let (h, pc, id) = coop_harness().await?;
    h.fixture
        .disconnect_during_preparation
        .store(true, Ordering::Release);
    let failed = launch_app(&h, 0, id).await?;
    assert!(failed.contains("status_code=\"503\""), "{failed}");
    assert!(failed.contains("disconnected or changing"), "{failed}");
    assert!(pending_of(&h, 0).is_none());
    let listed = h.h.nucleus.sessions();
    assert_eq!(listed[0].held, [false, false]);
    assert!(listed[0].session.players()[0].away);
    h.empty().await?;
    h.fixture
        .disconnect_during_preparation
        .store(false, Ordering::Release);
    h.fixture
        .session_disconnected
        .store(false, Ordering::Release);
    // The device gets its own slot back in the same session.
    assert!(launch_app(&h, 0, id).await?.contains("status_code=\"200\""));
    let launch = pending_of(&h, 0).unwrap();
    assert_eq!(
        launch.nucleus,
        Some(butterpollo_session::session::Seat {
            session: listed[0].session.id,
            slot: 0
        })
    );
    assert_eq!(pc.started.lock().unwrap().len(), 1);
    let canceled = h.h.sessions.lock().unwrap().pending.remove(&launch.id);
    drop(canceled);
    drop(launch);
    h.finish().await
}

/// A WebRTC launch of the co-op entry takes a seat as a Moonlight launch
/// does. Nucleus's refusals become start-session errors with their text:
/// busy or refused is SessionActive, anything else a failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn webrtc_co_op_launches_take_seats_and_answer_refusals() -> Result<()> {
    use butterpollo_stream::nvhttp::{WebRtcLaunch, WebRtcLaunchError};
    let (h, pc, id) = coop_harness().await?;
    let webrtc = |client: usize| {
        let request = WebRtcLaunch {
            client: h.h.paired.read().unwrap().clients[client].clone(),
            peer: std::net::Ipv4Addr::LOCALHOST.into(),
            this_pc: false,
            app_id: id,
            width: 1280,
            height: 720,
            fps: 60,
            hdr: false,
        };
        let host = h.h.clone();
        tokio::task::spawn_blocking(move || {
            butterpollo_stream::nvhttp::launch_webrtc(&host, request)
        })
    };
    *pc.start_error.lock().unwrap() = Some("The co-op helper did not start".into());
    match webrtc(0).await?.err() {
        Some(WebRtcLaunchError::Failed(message)) => {
            assert!(message.contains("did not start"), "{message}")
        }
        other => panic!("{other:?}"),
    }
    *pc.start_error.lock().unwrap() = None;
    assert!(launch_app(&h, 1, id).await?.contains("status_code=\"200\""));
    let seat = pending_of(&h, 1).unwrap().nucleus.unwrap();
    match webrtc(0).await?.err() {
        Some(WebRtcLaunchError::SessionActive(message)) => {
            assert!(message.contains("Can join co-op games"), "{message}")
        }
        other => panic!("{other:?}"),
    }
    h.h.paired.write().unwrap().clients[0]
        .extra
        .insert("coop_join".into(), true.into());
    let Ok(launch) = webrtc(0).await? else {
        panic!("the co-op WebRTC launch failed");
    };
    assert_eq!(
        launch.transport,
        butterpollo_session::session::Transport::WebRtc
    );
    assert_eq!(
        launch.nucleus,
        Some(butterpollo_session::session::Seat {
            session: seat.session,
            slot: 1
        })
    );
    assert_eq!(pc.started.lock().unwrap().len(), 1);
    let canceled = h.h.sessions.lock().unwrap().pending.remove(&launch.id);
    drop(canceled);
    drop(launch);
    // A co-op player's WebRTC launch leaves the other player's Moonlight launch alone.
    let other = pending_of(&h, 1).expect("the Moonlight launch stays").id;
    assert_eq!(h.h.nucleus.sessions()[0].held, [true, false]);
    let canceled = h.h.sessions.lock().unwrap().pending.remove(&other);
    drop(canceled);
    h.finish().await
}

/// A co-op launch never takes the host's session from its owner, and a co-op
/// player's stock /cancel or native End session ends only its own launch: the
/// other player's launch and the host's app stay.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_co_op_player_neither_owns_the_session_nor_cancels_the_others() -> Result<()> {
    let (h, _pc, id) = coop_harness().await?;
    let before = h.h.session_snapshot();
    let generation =
        h.h.current_app
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .generation
            .clone();
    assert!(launch_app(&h, 0, id).await?.contains("status_code=\"200\""));
    assert!(launch_app(&h, 1, id).await?.contains("status_code=\"200\""));
    let after = h.h.session_snapshot();
    assert_eq!((after.state, after.owner), (before.state, before.owner));
    let response = h.clients[1]
        .get(format!("https://127.0.0.1:{}/cancel", h.ports.https))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    assert_eq!(xml(&response, "cancel"), "1", "{response}");
    assert!(pending_of(&h, 1).is_none());
    assert!(pending_of(&h, 0).is_some());
    assert_eq!(h.h.nucleus.sessions()[0].held, [true, false]);
    assert_eq!(
        h.h.current_app.lock().unwrap().as_ref().unwrap().generation,
        generation
    );
    // Nightjar's End session from the player that is left ends its own launch
    // too, though it owns no host session; the host's app stays.
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
    assert!(pending_of(&h, 0).is_none());
    assert_eq!(h.h.nucleus.sessions()[0].held, [false, false]);
    assert_eq!(
        h.h.current_app.lock().unwrap().as_ref().unwrap().generation,
        generation
    );
    h.finish().await
}

/// A co-op launch replaced by the device's next launch gives its slot back:
/// the player shows as away and keeps the slot for `coop::GRACE`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_replaced_co_op_launch_gives_its_slot_back() -> Result<()> {
    let (h, _pc, id) = coop_harness().await?;
    assert!(launch_app(&h, 0, id).await?.contains("status_code=\"200\""));
    let listed = h.h.nucleus.sessions();
    assert_eq!(listed[0].held, [true, false]);
    assert!(!listed[0].session.players()[0].away);
    let other = h.launch(0, false).await?;
    assert!(other.nucleus.is_none());
    let listed = h.h.nucleus.sessions();
    assert_eq!(listed[0].held, [false, false]);
    assert!(listed[0].session.players()[0].away);
    let canceled = h.h.sessions.lock().unwrap().pending.remove(&other.id);
    drop(canceled);
    drop(other);
    h.finish().await
}

/// Moonlight's multi-controller packet: controller `id` of the `active` ones.
fn pad_packet(id: u16, active: u16, buttons: u16) -> Vec<u8> {
    let mut b = vec![0u8; 34];
    b[..4].copy_from_slice(&30u32.to_be_bytes());
    b[4..8].copy_from_slice(&12u32.to_le_bytes());
    b[10..12].copy_from_slice(&id.to_le_bytes());
    b[12..14].copy_from_slice(&active.to_le_bytes());
    b[16..18].copy_from_slice(&buttons.to_le_bytes());
    b
}
/// A key press of A.
fn key_packet() -> Vec<u8> {
    let mut b = vec![0u8; 14];
    b[..4].copy_from_slice(&10u32.to_be_bytes());
    b[4..8].copy_from_slice(&3u32.to_le_bytes());
    b[9..11].copy_from_slice(&0x8041u16.to_le_bytes());
    b
}

/// Two players' streams drive their own slots' pads and never the desktop:
/// a keyboard is dropped and said once. When the game crashes both streams
/// end with the reason, the session is stopped and nothing stays active.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn co_op_streams_drive_their_slots_and_end_with_the_session() -> Result<()> {
    let (h, pc, id) = coop_harness().await?;
    assert!(launch_app(&h, 0, id).await?.contains("status_code=\"200\""));
    assert!(launch_app(&h, 1, id).await?.contains("status_code=\"200\""));
    let (a, b) = (pending_of(&h, 0).unwrap(), pending_of(&h, 1).unwrap());
    pc.last().show();
    let (sa, ca) = h.play(&a).await?;
    let (sb, cb) = h.play(&b).await?;
    h.counts(2, 0).await?;
    ca.send_encrypted(&a.key, 0, 0x0206, &pad_packet(0, 1, 0x1000));
    ca.send_encrypted(&a.key, 1, 0x0206, &key_packet());
    ca.send_encrypted(&a.key, 2, 0x0206, &key_packet());
    cb.send_encrypted(&b.key, 0, 0x0206, &pad_packet(0, 1, 0x2000));
    until("both players' pads moved", WAIT, || {
        pc.last().inputs.lock().unwrap().len() == 2
    })
    .await;
    let mut inputs = pc.last().inputs.lock().unwrap().clone();
    inputs.sort_by_key(|(slot, _)| *slot);
    assert!(matches!(
        inputs[..],
        [
            (
                0,
                butterpollo_input_policy::input::Input::Controller {
                    buttons: 0x1000,
                    ..
                }
            ),
            (
                1,
                butterpollo_input_policy::input::Input::Controller {
                    buttons: 0x2000,
                    ..
                }
            )
        ]
    ));
    let codes = |s: &Session| -> Vec<String> {
        s.launch
            .warnings
            .snapshot()
            .into_iter()
            .map(|w| w.code)
            .filter(|code| code.starts_with("coop_"))
            .collect()
    };
    until("the keyboard was said to be dropped", WAIT, || {
        codes(&sa) == ["coop_keyboard_mouse"]
    })
    .await;
    assert!(codes(&sb).is_empty());
    pc.last()
        .session
        .lock()
        .unwrap()
        .end(butterpollo_session::nucleus::coop::End::Crashed(
            "portal2.exe exited".into(),
        ));
    until("both streams ended", WAIT, || {
        sa.stopping() && sb.stopping()
    })
    .await;
    for s in [&sa, &sb] {
        let ended = s
            .launch
            .warnings
            .snapshot()
            .into_iter()
            .find(|w| w.code == "coop_ended")
            .unwrap();
        assert!(
            ended.message.contains("portal2.exe exited"),
            "{}",
            ended.message
        );
    }
    drop((ca, cb, a, b, sa, sb));
    h.empty().await?;
    // The streams' last copies of their launches gave the slots back.
    until("the session stopped", WAIT, || pc.last().stopped()).await;
    h.finish().await
}

/// The console's End and every player gone (`End::Empty`, which Nucleus
/// Co-op's session reaches after `coop::GRACE`) close each player's stream
/// quietly with the reason, never as a failure, and stop Nucleus Co-op.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn co_op_streams_close_quietly_when_the_session_is_ended_or_empty() -> Result<()> {
    use butterpollo_session::nucleus::coop::End;
    let (h, pc, id) = coop_harness().await?;
    let closed_with = |s: &Session, text: &str| {
        let ended = s
            .launch
            .warnings
            .snapshot()
            .into_iter()
            .find(|w| w.code == "coop_ended")
            .unwrap();
        assert!(ended.message.contains(text), "{}", ended.message);
        assert_eq!(s.termination_reason(), CLOSED);
    };
    assert!(launch_app(&h, 0, id).await?.contains("status_code=\"200\""));
    assert!(launch_app(&h, 1, id).await?.contains("status_code=\"200\""));
    let (a, b) = (pending_of(&h, 0).unwrap(), pending_of(&h, 1).unwrap());
    let session = a.nucleus.unwrap().session;
    pc.last().show();
    let (sa, ca) = h.play(&a).await?;
    let (sb, cb) = h.play(&b).await?;
    h.counts(2, 0).await?;
    assert!(h.h.nucleus.end(session));
    until("both streams ended", WAIT, || {
        sa.stopping() && sb.stopping()
    })
    .await;
    closed_with(&sa, "The Portal 2 co-op session was ended on the PC");
    closed_with(&sb, "The Portal 2 co-op session was ended on the PC");
    assert!(pc.last().stopped());
    drop((ca, cb, a, b, sa, sb));
    h.empty().await?;
    // The next launch starts a new session; everybody leaves it.
    assert!(launch_app(&h, 0, id).await?.contains("status_code=\"200\""));
    let a = pending_of(&h, 0).unwrap();
    assert_ne!(a.nucleus.unwrap().session, session);
    assert_eq!(pc.started.lock().unwrap().len(), 2);
    pc.last().show();
    let (sa, ca) = h.play(&a).await?;
    pc.last().session.lock().unwrap().end(End::Empty);
    until("the stream ended", WAIT, || sa.stopping()).await;
    closed_with(&sa, "every player left");
    drop((ca, a, sa));
    h.empty().await?;
    until("the session stopped", WAIT, || pc.last().stopped()).await;
    h.finish().await
}

/// A co-op launch whose device never connects gives its slot back when it
/// expires, once the expired launch is dropped outside the sessions lock.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_expired_co_op_launch_gives_its_slot_back() -> Result<()> {
    let (h, _pc, id) = coop_harness().await?;
    assert!(launch_app(&h, 0, id).await?.contains("status_code=\"200\""));
    let expired = {
        let mut sessions = h.h.sessions.lock().unwrap();
        let long_ago = Instant::now()
            .checked_sub(butterpollo_session::session::LAUNCH_WAIT + Duration::from_secs(1))
            .unwrap();
        for launch in sessions.pending.values_mut() {
            launch.created = long_ago;
        }
        sessions.expire()
    };
    assert_eq!(expired.len(), 1);
    // The expired launch still holds the slot until it is dropped.
    assert_eq!(h.h.nucleus.sessions()[0].held, [true, false]);
    drop(expired);
    let listed = h.h.nucleus.sessions();
    assert_eq!(listed[0].held, [false, false]);
    assert!(listed[0].session.players()[0].away);
    h.finish().await
}
