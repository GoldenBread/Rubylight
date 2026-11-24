//! Unit tests for Nucleus Co-op handler catalog, own-handler writing, injection tracking, and sweeping.

use super::*;

mod settings_profiles;

const HANDLER: &str = r#"Game.GUID = "Borderlands 2";
Game.GameName = "Borderlands 2";
Game.ExecutableName = "Borderlands2.exe";
Game.MaxPlayers = 4;
"#;

fn player(xinput: u8) -> Player {
    Player {
        xinput,
        bounds: Rect {
            x: 4000 + i32::from(xinput) * 2000,
            y: 0,
            width: 1920,
            height: 1080,
        },
    }
}
const SCREEN: Rect = Rect {
    x: 0,
    y: 0,
    width: 2560,
    height: 1440,
};

#[test]
fn the_catalog_reads_handlers_and_only_the_list_nucleus_reads() {
    let nucleus = tempfile::tempdir().unwrap();
    let handlers = nucleus.path().join("handlers");
    std::fs::create_dir_all(&handlers).unwrap();
    std::fs::write(handlers.join("Borderlands 2.js"), HANDLER).unwrap();
    std::fs::write(handlers.join("broken.js"), "Game.GameName = 1;").unwrap();
    std::fs::write(handlers.join("notes.txt"), HANDLER).unwrap();
    std::fs::create_dir_all(nucleus.path().join("content")).unwrap();
    std::fs::write(
            nucleus.path().join("content").join("userprofile.json"),
            "\u{feff}{\"Games\":[{\"GameGuid\":\"Borderlands 2\",\"ExePath\":\"E:\\\\BL2\\\\Borderlands2.exe\"}]}",
        )
        .unwrap();
    let catalog = catalog(nucleus.path(), true).unwrap();
    assert_eq!(catalog.handlers.len(), 1);
    assert_eq!(catalog.handlers[0].executable, "Borderlands2.exe");
    assert_eq!(catalog.games.len(), 1);
    let found = catalog
        .find("Borderlands 2", Some(r"E:\BL2\Borderlands2.exe"))
        .unwrap();
    assert_eq!(found.handler.guid, "Borderlands 2");

    // No list and no handlers: an empty catalog, not an error.
    let empty = tempfile::tempdir().unwrap();
    let catalog = super::catalog(empty.path(), false).unwrap();
    assert!(catalog.handlers.is_empty() && catalog.games.is_empty());

    // A game list Nucleus wrote but the host cannot read is an error.
    std::fs::create_dir_all(empty.path().join("content")).unwrap();
    std::fs::write(empty.path().join("content").join("userprofile.json"), "{").unwrap();
    let error = format!("{:#}", super::catalog(empty.path(), false).unwrap_err());
    assert!(error.contains("userprofile.json"), "{error}");
}

#[test]
fn nucleus_lists_the_game_rubylight_adds() {
    // #59: Nucleus Co-op 2.4.2 reads only content\userprofile.json, so a
    // game added to the AppData list was never shown, nothing was
    // selected, and the profile Rubylight never appeared.
    let nucleus = tempfile::tempdir().unwrap();
    let game = tempfile::tempdir().unwrap();
    let exe = game.path().join("AC4BFSP.exe");
    std::fs::write(&exe, b"MZ").unwrap();
    let roaming = std::env::var_os("APPDATA").map(PathBuf::from);
    let nucleus_coop = roaming.map(|appdata| appdata.join("Nucleus Coop"));
    let there = nucleus_coop.as_ref().map(|folder| folder.exists());
    let setup = Setup::new(
        &exe.to_string_lossy(),
        "Assassin's Creed IV: Black Flag Resynced",
        None,
    );
    assert_eq!(
        listed_by_nucleus(nucleus.path()).unwrap(),
        Vec::<String>::new()
    );
    write_own(nucleus.path(), &setup).unwrap();
    assert_eq!(listed_by_nucleus(nucleus.path()).unwrap(), [setup.guid()]);
    // Nothing went to the user's roaming folder.
    assert_eq!(nucleus_coop.as_ref().map(|folder| folder.exists()), there);

    // As Nucleus loads it: an entry without a handler, or whose .exe is
    // gone, is not shown.
    std::fs::remove_file(&exe).unwrap();
    assert_eq!(
        listed_by_nucleus(nucleus.path()).unwrap(),
        Vec::<String>::new()
    );
    std::fs::write(&exe, b"MZ").unwrap();
    std::fs::remove_file(nucleus.path().join("handlers").join(setup.file())).unwrap();
    assert_eq!(
        listed_by_nucleus(nucleus.path()).unwrap(),
        Vec::<String>::new()
    );
}

