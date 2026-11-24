//! Placement, hash verification, executable discovery, and anti-cheat refusal tests for Neural Rendering.
use super::common::*;
use butterpollo_session::neural::{MANIFEST, Manifest, Mode, Placed};
use butterpollo_stream::neural::folder::{PACKAGE, Package, Request, Response, Writer, handle};
use std::path::{Path, PathBuf};

#[test]
fn the_mod_goes_beside_the_game_under_a_free_name_and_leaves_with_only_its_own_files() {
    let temp = tempdir();
    let cache = release(temp.path(), "Alpha0.3.5.8", b"OptiScaler proxy");
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    let before = names(folder);
    let game_files = snapshot(folder);

    // ReShade holds dxgi.dll: OptiScaler takes the next name and leaves it
    // alone, and the game's own DLSS frame generation stays as it is.
    assert_eq!(
        install(Some(&exe), None, &cache),
        Response::Placed {
            exe: exe.clone(),
            release: "Alpha0.3.5.8".into(),
            proxy: "winmm.dll".into(),
            updated: false,
            mode: Mode::Full,
        }
    );
    assert_eq!(std::fs::read(folder.join("dxgi.dll")).unwrap(), b"ReShade");
    assert_eq!(
        std::fs::read(folder.join("winmm.dll")).unwrap(),
        b"OptiScaler proxy"
    );
    assert_eq!(
        std::fs::read(folder.join("nvngx_dlssg.dll")).unwrap(),
        b"NVIDIA's DLSS frame generation"
    );
    assert_eq!(std::fs::read(folder.join(PDLL)).unwrap(), b"provider");
    let provider = "OptiScaler/custom/Apex Neural Rendering FG";
    let dlssg = format!("{provider}/dlssg");
    let kernels = format!("{provider}/dlssg/kernels");
    let mut expected = before.clone();
    expected.extend(
        [
            "OptiScaler",
            "OptiScaler.ini",
            "OptiScaler/LICENSES",
            "OptiScaler/LICENSES/OptiScaler-GPL-3.0.txt",
            "OptiScaler/custom",
            provider,
            dlssg.as_str(),
            kernels.as_str(),
            PDLL,
            PINI,
            PAK,
            "OptiScaler/libxess.dll",
            "winmm.dll",
            MANIFEST,
        ]
        .map(|name: &str| name.to_owned()),
    );
    expected.sort();
    assert_eq!(names(folder), expected);
    let manifest =
        Manifest::parse(&std::fs::read_to_string(folder.join(MANIFEST)).unwrap()).unwrap();
    assert_eq!(
        (manifest.mode, manifest.proxy.as_str()),
        (Mode::Full, "winmm.dll")
    );
    assert!(manifest.ini.is_none());
    assert!(manifest.files.contains_key(PDLL));
    assert!(!manifest.files.keys().any(|path| path.starts_with("addon")));
    assert!(manifest.folders.iter().any(|f| f == "OptiScaler"));

    // The menu saved settings, OptiScaler and the provider logged, someone
    // replaced the provider and put a file of their own in the folder.
    std::fs::write(
        folder.join("OptiScaler.ini"),
        "[FrameGen]\nFGInput=nvngxfg\nEnabled=true\n",
    )
    .unwrap();
    std::fs::write(folder.join(PINI), "path = gpu\n").unwrap();
    std::fs::write(folder.join("OptiScaler.log"), "log").unwrap();
    std::fs::write(folder.join(format!("{provider}/dlssg/nr_fg.log")), "log").unwrap();
    std::fs::write(folder.join(PDLL), "someone else's").unwrap();
    std::fs::write(folder.join("OptiScaler/user.txt"), "mine").unwrap();
    // Both stay, and the removal names both: the changed provider and the
    // file someone else put in a folder the host made.
    assert_eq!(
        remove(&exe),
        Response::Removed {
            kept: vec![PDLL.into(), "OptiScaler/user.txt".into()]
        }
    );
    let mut expected = before.clone();
    expected.extend(
        [
            "OptiScaler",
            "OptiScaler/custom",
            provider,
            dlssg.as_str(),
            PDLL,
            "OptiScaler/user.txt",
        ]
        .map(|name: &str| name.to_owned()),
    );
    expected.sort();
    assert_eq!(names(folder), expected);
    // Nothing left to remove.
    assert_eq!(remove(&exe), Response::Removed { kept: vec![] });

    // Without changes, the folder is again byte for byte as it was.
    std::fs::remove_dir_all(folder.join("OptiScaler")).unwrap();
    assert!(matches!(
        install(Some(&exe), None, &cache),
        Response::Placed { .. }
    ));
    assert_eq!(remove(&exe), Response::Removed { kept: vec![] });
    assert_eq!(snapshot(folder), game_files);
}

