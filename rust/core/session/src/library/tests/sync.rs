//! Tests for launcher sync settings, exclusions, recent games, covers, and ownership.
use super::super::stores::base64;
use super::*;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[test]
fn every_launcher_is_on_and_synced_by_default_like_steam() {
    let defaults = Settings::default();
    for source in [
        Source::Epic,
        Source::Battlenet,
        Source::Ea,
        Source::Ubisoft,
        Source::Gog,
        Source::Microsoft,
    ] {
        assert!(defaults.enabled(source), "{source:?}");
    }
    // Steam's defaults for every launcher and the game folders.
    let steam = crate::steam::Settings::from_config(&butterpollo_config::config::Config::default());
    for source in SYNCED {
        let options = defaults.options(source).unwrap();
        assert_eq!(options.auto_sync, steam.auto_sync, "{source:?}");
        assert_eq!(
            options.remove_uninstalled, steam.remove_uninstalled,
            "{source:?}"
        );
        assert_eq!(
            options.sync_all_installed, steam.sync_all_installed,
            "{source:?}"
        );
        assert_eq!(options.recent_games, steam.recent_games, "{source:?}");
        assert_eq!(
            options.recent_max_age_days, steam.recent_max_age_days,
            "{source:?}"
        );
        assert!(options.exclusions.is_empty(), "{source:?}");
    }
    assert!(
        defaults.options(Source::Steam).is_none(),
        "Steam keeps its own sync"
    );
    assert!(defaults.any_automatic());
    let off = sync_settings(
        "library_battlenet_enabled = false\nlibrary_ea_enabled = false\nlibrary_ubisoft_enabled = false\nlibrary_microsoft_enabled = false\nlibrary_auto_sync = false\n",
    );
    for source in [
        Source::Battlenet,
        Source::Ea,
        Source::Ubisoft,
        Source::Microsoft,
    ] {
        assert!(!off.enabled(source), "{source:?}");
    }
    assert!(off.enabled(Source::Epic) && !off.options(Source::Epic).unwrap().auto_sync);
    assert!(!off.any_automatic());
    // Off for one launcher only: the automatic sync skips it and keeps the rest.
    let gog_by_hand = sync_settings("gog_auto_sync = false\n");
    let automatic = gog_by_hand.automatic();
    assert!(
        gog_by_hand.enabled(Source::Gog),
        "Sync now and Add games still read it"
    );
    assert!(!automatic.options(Source::Gog).unwrap().enabled);
    assert!(automatic.options(Source::Epic).unwrap().enabled);
    // Only Battle.net keeps a play history: the others always add every installed game.
    let recent = sync_settings(
        "battlenet_sync_all_installed = false\nepic_sync_all_installed = false\nbattlenet_recent_games = 3\n",
    );
    assert!(
        !recent
            .options(Source::Battlenet)
            .unwrap()
            .sync_all_installed
    );
    assert_eq!(recent.options(Source::Battlenet).unwrap().recent_games, 3);
    assert!(recent.options(Source::Epic).unwrap().sync_all_installed);
}

#[test]
fn a_launchers_own_sync_key_overrides_the_shared_library_sync() {
    // The shared Library sync value applies to every launcher; one whose own
    // key of earlier releases is still set reads that instead.
    let settings = sync_settings("library_auto_sync = false\ngog_auto_sync = true\n");
    assert!(!settings.options(Source::Epic).unwrap().auto_sync);
    assert!(settings.options(Source::Gog).unwrap().auto_sync);
    // Steam and Playnite read the shared value through their own settings.
    let steam = crate::steam::Settings::from_config(
        &butterpollo_config::config::Config::parse("library_auto_sync = false\n").unwrap(),
    );
    assert!(!steam.auto_sync);
    let playnite = crate::playnite::Settings::from_config(
        &butterpollo_config::config::Config::parse(
            "library_auto_sync = false\nplaynite_auto_sync = true\n",
        )
        .unwrap(),
    );
    assert!(playnite.auto_sync);
    // The recent-games options are shared too, and the shared remove switch
    // reaches the launchers.
    let recent = sync_settings(
        "library_sync_all_installed = false\nlibrary_recent_games = 2\nlibrary_recent_max_age_days = 0\nlibrary_autosync_remove_uninstalled = false\n",
    );
    let options = recent.options(Source::Battlenet).unwrap();
    assert!(!options.sync_all_installed);
    assert_eq!(options.recent_games, 2);
    assert_eq!(options.recent_max_age_days, 0);
    assert!(!options.remove_uninstalled);
    assert!(!recent.options(Source::Epic).unwrap().remove_uninstalled);
}

