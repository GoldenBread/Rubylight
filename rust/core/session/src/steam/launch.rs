//! Steam executable launch resolution, library app identification, and VAC checks.
use super::{Vdf, appinfo_entries, installed, libraries, manifests, on_this_pc, same_path};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

/// How Steam starts an installed game on Windows ([`game_executable`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    pub exe: PathBuf,
    pub install_dir: PathBuf,
    /// The launch option's `arguments`, which Steam passes the .exe.
    pub arguments: String,
    /// Whether the store lists Valve Anti-Cheat for it (`common/category`
    /// `category_8`).
    pub vac: bool,
}

/// The .exe Steam starts for `appid` on Windows and the game's install
/// folder: the app cache's first launch option for Windows that is no
/// beta's and starts an .exe below the folder, a 64-bit one first.
pub fn game_executable(appid: u32, roots: &[PathBuf]) -> Result<Launch, String> {
    let install_dir = libraries(roots)
        .iter()
        .find_map(|steamapps| {
            let manifest = steamapps.join(format!("appmanifest_{appid}.acf"));
            installed(steamapps, &manifest).map(|(_, _, folder)| folder)
        })
        .ok_or_else(|| format!("Steam app {appid} is not installed"))?;
    let info = cached_app(appid, roots)?;
    let (relative, arguments) = launch_executable(&info)
        .ok_or_else(|| format!("Steam app {appid} has no launch option that starts an .exe"))?;
    let parts: Vec<&str> = relative
        .split(['\\', '/'])
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    if parts.iter().any(|part| *part == ".." || part.contains(':')) {
        return Err(format!(
            "Steam app {appid} starts {relative}, outside its folder"
        ));
    }
    let exe = parts
        .iter()
        .fold(install_dir.clone(), |path, part| path.join(part));
    if !exe.is_file() {
        return Err(format!(
            "{} is missing: verify the game's files in Steam",
            exe.display()
        ));
    }
    Ok(Launch {
        exe,
        install_dir,
        arguments,
        vac: vac(&info),
    })
}

/// The Steam app an .exe in a Steam library belongs to ([`library_app`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibraryApp {
    pub appid: u32,
    pub install_dir: PathBuf,
    /// As [`Launch::vac`].
    pub vac: bool,
}

/// The Steam app whose install folder holds `exe`, an .exe started directly
/// with `arguments`: for `<library>\steamapps\common\<installdir>\...`, an
/// app of that library's `appmanifest_<id>.acf` files with the same
/// `installdir` (in any case), with its Valve Anti-Cheat flag from the app
/// cache, as a link to the app gives it ([`game_executable`]).
///
/// None only for an .exe in no `steamapps\common`, or in one that no Steam
/// installation of `roots` lists and whose manifests name no app there: a
/// plain folder. An app's files in a library no installation lists are
/// refused, as Steam's flag for them can't be trusted, and so is a folder
/// of a listed library that no manifest names, or an app the cache does
/// not list. Several apps in one folder (Half-Life 2 and its episodes)
/// narrow to [`one_of`] them. Nothing is opened for a path off this PC's
/// drives.
pub fn library_app(
    exe: &Path,
    arguments: &str,
    roots: &[PathBuf],
) -> Result<Option<LibraryApp>, String> {
    let Some((steamapps, folder, relative)) = library_folder(exe) else {
        return Ok(None);
    };
    let named: Vec<(u32, PathBuf)> = manifests(&steamapps)
        .iter()
        .filter_map(|manifest| {
            let (appid, state, install_dir) = installed(&steamapps, manifest)?;
            state
                .get("installdir")
                .and_then(Vdf::text)
                .is_some_and(|name| {
                    name.trim_end_matches(['\\', '/'])
                        .eq_ignore_ascii_case(&folder)
                })
                .then_some((appid, install_dir))
        })
        .collect();
    let listed = libraries(roots)
        .iter()
        .any(|library| same_library(library, &steamapps));
    match (named.first(), listed) {
        (None, false) => return Ok(None),
        (None, true) => {
            return Err(format!(
                "{} is in Steam's library {}, but no Steam app there installs to {folder}: verify the game's files in Steam",
                exe.display(),
                steamapps.display()
            ));
        }
        (Some((appid, _)), false) => {
            return Err(format!(
                "{} is Steam app {appid}'s, but no Steam installation on this PC lists its library {}: start Steam once while signed in, or start the game from its Steam link",
                exe.display(),
                steamapps.display()
            ));
        }
        (Some(_), true) => {}
    }
    let wanted: BTreeSet<u32> = named.iter().map(|(appid, _)| *appid).collect();
    let infos = cached_apps(&wanted, roots);
    if let Some(appid) = wanted.iter().find(|appid| !infos.contains_key(appid)) {
        return Err(not_cached(*appid));
    }
    let candidates: Vec<(u32, &Vdf)> = named
        .iter()
        .map(|(appid, _)| (*appid, &infos[appid]))
        .collect();
    let (appid, vac) = one_of(&candidates, &relative, arguments).ok_or_else(|| {
        let ids: Vec<String> = wanted.iter().map(u32::to_string).collect();
        format!(
            "Steam apps {} install to {folder}, and none of them starts {relative}: start the game from its Steam link",
            ids.join(", ")
        )
    })?;
    // Every candidate installs to the folder.
    let install_dir = named[0].1.clone();
    Ok(Some(LibraryApp {
        appid,
        install_dir,
        vac,
    }))
}

