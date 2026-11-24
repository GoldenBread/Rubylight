//! `POST /session/continue-here` failed-press recovery, late stream arrival, and returning owner tests.
use super::*;

#[tokio::test]
async fn the_window_after_a_failed_press_is_a_minute_whatever_the_display_grace_says() {
    for (revert, timeout) in [
        (None, None),
        (Some("keep_while_paused"), None),
        (Some("keep_while_paused"), Some("0")),
    ] {
        let s = Setup::new();
        let _phone = s.streaming();
        {
            let mut config = s.host().config.write().unwrap();
            if let Some(revert) = revert {
                config
                    .values
                    .insert("dd_disconnect_behavior".into(), revert.into());
            }
            if let Some(timeout) = timeout {
                config.values.insert(
                    "dd_paused_virtual_display_timeout_secs".into(),
                    timeout.into(),
                );
            }
        }
        let (_, tv_connection) = device(&s, "tv", "TV", ALL);
        press(&s, &tv_connection).await.session();
        s.host().moves.expire_claim();
        s.host().settle_move();
        let left = s.host().moves.back_left().expect("the window");
        assert!(
            left > std::time::Duration::from_secs(50) && left <= std::time::Duration::from_secs(60),
            "{left:?} with {revert:?} and {timeout:?}"
        );
    }
}

#[tokio::test]
async fn after_a_failed_press_the_session_reads_as_the_first_devices_own_again() {
    let s = Setup::new();
    let phone = s.streaming();
    let (_, tv_connection) = device(&s, "tv", "TV", ALL);
    press(&s, &tv_connection).await.session();
    s.host()
        .sessions
        .lock()
        .unwrap()
        .active
        .remove(&phone.launch.id);
    drop(phone);
    // What the phone's open prompt watches: still moved.
    let moved = s.get("/session").await.session();
    assert!(moved.moved_to.is_some() && !moved.owns_game);
    s.host().moves.expire_claim();
    s.host().settle_move();
    let back = s.get("/session").await.session();
    assert!(back.moved_to.is_none());
    assert!(
        back.owns_game,
        "the prompt offers to resume on exactly this"
    );
    assert!(back.playing_on.is_none());
    assert_eq!(back.owner, rubylight_native_api::session::Owner::You);

    // A device without a kept display has no session between streams, and the game is still its.
    let s = Setup::new();
    s.run_game();
    let phone = s.stream_as(s.client.clone());
    let (_, tv_connection) = device(&s, "tv", "TV", ALL);
    press(&s, &tv_connection).await.session();
    s.host()
        .sessions
        .lock()
        .unwrap()
        .active
        .remove(&phone.launch.id);
    drop(phone);
    s.host().moves.expire_claim();
    s.host().settle_move();
    let back = s.get("/session").await.session();
    assert_eq!(s.state(), SessionState::Idle);
    assert!(back.moved_to.is_none() && back.owns_game);
}

#[tokio::test]
async fn a_note_that_runs_out_or_a_game_that_ended_does_not_read_as_coming_back() {
    let s = Setup::new();
    let phone = s.streaming();
    let (tv, tv_connection) = device(&s, "tv", "TV", ALL);
    press(&s, &tv_connection).await.session();
    let _tv = s.stream_as(tv);
    s.host()
        .sessions
        .lock()
        .unwrap()
        .active
        .remove(&phone.launch.id);
    s.host().settle_move();
    s.host().moves.expire_notes();
    let read = s.get("/session").await.session();
    assert!(read.moved_to.is_none() && !read.owns_game);
    assert_eq!(
        read.owner,
        rubylight_native_api::session::Owner::OtherDevice
    );

    // The game ended after the press failed.
    let s = Setup::new();
    let phone = s.streaming();
    let (_, tv_connection) = device(&s, "tv", "TV", ALL);
    press(&s, &tv_connection).await.session();
    s.host()
        .sessions
        .lock()
        .unwrap()
        .active
        .remove(&phone.launch.id);
    drop(phone);
    s.host().moves.expire_claim();
    s.host().settle_move();
    s.host()
        .stop_app(butterpollo_stream::state::AppStop::CloseGame(
            &s.client.uuid,
        ));
    s.host().reap_paused_display();
    let read = s.get("/session").await.session();
    assert!(read.moved_to.is_none() && !read.owns_game);
}

