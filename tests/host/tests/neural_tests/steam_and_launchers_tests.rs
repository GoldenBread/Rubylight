//! Steam Valve Anti-Cheat detection and launcher/interpreter exclusion tests for Neural Rendering.
use super::common::install;
use super::common::*;
use butterpollo_session::neural::Gpu;
use butterpollo_stream::neural::folder::Response;
use butterpollo_stream::neural::*;
use serde_json::json;
use std::{path::PathBuf, sync::Arc};

/// A Steam folder with app 242050 installed to `Black Flag` and its Valve
/// Anti-Cheat flag `vac` in the app cache (format 28).
fn steam_root(vac: &str) -> Temp {
    let root = tempdir();
    let apps = root.path().join("steamapps");
    std::fs::create_dir_all(apps.join(r"common\Black Flag")).unwrap();
    std::fs::write(
        apps.join("appmanifest_242050.acf"),
        "\"AppState\"\n{\n\t\"appid\"\t\t\"242050\"\n\t\"installdir\"\t\t\"Black Flag\"\n}\n",
    )
    .unwrap();
    let mut kv = Vec::new();
    for key in ["appinfo", "common", "category"] {
        kv.push(0x00);
        kv.extend(key.as_bytes());
        kv.push(0);
    }
    kv.push(0x01);
    kv.extend(b"category_8\0");
    kv.extend(vac.as_bytes());
    kv.push(0);
    kv.extend([0x08; 5]);
    let mut data = 0x0756_4428u32.to_le_bytes().to_vec();
    data.extend(1u32.to_le_bytes());
    data.extend(242050u32.to_le_bytes());
    data.extend(((60 + kv.len()) as u32).to_le_bytes());
    data.extend([0; 60]);
    data.extend(kv);
    data.extend(0u32.to_le_bytes());
    std::fs::create_dir_all(root.path().join("appcache")).unwrap();
    std::fs::write(root.path().join(r"appcache\appinfo.vdf"), data).unwrap();
    root
}

/// A Steam game Steam lists Valve Anti-Cheat for is refused, by its link or
/// by an .exe in its library, though no file in its install shows it; so is
/// one Steam can't say about. A game in a plain folder is not asked about.
#[test]
fn a_game_steam_lists_valve_anti_cheat_for_is_refused() {
    let (vac, clean) = (steam_root("1"), steam_root("0"));
    let mut link = app_named("Black Flag", "flag");
    link.cmd = "steam://rungameid/242050".into();
    let direct = app_named("Black Flag", "flag");
    for (steam, listed) in [(&vac, true), (&clean, false)] {
        let roots = [steam.path().to_path_buf()];
        let exe = steam
            .path()
            .join(r"steamapps\common\Black Flag\AC4BFSP.exe");
        for app in [&link, &direct] {
            let refusal = steam_refusal(app, std::slice::from_ref(&exe), &roots);
            assert_eq!(refusal.is_some(), listed, "{}: {refusal:?}", app.cmd);
            if let Some(reason) = refusal {
                assert!(
                    reason.starts_with("Steam lists Valve Anti-Cheat"),
                    "{reason}"
                );
            }
        }
        let plain = steam.path().join(r"Games\Game.exe");
        assert_eq!(steam_refusal(&direct, &[plain], &roots), None);
    }
    let reason = steam_refusal(&link, &[vac.path().join("Game.exe")], &[]).unwrap();
    assert!(
        reason.starts_with("Rubylight can't tell whether the game uses Valve Anti-Cheat"),
        "{reason}"
    );
}

