//! Tests for Steam VDF/appinfo parsing, artwork, launch resolution, and library discovery.
use super::super::launch::library_folder;
use super::super::*;
use super::*;

#[test]
fn newer_apps_find_their_portrait_in_the_hashed_folder_and_on_the_cdn() {
    let root = std::env::temp_dir().join(format!("rubylight-steam-art-{}", std::process::id()));
    let hash = "0123456789abcdef0123456789abcdef01234567";
    let folder = root
        .join("appcache")
        .join("librarycache")
        .join("2050650")
        .join(hash);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("library_600x900.jpg"), b"jpg").unwrap();
    let (portrait, _, _) = artwork(2050650, &root, &[]);
    let portrait = portrait.unwrap();
    assert_eq!(portrait, folder.join("library_600x900.jpg"));
    assert_eq!(
        hashed_store_portrait_url(2050650, &portrait).as_deref(),
        Some(
            "https://shared.fastly.steamstatic.com/store_item_assets/steam/apps/2050650/0123456789abcdef0123456789abcdef01234567/library_600x900_2x.jpg"
        )
    );
    let capsule = folder.join("library_capsule.png");
    assert!(
        hashed_store_portrait_url(7, &capsule)
            .unwrap()
            .ends_with(&format!("/7/{hash}/library_capsule_2x.jpg"))
    );
    // Not a content hash, or not a portrait: keep to the fixed path.
    assert_eq!(
        hashed_store_portrait_url(7, Path::new("cache/7/library_600x900.jpg")),
        None
    );
    assert_eq!(
        hashed_store_portrait_url(7, &folder.join("library_header.jpg")),
        None
    );
    assert_eq!(
        hashed_store_portrait_url(
            7,
            &folder
                .with_file_name("ABCDEF0123456789ABCDEF0123456789ABCDEF01")
                .join("library_600x900.jpg")
        ),
        None
    );
    let _ = std::fs::remove_dir_all(&root);
}
#[test]
fn text_vdf_reads_library_folders_and_manifests() {
    let folders = Vdf::parse(
        r#""libraryfolders"
{
	"0"
	{
		"path"		"C:\\Program Files (x86)\\Steam"
		"apps" { "228980" "1" }
	}
	// a comment
	"1" { "path" "D:\\SteamLibrary" }
	}
}"#,
    );
    let paths: Vec<_> = folders
        .get("libraryfolders")
        .unwrap()
        .entries()
        .iter()
        .filter_map(|(_, f)| f.get("path").and_then(Vdf::text))
        .collect();
    assert_eq!(
        paths,
        ["C:\\Program Files (x86)\\Steam", "D:\\SteamLibrary"]
    );
    let manifest = Vdf::parse(
        "\"AppState\" { \"appid\" \"570\" \"name\" \"Dota 2\" \"installdir\" \"dota 2 beta\" }",
    );
    assert_eq!(
        manifest.at(&["AppState", "appid"]).and_then(Vdf::number),
        Some(570)
    );
    assert_eq!(
        manifest.at(&["appstate", "NAME"]).and_then(Vdf::text),
        Some("Dota 2")
    );
}
#[test]
fn objects_nested_without_end_stop_the_parsers_rather_than_the_host() {
    // Both parsers recurse once per object.
    let deep = "a{".repeat(200_000);
    let parsed = Vdf::parse(&format!("\"first\" \"kept\" {deep}"));
    assert_eq!(parsed.at(&["first"]).and_then(Vdf::text), Some("kept"));
    let (mut at, mut depth) = (&parsed, 0);
    while let Some(inner) = at.get("a") {
        at = inner;
        depth += 1;
    }
    assert_eq!(depth, 64);
    // The older format: a string after each kind byte names the key.
    let kv = [0x00, b'a', 0x00].repeat(200_000);
    let mut data = 0x0756_4428u32.to_le_bytes().to_vec();
    data.extend(1u32.to_le_bytes());
    data.extend(binary_entry(70, &kv));
    data.extend(0u32.to_le_bytes());
    assert!(appinfo(&data, &BTreeSet::from([70])).is_empty());
}
#[test]
fn binary_appinfo_gives_names_and_types() {
    // Format 29: keys are indices into a string table.
    let keys = ["appinfo", "common", "name", "type"];
    let mut kv = vec![0x00];
    kv.extend(0u32.to_le_bytes());
    kv.push(0x00);
    kv.extend(1u32.to_le_bytes());
    kv.push(0x01);
    kv.extend(2u32.to_le_bytes());
    kv.extend(b"Half-Life\0");
    kv.push(0x01);
    kv.extend(3u32.to_le_bytes());
    kv.extend(b"Game\0");
    kv.extend([0x08, 0x08, 0x08]);
    let mut data = 0x0756_4429u32.to_le_bytes().to_vec();
    data.extend(1u32.to_le_bytes());
    data.extend([0; 8]);
    data.extend(binary_entry(70, &kv));
    data.extend(binary_entry(80, &kv));
    data.extend(0u32.to_le_bytes());
    let table = data.len() as u64;
    data[8..16].copy_from_slice(&table.to_le_bytes());
    data.extend((keys.len() as u32).to_le_bytes());
    for key in keys {
        data.extend(key.as_bytes());
        data.push(0);
    }
    let found = appinfo(&data, &BTreeSet::from([70]));
    assert_eq!(found.len(), 1);
    assert_eq!(found[&70], ("Half-Life".into(), "Game".into()));
    assert!(appinfo(&data[..40], &BTreeSet::from([70])).is_empty());
}
#[test]
fn the_app_cache_is_read_again_only_when_it_changed() {
    let steam = steam_with_appinfo(&[], &[text("name", "Black Flag"), text("type", "Game")]);
    let path = steam.path().join("appcache").join("appinfo.vdf");
    let wanted = BTreeSet::from([242050]);
    let name = |found: Option<AppNames>| found.map(|found| found[&242050].0.clone());
    assert_eq!(name(app_names(&path, &wanted)), Some("Black Flag".into()));
    // Same size and time: the answer is the remembered one, whatever the bytes say now.
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    let data = std::fs::read(&path).unwrap();
    let at = data.windows(5).position(|w| w == b"Black").unwrap();
    let mut renamed = data.clone();
    renamed[at..at + 5].copy_from_slice(b"White");
    std::fs::write(&path, &renamed).unwrap();
    let file = std::fs::File::options().write(true).open(&path).unwrap();
    file.set_modified(modified).unwrap();
    drop(file);
    assert_eq!(name(app_names(&path, &wanted)), Some("Black Flag".into()));
    // A new time reads the file again, as do other apps asked for.
    let file = std::fs::File::options().write(true).open(&path).unwrap();
    file.set_modified(modified + std::time::Duration::from_secs(2))
        .unwrap();
    drop(file);
    assert_eq!(name(app_names(&path, &wanted)), Some("White Flag".into()));
    assert_eq!(
        app_names(&path, &BTreeSet::from([70])),
        Some(AppNames::new())
    );
    std::fs::remove_file(&path).unwrap();
    assert_eq!(app_names(&path, &wanted), None);
}
#[test]
fn a_steam_game_starts_the_exe_of_its_windows_launch_option() {
    let steam = steam_with_launch_options(&[
        option("0", "AC4BFSP.app", "default", &[("oslist", "macos")]),
        option("1", "bin32\\AC4BFSP.exe", "default", &[("osarch", "32")]),
        option("2", "Launcher.exe", "option1", &[]),
        option(
            "3",
            "beta\\AC4BFSP.exe",
            "default",
            &[("betakey", "public_test")],
        ),
        option(
            "4",
            "bin/AC4BFSP.exe",
            "default",
            &[("oslist", "windows"), ("osarch", "64")],
        ),
    ]);
    let roots = [steam.path().to_path_buf()];
    let game = steam
        .path()
        .join("steamapps")
        .join("common")
        .join("Black Flag");
    assert_eq!(
        game_executable(242050, &roots).unwrap(),
        Launch {
            exe: game.join("bin").join("AC4BFSP.exe"),
            install_dir: game.clone(),
            arguments: String::new(),
            vac: false,
        }
    );
    assert_eq!(
        game_executable(7, &roots).unwrap_err(),
        "Steam app 7 is not installed"
    );
}
#[test]
fn a_steam_launch_options_arguments_come_with_its_exe() {
    // Portal starts hl2.exe -game portal: without them it is Half-Life 2.
    let mut launch = option("0", r"bin\AC4BFSP.exe", "default", &[]);
    let end = launch.len() - 1;
    launch.splice(end..end, text("arguments", " -game portal "));
    let steam = steam_with_launch_options(&[launch]);
    let found = game_executable(242050, &[steam.path().to_path_buf()]).unwrap();
    assert_eq!(found.arguments, "-game portal");
}
#[test]
fn a_steam_game_with_valve_anti_cheat_says_so() {
    let launch = option("0", r"bin\AC4BFSP.exe", "default", &[]);
    let vac = steam_with_appinfo(
        std::slice::from_ref(&launch),
        &[object(
            "category",
            &[text("category_2", "1"), text("category_8", "1")],
        )],
    );
    assert!(
        game_executable(242050, &[vac.path().to_path_buf()])
            .unwrap()
            .vac
    );
    let other = steam_with_appinfo(&[launch], &[object("category", &[text("category_2", "1")])]);
    assert!(
        !game_executable(242050, &[other.path().to_path_buf()])
            .unwrap()
            .vac
    );
}
/// A link's app and an .exe in a Steam library both answer with their
/// app's flag; a plain folder has none, and a cache that can't say is
/// an error.
#[test]
fn valve_anti_cheat_is_read_for_a_link_and_for_an_exe_in_a_library() {
    let launch = option("0", r"bin\AC4BFSP.exe", "default", &[]);
    let category = |flag| object("category", &[text("category_8", flag)]);
    let vac = steam_with_appinfo(std::slice::from_ref(&launch), &[category("1")]);
    let clean = steam_with_appinfo(&[launch], &[category("0")]);
    let exe = |steam: &tempfile::TempDir| {
        steam
            .path()
            .join(r"steamapps\common\Black Flag\bin\AC4BFSP.exe")
    };
    for (steam, listed) in [(&vac, true), (&clean, false)] {
        let roots = [steam.path().to_path_buf()];
        let plain = steam.path().join(r"Games\Game.exe");
        assert_eq!(
            valve_anti_cheat(Some(242050), &plain, "", &roots),
            Ok(listed)
        );
        assert_eq!(valve_anti_cheat(None, &exe(steam), "", &roots), Ok(listed));
        assert_eq!(valve_anti_cheat(None, &plain, "", &roots), Ok(false));
    }
    assert_eq!(
        valve_anti_cheat(Some(7), &exe(&vac), "", &[vac.path().to_path_buf()]),
        Err("Steam's app cache does not list app 7: start it once in Steam".into())
    );
}
#[test]
fn an_exe_started_directly_in_a_steam_library_is_its_apps() {
    let launch = option("0", r"bin\AC4BFSP.exe", "default", &[]);
    let steam = steam_with_appinfo(
        std::slice::from_ref(&launch),
        &[object("category", &[text("category_8", "1")])],
    );
    let roots = [steam.path().to_path_buf()];
    let common = steam.path().join("steamapps").join("common");
    // The folder in another case than the manifest's.
    let exe = common.join("BLACK FLAG").join("bin").join("AC4BFSP.exe");
    assert_eq!(
        library_app(&exe, "", &roots).unwrap(),
        Some(LibraryApp {
            appid: 242050,
            install_dir: common.join("Black Flag"),
            vac: true,
        })
    );
    let other = steam_with_appinfo(&[launch], &[]);
    let exe = other
        .path()
        .join(r"steamapps\common\Black Flag\AC4BFSP.exe");
    let others = [other.path().to_path_buf()];
    let found = library_app(&exe, "", &others).unwrap();
    assert!(!found.unwrap().vac);
    // Like a link: a game the app cache does not list is refused.
    std::fs::remove_file(other.path().join(r"appcache\appinfo.vdf")).unwrap();
    assert_eq!(
        library_app(&exe, "", &others).unwrap_err(),
        "Steam's app cache does not list app 242050: start it once in Steam"
    );
    // An .exe right in `common`, or none in a library: no Steam app.
    for exe in [
        common.join("Black Flag.exe"),
        steam.path().join("Black Flag").join("AC4BFSP.exe"),
    ] {
        assert_eq!(
            library_app(&exe, "", &roots).unwrap(),
            None,
            "{}",
            exe.display()
        );
    }
    // A folder in a listed library that no manifest names is refused.
    std::fs::create_dir_all(common.join("Mod")).unwrap();
    let error = library_app(&common.join(r"Mod\Mod.exe"), "", &roots).unwrap_err();
    assert!(
        error.ends_with("but no Steam app there installs to Mod: verify the game's files in Steam"),
        "{error}"
    );
}
#[test]
fn a_steam_apps_files_in_a_library_no_steam_lists_are_refused() {
    let launch = option("0", r"bin\AC4BFSP.exe", "default", &[]);
    let steam = steam_with_appinfo(&[launch], &[object("category", &[text("category_8", "1")])]);
    let exe = steam
        .path()
        .join(r"steamapps\common\Black Flag\bin\AC4BFSP.exe");
    // Its manifest names the folder, but no Steam is found: the Valve
    // Anti-Cheat flag can't be read, so it never streams as a plain .exe.
    let error = library_app(&exe, "", &[]).unwrap_err();
    assert!(
        error.contains(
            "is Steam app 242050's, but no Steam installation on this PC lists its library"
        ),
        "{error}"
    );
    // Nor beside a Steam installation that doesn't list it.
    let installed = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(installed.path().join("steamapps")).unwrap();
    assert!(library_app(&exe, "", &[installed.path().to_path_buf()]).is_err());
    // A `steamapps` no installation lists and no manifest names the
    // folder of is a plain folder.
    let plain = tempfile::tempdir().unwrap();
    let game = plain.path().join(r"steamapps\common\Puzzle");
    std::fs::create_dir_all(&game).unwrap();
    assert_eq!(
        library_app(&game.join("Puzzle.exe"), "", &[]).unwrap(),
        None
    );
}
#[cfg(windows)]
#[test]
fn a_library_spelled_another_way_is_the_one_steam_lists() {
    let launch = option("0", r"bin\AC4BFSP.exe", "default", &[]);
    let steam = steam_with_launch_options(&[launch]);
    let links = tempfile::tempdir().unwrap();
    let link = links.path().join("Link");
    let made = std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(&link)
        .arg(steam.path())
        .output()
        .unwrap();
    assert!(made.status.success(), "{made:?}");
    let exe = link.join(r"steamapps\common\Black Flag\bin\AC4BFSP.exe");
    let found = library_app(&exe, "", &[steam.path().to_path_buf()]).unwrap();
    assert_eq!(found.unwrap().appid, 242050);
    std::fs::remove_dir(&link).unwrap();
}
/// A Steam folder with apps that all install to `Half-Life 2`, each with
/// its type, launch options (executable, arguments) and VAC flag.
/// An app: its id, type, launch options (executable, arguments) and
/// whether Steam lists Valve Anti-Cheat for it.
type TestApp<'a> = (u32, &'a str, &'a [(&'a str, &'a str)], bool);
fn half_life_2(apps: &[TestApp]) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let steamapps = root.path().join("steamapps");
    std::fs::create_dir_all(steamapps.join(r"common\Half-Life 2")).unwrap();
    let mut data = 0x0756_4428u32.to_le_bytes().to_vec();
    data.extend(1u32.to_le_bytes());
    for (appid, kind, options, vac) in apps {
        std::fs::write(
            steamapps.join(format!("appmanifest_{appid}.acf")),
            format!("\"AppState\" {{ \"appid\" \"{appid}\" \"installdir\" \"Half-Life 2\" }}"),
        )
        .unwrap();
        let mut common = vec![text("type", kind)];
        if *vac {
            common.push(object("category", &[text("category_8", "1")]));
        }
        let options: Vec<Vec<u8>> = options
            .iter()
            .enumerate()
            .map(|(at, (exe, arguments))| {
                object(
                    &at.to_string(),
                    &[
                        text("executable", exe),
                        text("arguments", arguments),
                        text("type", "default"),
                    ],
                )
            })
            .collect();
        let mut kv = object(
            "appinfo",
            &[
                object("common", &common),
                object("config", &[object("launch", &options)]),
            ],
        );
        kv.extend([0x08, 0x08]);
        data.extend(binary_entry(*appid, &kv));
    }
    data.extend(0u32.to_le_bytes());
    std::fs::create_dir_all(root.path().join("appcache")).unwrap();
    std::fs::write(root.path().join(r"appcache\appinfo.vdf"), data).unwrap();
    root
}
#[test]
fn half_life_2_and_its_episodes_in_one_folder_are_told_apart() {
    let steam = half_life_2(&[
        (220, "Game", &[("hl2.exe", "")], false),
        (340, "Game", &[("hl2.exe", "-game lostcoast")], false),
        (380, "Game", &[("hl2.exe", "-game episodic")], false),
        (420, "Game", &[("hl2.exe", "-game ep2")], false),
        (218, "Tool", &[("hl2.exe", "")], false),
    ]);
    let roots = [steam.path().to_path_buf()];
    let exe = steam.path().join(r"steamapps\common\Half-Life 2\hl2.exe");
    let app = |arguments: &str| library_app(&exe, arguments, &roots).unwrap().unwrap();
    assert_eq!(app("").appid, 220);
    assert_eq!(app("-novid").appid, 220);
    assert_eq!(app("-game episodic -novid").appid, 380);
    assert_eq!(app("-GAME ep2").appid, 420);
    // Apps that can't be told apart: the lowest stands for them, with
    // Valve Anti-Cheat when any has it.
    let steam = half_life_2(&[
        (220, "Game", &[("hl2.exe", "")], false),
        (221, "Game", &[(r".\hl2.exe", "")], true),
    ]);
    let exe = steam.path().join(r"steamapps\common\Half-Life 2\hl2.exe");
    let found = library_app(&exe, "", &[steam.path().to_path_buf()])
        .unwrap()
        .unwrap();
    assert_eq!((found.appid, found.vac), (220, true));
    // Several apps there and none starts the .exe: refused.
    let other = steam.path().join(r"steamapps\common\Half-Life 2\other.exe");
    assert_eq!(
        library_app(&other, "", &[steam.path().to_path_buf()]).unwrap_err(),
        "Steam apps 220, 221 install to Half-Life 2, and none of them starts other.exe: start the game from its Steam link"
    );
}
#[test]
fn valve_anti_cheat_of_any_app_starting_the_exe_is_never_narrowed_away() {
    // The .exe loads Counter-Strike's game with `-game cstrike`, whichever
    // app id stands for it.
    let steam = half_life_2(&[
        (70, "Game", &[("hl.exe", "")], false),
        (10, "Game", &[("hl.exe", "-game cstrike")], true),
        (90, "Game", &[("other.exe", "")], true),
    ]);
    let roots = [steam.path().to_path_buf()];
    let exe = steam.path().join(r"steamapps\common\Half-Life 2\hl.exe");
    let app = |arguments: &str| {
        let found = library_app(&exe, arguments, &roots).unwrap().unwrap();
        (found.appid, found.vac)
    };
    // Quoted, the argument still names the anti-cheat app.
    assert_eq!(app(r#"-game "cstrike""#), (10, true));
    // The argument match drops it, and its anti-cheat stays.
    assert_eq!(app("-novid"), (70, true));
    // The type filter drops it, and its anti-cheat stays.
    let steam = half_life_2(&[
        (70, "Game", &[("hl.exe", "")], false),
        (80, "Tool", &[("hl.exe", "")], true),
    ]);
    let found = library_app(&exe_in(&steam, "hl.exe"), "", &[steam.path().to_path_buf()])
        .unwrap()
        .unwrap();
    assert_eq!((found.appid, found.vac), (70, true));
    // An app that starts another .exe in the folder says nothing about
    // this one.
    let steam = half_life_2(&[
        (70, "Game", &[("hl.exe", "")], false),
        (90, "Game", &[("other.exe", "")], true),
    ]);
    let found = library_app(&exe_in(&steam, "hl.exe"), "", &[steam.path().to_path_buf()])
        .unwrap()
        .unwrap();
    assert_eq!((found.appid, found.vac), (70, false));
}
fn exe_in(steam: &tempfile::TempDir, name: &str) -> PathBuf {
    steam
        .path()
        .join(r"steamapps\common\Half-Life 2")
        .join(name)
}
#[test]
fn a_library_folder_is_read_from_the_exes_path_on_this_pc_only() {
    let found = |exe: &str| library_folder(Path::new(exe));
    assert_eq!(
        found(r"\\?\D:\SteamLibrary\SteamApps\Common\Game\bin\g.exe"),
        Some((
            PathBuf::from(r"D:\SteamLibrary\SteamApps"),
            "Game".into(),
            r"bin\g.exe".into()
        ))
    );
    assert_eq!(
        found("D:/Steam/steamapps/common/Game/g.exe"),
        Some((
            PathBuf::from(r"D:\Steam\steamapps"),
            "Game".into(),
            "g.exe".into()
        ))
    );
    for exe in [
        r"\\nas\games\steamapps\common\Game\g.exe",
        r"\\?\UNC\nas\games\steamapps\common\Game\g.exe",
        r"\\?\GLOBALROOT\Device\Mup\nas\steamapps\common\Game\g.exe",
        r"\\.\pipe\steamapps\common\Game\g.exe",
        r"D:\Steam\steamapps\common\Game.exe",
        r"D:\Steam\steamapps\common\Game\..\..\..\Other\g.exe",
        r"D:\Games\Game\g.exe",
    ] {
        assert_eq!(found(exe), None, "{exe}");
    }
}
#[test]
fn a_library_on_a_network_share_is_never_opened() {
    let steam = tempfile::tempdir().unwrap();
    let apps = steam.path().join("steamapps");
    std::fs::create_dir_all(&apps).unwrap();
    // A library only a share path reaches, and one on a drive.
    let reached = tempfile::tempdir().unwrap();
    let reached_apps = reached.path().join("steamapps");
    std::fs::create_dir_all(reached_apps.join("common").join("Portal 2")).unwrap();
    std::fs::write(
        reached_apps.join("appmanifest_620.acf"),
        "\"AppState\" { \"appid\" \"620\" \"name\" \"Portal 2\" \"installdir\" \"Portal 2\" }",
    )
    .unwrap();
    let local = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(local.path().join("steamapps")).unwrap();
    let globalroot = format!(r"\\?\GLOBALROOT\GLOBAL??\{}", reached.path().display());
    // NT's namespace, which Windows resolves as well.
    let nt = format!(r"\??\{}", reached.path().display());
    if cfg!(windows) {
        // The paths reach the library: skipping them is what keeps it out.
        assert!(steamapps(Path::new(&globalroot)).is_some());
        assert!(steamapps(Path::new(&nt)).is_some());
    }
    let listed = [
        r"\\nas\games".to_string(),
        r"\\?\UNC\nas\games".into(),
        r"\\.\UNC\nas\games".into(),
        r"\??\UNC\nas\games".into(),
        globalroot,
        nt,
        local.path().display().to_string(),
    ];
    let folders: String = listed
        .iter()
        .enumerate()
        .map(|(at, path)| {
            format!(
                "\"{at}\" {{ \"path\" \"{}\" }}\n",
                path.replace('\\', "\\\\")
            )
        })
        .collect();
    std::fs::write(
        apps.join("libraryfolders.vdf"),
        format!("\"libraryfolders\" {{\n{folders}}}"),
    )
    .unwrap();
    let roots = [steam.path().to_path_buf()];
    assert_eq!(libraries(&roots), [apps, local.path().join("steamapps")]);
    assert_eq!(
        game_executable(620, &roots).unwrap_err(),
        "Steam app 620 is not installed"
    );
    assert!(discover(&roots).is_empty());
    // A Steam folder on a share is no root either.
    assert!(libraries(&[PathBuf::from(r"\\nas\Steam")]).is_empty());
}
#[test]
fn a_library_folder_that_appears_later_is_listed_with_the_vdf_unchanged() {
    let steam = tempfile::tempdir().unwrap();
    let apps = steam.path().join("steamapps");
    std::fs::create_dir_all(&apps).unwrap();
    // A library on a drive that is not there yet.
    let later = tempfile::tempdir().unwrap();
    let missing = later.path().join("Games");
    std::fs::write(
        apps.join("libraryfolders.vdf"),
        format!(
            "\"libraryfolders\" {{\n\"0\" {{ \"path\" \"{}\" }}\n}}",
            missing.display().to_string().replace('\\', "\\\\")
        ),
    )
    .unwrap();
    let roots = [steam.path().to_path_buf()];
    assert_eq!(libraries(&roots), std::slice::from_ref(&apps));
    // The drive appears; the file is not rewritten.
    std::fs::create_dir_all(missing.join("steamapps")).unwrap();
    assert_eq!(libraries(&roots), [apps, missing.join("steamapps")]);
}
#[test]
fn a_steam_launch_option_outside_its_folder_or_missing_is_refused() {
    let steam = steam_with_launch_options(&[option("0", "..\\Other\\Other.exe", "", &[])]);
    let roots = [steam.path().to_path_buf()];
    let error = game_executable(242050, &roots).unwrap_err();
    assert!(error.contains("outside its folder"), "{error}");
    let steam = steam_with_launch_options(&[option("0", "Gone.exe", "", &[])]);
    let error = game_executable(242050, &[steam.path().to_path_buf()]).unwrap_err();
    assert!(error.contains("Gone.exe is missing"), "{error}");
    let steam = steam_with_launch_options(&[option("0", "run.sh", "", &[])]);
    let error = game_executable(242050, &[steam.path().to_path_buf()]).unwrap_err();
    assert!(
        error.contains("no launch option that starts an .exe"),
        "{error}"
    );
    // Names and types still read from the same cache.
    let data = std::fs::read(steam.path().join("appcache").join("appinfo.vdf")).unwrap();
    assert_eq!(
        appinfo(&data, &BTreeSet::from([242050]))[&242050],
        (String::new(), String::new())
    );
}

#[test]
fn steam_rejects_escaped_or_remote_installdir_and_preserves_unreadable_library_apps() {
    let steam = tempfile::tempdir().unwrap();
    let apps = steam.path().join("steamapps");
    let outside = tempfile::tempdir().unwrap();
    let escaped_dir = outside.path().join("Secret");
    std::fs::create_dir_all(apps.join("common").join("ValidGame")).unwrap();
    std::fs::create_dir_all(&escaped_dir).unwrap();
    for (id, name, dir) in [
        (100, "UNC", r"\\attacker\share"),
        (101, "Traversal", r"..\..\Outside"),
        (102, "Drive", r"D:\Other"),
        (103, "ValidGame", "ValidGame"),
    ] {
        std::fs::write(
            apps.join(format!("appmanifest_{id}.acf")),
            format!(
                "\"AppState\" {{ \"appid\" \"{id}\" \"name\" \"{name}\" \"installdir\" \"{dir}\" }}"
            ),
        )
        .unwrap();
    }
    // Unreadable/oversized manifest marks discovery incomplete and preserves appid 104.
    std::fs::write(
        apps.join("appmanifest_104.acf"),
        vec![b' '; (MAX_STEAM_TEXT as usize) + 64],
    )
    .unwrap();
    let discovery = discover_checked(&[steam.path().to_path_buf()]);
    let ids: Vec<u32> = discovery.games.iter().map(|g| g.appid).collect();
    assert_eq!(ids, [103]);
    assert!(discovery.incomplete);
    assert!(discovery.unreadable_apps.contains(&104));
    assert!(discovery.unreadable_libraries.contains(&apps));

    // Lexical dotdot resolving back inside ValidGame still finds ValidGame cleanly.
    let linked_install = apps.join("common").join("ValidGame").join("sub");
    std::fs::create_dir_all(&linked_install).unwrap();
    let exe = apps.join("common").join("ValidGame").join("game.exe");
    std::fs::write(&exe, b"").unwrap();
    assert_eq!(
        super::super::launch::library_folder(&linked_install.join("..").join("game.exe"))
            .map(|(lib, game, rel)| (same_path(&lib, &apps), game, rel)),
        Some((true, "ValidGame".into(), "game.exe".into()))
    );
}
