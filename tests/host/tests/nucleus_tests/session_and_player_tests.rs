//! Session contention, slot ownership, kick/end, gamepad input/rumble routing, and window timeout tests.
use super::setup_helper::{admit, app, background, client, setup};
use butterpollo_config::config::Config;
use butterpollo_input_policy::input::Input;
use butterpollo_session::nucleus::coop::{End, Phase, Refused};
use butterpollo_session::nucleus::handler::Missing;
use butterpollo_session::session::{Seat, Warnings};
use butterpollo_stream::nucleus::*;
use butterpollo_win_io::input::PadReport;
use std::time::Duration;

#[test]
fn another_game_is_busy_while_a_session_runs() {
    let (registry, pc, mut apps) = setup();
    let coop = registry.entries(&apps, "").remove(0);
    admit(&registry, &coop, "a", &client("A", false)).unwrap();
    {
        let mut catalog = pc.catalog.lock().unwrap();
        let mut handler = catalog.handlers[0].clone();
        handler.guid = "Racer".into();
        handler.executable = "racer.exe".into();
        catalog.handlers.push(handler);
        catalog
            .games
            .push(butterpollo_session::nucleus::handler::Game {
                guid: "Racer".into(),
                exe_path: r"C:\Games\racer.exe".into(),
            });
    }
    apps.push(app("Racer", r"C:\Games\racer.exe"));
    registry.use_pc(pc.clone());
    let racer = registry
        .entries(&apps, "")
        .into_iter()
        .find(|coop| coop.app.name == "Racer")
        .unwrap();
    let busy = admit(&registry, &racer, "b", &client("B", true)).unwrap_err();
    assert_eq!(busy.status, 409);
    assert!(busy.message.contains("one game at a time"), "{busy}");
}

#[test]
fn refusals_carry_their_status() {
    let (registry, pc, apps) = setup();
    let coop = registry.entries(&apps, "").remove(0);
    let a = client("A", false);
    // Removed from Nucleus since the library was read: 404 with the reason.
    pc.catalog.lock().unwrap().games.clear();
    let missing = admit(&registry, &coop, "a", &a).unwrap_err();
    assert_eq!(missing.status, 404);
    assert!(
        missing.message.contains("the game is not added"),
        "{missing}"
    );
    pc.catalog.lock().unwrap().handlers.clear();
    let missing = admit(&registry, &coop, "a", &a).unwrap_err();
    assert_eq!(
        missing,
        Refusal::missing(&Missing::NoHandler {
            executable: "portal2.exe".into()
        })
    );
    // Unavailable here: 503 with the reason, before Nucleus is looked for.
    *pc.unavailable.lock().unwrap() = Some("no service".into());
    *pc.installed.lock().unwrap() = None;
    assert_eq!(
        admit(&registry, &coop, "a", &a).unwrap_err(),
        Refusal::unavailable("no service")
    );
}

#[test]
fn a_failed_start_or_no_free_pad_is_unavailable() {
    let (registry, pc, apps) = setup();
    let coop = registry.entries(&apps, "").remove(0);
    *pc.free.lock().unwrap() = 0;
    let no_pad = admit(&registry, &coop, "a", &client("A", false)).unwrap_err();
    assert_eq!(no_pad.status, 503);
    assert!(no_pad.message.contains("XInput"), "{no_pad}");
    *pc.free.lock().unwrap() = 1;
    *pc.start_error.lock().unwrap() = Some("Nucleus Co-op is already running".into());
    let failed = admit(&registry, &coop, "a", &client("A", false)).unwrap_err();
    assert_eq!(failed.status, 503);
    assert!(failed.message.contains("already running"), "{failed}");
    assert!(registry.sessions().is_empty());
    // One free pad: a one-player session.
    *pc.start_error.lock().unwrap() = None;
    admit(&registry, &coop, "a", &client("A", false)).unwrap();
    assert_eq!(pc.last().session.lock().unwrap().slots.len(), 1);
}

#[test]
fn only_the_launch_holding_a_slot_leaves_it() {
    let (registry, pc, apps) = setup();
    let coop = registry.entries(&apps, "").remove(0);
    let a = client("A", false);
    let first = admit(&registry, &coop, "a1", &a).unwrap();
    // The same device launches again: the newer launch holds the slot.
    assert_eq!(admit(&registry, &coop, "a2", &a).unwrap(), first);
    registry.leave(first, "a1");
    let session = pc.last();
    let away = |session: &fake::Instance| session.session.lock().unwrap().players()[0].away;
    assert!(!away(&session));
    assert_eq!(registry.sessions()[0].held, [true, false]);
    registry.leave(first, "a2");
    assert!(away(&session));
    assert_eq!(registry.sessions()[0].held, [false, false]);
    // Leaving twice changes nothing.
    registry.leave(first, "a2");
    // Back within the grace: its own slot.
    assert_eq!(admit(&registry, &coop, "a3", &a).unwrap(), first);
    assert!(!away(&session));
}

