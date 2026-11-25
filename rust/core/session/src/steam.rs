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
    is_excluded, launch_command, launched_through_steam, reconcile, reconcile_checked, select,
};
pub use tracker::{Process, Tracked, Tracker};
use vdf::appinfo_entries;
pub use vdf::{Vdf, appinfo};

/// Outcome of scanning configured Steam roots, preserving which libraries and
/// manifests could not be read so uninstalled-game cleanup skips them.
#[derive(Clone, Debug, Default)]
pub struct Discovery {
    pub games: Vec<Game>,
    pub readable_libraries: Vec<PathBuf>,
    pub unreadable_libraries: Vec<PathBuf>,
    pub unreadable_apps: BTreeSet<u32>,
    pub incomplete: bool,
}

impl Discovery {
    /// Whether removing `app` during `reconcile_checked` is safe (its library
    /// and manifest were read completely).
    pub fn allows_removal(&self, app: &serde_json::Value, appid: Option<u32>) -> bool {
        if let Some(id) = appid
            && self.unreadable_apps.contains(&id)
        {
            return false;
        }
        let install_dir = app
            .get("steam-install-dir")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        if !install_dir.is_empty() {
            if self
                .unreadable_libraries
                .iter()
                .any(|lib| dir_in_library(install_dir, lib))
            {
                return false;
            }
            if self
                .readable_libraries
                .iter()
                .any(|lib| dir_in_library(install_dir, lib))
            {
                return true;
            }
        }
        !self.incomplete && self.unreadable_libraries.is_empty() && self.unreadable_apps.is_empty()
    }
}

fn dir_in_library(install_dir: &str, steamapps: &Path) -> bool {
    let norm = |s: &str| {
        s.replace('/', "\\")
            .trim_end_matches('\\')
            .to_ascii_lowercase()
    };
    let dir = norm(install_dir);
    let prefix = format!("{}\\common\\", norm(&steamapps.to_string_lossy()));
    dir.starts_with(&prefix)
}

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
    discover_checked(roots).games
}

