//! Steam's installed games on Linux, read from its files: the libraries in
//! `libraryfolders.vdf`, the games in `appmanifest_*.acf`, their names and types from the
//! app cache, play history from the users' `localconfig.vdf` and covers from the library
//! cache. It does what `butterpollo_session::steam::discover` does on Windows, which takes
//! no Linux path for a path on this PC (it keeps drive paths only), and reuses that crate's
//! VDF reader, app cache reader and cover lookup, which have no such limit.

use butterpollo_session::steam::{self, Game, Vdf};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The largest VDF file read: Steam's own are a few hundred kilobytes.
const MAX_VDF_BYTES: u64 = 32 << 20;
/// The offset between a 64-bit Steam ID and the account number that names a `userdata` folder.
const ACCOUNT_BASE: u64 = 76_561_197_960_265_728;

/// A user who signed in to Steam on this PC, from `config/loginusers.vdf`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct User {
    /// The 64-bit Steam ID.
    pub steam_id: u64,
    pub account: String,
    pub persona: String,
    /// Steam's most recently used account, the one that signs in by itself.
    pub most_recent: bool,
}

impl User {
    /// The name of this user's folder below `userdata`.
    pub fn account_id(&self) -> u64 {
        self.steam_id.saturating_sub(ACCOUNT_BASE)
    }
}

fn read(path: &Path) -> Option<String> {
    use std::io::Read;
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            tracing::warn!(%error, file = %path.display(), "a Steam file could not be read");
            return None;
        }
    };
    // A VDF is text; a binary file or a damaged one is read lossily and parses to nothing.
    let mut bytes = vec![];
    if let Err(error) = file.take(MAX_VDF_BYTES).read_to_end(&mut bytes) {
        tracing::warn!(%error, file = %path.display(), "a Steam file could not be read");
        return None;
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// The users who signed in to the Steam installation `root`, most recent first.
pub fn users(root: &Path) -> Vec<User> {
    let Some(text) = read(&root.join("config/loginusers.vdf")) else {
        return vec![];
    };
    let document = Vdf::parse(&text);
    let mut users: Vec<(u64, User)> = document
        .get("users")
        .map(Vdf::entries)
        .unwrap_or_default()
        .iter()
        .filter_map(|(id, user)| {
            let text = |key: &str| user.get(key).and_then(Vdf::text).unwrap_or("").to_owned();
            let flag = user.get("MostRecent").and_then(Vdf::number) == Some(1);
            let since = user.get("Timestamp").and_then(Vdf::number).unwrap_or(0);
            Some((
                since,
                User {
                    steam_id: id.parse().ok()?,
                    account: text("AccountName"),
                    persona: text("PersonaName"),
                    most_recent: flag,
                },
            ))
        })
        .collect();
    users.sort_by_key(|(since, user)| (!user.most_recent, std::cmp::Reverse(*since)));
    users.into_iter().map(|(_, user)| user).collect()
}

/// The `steamapps` folders of the installations `roots` and of the libraries they list,
/// each once.
fn libraries(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = vec![];
    let mut add = |path: PathBuf| {
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        if path.is_dir() && !found.contains(&path) {
            found.push(path);
        }
    };
    for root in roots {
        let Some(apps) = steam::steamapps(root) else {
            continue;
        };
        add(apps.clone());
        // Steam keeps the list in `steamapps` and a copy in `config`.
        let listed = [
            apps.join("libraryfolders.vdf"),
            root.join("config/libraryfolders.vdf"),
        ]
        .iter()
        .find_map(|file| read(file));
        for path in listed.iter().flat_map(|text| listed_paths(text)) {
            add(PathBuf::from(path).join("steamapps"));
        }
    }
    found
}

/// The library folders a `libraryfolders.vdf` lists. Old files give the path as the value.
fn listed_paths(text: &str) -> Vec<String> {
    let document = Vdf::parse(text);
    let folders = document.get("libraryfolders").unwrap_or(&document);
    folders
        .entries()
        .iter()
        .filter_map(|(_, folder)| folder.get("path").or(Some(folder)).and_then(Vdf::text))
        .filter(|path| Path::new(path).is_absolute())
        .map(str::to_owned)
        .collect()
}

/// The installed apps of one `steamapps` folder.
fn manifests(apps: &Path, games: &mut BTreeMap<u32, Game>) {
    let Ok(entries) = std::fs::read_dir(apps) else {
        return;
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("appmanifest_") && name.ends_with(".acf"))
        })
        .collect();
    files.sort();
    for file in files {
        let Some(text) = read(&file) else {
            continue;
        };
        let document = Vdf::parse(&text);
        let Some(state) = document.get("AppState") else {
            tracing::warn!(file = %file.display(), "a Steam app manifest has no AppState");
            continue;
        };
        let appid = state
            .get("appid")
            .and_then(Vdf::number)
            .and_then(|id| u32::try_from(id).ok())
            .filter(|id| *id > 0);
        let folder = state.get("installdir").and_then(Vdf::text).unwrap_or("");
        let (Some(appid), false) = (appid, folder.is_empty() || folder.contains(['/', '\\']))
        else {
            continue;
        };
        // Bit 4 of the state is "fully installed"; a download that has not finished is not.
        let flags = state.get("StateFlags").and_then(Vdf::number).unwrap_or(4);
        let install_dir = apps.join("common").join(folder);
        if flags & 4 == 0 || !install_dir.is_dir() || games.contains_key(&appid) {
            continue;
        }
        games.insert(
            appid,
            Game {
                appid,
                name: state.get("name").and_then(Vdf::text).unwrap_or("").into(),
                app_type: state.get("type").and_then(Vdf::text).unwrap_or("").into(),
                install_dir,
                library_path: apps.parent().unwrap_or(apps).to_owned(),
                installed: true,
                ..Default::default()
            },
        );
    }
}