#[test]
fn rubylights_games_go_from_the_appdata_list_and_nothing_else() {
    let appdata = tempfile::tempdir().unwrap();
    // No list: nothing to do.
    assert_eq!(forget_appdata_games(appdata.path()).unwrap(), 0);
    let path = appdata.path().join("Nucleus Coop").join("userprofile.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let users = "{\r\n  \"Games\": [\r\n    {\r\n      \"GameGuid\": \"Borderlands 2\",\r\n      \"ExePath\": \"D:\\\\BL2\\\\Borderlands2.exe\"\r\n    }\r\n  ]\r\n}";
    let mut text = users.to_string();
    for (exe, name) in [(r"D:\A\A.exe", "A"), (r"D:\B\B.exe", "B")] {
        let setup = Setup::new(exe, name, None);
        text = generic::add_game(Some(&text), &setup.guid(), exe)
            .unwrap()
            .unwrap();
    }
    std::fs::write(&path, &text).unwrap();
    assert_eq!(forget_appdata_games(appdata.path()).unwrap(), 2);
    // The user's game and the file's layout stay, byte for byte.
    assert_eq!(std::fs::read_to_string(&path).unwrap(), users);
    assert_eq!(forget_appdata_games(appdata.path()).unwrap(), 0);
    // A list the host can't read is left as it is, loudly.
    std::fs::write(&path, "{").unwrap();
    let error = format!("{:#}", forget_appdata_games(appdata.path()).unwrap_err());
    assert!(error.contains("userprofile.json"), "{error}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{");
}

/// The handler as written for the account running the tests.
fn ours(setup: &Setup) -> String {
    setup.handler_of(crate::process::user_sid().as_deref())
}

#[test]
fn rubylights_handler_is_written_and_its_game_added_beside_the_users() {
    let nucleus = tempfile::tempdir().unwrap();
    let handlers = nucleus.path().join("handlers");
    std::fs::create_dir_all(&handlers).unwrap();
    std::fs::write(handlers.join("Borderlands 2.js"), HANDLER).unwrap();
    let list = list(nucleus.path());
    std::fs::create_dir_all(list.parent().unwrap()).unwrap();
    let users = "{\r\n  \"Games\": [\r\n    {\r\n      \"GameGuid\": \"Borderlands 2\",\r\n      \"ExePath\": \"D:\\\\BL2\\\\Borderlands2.exe\"\r\n    }\r\n  ]\r\n}";
    std::fs::write(&list, users).unwrap();
    let setup = Setup::new(r"D:\Puzzle\Puzzle.exe", "Puzzle", None);

    let handler = write_own(nucleus.path(), &setup).unwrap();
    assert_eq!(handler.guid, setup.guid());
    assert!(handler.own);
    let file = handlers.join(setup.file());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), ours(&setup));
    assert_eq!(
        std::fs::read_to_string(handlers.join("Borderlands 2.js")).unwrap(),
        HANDLER
    );
    let written = std::fs::read_to_string(&list).unwrap();
    // The user's game, byte for byte, then ours.
    let end = users.len() - "\r\n  ]\r\n}".len();
    assert!(written.starts_with(&users[..end]), "{written}");
    assert!(written[end..].starts_with(",\r\n    {"), "{written}");
    // Nucleus reads both, and co-op still has only the hub's handler.
    let catalog = catalog(nucleus.path(), true).unwrap();
    assert_eq!(catalog.games.len(), 2);
    assert!(catalog.handlers.iter().any(|h| h.own));
    assert!(catalog.find("Puzzle", Some(&setup.exe_path)).is_err());

    // Again: nothing changes. A changed template is rewritten.
    write_own(nucleus.path(), &setup).unwrap();
    assert_eq!(std::fs::read_to_string(&list).unwrap(), written);
    std::fs::write(&file, format!("{}\r\n// v0", generic::MARKER)).unwrap();
    write_own(nucleus.path(), &setup).unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), ours(&setup));
    assert_eq!(std::fs::read_to_string(&list).unwrap(), written);

    // A file with our name that someone else wrote stays as it is.
    std::fs::write(&file, HANDLER).unwrap();
    let error = write_own(nucleus.path(), &setup).unwrap_err().to_string();
    assert!(error.contains("is not Rubylight's handler"), "{error}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), HANDLER);

    // A game list the host cannot read is left alone, loudly.
    std::fs::write(&list, "{").unwrap();
    std::fs::remove_file(&file).unwrap();
    let error = format!("{:#}", write_own(nucleus.path(), &setup).unwrap_err());
    assert!(error.contains("not readable"), "{error}");
    assert_eq!(std::fs::read_to_string(&list).unwrap(), "{");

    // No game list yet: its folder and file are made.
    let fresh = tempfile::tempdir().unwrap();
    write_own(fresh.path(), &setup).unwrap();
    let games =
        handler::user_games(&std::fs::read_to_string(super::list(fresh.path())).unwrap()).unwrap();
    assert_eq!(games[0].guid, setup.guid());
}