/// Like [`discover`], while recording any unreadable Steam roots, library
/// folders, or manifests so sync never deletes apps on unreachable drives.
pub fn discover_checked(roots: &[PathBuf]) -> Discovery {
    let mut discovery = Discovery::default();
    let local_roots = only_on_this_pc(roots);
    if local_roots.is_empty() {
        discovery.incomplete = true;
        return discovery;
    }
    let libraries = libraries_checked(&local_roots, &mut discovery);
    let mut games: BTreeMap<u32, Game> = BTreeMap::new();
    for steamapps in &libraries {
        let Some(manifests) = manifests_checked(steamapps) else {
            discovery.incomplete = true;
            add_unique_path(&mut discovery.unreadable_libraries, steamapps.clone());
            continue;
        };
        let mut library_clean = true;
        for manifest in manifests {
            match installed_checked(steamapps, &manifest) {
                ManifestOutcome::App(appid, state, install_dir) => {
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
                ManifestOutcome::Unreadable(appid) => {
                    library_clean = false;
                    discovery.incomplete = true;
                    if let Some(id) = appid {
                        discovery.unreadable_apps.insert(id);
                    }
                    add_unique_path(&mut discovery.unreadable_libraries, steamapps.clone());
                }
                ManifestOutcome::NotInstalled => {}
            }
        }
        if library_clean {
            add_unique_path(&mut discovery.readable_libraries, steamapps.clone());
        }
    }
    let steam_roots: Vec<&PathBuf> = local_roots
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
        let (portrait, header, icon) = artwork(game.appid, &game.library_path, &local_roots);
        game.portrait_path = portrait;
        game.header_path = header;
        game.icon_path = icon;
    }
    discovery.games = games.into_values().filter(|g| !g.name.is_empty()).collect();
    discovery
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
const MAX_APPINFO_BYTES: u64 = 256 * 1024 * 1024;

fn app_names(path: &Path, wanted: &BTreeSet<u32>) -> Option<AppNames> {
    let stamp = std::fs::metadata(path)
        .ok()
        .and_then(|m| Some((m.len(), m.modified().ok()?)));
    if stamp.is_some_and(|(size, _)| size > MAX_APPINFO_BYTES) {
        return None;
    }
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
/// Reads `libraryfolders.vdf` using the `(len, modified)` cache when metadata is available.
fn vdf_listed_cached(apps: &Path) -> std::io::Result<Vec<String>> {
    let file = apps.join("libraryfolders.vdf");
    let stamp = std::fs::metadata(&file)
        .ok()
        .and_then(|metadata| Some((metadata.len(), metadata.modified().ok()?)));
    if let Some(stamp) = stamp {
        let read = VDF_LIBRARIES.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((last, listed)) = read.get(apps)
            && *last == stamp
        {
            return Ok(listed.clone());
        }
        drop(read);
        let text = read_capped(&file)?;
        let listed = vdf_listed_paths(&text);
        VDF_LIBRARIES
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(apps.to_owned(), (stamp, listed.clone()));
        return Ok(listed);
    }
    Ok(vdf_listed_paths(&read_capped(&file)?))
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
fn add_unique_path(list: &mut Vec<PathBuf>, path: PathBuf) {
    if !list.iter().any(|known| same_path(known, &path)) {
        list.push(path);
    }
}

fn expected_steamapps(path: &Path) -> PathBuf {
    let named = path
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("steamapps"));
    if named || path.join("libraryfolders.vdf").is_file() {
        path.to_path_buf()
    } else {
        path.join("steamapps")
    }
}

/// The `steamapps` folders of every library of `roots` on this PC's drives.
fn libraries(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut dummy = Discovery::default();
    libraries_checked(roots, &mut dummy)
}

fn libraries_checked(roots: &[PathBuf], discovery: &mut Discovery) -> Vec<PathBuf> {
    let mut libraries: Vec<PathBuf> = vec![];
    for root in roots.iter().filter(|root| on_this_pc(root)) {
        let Some(apps) = steamapps(root) else {
            discovery.incomplete = true;
            add_unique_path(
                &mut discovery.unreadable_libraries,
                expected_steamapps(root),
            );
            continue;
        };
        add_unique_path(&mut libraries, apps.clone());
        match vdf_listed_cached(&apps) {
            Ok(listed) => {
                for raw in listed {
                    let candidate = Path::new(&raw);
                    if !on_this_pc(candidate) {
                        continue;
                    }
                    if let Some(lib) = steamapps(candidate) {
                        add_unique_path(&mut libraries, lib);
                    } else {
                        discovery.incomplete = true;
                        add_unique_path(
                            &mut discovery.unreadable_libraries,
                            expected_steamapps(candidate),
                        );
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                discovery.incomplete = true;
            }
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
    manifests_checked(steamapps).unwrap_or_default()
}

fn manifests_checked(steamapps: &Path) -> Option<Vec<PathBuf>> {
    let modified = std::fs::metadata(steamapps)
        .ok()
        .and_then(|m| m.modified().ok());
    let Some(modified) = modified else {
        return manifest_names(steamapps);
    };
    let mut read = MANIFESTS.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((last, names)) = read.get(steamapps)
        && *last == modified
    {
        return Some(names.clone());
    }
    let names = manifest_names(steamapps)?;
    // A folder changed within the clock's granularity may change again with the same time, so its
    // names are cached only once that time is a moment old.
    let settled = std::time::SystemTime::now()
        .duration_since(modified)
        .is_ok_and(|age| age >= MANIFESTS_SETTLE);
    if settled {
        read.insert(steamapps.to_owned(), (modified, names.clone()));
    }
    Some(names)
}

/// How old a `steamapps` folder's time must be before its manifest names are cached.
const MANIFESTS_SETTLE: std::time::Duration = std::time::Duration::from_secs(2);
/// The folder's manifest names, `None` when they cannot be read completely.
fn manifest_names(steamapps: &Path) -> Option<Vec<PathBuf>> {
    let entries = std::fs::read_dir(steamapps).ok()?;
    let mut manifests = Vec::new();
    for entry in entries {
        let entry = entry.ok()?;
        let path = entry.path();
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|name| {
                let name = name.to_ascii_lowercase();
                name.starts_with("appmanifest_") && name.ends_with(".acf")
            })
        {
            manifests.push(path);
        }
    }
    manifests.sort();
    Some(manifests)
}

enum ManifestOutcome {
    App(u32, Vdf, PathBuf),
    NotInstalled,
    Unreadable(Option<u32>),
}

fn manifest_appid_from_name(manifest: &Path) -> Option<u32> {
    let stem = manifest.file_stem()?.to_str()?;
    stem.to_ascii_lowercase()
        .strip_prefix("appmanifest_")?
        .parse::<u32>()
        .ok()
        .filter(|id| *id > 0)
}

/// Validate `installdir` is a single safe directory name under `steamapps/common`
/// on a local PC drive without path traversal or network escapes.
fn safe_install_dir(steamapps: &Path, folder: &str) -> Option<PathBuf> {
    let trimmed = folder.trim();
    if trimmed.is_empty()
        || trimmed == "."
        || trimmed == ".."
        || trimmed.contains(['/', '\\', ':', '\0'])
    {
        return None;
    }
    let common = steamapps.join("common");
    let install_dir = common.join(trimmed);
    if !on_this_pc(&install_dir) || !install_dir.is_dir() {
        return None;
    }
    if let (Ok(canon_common), Ok(canon_install)) = (
        std::fs::canonicalize(&common),
        std::fs::canonicalize(&install_dir),
    ) && (!on_this_pc(&canon_install) || !canon_install.starts_with(&canon_common))
    {
        return None;
    }
    Some(install_dir)
}

/// A manifest's app, its `AppState` and its install folder, when the folder
/// is there and local to this PC.
fn installed(steamapps: &Path, manifest: &Path) -> Option<(u32, Vdf, PathBuf)> {
    match installed_checked(steamapps, manifest) {
        ManifestOutcome::App(appid, state, dir) => Some((appid, state, dir)),
        _ => None,
    }
}

fn installed_checked(steamapps: &Path, manifest: &Path) -> ManifestOutcome {
    let Ok(text) = read_capped(manifest) else {
        return ManifestOutcome::Unreadable(manifest_appid_from_name(manifest));
    };
    let Some(state) = Vdf::parse(&text).get("AppState").cloned() else {
        return if text.trim().is_empty() {
            ManifestOutcome::Unreadable(manifest_appid_from_name(manifest))
        } else {
            ManifestOutcome::NotInstalled
        };
    };
    let Some(appid) = state
        .get("appid")
        .and_then(Vdf::number)
        .and_then(|id| u32::try_from(id).ok())
        .filter(|id| *id > 0)
    else {
        return ManifestOutcome::NotInstalled;
    };
    let folder = state.get("installdir").and_then(Vdf::text).unwrap_or("");
    match safe_install_dir(steamapps, folder) {
        Some(install_dir) => ManifestOutcome::App(appid, state, install_dir),
        None => ManifestOutcome::NotInstalled,
    }
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
