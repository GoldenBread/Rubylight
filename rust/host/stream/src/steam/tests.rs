//! Unit tests for host Steam library sync and cover caching.

use super::*;
use crate::state::test_support::Fixture;
use butterpollo_config::config::Config;

/// A 600x900 portrait in the profile, so a sync needs no store download.
fn portrait(f: &Fixture) -> PathBuf {
    let cover = f.host.directory.join("portrait.bmp");
    // An uncompressed 600x900 BGR bitmap avoids both codec/GPU dependencies and store downloads.
    let mut bmp = vec![0u8; 54 + 600 * 900 * 3];
    let size = bmp.len() as u32;
    bmp[..2].copy_from_slice(b"BM");
    bmp[2..6].copy_from_slice(&size.to_le_bytes());
    bmp[10..14].copy_from_slice(&54u32.to_le_bytes());
    bmp[14..18].copy_from_slice(&40u32.to_le_bytes());
    bmp[18..22].copy_from_slice(&600u32.to_le_bytes());
    bmp[22..26].copy_from_slice(&900u32.to_le_bytes());
    bmp[26..28].copy_from_slice(&1u16.to_le_bytes());
    bmp[28..30].copy_from_slice(&24u16.to_le_bytes());
    std::fs::write(&cover, bmp).unwrap();
    cover
}

#[test]
fn a_portrait_steam_does_not_have_is_asked_for_less_often() {
    // A marker from before the count was kept, or one that is not a number, is one failure.
    assert_eq!(portrait_failures(b""), 1);
    assert_eq!(portrait_failures(b"soon"), 1);
    assert_eq!(portrait_failures(b"3"), 3);
    assert_eq!(portrait_failures(b"2\r\n"), 2);
    assert_eq!(portrait_retry_after(1), Duration::from_secs(600));
    assert_eq!(portrait_retry_after(2), Duration::from_secs(3600));
    assert_eq!(portrait_retry_after(3), Duration::from_secs(86_400));
    assert_eq!(portrait_retry_after(u32::MAX), Duration::from_secs(86_400));
}