/// #56: an Epic game named by its bare .exe starts with what its launcher
/// would give it, so it doesn't restart through the launcher, on the PC's
/// monitor. Other games, and a broken manifest, change nothing.
#[test]
fn an_epic_games_handler_starts_it_as_its_launcher_would() {
    let manifests = tempfile::tempdir().unwrap();
    let item = |name: &str, text: &str| {
        std::fs::write(manifests.path().join(name), text).unwrap();
    };
    item("broken.item", "not json");
    item("other.txt", "not a manifest");
    item(
        "game.item",
        r#"{"AppName":"Jackdaw","CatalogNamespace":"plague","InstallLocation":"D:\\APlagueTale","LaunchExecutable":"APlagueTaleInnocence_x64.exe","LaunchCommand":""}"#,
    );
    let started = |exe: &str, arguments: &str| {
        let setup = Setup {
            arguments: arguments.into(),
            ..Setup::new(exe, "Game", None)
        };
        start_arguments(&setup, manifests.path())
    };
    let exe = r"d:/aplaguetale/aplaguetaleinnocence_x64.EXE";
    let epic = "-EpicPortal -epicapp=Jackdaw -epicenv=Prod -epicsandboxid=plague";
    assert_eq!(started(exe, ""), epic);
    assert_eq!(started(exe, "-x"), format!("-x {epic}"));
    // An app's own arguments that already say so stay as they are.
    assert_eq!(started(exe, "-epicportal -y"), "-epicportal -y");
    assert_eq!(started(r"D:\Games\Other\Other.exe", "-x"), "-x");
    assert!(epic_arguments(manifests.path(), exe).is_some());
    assert!(epic_arguments(&manifests.path().join("none"), exe).is_none());
}

/// A manifest too large for one, or a link to another file, is never read.
#[test]
fn an_epic_manifest_is_read_only_at_its_size_and_never_through_a_link() {
    let manifests = tempfile::tempdir().unwrap();
    let good = manifests.path().join("good.json");
    let text = r#"{"AppName":"App","CatalogNamespace":"Space","InstallLocation":"D:\\Game","LaunchExecutable":"Game.exe"}"#;
    std::fs::write(&good, text).unwrap();
    assert_eq!(read_epic_manifest(&good).unwrap(), text.as_bytes());
    let large = manifests.path().join("large.item");
    std::fs::write(&large, vec![b' '; EPIC_MANIFEST_MAX as usize + 1]).unwrap();
    assert!(read_epic_manifest(&large).unwrap_err().contains("over"));
    let exe = r"D:\Game\Game.exe";
    assert!(epic_arguments(manifests.path(), exe).is_none());
    // Linking takes a privilege that a developer's own shell may lack.
    let link = manifests.path().join("link.item");
    match std::os::windows::fs::symlink_file(&good, &link) {
        Ok(()) => {
            assert!(read_epic_manifest(&link).unwrap_err().contains("link"));
            assert!(epic_arguments(manifests.path(), exe).is_none());
        }
        Err(error) => println!("no link to test: {error}"),
    }
}