#[test]
fn a_launchers_games_to_leave_out_are_never_added_and_their_synced_apps_go() {
    let settings = sync_settings(
        "ubisoft_exclude_games = [{\"name\": \"MIRAGE\"}]\nepic_exclude_games = [\"namespace-1:item-2:Orion\"]\n",
    );
    let ubisoft = settings.options(Source::Ubisoft).unwrap();
    assert!(
        ubisoft.excludes("13504", "Mirage"),
        "by name, as names compare"
    );
    assert!(!ubisoft.excludes("13504", "Mirage 2"));
    // A plain entry is an id or a name.
    let plain = sync_settings("gog_exclude_games = [\"1207658924\", \"The Witcher\"]\n");
    let gog = plain.options(Source::Gog).unwrap();
    assert!(gog.excludes("1207658924", "Anything"));
    assert!(gog.excludes("1", "The Witcher™"));
    assert!(!gog.excludes("1", "The Witcher 2"));
    let games = vec![epic_game(), mirage()];
    let mut apps = vec![];
    assert_eq!(
        reconcile(&mut apps, &games, &settings, &Source::ALL, &[]).added,
        0,
        "{apps:?}"
    );
    // Synced before they were left out: they leave, whatever the read; an import stays.
    let mut apps = vec![];
    reconcile(&mut apps, &games, &Settings::default(), &Source::ALL, &[]);
    let mut kept = mirage();
    kept.id = "1".into();
    kept.name = "Mirage".into();
    let mut imported = vec![];
    import(&mut imported, &[kept.clone()], &[selected(&kept)]);
    apps.extend(imported);
    assert_eq!(apps.len(), 3);
    let change = reconcile(&mut apps, &games, &settings, &[], &[]);
    assert_eq!(change.removed, 2);
    assert_eq!(apps.len(), 1);
    assert_eq!(apps[0]["launcher-id"], "1");
}

#[test]
fn a_launcher_with_a_play_history_can_keep_only_its_recent_games_as_steam_does() {
    const DAY: u64 = 86_400;
    let now = 1_800_000_000;
    let game = |uid: &str, played: Option<u64>| {
        let mut game = battlenet(
            uid,
            "",
            Path::new(&format!("C:/Games/{uid}")),
            Some(Path::new("C:/Battle.net/Battle.net.exe")),
        )
        .unwrap();
        game.last_played = played;
        game
    };
    let games = vec![
        game("wow", Some(now - DAY)),
        game("fen", Some(now - 2 * DAY)),
        game("odin", Some(now - 40 * DAY)),
        game("w3", None),
    ];
    let recent =
        sync_settings("battlenet_sync_all_installed = false\nbattlenet_recent_games = 1\n");
    let mut apps = vec![];
    let change = reconcile_at(&mut apps, &games, &recent, &Source::ALL, &[], now);
    assert_eq!(change.added, 1);
    assert_eq!(apps[0]["name"], "World of Warcraft");
    // Two, played within 30 days: Diablo IV joins; Modern Warfare is too old, Warcraft III never played.
    let two = sync_settings("battlenet_sync_all_installed = false\nbattlenet_recent_games = 2\n");
    assert_eq!(
        reconcile_at(&mut apps, &games, &two, &Source::ALL, &[], now).added,
        1
    );
    let any_age = sync_settings(
        "battlenet_sync_all_installed = false\nbattlenet_recent_games = 5\nbattlenet_recent_max_age_days = 0\n",
    );
    assert_eq!(chosen(&games, &any_age, now).len(), 3);
    // Fallen out of the list: the synced app leaves even with Remove uninstalled games off, as Steam's.
    let one_kept = sync_settings(
        "battlenet_sync_all_installed = false\nbattlenet_recent_games = 1\nbattlenet_autosync_remove_uninstalled = false\n",
    );
    let change = reconcile_at(&mut apps, &games, &one_kept, &Source::ALL, &[], now);
    assert_eq!(change.removed, 1);
    assert_eq!(apps.len(), 1);
    // None at all: every recent app leaves.
    let none = sync_settings("battlenet_sync_all_installed = false\nbattlenet_recent_games = 0\n");
    assert_eq!(
        reconcile_at(&mut apps, &games, &none, &Source::ALL, &[], now).removed,
        1
    );
    // Every installed game, the default: all four.
    assert_eq!(
        reconcile_at(
            &mut apps,
            &games,
            &Settings::default(),
            &Source::ALL,
            &[],
            now
        )
        .added,
        4
    );
}