/// The app an .exe at `relative` in their shared install folder, started
/// with `arguments`, belongs to among `candidates`, and whether Valve
/// Anti-Cheat comes with it. One app is its own. Of several, those whose
/// launch option starts the .exe; then those whose option's arguments the
/// .exe's include, the most of them (Half-Life 2: Episode One is `hl2.exe
/// -game episodic`); then games before tools and demos. The lowest app id
/// stands for those left. Valve Anti-Cheat comes with it when any app that
/// starts the .exe has it: the arguments only choose the app id, and the
/// .exe may still load the anti-cheat app's game. None when several share
/// the folder and none starts the .exe.
fn one_of(candidates: &[(u32, &Vdf)], relative: &str, arguments: &str) -> Option<(u32, bool)> {
    if let [(appid, info)] = candidates {
        return Some((*appid, vac(info)));
    }
    // `-game "cstrike"` is `-game cstrike`.
    let tokens = |arguments: &str| -> Vec<String> {
        arguments
            .split_whitespace()
            .map(|token| token.trim_matches('"').to_ascii_lowercase())
            .filter(|token| !token.is_empty())
            .collect()
    };
    let given = tokens(arguments);
    // Each app that starts the .exe, with the most of its options' arguments
    // the .exe has, when all of one option's are.
    let starting: Vec<(u32, &Vdf, Option<usize>)> = candidates
        .iter()
        .filter_map(|(appid, info)| {
            let options: Vec<String> = launch_options(info)
                .into_iter()
                .filter(|(exe, _)| {
                    exe.replace('/', "\\")
                        .trim_start_matches(".\\")
                        .trim_start_matches('\\')
                        .eq_ignore_ascii_case(relative)
                })
                .map(|(_, arguments)| arguments)
                .collect();
            let matched = options
                .iter()
                .filter_map(|option| {
                    let tokens = tokens(option);
                    tokens
                        .iter()
                        .all(|token| given.contains(token))
                        .then_some(tokens.len())
                })
                .max();
            (!options.is_empty()).then_some((*appid, *info, matched))
        })
        .collect();
    let best = starting.iter().filter_map(|(_, _, matched)| *matched).max();
    let mut left: Vec<(u32, &Vdf)> = starting
        .iter()
        .filter(|(_, _, matched)| best.is_none() || *matched == best)
        .map(|(appid, info, _)| (*appid, *info))
        .collect();
    let game = |info: &Vdf| {
        info.at(&["common", "type"])
            .and_then(Vdf::text)
            .is_some_and(|kind| kind.eq_ignore_ascii_case("game"))
    };
    if left.iter().any(|(_, info)| game(info)) {
        left.retain(|(_, info)| game(info));
    }
    let appid = left.iter().map(|(appid, _)| *appid).min()?;
    Some((appid, starting.iter().any(|(_, info, _)| vac(info))))
}

/// The executables and arguments of an app's launch options for Windows
/// that are no beta's.
fn launch_options(info: &Vdf) -> Vec<(String, String)> {
    let Some(options) = info.at(&["config", "launch"]) else {
        return vec![];
    };
    options
        .entries()
        .iter()
        .filter_map(|(_, option)| {
            let config = |key: &str| {
                option
                    .at(&["config", key])
                    .and_then(Vdf::text)
                    .unwrap_or("")
                    .to_ascii_lowercase()
            };
            let windows = config("oslist").is_empty() || config("oslist").contains("windows");
            let executable = option.get("executable").and_then(Vdf::text)?.trim();
            let arguments = option.get("arguments").and_then(Vdf::text).unwrap_or("");
            (windows && config("betakey").is_empty())
                .then(|| (executable.to_string(), arguments.trim().to_string()))
        })
        .collect()
}