#[test]
fn a_join_from_the_list_holds_the_slot_until_the_launch() {
    let (registry, pc, apps) = setup();
    let coop = registry.entries(&apps, "").remove(0);
    let a = client("A", false);
    admit(&registry, &coop, "a", &a).unwrap();
    assert_eq!(registry.join(9, &a), Err(JoinError::NoSession));
    assert_eq!(
        registry.join(1, &client("B", false)),
        Err(JoinError::Refused(Refused::NotPermitted))
    );
    let b = client("B", true);
    assert_eq!(registry.join(1, &b), Ok((1, coop.id())));
    let session = pc.last();
    assert!(session.session.lock().unwrap().players()[1].away);
    assert_eq!(
        admit(&registry, &coop, "b", &b).unwrap(),
        Seat {
            session: 1,
            slot: 1
        }
    );
    assert!(!session.session.lock().unwrap().players()[1].away);
    // The streaming owner asking again is not marked away.
    assert_eq!(registry.join(1, &a), Ok((0, coop.id())));
    assert!(!session.session.lock().unwrap().players()[0].away);
    // Join co-op taken away: the player cannot come back to its slot,
    // from the list or by launching the entry; the owner still can.
    let revoked = client("B", false);
    assert_eq!(
        registry.join(1, &revoked),
        Err(JoinError::Refused(Refused::NotPermitted))
    );
    assert_eq!(
        admit(&registry, &coop, "b2", &revoked).unwrap_err(),
        Refusal::busy(Refused::NotPermitted.to_string())
    );
    assert_eq!(registry.join(1, &client("A", false)), Ok((0, coop.id())));
}

#[test]
fn kick_and_end_reach_the_session() {
    let (registry, pc, apps) = setup();
    let coop = registry.entries(&apps, "").remove(0);
    let b = client("B", true);
    admit(&registry, &coop, "a", &client("A", false)).unwrap();
    admit(&registry, &coop, "b", &b).unwrap();
    assert_eq!(registry.kick(1, 1).as_deref(), Some("B"));
    assert_eq!(registry.kick(1, 1), None);
    assert_eq!(registry.kick(7, 0), None);
    assert_eq!(registry.sessions()[0].held, [true, false]);
    assert_eq!(
        admit(&registry, &coop, "b2", &b).unwrap_err(),
        Refusal::busy(
            Refused::Kicked {
                title: "Portal 2".into()
            }
            .to_string()
        )
    );
    assert!(!registry.end(7));
    assert!(registry.end(1));
    let session = pc.last();
    assert!(session.stopped());
    let listed = registry.sessions();
    assert_eq!(listed[0].session.phase, Phase::Ended(End::Ended));
    assert_eq!(
        registry.join(1, &b),
        Err(JoinError::Refused(Refused::Ended {
            title: "Portal 2".into()
        }))
    );
    // An ended session no longer holds Nucleus: the next launch starts anew.
    assert_eq!(
        admit(&registry, &coop, "c", &client("C", false)).unwrap(),
        Seat {
            session: 2,
            slot: 0
        }
    );
    assert_eq!(pc.started.lock().unwrap().len(), 2);
}

#[test]
fn a_session_that_ends_by_itself_is_stopped_once_seen() {
    let (registry, pc, apps) = setup();
    let coop = registry.entries(&apps, "").remove(0);
    let seat = admit(&registry, &coop, "a", &client("A", false)).unwrap();
    let session = pc.last();
    session
        .session
        .lock()
        .unwrap()
        .end(End::Crashed("gone".into()));
    assert!(!session.stopped());
    registry.leave(seat, "a");
    assert!(session.stopped());
    // Still listed for a while so the console can say why.
    assert_eq!(registry.sessions().len(), 1);
}

fn pad(id: u16, active: u16, buttons: u32) -> Input {
    Input::Controller {
        id,
        active,
        buttons,
        left_trigger: 0,
        right_trigger: 0,
        sticks: [0; 4],
    }
}
fn codes(warnings: &Warnings) -> Vec<String> {
    warnings.snapshot().into_iter().map(|w| w.code).collect()
}