/// #56: an Unreal game's handler starts it in a window, so it stays where
/// it is parked instead of covering the PC's monitor.
#[test]
fn an_unreal_games_handler_starts_it_in_a_window() {
    let nucleus = tempfile::tempdir().unwrap();
    let sized = |exe: &str, size: Option<(u32, u32)>| {
        let setup = Setup {
            arguments: "-x".into(),
            size,
            ..Setup::new(exe, "Game", None)
        };
        write_own(nucleus.path(), &setup).unwrap();
        let text =
            std::fs::read_to_string(nucleus.path().join("handlers").join(setup.file())).unwrap();
        text.lines()
            .find_map(|l| l.strip_prefix("Game.StartArguments = "))
            .unwrap_or_default()
            .to_string()
    };
    let arguments = |exe: &str| sized(exe, None);
    let unreal = r"D:\Games\Game\Game\Binaries\Win64\Game-Win64-Shipping.exe";
    assert_eq!(arguments(unreal), r#""-x -windowed";"#);
    // Any other game starts as it would.
    assert_eq!(arguments(r"D:\Games\Other\Other.exe"), r#""-x";"#);
    // At the stream's size: the phone's 20:9, not the game's saved 16:9.
    assert_eq!(
        sized(unreal, Some((2400, 1080))),
        r#""-x -windowed -ResX=2400 -ResY=1080 -ForceRes";"#
    );
}

/// #59: a game that refused the way ProtoInput's hooks were injected
/// gets a handler with the next way at its next start, down to none,
/// also after the host restarts (the file in the host's Nucleus folder
/// keeps it); other games keep the first.
#[test]
fn a_game_that_refused_the_hooks_gets_the_next_way_down_to_none() {
    let nucleus = tempfile::tempdir().unwrap();
    let setup = Setup::new(r"D:\Games\Black Flag\ACBlackFlag.exe", "Black Flag", None);
    let other = Setup::new(r"D:\Games\Other\Game.exe", "Other", None);
    let file = nucleus.path().join("handlers").join(setup.file());
    let kept = nucleus.path().join("Rubylight-injection-2.json");
    let written = |way: Inject| {
        write_own(nucleus.path(), &setup).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(generic::owns(&text));
        let wanted = Setup {
            inject: way,
            ..setup.clone()
        };
        let owner = crate::process::user_sid().unwrap();
        assert_eq!(text, wanted.handler_of(Some(&owner)), "{way:?}");
    };
    written(Inject::Runtime);
    assert_eq!(
        inject_for(nucleus.path(), &setup.exe_path).unwrap(),
        Inject::Runtime
    );
    for next in [Inject::Startup, Inject::Stealth, Inject::None] {
        // By its path, in any case.
        let exe = match next {
            Inject::Stealth => setup.exe_path.to_uppercase(),
            _ => setup.exe_path.clone(),
        };
        assert_eq!(refuse_inject(nucleus.path(), &exe).unwrap(), next);
        let text = std::fs::read_to_string(&kept).unwrap();
        assert_eq!(text.matches("acblackflag.exe").count(), 1, "{text}");
        assert_eq!(inject_for(nucleus.path(), &setup.exe_path).unwrap(), next);
        written(next);
    }
    // None is the last.
    assert_eq!(
        refuse_inject(nucleus.path(), &setup.exe_path).unwrap(),
        Inject::None
    );
    write_own(nucleus.path(), &other).unwrap();
    let other_text =
        std::fs::read_to_string(nucleus.path().join("handlers").join(other.file())).unwrap();
    assert!(other_text.contains("Game.ProtoInput.InjectRuntime_EasyHookMethod = true;"));
    // What the files before held were mostly Nucleus' double starts (#59): a
    // game kept there starts at the first way again.
    let older = tempfile::tempdir().unwrap();
    let game = r#"{"Games": {"d:\\games\\black flag\\acblackflag.exe": "none"}}"#;
    std::fs::write(older.path().join("Rubylight-injection.json"), game).unwrap();
    let list = r#"{"Games": ["d:\\games\\black flag\\acblackflag.exe"]}"#;
    std::fs::write(older.path().join("Rubylight-unhooked.json"), list).unwrap();
    assert_eq!(
        inject_for(older.path(), &setup.exe_path).unwrap(),
        Inject::default()
    );
    // A file Rubylight can't read is loud, never taken for the first way.
    for broken in [
        "{",
        r#"{"Games": []}"#,
        r#"{"Games": {"d:\\a.exe": "sideways"}}"#,
    ] {
        std::fs::write(&kept, broken).unwrap();
        let error = format!("{:#}", write_own(nucleus.path(), &setup).unwrap_err());
        assert!(error.contains("delete it"), "{broken}: {error}");
    }
}

#[test]
fn ours_goes_once_the_user_adds_the_game_with_a_hub_handler() {
    let nucleus = tempfile::tempdir().unwrap();
    let handlers = nucleus.path().join("handlers");
    let setup = Setup::new(r"D:\BL2\Borderlands2.exe", "Borderlands 2", None);
    write_own(nucleus.path(), &setup).unwrap();
    // The hub's handler comes, and the user adds the game with it.
    std::fs::write(handlers.join("Borderlands 2.js"), HANDLER).unwrap();
    let list = list(nucleus.path());
    let ours = std::fs::read_to_string(&list).unwrap();
    std::fs::write(
        &list,
        generic::add_game(Some(&ours), "Borderlands 2", &setup.exe_path)
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let before = catalog(nucleus.path(), true).unwrap();
    let superseded = before.superseded(None);
    assert_eq!(superseded.len(), 1);
    remove_own(nucleus.path(), superseded[0]).unwrap();
    assert!(!handlers.join(setup.file()).exists());
    let after = catalog(nucleus.path(), true).unwrap();
    assert!(after.handlers.iter().all(|h| !h.own));
    assert_eq!(
        after
            .games
            .iter()
            .map(|g| g.guid.as_str())
            .collect::<Vec<_>>(),
        ["Borderlands 2"]
    );
    // A hub handler is never removed as ours.
    let error = remove_own(nucleus.path(), &after.handlers[0]).unwrap_err();
    assert!(
        error.to_string().contains("is not Rubylight's handler"),
        "{error}"
    );
    assert!(handlers.join("Borderlands 2.js").exists());
}

#[test]
fn a_handler_that_cannot_be_removed_keeps_its_game_and_is_found_again() {
    use std::os::windows::fs::OpenOptionsExt;
    let nucleus = tempfile::tempdir().unwrap();
    let setup = Setup::new(r"D:\Puzzle\Game.exe", "Puzzle", None);
    write_own(nucleus.path(), &setup).unwrap();
    let file = nucleus.path().join("handlers").join(setup.file());
    let ours = || {
        catalog(nucleus.path(), true)
            .unwrap()
            .handlers
            .into_iter()
            .find(|h| h.own)
    };
    let own = ours().unwrap();
    // Open elsewhere without sharing: the removal fails, loudly.
    let open = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&file)
        .unwrap();
    let error = format!("{:#}", remove_own(nucleus.path(), &own).unwrap_err());
    assert!(error.contains(&setup.file()), "{error}");
    drop(open);
    // The game stays listed with it, so the next start finds both again.
    let left = catalog(nucleus.path(), true).unwrap();
    assert!(left.games.iter().any(|g| g.guid == setup.guid()));
    assert!(ours().is_some());
    remove_own(nucleus.path(), &own).unwrap();
    let after = catalog(nucleus.path(), true).unwrap();
    assert!(!file.exists());
    assert!(after.games.is_empty());
}

#[test]
fn an_unused_handler_of_ours_is_swept_and_a_foreign_one_never() {
    let nucleus = tempfile::tempdir().unwrap();
    let setup = Setup::new(r"D:\Puzzle\Game.exe", "Puzzle", None);
    write_own(nucleus.path(), &setup).unwrap();
    // A removal that took the game but not the handler, as before.
    let list = list(nucleus.path());
    let text = std::fs::read_to_string(&list).unwrap();
    std::fs::write(
        &list,
        generic::remove_game(&text, &setup.guid()).unwrap().unwrap(),
    )
    .unwrap();
    let handlers = nucleus.path().join("handlers");
    std::fs::write(handlers.join("Other.js"), HANDLER).unwrap();
    let before = catalog(nucleus.path(), true).unwrap();
    let stale = before.stale(None);
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0].guid, setup.guid());
    remove_own(nucleus.path(), stale[0]).unwrap();
    assert!(!handlers.join(setup.file()).exists());
    assert!(handlers.join("Other.js").exists());
    let after = catalog(nucleus.path(), true).unwrap();
    assert!(after.stale(None).is_empty());
}