#[test]
fn a_game_with_anti_cheat_or_another_upscaler_mod_is_refused() {
    let temp = tempdir();
    let cache = release(temp.path(), "Alpha0.3.5.8", b"OptiScaler proxy");
    // An Unreal game: anti-cheat two folders above the .exe.
    let install_dir = temp.path().join("Unreal");
    let binaries = install_dir.join("Game/Binaries/Win64");
    std::fs::create_dir_all(&binaries).unwrap();
    std::fs::create_dir_all(install_dir.join("EasyAntiCheat")).unwrap();
    let exe = binaries.join("Game-Win64-Shipping.exe");
    std::fs::write(&exe, "MZ").unwrap();
    let Response::Refused { reason } = install(None, Some(&install_dir), &cache) else {
        panic!()
    };
    assert!(reason.contains("Easy Anti-Cheat"), "{reason}");
    // Without its install folder too.
    assert!(matches!(
        install(Some(&exe), None, &cache),
        Response::Refused { .. }
    ));
    assert_eq!(names(&binaries), ["Game-Win64-Shipping.exe"]);

    let exe = game(temp.path());
    std::fs::write(exe.with_file_name("OptiScaler.ini"), "").unwrap();
    let Response::Refused { reason } = install(Some(&exe), None, &cache) else {
        panic!()
    };
    assert!(reason.contains("OptiScaler"), "{reason}");
    std::fs::remove_file(exe.with_file_name("OptiScaler.ini")).unwrap();
    // The user's own OptiScaler that can't load another frame generation.
    std::fs::write(exe.with_file_name("winmm.dll"), "MZ ... OptiScaler ...").unwrap();
    let Response::Refused { reason } = install(Some(&exe), None, &cache) else {
        panic!()
    };
    assert!(
        reason.contains("can't load another frame generation"),
        "{reason}"
    );
    assert!(!exe.with_file_name(MANIFEST).exists());
}

#[test]
fn a_name_taken_by_the_game_is_never_overwritten() {
    let temp = tempdir();
    let cache = release(temp.path(), "Alpha0.3.5.8", b"OptiScaler proxy");
    let exe = game(temp.path());
    let folder = exe.parent().unwrap().to_path_buf();
    for name in butterpollo_session::neural::PROXIES {
        std::fs::write(folder.join(name), format!("game's {name}")).unwrap();
    }
    let Response::Refused { reason } = install(Some(&exe), None, &cache) else {
        panic!()
    };
    assert!(reason.contains("every name"), "{reason}");
    std::fs::remove_file(folder.join("version.dll")).unwrap();
    // A folder of the package the game has itself.
    std::fs::create_dir(folder.join("OptiScaler")).unwrap();
    let Response::Refused { reason } = install(Some(&exe), None, &cache) else {
        panic!()
    };
    assert!(reason.contains("OptiScaler"), "{reason}");
    for name in butterpollo_session::neural::PROXIES
        .iter()
        .filter(|n| **n != "version.dll")
    {
        assert_eq!(
            std::fs::read_to_string(folder.join(name)).unwrap(),
            format!("game's {name}")
        );
    }
    assert!(!folder.join(MANIFEST).exists());
}

#[test]
fn a_cached_file_that_changed_never_reaches_the_game() {
    let temp = tempdir();
    let cache = release(temp.path(), "Alpha0.3.5.8", b"OptiScaler proxy");
    std::fs::write(cache.join(format!("files/full/game/{PAK}")), "tampered").unwrap();
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    let before = names(folder);
    let Response::Error { message } = install(Some(&exe), None, &cache) else {
        panic!()
    };
    assert!(message.contains("does not match"), "{message}");
    // The first install goes again at once: nothing of it stays.
    assert_eq!(names(folder), before);
    assert_eq!(remove(&exe), Response::Removed { kept: vec![] });
    assert_eq!(names(folder), before);
}