#[tokio::test]
async fn a_late_stream_leaves_a_first_device_that_came_back_its_game_and_tells_one_that_did_not() {
    let expired = |s: &Setup, phone: Arc<Session>| {
        s.host()
            .sessions
            .lock()
            .unwrap()
            .active
            .remove(&phone.launch.id);
        drop(phone);
        s.host().moves.expire_claim();
        s.host().settle_move();
    };
    // It came back, and its stream is down for a moment just as the late one comes.
    let s = Setup::new();
    let phone = s.streaming();
    let (tv, tv_connection) = device(&s, "tv", "TV", ALL);
    press(&s, &tv_connection).await.session();
    expired(&s, phone);
    let back = s.stream_as(s.client.clone());
    s.host().settle_move();
    s.host()
        .sessions
        .lock()
        .unwrap()
        .active
        .remove(&back.launch.id);
    drop(back);
    let stream = s.stream_as(tv);
    s.host().settle_move();
    assert_eq!(game_owner(&s), s.client.uuid);
    let read = s.get("/session").await.session();
    assert!(read.moved_to.is_none() && read.owns_game);
    drop(stream);

    // Its Resume is on its way: what a launch does before anything is running.
    let s = Setup::new();
    let phone = s.streaming();
    let (tv, tv_connection) = device(&s, "tv", "TV", ALL);
    press(&s, &tv_connection).await.session();
    expired(&s, phone);
    s.host().moves.forget(&s.client.uuid);
    let stream = s.stream_as(tv);
    s.host().settle_move();
    assert_eq!(game_owner(&s), s.client.uuid);
    drop(stream);

    // It stayed away: the game goes, and it reads where to.
    let s = Setup::new();
    let phone = s.streaming();
    let (tv, tv_connection) = device(&s, "tv", "TV", ALL);
    press(&s, &tv_connection).await.session();
    expired(&s, phone);
    assert!(s.get("/session").await.session().moved_to.is_none());
    let stream = s.stream_as(tv.clone());
    s.host().settle_move();
    assert_eq!(game_owner(&s), tv.uuid);
    let read = s.get("/session").await.session();
    assert_eq!(read.moved_to.expect("told").device, "TV");
    assert!(!read.owns_game);
    // That note ends with the game, as any other.
    *s.host().current_app.lock().unwrap() = None;
    assert!(s.get("/session").await.session().moved_to.is_none());
    drop(stream);
}

#[tokio::test]
async fn a_device_a_failed_press_displaced_without_owning_the_game_hears_that_nothing_moved() {
    // The phone owns the game, the laptop streams beside it, the TV presses and never comes.
    let failed = || async {
        let s = Setup::new();
        let phone = s.streaming();
        let (laptop, laptop_connection) = device(&s, "laptop", "Laptop", ALL);
        let beside = s.stream_as(laptop);
        let (tv, tv_connection) = device(&s, "tv", "TV", ALL);
        press(&s, &tv_connection).await.session();
        let read = session_of(&s, &laptop_connection).await;
        assert!(
            read.moved_to.is_some() && !read.press_failed,
            "while the press is live"
        );
        for stream in [phone, beside] {
            s.host()
                .sessions
                .lock()
                .unwrap()
                .active
                .remove(&stream.launch.id);
        }
        s.host().moves.expire_claim();
        s.host().settle_move();
        (s, laptop_connection, tv)
    };
    let (s, laptop_connection, _) = failed().await;
    let read = session_of(&s, &laptop_connection).await;
    assert!(read.moved_to.is_none() && !read.owns_game && read.press_failed);
    let first = s.get("/session").await.session();
    assert!(
        first.owns_game && !first.press_failed,
        "the owner hears it came back"
    );
    // It answered: the prompt that asked is long gone.
    s.host().moves.forget("laptop");
    assert!(!session_of(&s, &laptop_connection).await.press_failed);

    // The stream came after all, so the move happened.
    let (s, laptop_connection, tv) = failed().await;
    let stream = s.stream_as(tv);
    s.host().settle_move();
    assert!(!session_of(&s, &laptop_connection).await.press_failed);
    drop(stream);

    // The game ended.
    let (s, laptop_connection, _) = failed().await;
    *s.host().current_app.lock().unwrap() = None;
    assert!(!session_of(&s, &laptop_connection).await.press_failed);
}

