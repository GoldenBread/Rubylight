//! Steam library discovery and the apps it maintains, as in Vibepollo 2.0.
//!
//! Only local files are read: `libraryfolders.vdf`, `appmanifest_*.acf`,
//! `userdata/*/config/localconfig.vdf`, `userdata/*/config/cloudstorage` (the
//! collections, read only), `appcache/appinfo.vdf` and the
//! library cache artwork. Apps the sync adds carry `steam-managed: "auto"`,
//! a UUID derived from the Steam app ID and `synced-by: steam`; only the
//! stamp makes an app the sync's own, since Vibepollo writes the rest too.
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex, PoisonError},
    time::SystemTime,
};

mod artwork;
mod collections;
mod launch;
mod sync;
mod tracker;
mod vdf;
pub use artwork::{artwork, hashed_store_portrait_url, store_portrait_url};
use collections::collections_file;
pub use collections::{Collections, LastCollections, MAX_FAILED_COLLECTION_READS, collections};
pub use launch::{Launch, LibraryApp, game_executable, library_app, valve_anti_cheat};
pub use sync::{
    FALLBACK_IMAGE, Settings, app_id_of, canonical_uuid, game_of, id_names, importable,
    is_excluded, launch_command, launched_through_steam, reconcile, select,
};
pub use tracker::{Process, Tracked, Tracker};
use vdf::appinfo_entries;
pub use vdf::{Vdf, appinfo};

/// An installed (or recently played) Steam app.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Game {
    pub appid: u32,
    pub name: String,
    pub app_type: String,
    pub install_dir: PathBuf,
    pub library_path: PathBuf,
    pub installed: bool,
    /// Unix seconds; 0 when never played (or `history` is false).
    pub last_played: u64,
    /// A user's `localconfig.vdf` was read, so `last_played` 0 is never played,
    /// not unknown.
    #[serde(skip)]
    pub history: bool,
    pub playtime_minutes: u64,
    pub portrait_path: Option<PathBuf>,
    pub header_path: Option<PathBuf>,
    pub icon_path: Option<PathBuf>,
}
impl Game {
    /// The best local cover: portrait, else header, else icon.
    pub fn artwork(&self) -> Option<&Path> {
        self.portrait_path
            .as_deref()
            .or(self.header_path.as_deref())
            .or(self.icon_path.as_deref())
    }
}

/// The `steamapps` folder of a Steam or library root.
pub fn steamapps(root: &Path) -> Option<PathBuf> {
    let named = root
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("steamapps"));
    if (named && root.is_dir()) || root.join("libraryfolders.vdf").is_file() {
        return Some(root.to_owned());
    }
    let nested = root.join("steamapps");
    nested.is_dir().then_some(nested)
}
/// Every installed app in the libraries of `roots` (Steam installations),
/// with play history, names and types from the app cache, and artwork.
pub fn discover(roots: &[PathBuf]) -> Vec<Game> {
    let roots = &only_on_this_pc(roots);
    let libraries = libraries(roots);
    let mut games: BTreeMap<u32, Game> = BTreeMap::new();
    for steamapps in &libraries {
        for manifest in manifests(steamapps) {
            let Some((appid, state, install_dir)) = installed(steamapps, &manifest) else {
                continue;
            };
            if games.contains_key(&appid) {
                continue;
            }
            games.insert(
                appid,
                Game {
                    appid,
                    name: state.get("name").and_then(Vdf::text).unwrap_or("").into(),
                    app_type: state.get("type").and_then(Vdf::text).unwrap_or("").into(),
                    install_dir,
                    library_path: steamapps.parent().unwrap_or(steamapps).to_owned(),
                    installed: true,
                    ..Default::default()
                },
            );
        }
    }
    let steam_roots: Vec<&PathBuf> = roots
        .iter()
        .filter(|r| r.join("userdata").is_dir())
        .collect();
    for root in &steam_roots {
        apply_history(root, &mut games);
    }
    let wanted: BTreeSet<u32> = games.keys().copied().collect();
    for root in &steam_roots {
        if let Some(found) = app_names(&root.join("appcache").join("appinfo.vdf"), &wanted) {
            for (appid, (name, kind)) in found {
                let game = games.get_mut(&appid).unwrap();
                if !name.is_empty() {
                    game.name = name;
                }
                if !kind.is_empty() {
                    game.app_type = kind.to_ascii_lowercase();
                }
            }
            break;
        }
    }
    for game in games.values_mut() {
        let (portrait, header, icon) = artwork(game.appid, &game.library_path, roots);
        game.portrait_path = portrait;
        game.header_path = header;
        game.icon_path = icon;
    }
    games.into_values().filter(|g| !g.name.is_empty()).collect()
}
/// The names and types [`appinfo`] reads for `wanted`, by app.
type AppNames = BTreeMap<u32, (String, String)>;
/// The app cache file the last [`app_names`] read: its path, size and time, and the apps asked for.
type AppNamesKey = (PathBuf, u64, std::time::SystemTime, BTreeSet<u32>);
/// The last app cache read. The file runs to tens of megabytes and the library checks read it
/// every few minutes, mostly unchanged.
static APP_NAMES: std::sync::Mutex<Option<(AppNamesKey, AppNames)>> = std::sync::Mutex::new(None);
/// The names and types of `wanted` in the app cache at `path`, `None` when it cannot be read. The
/// file is read again only when its size, its time or `wanted` changed since the last read.
fn app_names(path: &Path, wanted: &BTreeSet<u32>) -> Option<AppNames> {
    let stamp = std::fs::metadata(path)
        .ok()
        .and_then(|m| Some((m.len(), m.modified().ok()?)));
    let key = stamp.map(|(size, modified)| (path.to_owned(), size, modified, wanted.clone()));
    if let Some(key) = &key
        && let Some((last, found)) = &*APP_NAMES.lock().unwrap_or_else(|e| e.into_inner())
        && last == key
    {
        return Some(found.clone());
    }
    let found = appinfo(&std::fs::read(path).ok()?, wanted);
    if let Some(key) = key {
        *APP_NAMES.lock().unwrap_or_else(|e| e.into_inner()) = Some((key, found.clone()));
    }
    Some(found)
}
/// The most this reads of one of Steam's text files. Any user can write to Steam's folder and the
/// host reads it as SYSTEM: a huge file must not make it allocate until it aborts.
const MAX_STEAM_TEXT: u64 = 16 * 1024 * 1024;

