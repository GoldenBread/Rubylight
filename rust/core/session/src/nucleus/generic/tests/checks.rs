//! Tests for anti-cheat detection and path safety in generic Nucleus Co-op handlers.
use super::super::*;

#[cfg(windows)]
#[test]
fn anti_cheat_is_found_beside_the_exe_and_up_to_the_root() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Shooter");
    let bin = root.join("Shooter").join("Binaries").join("Win64");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir(root.join("Engine")).unwrap();
    let exe = bin.join("Shooter.exe");
    std::fs::write(&exe, b"").unwrap();
    let setup = Setup::new(&exe.to_string_lossy(), "Shooter", None);
    assert_eq!(setup.root, root.to_string_lossy());
    assert_eq!(anti_cheat(&setup), None);
    std::fs::create_dir(root.join("EasyAntiCheat")).unwrap();
    assert_eq!(anti_cheat(&setup), Some("Easy Anti-Cheat"));
    std::fs::remove_dir(root.join("EasyAntiCheat")).unwrap();
    std::fs::write(bin.join("BEClient_x64.dll"), b"").unwrap();
    assert_eq!(anti_cheat(&setup), Some("BattlEye"));
    // Above the root is another game's business.
    std::fs::remove_file(bin.join("BEClient_x64.dll")).unwrap();
    std::fs::create_dir(dir.path().join("BattlEye")).unwrap();
    assert_eq!(anti_cheat(&setup), None);
    for (name, found) in [
        ("vgc.exe", "Riot Vanguard"),
        ("GameGuard", "nProtect GameGuard"),
        ("randgrid.sys", "Ricochet"),
        ("EAAntiCheat.GameServiceLauncher.exe", "EA Javelin"),
        ("XIGNCODE", "XIGNCODE3"),
        ("PnkBstrA.exe", "PunkBuster"),
    ] {
        assert_eq!(anti_cheat_of(name), Some(found), "{name}");
    }
    assert_eq!(anti_cheat_of("Shooter.exe"), None);
    let protected = Setup::new(r"C:\G\start_protected_game.exe", "G", None);
    assert_eq!(anti_cheat(&protected), Some("Easy Anti-Cheat"));
}

#[cfg(windows)]
#[test]
fn an_unreal_scan_stops_at_the_game_holding_binaries_unless_engine_is_above() {
    let dir = tempfile::tempdir().unwrap();
    let games = dir.path().join("Games");
    // Another game's anti-cheat in the user's games folder.
    std::fs::create_dir_all(games.join("BattlEye")).unwrap();
    // Unreal Engine 3: Binaries and Engine in the game's folder.
    let borderlands = games.join("Borderlands 2");
    let win32 = borderlands.join("Binaries").join("Win32");
    std::fs::create_dir_all(&win32).unwrap();
    std::fs::create_dir(borderlands.join("Engine")).unwrap();
    let setup = Setup::new(
        &win32.join("Borderlands2.exe").to_string_lossy(),
        "Borderlands 2",
        None,
    );
    assert_eq!(anti_cheat(&setup), None);
    std::fs::create_dir(borderlands.join("EasyAntiCheat")).unwrap();
    assert_eq!(anti_cheat(&setup), Some("Easy Anti-Cheat"));
    // Unreal Engine 4 and 5: `<Game>\<Project>\Binaries`, Engine in
    // `<Game>`, whose anti-cheat counts.
    let shooter = games.join("Shooter");
    let win64 = shooter.join("Shooter").join("Binaries").join("Win64");
    std::fs::create_dir_all(&win64).unwrap();
    std::fs::create_dir(shooter.join("Engine")).unwrap();
    let exe = win64.join("Shooter-Win64-Shipping.exe");
    std::fs::write(&exe, b"").unwrap();
    let setup = Setup::new(&exe.to_string_lossy(), "Shooter", None);
    assert_eq!(anti_cheat(&setup), None);
    std::fs::create_dir(shooter.join("EasyAntiCheat")).unwrap();
    assert_eq!(anti_cheat(&setup), Some("Easy Anti-Cheat"));
    // The root stub's game, as `prepared` starts it.
    std::fs::write(shooter.join("Shooter.exe"), b"").unwrap();
    let mut stub = Setup::new(
        &shooter.join("Shooter.exe").to_string_lossy(),
        "Shooter",
        None,
    );
    stub.exe_path = unreal_shipping(&stub.exe_path).unwrap();
    assert_eq!(stub.exe_path, exe.to_string_lossy());
    assert_eq!(anti_cheat(&stub), Some("Easy Anti-Cheat"));
    std::fs::remove_dir(shooter.join("EasyAntiCheat")).unwrap();
    assert_eq!(anti_cheat(&stub), None);
    // No Engine folder, but the root stub beside the project: the game's
    // folder is the stub's, and the games folder still is not read.
    let hero = games.join("Hero");
    let win64 = hero.join("HeroGame").join("Binaries").join("Win64");
    std::fs::create_dir_all(&win64).unwrap();
    std::fs::create_dir(hero.join("EasyAntiCheat")).unwrap();
    let exe = win64.join("Hero-Win64-Shipping.exe");
    std::fs::write(&exe, b"").unwrap();
    let setup = Setup::new(&exe.to_string_lossy(), "Hero", None);
    assert_eq!(anti_cheat(&setup), None);
    std::fs::write(hero.join("Hero.exe"), b"").unwrap();
    assert_eq!(anti_cheat(&setup), Some("Easy Anti-Cheat"));
    std::fs::remove_dir(hero.join("EasyAntiCheat")).unwrap();
    assert_eq!(anti_cheat(&setup), None);
    assert_eq!(stub_name(r"C:\G\Stray-WIN64-Shipping.exe"), "Stray.exe");
    assert_eq!(stub_name(r"C:\G\HogwartsLegacy.exe"), "HogwartsLegacy.exe");
}