/// The helper copies against the hashes the host holds, never the cache's
/// package.json read again: a file changed with the record rewritten to
/// match after the host read it still never reaches the game. A package
/// naming a file outside the cache is refused.
#[test]
fn the_helper_checks_the_files_against_the_hashes_the_host_holds() {
    let temp = tempdir();
    let cache = release(temp.path(), "Alpha0.3.5.8", b"OptiScaler proxy");
    let held = cached_at(&cache);
    std::fs::write(cache.join(format!("files/full/game/{PAK}")), "tampered").unwrap();
    let mut rewritten = held.package.clone();
    rewritten.files.insert(
        format!("full/game/{PAK}"),
        Placed {
            sha256: sha(b"tampered"),
            size: 8,
        },
    );
    std::fs::write(
        cache.join(PACKAGE),
        serde_json::to_string(&rewritten).unwrap(),
    )
    .unwrap();
    let exe = game(temp.path());
    let before = names(exe.parent().unwrap());
    let install = |package| {
        handle(
            Request::Install {
                exe: Some(exe.clone()),
                install: None,
                package,
            },
            Writer::User,
            &|_, _| {},
        )
    };
    let Response::Error { message } = install(held.clone()) else {
        panic!("a changed cache was copied")
    };
    assert!(message.contains("does not match its release"), "{message}");
    assert_eq!(names(exe.parent().unwrap()), before);
    let mut outside = held;
    outside.package.files.insert(
        "../evil.dll".into(),
        Placed {
            sha256: sha(b"evil"),
            size: 4,
        },
    );
    let Response::Error { message } = install(outside) else {
        panic!("a file outside the cache was named")
    };
    assert!(message.contains("names a file outside it"), "{message}");
    assert_eq!(names(exe.parent().unwrap()), before);
}

/// A launch never applies another release on its own; turning the switch
/// on again does, and keeps the user's settings. A changed file fails the
/// launch.
#[test]
fn only_an_install_applies_another_release_and_a_launch_fails_on_a_changed_file() {
    let temp = tempdir();
    let old = release(temp.path(), "Alpha0.3.5.7", b"OptiScaler old");
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    std::fs::remove_file(folder.join("dxgi.dll")).unwrap();
    // The older release had a file the newer one dropped.
    std::fs::write(old.join("files/full/game/old.dll"), "old").unwrap();
    let mut package = Package::read(&old).unwrap();
    package.files.insert(
        "full/game/old.dll".into(),
        Placed {
            sha256: sha(b"old"),
            size: 3,
        },
    );
    std::fs::write(old.join(PACKAGE), serde_json::to_string(&package).unwrap()).unwrap();
    assert!(matches!(
        install(Some(&exe), None, &old),
        Response::Placed { .. }
    ));
    assert_eq!(
        prepare(&exe, Some(&old)),
        Response::Placed {
            exe: exe.clone(),
            release: "Alpha0.3.5.7".into(),
            proxy: "dxgi.dll".into(),
            updated: false,
            mode: Mode::Full,
        }
    );
    std::fs::write(folder.join("OptiScaler.ini"), "mine").unwrap();

    let new = release(&temp.path().join("new"), "Alpha0.3.5.8", b"OptiScaler new");
    assert_eq!(
        prepare(&exe, Some(&new)),
        Response::Placed {
            exe: exe.clone(),
            release: "Alpha0.3.5.7".into(),
            proxy: "dxgi.dll".into(),
            updated: false,
            mode: Mode::Full,
        }
    );
    assert_eq!(
        std::fs::read(folder.join("dxgi.dll")).unwrap(),
        b"OptiScaler old"
    );
    assert_eq!(
        install(Some(&exe), None, &new),
        Response::Placed {
            exe: exe.clone(),
            release: "Alpha0.3.5.8".into(),
            proxy: "dxgi.dll".into(),
            updated: true,
            mode: Mode::Full,
        }
    );
    assert_eq!(
        std::fs::read(folder.join("dxgi.dll")).unwrap(),
        b"OptiScaler new"
    );
    assert_eq!(
        std::fs::read_to_string(folder.join("OptiScaler.ini")).unwrap(),
        "mine"
    );
    assert!(!folder.join("old.dll").exists());
    assert!(matches!(
        prepare(&exe, Some(&new)),
        Response::Placed { updated: false, .. }
    ));

    std::fs::write(folder.join(PDLL), "patched").unwrap();
    let Response::Error { message } = prepare(&exe, Some(&new)) else {
        panic!()
    };
    assert!(message.contains("nvngx_dlssg.dll changed"), "{message}");
    std::fs::remove_file(folder.join(MANIFEST)).unwrap();
    let Response::Error { message } = prepare(&exe, None) else {
        panic!()
    };
    assert!(message.contains("gone"), "{message}");

    // The game gained anti-cheat since.
    let temp = tempdir();
    let cache = release(temp.path(), "Alpha0.3.5.8", b"OptiScaler proxy");
    let exe = game(temp.path());
    assert!(matches!(
        install(Some(&exe), None, &cache),
        Response::Placed { .. }
    ));
    std::fs::write(exe.with_file_name("BEClient_x64.dll"), "").unwrap();
    let Response::Refused { reason } = prepare(&exe, Some(&cache)) else {
        panic!()
    };
    assert!(reason.contains("BattlEye"), "{reason}");
}