#[test]
fn ours_another_account_or_an_earlier_build_wrote_stays() {
    let nucleus = tempfile::tempdir().unwrap();
    let handlers = nucleus.path().join("handlers");
    std::fs::create_dir_all(&handlers).unwrap();
    // Another account wrote one, which its sweep removes; an earlier
    // build wrote one without an owner.
    let theirs = Setup::new(r"D:\Theirs\Game.exe", "Theirs", None);
    std::fs::write(
        handlers.join(theirs.file()),
        theirs.handler_of(Some("S-1-5-21-9-9-9-1002")),
    )
    .unwrap();
    let earlier = Setup::new(r"D:\Earlier\Game.exe", "Earlier", None);
    std::fs::write(handlers.join(earlier.file()), earlier.handler()).unwrap();
    // This account's, whose entry is gone.
    let mine = Setup::new(r"D:\Mine\Game.exe", "Mine", None);
    write_own(nucleus.path(), &mine).unwrap();
    let list = list(nucleus.path());
    let text = std::fs::read_to_string(&list).unwrap();
    std::fs::write(
        &list,
        generic::remove_game(&text, &mine.guid()).unwrap().unwrap(),
    )
    .unwrap();
    let catalog = catalog(nucleus.path(), true).unwrap();
    assert_eq!(catalog.user, crate::process::user_sid());
    let stale: Vec<&str> = catalog
        .stale(None)
        .iter()
        .map(|h| h.guid.as_str())
        .collect();
    assert_eq!(stale, [mine.guid()]);
    // Without a signed-in user's list, none.
    let nobody = super::catalog(nucleus.path(), false).unwrap();
    assert!(nobody.user.is_none());
    assert!(nobody.stale(None).is_empty());
}