#[cfg(windows)]
#[test]
fn a_battle_net_game_has_blizzards_anti_cheat() {
    let dir = tempfile::tempdir().unwrap();
    for (game, exe) in [
        ("World of Warcraft", r"_retail_\Wow.exe"),
        ("StarCraft II", r"Versions\Base90136\SC2_x64.exe"),
        ("Diablo IV", "Diablo IV.exe"),
    ] {
        let game = dir.path().join(game);
        let exe = game.join(exe);
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        let setup = Setup::new(&exe.to_string_lossy(), "G", None);
        assert_eq!(anti_cheat(&setup), None, "{}", exe.display());
        std::fs::write(game.join(".build.info"), b"Branch!STRING:0").unwrap();
        assert_eq!(
            anti_cheat(&setup),
            Some("Blizzard's anti-cheat"),
            "{}",
            exe.display()
        );
    }
    // Never above the game's folder or in a folder games share.
    let deep = dir.path().join(r"Mods\a\b\c\Mod.exe");
    std::fs::create_dir_all(deep.parent().unwrap()).unwrap();
    std::fs::write(dir.path().join(r"Mods\.build.info"), b"").unwrap();
    assert_eq!(
        anti_cheat(&Setup::new(&deep.to_string_lossy(), "G", None)),
        None
    );
    let programs = dir.path().join("Program Files (x86)");
    std::fs::create_dir_all(programs.join("Puzzle")).unwrap();
    std::fs::write(programs.join(".build.info"), b"").unwrap();
    let puzzle = programs.join(r"Puzzle\Puzzle.exe");
    assert_eq!(
        anti_cheat(&Setup::new(&puzzle.to_string_lossy(), "G", None)),
        None
    );
}