/// Whether two `steamapps` folders on this PC's drives are one, also when
/// spelled differently (a junction, a short name, another case).
fn same_library(a: &Path, b: &Path) -> bool {
    same_path(a, b)
        || (on_this_pc(a)
            && on_this_pc(b)
            && matches!(
                (std::fs::canonicalize(a), std::fs::canonicalize(b)),
                (Ok(a), Ok(b)) if a == b
            ))
}

/// For an .exe below `<library>\steamapps\common\<folder>`, on this PC's
/// drives: that `steamapps` folder, `<folder>`, and the .exe's path below
/// it. Lexically resolves `.` and `..` segments so traversal inside a Steam
/// library (such as `win64\..\win64\cs2.exe`) still resolves to its app.
pub(super) fn library_folder(exe: &Path) -> Option<(PathBuf, String, String)> {
    let local = crate::nucleus::generic::on_this_pc(&exe.to_string_lossy())?;
    let mut raw = local.split('\\');
    let root = raw.next()?;
    let bytes = root.as_bytes();
    if bytes.len() != 2 || !bytes[0].is_ascii_alphabetic() || bytes[1] != b':' {
        return None;
    }
    let mut parts: Vec<&str> = vec![root];
    for part in raw {
        match part {
            "" | "." => {}
            ".." => {
                if parts.len() > 1 {
                    parts.pop();
                }
            }
            other => parts.push(other),
        }
    }
    let is = |at: usize, name: &str| parts.get(at).is_some_and(|p| p.eq_ignore_ascii_case(name));
    // The last `steamapps\common` with a folder and something in it below.
    let at = (1..parts.len().saturating_sub(3))
        .rev()
        .find(|&at| is(at, "steamapps") && is(at + 1, "common"))?;
    Some((
        PathBuf::from(parts[..=at].join("\\")),
        parts[at + 2].to_string(),
        parts[at + 3..].join("\\"),
    ))
}

/// `appid`'s entry in the app cache of the first of `roots` that has one.
fn cached_app(appid: u32, roots: &[PathBuf]) -> Result<Vdf, String> {
    cached_apps(&BTreeSet::from([appid]), roots)
        .remove(&appid)
        .ok_or_else(|| not_cached(appid))
}

/// The entries of `wanted` in the app cache of the first of `roots` that
/// has one.
fn cached_apps(wanted: &BTreeSet<u32>, roots: &[PathBuf]) -> BTreeMap<u32, Vdf> {
    roots
        .iter()
        .filter(|root| on_this_pc(root))
        .find_map(|root| std::fs::read(root.join("appcache").join("appinfo.vdf")).ok())
        .map(|data| appinfo_entries(&data, wanted))
        .unwrap_or_default()
}

fn not_cached(appid: u32) -> String {
    format!("Steam's app cache does not list app {appid}: start it once in Steam")
}

/// Whether the store lists Valve Anti-Cheat for an app (`common/category`
/// `category_8`).
fn vac(info: &Vdf) -> bool {
    info.at(&["common", "category", "category_8"])
        .and_then(Vdf::text)
        .is_some_and(|flag| flag.trim() == "1")
}

/// Whether Steam lists Valve Anti-Cheat for a game: for a Steam game's link
/// (`appid`) its app cache entry's flag, for an .exe started directly its
/// Steam app's ([`library_app`]), and none for an .exe in a plain folder.
/// An error when Steam can't say.
pub fn valve_anti_cheat(
    appid: Option<u32>,
    exe: &Path,
    arguments: &str,
    roots: &[PathBuf],
) -> Result<bool, String> {
    match appid {
        Some(appid) => cached_app(appid, roots).map(|info| vac(&info)),
        None => Ok(library_app(exe, arguments, roots)?.is_some_and(|app| app.vac)),
    }
}

