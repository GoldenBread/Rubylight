//! Unit tests for host per-device game settings file swapping.

use super::*;
use crate::state::test_support::Fixture;

/// The app, in the host's library, and its settings file in a folder of its own.
fn game(f: &Fixture) -> (App, PathBuf, BTreeMap<String, String>) {
    let folder = f.host.directory.join("game");
    std::fs::create_dir_all(&folder).unwrap();
    let file = folder.join("Settings.ini");
    std::fs::write(&file, "pc").unwrap();
    let app: App = serde_json::from_value(json!({
        "name": "Game",
        "uuid": "game-1",
        "game-settings-files": ["%GameDir%\\Settings.ini"],
    }))
    .unwrap();
    f.host.apps.write().unwrap().push(app.clone());
    // Windows names are compared without regard to case.
    let environment =
        BTreeMap::from([("GAMEDIR".to_owned(), folder.to_string_lossy().into_owned())]);
    (app, file, environment)
}

fn args(device: &str) -> HashMap<String, String> {
    HashMap::from([
        ("clientUuid".to_owned(), device.to_owned()),
        ("clientName".to_owned(), format!("{device} name")),
    ])
}

fn read(file: &Path) -> String {
    std::fs::read_to_string(file).unwrap()
}

#[cfg(windows)]
#[test]
fn a_launch_swaps_the_devices_copy_in_and_the_app_stopping_puts_the_pcs_back() {
    let f = Fixture::new();
    let (app, file, environment) = game(&f);
    let swap = launching(&f.host, &app, &args("tv"), &environment)
        .unwrap()
        .expect("the app lists a file");
    std::fs::write(&file, "tv").unwrap();
    let shown = status(&f.host, "game-1").unwrap();
    assert_eq!(shown["pending"][0]["running"], true);
    assert_eq!(shown["pending"][0]["deviceName"], "tv name");
    drop(swap);
    assert_eq!(read(&file), "pc");
    let swap = launching(&f.host, &app, &args("tv"), &environment)
        .unwrap()
        .unwrap();
    assert_eq!(read(&file), "tv", "the TV's copy is in place");
    drop(swap);
    assert_eq!(read(&file), "pc");
    assert!(
        status(&f.host, "game-1").unwrap()["pending"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[cfg(windows)]
#[test]
fn a_launch_without_a_device_or_files_swaps_nothing_and_a_missing_file_fails_it() {
    let f = Fixture::new();
    let (app, file, environment) = game(&f);
    let plain: App = serde_json::from_value(json!({"name": "Plain"})).unwrap();
    assert!(
        launching(&f.host, &plain, &args("tv"), &environment)
            .unwrap()
            .is_none()
    );
    assert!(
        launching(&f.host, &app, &HashMap::new(), &environment)
            .unwrap()
            .is_none(),
        "a console launch has no device"
    );
    std::fs::remove_file(&file).unwrap();
    let error = launching(&f.host, &app, &args("tv"), &environment)
        .err()
        .unwrap();
    assert!(format!("{error:#}").contains("does not exist"), "{error:#}");
    assert!(
        format!("{error:#}").contains("Game did not start"),
        "{error:#}"
    );
    let unset = launching(&f.host, &app, &args("tv"), &BTreeMap::new())
        .err()
        .unwrap();
    assert!(format!("{unset:#}").contains("not set"), "{unset:#}");
}

#[cfg(windows)]
#[test]
fn a_game_that_outlives_its_app_keeps_the_files_until_it_closes() {
    let f = Fixture::new();
    let (app, file, environment) = game(&f);
    let mut swap = launching(&f.host, &app, &args("tv"), &environment)
        .unwrap()
        .unwrap();
    let program = f.host.directory.join("game").join("Outlives.exe");
    let program = Watched {
        pid: 4242,
        started: 99,
        program: program.to_string_lossy().into_owned(),
    };
    swap.watch([program.clone()]);
    locked(&RUNNING).insert((4242, 99));
    std::fs::write(&file, "tv").unwrap();
    // The app stopped, but the game it started still runs.
    drop(swap);
    assert_eq!(
        read(&file),
        "tv",
        "the PC's file waits for the game to close"
    );
    assert!(retry(&f.host), "still left");
    assert_eq!(read(&file), "tv");
    std::fs::write(&file, "tv-exit").unwrap();
    locked(&RUNNING).remove(&(4242, 99));
    assert!(!retry(&f.host));
    assert_eq!(read(&file), "pc");
    let key = game_settings::app_key(&app);
    let copy = store(&f.host.directory).device_copy(
        &key,
        "tv",
        &game_settings::file_key("%GameDir%\\Settings.ini"),
    );
    assert_eq!(read(&copy), "tv-exit");
}

#[cfg(windows)]
#[test]
fn a_swap_stuck_on_a_process_is_put_back_from_the_console() {
    let f = Fixture::new();
    let (app, file, environment) = game(&f);
    let mut swap = launching(&f.host, &app, &args("tv"), &environment)
        .unwrap()
        .unwrap();
    let id = swap.id.clone();
    swap.watch([Watched {
        pid: 5151,
        started: 3,
        program: "C:\\Windows\\SystemApps\\msedgewebview2.exe".into(),
    }]);
    locked(&RUNNING).insert((5151, 3));
    std::fs::write(&file, "tv").unwrap();
    assert!(
        release(&f.host, &id).is_err(),
        "not while the app still streams"
    );
    drop(swap);
    assert_eq!(read(&file), "tv");
    let shown = status(&f.host, "game-1").unwrap();
    assert_eq!(shown["pending"][0]["id"], id.as_str());
    assert!(
        shown["pending"][0]["error"]
            .as_str()
            .unwrap()
            .contains("still running")
    );
    release(&f.host, &id).unwrap();
    locked(&RUNNING).remove(&(5151, 3));
    assert_eq!(read(&file), "pc");
    assert!(
        status(&f.host, "game-1").unwrap()["pending"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_game_guarded_against_process_queries_counts_as_running_while_listed() {
    let game = Watched {
        pid: 7,
        started: 100,
        program: "Game.exe".into(),
    };
    assert!(alive(&game, 100, || unreachable!()));
    assert!(!alive(&game, 101, || unreachable!()), "a later process");
    assert!(alive(&game, 0, || true), "access denied, still listed");
    assert!(!alive(&game, 0, || false), "gone");
}

#[cfg(windows)]
#[test]
fn a_swap_that_never_saw_its_game_warns_in_the_console() {
    let f = Fixture::new();
    let (app, _, environment) = game(&f);
    let swap = launching(&f.host, &app, &args("tv"), &environment)
        .unwrap()
        .unwrap();
    drop(swap);
    let warned = |h: &Host| {
        h.warnings
            .snapshot()
            .iter()
            .any(|warning| warning.code == UNWATCHED_WARNING)
    };
    assert!(warned(&f.host));
    let mut swap = launching(&f.host, &app, &args("tv"), &environment)
        .unwrap()
        .unwrap();
    swap.watch([Watched {
        pid: 6161,
        started: 5,
        program: "Game.exe".into(),
    }]);
    drop(swap);
    assert!(!warned(&f.host), "cleared once the game was seen");
}

#[cfg(windows)]
#[test]
fn a_game_moved_to_another_device_saves_its_settings_for_the_device_now_playing() {
    let f = Fixture::new();
    let (app, file, environment) = game(&f);
    let swap = launching(&f.host, &app, &args("tv"), &environment)
        .unwrap()
        .unwrap();
    std::fs::write(&file, "tv changes").unwrap();
    // Continue here on the phone: its stream runs.
    hand_over(&swap.handle(), "phone", "phone name");
    assert_eq!(read(&file), "tv changes", "the running game's file stays");
    let key = game_settings::app_key(&app);
    let copy = |device: &str| {
        read(&store(&f.host.directory).device_copy(
            &key,
            device,
            &game_settings::file_key("%GameDir%\\Settings.ini"),
        ))
    };
    assert_eq!(copy("tv"), "tv changes", "the TV keeps what it changed");
    assert_eq!(
        status(&f.host, "game-1").unwrap()["pending"][0]["deviceName"],
        "phone name"
    );
    std::fs::write(&file, "phone changes").unwrap();
    drop(swap);
    assert_eq!(read(&file), "pc");
    assert_eq!(copy("phone"), "phone changes");
    assert_eq!(
        copy("tv"),
        "tv changes",
        "never overwritten with the phone's"
    );
}

#[cfg(windows)]
#[test]
fn a_cancelled_launch_saves_no_copy_for_the_device() {
    let f = Fixture::new();
    let (app, file, environment) = game(&f);
    let swap = launching(&f.host, &app, &args("tv"), &environment)
        .unwrap()
        .unwrap();
    assert_eq!(swap.files(), std::slice::from_ref(&file));
    std::fs::write(&file, "half").unwrap();
    swap.cancel();
    assert_eq!(read(&file), "pc");
    let key = game_settings::app_key(&app);
    assert!(!store(&f.host.directory).saved(&key, "tv", "%GameDir%\\Settings.ini"));
}

#[cfg(windows)]
#[test]
fn what_a_crash_left_is_put_back_by_the_retry() {
    let f = Fixture::new();
    let (app, file, environment) = game(&f);
    let swap = launching(&f.host, &app, &args("tv"), &environment)
        .unwrap()
        .unwrap();
    std::fs::write(&file, "tv").unwrap();
    // A crash: the swap never finishes, and the next run has none live.
    locked(&LIVE).remove(&swap.id);
    std::mem::forget(swap);
    assert!(!retry(&f.host));
    assert_eq!(read(&file), "pc");
    let key = game_settings::app_key(&app);
    assert!(store(&f.host.directory).saved(&key, "tv", "%GameDir%\\Settings.ini"));
}