#[test]
fn anti_cheat_without_files_in_the_game_is_known_by_name_or_steam() {
    for (exe, found) in [
        (
            r"D:\S\common\Counter-Strike Global Offensive\game\bin\win64\cs2.exe",
            "Valve Anti-Cheat",
        ),
        (
            r"D:\S\common\dota 2 beta\game\bin\win64\dota2.exe",
            "Valve Anti-Cheat",
        ),
        (
            r"D:\S\common\Team Fortress 2\tf_win64.exe",
            "Valve Anti-Cheat",
        ),
        (
            r"C:\Games\Overwatch\_retail_\Overwatch.exe",
            "Blizzard's anti-cheat",
        ),
        (r"C:\Games\PUBG\TslGame.exe", "BattlEye"),
    ] {
        assert_eq!(
            anti_cheat(&Setup::new(exe, "G", None)),
            Some(found),
            "{exe}"
        );
    }
    assert_eq!(anti_cheat_of("FACEITClient.exe"), Some("FACEIT"));
    // Steam's Valve Anti-Cheat flag, for a game whose files say nothing.
    let flagged = Setup {
        anti_cheat: Some("Valve Anti-Cheat"),
        ..Setup::new(r"Z:\NoSuch\Portal\hl2.exe", "Portal", None)
    };
    assert_eq!(anti_cheat(&flagged), Some("Valve Anti-Cheat"));
}

#[test]
fn a_network_share_or_windows_own_program_is_refused_before_it_opens() {
    let windows = r"C:\Windows";
    for share in [
        r"\\nas\games\Ice\Ice.exe",
        "//nas/games/Ice/Ice.exe",
        r"\\?\UNC\nas\games\Ice\Ice.exe",
        r"\\.\pipe\Ice.exe",
        r"\\?\GLOBALROOT\Device\Mup\nas\games\Ice.exe",
        r"\\.\UNC\nas\games\Ice\Ice.exe",
        r"\\?\Volume{0c0d2a1e-0000-0000-0000-100000000000}\Ice.exe",
        // NT's own namespace, which Windows resolves too.
        r"\??\UNC\nas\games\Ice\Ice.exe",
        r"\??\GLOBALROOT\Device\Mup\nas\games\Ice.exe",
        r"\??\C:\Games\Ice\Ice.exe",
        "/??/UNC/nas/games/Ice/Ice.exe",
        r"\Device\Mup\nas\games\Ice.exe",
        r"\Games\Ice\Ice.exe",
    ] {
        let why = refused_path(share, windows).unwrap();
        assert!(
            why.ends_with(
                "is on a network share: Rubylight starts only a game on this PC's own drives"
            ),
            "{why}"
        );
    }
    assert_eq!(
        refused_path(r"C:\Windows\notepad.exe", windows).unwrap(),
        r"C:\Windows\notepad.exe is part of Windows, not a game"
    );
    assert!(refused_path(r"\\?\c:\windows\System32\mspaint.exe", windows).is_some());
    // The user's own folders and other drives are fine.
    for game in [
        r"C:\Users\Player\Games\Ice\Ice.exe",
        r"\\?\D:\Games\Ice\Ice.exe",
        r"C:\WindowsGames\Ice.exe",
        r"D:\Games\Windows\Ice.exe",
    ] {
        assert_eq!(refused_path(game, windows), None, "{game}");
    }
}

#[test]
fn the_scan_stays_in_the_games_folder_and_out_of_shared_ones() {
    let dir = tempfile::tempdir().unwrap();
    // Epic's system-wide EasyAntiCheat_EOS beside a plain game's folder.
    let programs = dir.path().join("Programs");
    let puzzle = programs.join("Puzzle");
    std::fs::create_dir_all(&puzzle).unwrap();
    std::fs::create_dir(programs.join("EasyAntiCheat_EOS")).unwrap();
    let setup = Setup::new(&puzzle.join("Puzzle.exe").to_string_lossy(), "Puzzle", None);
    assert_eq!(anti_cheat(&setup), None);
    // An .exe right in Program Files (x86) or a Steam library's common
    // folder: those are never read.
    for shared in [
        dir.path().join("Program Files (x86)"),
        dir.path().join("steamapps").join("common"),
        dir.path().join("Epic Games"),
    ] {
        std::fs::create_dir_all(shared.join("EasyAntiCheat_EOS")).unwrap();
        let setup = Setup::new(&shared.join("Game.exe").to_string_lossy(), "Game", None);
        assert_eq!(anti_cheat(&setup), None, "{}", shared.display());
    }
    assert!(shared_folder(r"C:\") && shared_folder("C:"));
    assert!(!shared_folder(r"C:\Games\Puzzle"));
}