#[test]
fn battlenet_play_history_and_epic_box_art_are_read_from_the_launchers_files() {
    let config = br#"{"Client": {}, "Games": {"battle_net": {"Resumable": "false"},
        "W3": {"LastPlayed": "1785312857"}, "odin": {"LastPlayed": 1767294556}, "fen": {"LastPlayed": "0"}}}"#;
    let mut with_bom = b"\xef\xbb\xbf".to_vec();
    with_bom.extend_from_slice(config);
    let played = battlenet_last_played(&with_bom).unwrap();
    assert_eq!(played.len(), 2, "{played:?}");
    assert_eq!(played["w3"], 1_785_312_857);
    assert_eq!(played["odin"], 1_767_294_556);
    assert!(battlenet_last_played(b"not json").is_err());
    // catcache.bin: base64 of the catalog items, as the launcher writes it.
    let items = json!([
        {"namespace": "namespace-1", "id": "item-2", "keyImages": [
            {"type": "DieselGameBox", "url": "https://cdn1.epicgames.com/wide.jpg"},
            {"type": "DieselGameBoxTall", "url": "https://cdn1.epicgames.com/item/tall_1200x1600.jpg"}]},
        {"namespace": "other", "id": "offer", "keyImages": [
            {"type": "OfferImageTall", "url": "https://cdn1.epicgames.com/offer.png"}]},
        {"namespace": "evil", "id": "1", "keyImages": [
            {"type": "DieselGameBoxTall", "url": "https://cdn1.epicgames.com.example.org/x.jpg"}]},
        {"namespace": "evil", "id": "2", "keyImages": [
            {"type": "DieselGameBoxTall", "url": "http://cdn1.epicgames.com/x.jpg"}]},
        {"namespace": "evil", "id": "3", "keyImages": [
            {"type": "DieselGameBoxTall", "url": "https://user@cdn1.epicgames.com/x.jpg"}]},
        {"namespace": "none", "id": "4"},
    ]);
    let encoded = base64_for_test(items.to_string().as_bytes());
    let covers = epic_covers(encoded.as_bytes()).unwrap();
    assert_eq!(covers.len(), 2, "{covers:?}");
    let game = epic_game();
    let key = epic_cover_key(&game.id).unwrap();
    assert_eq!(key, "namespace-1:item-2");
    assert_eq!(
        covers[&key],
        "https://cdn1.epicgames.com/item/tall_1200x1600.jpg"
    );
    assert_eq!(
        covers["other:offer"],
        "https://cdn1.epicgames.com/offer.png"
    );
    assert_eq!(epic_cover_key("no-app"), None);
    assert!(epic_covers(b"not base64!").is_err());
    // Every length decodes, with and without padding and line breaks.
    for length in 0..8 {
        let bytes: Vec<u8> = (0..length).map(|at| (at * 37 % 256) as u8).collect();
        let text = base64_for_test(&bytes);
        assert_eq!(base64(text.as_bytes()).unwrap(), bytes, "{text}");
        assert_eq!(
            base64(text.trim_end_matches('=').as_bytes()).unwrap(),
            bytes
        );
        let wrapped = format!("{}\r\n{}", &text[..text.len() / 2], &text[text.len() / 2..]);
        assert_eq!(base64(wrapped.as_bytes()).unwrap(), bytes);
    }
}
/// Standard base64 with padding, to write what the Epic launcher writes.
fn base64_for_test(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut text = String::new();
    for chunk in bytes.chunks(3) {
        let value = chunk.iter().enumerate().fold(0u32, |value, (at, byte)| {
            value | (u32::from(*byte) << (16 - 8 * at))
        });
        for at in 0..4 {
            if at <= chunk.len() {
                text.push(ALPHABET[((value >> (18 - 6 * at)) & 63) as usize] as char);
            } else {
                text.push('=');
            }
        }
    }
    text
}

