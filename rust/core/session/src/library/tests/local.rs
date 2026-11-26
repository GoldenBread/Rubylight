//! Tests for local game folder scanning, settings, and cross-launcher matching.
use super::*;
use serde_json::json;
use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;

#[cfg(windows)]
#[test]
fn library_settings_turn_launchers_on_by_default_and_read_the_game_folders() {
    let defaults = Settings::default();
    assert!(defaults.enabled(Source::Epic) && defaults.enabled(Source::Gog));
    assert!(
        !defaults.enabled(Source::Local),
        "no game folders by default"
    );
    assert!(defaults.enabled(Source::Steam) && defaults.enabled(Source::Microsoft));
    let settings = local_settings();
    assert!(settings.enabled(Source::Epic) && !settings.enabled(Source::Gog));
    assert!(settings.enabled(Source::Local));
    assert_eq!(
        settings.local_folders,
        [PathBuf::from(r"D:\Games"), PathBuf::from("E:/More games")]
    );
    assert!(settings.excluded(Path::new(r"D:\Games\Portal Clone")));
    assert!(settings.excluded(Path::new("f:/old/thing/")));
    assert!(!settings.excluded(Path::new(r"D:\Games\Hades")));
    let by_name = Settings {
        local_exclude: vec![r"PORTAL CLONE\".into()],
        ..Settings::default()
    };
    assert!(by_name.excluded(Path::new(r"D:\Games\Portal Clone")));
    // The scan reads a configured folder's subfolders that are not left out.
    assert!(settings.reads_local(r"d:\games\hades"));
    assert!(settings.reads_local(r"e:\more games\celeste"));
    assert!(!settings.reads_local(r"d:\games\portal clone"));
    assert!(!settings.reads_local(r"d:\games\hades\bin"));
    assert!(!settings.reads_local(r"c:\games\hades"));
}

/// Nightjar issue 7: Steam library turned off in the settings showed on again in the Library's
/// launcher list, whose switch is the scan's state. The scan reads Steam's own switch now.
#[test]
fn the_scan_skips_steam_while_steam_library_is_off() {
    let defaults = Settings::default();
    assert!(defaults.enabled(Source::Steam), "on by default");
    for off in ["steam_enabled = false\n", "steam_enabled = \"off\"\n"] {
        let config = butterpollo_config::config::Config::parse(off).unwrap();
        let settings = Settings::from_config(&config);
        assert!(!settings.enabled(Source::Steam), "{off}");
        // The same switch as Steam's own sync, and the other launchers stay on.
        let steam = crate::steam::Settings::from_config(&config);
        assert_eq!(settings.steam, steam.enabled, "{off}");
        assert!(settings.enabled(Source::Epic), "{off}");
    }
    let on = sync_settings("steam_enabled = true\n");
    assert!(on.enabled(Source::Steam));
    // Another launcher's switch is not Steam's.
    let epic_off = sync_settings("library_epic_enabled = false\n");
    assert!(epic_off.enabled(Source::Steam) && !epic_off.enabled(Source::Epic));
}

#[cfg(windows)]
#[test]
fn a_game_folder_starts_its_game_not_its_installers_redistributables_or_crash_reporters() {
    let games = local_games();
    let found: Vec<_> = games
        .iter()
        .map(|game| (game.name.as_str(), game.launch.as_ref().unwrap().command()))
        .collect();
    assert_eq!(
        found,
        [
            ("Celeste", r#""D:\Games\Celeste\Celeste.exe""#.to_owned()),
            ("Hades", r#""D:\Games\Hades\Hades.exe""#.to_owned()),
            (
                "The Witcher 3",
                r#""D:\Games\The Witcher 3\bin\x64\witcher3.exe""#.to_owned()
            ),
        ],
        "Runtimes holds only helpers, Too Deep's program is three folders down, Portal Clone is excluded"
    );
    let witcher = &games[2];
    assert_eq!(witcher.source, Source::Local);
    assert_eq!(witcher.id, r"d:\games\the witcher 3");
    assert_eq!(witcher.install_dir, r"D:\Games\The Witcher 3");
    assert_eq!(
        witcher.working_dir,
        Path::new(r"D:\Games\The Witcher 3\bin\x64")
    );
    assert!(witcher.unavailable_reason.is_none());
    // Known by its folder: a new program in it does not make it a new game.
    let (folder, mut programs) = local_folders().remove(4);
    programs.push(Program {
        path: "bin/x64/witcher3_dx12.exe".into(),
        size: 99_999_999,
    });
    let again = local(&folder, &programs).unwrap();
    assert_eq!(
        (again.id.as_str(), again.uuid()),
        (witcher.id.as_str(), witcher.uuid())
    );
    // "Crash" in the game's own name does not make it a crash reporter.
    let crashlands = local(
        Path::new(r"D:\Games\Crashlands"),
        &[
            Program {
                path: "Crashlands.exe".into(),
                size: 1_000_000,
            },
            Program {
                path: "CrashSender1403.exe".into(),
                size: 9_000_000,
            },
        ],
    )
    .unwrap();
    assert_eq!(
        crashlands.launch.unwrap().command(),
        r#""D:\Games\Crashlands\Crashlands.exe""#
    );
}

#[test]
fn a_game_folder_a_launcher_already_lists_is_listed_once_under_the_launcher() {
    let mut epic = epic_game();
    epic.install_dir = "d:/games/hades/".into();
    let steam = Game::new(
        Source::Steam,
        "570".into(),
        "Dota 2".into(),
        Path::new(r"D:\Games\Steam\steamapps\common\dota 2 beta"),
        None,
    );
    let mut games = local_games();
    games.extend(local(
        Path::new(r"D:\Games\Steam"),
        &[Program {
            path: "steam.exe".into(),
            size: 4_000_000,
        }],
    ));
    // A launcher record pointing at the folder of game folders hides none of them.
    let library = Game::new(
        Source::Ea,
        "1".into(),
        "Broken record".into(),
        Path::new(r"D:\Games\"),
        None,
    );
    games.extend([epic, steam, library]);
    games.extend(local_games());
    let names: Vec<_> = finish(games)
        .into_iter()
        .map(|game| (game.name, game.source))
        .collect();
    assert_eq!(
        names,
        [
            ("Broken record".to_owned(), Source::Ea),
            ("Celeste".to_owned(), Source::Local),
            ("Dota 2".to_owned(), Source::Steam),
            ("The Witcher 3".to_owned(), Source::Local),
            ("Étoile".to_owned(), Source::Epic),
        ],
        "Hades is Epic's, D:\\Games\\Steam holds Steam's games, and repeats are dropped"
    );
}

#[test]
fn games_steam_or_playnite_already_keep_are_already_added() {
    let epic = epic_game();
    let witcher = local_games().remove(2);
    let playnite = |name: &str, plugin: &str| json!({"uuid": "P1", "name": name, "playnite-id": "p1", "playnite-plugin-name": plugin});
    // Playnite's Epic library added Étoile: the Epic scan's copy is that game.
    assert!(epic.matches(&playnite("ÉTOILE™", "Epic")));
    // Playnite's GOG copy of the same title is another copy, as with two launchers.
    assert!(!epic.matches(&playnite("Étoile", "GOG")));
    assert!(!epic.matches(&playnite("Another game", "Epic")));
    // Playnite's EA library from before the rename is still EA's.
    let ea = Game::new(
        Source::Ea,
        "1".into(),
        "Mass Effect".into(),
        Path::new(r"D:\EA\Mass Effect"),
        None,
    );
    assert!(ea.matches(&playnite("Mass Effect", "Origin")));
    assert!(ea.matches(&playnite("Mass Effect", "EA app")));
    assert!(!ea.matches(&playnite("Mass Effect", "Steam")));
    // A game folder is the game any Steam or Playnite app of that name is,
    // and any app started from that folder.
    assert!(witcher.matches(&playnite("The Witcher 3", "GOG")));
    assert!(witcher.matches(&json!({"name": "The Witcher 3", "steam-id": "292030"})));
    assert!(witcher.matches(
        &json!({"name": "Mine", "working-dir": "d:/games/the witcher 3/", "cmd": "w3.exe"})
    ));
    for started in [
        json!({"name": "Mine", "working-dir": r"D:\Games\The Witcher 3\bin\x64"}),
        json!({"name": "Mine", "cmd": r#""D:\Games\The Witcher 3\bin\x64\witcher3.exe" -dx12"#}),
        json!({"name": "Mine", "cmd": r"d:\games\the witcher 3\bin\x64\witcher3.exe -dx12"}),
    ] {
        assert!(witcher.matches(&started), "{started}");
    }
    for elsewhere in [
        json!({"name": "The Witcher 3", "cmd": "other.exe"}),
        json!({"name": "Mine", "cmd": r#""D:\Games\The Witcher 3 Mods\tool.exe""#}),
        json!({"name": "Mine", "working-dir": r"D:\Games\The Witcher 3\..\Hades"}),
    ] {
        assert!(!witcher.matches(&elsewhere), "{elsewhere}");
    }
    let mut apps = vec![json!({"name": "The Witcher 3", "steam-id": "292030"})];
    let results = import(
        &mut apps,
        std::slice::from_ref(&witcher),
        &[selected(&witcher)],
    );
    assert_eq!(results[0].result, "already_present");
    assert_eq!(apps.len(), 1);
}

#[cfg(windows)]
#[test]
fn a_game_folder_import_starts_its_program_from_its_folder() {
    let hades = local_games().remove(1);
    let mut apps = vec![];
    assert_eq!(
        import(&mut apps, std::slice::from_ref(&hades), &[selected(&hades)])[0].result,
        "added"
    );
    let app = &apps[0];
    assert_eq!(app["cmd"], r#""D:\Games\Hades\Hades.exe""#);
    assert_eq!(app["launcher-source"], "local");
    assert_eq!(app["launcher-id"], r"d:\games\hades");
    assert_eq!(app["working-dir"], r"D:\Games\Hades");
    assert!(hades.matches(app));
}

// A Windows folder (`D:\Games\Dispatcher`): its name is the last component on Windows only.
#[cfg(windows)]
#[test]
fn helper_token_check_does_not_hide_dispatcher_exe() {
    let folder = Path::new(r"D:\Games\Dispatcher");
    let programs = [Program {
        path: "Dispatcher.exe".into(),
        size: 1_000_000,
    }];
    let game = local(folder, &programs).expect("Dispatcher.exe must not match patcher substring");
    assert_eq!(game.name, "Dispatcher");
}