#[test]
fn the_games_program_is_found_in_its_install_or_chosen() {
    let temp = tempdir();
    let cache = release(temp.path(), "Alpha0.3.5.8", b"OptiScaler proxy");
    // Unreal: the launcher stub at the top, the shipping build below.
    let unreal = temp.path().join("unreal");
    std::fs::create_dir_all(unreal.join("Game/Binaries/Win64")).unwrap();
    std::fs::write(unreal.join("Game.exe"), "MZ").unwrap();
    let shipping = unreal.join("Game/Binaries/Win64/Game-Win64-Shipping.exe");
    std::fs::write(&shipping, "MZ").unwrap();
    std::fs::write(shipping.with_file_name("nvngx_dlssg.dll"), "DLSS").unwrap();
    let Response::Placed { exe, .. } = install(None, Some(&unreal), &cache) else {
        panic!()
    };
    assert_eq!(exe, shipping);
    // Unity: the game beside its crash handler.
    let unity = temp.path().join("unity");
    std::fs::create_dir_all(&unity).unwrap();
    std::fs::write(unity.join("Game.exe"), "MZ").unwrap();
    std::fs::write(unity.join("UnityCrashHandler64.exe"), "MZ").unwrap();
    std::fs::write(unity.join("nvngx_dlssg.dll"), "DLSS").unwrap();
    let Response::Placed { exe, .. } = install(None, Some(&unity), &cache) else {
        panic!()
    };
    assert_eq!(exe, unity.join("Game.exe"));
    // Two programs: the user picks.
    let two = temp.path().join("two");
    std::fs::create_dir_all(&two).unwrap();
    std::fs::write(two.join("GameDX11.exe"), "MZ").unwrap();
    std::fs::write(two.join("GameDX12.exe"), "MZ").unwrap();
    assert!(matches!(
        install(None, Some(&two), &cache),
        Response::ChooseExe { .. }
    ));
    assert!(matches!(
        install(None, None, &cache),
        Response::ChooseExe { .. }
    ));
    assert!(matches!(
        install(Some(&two.join("missing.exe")), None, &cache),
        Response::Error { .. }
    ));
}

