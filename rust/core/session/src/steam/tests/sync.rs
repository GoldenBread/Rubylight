//! Tests for Steam sync settings, selection, reconciliation, and process tracking.
use super::super::sync::update;
use super::super::*;
use super::*;
use crate::library;
use butterpollo_config::config::Config;
use serde_json::{Value, json};
use std::collections::HashMap;

#[test]
fn every_installed_game_is_the_default_and_an_explicit_false_keeps_recent() {
    assert!(Settings::from_config(&Config::default()).sync_all_installed);
    assert_eq!(
        Settings::from_config(&Config::default()).source(),
        "installed"
    );
    let recent = Settings::from_config(&Config::parse("steam_sync_all_installed = false").unwrap());
    assert!(!recent.sync_all_installed);
    assert_eq!(recent.source(), "recent");
}
/// The apps whose games read controllers through Steam Input: the Steam Big Picture entry,
/// `steam://` commands, commands that run Steam's own program and the Steam library's entries;
/// not the Desktop, a game's own .exe started directly, another store's link or a Playnite
/// entry, which Playnite starts its own way.
#[test]
fn apps_launched_through_steam_are_told_apart() {
    for app in [
        json!({"name": "Steam Big Picture", "detached": ["steam://open/bigpicture"]}),
        json!({"name": "Dota 2", "cmd": "cmd /c start \"\" steam://rungameid/570"}),
        json!({"name": "Hades", "uuid": canonical_uuid(1145360)}),
        json!({"name": "Portal", "steam-id": "400"}),
        json!({"name": "Imported", "launcher-source": "steam", "launcher-id": "620"}),
        json!({"name": "Gamepad UI", "cmd": "STEAM://open/gamepadui"}),
        json!({"name": "Dota 2 by Steam", "cmd": "steam.exe -applaunch 570"}),
        json!({"name": "Steam UI", "cmd": r#""C:\Program Files (x86)\Steam\steam.exe" -gamepadui"#}),
        json!({"name": "Big Picture", "detached": [r"C:\Program Files (x86)\Steam\Steam.exe -bigpicture"]}),
    ] {
        assert!(launched_through_steam(&app), "{app}");
    }
    for app in [
        json!({"name": "Desktop"}),
        json!({"name": "Game", "cmd": "C:\\Games\\game.exe"}),
        json!({"name": "Epic", "cmd": "com.epicgames.launcher://apps/x?action=launch"}),
        json!({"name": "Emulator", "detached": ["C:\\Emulators\\emu.exe"]}),
        json!({"name": "Other uuid", "uuid": "0b3d5a8e-6f39-4c7e-9a15-2f4f6c1d7e90"}),
        json!({"name": "Dota 2 directly", "cmd": r"C:\Program Files (x86)\Steam\steamapps\common\dota 2 beta\game\bin\win64\dota2.exe"}),
        json!({"name": "Server", "cmd": r"C:\Tools\steamcmd.exe +login anonymous"}),
        json!({"name": "Dota 2", "playnite-id": "aa", "playnite-plugin-id": "cb91dfc9-b977-43bf-8e70-55f46e410fab"}),
    ] {
        assert!(!launched_through_steam(&app), "{app}");
    }
}
#[test]
fn selection_and_identity_follow_vibepollo() {
    assert_eq!(canonical_uuid(570), "53544541-4d00-5000-8000-00000000023a");
    assert_eq!(app_id_of(&json!({"uuid": canonical_uuid(570)})), Some(570));
    assert_eq!(app_id_of(&json!({"steam-id": "steam-730"})), Some(730));
    let games = vec![
        game(1, "Old", 100),
        game(2, "New", 300),
        game(3, "Never", 0),
        game(228980, "Steamworks Common Redistributables", 400),
        Game {
            app_type: "Tool".into(),
            ..game(4, "SDK", 500)
        },
    ];
    let mut settings = Settings::from_config(&Config::default());
    settings.sync_all_installed = false;
    settings.recent_max_age_days = 0;
    let ids = |s: &Settings| {
        select(&games, s, 1000)
            .iter()
            .map(|g| g.appid)
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&settings), [2, 1]);
    settings.recent_games = 1;
    assert_eq!(ids(&settings), [2]);
    settings.recent_games = 10;
    settings.exclusions = id_names(r#"[{"id":"","name":"new"}]"#);
    assert_eq!(ids(&settings), [1]);
    settings.sync_all_installed = true;
    settings.exclusions = id_names("1");
    assert_eq!(ids(&settings), [2, 3]);
    settings.include_tools = true;
    assert_eq!(ids(&settings), [2, 3, 228980, 4]);
}
#[test]
fn reconcile_owns_only_managed_apps() {
    let mut settings = Settings::from_config(&Config::default());
    settings.sync_all_installed = true;
    settings.remove_uninstalled = true;
    let dota = game(570, "Dota 2", 10);
    let cs = game(730, "Counter-Strike 2", 20);
    let mut apps = vec![
        json!({"name":"Desktop"}),
        // Added by hand: only its missing cover is filled in.
        json!({"name":"My CS","steam-id":"730","cmd":"custom","image-path":""}),
        // Synced but no longer installed.
        json!({"name":"Gone","uuid":canonical_uuid(440),"steam-id":"440","steam-managed":"auto","synced-by":"steam"}),
    ];
    let covers = HashMap::from([(730, PathBuf::from("C:\\covers\\steam_730.png"))]);
    assert!(reconcile(&mut apps, &[&dota, &cs], &settings, &covers, &[]));
    let names: Vec<_> = apps.iter().map(|a| a["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Desktop", "My CS", "Dota 2"]);
    assert_eq!(apps[1]["cmd"], "custom");
    assert_eq!(apps[1]["image-path"], "C:/covers/steam_730.png");
    assert_eq!(apps[2]["uuid"], canonical_uuid(570));
    assert_eq!(apps[2]["cmd"], "cmd /c start \"\" steam://rungameid/570");
    assert_eq!(apps[2]["image-path"], FALLBACK_IMAGE);
    assert_eq!(
        apps[2]["steam-install-dir"],
        "C:/Steam/steamapps/common/Dota 2"
    );
    // A second pass changes nothing.
    assert!(!reconcile(
        &mut apps,
        &[&dota, &cs],
        &settings,
        &covers,
        &[]
    ));
    // A cover the user picked stays.
    apps[2]["image-path"] = json!("C:/mine.png");
    assert!(!reconcile(
        &mut apps,
        &[&dota, &cs],
        &settings,
        &covers,
        &[]
    ));
    assert_eq!(apps[2]["image-path"], "C:/mine.png");
}
/// Every sync that could rename, remove or duplicate `apps`: the game installed and
/// selected, excluded, uninstalled, and outside the recent window. Each leaves them as they are.
fn untouched_by_every_sync(apps: Vec<Value>, appid: u32) {
    let portal = game(appid, "Portal 2", 1000);
    let mut installed = Settings::from_config(&Config::default());
    installed.sync_all_installed = true;
    installed.remove_uninstalled = true;
    let mut excluded = installed.clone();
    excluded.exclusions = id_names(&appid.to_string());
    let mut recent = Settings::from_config(&Config::default());
    recent.sync_all_installed = false;
    recent.recent_games = 1;
    for (case, settings, selected) in [
        ("installed", &installed, vec![&portal]),
        ("excluded", &excluded, vec![&portal]),
        ("uninstalled", &installed, vec![]),
        ("outside the recent window", &recent, vec![]),
    ] {
        let mut synced = apps.clone();
        assert!(
            !reconcile(&mut synced, &selected, settings, &HashMap::new(), &[]),
            "{case}"
        );
        assert_eq!(synced, apps, "{case}");
    }
}
#[test]
fn a_legacy_steam_uuid_app_keeps_its_name_command_and_detach_through_every_sync() {
    let mine = json!({"name":"My Portal","uuid":canonical_uuid(620),"cmd":"C:/portal.bat",
            "auto-detach":false,"image-path":"C:/portal.png"});
    // A second copy, as another host left it: both are the user's.
    let copy = json!({"name":"Portal (couch)","uuid":canonical_uuid(620),"cmd":"C:/couch.bat",
            "image-path":"C:/couch.png"});
    untouched_by_every_sync(vec![mine, copy], 620);
}
#[test]
fn an_unstamped_vibepollo_synced_app_is_the_users() {
    let vibepollo = json!({"name":"Portal 2 (mine)","uuid":canonical_uuid(620),"steam-id":"620",
            "steam-managed":"auto","steam-source":"recent","cmd":"C:/portal.bat","auto-detach":false,
            "image-path":"C:/portal.png"});
    untouched_by_every_sync(vec![vibepollo.clone(), vibepollo], 620);
}
#[test]
fn no_second_app_joins_a_users_app_for_the_same_game() {
    let mut settings = Settings::from_config(&Config::default());
    settings.sync_all_installed = true;
    let games = [
        game(570, "Dota 2", 10),
        game(620, "Portal 2", 10),
        game(730, "Counter-Strike 2", 10),
        game(440, "Team Fortress 2", 10),
    ];
    let selected: Vec<&Game> = games.iter().collect();
    let mut apps = vec![
        json!({"name":"Dota","steam-id":"570"}),
        // An "Add games" import and a hand-made app that starts the game through Steam.
        json!({"name":"Portal","launcher-source":"steam","launcher-id":"620","cmd":"C:/portal.bat"}),
        json!({"name":"CS","cmd":"cmd /c start steam://rungameid/730"}),
        // The sync's own copy of a game the user also has: the user's stays, alone.
        json!({"name":"Dota 2","uuid":canonical_uuid(570),"steam-id":"570","steam-managed":"auto","synced-by":"steam"}),
    ];
    assert!(reconcile(
        &mut apps,
        &selected,
        &settings,
        &HashMap::new(),
        &[]
    ));
    let names: Vec<_> = apps.iter().map(|a| a["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Dota", "Portal", "CS", "Team Fortress 2"]);
    assert_eq!(game_of(&apps[2]), Some(730));
}
#[test]
fn the_sync_updates_and_removes_only_the_apps_it_stamped() {
    let mut settings = Settings::from_config(&Config::default());
    settings.sync_all_installed = true;
    settings.remove_uninstalled = true;
    let dota = game(570, "Dota 2", 10);
    let mut apps = vec![
        json!({"name":"Old name","uuid":canonical_uuid(570),"steam-id":"570",
            "steam-managed":"auto","synced-by":"steam","cmd":"old","auto-detach":false}),
    ];
    assert!(reconcile(
        &mut apps,
        &[&dota],
        &settings,
        &HashMap::new(),
        &[]
    ));
    assert_eq!(apps[0]["name"], "Dota 2");
    assert_eq!(apps[0]["cmd"], launch_command(570));
    assert_eq!(apps[0]["auto-detach"], true);
    assert_eq!(apps[0]["synced-by"], "steam");
    assert!(reconcile(&mut apps, &[], &settings, &HashMap::new(), &[]));
    assert!(apps.is_empty(), "uninstalled");
    // In recent mode a game that left the window takes only the stamped app with it.
    let mut recent = Settings::from_config(&Config::default());
    recent.sync_all_installed = false;
    recent.recent_games = 1;
    let stamped = json!({"name":"Dota 2","uuid":canonical_uuid(570),"steam-id":"570",
            "steam-managed":"auto","synced-by":"steam"});
    let unstamped = json!({"name":"Portal 2","uuid":canonical_uuid(620),"steam-id":"620",
            "steam-managed":"auto"});
    let mut apps = vec![stamped, unstamped.clone()];
    assert!(reconcile(&mut apps, &[], &recent, &HashMap::new(), &[]));
    assert_eq!(apps, [unstamped]);
}
#[test]
fn a_steam_game_deleted_from_the_library_is_not_added_again() {
    let mut settings = Settings::from_config(&Config::default());
    settings.sync_all_installed = true;
    let (dota, cs) = (game(570, "Dota 2", 10), game(730, "Counter-Strike 2", 10));
    let mut apps = vec![];
    assert!(reconcile(
        &mut apps,
        &[&dota, &cs],
        &settings,
        &HashMap::new(),
        &["steam:570".into()]
    ));
    let names: Vec<_> = apps.iter().map(|a| a["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Counter-Strike 2"]);
    // Any app that starts a Steam game dismisses it when deleted, whoever made it.
    assert_eq!(library::dismissal_keys(&apps[0]), ["steam:730"]);
    assert_eq!(
        library::dismissal_keys(&json!({"name":"Mine","cmd":"steam://rungameid/620"})),
        ["steam:620"]
    );
    assert_eq!(
        library::dismissal_keys(
            &json!({"launcher-source":"steam","launcher-id":"440","steam-id":"440"})
        ),
        ["steam:440"]
    );
}
#[test]
fn tracker_follows_the_game_and_its_children() {
    let process = |pid, parent, name: &str| Process {
        pid,
        parent,
        started: u64::from(pid) * 10,
        name: name.into(),
    };
    let before = [process(1, 0, "explorer.exe"), process(2, 1, "steam.exe")];
    let mut tracker = Tracker::new(
        &before,
        "C:\\Games\\Portal",
        std::time::Duration::from_secs(15),
    )
    .with_exit_grace(std::time::Duration::ZERO);
    let images = |pid| match pid {
        3 => Some("c:\\games\\portal\\portal.exe".to_owned()),
        5 => Some("C:\\Games\\Portal2\\other.exe".to_owned()),
        _ => None,
    };
    assert_eq!(tracker.update(&before, images), Tracked::Waiting);
    let launched = [
        process(1, 0, "explorer.exe"),
        process(2, 1, "steam.exe"),
        process(3, 2, "portal.exe"),
        process(4, 3, "crashhandler.exe"),
        process(5, 1, "other.exe"),
    ];
    assert_eq!(tracker.update(&launched, images), Tracked::Running);
    assert_eq!(tracker.tracked.keys().copied().collect::<Vec<_>>(), [3, 4]);
    assert_eq!(tracker.update(&before, images), Tracked::Exited);
}
#[test]
fn the_paths_an_update_asks_for_are_the_ones_listed_before_it() {
    let process = |pid, parent, started, name: &str| Process {
        pid,
        parent,
        started,
        name: name.into(),
    };
    let before = [
        process(1, 0, 10, "explorer.exe"),
        process(2, 1, 20, "steam.exe"),
    ];
    let mut tracker = Tracker::new(
        &before,
        "C:\\Games\\Portal",
        std::time::Duration::from_secs(15),
    );
    let images = |pid| (pid == 3).then(|| "c:\\games\\portal\\portal.exe".to_owned());
    let rounds = [
        vec![
            process(1, 0, 10, "explorer.exe"),
            process(2, 1, 20, "steam.exe"),
            process(3, 2, 30, "portal.exe"),
            process(4, 3, 40, "crashhandler.exe"),
            process(5, 1, 50, "other.exe"),
            process(6, 1, 60, "chrome.exe"),
        ],
        // Looked at once: the same processes are not asked about again,
        // while a new one and a reused id are.
        vec![
            process(1, 0, 10, "explorer.exe"),
            process(3, 2, 30, "portal.exe"),
            process(5, 1, 50, "other.exe"),
            process(7, 1, 70, "notes.exe"),
            process(4, 1, 80, "reused.exe"),
        ],
    ];
    let mut listed = vec![];
    for now in rounds {
        let unchecked = tracker.unchecked(&now);
        let mut asked = vec![];
        tracker.update(&now, |pid| {
            asked.push(pid);
            images(pid)
        });
        assert!(
            asked.iter().all(|pid| unchecked.contains(pid)),
            "{asked:?} not in {unchecked:?}"
        );
        listed.push(unchecked);
    }
    assert_eq!(listed, [vec![3, 4, 5], vec![7, 4]]);
}
#[test]
fn a_game_restarting_through_steam_and_a_browser_it_opens_do_not_end_or_extend_it() {
    let process = |pid, parent, name: &str| Process {
        pid,
        parent,
        started: u64::from(pid) * 10,
        name: name.into(),
    };
    let before = [process(1, 0, "explorer.exe"), process(2, 1, "steam.exe")];
    let images = |pid| match pid {
        3 | 6 => Some("c:\\games\\portal\\portal.exe".to_owned()),
        _ => None,
    };
    let mut tracker = Tracker::new(
        &before,
        "C:\\Games\\Portal",
        std::time::Duration::from_secs(15),
    );
    let running = [
        process(1, 0, "explorer.exe"),
        process(2, 1, "steam.exe"),
        process(3, 2, "portal.exe"),
        process(4, 3, "chrome.exe"),
    ];
    assert_eq!(tracker.update(&running, images), Tracked::Running);
    // The browser the game opened is not the game.
    assert_eq!(tracker.tracked.keys().copied().collect::<Vec<_>>(), [3]);
    // Restarting through Steam: no game process for a moment.
    assert_eq!(tracker.update(&before, images), Tracked::Running);
    let restarted = [
        process(1, 0, "explorer.exe"),
        process(2, 1, "steam.exe"),
        process(6, 2, "portal.exe"),
    ];
    assert_eq!(tracker.update(&restarted, images), Tracked::Running);
    assert_eq!(tracker.tracked.keys().copied().collect::<Vec<_>>(), [6]);
}
#[test]
fn launcher_handoff_never_owns_shared_launchers_even_inside_the_game_folder() {
    let process = |pid, parent, name: &str| Process {
        pid,
        parent,
        started: u64::from(pid),
        name: name.into(),
    };
    let mut tracker = Tracker::new(
        &[],
        r"E:\Games\Diablo IV",
        std::time::Duration::from_secs(15),
    );
    let launchers = [
        process(1, 0, "Battle.net Launcher.exe"),
        process(2, 1, "Agent.exe"),
    ];
    let image = |_| Some(r"E:\Games\Diablo IV\program.exe".into());
    assert_eq!(tracker.update(&launchers, image), Tracked::Waiting);
    assert!(tracker.tracked.is_empty());
    let started = [
        launchers[0].clone(),
        launchers[1].clone(),
        process(3, 1, "Diablo IV.exe"),
        process(4, 3, "Battle.net.exe"),
    ];
    assert_eq!(tracker.update(&started, image), Tracked::Running);
    assert_eq!(tracker.tracked.keys().copied().collect::<Vec<_>>(), [3]);
    assert_eq!(
        tracker.update(&launchers, image),
        Tracked::Running,
        "launcher handoff gets the existing restart grace period"
    );
    let restarted = [process(5, 1, "Diablo IV.exe")];
    assert_eq!(tracker.update(&restarted, image), Tracked::Running);
    assert_eq!(tracker.tracked.keys().copied().collect::<Vec<_>>(), [5]);
    tracker.grace = std::time::Duration::ZERO;
    assert_eq!(tracker.update(&launchers, image), Tracked::Exited);
    assert!(tracker.tracked.is_empty());
}
#[test]
fn steams_own_cover_drops_a_steamgriddb_mark() {
    let ours = "C:/profile/covers/sgdb_x.png";
    let mut app =
        json!({"name": "Fixture", "image-path": ours, crate::steamgriddb::COVER_FIELD: ours});
    let game = Game {
        appid: 570,
        name: "Fixture".into(),
        ..Default::default()
    };
    update(
        &mut app,
        &game,
        "auto",
        Some(Path::new("C:/profile/covers/steam_570.png")),
    );
    assert_eq!(app["image-path"], "C:/profile/covers/steam_570.png");
    assert!(app.get(crate::steamgriddb::COVER_FIELD).is_none());
}
