//! Unit tests for server/app menu commands, non-blocking foreground checks, input queuing, and cursor scale lookup.

use super::super::*;

#[test]
fn server_commands_require_permission_app_opt_in_and_a_valid_index_and_are_throttled() {
    use crate::state::test_support::Fixture;
    use serde_json::json;
    let f = Fixture::new();
    let mut client = f.client(1 << 20);
    f.host.config.write().unwrap().values.insert("server_cmd".into(), json!([
            {"name":"First","cmd":"echo first"}, {"name":"Second","cmd":"echo second","elevated":true}
        ]).to_string());
    let run = |run: Option<crate::process::MenuRun>| run.map(|run| (run.cmd, run.elevated));
    let mut at = None;
    assert_eq!(server_command(&f.host, &client, &[1], false, &mut at), None);
    assert!(at.is_none());
    assert_eq!(
        run(server_command(&f.host, &client, &[1], true, &mut at)),
        Some(("echo second".into(), true))
    );
    assert!(at.is_some());
    assert_eq!(server_command(&f.host, &client, &[0], true, &mut at), None);
    at = Some(Instant::now() - Duration::from_secs(2));
    assert_eq!(
        run(server_command(&f.host, &client, &[0], true, &mut at)),
        Some(("echo first".into(), false))
    );
    for payload in [vec![], vec![0, 1], vec![2]] {
        at = None;
        assert_eq!(
            server_command(&f.host, &client, &payload, true, &mut at),
            None
        );
    }
    for (permission, enabled) in [(0, true), (1 << 20, false)] {
        client.perm = permission;
        client.enabled = enabled;
        at = None;
        assert_eq!(server_command(&f.host, &client, &[0], true, &mut at), None);
        assert!(at.is_none());
    }
    client.perm = 1 << 20;
    client.enabled = true;
    let app = serde_json::from_value(json!({"name":"No commands","allow-client-commands":false}))
        .unwrap();
    *f.host.current_app.lock().unwrap() =
        Some(crate::process::RunningApp::with_environment(&app, Default::default()).unwrap());
    assert_eq!(server_command(&f.host, &client, &[0], true, &mut at), None);
    assert!(at.is_none());
}
#[test]
fn app_menu_commands_follow_the_host_commands_and_their_undo_waits_for_the_app() {
    use crate::state::test_support::Fixture;
    use serde_json::json;
    let f = Fixture::new();
    let client = f.client(1 << 20);
    f.host.config.write().unwrap().values.insert(
        "server_cmd".into(),
        json!([{"name":"Lock","cmd":"lock"}]).to_string(),
    );
    let app = serde_json::from_value(json!({"name":"Game","menu-cmd":[
        {"name":"Pet","cmd":"pet $(SUNSHINE_CLIENT_NAME)","undo":"rem pet closed","elevated":false},
        {"name":"Broken"}
    ]}))
    .unwrap();
    let environment = [("SUNSHINE_CLIENT_NAME".to_owned(), "Phone".to_owned())]
        .into_iter()
        .collect();
    *f.host.current_app.lock().unwrap() =
        Some(crate::process::RunningApp::with_environment(&app, environment).unwrap());
    let send = |index: u8| {
        let mut at = None;
        server_command(&f.host, &client, &[index], true, &mut at)
    };
    assert_eq!(send(0).map(|run| run.cmd), Some("lock".into()));
    let pet = send(1).unwrap();
    assert_eq!((pet.cmd.as_str(), pet.elevated), ("pet Phone", false));
    assert_eq!(pet.environment["SUNSHINE_CLIENT_NAME"], "Phone");
    // An entry without a command keeps its index and runs nothing.
    assert!(send(2).is_none());
    assert!(send(3).is_none());
    // An app that hides the host-wide commands lists only its own.
    let hidden = serde_json::from_value(json!({"name":"Hidden","exclude-global-menu-cmd":true,
        "menu-cmd":[{"name":"Pet","cmd":"pet"}]}))
    .unwrap();
    *f.host.current_app.lock().unwrap() =
        Some(crate::process::RunningApp::with_environment(&hidden, Default::default()).unwrap());
    assert_eq!(send(0).map(|run| run.cmd), Some("pet".into()));
    assert!(send(1).is_none());
    // An app with device commands off lists none, the host-wide ones included, and runs none.
    let off = serde_json::from_value(json!({"name":"Off","allow-client-commands":false,
        "menu-cmd":[{"name":"Pet","cmd":"pet"}]}))
    .unwrap();
    *f.host.current_app.lock().unwrap() =
        Some(crate::process::RunningApp::with_environment(&off, Default::default()).unwrap());
    let config = f.host.config.read().unwrap().clone();
    assert!(crate::process::pc_command_names(&f.host, &config).is_empty());
    assert!(send(0).is_none());
}
#[test]
fn the_stream_thread_skips_its_game_look_while_another_thread_holds_the_app() {
    use crate::state::test_support::Fixture;
    let f = Fixture::new();
    *f.host.current_app.lock().unwrap() = Some(
        crate::process::RunningApp::with_environment(
            &butterpollo_session::state::App::desktop(),
            Default::default(),
        )
        .unwrap(),
    );
    // Housekeeping holds the app for 400 ms, as on a PC busy loading a game.
    let (held, holding) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel::<()>();
    std::thread::scope(|scope| {
        let host = &f.host;
        scope.spawn(move || {
            let _current = host.current_app.lock().unwrap();
            held.send(()).unwrap();
            let _ = released.recv_timeout(Duration::from_millis(400));
        });
        holding.recv().unwrap();
        // A blocking look would wait the whole 400 ms and then report true.
        assert!(!observe_foreground_game(
            &f.host, "client", "game.exe", 4242
        ));
        release.send(()).unwrap();
    });
    // Free again, the look goes through.
    assert!(observe_foreground_game(&f.host, "client", "game.exe", 4242));
}
#[test]
fn a_program_another_program_started_is_checked_off_the_stream_thread() {
    use crate::state::test_support::Fixture;
    let f = Fixture::new();
    let mut app = crate::process::RunningApp::with_environment(
        &butterpollo_session::state::App::desktop(),
        Default::default(),
    )
    .unwrap();
    app.mark_launching();
    *f.host.current_app.lock().unwrap() = Some(app);
    // A "game" started after the launch by a program other than the
    // desktop (this test), as a store client starts one.
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    let program = format!(r"{root}\System32\PING.EXE");
    /// Ends the stand-in game however the test ends.
    struct Game(std::process::Child);
    impl Drop for Game {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let game = Game(
        std::process::Command::new(&program)
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let game_up = || {
        f.host
            .current_app
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .launch_progress()
            .map(|(up, _)| up)
    };
    // Each look hands a check off and returns, as the stream's once-a-second
    // look does: the process list is read and the result applied on the
    // check's own thread.
    let deadline = Instant::now() + Duration::from_secs(10);
    while game_up() != Some(true) {
        assert!(Instant::now() < deadline, "the check never found the game");
        assert!(observe_foreground_game(
            &f.host,
            "client",
            &program,
            game.0.id()
        ));
        thread::sleep(Duration::from_millis(5));
    }
}
#[test]
fn input_skips_controller_cleanup_while_another_thread_holds_the_app() {
    use crate::state::test_support::Fixture;
    let f = Fixture::new();
    let due = Instant::now() - Duration::from_secs(2);
    let mut last = due;
    let (held, holding) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel::<()>();
    std::thread::scope(|scope| {
        let host = &f.host;
        scope.spawn(move || {
            let _current = host.current_app.lock().unwrap();
            held.send(()).unwrap();
            let _ = released.recv_timeout(Duration::from_secs(5));
        });
        holding.recv().unwrap();
        expire_controllers(&f.host, &mut last);
        // Skipped, not waited for: still due.
        assert_eq!(last, due);
        release.send(()).unwrap();
    });
    expire_controllers(&f.host, &mut last);
    assert!(last > due);
}
#[test]
fn refused_steam_attributes_are_warned_about_once_per_device() {
    let mut packet = [0u8; 84];
    packet[4..8].copy_from_slice(&0x5500_0008u32.to_le_bytes());
    let refused = input::decode(&packet[..80]).unwrap_err();
    let other = input::decode(&[0; 8]).unwrap_err();
    let mut warned = false;
    assert_eq!(first_steam_attributes_refusal(&mut warned, &other), None);
    assert!(!warned);
    assert_eq!(
        first_steam_attributes_refusal(&mut warned, &refused),
        Some(input::SteamAttributesError::Length { len: 80 })
    );
    assert_eq!(first_steam_attributes_refusal(&mut warned, &refused), None);
    let mut other_device = false;
    assert!(first_steam_attributes_refusal(&mut other_device, &refused).is_some());
}
#[test]
fn a_full_input_queue_is_refused_not_grown() {
    let key = |down| input::Input::Keyboard {
        key: 0x41,
        modifiers: 0,
        flags: 0,
        down,
    };
    let mut inputs = vec![];
    for i in 0..INPUT_QUEUE {
        assert!(queue_input(&mut inputs, key(i % 2 == 0)));
    }
    assert!(!queue_input(&mut inputs, key(true)));
    assert_eq!(inputs.len(), INPUT_QUEUE);
    // A move that merges into the last queued one still fits.
    inputs.pop();
    assert!(queue_input(
        &mut inputs,
        input::Input::Relative { x: 1, y: 1 }
    ));
    assert!(queue_input(
        &mut inputs,
        input::Input::Relative { x: 2, y: 3 }
    ));
    assert_eq!(inputs.len(), INPUT_QUEUE);
    assert!(matches!(
        inputs.last(),
        Some(input::Input::Relative { x: 3, y: 4 })
    ));
    assert!(INPUT_OVERFLOW.contains("overflow"));
}
#[test]
fn input_waiting_on_a_refused_controller_acquire_is_not_replayed_later() {
    let arrival = input::Input::Arrival {
        id: 1,
        kind: 2,
        capabilities: 0,
        buttons: 0,
    };
    let mut inputs = vec![
        input::Input::Keyboard {
            key: 0x41,
            modifiers: 0,
            flags: 0,
            down: true,
        },
        arrival.clone(),
        input::Input::MouseButton {
            button: 1,
            down: true,
        },
        input::Input::Relative { x: 5, y: 5 },
    ];
    drop_uninjectable_input(&mut inputs);
    assert_eq!(inputs, vec![arrival]);
}
#[test]
fn a_refused_controller_acquire_keeps_the_state_the_client_sends_once() {
    let pad = |id, active, buttons| input::Input::Controller {
        id,
        active,
        buttons,
        left_trigger: 0,
        right_trigger: 0,
        sticks: [0; 4],
    };
    let steam = input::Input::SteamAttributes(Default::default());
    let mut inputs = vec![
        pad(0, 0b11, 1),
        input::Input::Haptics(false),
        pad(1, 0b11, 2),
        steam.clone(),
        input::Input::Keyboard {
            key: 0x41,
            modifiers: 0,
            flags: 0,
            down: true,
        },
        // The client unplugged pad 1: only pad 0's latest state carries that.
        pad(0, 0b01, 0),
    ];
    drop_uninjectable_input(&mut inputs);
    assert_eq!(
        inputs,
        vec![
            input::Input::Haptics(false),
            pad(1, 0b11, 2),
            steam,
            pad(0, 0b01, 0),
        ]
    );
}
#[test]
fn drop_uninjectable_input_deduplicates_setup_events_and_caps_total() {
    let arrival = |id, kind| input::Input::Arrival {
        id,
        kind,
        capabilities: 0,
        buttons: 0,
    };
    let mut inputs = vec![
        arrival(0, 1),
        input::Input::Haptics(false),
        arrival(0, 2),
        input::Input::Haptics(true),
    ];
    for id in 1..100u8 {
        inputs.push(arrival(id, 1));
    }
    drop_uninjectable_input(&mut inputs);
    assert_eq!(inputs.len(), 64);
    assert_eq!(inputs.last(), Some(&arrival(99, 1)));
    // Only the latest Haptics and latest Arrival per controller survive when under cap.
    let mut small = vec![
        arrival(0, 1),
        input::Input::Haptics(false),
        arrival(0, 2),
        input::Input::Haptics(true),
    ];
    drop_uninjectable_input(&mut small);
    assert_eq!(small, vec![arrival(0, 2), input::Input::Haptics(true)]);
}
#[test]
fn keyboard_and_mouse_go_on_while_the_controllers_wait_and_theirs_is_held() {
    let pad = |id, active, buttons| input::Input::Controller {
        id,
        active,
        buttons,
        left_trigger: 0,
        right_trigger: 0,
        sticks: [0; 4],
    };
    let key = input::Input::Keyboard {
        key: 0x41,
        modifiers: 0,
        flags: 0,
        down: true,
    };
    let click = input::Input::MouseButton {
        button: 1,
        down: true,
    };
    let mut held = Vec::new();
    let mut inputs = vec![
        pad(0, 0b1, 1),
        key.clone(),
        input::Input::Haptics(false),
        click.clone(),
    ];
    hold_pad_input(&mut inputs, &mut held);
    assert_eq!(inputs, [key.clone(), click.clone()]);
    // A later pass: only the pad's latest state waits with what the client sends once.
    let mut later = vec![pad(0, 0b1, 2), input::Input::Relative { x: 5, y: 5 }];
    hold_pad_input(&mut later, &mut held);
    assert_eq!(later, [input::Input::Relative { x: 5, y: 5 }]);
    assert_eq!(held, [input::Input::Haptics(false), pad(0, 0b1, 2)]);
    // Nothing of the controllers' in a pass leaves it as it was.
    let mut plain = vec![key.clone()];
    hold_pad_input(&mut plain, &mut held);
    assert_eq!(plain, [key]);
    assert_eq!(held.len(), 2);
}
#[test]
fn an_injector_without_controllers_retries_on_its_interval_and_replays_what_waited() {
    use std::time::{Duration, Instant};
    let start = Instant::now();
    let after = |ms| start + Duration::from_millis(ms);
    let mut wait = PadsWait::default();
    let moved = input::Input::Relative { x: 1, y: 1 };
    let mut inputs = vec![input::Input::Haptics(false), moved.clone()];
    wait.hold(&mut inputs);
    assert_eq!(
        inputs.len(),
        2,
        "nothing waits while the controllers are there"
    );
    // GameStream's control loop and the WebRTC worker alike.
    wait.failed(start);
    wait.hold(&mut inputs);
    assert_eq!(inputs, [moved]);
    let mut tries = 0;
    let none = wait.retry(after(50), || {
        tries += 1;
        Ok(())
    });
    assert!(none.is_none());
    assert_eq!(tries, 0, "not before the interval");
    let refused = wait.retry(after(100), || {
        tries += 1;
        anyhow::bail!("the previous stream's controllers are closing")
    });
    assert!(refused.is_none());
    assert_eq!(wait.retry_at(), Some(after(200)));
    let held = wait.retry(after(200), || {
        tries += 1;
        Ok(())
    });
    assert_eq!(held, Some(vec![input::Input::Haptics(false)]));
    assert_eq!(tries, 2);
    assert_eq!(wait.retry_at(), None);
}
#[test]
fn an_arrived_pad_is_asked_for_the_sensors_it_has() {
    assert_eq!(
        motion_requests(3, 0x30),
        [
            (MOTION_EVENT_REQUEST, vec![3, 0, 200, 0, 1]),
            (MOTION_EVENT_REQUEST, vec![3, 0, 200, 0, 2])
        ]
    );
    assert_eq!(
        motion_requests(1, 0x20),
        [(MOTION_EVENT_REQUEST, vec![1, 0, 200, 0, 2])]
    );
    assert!(motion_requests(1, 0x0f).is_empty());
}

#[test]
fn a_peer_config_is_made_once_per_settings_change_and_calibration_period() {
    let mut cache = PeerConfig::default();
    let made = std::cell::Cell::new(0);
    let load = || {
        made.set(made.get() + 1);
        Ok(Config::default())
    };
    let start = Instant::now();
    let first = cache.get(1, start, None, load).unwrap();
    let again = cache
        .get(1, start + Duration::from_secs(600), None, load)
        .unwrap();
    assert!(Arc::ptr_eq(&first, &again));
    assert_eq!(made.get(), 1);
    // A saved setting or app.
    cache.get(2, start, None, load).unwrap();
    assert_eq!(made.get(), 2);
    // An HDR calibration is read again once its period passed.
    let refresh = Some(Duration::from_secs(30));
    cache
        .get(2, start + Duration::from_secs(29), refresh, load)
        .unwrap();
    assert_eq!(made.get(), 2);
    cache
        .get(2, start + Duration::from_secs(30), refresh, load)
        .unwrap();
    assert_eq!(made.get(), 3);
    // A failure is not kept: the next packet tries again.
    assert!(
        cache
            .get(3, start, None, || anyhow::bail!("unreadable"))
            .is_err()
    );
    cache.get(3, start, None, load).unwrap();
    assert_eq!(made.get(), 4);
}

#[test]
fn a_cursor_scale_is_looked_up_off_the_control_thread() {
    static RELEASE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    fn slow(output: &str) -> Result<u32> {
        while !RELEASE.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(1));
        }
        match output {
            "D1" => Ok(150),
            _ => anyhow::bail!("no such display"),
        }
    }
    let (woken, wakes) = std::sync::mpsc::channel();
    let mut lookup = ScaleLookup::default();
    let wake = |woken: &std::sync::mpsc::Sender<()>| {
        let woken = woken.clone();
        move || woken.send(()).unwrap()
    };
    // Never blocks: the first ask starts the lookup, later ones see it running.
    assert!(lookup.poll("D1", slow, wake(&woken)).is_none());
    assert!(lookup.poll("D1", slow, wake(&woken)).is_none());
    RELEASE.store(true, Ordering::Release);
    wakes
        .recv_timeout(Duration::from_secs(5))
        .expect("the lookup woke the loop");
    // The woken pass has the result: it was stored before the wake.
    let scale = lookup.poll("D1", slow, wake(&woken));
    assert_eq!(scale.unwrap().unwrap(), 150);
    // Another display is looked up afresh, and its failure reaches the caller.
    assert!(lookup.poll("D2", slow, wake(&woken)).is_none());
    wakes.recv_timeout(Duration::from_secs(5)).unwrap();
    let failed = lookup.poll("D2", slow, wake(&woken));
    assert!(failed.unwrap().is_err());
}