/// The file's text, or an error when it is over [`MAX_STEAM_TEXT`] (also one still growing).
fn read_capped(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let too_large = |size: u64| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{size} bytes, more than the {MAX_STEAM_TEXT} this reads"),
        )
    };
    let file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    if size > MAX_STEAM_TEXT {
        return Err(too_large(size));
    }
    let mut text = String::new();
    file.take(MAX_STEAM_TEXT + 1).read_to_string(&mut text)?;
    if text.len() as u64 > MAX_STEAM_TEXT {
        return Err(too_large(text.len() as u64));
    }
    Ok(text)
}

/// A file's parse, keyed by path, with the size and time it was read at.
type StampCache<T> = Mutex<HashMap<PathBuf, ((u64, SystemTime), T)>>;
/// A folder's listing, keyed by path, with the time it was read at.
type TimeCache<T> = Mutex<HashMap<PathBuf, (SystemTime, T)>>;
/// What changes when an app is installed or removed, or a collection edited: the libraries'
/// manifest names, their `common` folders' times and the collections file. Not a manifest's
/// content, which a download rewrites all the time, so reading this every few seconds is cheap and
/// a running download starts no scans. Equal only within one run.
pub fn library_stamp(roots: &[PathBuf]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for steamapps in libraries(roots) {
        steamapps.hash(&mut hasher);
        manifests(&steamapps).hash(&mut hasher);
        std::fs::metadata(steamapps.join("common"))
            .and_then(|folder| folder.modified())
            .ok()
            .hash(&mut hasher);
    }
    if let Some(file) = collections_file(roots) {
        file.hash(&mut hasher);
        if let Ok(metadata) = std::fs::metadata(&file) {
            metadata.len().hash(&mut hasher);
            metadata.modified().ok().hash(&mut hasher);
        }
    }
    hasher.finish()
}

/// Whether `path` is on one of this PC's drives. The host runs as SYSTEM:
/// opening a network share would hand it the PC's account, so a library on
/// a share is skipped before anything opens it.
fn on_this_pc(path: &Path) -> bool {
    crate::nucleus::generic::on_this_pc(&path.to_string_lossy()).is_some()
}

/// The roots on this PC's drives ([`on_this_pc`]).
fn only_on_this_pc(roots: &[PathBuf]) -> Vec<PathBuf> {
    roots
        .iter()
        .filter(|root| on_this_pc(root))
        .cloned()
        .collect()
}