/// `LastPlayed` and `Playtime` from the users' `localconfig.vdf` below `root`.
fn history(root: &Path, games: &mut BTreeMap<u32, Game>) {
    let Ok(users) = std::fs::read_dir(root.join("userdata")) else {
        return;
    };
    for user in users.flatten() {
        let Some(text) = read(&user.path().join("config/localconfig.vdf")) else {
            continue;
        };
        let document = Vdf::parse(&text);
        let Some(apps) =
            document.at(&["UserLocalConfigStore", "Software", "Valve", "Steam", "apps"])
        else {
            continue;
        };
        for game in games.values_mut() {
            game.history = true;
        }
        for (id, app) in apps.entries() {
            let Some(game) = id.parse().ok().and_then(|id: u32| games.get_mut(&id)) else {
                continue;
            };
            if let Some(last) = app.get("LastPlayed").and_then(Vdf::number) {
                game.last_played = game.last_played.max(last);
            }
            if let Some(minutes) = app.get("Playtime").and_then(Vdf::number) {
                game.playtime_minutes = game.playtime_minutes.max(minutes);
            }
        }
    }
}

/// Every installed app in the libraries of the Steam installations `roots`, with names and
/// types from the app cache, play history and covers.
pub fn discover(roots: &[PathBuf]) -> Vec<Game> {
    let mut games = BTreeMap::new();
    for apps in libraries(roots) {
        manifests(&apps, &mut games);
    }
    let installations: Vec<&PathBuf> = roots
        .iter()
        .filter(|root| root.join("userdata").is_dir())
        .collect();
    for root in &installations {
        history(root, &mut games);
    }
    let wanted: BTreeSet<u32> = games.keys().copied().collect();
    for root in roots {
        let Ok(bytes) = std::fs::read(root.join("appcache/appinfo.vdf")) else {
            continue;
        };
        for (appid, (name, kind)) in steam::appinfo(&bytes, &wanted) {
            let Some(game) = games.get_mut(&appid) else {
                continue;
            };
            if !name.is_empty() {
                game.name = name;
            }
            if !kind.is_empty() {
                game.app_type = kind.to_ascii_lowercase();
            }
        }
        break;
    }
    for game in games.values_mut() {
        let (portrait, header, icon) = steam::artwork(game.appid, &game.library_path, roots);
        game.portrait_path = portrait;
        game.header_path = header;
        game.icon_path = icon;
    }
    games
        .into_values()
        .filter(|game| !game.name.is_empty())
        .collect()
}
