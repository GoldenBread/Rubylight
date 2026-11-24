//! Tests for full Nucleus Co-op session lifecycle, Steam environment, user-copy
//! launches, settings/layers restoration, pad routing, and per-player audio.
use super::*;

#[test]
fn a_session_starts_runs_and_stops_everything() {
    let folder = tempfile::tempdir().unwrap();
    let (screen, probe) = (screen(), Probe::default());
    label(&screen);
    let (running, recorder) = start_with(
        folder.path(),
        2,
        [false, true, false, false],
        &screen,
        &probe,
    );
    let running = running.unwrap();
    let error = running.capture_target(0, r"\\.\DISPLAY1").unwrap_err();
    assert_eq!(error.to_string(), "Player 1's game window is not open");
    // One pad per player, skipping the PC's own at index 1.
    assert_eq!(
        running.with_session(|s| s.slots.iter().map(|s| s.pad).collect::<Vec<_>>()),
        [0, 2]
    );
    assert_eq!(recorder.sent.lock().unwrap().len(), 2);
    assert_eq!(*probe.launched.lock().unwrap(), ["Game"]);
    // Found from no Steam app: Nucleus gets the user's environment alone.
    assert_eq!(*probe.environments.lock().unwrap(), [BTreeMap::new()]);
    let profile = std::fs::read_to_string(
        folder
            .path()
            .join("game profiles")
            .join("Game")
            .join("Profile[1].json"),
    )
    .unwrap();
    assert!(profile.contains(TITLE), "{profile}");
    assert!(folder.path().join("Settings.ini").is_file());
    // Nucleus loads the profile and starts both instances.
    let deadline = Instant::now() + Duration::from_secs(10);
    while screen.0.lock().unwrap().clicks.is_empty() {
        assert!(Instant::now() < deadline, "the profile was never clicked");
        std::thread::sleep(Duration::from_millis(10));
    }
    instance(&screen, 0);
    instance(&screen, 1);
    wait_for(&running, |p| *p == Phase::Running);
    assert_eq!(running.windows(), [Some(200), Some(210)]);
    // Each player's stream captures that player's window, which capture
    // reads back as the same window and display.
    let target = running.capture_target(1, r"\\.\DISPLAY2").unwrap();
    assert_eq!(target, r"window:210@\\.\DISPLAY2");
    assert_eq!(
        butterpollo_media_policy::app_window::WindowTarget::parse(&target).unwrap(),
        Some(butterpollo_media_policy::app_window::WindowTarget::new(
            210,
            r"\\.\DISPLAY2"
        ))
    );
    assert_eq!(
        running.capture_target(0, r"\\.\DISPLAY1").unwrap(),
        r"window:200@\\.\DISPLAY1"
    );
    let error = running.capture_target(2, r"\\.\DISPLAY1").unwrap_err();
    assert_eq!(error.to_string(), "the session has 2 players, not 3");
    // Player 2's game replaced its window: the target follows it.
    {
        let mut desktop = screen.0.lock().unwrap();
        desktop.windows.retain(|w| w.id != 210);
        desktop.windows.push(window(211, 21, ON_SCREEN));
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while running.capture_target(1, r"\\.\DISPLAY2").ok().as_deref()
        != Some(r"window:211@\\.\DISPLAY2")
    {
        assert!(Instant::now() < deadline, "{:?}", running.windows());
        std::thread::sleep(Duration::from_millis(10));
    }
    running.stop();
    assert_eq!(
        running.with_session(|s| s.phase.clone()),
        Phase::Ended(End::Ended)
    );
    assert_eq!(
        *probe.stopped.lock().unwrap(),
        Some(BTreeMap::from([(20, SINCE + 1), (21, SINCE + 2)]))
    );
}

#[test]
fn a_game_found_from_steam_starts_with_its_app_id() {
    let folder = tempfile::tempdir().unwrap();
    let (screen, probe) = (screen(), Probe::default());
    let (running, _) = start_steam(
        folder.path(),
        1,
        [false; 4],
        &screen,
        &probe,
        Some(242050),
        GAME,
    );
    running.unwrap().stop();
    let app = || "242050".to_string();
    assert_eq!(
        *probe.environments.lock().unwrap(),
        [BTreeMap::from([
            ("SteamAppId".to_string(), app()),
            ("SteamGameId".to_string(), app()),
        ])]
    );
    assert_eq!(steam_environment(None), BTreeMap::new());
}

#[test]
fn a_game_nucleus_saved_with_pads_by_button_press_gets_them_by_index() {
    // As Nucleus 2.4.2 saves the first game it loads without the file: it
    // then waits for a button press on Player 1's pad, and never starts the
    // game in a background stream (#56).
    let saved = r#"{"Game": {"Favorite": true, "UseApiIndex": false, "UseApiIndexForGuests": false}, "Other": {"UseApiIndex": false}}"#;
    let folder = tempfile::tempdir().unwrap();
    let profile = tempfile::tempdir().unwrap();
    let meta_info = profile.path().join("NucleusCoop").join("metaInfo.json");
    std::fs::create_dir_all(meta_info.parent().unwrap()).unwrap();
    std::fs::write(&meta_info, saved).unwrap();
    let probe = Probe::default();
    *probe.profile.lock().unwrap() = Some(profile.path().to_path_buf());
    // A screen each: the last session's Nucleus stays on a fake one.
    let start = || start_with(folder.path(), 1, [false; 4], &screen(), &probe).0;
    let running = start().unwrap();
    let games: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&meta_info).unwrap()).unwrap();
    assert_eq!(games["Game"]["UseApiIndex"], true);
    assert_eq!(games["Game"]["UseApiIndexForGuests"], true);
    assert_eq!(games["Game"]["Favorite"], true);
    // Another game is the user's.
    assert_eq!(games["Other"]["UseApiIndex"], false);
    running.stop();
    assert_eq!(
        std::fs::read(&meta_info).unwrap(),
        serde_json::to_vec_pretty(&games).unwrap()
    );

    // Without the file, Nucleus finds an empty one, so the game it adds
    // takes pads by index from Settings.ini.
    std::fs::remove_dir_all(meta_info.parent().unwrap()).unwrap();
    start().unwrap().stop();
    assert_eq!(std::fs::read(&meta_info).unwrap(), b"{}");

    // One Nucleus can't read fails the start and says which file.
    std::fs::write(&meta_info, "{").unwrap();
    let error = format!("{:#}", start().err().unwrap());
    assert!(
        error.contains("per-game settings") && error.contains("metaInfo.json"),
        "{error}"
    );
}

