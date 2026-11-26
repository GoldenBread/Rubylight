//! Least-privilege user-copy execution, elevation policy, hub-handler precedence, and anti-cheat refusal tests.
use super::setup_helper::{admit, app, background, client, setup};
use butterpollo_config::config::Config;
use butterpollo_session::nucleus::elevation::{self, Account, Need, Run, Why};
use butterpollo_session::nucleus::generic::Setup;
use butterpollo_session::nucleus::handler::Handler;
use butterpollo_session::session::Warnings;
use butterpollo_stream::nucleus::*;
use std::{path::PathBuf, sync::Arc, time::Instant};

/// The copy the fake PC makes for the signed-in user.
fn user_copy() -> PathBuf {
    PathBuf::from(format!(
        r"C:\ProgramData\Butterpollo\nucleus-user\{}\0123456789abcdef",
        fake::USER
    ))
}

#[test]
fn ours_runs_as_the_user_for_a_game_that_needs_no_administrator_rights() {
    let (registry, pc, _) = setup();
    let ice = app("Ice", r"C:\NoSuchGames\Ice\Ice.exe");
    // Least privilege: also on an account that could elevate, and on one
    // that can't.
    for (launch, account) in [
        ("a", Account::Elevates),
        ("b", Account::Standard),
        ("c", Account::Protected),
    ] {
        *pc.account.lock().unwrap() = account;
        let seat = background(
            &registry,
            &ice,
            launch,
            &client(launch, false),
            &Config::default(),
        )
        .unwrap();
        assert!(registry.end(seat.session));
    }
    assert_eq!(
        *pc.ready_runs.lock().unwrap(),
        [Run::User, Run::User, Run::User]
    );
    // Its handler and Nucleus in the user's copy, never the host's.
    assert_eq!(*pc.own_in.lock().unwrap(), vec![Some(user_copy()); 3]);
    assert_eq!(*pc.user_copies.lock().unwrap(), vec![Some(user_copy()); 3]);
    // The game's folder was walked at launch, as Nucleus will.
    assert_eq!(pc.walks.load(std::sync::atomic::Ordering::Relaxed), 3);
    // Made from the host's Nucleus at each launch, which checks it.
    assert_eq!(
        *pc.copied_from.lock().unwrap(),
        [
            PathBuf::from(r"C:\Nucleus"),
            PathBuf::from(r"C:\Nucleus"),
            PathBuf::from(r"C:\Nucleus")
        ]
    );
}

#[test]
fn the_users_copy_is_made_once_the_consoles_nucleus_closed() {
    use std::sync::atomic::Ordering::Relaxed;
    let (registry, pc, _) = setup();
    registry.open("").unwrap();
    let window = pc.window();
    let ice = app("Ice", r"C:\NoSuchGames\Ice\Ice.exe");
    let seat = background(
        &registry,
        &ice,
        "a",
        &client("A", false),
        &Config::default(),
    )
    .unwrap();
    assert!(registry.end(seat.session));
    assert!(!window.open.load(Relaxed));
    // Copied with no Nucleus Co-op running from the host's folder.
    assert_eq!(*pc.copied_while.lock().unwrap(), [None]);
    assert_eq!(*pc.user_copies.lock().unwrap(), [Some(user_copy())]);
}

#[test]
fn ours_runs_elevated_for_a_game_that_needs_administrator_rights() {
    let (registry, pc, _) = setup();
    let exe = r"C:\NoSuchGames\Ice\Ice.exe";
    let ice = app("Ice", exe);
    for (launch, need) in [
        ("a", Need::Administrator(Why::Manifest)),
        ("b", Need::Administrator(Why::Compatibility)),
        ("c", Need::Highest),
    ] {
        pc.needs.lock().unwrap().insert(exe.into(), need);
        let seat = background(
            &registry,
            &ice,
            launch,
            &client("A", false),
            &Config::default(),
        )
        .unwrap();
        assert!(registry.end(seat.session));
    }
    // Found only by walking its folder (a game under Program Files).
    pc.needs.lock().unwrap().clear();
    pc.folder_needs
        .lock()
        .unwrap()
        .insert(exe.into(), Need::Administrator(Why::Folder));
    background(
        &registry,
        &ice,
        "d",
        &client("A", false),
        &Config::default(),
    )
    .unwrap();
    assert_eq!(*pc.ready_runs.lock().unwrap(), [Run::Elevated; 4]);
    assert_eq!(*pc.own_in.lock().unwrap(), [None, None, None, None]);
    assert_eq!(*pc.user_copies.lock().unwrap(), [None, None, None, None]);
}