/// An Unreal game's anti-cheat at its install's root is found however the
/// install's path is written, and when the chosen program is outside the
/// install; an install too big to search through is refused.
#[test]
fn anti_cheat_is_found_however_the_install_is_written_and_a_huge_one_is_refused() {
    let temp = tempdir();
    let cache = release(temp.path(), "Alpha0.3.5.8", b"OptiScaler proxy");
    let install_dir = temp.path().join("SteamApps/common/Game");
    let binaries = install_dir.join("Game/Plugins/Runtime/Binaries/Win64");
    std::fs::create_dir_all(&binaries).unwrap();
    std::fs::create_dir_all(install_dir.join("EasyAntiCheat")).unwrap();
    let exe = binaries.join("Game-Win64-Shipping.exe");
    std::fs::write(&exe, "MZ").unwrap();
    // The library as the launcher wrote it, not as the folder has it.
    let written = temp.path().join("steamapps/common/game");
    let Response::Refused { reason } = install(Some(&exe), Some(&written), &cache) else {
        panic!()
    };
    assert!(reason.contains("Easy Anti-Cheat"), "{reason}");
    // A program chosen outside the install is not the game's.
    let elsewhere = temp.path().join("Elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::write(elsewhere.join("Game.exe"), "MZ").unwrap();
    let Response::ChooseExe { reason } = install(
        Some(&elsewhere.join("Game.exe")),
        Some(&install_dir),
        &cache,
    ) else {
        panic!()
    };
    assert!(reason.contains("not in the game's folder"), "{reason}");
    assert_eq!(names(&elsewhere), ["Game.exe"]);

    // More files than the search looks at (fewer in tests).
    let big = temp.path().join("Big");
    std::fs::create_dir_all(&big).unwrap();
    for index in 0..1_000 {
        std::fs::write(big.join(format!("{index}.pak")), "").unwrap();
    }
    std::fs::write(big.join("Game.exe"), "MZ").unwrap();
    let Response::Refused { reason } = install(Some(&big.join("Game.exe")), Some(&big), &cache)
    else {
        panic!()
    };
    assert!(reason.contains("too many files"), "{reason}");
    let Response::ChooseExe { reason } = install(None, Some(&big), &cache) else {
        panic!()
    };
    assert!(reason.contains("too many files"), "{reason}");
    assert!(!big.join(MANIFEST).exists());
}

/// With no install known, the search above the .exe stops at a Program
/// Files folder: anti-cheat installed there for other games is not this
/// game's. A studio's own beside its games still is.
#[test]
fn anti_cheat_installed_for_every_game_does_not_refuse_one() {
    let temp = tempdir();
    let cache = release(temp.path(), "Alpha0.3.5.8", b"OptiScaler proxy");
    let programs = temp.path().join("Program Files (x86)");
    std::fs::create_dir_all(programs.join("EasyAntiCheat_EOS")).unwrap();
    std::fs::create_dir_all(programs.join("Game")).unwrap();
    let exe = programs.join("Game/Game.exe");
    std::fs::write(&exe, "MZ").unwrap();
    std::fs::write(exe.with_file_name("nvngx_dlssg.dll"), "DLSS").unwrap();
    assert!(matches!(
        install(Some(&exe), None, &cache),
        Response::Placed { .. }
    ));
    assert_eq!(remove(&exe), Response::Removed { kept: vec![] });
    assert_eq!(
        names(&programs.join("Game")),
        ["Game.exe", "nvngx_dlssg.dll"]
    );

    let studio = programs.join("Studio");
    std::fs::create_dir_all(studio.join("EasyAntiCheat")).unwrap();
    std::fs::create_dir_all(studio.join("Game/Bin")).unwrap();
    let exe = studio.join("Game/Bin/Game.exe");
    std::fs::write(&exe, "MZ").unwrap();
    let Response::Refused { reason } = install(Some(&exe), None, &cache) else {
        panic!()
    };
    assert!(reason.contains("Easy Anti-Cheat"), "{reason}");
}

/// A game in the user's profile whose install is not known: BattlEye's own
/// folder in `%LOCALAPPDATA%` is no game's, so the search above the .exe
/// stops at the profile's folders, whatever their case. A studio's
/// anti-cheat beside its game below them still refuses it.
#[test]
fn anti_cheat_in_the_users_profile_does_not_refuse_a_game_there() {
    let temp = tempdir();
    let profile = temp.path().join("Player");
    let local = profile.join(r"AppData\Local");
    std::fs::create_dir_all(local.join("BattlEye")).unwrap();
    std::fs::create_dir_all(local.join(r"Games\Game")).unwrap();
    let exe = local.join(r"Games\Game\Game.exe");
    std::fs::write(&exe, "MZ").unwrap();
    let shared = [
        profile.clone(),
        profile.join(r"AppData\Roaming"),
        PathBuf::from(local.to_string_lossy().to_uppercase()),
    ];
    let refusal = |exe: &Path, shared: &[PathBuf]| {
        butterpollo_stream::neural::folder::anti_cheat_refusal(exe, None, shared)
    };
    // Without the profile's folders the search reaches BattlEye's.
    let error = refusal(&exe, &[]).unwrap_err();
    assert!(error.contains("BattlEye"), "{error}");
    assert_eq!(refusal(&exe, &shared), Ok(()));

    let studio = local.join("Studio");
    std::fs::create_dir_all(studio.join("EasyAntiCheat")).unwrap();
    std::fs::create_dir_all(studio.join(r"Game\Bin")).unwrap();
    let exe = studio.join(r"Game\Bin\Game.exe");
    std::fs::write(&exe, "MZ").unwrap();
    let error = refusal(&exe, &shared).unwrap_err();
    assert!(error.contains("Easy Anti-Cheat"), "{error}");

    // This account's own.
    let by_place = butterpollo_stream::neural::folder::shared_folders();
    for name in ["USERPROFILE", "LOCALAPPDATA", "APPDATA"] {
        let root = PathBuf::from(std::env::var_os(name).unwrap());
        assert!(by_place.contains(&root), "{name}");
    }
}
