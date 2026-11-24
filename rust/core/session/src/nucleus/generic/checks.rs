//! Path safety, anti-cheat detection, launcher filtering, and Epic/Unreal launch helpers for generic Nucleus handlers.
use super::{Setup, below, file_name, normalized, parent, unreal_root};
use std::path::Path;

/// The anti-cheat a file or folder name belongs to.
pub(super) fn anti_cheat_of(name: &str) -> Option<&'static str> {
    let name = name.to_ascii_lowercase();
    let starts = |prefix: &str| name.starts_with(prefix);
    let any = |names: &[&str]| names.contains(&name.as_str());
    Some(
        if starts("easyanticheat") || any(&["start_protected_game.exe"]) {
            "Easy Anti-Cheat"
        } else if any(&["battleye"]) || starts("beservice") || starts("beclient") {
            "BattlEye"
        } else if any(&[
            "riot vanguard",
            "vgc.exe",
            "vgk.sys",
            "valorant.exe",
            "valorant-win64-shipping.exe",
            "leagueclient.exe",
            "league of legends.exe",
        ]) {
            "Riot Vanguard"
        } else if any(&["gameguard", "gamemon.des", "gameguard.des"]) {
            "nProtect GameGuard"
        } else if any(&["xigncode", "x3.xem", "xhunter1.sys"]) {
            "XIGNCODE3"
        } else if starts("randgrid") || starts("atvi-randgrid") {
            "Ricochet"
        } else if starts("eaanticheat") {
            "EA Javelin"
        } else if any(&["pnkbstra.exe", "pnkbstrb.exe"]) {
            "PunkBuster"
        } else if starts("mhyprot") || any(&["hoyokprotect.sys"]) {
            "HoYoverse's anti-cheat"
        } else if any(&["anticheatexpert"]) {
            "Anti-Cheat Expert"
        } else if any(&["robloxplayerbeta.exe"]) {
            "Hyperion"
        } else if any(&["cs2.exe", "csgo.exe", "dota2.exe", "tf.exe", "tf_win64.exe"]) {
            "Valve Anti-Cheat"
        } else if any(&["overwatch.exe"]) {
            "Blizzard's anti-cheat"
        } else if starts("faceit") {
            "FACEIT"
        } else if any(&["tslgame.exe", "rainbowsix.exe", "rainbowsix_be.exe"]) {
            "BattlEye"
        } else if any(&["fortniteclient-win64-shipping.exe", "r5apex.exe"]) {
            "Easy Anti-Cheat"
        } else {
            return None;
        },
    )
}

/// A folder many games share, whose files say nothing about one game: a
/// drive's root, Program Files, Steam's and Epic's game folders, XboxGames.
pub(super) fn shared_folder(folder: &str) -> bool {
    let name = file_name(folder);
    let is = |other: &str| name.eq_ignore_ascii_case(other);
    folder.trim_end_matches('\\').len() <= 3
        || !folder.contains('\\')
        || [
            "Program Files",
            "Program Files (x86)",
            "Epic Games",
            "XboxGames",
            "WindowsApps",
            "steamapps",
        ]
        .iter()
        .any(|shared| is(shared))
        || (is("common") && file_name(parent(folder)).eq_ignore_ascii_case("steamapps"))
}

/// Why Rubylight never writes its handler for `exe_path`, checked before
/// anything opens the path: the host runs as SYSTEM, which must not reach
/// out to a network share, and Windows' own programs (`windows`, the
/// Windows folder) are no game. A folder in the user's profile is fine.
pub fn refused_path(exe_path: &str, windows: &str) -> Option<String> {
    let Some(local) = on_this_pc(exe_path) else {
        return Some(format!(
            "{exe_path} is on a network share: Rubylight starts only a game on this PC's own drives"
        ));
    };
    below(&local, &normalized(windows))
        .map(|_| format!("{exe_path} is part of Windows, not a game"))
}