#[test]
fn a_player_drives_its_slot_and_hears_of_each_dropped_kind_once() {
    let (registry, pc, apps) = setup();
    let coop = registry.entries(&apps, "").remove(0);
    admit(&registry, &coop, "a", &client("A", false)).unwrap();
    let seat = admit(&registry, &coop, "b", &client("B", true)).unwrap();
    pc.last().show();
    let mut player = Player {
        instance: registry.instance(seat.session).unwrap(),
        seat,
        device: "B".into(),
        rumble: true,
        told: Vec::new(),
    };
    let warnings = Warnings::default();
    player.input(pad(0, 1, 0x1000), &warnings);
    let key = Input::Keyboard {
        key: 0x41,
        modifiers: 0,
        flags: 0,
        down: true,
    };
    player.input(key.clone(), &warnings);
    player.input(key, &warnings);
    assert_eq!(codes(&warnings), ["coop_keyboard_mouse"]);
    player.input(pad(1, 0b11, 0x1000), &warnings);
    player.input(pad(1, 0b11, 0), &warnings);
    assert_eq!(
        codes(&warnings),
        ["coop_keyboard_mouse", "coop_second_controller"]
    );
    assert_eq!(player.told.len(), 2);
    // Only the first controller reached the pad, as slot 1's.
    let inputs = pc.last().inputs.lock().unwrap().clone();
    assert_eq!(inputs.len(), 1);
    assert_eq!(inputs[0].0, 1);
    assert!(matches!(
        inputs[0].1,
        Input::Controller {
            buttons: 0x1000,
            ..
        }
    ));
}

#[test]
fn rumble_reaches_a_player_only_where_its_stream_forwards_it() {
    let (registry, pc, apps) = setup();
    let coop = registry.entries(&apps, "").remove(0);
    let seat = admit(&registry, &coop, "a", &client("A", false)).unwrap();
    let mut player = Player {
        instance: registry.instance(seat.session).unwrap(),
        seat,
        device: "A".into(),
        rumble: false,
        told: Vec::new(),
    };
    assert!(!player.feedback_allowed(0x010b));
    assert!(player.feedback_allowed(0x5502));
    player.rumble = true;
    assert!(player.feedback_allowed(0x010b));
    drop(pc);
}

#[test]
fn each_slot_reads_its_own_feedback_whoever_drains_the_pads() {
    let (registry, pc, apps) = setup();
    let coop = registry.entries(&apps, "").remove(0);
    let a = admit(&registry, &coop, "a", &client("A", false)).unwrap();
    let b = admit(&registry, &coop, "b", &client("B", true)).unwrap();
    let rumble = |id, data: u8| PadReport::Feedback {
        id,
        kind: 0x010b,
        data: vec![data],
    };
    pc.last()
        .reports
        .lock()
        .unwrap()
        .extend([(0, rumble(0, 1)), (1, rumble(0, 2))]);
    let data = |reports: Vec<PadReport>| -> Vec<u8> {
        reports
            .into_iter()
            .filter_map(|r| match r {
                PadReport::Feedback { data, .. } => Some(data[0]),
                _ => None,
            })
            .collect()
    };
    assert_eq!(data(registry.feedback(a)), [1]);
    // Slot 1's report waited for its own stream.
    assert_eq!(data(registry.feedback(b)), [2]);
    assert!(registry.feedback(b).is_empty());
    // A stream that stopped reading keeps only the newest reports.
    pc.last()
        .reports
        .lock()
        .unwrap()
        .extend((0..FEEDBACK_KEPT as u8 + 4).map(|k| (1, rumble(0, k))));
    assert!(registry.feedback(a).is_empty());
    let kept = data(registry.feedback(b));
    assert_eq!(kept.len(), FEEDBACK_KEPT);
    assert_eq!(kept[0], 4);
}

#[test]
fn the_first_window_is_waited_for_and_its_absence_is_refused() {
    let (registry, pc, apps) = setup();
    let coop = registry.entries(&apps, "").remove(0);
    let seat = admit(&registry, &coop, "a", &client("A", false)).unwrap();
    let instance = registry.instance(seat.session).unwrap();
    let none = first_window(instance.as_ref(), 0, "d", Duration::ZERO).unwrap_err();
    assert!(
        format!("{none:#}").contains("showed no window for Player 1 within 0 s"),
        "{none:#}"
    );
    assert_eq!(none.downcast_ref::<Refusal>().unwrap().status, 503);
    let shown = std::thread::spawn({
        let session = pc.last();
        move || {
            std::thread::sleep(Duration::from_millis(50));
            session.show();
        }
    });
    let first = first_window(instance.as_ref(), 0, "d", Duration::from_secs(10)).unwrap();
    assert_eq!(first, "window:100@d");
    shown.join().unwrap();
    pc.last()
        .session
        .lock()
        .unwrap()
        .end(End::Crashed("gone".into()));
    let ended = first_window(instance.as_ref(), 0, "d", Duration::from_secs(10)).unwrap_err();
    assert!(
        format!("{ended:#}").contains("co-op session ended: "),
        "{ended:#}"
    );
}

