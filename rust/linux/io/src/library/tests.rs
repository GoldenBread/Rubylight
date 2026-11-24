//! Tests for the Linux library scan: Steam, Heroic and Lutris from small fixture files.

use super::*;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

/// A fresh folder below the test temp folder, removed when dropped.
struct Scratch(PathBuf);
impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("rubylight-library-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn write(&self, relative: &str, content: impl AsRef<[u8]>) -> PathBuf {
        let path = self.0.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
        path
    }
    fn dir(&self, relative: &str) -> PathBuf {
        let path = self.0.join(relative);
        std::fs::create_dir_all(&path).unwrap();
        path
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn manifest(appid: u32, name: &str, folder: &str) -> String {
    format!(
        "\"AppState\"\n{{\n\t\"appid\"\t\t\"{appid}\"\n\t\"name\"\t\t\"{name}\"\n\t\"installdir\"\t\t\"{folder}\"\n\t\"StateFlags\"\t\t\"4\"\n}}\n"
    )
}

#[test]
fn steam_lists_games_without_tools_and_finds_both_cover_layouts() {
    let root = Scratch::new("steam");
    let steam = root.dir("Steam");
    let apps = steam.join("steamapps");
    for (id, name, folder) in [
        (100, "Alpha", "Alpha"),
        (200, "Beta", "Beta"),
        (1493710, "Proton Experimental", "Proton - Experimental"),
        (
            1628350,
            "Steam Linux Runtime 3.0 (sniper)",
            "SteamLinuxRuntime_sniper",
        ),
        (1826330, "Some Runtime", "Runtime"),
        (
            228980,
            "Steamworks Common Redistributables",
            "Steamworks Shared",
        ),
    ] {
        root.write(
            &format!("Steam/steamapps/appmanifest_{id}.acf"),
            manifest(id, name, folder),
        );
        root.dir(&format!("Steam/steamapps/common/{folder}"));
    }
    // The old flat layout for one game, the per-app folder for the other.
    root.write("Steam/appcache/librarycache/100_library_600x900.jpg", "x");
    root.write("Steam/appcache/librarycache/200/library_600x900.jpg", "x");
    assert!(apps.is_dir());
    // A second library, listed in the new and in the old way, and a game in it.
    let second = root.dir("Second");
    root.write(
        "Second/steamapps/appmanifest_300.acf",
        manifest(300, "Gamma", "Gamma"),
    );
    root.dir("Second/steamapps/common/Gamma");
    root.write(
        "Steam/steamapps/libraryfolders.vdf",
        format!(
            "\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n\t\"1\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n}}\n",
            steam.display(),
            second.display()
        ),
    );
    // A game still downloading is not installed.
    root.write(
        "Steam/steamapps/appmanifest_400.acf",
        manifest(400, "Delta", "Delta").replace("\"4\"", "\"1026\""),
    );
    root.dir("Steam/steamapps/common/Delta");
    root.write("Steam/steamapps/appmanifest_500.acf", "garbage {{{");
    let mut games = vec![];
    steam_games(std::slice::from_ref(&steam), &mut games);
    let mut names: Vec<&str> = games.iter().map(|game| game.name.as_str()).collect();
    names.sort();
    assert_eq!(names, ["Alpha", "Beta", "Gamma"]);
    let cover = |name: &str| {
        games
            .iter()
            .find(|game| game.name == name)
            .unwrap()
            .cover
            .clone()
    };
    assert!(cover("Alpha").unwrap().ends_with("100_library_600x900.jpg"));
    assert!(cover("Beta").unwrap().ends_with("200/library_600x900.jpg"));
    let alpha = games.iter().find(|game| game.name == "Alpha").unwrap();
    assert_eq!(
        alpha.launch,
        Some(Launch::Uri {
            uri: "steam://rungameid/100".into()
        })
    );
    assert_eq!(alpha.source, Source::Steam);
}

#[test]
fn steam_users_come_from_loginusers_most_recent_first() {
    let root = Scratch::new("steam-users");
    root.write(
        "Steam/config/loginusers.vdf",
        "\"users\"\n{\n\"76561198000000001\"\n{\n\"AccountName\"\t\"old\"\n\"PersonaName\"\t\"Old\"\n\"MostRecent\"\t\"0\"\n\"Timestamp\"\t\"5\"\n}\n\"76561198000000002\"\n{\n\"AccountName\"\t\"new\"\n\"PersonaName\"\t\"New\"\n\"MostRecent\"\t\"1\"\n\"Timestamp\"\t\"9\"\n}\n}\n",
    );
    let users = crate::steam::users(&root.0.join("Steam"));
    assert_eq!(users.len(), 2);
    assert_eq!(users[0].account, "new");
    assert!(users[0].most_recent);
    assert_eq!(users[0].account_id(), 39_734_274);
    assert!(crate::steam::users(&root.0.join("Nothing")).is_empty());
}

#[test]
fn steam_tool_ids_are_not_games() {
    let game = |appid, name: &str| butterpollo_session::steam::Game {
        appid,
        name: name.into(),
        ..Default::default()
    };
    assert!(crate::steam::importable(&game(570, "Dota 2")));
    assert!(!crate::steam::importable(&game(1161040, "BattlEye Thing")));
    assert!(!crate::steam::importable(&game(7, "Proton 9.0")));
}

fn hashed(root: &Path, url: &str, suffix: &str) {
    let digest = Sha256::digest(format!("{url}{suffix}").as_bytes());
    let name: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    std::fs::create_dir_all(root.join("images-cache")).unwrap();
    std::fs::write(root.join("images-cache").join(name), "image").unwrap();
}

#[test]
fn heroic_lists_every_runner_with_titles_and_cached_covers() {
    let scratch = Scratch::new("heroic");
    let root = scratch.dir("heroic");
    let epic = scratch.dir("Games/epic-one");
    let gog = scratch.dir("Games/gog-one");
    let amazon = scratch.dir("Games/amazon-one");
    let side = scratch.dir("Games/side-one");
    scratch.write(
        "heroic/legendaryConfig/legendary/installed.json",
        serde_json::json!({
            "e1": {"app_name": "e1", "title": "Epic One", "install_path": epic, "is_dlc": false},
            "e2": {"app_name": "e2", "title": "Epic Dlc", "install_path": epic, "is_dlc": true},
            "e3": {"app_name": "e3", "title": "Gone", "install_path": "/nonexistent/folder"}
        })
        .to_string(),
    );
    scratch.write(
        "heroic/gog_store/installed.json",
        serde_json::json!({"installed": [{"appName": "g1", "install_path": gog, "platform": "windows"}]}).to_string(),
    );
    scratch.write(
        "heroic/store_cache/gog_library.json",
        serde_json::json!({"library": [{"app_name": "g1", "title": "Gog One", "art_square": "https://img/g1.jpg"}]}).to_string(),
    );
    hashed(&root, "https://img/g1.jpg", "?h=400&resize=1&w=300");
    scratch.write(
        "heroic/nile_store/installed.json",
        serde_json::json!([{"id": "n1", "path": amazon}]).to_string(),
    );
    scratch.write(
        "heroic/store_cache/nile_library.json",
        serde_json::json!({"library": [{"app_name": "n1", "title": "Amazon One"}]}).to_string(),
    );
    scratch.write(
        "heroic/sideload_apps/library.json",
        serde_json::json!({"games": [
            {"app_name": "s1", "title": "Side One", "folder_name": side, "is_installed": true},
            {"app_name": "s2", "title": "Side Two", "folder_name": side, "is_installed": false}
        ]})
        .to_string(),
    );
    let mut errors = vec![];
    let mut found = vec![];
    for runner in heroic::Runner::ALL {
        for game in heroic::games(&root, runner, &mut errors) {
            let mut game = game;
            if !Path::new(&game.install_dir).is_dir() {
                continue;
            }
            game.cover = game.cover.take();
            found.push(game);
        }
    }
    assert!(errors.is_empty(), "{errors:?}");
    let by_name = |name: &str| found.iter().find(|game| game.name == name).unwrap();
    assert_eq!(
        found.len(),
        4,
        "{:?}",
        found.iter().map(|g| &g.name).collect::<Vec<_>>()
    );
    assert_eq!(by_name("Epic One").source, Source::Epic);
    assert_eq!(by_name("Gog One").source, Source::Gog);
    assert_eq!(by_name("Amazon One").source, Source::Local);
    assert_eq!(by_name("Amazon One").id, "heroic-nile:n1");
    assert_eq!(
        by_name("Gog One").launch,
        Some(Launch::Uri {
            uri: "heroic://launch/gog/g1".into()
        })
    );
    assert!(by_name("Gog One").cover.is_some());
    assert!(by_name("Epic One").cover.is_none());
}

#[test]
fn a_malformed_heroic_file_is_an_error_line_not_a_panic() {
    let scratch = Scratch::new("heroic-bad");
    let root = scratch.dir("heroic");
    scratch.write("heroic/gog_store/installed.json", "{ not json");
    scratch.write("heroic/legendaryConfig/legendary/installed.json", "[1, 2]");
    let mut errors = vec![];
    assert!(heroic::games(&root, heroic::Runner::Gog, &mut errors).is_empty());
    assert!(heroic::games(&root, heroic::Runner::Legendary, &mut errors).is_empty());
    assert_eq!(errors.len(), 2);
    // Missing files are no error.
    let mut quiet = vec![];
    assert!(heroic::games(&root, heroic::Runner::Nile, &mut quiet).is_empty());
    assert!(quiet.is_empty());
}

#[test]
fn lutris_lists_installed_visible_games_that_do_not_run_through_steam() {
    let scratch = Scratch::new("lutris");
    let root = scratch.dir("lutris");
    std::fs::write(root.join("pga.db"), include_bytes!("fixtures/pga.db")).unwrap();
    scratch.write("lutris/coverart/quake.jpg", "image");
    scratch.write("lutris/banners/no-folder.png", "image");
    let games = lutris::games(&root).unwrap();
    let mut names: Vec<&str> = games.iter().map(|game| game.name.as_str()).collect();
    names.sort_by_key(|name| name.len());
    assert_eq!(games.len(), 3, "{names:?}");
    let quake = games.iter().find(|game| game.name == "Quake").unwrap();
    assert_eq!(quake.id, "lutris:1");
    assert_eq!(quake.source, Source::Local);
    assert_eq!(
        quake.launch,
        Some(Launch::Uri {
            uri: "lutris:rungameid/1".into()
        })
    );
    assert!(
        quake
            .cover
            .as_ref()
            .unwrap()
            .ends_with("coverart/quake.jpg")
    );
    let none = games.iter().find(|game| game.name == "No Folder").unwrap();
    assert!(
        none.cover
            .as_ref()
            .unwrap()
            .ends_with("banners/no-folder.png")
    );
    assert!(
        games
            .iter()
            .any(|game| game.name.starts_with("Overflow ") && game.name.len() > 2000)
    );
}

#[test]
fn a_database_that_is_not_sqlite_is_an_error() {
    assert!(sqlite::rows(b"hello", "games").is_err());
    let mut bytes = include_bytes!("fixtures/pga.db").to_vec();
    assert!(sqlite::rows(&bytes, "missing").is_err());
    bytes.truncate(1000);
    assert!(sqlite::rows(&bytes, "games").is_err());
    let scratch = Scratch::new("lutris-bad");
    let root = scratch.dir("lutris");
    scratch.write("lutris/pga.db", "garbage");
    assert!(lutris::games(&root).is_err());
}

#[test]
fn the_sqlite_reader_reads_all_rows_of_a_multi_page_table() {
    let rows = sqlite::rows(include_bytes!("fixtures/pga.db"), "games").unwrap();
    assert_eq!(rows.len(), 86);
    assert_eq!(rows[0]["id"], sqlite::Value::Int(1));
    assert_eq!(rows[85]["id"], sqlite::Value::Int(86));
    assert_eq!(rows[1]["name"].text(), Some("Hidden Game"));
    assert_eq!(rows[0]["lastplayed"].int(), Some(5));
}

#[test]
fn discover_marks_disabled_sources_and_never_panics_without_launchers() {
    let discovery = discover(&library::Settings::default());
    assert_eq!(discovery.sources.len(), Source::ALL.len());
}

/// Reads this PC's real launcher installs; run by hand with `-- --ignored --nocapture`.
#[test]
#[ignore]
fn real_installs() {
    let discovery = discover(&library::Settings::default());
    for status in &discovery.sources {
        let count = discovery
            .games
            .iter()
            .filter(|game| game.source == status.source)
            .count();
        println!(
            "{:<10} {:<12} {count:>3} games {:?}",
            status.name, status.state, status.errors
        );
    }
    for game in discovery.games.iter().take(400) {
        println!(
            "{:?} {} cover={}",
            game.source,
            game.name,
            game.cover.is_some()
        );
    }
}