#[test]
fn per_game_settings_are_in_the_profile_folder_nucleus_reads() {
    // Nucleus' `Globals.UserEnvironmentRoot`: the account's profile folder.
    let mut seen = None;
    in_profile_of(None, &mut |profile| {
        seen = Some(profile.to_path_buf());
        Ok(())
    })
    .unwrap();
    let expected = PathBuf::from(std::env::var_os("USERPROFILE").unwrap());
    assert_eq!(seen, Some(expected.clone()));
    assert_eq!(
        files::meta_info(&expected),
        expected.join(r"NucleusCoop\metaInfo.json")
    );
}

#[test]
fn a_session_from_the_users_own_copy_writes_and_runs_there() {
    let host = tempfile::tempdir().unwrap();
    let copy = tempfile::tempdir().unwrap();
    // A favourites-only list in the copy, which would hide the game.
    let ini = copy.path().join("Settings.ini");
    std::fs::write(&ini, "[Dev]\r\nShowFavoriteOnly=True\r\n").unwrap();
    let favourites = || {
        butterpollo_session::nucleus::profile::ini_value(
            &std::fs::read(&ini).unwrap(),
            "Dev",
            "ShowFavoriteOnly",
        )
        .unwrap()
    };
    let (screen, probe) = (screen(), Probe::default());
    label(&screen);
    let recorder = Arc::new(Recorder::default());
    let handler = handler();
    let mut xinput = FakeXInput {
        pads: &recorder,
        taken: [false; 4],
        hidden: false,
        intruder: false,
    };
    let request = Start {
        id: 1,
        kind: kind(1),
        folder: host.path(),
        user_copy: Some(copy.path()),
        handler: &handler,
        exe_path: r"D:\Games\Game\Game.exe",
        steam_app: None,
        title: "Game",
        players: 1,
        owner: ("phone", "Phone"),
        size: (1280, 720),
        instances: None,
    };
    let running = start(
        &request,
        Box::new(Recorded(recorder.clone())),
        &mut xinput,
        Box::new(screen.clone()),
        Box::new(probe.layers.clone()),
        &mut (probe.clone(), screen.clone(), ()),
    )
    .unwrap();
    // Settings and profile in the copy, written as the user; Nucleus
    // started from it; the compatibility settings' file in the host's.
    assert_eq!(*probe.writes.lock().unwrap(), 1);
    assert!(copy.path().join("Settings.ini").is_file());
    assert!(copy.path().join("game profiles").join("Game").is_dir());
    assert!(!host.path().join("Settings.ini").exists());
    assert!(!host.path().join("game profiles").exists());
    assert_eq!(*probe.folders.lock().unwrap(), [copy.path().to_path_buf()]);
    let journal: Vec<_> = std::fs::read_dir(host.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert!(
        journal
            .iter()
            .any(|name| name.starts_with("Rubylight-layers-")),
        "{journal:?}"
    );
    // The copy's settings are the session's, written as the user with
    // no journal: nothing in the copy is put back as the host.
    assert_eq!(favourites(), b"False");
    // A start that times out quotes the crashlog of the copy Nucleus
    // runs from, not the host's.
    assert_eq!(running.crashlog, copy.path().join("crashlog.txt"));
    let journals =
        || [host.path(), copy.path()].map(|folder| folder.join("Rubylight-settings.json").exists());
    assert_eq!(journals(), [false, false]);
    running.stop();
    assert_eq!(favourites(), b"False");
    assert_eq!(journals(), [false, false]);
    assert_eq!(*probe.writes.lock().unwrap(), 1);
}

#[test]
fn the_games_compatibility_settings_are_back_after_a_session_or_a_failed_start() {
    use crate::nucleus::layers::tests::text;
    let folder = tempfile::tempdir().unwrap();
    let (screen, probe) = (screen(), Probe::default());
    let game = r"D:\Games\Game\Game.exe";
    let set = |value: &str| {
        probe
            .layers
            .values
            .lock()
            .unwrap()
            .insert(game.into(), text(value))
    };
    set("~ RUNASADMIN");
    let ini = folder.path().join("Settings.ini");
    std::fs::write(&ini, "[Dev]\r\nShowFavoriteOnly=True\r\n").unwrap();
    let favourites = || {
        let text = std::fs::read(&ini).unwrap();
        butterpollo_session::nucleus::profile::ini_value(&text, "Dev", "ShowFavoriteOnly").unwrap()
    };
    let (running, _) = start_with(folder.path(), 1, [false; 4], &screen, &probe);
    let running = running.unwrap();
    assert_eq!(favourites(), b"False");
    // Nucleus sets the instance's DPI mode as it starts it.
    set("~ HIGHDPIAWARE");
    running.stop();
    // The user's own Nucleus settings are back too.
    assert_eq!(favourites(), b"True");
    assert_eq!(
        *probe.layers.values.lock().unwrap(),
        BTreeMap::from([(game.to_string(), text("~ RUNASADMIN"))])
    );
    // Nucleus did not start: nothing left behind either.
    *probe.refused.lock().unwrap() = Some("access denied".into());
    let (running, _) = start_with(folder.path(), 1, [false; 4], &super::screen(), &probe);
    assert_eq!(running.err().unwrap().to_string(), "access denied");
    assert_eq!(
        *probe.layers.values.lock().unwrap(),
        BTreeMap::from([(game.to_string(), text("~ RUNASADMIN"))])
    );
    assert_eq!(favourites(), b"True");
    // No journal of the settings is left either.
    let mut files: Vec<_> = std::fs::read_dir(folder.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    files.sort();
    assert_eq!(files, ["Settings.ini", "game profiles"]);
}

#[test]
fn a_session_that_fails_on_its_own_stops_nucleus() {
    let folder = tempfile::tempdir().unwrap();
    let (screen, probe) = (screen(), Probe::default());
    let (running, _) = start_with(folder.path(), 1, [false; 4], &screen, &probe);
    let running = running.unwrap();
    *probe.exit.lock().unwrap() = Some(1);
    wait_for(&running, |p| matches!(p, Phase::Ended(_)));
    drop(running);
    assert!(probe.stopped.lock().unwrap().is_some());
}

#[test]
fn a_running_nucleus_or_too_few_controller_slots_refuse_the_start() {
    let folder = tempfile::tempdir().unwrap();
    let (screen, probe) = (screen(), Probe::default());
    screen
        .0
        .lock()
        .unwrap()
        .processes
        .push(process(42, 1, 1, "nucleuscoop.exe"));
    let error = start_with(folder.path(), 1, [false; 4], &screen, &probe)
        .0
        .err()
        .unwrap()
        .to_string();
    assert!(
        error.starts_with("Nucleus Co-op is already running on the PC (process 42)"),
        "{error}"
    );
    let screen = super::screen();
    let (result, recorder) =
        start_with(folder.path(), 2, [true, true, true, false], &screen, &probe);
    let error = result.err().unwrap().to_string();
    assert!(
        error.starts_with(
            "2 players need 2 free XInput controller slots and the PC's own controllers leave 1"
        ),
        "{error}"
    );
    // Nothing was plugged, written or started.
    assert!(recorder.sent.lock().unwrap().is_empty());
    assert!(!folder.path().join("game profiles").exists());
    assert!(probe.launched.lock().unwrap().is_empty());
}

fn state(id: u16, active: u16, buttons: u32) -> Input {
    Input::Controller {
        id,
        active,
        buttons,
        left_trigger: 0,
        right_trigger: 0,
        sticks: [0; 4],
    }
}

#[test]
fn each_device_reaches_its_own_slot_s_pad_only() {
    let folder = tempfile::tempdir().unwrap();
    let (screen, probe) = (screen(), Probe::default());
    let (running, recorder) = start_with(folder.path(), 2, [false; 4], &screen, &probe);
    let running = running.unwrap();
    assert_eq!(
        running.with_session(|s| s.join("laptop", "Laptop", true)),
        Ok(coop::Joined::Seated(1))
    );
    recorder.sent.lock().unwrap().clear();
    assert_eq!(running.input("laptop", state(0, 1, 0x1000)), Ok(()));
    assert_eq!(running.input("phone", state(0, 1, 0x10)), Ok(()));
    assert_eq!(
        *recorder.sent.lock().unwrap(),
        [state(1, 0b11, 0x1000), state(0, 0b11, 0x10)]
    );
    // Keyboards, second controllers, strangers and away players reach nothing.
    let key = Input::Keyboard {
        key: 0x41,
        modifiers: 0,
        flags: 0,
        down: true,
    };
    assert_eq!(running.input("phone", key), Err(Dropped::KeyboardMouse));
    assert_eq!(
        running.input("phone", state(1, 0b11, 0)),
        Err(Dropped::SecondController)
    );
    assert_eq!(running.input("tv", state(0, 1, 0)), Err(Dropped::Ignored));
    running.with_session(|s| s.leave("laptop", Instant::now()));
    assert_eq!(
        running.input("laptop", state(0, 1, 0)),
        Err(Dropped::Ignored)
    );
    assert_eq!(recorder.sent.lock().unwrap().len(), 2);
    // Pad 1's rumble goes to the laptop's first controller.
    recorder.reports.lock().unwrap().push(PadReport::Feedback {
        id: 1,
        kind: 1,
        data: vec![9],
    });
    assert_eq!(
        running.feedback(),
        [(
            1,
            PadReport::Feedback {
                id: 0,
                kind: 1,
                data: vec![9]
            }
        )]
    );
    running.resize(1, (1920, 1080)).unwrap();
    assert_eq!(running.shared.sizes.lock().unwrap()[1], (1920, 1080));
    assert!(running.resize(2, (1920, 1080)).is_err());
}

#[test]
fn audio_is_refused_on_old_builds_and_before_the_instance_runs() {
    let folder = tempfile::tempdir().unwrap();
    let (screen, probe) = (screen(), Probe::default());
    let running = start_with(folder.path(), 1, [false; 4], &screen, &probe)
        .0
        .unwrap();
    let error = running.audio_on(Some(19045), 0).err().unwrap().to_string();
    assert!(
        error.starts_with("Per-player audio needs Windows build 20348"),
        "{error}"
    );
    let error = running.audio_on(Some(26100), 0).err().unwrap().to_string();
    assert_eq!(error, "Player 1's game instance is not running");
}