fn game_display(s: &Setup) -> Option<String> {
    let current = s.host().current_app.lock().unwrap();
    current.as_ref().unwrap().display.clone()
}
fn set_game_display(s: &Setup, display: &str) {
    let mut current = s.host().current_app.lock().unwrap();
    current.as_mut().unwrap().display = Some(display.into());
}
/// A press from the phone's display whose time ran out: the TV's launch recorded its
/// display at once, as a launch does, and its stream has not come.
async fn press_that_ran_out(s: &Setup, tv_connection: &Connection, phone: Arc<Session>) {
    set_game_display(s, "phone-display");
    press(s, tv_connection).await.session();
    set_game_display(s, "tv-display");
    s.host()
        .sessions
        .lock()
        .unwrap()
        .active
        .remove(&phone.launch.id);
    drop(phone);
    s.host().moves.expire_claim();
    s.host().settle_move();
}

#[tokio::test]
async fn a_press_that_fails_puts_the_game_back_on_the_display_it_is_on() {
    let s = Setup::new();
    let phone = s.streaming();
    let (_, tv_connection) = device(&s, "tv", "TV", ALL);
    press_that_ran_out(&s, &tv_connection, phone).await;
    assert_eq!(game_owner(&s), s.client.uuid);
    // Not the TV's, which goes with its launch: the next device joins the game's.
    assert_eq!(game_display(&s).as_deref(), Some("phone-display"));
}

#[tokio::test]
async fn a_late_stream_moves_the_games_display_with_it_unless_it_joined() {
    let late = |identity: &'static str, joined: bool| async move {
        let s = Setup::new();
        let phone = s.streaming();
        let (tv, tv_connection) = device(&s, "tv", "TV", ALL);
        press_that_ran_out(&s, &tv_connection, phone).await;
        let config = s.host().config.read().unwrap().clone();
        let display = butterpollo_stream::display_session::test_support::retained_on(
            s.host(),
            &config,
            identity,
            joined,
        );
        s.host()
            .app_display
            .lock()
            .unwrap()
            .insert(tv.uuid.clone(), (display, None));
        let stream = s.stream_as(tv.clone());
        s.host().settle_move();
        assert_eq!(game_owner(&s), tv.uuid);
        let display = game_display(&s);
        drop(stream);
        display
    };
    assert_eq!(
        late("tv-display", false).await.as_deref(),
        Some("tv-display")
    );
    // A stream that joined moves nothing, whatever display it holds by now.
    assert_eq!(
        late("joined-display", true).await.as_deref(),
        Some("phone-display")
    );
}

#[tokio::test]
async fn a_stream_that_arrives_after_the_press_expired_takes_the_game() {
    let expired = |s: &Setup, phone: Arc<Session>| {
        s.host()
            .sessions
            .lock()
            .unwrap()
            .active
            .remove(&phone.launch.id);
        drop(phone);
        s.host().moves.expire_claim();
        s.host().settle_move();
    };
    let s = Setup::new();
    let phone = s.streaming();
    let (tv, tv_connection) = device(&s, "tv", "TV", ALL);
    press(&s, &tv_connection).await.session();
    expired(&s, phone);
    assert_eq!(game_owner(&s), s.client.uuid);
    let stream = s.stream_as(tv.clone());
    s.host().settle_move();
    assert_eq!(
        game_owner(&s),
        tv.uuid,
        "its own recovery must find the game"
    );
    drop(stream);

    // The first device came back first: it keeps its game, the late one streams beside it.
    let s = Setup::new();
    let phone = s.streaming();
    let (tv, tv_connection) = device(&s, "tv", "TV", ALL);
    press(&s, &tv_connection).await.session();
    expired(&s, phone);
    let back = s.stream_as(s.client.clone());
    let stream = s.stream_as(tv);
    s.host().settle_move();
    assert_eq!(game_owner(&s), s.client.uuid);
    drop((back, stream));

    // The game went on to a third device in the meantime: it is not taken from there.
    let s = Setup::new();
    let phone = s.streaming();
    let (tv, tv_connection) = device(&s, "tv", "TV", ALL);
    press(&s, &tv_connection).await.session();
    expired(&s, phone);
    s.host().current_app.lock().unwrap().as_mut().unwrap().owner = "laptop".into();
    let stream = s.stream_as(tv);
    s.host().settle_move();
    assert_eq!(game_owner(&s), "laptop");
    drop(stream);
}