/// `path` as a plain drive path (`C:\...`) when it is on one of this PC's
/// drives; None for `\\server\share`, `\\?\UNC\...`, `\\?\GLOBALROOT\...`,
/// every other `\\?\` or `\\.\` path but `\\?\C:\...`, and every other path
/// starting with `\`, such as NT's `\??\UNC\...`. The host runs as SYSTEM and
/// must not reach out to a share.
pub fn on_this_pc(path: &str) -> Option<String> {
    let path = normalized(path);
    let drive = |rest: &str| {
        let bytes = rest.as_bytes();
        bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && &bytes[1..3] == br":\"
    };
    // Any other path from the root of a namespace (`\??\UNC\...`,
    // `\Device\...`) is no drive path either.
    let local = match path
        .strip_prefix(r"\\?\")
        .or_else(|| path.strip_prefix(r"\\.\"))
    {
        Some(rest) => drive(rest).then_some(rest)?,
        None => (!path.starts_with('\\')).then_some(path.as_str())?,
    };
    Some(local.to_string())
}

/// What says which Epic game the game at `exe_path` is, from one of the Epic
/// Games Launcher's `.item` manifests, without the sign-in we don't have; None
/// when the manifest is another game's. The game may be any .exe below its
/// install folder, since many manifests name a launcher stub, and the manifest
/// of a DLC names none. Started from its .exe without these, a game that asks
/// the Epic SDK restarts through the launcher, outside Nucleus Co-op and on the
/// PC's monitor, and the instance exits (#56). `SteamAppId` does the same for a
/// Steam game ([`Setup::steam_app`]).
///
/// Any user can write these manifests and the game may run elevated, so only
/// names that can't carry an option reach its command line, and never the
/// manifest's `LaunchCommand`: an app that needs those options has its own
/// arguments.
pub fn epic_arguments(manifest: &[u8], exe_path: &str) -> Result<Option<String>, String> {
    let record: serde_json::Value =
        serde_json::from_slice(manifest).map_err(|error| error.to_string())?;
    let text = |key: &str| record[key].as_str().unwrap_or("").trim();
    let (folder, launched) = (text("InstallLocation"), text("LaunchExecutable"));
    if folder.is_empty() || launched.is_empty() {
        return Ok(None);
    }
    let folder = normalized(folder);
    let folder = folder.trim_end_matches('\\');
    let exe = normalized(exe_path);
    // A drive's root is no game's folder: it would claim every game on it.
    if !folder.contains('\\')
        || !exe.to_ascii_lowercase().ends_with(".exe")
        || below(&exe, folder).is_none()
    {
        return Ok(None);
    }
    let name = |value: &str| {
        (1..=128).contains(&value.len())
            && value
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
    };
    let (app, namespace) = (text("AppName"), text("CatalogNamespace"));
    if !name(app) || !name(namespace) {
        return Err(format!("{exe} has no usable app name or namespace"));
    }
    Ok(Some(format!(
        "-EpicPortal -epicapp={app} -epicenv=Prod -epicsandboxid={namespace}"
    )))
}

/// Why an Epic game's instance ended soon after it started.
pub fn epic_exit(exited: &str, title: &str) -> String {
    format!(
        "{exited}: {title} is an Epic Games game, and it may have started again through the Epic Games Launcher, on the PC's monitor and outside Nucleus Co-op. Close it there if it is open; a normal stream plays it"
    )
}

/// Whether `exe_path` is a launcher rather than a game: Nucleus would hook
/// the launcher, which starts the game outside its hooks (Steam's
/// `-applaunch`, Bethesda's and Rockstar's launchers) or a whole library.
pub fn launcher(exe_path: &str) -> bool {
    let name = file_name(&normalized(exe_path)).to_ascii_lowercase();
    name.ends_with("launcher.exe")
        || [
            "steam.exe",
            "playnite.desktopapp.exe",
            "playnite.fullscreenapp.exe",
            "galaxyclient.exe",
            "eadesktop.exe",
            "origin.exe",
            "upc.exe",
            "ubisoftconnect.exe",
            "battle.net.exe",
            "playgtav.exe",
            "playrdr2.exe",
            "xboxpcapp.exe",
            // Frontends that start a library's games.
            "retrobat.exe",
            "emulationstation.exe",
            "launchbox.exe",
            "bigbox.exe",
        ]
        .contains(&name.as_str())
}

/// The game an Unreal root stub starts: for `<root>\<Game>.exe` beside
/// `<root>\Engine`, the one `<root>\<Project>\Binaries\Win64\
/// *-Win64-Shipping.exe`. The stub only starts it and exits, so Nucleus
/// would hook nothing. None when there is none or more than one.
pub fn unreal_shipping(exe_path: &str) -> Option<String> {
    let exe_path = normalized(exe_path);
    let root = parent(&exe_path);
    if shared_folder(root) || !Path::new(root).join("Engine").is_dir() {
        return None;
    }
    let mut found = std::fs::read_dir(root)
        .ok()?
        .flatten()
        .filter(|project| project.file_type().is_ok_and(|kind| kind.is_dir()))
        .flat_map(|project| {
            std::fs::read_dir(project.path().join("Binaries").join("Win64"))
                .into_iter()
                .flatten()
                .flatten()
        })
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name().is_some_and(|name| {
                name.to_string_lossy()
                    .to_ascii_lowercase()
                    .ends_with("-win64-shipping.exe")
            })
        });
    let first = found.next()?;
    found
        .next()
        .is_none()
        .then(|| first.to_string_lossy().into_owned())
}

/// The anti-cheat `setup`'s game ships: one known from elsewhere (Steam's
/// Valve Anti-Cheat flag), else from the .exe's name and the files and
/// folders beside it and in the folders above it up to the game's root,
/// never above it or in a folder many games share, else from Battle.net's
/// install marker ([`battle_net`]). Nucleus Co-op injects hooks, which
/// anti-cheat treats as cheating, so such a game is refused.
pub fn anti_cheat(setup: &Setup) -> Option<&'static str> {
    if let Some(found) = setup
        .anti_cheat
        .or_else(|| anti_cheat_of(file_name(&setup.exe_path)))
    {
        return Some(found);
    }
    let exe_folder = parent(&setup.exe_path);
    scan(exe_folder, scan_root(setup)).or_else(|| battle_net(exe_folder))
}