/// The library paths a `libraryfolders.vdf` listed, as text, with the file's size and time it
/// was read. The library checks ask for the libraries every few seconds and Steam rewrites the
/// file only when a library folder comes or goes, so it is parsed again only when its size or
/// time moved. Only the paths are cached: whether a folder is there is checked on every call, as
/// a drive can appear after the file was read. A read that fails is not cached.
static VDF_LIBRARIES: LazyLock<StampCache<Vec<String>>> = LazyLock::new(Default::default);
/// The libraries a `steamapps` folder's `libraryfolders.vdf` lists, as [`libraries`] reads them:
/// on this PC's drives, the path from old files' values included.
fn vdf_libraries(apps: &Path) -> Vec<PathBuf> {
    let file = apps.join("libraryfolders.vdf");
    let stamp = std::fs::metadata(&file)
        .ok()
        .and_then(|metadata| Some((metadata.len(), metadata.modified().ok()?)));
    let Some(stamp) = stamp else {
        // No file to cache by: read it, as before.
        return read_capped(&file)
            .ok()
            .map(|text| existing_libraries(&vdf_listed_paths(&text)))
            .unwrap_or_default();
    };
    let mut read = VDF_LIBRARIES.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((last, listed)) = read.get(apps)
        && *last == stamp
    {
        return existing_libraries(listed);
    }
    let Some(text) = read_capped(&file).ok() else {
        // Unreadable now, so not cached: the next check reads it again.
        return vec![];
    };
    let listed = vdf_listed_paths(&text);
    let libraries = existing_libraries(&listed);
    read.insert(apps.to_owned(), (stamp, listed));
    libraries
}
/// The library paths the text of `libraryfolders.vdf` lists on this PC's drives. Old files give
/// the path as the value itself.
fn vdf_listed_paths(text: &str) -> Vec<String> {
    let folders = Vdf::parse(text);
    let folders = folders.get("libraryfolders").unwrap_or(&folders);
    folders
        .entries()
        .iter()
        .filter_map(|(_, folder)| folder.get("path").or(Some(folder)).and_then(Vdf::text))
        .filter(|path| !path.is_empty() && on_this_pc(Path::new(path)))
        .map(str::to_owned)
        .collect()
}
/// The `steamapps` folders of the listed library paths that exist now.
fn existing_libraries(paths: &[String]) -> Vec<PathBuf> {
    paths
        .iter()
        .filter_map(|path| steamapps(Path::new(path)))
        .collect()
}
/// The `steamapps` folders of every library of `roots` on this PC's drives.
fn libraries(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut libraries: Vec<PathBuf> = vec![];
    let add = |path: PathBuf, libraries: &mut Vec<PathBuf>| {
        if !libraries.iter().any(|known| same_path(known, &path)) {
            libraries.push(path);
        }
    };
    for root in roots.iter().filter(|root| on_this_pc(root)) {
        let Some(apps) = steamapps(root) else {
            continue;
        };
        add(apps.clone(), &mut libraries);
        for library in vdf_libraries(&apps) {
            add(library, &mut libraries);
        }
    }
    libraries
}
/// The manifest names of a `steamapps` folder, with the folder's time they were read. The library
/// checks ask every few seconds: the folder's time moves when a manifest appears, is renamed or
/// goes, and a download's rewrites of a manifest's content leave it put, so the names are read
/// again only when that time moved. A folder whose time cannot be read is not cached.
static MANIFESTS: LazyLock<TimeCache<Vec<PathBuf>>> = LazyLock::new(Default::default);
/// The `appmanifest_*.acf` files of a `steamapps` folder, in name order.
fn manifests(steamapps: &Path) -> Vec<PathBuf> {
    let modified = std::fs::metadata(steamapps)
        .ok()
        .and_then(|m| m.modified().ok());
    let Some(modified) = modified else {
        return manifest_names(steamapps).unwrap_or_default();
    };
    let mut read = MANIFESTS.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((last, names)) = read.get(steamapps)
        && *last == modified
    {
        return names.clone();
    }
    let Some(names) = manifest_names(steamapps) else {
        // Unreadable now, so not cached: the next check reads it again.
        return vec![];
    };
    // A folder changed within the clock's granularity may change again with the same time, so its
    // names are cached only once that time is a moment old.
    let settled = std::time::SystemTime::now()
        .duration_since(modified)
        .is_ok_and(|age| age >= MANIFESTS_SETTLE);
    if settled {
        read.insert(steamapps.to_owned(), (modified, names.clone()));
    }
    names
}

/// How old a `steamapps` folder's time must be before its manifest names are cached.
const MANIFESTS_SETTLE: std::time::Duration = std::time::Duration::from_secs(2);
/// The folder's manifest names, `None` when they cannot be read.
fn manifest_names(steamapps: &Path) -> Option<Vec<PathBuf>> {
    let entries = std::fs::read_dir(steamapps).ok()?;
    let mut manifests: Vec<_> = entries
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|name| {
                    let name = name.to_ascii_lowercase();
                    name.starts_with("appmanifest_") && name.ends_with(".acf")
                })
        })
        .collect();
    manifests.sort();
    Some(manifests)
}
/// A manifest's app, its `AppState` and its install folder, when the folder
/// is there.
fn installed(steamapps: &Path, manifest: &Path) -> Option<(u32, Vdf, PathBuf)> {
    let text = std::fs::read_to_string(manifest).ok()?;
    let state = Vdf::parse(&text).get("AppState")?.clone();
    let appid = state
        .get("appid")
        .and_then(Vdf::number)
        .and_then(|id| u32::try_from(id).ok())
        .filter(|id| *id > 0)?;
    let folder = state.get("installdir").and_then(Vdf::text).unwrap_or("");
    let install_dir = steamapps.join("common").join(folder);
    (!folder.is_empty() && install_dir.is_dir()).then_some((appid, state, install_dir))
}
fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .eq_ignore_ascii_case(
            b.to_string_lossy()
                .replace('/', "\\")
                .trim_end_matches('\\'),
        )
}
/// LastPlayed and Playtime from every signed-in user's local config.
fn apply_history(root: &Path, games: &mut BTreeMap<u32, Game>) {
    let Ok(users) = std::fs::read_dir(root.join("userdata")) else {
        return;
    };
    for user in users.flatten() {
        let file = user.path().join("config").join("localconfig.vdf");
        let text = match read_capped(&file) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                tracing::warn!(%error, file = %file.display(), "Steam's play history could not be read");
                continue;
            }
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

#[cfg(test)]
mod tests;