/// A background stream whose game window never came back (NieR, Sekiro in
/// #56) ends when its device's return times out waiting for it, saying why:
/// it no longer refuses every other background launch until the game exits
/// by itself. One still starting (a slow first window) goes on, and so does
/// a co-op session for its other players.
#[test]
fn a_background_stream_without_its_window_ends_when_its_return_times_out() {
    let (registry, pc, apps) = setup();
    let owner = client("A", true);
    let config = Config::default();
    let seat = background(&registry, &apps[1], "a", &owner, &config).unwrap();
    let instance = registry.instance(seat.session).unwrap();
    let wait = Duration::from_secs(90);
    assert_eq!(
        end_without_window(&registry, seat, instance.as_ref(), wait),
        None
    );
    assert!(!pc.last().stopped());
    // It ran, and its window went away.
    pc.last().show();
    instance.session(|s| s.slots[0].window = None);
    registry.leave(seat, "a");
    let seat = background(&registry, &apps[1], "b", &owner, &config).unwrap();
    assert!(first_window(instance.as_ref(), 0, "d", Duration::ZERO).is_err());
    let text =
        end_without_window(&registry, seat, instance.as_ref(), Duration::from_secs(90)).unwrap();
    assert_eq!(
        text,
        "The Portal 2 background stream ended: the game stopped: Player 1's game window did not come back within 90 s, so Rubylight closed the game; start the stream again"
    );
    assert!(pc.last().stopped());
    assert!(instance.end().is_some());
    // Once is enough: the next look finds it ended and keeps its reason.
    assert_eq!(
        end_without_window(&registry, seat, instance.as_ref(), Duration::from_secs(90)),
        None
    );
    assert!(matches!(instance.end(), Some(End::Crashed(_))));
    // Another background launch starts at once.
    registry.leave(seat, "b");
    let other = background(&registry, &apps[1], "c", &client("B", true), &config).unwrap();
    assert_ne!(other.session, seat.session);
    registry.leave(other, "c");

    let (registry, pc, apps) = setup();
    let coop = registry.entries(&apps, "").remove(0);
    let seat = admit(&registry, &coop, "a", &client("A", false)).unwrap();
    let instance = registry.instance(seat.session).unwrap();
    assert_eq!(
        end_without_window(&registry, seat, instance.as_ref(), Duration::from_secs(90)),
        None
    );
    assert!(!pc.last().stopped());
    assert!(instance.end().is_none());
}

/// The registry knows the background stream a device stepped away from, so
/// its quit ends it after its stream already ended (#56); nobody else's.
#[test]
fn the_background_stream_a_device_stepped_away_from_is_known() {
    let (registry, pc, apps) = setup();
    let config = Config::default();
    let seat = background(&registry, &apps[1], "a", &client("A", true), &config).unwrap();
    assert_eq!(registry.background_of("A"), Some(seat.session));
    registry.leave(seat, "a");
    assert_eq!(registry.background_of("A"), Some(seat.session));
    assert_eq!(registry.background_of("B"), None);
    assert!(registry.end(seat.session));
    assert!(pc.last().stopped());
    assert_eq!(registry.background_of("A"), None);
    // A co-op seat is no background stream.
    let coop = registry.entries(&apps, "").remove(0);
    admit(&registry, &coop, "c", &client("A", true)).unwrap();
    assert_eq!(registry.background_of("A"), None);
}

#[test]
fn a_player_stream_leaves_the_desktop_input_and_clipboard_alone() {
    let mut config = Config::default();
    assert!(!player(&config));
    isolate(&mut config);
    assert!(player(&config));
    assert!(!config.boolean("keyboard", true));
    assert!(!config.boolean("mouse", true));
    assert_eq!(config.get("audio_capture_mode", "system"), "game");
    // Not a background stream: no isolated display is made for it.
    assert!(!butterpollo_session::background::enabled(&config));
    let mut owner = client("A", false);
    owner.perm = u32::MAX;
    assert!(!butterpollo_stream::stream::clipboard_sync::allowed(&config, &owner).any());
}

#[test]
fn the_stream_says_when_windows_cannot_capture_a_players_audio() {
    let warnings = Warnings::default();
    warn_audio(None, &warnings);
    warn_audio(Some(22631), &warnings);
    assert!(codes(&warnings).is_empty());
    warn_audio(Some(19045), &warnings);
    assert_eq!(codes(&warnings), ["coop_audio"]);
    assert!(warnings.snapshot()[0].message.contains("20348"));
}