#[test]
fn an_admin_game_or_a_hub_handler_on_an_account_without_elevation_is_refused() {
    let (registry, pc, apps) = setup();
    *pc.account.lock().unwrap() = Account::Standard;
    let exe = r"C:\NoSuchGames\Ice\Ice.exe";
    pc.needs
        .lock()
        .unwrap()
        .insert(exe.into(), Need::Administrator(Why::Manifest));
    let refused = background(
        &registry,
        &app("Ice", exe),
        "a",
        &client("A", false),
        &Config::default(),
    )
    .unwrap_err();
    assert_eq!(refused.status, 503);
    assert_eq!(
        refused.message,
        format!(
            "Ice can't stream in the background: the co-op helper did not start Ice: The game needs administrator rights (its manifest asks for them), so the co-op helper runs elevated, and {}",
            elevation::STANDARD
        )
    );
    // Under Windows' Administrator protection: that the game can't run,
    // not to sign in as an administrator, which doesn't help.
    *pc.account.lock().unwrap() = Account::Protected;
    let refused = background(
        &registry,
        &app("Ice", exe),
        "p",
        &client("P", false),
        &Config::default(),
    )
    .unwrap_err();
    assert!(
            refused.message.ends_with(", and Windows' Administrator protection keeps them from Rubylight, so the co-op helper can't run this game on this PC; games Rubylight adds itself still stream if they need no administrator rights"),
            "{refused:?}"
        );
    *pc.account.lock().unwrap() = Account::Standard;
    // A hub handler, whatever it says, background or co-op.
    let refused = background(
        &registry,
        &apps[1],
        "b",
        &client("B", false),
        &Config::default(),
    )
    .unwrap_err();
    assert!(
        refused
            .message
            .contains("needs administrator rights for a hub handler"),
        "{refused:?}"
    );
    let coop = registry.entries(&apps, "").remove(0);
    let refused = admit(&registry, &coop, "c", &client("C", true)).unwrap_err();
    assert!(
        refused
            .message
            .contains("needs administrator rights for a hub handler"),
        "{refused:?}"
    );
    // Nothing written, opened or started another way.
    assert!(pc.own.lock().unwrap().is_empty());
    assert!(pc.ready_runs.lock().unwrap().is_empty());
    assert!(pc.started.lock().unwrap().is_empty());
}

#[test]
fn a_hub_handler_runs_elevated_whatever_it_says() {
    let (registry, pc, apps) = setup();
    background(
        &registry,
        &apps[1],
        "a",
        &client("A", false),
        &Config::default(),
    )
    .unwrap();
    assert_eq!(*pc.ready_runs.lock().unwrap(), [Run::Elevated]);
    assert_eq!(*pc.user_copies.lock().unwrap(), [None]);
    // Its game's needs are never read.
    assert_eq!(pc.walks.load(std::sync::atomic::Ordering::Relaxed), 0);
}

#[test]
fn a_user_copy_that_is_not_made_refuses_the_stream() {
    let (registry, pc, _) = setup();
    registry.open("").unwrap();
    *pc.copy_error.lock().unwrap() = Some(
        r"an administrator of this PC can delete C:\ProgramData\Butterpollo\nucleus-user".into(),
    );
    let refused = background(
        &registry,
        &app("Ice", r"C:\NoSuchGames\Ice\Ice.exe"),
        "a",
        &client("A", false),
        &Config::default(),
    )
    .unwrap_err();
    assert_eq!(refused.status, 503);
    assert!(
        refused.message.contains(
            "the signed-in user's own copy of the co-op helper was not made: an administrator"
        ),
        "{refused:?}"
    );
    assert!(pc.started.lock().unwrap().is_empty());
    // Made only once the console's Nucleus Co-op is closed.
    assert!(
        !pc.windows.lock().unwrap()[0]
            .open
            .load(std::sync::atomic::Ordering::Relaxed)
    );
}

