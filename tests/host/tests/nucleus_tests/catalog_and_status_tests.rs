//! Co-op catalog entry listing, console game status caching, and install-folder check tests for Nucleus Co-op.
use super::setup_helper::{app, refuse, setup};
use butterpollo_config::config::Config;
use butterpollo_session::nucleus::elevation::{Account, Need, Run, Why};
use butterpollo_session::nucleus::handler::Missing;
use butterpollo_session::nucleus::install::Problem;
use butterpollo_stream::nucleus::*;
use serde_json::json;
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

#[test]
fn entries_list_apps_with_a_handler_and_an_added_game_only() {
    let (registry, pc, apps) = setup();
    let entries = registry.entries(&apps, "");
    assert_eq!(entries.len(), 1);
    let coop = &entries[0];
    assert_eq!(coop.entry.name, "Portal 2 (Co-op)");
    assert_eq!(coop.app.name, "Portal 2");
    assert_eq!(coop.id(), app("Portal 2 (Co-op)", "").id());
    // The id follows from the name (its CRC-32), as for every app
    // without its own: the rename changed it, before any release.
    assert_eq!(coop.id(), 242_170_942);
    assert!(coop.entry.cmd.is_empty());
    assert_eq!(
        registry.resolve(&apps, "", coop.id()).unwrap().app.name,
        "Portal 2"
    );
    assert!(registry.resolve(&apps, "", apps[1].id()).is_none());
    // A detached command names the program too.
    let mut detached = app("Portal 2 detached", "");
    detached.extra.insert(
        "detached".into(),
        serde_json::json!([format!("\"{}\"", fake::EXE)]),
    );
    assert_eq!(registry.entries(&[detached], "").len(), 1);
    // A name an app already has is not listed twice.
    let taken = app("Portal 2 (Co-op)", "");
    assert!(entries_of(&pc.catalog.lock().unwrap(), &[apps[1].clone(), taken]).is_empty());
    // Not added in Nucleus: no entry.
    pc.catalog.lock().unwrap().games.clear();
    registry.use_pc(pc.clone());
    assert!(registry.entries(&apps, "").is_empty());
}

#[test]
fn no_entries_without_nucleus_or_where_it_cannot_run() {
    let (registry, pc, apps) = setup();
    *pc.installed.lock().unwrap() = None;
    assert!(registry.entries(&apps, "").is_empty());
    assert_eq!(registry.install(""), Install::Missing);
    let unavailable = Arc::new(fake::Pc::unavailable());
    registry.use_pc(unavailable);
    assert!(registry.entries(&apps, "").is_empty());
    assert_eq!(
        registry.unavailable().as_deref(),
        Some("not on this test PC")
    );
}

#[test]
fn an_app_can_hide_its_coop_entry_and_its_status_stays() {
    let (registry, _pc, mut apps) = setup();
    // As every app switch reads (and the console's flagValue): true, "true", "1" and "yes"
    // are on, any other text is off, and any other value is the default, on.
    for hidden in [
        json!(false),
        json!("false"),
        json!("0"),
        json!("off"),
        json!("TRUE"),
    ] {
        apps[1].extra.insert(ENTRY_KEY.into(), hidden.clone());
        assert!(registry.entries(&apps, "").is_empty(), "{hidden}");
    }
    for shown in [
        json!(true),
        json!("true"),
        json!("1"),
        json!("yes"),
        json!(0),
        json!(null),
    ] {
        apps[1].extra.insert(ENTRY_KEY.into(), shown.clone());
        assert_eq!(registry.entries(&apps, "").len(), 1, "{shown}");
    }
    apps[1].extra.insert(ENTRY_KEY.into(), json!(false));
    let games = registry.games(&apps, &Config::default()).unwrap().unwrap();
    assert_eq!(
        games[1],
        GameStatus::Ready {
            handler: "Portal 2".into(),
            players: 2
        }
    );
    assert!(matches!(
        games[0],
        GameStatus::Missing(Missing::NoExecutable { .. })
    ));
    assert!(matches!(
        &games[2],
        GameStatus::Missing(Missing::NoHandler { .. })
    ));
    assert!(!names_executable(&apps[0]) && names_executable(&apps[1]));
    // A launcher's link names no game .exe: no handler can match it.
    let steam = app(
        "Counter-Strike 2",
        "cmd /c start \"\" steam://rungameid/730",
    );
    assert!(names_executable(&steam) && !starts_exe(&steam));
    assert!(!starts_exe(&apps[2]) && starts_exe(&apps[1]));
    assert!(starts_exe(&app("Puzzle", r"D:\Games\PUZZLE.EXE")));
}