/// A Steam game's folder moved elsewhere and linked back into the library
/// (`mklink /J`) is still Steam's: where the link leads is a plain folder,
/// but the paths that reached it are asked too (the chosen program, the
/// app's command, its install), for an install and for a launch.
#[test]
fn a_games_folder_linked_back_into_steams_library_is_still_asked_about() {
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let h = &f.host;
    for (flag, refused) in [("1", true), ("0", false)] {
        let steam = steam_root(flag);
        let roots = vec![steam.path().to_path_buf()];
        *h.neural.steam_roots.lock().unwrap() = Some(roots.clone());
        let moved = tempdir();
        std::fs::write(moved.path().join("AC4BFSP.exe"), b"MZ game").unwrap();
        std::fs::write(moved.path().join("sl.dlss_g.dll"), b"Streamline").unwrap();
        let linked = steam.path().join(r"steamapps\common\Black Flag");
        std::fs::remove_dir(&linked).unwrap();
        junction(&linked, moved.path());
        let through = linked.join("AC4BFSP.exe");
        let followed = folder::real(moved.path()).unwrap().join("AC4BFSP.exe");

        let chosen = app_named("Black Flag", "flag");
        // Before the fix only where the link leads was asked.
        assert_eq!(
            steam_refusal(&chosen, std::slice::from_ref(&followed), &roots),
            None
        );
        let mut command = app_named("Black Flag", "flag");
        command.cmd = format!("\"{}\"", through.display());
        let mut installed = app_named("Black Flag", "flag");
        installed
            .extra
            .insert("launcher-install-dir".into(), json!(linked));
        for (app, requested, launch) in [
            (&chosen, Some(through.clone()), false),
            // The user chose the program where the link leads.
            (&command, Some(followed.clone()), true),
            (&installed, None, true),
        ] {
            let response =
                find_game(h, app, requested, butterpollo_stream::neural::install(app)).unwrap();
            match (&response, refused) {
                (Response::Refused { reason }, true) => {
                    assert!(
                        reason.starts_with("Steam lists Valve Anti-Cheat"),
                        "{reason}"
                    );
                }
                (Response::Found { exe }, false) => assert_eq!(*exe, followed),
                _ => panic!("{}: {response:?}", app.cmd),
            }
            if launch {
                let mut on = app.clone();
                on.extra.insert(KEY.into(), json!({"exe": followed}));
                let error = before_launch(h, &on).err().map(|e| format!("{e:#}"));
                assert_eq!(
                    error
                        .as_deref()
                        .is_some_and(|e| e.contains("Valve Anti-Cheat")),
                    refused,
                    "{}: {error:?}",
                    app.cmd
                );
            }
        }
        std::fs::remove_dir(&linked).unwrap();
    }
}

/// A game Steam lists Valve Anti-Cheat for is refused before the download:
/// GitHub is never asked.
#[tokio::test(flavor = "multi_thread")]
async fn a_vac_game_is_refused_before_the_download() {
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let h = &f.host;
    *h.neural.probe.lock().unwrap() = Some(Probe {
        gpus: vec![Gpu {
            name: "AMD Radeon RX 7900 XT".into(),
            arch: "gfx1100".into(),
            compute_units: 84,
            luid: None,
        }],
        error: None,
    });
    accept(h).unwrap();
    let steam = steam_root("1");
    *h.neural.steam_roots.lock().unwrap() = Some(vec![steam.path().to_path_buf()]);
    let folder = steam.path().join(r"steamapps\common\Black Flag");
    std::fs::write(folder.join("AC4BFSP.exe"), b"MZ game").unwrap();
    std::fs::write(folder.join("sl.dlss_g.dll"), b"Streamline").unwrap();
    butterpollo_stream::steam::update_library(h, |apps, _| {
        apps.push(json!({"name": "Black Flag", "uuid": "flag", "launcher-install-dir": folder}));
        true
    })
    .unwrap();
    let files = anr_files();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let list = release_list(&base, TAG, &files);
    pin_files(h, &files);
    let server = serve(listener, list, Arc::new(files)).await;
    *h.neural.origin.lock().unwrap() = Some(server.base);

    enable(h, "flag", None).unwrap();
    let job = ended(h, "flag").await;
    assert_eq!(job.phase, "failed", "{job:?}");
    let error = job.error.unwrap();
    assert!(error.starts_with("Steam lists Valve Anti-Cheat"), "{error}");
    assert!(server.log.lock().unwrap().is_empty());
    assert!(cached(&cache_root(h)).is_none());
    assert_eq!(names(&folder), ["AC4BFSP.exe", "sl.dlss_g.dll"]);
}