#[tokio::test]
async fn the_displaced_device_cannot_close_the_game_while_the_press_is_live() {
    let s = Setup::new();
    s.host_control(true);
    let phone = s.streaming();
    let (_, tv_connection) = device(&s, "tv", "TV", ALL);
    press(&s, &tv_connection).await.session();
    s.host()
        .sessions
        .lock()
        .unwrap()
        .active
        .remove(&phone.launch.id);
    drop(phone);
    s.post("/session/close-game")
        .await
        .assert_error(409, "session_busy");
    assert!(s.game_running());
    assert!(
        terminate_refused(s.host(), &s.client.uuid),
        "nor by Moonlight's Quit"
    );
    // Once the press is over it is the phone's game again.
    s.host().moves.expire_claim();
    assert_eq!(
        s.post("/session/close-game").await.status,
        StatusCode::OK,
        "the phone owns it again"
    );
    assert!(!s.game_running());
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // The press only tries the lock.
async fn a_launch_in_flight_refuses_the_press() {
    let s = Setup::new();
    let phone = s.streaming();
    let (_, tv_connection) = device(&s, "tv", "TV", ALL);
    let held = s.host().launch_transition.lock().unwrap();
    press(&s, &tv_connection)
        .await
        .assert_error(409, "session_busy");
    drop(held);
    assert!(!phone.stopping());
}

#[tokio::test]
async fn a_press_never_followed_by_a_stream_gives_the_game_back() {
    let s = Setup::new();
    let phone = s.streaming();
    let (_, tv_connection) = device(&s, "tv", "TV", ALL);
    press(&s, &tv_connection).await.session();
    s.host()
        .sessions
        .lock()
        .unwrap()
        .active
        .remove(&phone.launch.id);
    drop(phone);
    // Still in time: the display and the claim stay.
    s.host().reap_paused_display();
    assert!(s.host().claim_live());
    s.host().moves.expire_claim();
    s.host().reap_paused_display();
    assert!(!s.host().claim_live());
    assert!(s.game_running());
    assert_eq!(game_owner(&s), s.client.uuid);
    assert_eq!(s.displays(), 1, "the phone can come back to its display");
    assert_eq!(s.state(), SessionState::Suspended);
}

#[tokio::test]
async fn an_ordinary_webrtc_stream_moves_but_one_guests_watch_is_refused() {
    let s = Setup::new();
    s.run_game();
    let mut launch = s.f.launch(s.client.clone(), Role::Stream);
    launch.transport = Transport::WebRtc;
    let console = running(&s, launch);
    let (_, tv_connection) = device(&s, "tv", "TV", ALL);
    let guests = &butterpollo_webrtc::WebRtcRuntime::for_host(s.host()).guests;
    let seated = guests.seat_for_test();
    press(&s, &tv_connection)
        .await
        .assert_error(409, "host_in_use");
    assert!(!console.stopping());
    assert_eq!(game_owner(&s), s.client.uuid);
    assert!(s.host().moves.note_for(&s.client.uuid).is_none());
    guests.left(seated);
    press(&s, &tv_connection).await.session();
    assert!(console.stopping());
}

/// A kept display that would otherwise stay for hours; the phone's stream ended with the press.
async fn pressed_with_kept_display() -> (Setup, Client) {
    let s = Setup::new();
    let phone = s.streaming();
    s.host()
        .config
        .write()
        .unwrap()
        .values
        .insert("dd_disconnect_behavior".into(), "keep_while_paused".into());
    let (tv, tv_connection) = device(&s, "tv", "TV", ALL);
    press(&s, &tv_connection).await.session();
    s.host()
        .sessions
        .lock()
        .unwrap()
        .active
        .remove(&phone.launch.id);
    drop(phone);
    (s, tv)
}

#[tokio::test]
async fn an_owner_that_returns_during_a_live_press_keeps_its_display_and_its_game() {
    // The press's stream comes on time: the display the phone came back to stays.
    let (s, tv) = pressed_with_kept_display().await;
    s.host().moves.forget(&s.client.uuid);
    assert!(!s.host().moves.displays_held().contains(&s.client.uuid));
    let stream = s.stream_as(tv);
    s.host().settle_move();
    s.host().reap_paused_display();
    assert_eq!(s.displays(), 1, "on time");
    drop(stream);

    // It comes late: the same, and the game stays the phone's.
    let (s, tv) = pressed_with_kept_display().await;
    s.host().moves.forget(&s.client.uuid);
    s.host().moves.expire_claim();
    s.host().settle_move();
    assert_eq!(game_owner(&s), s.client.uuid);
    let stream = s.stream_as(tv);
    s.host().settle_move();
    s.host().reap_paused_display();
    assert_eq!(s.displays(), 1, "late");
    assert_eq!(game_owner(&s), s.client.uuid);
    assert!(s.get("/session").await.session().moved_to.is_none());
    drop(stream);
}

#[tokio::test]
async fn an_owner_whose_resume_was_refused_has_not_returned() {
    // After the press ended: the late stream takes the game, so its own recovery finds it.
    let (s, tv) = pressed_with_kept_display().await;
    s.host().moves.expire_claim();
    s.host().settle_move();
    s.host().moves.forget(&s.client.uuid);
    drop(butterpollo_stream::continue_here::Attempt::new(
        s.host(),
        &s.client.uuid,
        true,
    ));
    let stream = s.stream_as(tv.clone());
    s.host().settle_move();
    assert_eq!(game_owner(&s), tv.uuid);
    assert_eq!(
        s.get("/session")
            .await
            .session()
            .moved_to
            .expect("told")
            .device,
        "TV"
    );
    drop(stream);

    // During the press: the same.
    let (s, tv) = pressed_with_kept_display().await;
    s.host().moves.forget(&s.client.uuid);
    drop(butterpollo_stream::continue_here::Attempt::new(
        s.host(),
        &s.client.uuid,
        true,
    ));
    s.host().moves.expire_claim();
    s.host().settle_move();
    let stream = s.stream_as(tv.clone());
    s.host().settle_move();
    assert_eq!(game_owner(&s), tv.uuid);
    drop(stream);

    // A launch that would not have been its stream says nothing, nor does one that was refused
    // beside a stream it still has.
    let (s, tv) = pressed_with_kept_display().await;
    s.host().moves.expire_claim();
    s.host().settle_move();
    s.host().moves.forget(&s.client.uuid);
    drop(butterpollo_stream::continue_here::Attempt::new(
        s.host(),
        &s.client.uuid,
        false,
    ));
    let stream = s.stream_as(tv);
    s.host().settle_move();
    assert_eq!(game_owner(&s), s.client.uuid, "no stream launch");
    drop(stream);
    let (s, tv) = pressed_with_kept_display().await;
    s.host().moves.expire_claim();
    s.host().settle_move();
    let back = s.stream_as(s.client.clone());
    s.host().moves.forget(&s.client.uuid);
    drop(butterpollo_stream::continue_here::Attempt::new(
        s.host(),
        &s.client.uuid,
        true,
    ));
    s.host()
        .sessions
        .lock()
        .unwrap()
        .active
        .remove(&back.launch.id);
    let stream = s.stream_as(tv);
    s.host().settle_move();
    assert_eq!(game_owner(&s), s.client.uuid, "it still streamed");
    drop((back, stream));
}

#[tokio::test]
async fn what_a_failed_press_says_to_a_device_beside_it_ends() {
    let failed = || async {
        let s = Setup::new();
        let phone = s.streaming();
        let (laptop, laptop_connection) = device(&s, "laptop", "Laptop", ALL);
        let beside = s.stream_as(laptop);
        let (_, tv_connection) = device(&s, "tv", "TV", ALL);
        press(&s, &tv_connection).await.session();
        for stream in [phone, beside] {
            s.host()
                .sessions
                .lock()
                .unwrap()
                .active
                .remove(&stream.launch.id);
        }
        s.host().moves.expire_claim();
        s.host().settle_move();
        assert!(session_of(&s, &laptop_connection).await.press_failed);
        (s, laptop_connection)
    };
    // After its two minutes, with nothing settling the press since.
    let (s, laptop_connection) = failed().await;
    s.host().moves.expire_late();
    assert!(!session_of(&s, &laptop_connection).await.press_failed);

    // Another game runs now, not no game.
    let (s, laptop_connection) = failed().await;
    s.host()
        .current_app
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .generation = "another game".into();
    assert!(!session_of(&s, &laptop_connection).await.press_failed);
}

#[tokio::test]
async fn an_owner_that_presses_fails_and_then_arrives_gets_no_note_to_itself() {
    let s = Setup::new();
    let phone = s.streaming();
    let (laptop, _) = device(&s, "laptop", "Laptop", ALL);
    let beside = s.stream_as(laptop);
    s.host()
        .sessions
        .lock()
        .unwrap()
        .active
        .remove(&phone.launch.id);
    drop(phone);
    press(&s, &s.f.connection(true)).await.session();
    assert!(s.host().claim_live());
    assert!(beside.stopping());
    s.host().moves.expire_claim();
    s.host().settle_move();
    assert!(!s.host().claim_live());
    let stream = s.stream_as(s.client.clone());
    s.host().settle_move();
    assert_eq!(game_owner(&s), s.client.uuid);
    assert!(s.get("/session").await.session().moved_to.is_none());
    drop((stream, beside));
}