#[test]
fn an_app_of_a_launchers_game_without_a_cover_gets_the_launchers_and_a_users_cover_stays() {
    let mut covered = mirage();
    covered.cover = Some(PathBuf::from(r"C:\Profile\covers\launcher_mirage.png"));
    let mut apps = vec![];
    // Synced before covers were read.
    reconcile(
        &mut apps,
        &[mirage()],
        &Settings::default(),
        &Source::ALL,
        &[],
    );
    assert!(apps[0].get("image-path").is_none());
    let mut imported_game = mirage();
    imported_game.id = "2".into();
    imported_game.name = "Imported".into();
    import(
        &mut apps,
        &[imported_game.clone()],
        &[selected(&imported_game)],
    );
    apps.push(json!({"name": "Mine", "launcher-source": "ubisoft", "launcher-id": "3", "image-path": "D:/mine.png"}));
    let mut mine = mirage();
    mine.id = "3".into();
    mine.name = "Mine".into();
    mine.cover = covered.cover.clone();
    let mut imported_cover = imported_game.clone();
    imported_cover.cover = covered.cover.clone();
    let change = reconcile(
        &mut apps,
        &[covered, imported_cover, mine],
        &Settings::default(),
        &Source::ALL,
        &[],
    );
    assert_eq!(change.covered, 2);
    assert_eq!(
        apps[0]["image-path"],
        "C:/Profile/covers/launcher_mirage.png"
    );
    assert_eq!(
        apps[1]["image-path"],
        "C:/Profile/covers/launcher_mirage.png"
    );
    assert_eq!(apps[2]["image-path"], "D:/mine.png");
    // Once: nothing more to fill.
    assert!(!reconcile(&mut apps, &[], &Settings::default(), &[], &[]).changed());
}

#[test]
fn the_launcher_sync_adds_installed_games_once_and_leaves_steam_imports_and_dismissed_games() {
    let settings = Settings::default();
    let epic = epic_game();
    let mut broken = mirage();
    broken.id = "999".into();
    broken.name = "Broken".into();
    broken.unavailable_reason = Some("Install or repair Ubisoft Connect.".into());
    let mut dismissed = mirage();
    dismissed.id = "777".into();
    dismissed.name = "Deleted by the user".into();
    let steam = Game::new(
        Source::Steam,
        "570".into(),
        "Dota 2".into(),
        Path::new("C:/Steam/dota"),
        Some(Launch::Uri {
            uri: "steam://rungameid/570".into(),
        }),
    );
    let games = vec![epic.clone(), mirage(), broken, dismissed, steam];
    let mut apps = vec![json!({"name": "Desktop", "uuid": "d"})];
    let change = reconcile(
        &mut apps,
        &games,
        &settings,
        &Source::ALL,
        &["ubisoft:777".into()],
    );
    assert_eq!(
        change,
        SyncChange {
            added: 2,
            ..SyncChange::default()
        }
    );
    let synced: Vec<&str> = apps
        .iter()
        .filter_map(|app| managed_key(app).map(|_| app["name"].as_str().unwrap()))
        .collect();
    assert_eq!(synced, [epic.name.as_str(), "Mirage"]);
    assert!(apps.iter().all(|app| app["launcher-source"] != "steam"));
    // Idempotent.
    assert_eq!(
        reconcile(
            &mut apps,
            &games,
            &settings,
            &Source::ALL,
            &["ubisoft:777".into()]
        ),
        SyncChange::default()
    );
    // A game imported by hand is not synced twice.
    let mut manual = vec![];
    import(&mut manual, &games, &[selected(&epic)]);
    assert_eq!(
        reconcile(&mut manual, &games[..1], &settings, &Source::ALL, &[]),
        SyncChange::default()
    );
    // Not synced automatically: the automatic sync changes nothing; Sync now adds the games,
    // as Steam's does.
    let off = sync_settings("library_auto_sync = false\n");
    let mut none = vec![];
    assert!(
        !reconcile(&mut none, &games, &off.automatic(), &Source::ALL, &[]).changed()
            && none.is_empty()
    );
    assert_eq!(
        reconcile(
            &mut none,
            &games,
            &off,
            &Source::ALL,
            &["ubisoft:777".into()]
        )
        .added,
        2
    );
    // A launcher that is off syncs nothing.
    let no_ubisoft = sync_settings("library_ubisoft_enabled = false\n");
    let mut epic_only = vec![];
    reconcile(&mut epic_only, &games, &no_ubisoft, &Source::ALL, &[]);
    assert_eq!(epic_only.len(), 1);
}

