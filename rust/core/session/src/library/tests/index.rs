//! Tests for AppIndex equivalence and indexed launcher reconciliation ordering.
use super::*;
use serde_json::{Value, json};
use std::path::Path;

fn program(path: &str) -> Option<Launch> {
    Some(Launch::Program {
        program: path.into(),
        arguments: String::new(),
    })
}
/// Apps every kind of [`Game::matches`] finds, near misses and apps it never finds.
fn indexed_apps() -> Vec<Value> {
    vec![
        json!({"name": "Desktop", "cmd": ""}),
        json!({"name": "The Witcher 3", "steam-id": "292030"}),
        json!({"name": "ÉTOILE", "playnite-id": "p1", "playnite-plugin-name": "Epic Library"}),
        json!({"name": "Étoile", "playnite-id": "p2", "playnite-plugin-name": "GOG Library"}),
        json!({"name": "Mass Effect", "playnite-id": "p3", "playnite-plugin-name": "Origin"}),
        json!({"name": "Celeste (copy)", "working-dir": "d:/GAMES/celeste/"}),
        json!({"name": "Hades tools", "cmd": "\"D:\\Games\\Hades\\x64\\Hades.exe\" -dx12"}),
        json!({"name": "Climber", "working-dir": "D:\\Games\\..\\Hades"}),
        json!({"name": "Mods", "launcher-install-dir": "D:\\Games\\The Witcher 3 Mods"}),
        json!({"name": "Dota", "cmd": "steam://rungameid/570"}),
        json!({"name": "Mirage", "launcher-source": "ubisoft", "launcher-id": "13504"}),
        json!({"name": "Root", "working-dir": "D:\\"}),
        json!({"name": "Hades", "playnite-id": "p4", "working-dir": "E:\\Elsewhere"}),
        Value::Null,
        json!(["not", "an", "app"]),
        json!({"uuid": epic_game().uuid()}),
    ]
}
/// Games of every launcher, and games of no name or folder.
fn indexed_games() -> Vec<Game> {
    let mut games = vec![epic_game(), mirage()];
    games.extend(local_games());
    games.extend([
        Game::new(
            Source::Ea,
            "mass-effect".into(),
            "Mass Effect".into(),
            Path::new(r"C:\EA\Mass Effect"),
            program(r"C:\EA\Mass Effect\MassEffect.exe"),
        ),
        Game::new(
            Source::Ea,
            "broken".into(),
            "Broken".into(),
            Path::new(r"C:\EA\Broken"),
            None,
        ),
        Game::new(
            Source::Gog,
            "1".into(),
            "Étoile".into(),
            Path::new(r"C:\GOG\Etoile"),
            program(r"C:\GOG\Etoile\Etoile.exe"),
        ),
        Game::new(
            Source::Steam,
            "570".into(),
            "Dota 2".into(),
            Path::new(r"F:\Steam\steamapps\common\dota 2 beta"),
            None,
        ),
        Game::new(
            Source::Steam,
            "292030".into(),
            "The Witcher 3".into(),
            Path::new(r"F:\Steam\steamapps\common\The Witcher 3"),
            None,
        ),
        Game::new(
            Source::Local,
            r"d:\games".into(),
            "Games".into(),
            Path::new("D:/Games/"),
            program(r"D:\Games\games.exe"),
        ),
        Game::new(
            Source::Local,
            r"d:\".into(),
            "Root".into(),
            Path::new(r"D:\"),
            None,
        ),
        Game::new(
            Source::Epic,
            "nameless".into(),
            "!!!".into(),
            Path::new(""),
            program(r"C:\Epic\nameless.exe"),
        ),
    ]);
    games
}

#[test]
fn the_app_index_finds_what_trying_every_app_finds() {
    let apps = indexed_apps();
    let games = indexed_games();
    let (mut found, mut missed) = (0, 0);
    // Every list of apps from none to all, as the sync grows it.
    for end in 0..=apps.len() {
        let apps = &apps[..end];
        let index = AppIndex::new(apps);
        for game in &games {
            let expected = apps.iter().any(|app| game.matches(app));
            assert_eq!(
                index.matches(apps, game),
                expected,
                "{} among {end} apps",
                game.name
            );
            found += usize::from(expected);
            missed += usize::from(!expected);
        }
    }
    assert!(found > 0 && missed > 0, "{found} found, {missed} missed");
}

/// `reconcile` without the app index: every game tries every app.
fn reconcile_by_search(
    apps: &mut Vec<Value>,
    games: &[Game],
    settings: &Settings,
    read: &[Source],
    dismissed: &[String],
) -> SyncChange {
    let mut change = SyncChange::default();
    let wanted = chosen(games, settings, u64::MAX / 2);
    let before = apps.len();
    apps.retain(|app| {
        let Some(source) = app["launcher-source"]
            .as_str()
            .and_then(|key| Source::ALL.into_iter().find(|source| source.key() == key))
        else {
            return true;
        };
        let Some(options) = settings.options(source).filter(|options| options.enabled) else {
            return true;
        };
        let id = app["launcher-id"].as_str().unwrap_or("");
        if !synced_by(app, "launcher") {
            return true;
        }
        if options.excludes(id, app["name"].as_str().unwrap_or("")) {
            return false;
        }
        let installed = games
            .iter()
            .any(|game| game.source == source && game.id == id);
        !read.contains(&source)
            || (installed && (options.sync_all_installed || wanted.contains(&(source, id))))
            || (!installed && options.sync_all_installed && !options.remove_uninstalled)
    });
    change.removed = before - apps.len();
    for game in games {
        if !wanted.contains(&(game.source, game.id.as_str()))
            || dismissed.contains(&dismissed_key(game.source, &game.id))
            || apps.iter().any(|app| game.matches(app))
        {
            continue;
        }
        if let Ok(mut app) = game.app() {
            app[MANAGED] = json!("auto");
            app[SYNCED_BY] = json!("launcher");
            apps.push(app);
            change.added += 1;
        }
    }
    change
}

#[test]
fn the_launcher_sync_adds_and_removes_the_apps_it_did_in_the_same_order() {
    let mut apps = indexed_apps();
    // Synced apps whose games are gone, still installed, or of a launcher that is off.
    for (name, source, id) in [
        ("Gone", "epic", "gone"),
        ("Mirage", "ubisoft", "13504"),
        ("Old GOG", "gog", "old"),
        ("Left Local", "local", r"d:\games\left"),
    ] {
        let mut app = json!({"name": name, "launcher-source": source, "launcher-id": id});
        app[MANAGED] = json!("auto");
        app[SYNCED_BY] = json!("launcher");
        apps.insert(1, app);
    }
    // Hades is added before the `D:\Games` folder game, which then finds Hades' app.
    let mut games = local_games();
    games.extend(indexed_games());
    let all_settings = [
        Settings::default(),
        local_settings(),
        sync_settings(concat!(
            "library_autosync_remove_uninstalled = false\n",
            r#"library_local_folders = ["D:\\Games"]"#,
        )),
        sync_settings("library_auto_sync = false\n").automatic(),
        sync_settings(concat!(
            "battlenet_sync_all_installed = false\n",
            "epic_exclude_games = [{\"name\": \"Étoile\"}]\n",
            "ubisoft_exclude_games = [\"13504\"]\n",
        )),
    ];
    let reads: [&[Source]; 2] = [&Source::ALL, &[Source::Epic, Source::Local]];
    let dismissals = [
        vec![],
        vec![dismissed_key(Source::Local, r"d:\games\hades")],
    ];
    let (mut added, mut removed) = (0, 0);
    for settings in &all_settings {
        for read in reads {
            for dismissed in &dismissals {
                let (mut indexed, mut searched) = (apps.clone(), apps.clone());
                for pass in 0..2 {
                    let change = reconcile(&mut indexed, &games, settings, read, dismissed);
                    let expected =
                        reconcile_by_search(&mut searched, &games, settings, read, dismissed);
                    assert_eq!(change, expected, "pass {pass} with {settings:?}");
                    assert_eq!(indexed, searched, "pass {pass} with {settings:?}");
                    added += change.added;
                    removed += change.removed;
                }
            }
        }
    }
    assert!(added > 0 && removed > 0, "{added} added, {removed} removed");
}