/// The anti-cheat in `folder` and the folders above it up to `root`.
fn scan(mut folder: &str, root: &str) -> Option<&'static str> {
    loop {
        if shared_folder(folder) {
            return None;
        }
        let entries = std::fs::read_dir(Path::new(folder)).into_iter().flatten();
        for entry in entries.flatten() {
            if let Some(found) = anti_cheat_of(&entry.file_name().to_string_lossy()) {
                return Some(found);
            }
        }
        // Up to the game's root, never past it.
        below(folder, root)?;
        folder = parent(folder);
    }
}

/// The folder the scan stops at: the game's root, but the folder holding an
/// Unreal game's `Binaries` when nothing says the folder above is the
/// game's. Unreal Engine 4 and 5 keep `<Game>\Engine`, or at least the root
/// stub `<Game>\<Name>.exe`, beside `<Game>\<Project>\Binaries`; Unreal
/// Engine 3 keeps `<Game>\Binaries` beside `<Game>\Engine`, and the folder
/// above `<Game>` is the user's games folder.
fn scan_root(setup: &Setup) -> &str {
    let exe_folder = parent(&setup.exe_path);
    let holder = parent(parent(exe_folder));
    match unreal_root(exe_folder) {
        Some(above)
            if setup.steam_app.is_none()
                && above.eq_ignore_ascii_case(&setup.root)
                && !above.eq_ignore_ascii_case(holder) =>
        {
            let game = Path::new(&above);
            let stub = || {
                !Path::new(holder).join("Engine").is_dir()
                    && game.join(stub_name(&setup.exe_path)).is_file()
            };
            if game.join("Engine").is_dir() || stub() {
                &setup.root
            } else {
                holder
            }
        }
        _ => &setup.root,
    }
}

/// The root stub's name for an Unreal game's .exe: `Stray.exe` for
/// `Stray-Win64-Shipping.exe`, else the .exe's own name.
pub(super) fn stub_name(exe_path: &str) -> String {
    let name = file_name(exe_path);
    let lower = name.to_ascii_lowercase();
    [
        "-win64-shipping.exe",
        "-win32-shipping.exe",
        "-wingdk-shipping.exe",
    ]
    .iter()
    .find(|suffix| lower.ends_with(*suffix))
    .map_or_else(
        || name.to_string(),
        |suffix| format!("{}.exe", &name[..name.len() - suffix.len()]),
    )
}

/// Blizzard's anti-cheat for a game Battle.net installed: its `.build.info`
/// in the game's folder, which is the .exe's or up to two above it (World of
/// Warcraft's `_retail_`, StarCraft II's `Versions\Base<n>`).
fn battle_net(exe_folder: &str) -> Option<&'static str> {
    let mut folder = exe_folder;
    for _ in 0..3 {
        if shared_folder(folder) {
            return None;
        }
        if Path::new(folder).join(".build.info").is_file() {
            return Some("Blizzard's anti-cheat");
        }
        folder = parent(folder);
    }
    None
}