#[test]
fn an_uninstalled_synced_game_leaves_only_after_a_clean_read_and_manual_apps_stay() {
    let settings = Settings::default();
    let games = vec![epic_game(), mirage()];
    let mut apps = vec![];
    reconcile(&mut apps, &games, &settings, &Source::ALL, &[]);
    let mut kept = mirage();
    kept.id = "1".into();
    kept.name = "Imported by hand".into();
    import(&mut apps, &[kept.clone()], &[selected(&kept)]);
    assert_eq!(apps.len(), 3);
    // Ubisoft's read failed: its games stay although the scan has none.
    let read: Vec<Source> = Source::ALL
        .into_iter()
        .filter(|s| *s != Source::Ubisoft)
        .collect();
    assert_eq!(
        reconcile(&mut apps, &games[..1], &settings, &read, &[]).removed,
        0
    );
    // A clean read without Mirage removes the synced copy, never the import.
    let change = reconcile(&mut apps, &games[..1], &settings, &Source::ALL, &[]);
    assert_eq!(
        change,
        SyncChange {
            removed: 1,
            ..SyncChange::default()
        }
    );
    assert!(apps.iter().any(|app| app["name"] == "Imported by hand"));
    assert!(!apps.iter().any(|app| app["name"] == "Mirage"));
    // Without remove-uninstalled the synced copy stays.
    let keep = sync_settings("library_autosync_remove_uninstalled = false\n");
    let mut apps = vec![];
    reconcile(&mut apps, &games, &keep, &Source::ALL, &[]);
    assert_eq!(
        reconcile(&mut apps, &[], &keep, &Source::ALL, &[]).removed,
        0
    );
    assert_eq!(apps.len(), 2);
}

#[test]
fn the_ownership_stamp_runs_once_and_takes_steam_apps_only_after_the_old_opt_in() {
    let document = || {
        json!({"apps": [
            {"name": "Vibepollo's Portal", "steam-id": "620", "steam-managed": "auto"},
            {"name": "Mirage", "launcher-source": "ubisoft", "launcher-id": "13504", "launcher-managed": "auto"},
            {"name": "Kart", "playnite-id": "aa", "playnite-managed": "auto"},
            {"name": "Imported", "steam-id": "570", "steam-managed": "manual"},
            {"name": "Desktop"},
        ]})
    };
    let mut without = document();
    assert_eq!(
        stamp_synced(&mut without, false),
        Some(Stamped {
            steam: 0,
            launcher: 1,
            playnite: 1
        })
    );
    let stamps: Vec<&Value> = without["apps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|app| &app[SYNCED_BY])
        .collect();
    assert_eq!(
        stamps,
        [
            &Value::Null,
            &json!("launcher"),
            &json!("playnite"),
            &Value::Null,
            &Value::Null
        ]
    );
    assert_eq!(without[SYNC_OWNER_VERSION], 1);
    // Once: a later start stamps nothing, even an app that lost its stamp.
    let ran = without.clone();
    assert_eq!(stamp_synced(&mut without, true), None);
    assert_eq!(without, ran);
    let mut opted_in = document();
    assert_eq!(stamp_synced(&mut opted_in, true).unwrap().steam, 1);
    assert_eq!(opted_in["apps"][0][SYNCED_BY], "steam");
    assert_eq!(opted_in["apps"][3].get(SYNCED_BY), None);
}

#[test]
fn a_handed_over_app_loses_its_stamp_and_the_launcher_sync_leaves_it() {
    let settings = Settings::default();
    let games = vec![mirage()];
    let mut apps = vec![];
    reconcile(&mut apps, &games, &settings, &Source::ALL, &[]);
    assert_eq!(apps[0][SYNCED_BY], "launcher");
    hand_over(&mut apps[0], "launcher");
    assert_eq!(apps[0].get(SYNCED_BY), None);
    assert_eq!(apps[0][MANAGED], "manual");
    // Uninstalled: the user's app stays, and the game is not added a second time.
    assert_eq!(
        reconcile(&mut apps, &[], &settings, &Source::ALL, &[]),
        SyncChange::default()
    );
    assert_eq!(
        reconcile(&mut apps, &games, &settings, &Source::ALL, &[]),
        SyncChange::default()
    );
    assert_eq!(apps.len(), 1);
    // An unstamped `launcher-managed: auto` app is the user's as well.
    let mut theirs = vec![json!({"name": "Mirage", "launcher-source": "ubisoft",
        "launcher-id": "13504", "launcher-managed": "auto"})];
    assert!(!reconcile(&mut theirs, &[], &settings, &Source::ALL, &[]).changed());
    assert_eq!(theirs.len(), 1);
    assert_eq!(dismissal_keys(&theirs[0]), ["ubisoft:13504"]);
}