#[test]
fn the_console_reads_whether_the_account_elevates_once_a_minute() {
    let (registry, pc, _) = setup();
    let reads = || pc.elevations.load(std::sync::atomic::Ordering::Relaxed);
    assert_eq!(registry.account(), Some(Account::Elevates));
    // The 3 s poll asks Windows once.
    *pc.account.lock().unwrap() = Account::Standard;
    assert_eq!(registry.account(), Some(Account::Elevates));
    assert_eq!(reads(), 1);
    // A minute later it reads the account again.
    registry.account.lock().unwrap().as_mut().unwrap().0 = Instant::now() - OWN_STATUS_TTL;
    assert_eq!(registry.account(), Some(Account::Standard));
    assert_eq!(reads(), 2);
    // One Rubylight could not tell is unknown, and asked again next time.
    registry.account.lock().unwrap().as_mut().unwrap().0 = Instant::now() - OWN_STATUS_TTL;
    *pc.account_error.lock().unwrap() = Some("nobody is signed in".into());
    assert_eq!(registry.account(), None);
    *pc.account_error.lock().unwrap() = None;
    assert_eq!(registry.account(), Some(Account::Standard));
    assert_eq!(reads(), 4);
}

#[test]
fn the_console_says_how_ours_runs_or_why_it_cant() {
    let (registry, pc, _) = setup();
    let dir = std::env::temp_dir().join(format!("nucleus-own-{}", uuid::Uuid::new_v4()));
    let exe = dir.join("Ice").join("Ice.exe");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(&exe, b"").unwrap();
    let apps = [app("Ice", &format!("\"{}\"", exe.display()))];
    let path = exe.to_string_lossy().into_owned();
    let status = || {
        registry.own_statuses.lock().unwrap().clear();
        registry.games(&apps, &Config::default()).unwrap().unwrap()[0].clone()
    };
    let run = |status: GameStatus| match status {
        GameStatus::Own { run, .. } => Ok(run),
        GameStatus::Missing(missing) => Err(missing.to_string()),
        other => Err(format!("{other:?}")),
    };
    assert_eq!(run(status()), Ok(Run::User));
    pc.needs
        .lock()
        .unwrap()
        .insert(path.clone(), Need::Administrator(Why::Compatibility));
    assert_eq!(run(status()), Ok(Run::Elevated));
    *pc.account.lock().unwrap() = Account::Standard;
    let refused = run(status()).unwrap_err();
    assert!(
            refused.starts_with("The game needs administrator rights (its compatibility settings run it as administrator)"),
            "{refused}"
        );
    // "The most the account has" is the user's own rights without elevation.
    pc.needs.lock().unwrap().insert(path, Need::Highest);
    assert_eq!(run(status()), Ok(Run::User));
    // The console never walks the game's folder.
    assert_eq!(pc.walks.load(std::sync::atomic::Ordering::Relaxed), 0);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_hub_handler_always_wins_over_ours() {
    let (registry, pc, apps) = setup();
    let owner = client("A", false);
    let config = Config::default();
    // Portal 2 has the hub's handler and the user added it.
    let seat = background(&registry, &apps[1], "a", &owner, &config).unwrap();
    assert_eq!(registry.sessions()[0].session.game, fake::GUID);
    assert!(pc.own.lock().unwrap().is_empty());
    assert!(registry.end(seat.session));
    // Found from a link rather than the app's command: still the hub's.
    let link = app("Portal 2 Link", "steam://rungameid/620");
    pc.one_player.lock().unwrap().insert(
        link.name.clone(),
        Setup::new(fake::EXE, &link.name, Some((620, r"C:\Games\Portal 2"))),
    );
    background(&registry, &link, "b", &owner, &config).unwrap();
    assert_eq!(pc.last().session.lock().unwrap().game, fake::GUID);
    assert!(pc.own.lock().unwrap().is_empty());
    // A hub handler sets up Steam as it needs: Nucleus gets no app id.
    assert_eq!(*pc.steam_apps.lock().unwrap(), [None, None]);
    // A hub handler for a game the user did not add: ours streams it.
    for listed in registry.sessions() {
        registry.end(listed.session.id);
    }
    pc.catalog.lock().unwrap().games.clear();
    background(&registry, &apps[1], "c", &owner, &config).unwrap();
    assert_eq!(pc.own.lock().unwrap().len(), 1);
}

#[test]
fn ours_goes_once_the_user_added_the_game_with_a_hub_handler() {
    let (registry, pc, apps) = setup();
    let owner = client("A", false);
    let config = Config::default();
    // Ours from before the hub's handler came, for the same .exe.
    let ours = Setup::new(fake::EXE, "Portal 2", None);
    {
        let mut catalog = pc.catalog.lock().unwrap();
        let hub = catalog.handlers[0].clone();
        catalog.handlers.push(Handler {
            file: ours.file(),
            guid: ours.guid(),
            own: true,
            ..hub
        });
        catalog
            .games
            .push(butterpollo_session::nucleus::handler::Game {
                guid: ours.guid(),
                exe_path: fake::EXE.into(),
            });
    }
    // A refused launch removes nothing: Nucleus' window stays open.
    *pc.free.lock().unwrap() = 0;
    background(&registry, &apps[1], "a", &owner, &config).unwrap_err();
    assert!(pc.removed.lock().unwrap().is_empty());
    *pc.free.lock().unwrap() = 4;
    background(&registry, &apps[1], "a", &owner, &config).unwrap();
    assert_eq!(registry.sessions()[0].session.game, fake::GUID);
    assert_eq!(*pc.removed.lock().unwrap(), [ours.file()]);
    // Checked, with the list it edits, before the window closed.
    assert_eq!(
        pc.ready_stale.lock().unwrap().last().unwrap(),
        &[ours.file()]
    );
    // Nucleus starts the game the user added, at its path.
    assert_eq!(*pc.exe_paths.lock().unwrap(), [fake::EXE]);
}

#[test]
fn ours_goes_as_soon_as_a_hub_handler_for_its_exe_is_installed() {
    // Nucleus' add-game search skips a path in its list: our entry would
    // keep the user from adding the game with the hub's handler.
    let (registry, pc, apps) = setup();
    let owner = client("A", false);
    let config = Config::default();
    let ice = app("Ice", r"C:\Games\Ice\portal2.exe");
    let ours = Setup::new(r"C:\Games\Ice\portal2.exe", "Ice", None);
    let unused = Setup::new(r"C:\Games\Old\Old.exe", "Old", None);
    {
        let mut catalog = pc.catalog.lock().unwrap();
        for setup in [&ours, &unused] {
            catalog.handlers.push(
                butterpollo_session::nucleus::handler::parse(
                    &setup.file(),
                    &setup.handler_of(Some(fake::USER)),
                )
                .unwrap(),
            );
        }
        catalog
            .games
            .push(butterpollo_session::nucleus::handler::Game {
                guid: ours.guid(),
                exe_path: ours.exe_path.clone(),
            });
    }
    // Ice itself streams with ours (the hub's portal2.exe is another
    // game's); only the unused one goes.
    let seat = background(&registry, &ice, "a", &owner, &config).unwrap();
    assert_eq!(*pc.removed.lock().unwrap(), [unused.file()]);
    assert_eq!(
        *pc.exe_paths.lock().unwrap(),
        std::slice::from_ref(&ours.exe_path)
    );
    assert!(registry.end(seat.session));
    // Another session's start removes ours; a failure is the stream's
    // warning, and the session still starts.
    pc.removed.lock().unwrap().clear();
    *pc.remove_error.lock().unwrap() = Some("access denied".into());
    let warnings = Arc::new(Warnings::default());
    let coop = registry.entries(&apps, "").remove(0);
    registry
        .admit(Admission {
            launch: "b",
            client: &owner,
            game: Game::Coop(&coop),
            size: (1280, 720),
            config: &config,
            warnings: warnings.clone(),
            release_parked: &|| Ok(()),
            instances: None,
        })
        .unwrap();
    let shown = warnings.snapshot();
    assert_eq!(shown.len(), 1, "{shown:?}");
    assert_eq!(shown[0].code, HANDLER_NOT_REMOVED);
    assert_eq!(
        shown[0].message,
        format!(
            "Rubylight's co-op handlers {}, {} were not fully removed: access denied",
            ours.file(),
            unused.file()
        )
    );
    assert_eq!(pc.started.lock().unwrap().len(), 2);
    *pc.remove_error.lock().unwrap() = None;
    for listed in registry.sessions() {
        registry.end(listed.session.id);
    }
    admit(&registry, &coop, "c", &client("C", false)).unwrap();
    assert_eq!(*pc.removed.lock().unwrap(), [ours.file(), unused.file()]);
}

#[test]
fn a_steam_link_passes_only_its_launch_options_arguments() {
    // An imported Steam game's command handed cmd's own arguments on.
    assert_eq!(
        steam_arguments("-game portal", r#"cmd /c start "" steam://rungameid/400"#),
        "-game portal"
    );
    assert_eq!(
        steam_arguments(" -game portal ", "steam://rungameid/400  -novid"),
        "-game portal -novid"
    );
    assert_eq!(steam_arguments("", "steam://rungameid/400"), "");
}

#[test]
fn a_launcher_is_refused_and_an_unreal_stub_starts_its_game() {
    let (registry, pc, _) = setup();
    let owner = client("A", false);
    let config = Config::default();
    // Steam would start the game outside Nucleus' hooks.
    let steam = app(
        "Portal",
        r#""C:\Program Files (x86)\Steam\steam.exe" -applaunch 400"#,
    );
    let refused = background(&registry, &steam, "a", &owner, &config).unwrap_err();
    assert_eq!(refused.status, 404);
    assert_eq!(
        refused.message,
        "Portal can't stream in the background: steam.exe is a launcher, not the game: set the app's command to the game's .exe"
    );
    assert!(pc.own.lock().unwrap().is_empty());
    // The stub at an Unreal game's root only starts its Shipping .exe.
    let root = std::env::temp_dir().join(format!("nucleus-ue-{}", uuid::Uuid::new_v4()));
    let win64 = root.join("Stray").join("Binaries").join("Win64");
    std::fs::create_dir_all(&win64).unwrap();
    std::fs::create_dir_all(root.join("Engine")).unwrap();
    std::fs::write(win64.join("Stray-Win64-Shipping.exe"), b"").unwrap();
    let stub = root.join("Stray.exe");
    std::fs::write(&stub, b"").unwrap();
    let stray = app("Stray", &format!("\"{}\" -dx12", stub.display()));
    let started = background(&registry, &stray, "b", &owner, &config);
    std::fs::remove_dir_all(&root).unwrap();
    started.unwrap();
    let own = pc.own.lock().unwrap().clone();
    assert_eq!(
        own[0].exe_path,
        win64.join("Stray-Win64-Shipping.exe").to_string_lossy()
    );
    assert_eq!(own[0].arguments, "-dx12");
}

#[test]
fn another_games_exe_of_the_same_file_name_streams_with_ours() {
    // Portal's hl2.exe started Half-Life 2's hub handler by its name.
    let (registry, pc, _) = setup();
    let mod_game = app(
        "Portal 2 Mod",
        r#""C:\NoSuchGames\Portal 2 Mod\portal2.exe" -game mod"#,
    );
    background(
        &registry,
        &mod_game,
        "a",
        &client("A", false),
        &Config::default(),
    )
    .unwrap();
    let own = pc.own.lock().unwrap().clone();
    assert_eq!(own.len(), 1);
    assert_eq!(own[0].exe_path, r"C:\NoSuchGames\Portal 2 Mod\portal2.exe");
    assert_eq!(registry.sessions()[0].session.game, own[0].guid());
}

#[test]
fn a_link_to_another_game_of_the_same_file_name_streams_with_ours() {
    // Portal's hl2.exe found Half-Life 2's handler by its file name.
    let (registry, pc, _) = setup();
    let owner = client("A", false);
    let config = Config::default();
    let link = app("Portal 2 Mod", "steam://rungameid/12345");
    let exe = r"C:\Games\Portal 2 Mod\portal2.exe";
    pc.one_player.lock().unwrap().insert(
        link.name.clone(),
        Setup::new(exe, &link.name, Some((12345, r"C:\Games\Portal 2 Mod"))),
    );
    background(&registry, &link, "a", &owner, &config).unwrap();
    let own = pc.own.lock().unwrap().clone();
    assert_eq!(own.len(), 1);
    assert_eq!(own[0].exe_path, exe);
    assert_eq!(registry.sessions()[0].session.game, own[0].guid());
}

#[test]
fn a_background_stream_of_an_anti_cheat_game_is_refused_and_never_hooked() {
    let (registry, pc, _) = setup();
    let owner = client("A", false);
    let config = Config::default();
    let dir = std::env::temp_dir().join(format!("nucleus-ac-{}", uuid::Uuid::new_v4()));
    let game = dir.join("Shooter");
    std::fs::create_dir_all(game.join("EasyAntiCheat")).unwrap();
    let shooter = app(
        "Shooter",
        &format!("\"{}\"", game.join("Shooter.exe").display()),
    );
    let refused = background(&registry, &shooter, "a", &owner, &config).unwrap_err();
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(refused.status, 404);
    assert_eq!(
        refused.message,
        "Shooter can't stream in the background: Shooter.exe uses Easy Anti-Cheat, and Rubylight never hooks a game with anti-cheat"
    );
    assert!(pc.own.lock().unwrap().is_empty());
    assert!(pc.started.lock().unwrap().is_empty());
    // Steam lists Valve Anti-Cheat for a game whose files say nothing.
    let link = app("Portal", "steam://rungameid/400");
    pc.one_player.lock().unwrap().insert(
        link.name.clone(),
        Setup {
            anti_cheat: Some("Valve Anti-Cheat"),
            ..Setup::new(
                r"Z:\NoSuch\common\Portal\hl2.exe",
                &link.name,
                Some((400, r"Z:\NoSuch\common\Portal")),
            )
        },
    );
    let refused = background(&registry, &link, "b", &owner, &config).unwrap_err();
    assert_eq!(
        refused.message,
        "Portal can't stream in the background: hl2.exe uses Valve Anti-Cheat, and Rubylight never hooks a game with anti-cheat"
    );
    assert!(pc.own.lock().unwrap().is_empty());
}

#[test]
fn our_handler_is_never_written_while_nucleus_runs_or_when_it_fails() {
    let (registry, pc, _) = setup();
    let owner = client("A", false);
    let config = Config::default();
    let ice = app("Ice", r"C:\NoSuchGames\Ice\Ice.exe");
    *pc.foreign.lock().unwrap() = Some(77);
    let refused = background(&registry, &ice, "a", &owner, &config).unwrap_err();
    assert_eq!(refused.status, 503);
    assert!(
            refused
                .message
                .starts_with("Ice can't stream in the background: Rubylight's co-op handler for Ice was not written: "),
            "{refused}"
        );
    assert!(pc.own.lock().unwrap().is_empty());
    *pc.foreign.lock().unwrap() = None;
    *pc.write_error.lock().unwrap() = Some("access denied".into());
    let refused = background(&registry, &ice, "b", &owner, &config).unwrap_err();
    assert!(
        refused.message.ends_with("was not written: access denied"),
        "{refused}"
    );
    assert!(pc.started.lock().unwrap().is_empty());
}