/// A launcher is never the game: an app whose command starts Steam or
/// another store has the game's program found in its install, a launcher
/// picked is refused, and so is a program outside the game's install.
#[test]
fn the_mod_never_goes_beside_a_launcher_or_outside_the_install() {
    let temp = tempdir();
    let cache = release(temp.path(), "Alpha0.3.5.8", b"OptiScaler proxy");
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    let launchers = temp.path().join("Launchers");
    std::fs::create_dir(&launchers).unwrap();
    let mut app = app_named("Game", "game");
    app.extra.insert("steam-install-dir".into(), json!(folder));
    for name in [
        "steam.exe",
        "upc.exe",
        "UbisoftConnect.exe",
        "EpicGamesLauncher.exe",
        "Battle.net.exe",
        "EADesktop.exe",
        "GalaxyClient.exe",
        "Playnite.DesktopApp.exe",
    ] {
        let launcher = launchers.join(name);
        std::fs::write(&launcher, "MZ").unwrap();
        let response = install(Some(&launcher), None, &cache);
        assert!(
            matches!(response, Response::ChooseExe { .. }),
            "{name}: {response:?}"
        );
        app.cmd = format!("\"{}\" -applaunch 620", launcher.display());
        assert_eq!(game_command(&app), None, "{name}");
    }
    assert_eq!(names(&launchers).len(), 8, "nothing beside a launcher");

    let other = temp.path().join(r"Other\Other.exe");
    std::fs::create_dir_all(other.parent().unwrap()).unwrap();
    std::fs::write(&other, "MZ").unwrap();
    let Response::ChooseExe { reason } = install(Some(&other), Some(folder), &cache) else {
        panic!("a program outside the install was taken")
    };
    assert!(reason.contains("not in the game's folder"), "{reason}");
    app.cmd = format!("\"{}\"", other.display());
    assert_eq!(game_command(&app), None);
    app.cmd = format!("\"{}\"", exe.display());
    assert_eq!(game_command(&app), Some(exe.clone()));
    // With no install known, nothing is taken unasked.
    app.extra.remove("steam-install-dir");
    assert_eq!(game_command(&app), None);
    app.cmd = format!("\"{}\"", other.display());
    assert_eq!(game_command(&app), None);
}

/// A script host, an interpreter, another store's launcher or a program of
/// Windows is never the game, picked or in the app's command: an app that
/// runs `pwsh.exe -File x.ps1` with no install known asks for the game's
/// program, and nothing is written beside any of them.
#[test]
fn an_interpreter_or_windows_program_is_never_the_game() {
    let temp = tempdir();
    let cache = release(temp.path(), "Alpha0.3.5.8", b"OptiScaler proxy");
    let tools = temp.path().join("Tools");
    std::fs::create_dir(&tools).unwrap();
    let names_ = [
        "pwsh.exe",
        "powershell.exe",
        "python.exe",
        "python3.12.exe",
        "javaw.exe",
        "node.exe",
        "Origin.exe",
        "EALauncher.exe",
        "Launcher.exe",
        "Amazon Games.exe",
        "itch.exe",
        "heroic.exe",
    ];
    for name in names_ {
        let program = tools.join(name);
        std::fs::write(&program, "MZ").unwrap();
        let Response::ChooseExe { reason } = install(Some(&program), None, &cache) else {
            panic!("{name} was taken")
        };
        assert!(reason.contains("not the game"), "{reason}");
    }
    assert_eq!(names(&tools).len(), names_.len(), "nothing beside them");
    let windows =
        PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join(r"System32\notepad.exe");
    let Response::ChooseExe { reason } = install(Some(&windows), None, &cache) else {
        panic!("a program of Windows was taken")
    };
    assert!(reason.contains("part of Windows"), "{reason}");

    // An install whose only top program is an interpreter has no game to pick.
    let bundled = temp.path().join("Bundled");
    std::fs::create_dir(&bundled).unwrap();
    std::fs::write(bundled.join("python.exe"), "MZ").unwrap();
    assert!(matches!(
        install(None, Some(&bundled), &cache),
        Response::ChooseExe { .. }
    ));

    // The switch's request for an app that runs a script, its install
    // unknown, as `enable` makes it.
    let mut app = app_named("Script", "script");
    app.cmd = format!("\"{}\" -File game.ps1", tools.join("pwsh.exe").display());
    assert_eq!(game_command(&app), None);
    let Response::ChooseExe { reason } = install(
        game_command(&app).as_deref(),
        butterpollo_stream::neural::install(&app).as_deref(),
        &cache,
    ) else {
        panic!("a program was taken for a script")
    };
    assert!(reason.contains("doesn't know where"), "{reason}");
    assert_eq!(names(&tools).len(), names_.len());
}