#[test]
fn a_game_entry_left_after_its_handler_went_is_taken_over_again() {
    use std::os::windows::fs::OpenOptionsExt;
    let nucleus = tempfile::tempdir().unwrap();
    let setup = Setup::new(r"D:\Puzzle\Game.exe", "Puzzle", None);
    let own = write_own(nucleus.path(), &setup).unwrap();
    let list = list(nucleus.path());
    let open = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&list)
        .unwrap();
    let error = format!("{:#}", remove_own(nucleus.path(), &own).unwrap_err());
    drop(open);
    // The warning says what stays and why it does no harm.
    assert!(error.contains("still lists its game"), "{error}");
    assert!(
        error.contains("leaves a game whose handler is gone out"),
        "{error}"
    );
    let left = catalog(nucleus.path(), true).unwrap();
    assert!(left.handlers.iter().all(|h| !h.own));
    assert!(left.stale(None).is_empty());
    assert!(left.playable().is_empty());
    // The game's next background stream writes the handler and keeps
    // the one entry.
    write_own(nucleus.path(), &setup).unwrap();
    let again = catalog(nucleus.path(), true).unwrap();
    assert_eq!(
        again
            .games
            .iter()
            .filter(|g| g.guid == setup.guid())
            .count(),
        1
    );
    assert!(again.handlers.iter().any(|h| h.own));
}