/// The executable of the launch option (`config/launch`) Steam starts on
/// Windows, relative to the install folder, and its arguments.
fn launch_executable(info: &Vdf) -> Option<(String, String)> {
    let options = info.at(&["config", "launch"])?.entries();
    options
        .iter()
        .filter_map(|(_, option)| {
            let executable = option.get("executable").and_then(Vdf::text)?.trim();
            let config = |key: &str| {
                option
                    .at(&["config", key])
                    .and_then(Vdf::text)
                    .unwrap_or("")
                    .to_ascii_lowercase()
            };
            let windows = config("oslist").is_empty() || config("oslist").contains("windows");
            let kind = option
                .get("type")
                .and_then(Vdf::text)
                .unwrap_or("")
                .to_ascii_lowercase();
            (windows
                && config("betakey").is_empty()
                && executable.to_ascii_lowercase().ends_with(".exe")
                && matches!(kind.as_str(), "" | "default" | "none"))
            .then(|| {
                let arguments = option.get("arguments").and_then(Vdf::text).unwrap_or("");
                (
                    config("osarch") == "32",
                    (executable.to_string(), arguments.trim().to_string()),
                )
            })
        })
        // 64-bit first, then Steam's order.
        .min_by_key(|(only_32, _)| *only_32)
        .map(|(_, launch)| launch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_folder_normalizes_dot_dot_and_verbatim_paths_inside_steam_library() {
        assert_eq!(
            library_folder(Path::new(
                r"C:\Steam\steamapps\common\CSGO\game\bin\win64\..\win64\cs2.exe"
            )),
            Some((
                PathBuf::from(r"C:\Steam\steamapps"),
                "CSGO".into(),
                r"game\bin\win64\cs2.exe".into()
            ))
        );
        assert_eq!(
            library_folder(Path::new(
                r"C:\Steam\steamapps\common\Other\..\CSGO\game\bin\win64\cs2.exe"
            )),
            Some((
                PathBuf::from(r"C:\Steam\steamapps"),
                "CSGO".into(),
                r"game\bin\win64\cs2.exe".into()
            ))
        );
        assert_eq!(
            library_folder(Path::new(
                r"\\?\C:\Steam\steamapps\common\CSGO\game\bin\win64\..\win64\cs2.exe"
            )),
            Some((
                PathBuf::from(r"C:\Steam\steamapps"),
                "CSGO".into(),
                r"game\bin\win64\cs2.exe".into()
            ))
        );
        assert_eq!(
            library_folder(Path::new(r"\\.\C:\Steam\steamapps\common\CSGO\cs2.exe")),
            Some((
                PathBuf::from(r"C:\Steam\steamapps"),
                "CSGO".into(),
                "cs2.exe".into()
            ))
        );
        // Traversing out of `steamapps\common` is no longer in the library.
        assert_eq!(
            library_folder(Path::new(
                r"D:\Steam\steamapps\common\Game\..\..\..\Other\g.exe"
            )),
            None
        );
    }

    #[test]
    fn vac_check_is_not_bypassed_by_dot_dot_or_verbatim_prefix() {
        let root = tempfile::tempdir().unwrap();
        let steamapps = root.path().join("steamapps");
        let bin = steamapps.join(r"common\CSGO\game\bin\win64");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("cs2.exe"), b"MZ").unwrap();
        std::fs::write(
            steamapps.join("appmanifest_730.acf"),
            "\"AppState\" { \"appid\" \"730\" \"installdir\" \"CSGO\" }",
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
        kv.extend(b"1\0");
        kv.extend([0x08; 5]);
        let mut data = 0x0756_4428u32.to_le_bytes().to_vec();
        data.extend(1u32.to_le_bytes());
        data.extend(730u32.to_le_bytes());
        data.extend(((60 + kv.len()) as u32).to_le_bytes());
        data.extend([0; 60]);
        data.extend(kv);
        data.extend(0u32.to_le_bytes());
        std::fs::create_dir_all(root.path().join("appcache")).unwrap();
        std::fs::write(root.path().join(r"appcache\appinfo.vdf"), data).unwrap();

        let roots = [root.path().to_path_buf()];
        let traversal = PathBuf::from(format!(
            r"{}\steamapps\common\CSGO\game\bin\win64\..\win64\cs2.exe",
            root.path().display()
        ));
        let verbatim = PathBuf::from(format!(
            r"\\?\{}\steamapps\common\CSGO\game\bin\win64\cs2.exe",
            root.path().display()
        ));
        let both = PathBuf::from(format!(
            r"\\?\{}\steamapps\common\CSGO\game\bin\win64\..\win64\cs2.exe",
            root.path().display()
        ));
        for exe in [&traversal, &verbatim, &both] {
            assert_eq!(
                valve_anti_cheat(None, exe, "", &roots),
                Ok(true),
                "{}",
                exe.display()
            );
        }
    }
}