#[test]
fn the_console_shows_why_a_background_stream_would_be_refused() {
    // These showed "streams in the background without a handler" and
    // were then refused at launch.
    let (registry, _pc, _) = setup();
    let dir = std::env::temp_dir().join(format!("nucleus-status-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(dir.join("Shooter").join("EasyAntiCheat")).unwrap();
    std::fs::create_dir_all(dir.join("Ice")).unwrap();
    std::fs::write(dir.join("Shooter").join("Shooter.exe"), b"").unwrap();
    std::fs::write(dir.join("Ice").join("Ice.exe"), b"").unwrap();
    let quoted = |path: PathBuf| format!("\"{}\"", path.display());
    let apps = [
        app("Notepad", r"C:\Windows\notepad.exe"),
        app("Shared", r"\\nas\games\Ice\Ice.exe"),
        app("Shooter", &quoted(dir.join("Shooter").join("Shooter.exe"))),
        app("Relative", "Ice.exe"),
        app("Gone", r"C:\NoSuchGames\Gone\Gone.exe"),
        app("Ice", &quoted(dir.join("Ice").join("Ice.exe"))),
        app("Link", "steam://rungameid/400"),
    ];
    let statuses = registry.games(&apps, &Config::default()).unwrap().unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    let shown: Vec<String> = statuses
        .iter()
        .map(|status| match status {
            GameStatus::Missing(missing) | GameStatus::Own { missing, .. } => missing.to_string(),
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(
        shown[0],
        r"C:\Windows\notepad.exe is part of Windows, not a game"
    );
    assert!(
        shown[1].ends_with(
            "is on a network share: Rubylight starts only a game on this PC's own drives"
        )
    );
    assert_eq!(
        shown[2],
        "Shooter.exe uses Easy Anti-Cheat, and Rubylight never hooks a game with anti-cheat"
    );
    assert!(
        shown[3].starts_with("The co-op helper starts games by their .exe"),
        "{}",
        shown[3]
    );
    assert!(shown[4].ends_with("is not on this PC: set the app's command to the game's .exe"));
    // A game ours streams, and a link checked only at launch.
    assert!(
        shown[5].starts_with("No co-op handler for Ice.exe"),
        "{}",
        shown[5]
    );
    // Without administrator rights, as nothing asks for them.
    assert!(matches!(
        statuses[5],
        GameStatus::Own {
            missing: Missing::NoHandler { .. },
            run: Run::User
        }
    ));
    assert!(matches!(
        statuses[6],
        GameStatus::Missing(Missing::NoHandler { .. })
    ));
    assert!(matches!(
        statuses[2],
        GameStatus::Missing(Missing::AntiCheat { .. })
    ));
    assert!(matches!(
        statuses[0],
        GameStatus::Missing(Missing::Refused { .. })
    ));
}

#[test]
fn game_statuses_need_an_install_and_follow_the_player_cap() {
    let (registry, pc, apps) = setup();
    let mut config = Config::default();
    config
        .values
        .insert("nucleus_coop_players".into(), "1".into());
    assert_eq!(
        registry.games(&apps, &config).unwrap().unwrap()[1],
        GameStatus::Ready {
            handler: "Portal 2".into(),
            players: 1
        }
    );
    // Not added in Nucleus Co-op yet, and the .exe isn't on this PC: a
    // background stream would say so.
    pc.catalog.lock().unwrap().games.clear();
    registry.use_pc(pc.clone());
    assert!(matches!(
        &registry.games(&apps, &config).unwrap().unwrap()[1],
        GameStatus::Missing(Missing::Refused { reason }) if reason.contains("is not on this PC")
    ));
    *pc.installed.lock().unwrap() = None;
    assert!(registry.games(&apps, &config).is_none());
}

#[test]
fn a_hub_handler_without_the_added_game_says_to_add_it_unless_that_cant_help() {
    // Adding the game makes the hub handler start it, anti-cheat and
    // all: the console says to add it. A game that isn't there says so.
    let (registry, pc, _) = setup();
    pc.catalog.lock().unwrap().games.clear();
    registry.use_pc(pc.clone());
    let dir = std::env::temp_dir().join(format!("nucleus-own-{}", uuid::Uuid::new_v4()));
    let game = dir.join("Portal 2");
    std::fs::create_dir_all(game.join("EasyAntiCheat")).unwrap();
    std::fs::write(game.join("portal2.exe"), b"").unwrap();
    let apps = [app(
        "Portal 2",
        &format!("\"{}\"", game.join("portal2.exe").display()),
    )];
    assert!(matches!(
        &registry.games(&apps, &Config::default()).unwrap().unwrap()[0],
        GameStatus::Missing(Missing::NotAdded { .. })
    ));
    // Not on this PC, or no .exe named: adding can't help.
    let gone = [
        app(
            "Portal 2",
            &format!("\"{}\"", dir.join(r"Gone\portal2.exe").display()),
        ),
        app("Portal 2", "portal2.exe"),
    ];
    let statuses = registry.games(&gone, &Config::default()).unwrap().unwrap();
    assert!(matches!(
        &statuses[0],
        GameStatus::Missing(Missing::Refused { reason }) if reason.contains("is not on this PC")
    ));
    assert!(matches!(
        &statuses[1],
        GameStatus::Missing(Missing::NoExecutable { .. })
    ));
    // A game ours streams keeps the hint to add it for co-op.
    let other = dir.join("Clean");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("portal2.exe"), b"").unwrap();
    let apps = [app(
        "Portal 2",
        &format!("\"{}\"", other.join("portal2.exe").display()),
    )];
    assert!(matches!(
        &registry.games(&apps, &Config::default()).unwrap().unwrap()[0],
        GameStatus::Own {
            missing: Missing::NotAdded { .. },
            run: Run::User
        }
    ));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_consoles_answer_for_an_exe_is_kept_until_it_changes_or_ages() {
    let (registry, _pc, _) = setup();
    let dir = std::env::temp_dir().join(format!("nucleus-own-{}", uuid::Uuid::new_v4()));
    let exe = dir.join("Ice").join("Ice.exe");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(&exe, b"").unwrap();
    let apps = [app("Ice", &format!("\"{}\"", exe.display()))];
    let status = || registry.games(&apps, &Config::default()).unwrap().unwrap()[0].clone();
    let streams = |status: GameStatus| {
        matches!(
            status,
            GameStatus::Own {
                missing: Missing::NoHandler { .. },
                ..
            }
        )
    };
    assert!(streams(status()));
    // Anti-cheat added beside it: the console asks again within the
    // minute and gets the kept answer, without a scan.
    std::fs::create_dir(dir.join("Ice").join("EasyAntiCheat")).unwrap();
    assert!(streams(status()));
    // The .exe changed: read again.
    let file = std::fs::File::options().write(true).open(&exe).unwrap();
    file.set_modified(std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000))
        .unwrap();
    drop(file);
    assert!(matches!(
        status(),
        GameStatus::Missing(Missing::AntiCheat { .. })
    ));
    // Aged out: read again.
    std::fs::remove_dir(dir.join("Ice").join("EasyAntiCheat")).unwrap();
    assert!(matches!(
        status(),
        GameStatus::Missing(Missing::AntiCheat { .. })
    ));
    for kept in registry.own_statuses.lock().unwrap().values_mut() {
        kept.at = Instant::now() - OWN_STATUS_TTL;
    }
    assert!(streams(status()));
    // A share is refused before anything opens it, and never kept.
    let shared = [app("Shared", r"\\nas\games\Ice\Ice.exe")];
    let _ = registry.games(&shared, &Config::default());
    assert_eq!(registry.own_statuses.lock().unwrap().len(), 1);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_console_asks_again_when_it_could_not_tell() {
    let (registry, pc, _) = setup();
    let dir = std::env::temp_dir().join(format!("nucleus-own-{}", uuid::Uuid::new_v4()));
    let exe = dir.join("Ice").join("Ice.exe");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(&exe, b"").unwrap();
    let apps = [app("Ice", &format!("\"{}\"", exe.display()))];
    let status = || registry.games(&apps, &Config::default()).unwrap().unwrap()[0].clone();
    // No signed-in user yet: said, and asked again at the next refresh.
    *pc.need_error.lock().unwrap() = Some("nobody is signed in".into());
    let GameStatus::Missing(Missing::Refused { reason }) = status() else {
        panic!("{:?}", status());
    };
    assert_eq!(
        reason,
        "Rubylight could not tell whether Ice.exe needs administrator rights: nobody is signed in"
    );
    *pc.need_error.lock().unwrap() = None;
    assert!(matches!(status(), GameStatus::Own { run: Run::User, .. }));
    // A refusal stands until the .exe changes or the minute passes.
    for kept in registry.own_statuses.lock().unwrap().values_mut() {
        kept.at = Instant::now() - OWN_STATUS_TTL;
    }
    *pc.account.lock().unwrap() = Account::Standard;
    pc.needs.lock().unwrap().insert(
        exe.to_string_lossy().into_owned(),
        Need::Administrator(Why::Manifest),
    );
    let refused = status();
    assert!(
        matches!(refused, GameStatus::Missing(Missing::Refused { .. })),
        "{refused:?}"
    );
    pc.needs.lock().unwrap().clear();
    assert_eq!(status(), refused);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_chosen_folder_is_checked_before_it_is_kept_and_an_old_install_still_counts() {
    let (registry, pc, _apps) = setup();
    // An earlier Rubylight's install, with nothing chosen.
    assert_eq!(
        registry.check_folder("").unwrap(),
        Install::Installed(PathBuf::from(r"C:\Nucleus"))
    );
    assert_eq!(
        registry.check_folder(r" D:\Nucleus ").unwrap(),
        Install::Configured(PathBuf::from(r"D:\Nucleus"))
    );
    for (problem, message) in [
        (
            Problem::Missing,
            r"D:\Downloads doesn't hold the co-op helper",
        ),
        (
            Problem::Version,
            r"The co-op helper in D:\Old isn't version 2.4.2",
        ),
        (
            Problem::Untrusted,
            r"Rubylight doesn't run the co-op helper from D:\Mine",
        ),
    ] {
        refuse(&pc, r"D:\Chosen", problem, message);
        let refused = registry.check_folder(r"D:\Chosen").unwrap_err();
        assert_eq!((refused.status, refused.message.as_str()), (400, message));
        assert_eq!(
            registry.install(r"D:\Chosen"),
            Install::Failed {
                problem: Some(problem),
                error: message.into()
            }
        );
    }
    // One that couldn't be checked says so, with no reason of its own.
    *pc.installed_error.lock().unwrap() = Some("access denied".into());
    assert_eq!(
        registry.install(r"D:\Chosen"),
        Install::Failed {
            problem: None,
            error: "access denied".into()
        }
    );
    *pc.installed_error.lock().unwrap() = None;
    *pc.installed.lock().unwrap() = None;
    assert_eq!(registry.check_folder("").unwrap(), Install::Missing);
}

#[test]
fn the_same_refusal_of_co_op_entries_is_logged_once_until_it_changes() {
    let (registry, pc, apps) = setup();
    let refused = "C:\\ can be moved or renamed";
    *pc.installed_error.lock().unwrap() = Some(refused.into());
    // The app list asks every few seconds: only the first refusal is new.
    assert!(registry.entries(&apps, "").is_empty());
    assert!(!registry.newly_refused(Some(refused)));
    assert!(registry.newly_refused(Some("another reason")));
    // Accepted in between, the same refusal logs again.
    *pc.installed_error.lock().unwrap() = None;
    registry.entries(&apps, "");
    assert!(registry.newly_refused(Some(refused)));
}

#[test]
fn choosing_a_folder_and_open_are_refused_where_nucleus_cannot_run() {
    let (registry, _pc, _apps) = setup();
    let unavailable = Arc::new(fake::Pc::unavailable());
    *unavailable.installed.lock().unwrap() = None;
    registry.use_pc(unavailable.clone());
    let refused = registry.check_folder(r"D:\Nucleus").unwrap_err();
    assert_eq!(
        (refused.status, refused.message.as_str()),
        (503, "not on this test PC")
    );
    assert_eq!(registry.install(""), Install::Missing);
    let refused = registry.open("").unwrap_err();
    assert_eq!(
        (refused.status, refused.message.as_str()),
        (503, "not on this test PC")
    );
    assert_eq!(
        unavailable
            .opened
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
}