#[test]
fn steam_sync_stands_aside_while_playnite_syncs_the_library() {
    let f = Fixture::new();
    let games = [Game {
        appid: 570,
        name: "Dota 2".into(),
        app_type: "Game".into(),
        installed: true,
        portrait_path: Some(portrait(&f)),
        ..Default::default()
    }];
    let settings = Settings::from_config(&Config::parse("steam_enabled=true").unwrap());
    let before = f.host.apps.read().unwrap().len();
    let aside = sync_beside(&f.host, &settings, &games, true).unwrap();
    assert!(!aside.changed);
    assert_eq!((aside.games, aside.importable), (1, 1));
    assert_eq!(f.host.apps.read().unwrap().len(), before);
    assert!(
        sync_beside(&f.host, &settings, &games, false)
            .unwrap()
            .changed
    );
    assert_eq!(f.host.apps.read().unwrap().len(), before + 1);
}
#[test]
fn steam_sync_converts_local_covers_persists_apps_and_is_idempotent() {
    let f = Fixture::new();
    let cover = portrait(&f);
    let games = [Game {
        appid: 570,
        name: "Fixture game".into(),
        app_type: "Game".into(),
        installed: true,
        portrait_path: Some(cover),
        ..Default::default()
    }];
    let mut settings = Settings::from_config(
        &Config::parse("steam_enabled=true\nsteam_sync_all_installed=true").unwrap(),
    );
    let original = f.host.apps.read().unwrap().len();
    let first = sync_catalog(&f.host, &settings, &games).unwrap();
    assert!(first.changed);
    assert_eq!((first.games, first.importable), (1, 1));
    let saved = butterpollo_session::state::load_json(&f.host.apps_path, json!({})).unwrap();
    assert_eq!(saved["apps"].as_array().unwrap().len(), original + 1);
    let app = saved["apps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|app| app["steam-id"] == "570")
        .unwrap();
    let png = Path::new(app["image-path"].as_str().unwrap());
    assert_eq!(&std::fs::read(png).unwrap()[..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(
        butterpollo_win_graphics::image::dimensions(png).unwrap(),
        (600, 900)
    );
    assert!(!sync_catalog(&f.host, &settings, &games).unwrap().changed);
    assert_eq!(
        butterpollo_session::state::load_json(&f.host.apps_path, json!({})).unwrap(),
        saved
    );
    settings.exclusions.push(("570".into(), String::new()));
    assert!(sync_catalog(&f.host, &settings, &games).unwrap().changed);
    assert_eq!(f.host.apps.read().unwrap().len(), original);
}

#[test]
fn a_deleted_steam_game_stays_out_of_the_sync_until_add_games_brings_it_back() {
    use butterpollo_session::library::{self, Launch, Selection, Source};
    let f = Fixture::new();
    let games = [Game {
        appid: 570,
        name: "Dota 2".into(),
        app_type: "Game".into(),
        installed: true,
        portrait_path: Some(portrait(&f)),
        ..Default::default()
    }];
    let settings = Settings::from_config(&Config::parse("steam_sync_all_installed=true").unwrap());
    assert!(sync_catalog(&f.host, &settings, &games).unwrap().changed);
    let dota = |h: &Shared| {
        h.apps
            .read()
            .unwrap()
            .iter()
            .map(|app| serde_json::to_value(app).unwrap())
            .find(|app| steam::game_of(app) == Some(570))
    };
    let app = dota(&f.host).unwrap();
    assert_eq!(app["synced-by"], "steam");
    // Deleted from the library, as the console's delete does.
    update_library(&f.host, |apps, document| {
        apps.retain(|app| steam::game_of(app) != Some(570));
        crate::library::dismiss(document, &app);
        true
    })
    .unwrap();
    assert_eq!(
        f.host.app_document.read().unwrap()["launcher-dismissed"],
        json!(["steam:570"])
    );
    assert!(!sync_catalog(&f.host, &settings, &games).unwrap().changed);
    assert!(dota(&f.host).is_none());
    // "Add games" brings it back as the user's app and lifts the deletion.
    let game = library::Game::new(
        Source::Steam,
        "570".into(),
        "Dota 2".into(),
        Path::new("C:/Steam/steamapps/common/dota 2 beta"),
        Some(Launch::Uri {
            uri: "steam://rungameid/570".into(),
        }),
    );
    let selection = Selection {
        source: Source::Steam,
        id: "570".into(),
    };
    crate::library::import_games(&f.host, vec![game], &[selection]).unwrap();
    let app = dota(&f.host).unwrap();
    assert_eq!(app["steam-managed"], "manual");
    assert_eq!(app.get("synced-by"), None);
    assert_eq!(
        f.host.app_document.read().unwrap()["launcher-dismissed"],
        json!([])
    );
    // The sync keeps the game once, as the user's: it only fills in the missing cover.
    assert!(sync_catalog(&f.host, &settings, &games).unwrap().changed);
    let app = dota(&f.host).unwrap();
    assert!(
        app["image-path"]
            .as_str()
            .unwrap()
            .ends_with("steam_570.png")
    );
    assert_eq!(app.get("synced-by"), None);
    let copies = f
        .host
        .apps
        .read()
        .unwrap()
        .iter()
        .filter(|app| steam::game_of(&serde_json::to_value(app).unwrap()) == Some(570))
        .count();
    assert_eq!(copies, 1);
    assert!(!sync_catalog(&f.host, &settings, &games).unwrap().changed);
}

#[test]
fn steam_watch_fingerprint_detects_manifest_and_settings_changes_without_waiting() {
    let f = Fixture::new();
    let library = f.host.directory.join("steamapps");
    std::fs::create_dir(&library).unwrap();
    let manifest = library.join("appmanifest_570.acf");
    std::fs::write(&manifest, "first").unwrap();
    let roots = [f.host.directory.clone()];
    let mut settings = Settings::from_config(&Config::default());
    let first = fingerprint(&settings, &roots);
    assert_eq!(fingerprint(&settings, &roots), first);
    std::fs::write(&manifest, "changed size").unwrap();
    let changed = fingerprint(&settings, &roots);
    assert_ne!(changed, first);
    settings.sync_all_installed = !settings.sync_all_installed;
    assert_ne!(fingerprint(&settings, &roots), changed);
}

#[test]
fn a_library_folder_that_appears_later_is_listed_with_the_vdf_unchanged() {
    let f = Fixture::new();
    let apps = f.host.directory.join("steamapps");
    std::fs::create_dir(&apps).unwrap();
    let missing = f.host.directory.join("Games");
    std::fs::write(
        apps.join("libraryfolders.vdf"),
        format!(
            "\"libraryfolders\" {{\n\"0\" {{ \"path\" \"{}\" }}\n}}",
            missing.display().to_string().replace('\\', "\\\\")
        ),
    )
    .unwrap();
    assert!(vdf_libraries(&apps).is_empty());
    // The drive appears; the file is not rewritten.
    std::fs::create_dir_all(missing.join("steamapps")).unwrap();
    assert_eq!(vdf_libraries(&apps), [missing.join("steamapps")]);
}
