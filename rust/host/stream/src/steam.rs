//! The Steam library sync, as in Vibepollo: apps for the installed or
//! recently played Steam games, with PNG covers, kept in step by a check
//! every 30 seconds while `steam_auto_sync` is on.
use crate::state::Shared;
use anyhow::{Result, bail};
use butterpollo_session::{
    state::App,
    steam::{self, Game, Settings},
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex, PoisonError},
    time::{Duration, Instant, SystemTime},
};

/// A file's parse, keyed by path, with the size and time it was read at.
type StampCache<T> = Mutex<HashMap<PathBuf, ((u64, SystemTime), T)>>;
/// A folder's listing, keyed by path, with the time it was read at.
type TimeCache<T> = Mutex<HashMap<PathBuf, (SystemTime, T)>>;

pub struct Outcome {
    pub changed: bool,
    pub games: usize,
    pub importable: usize,
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
/// The installed games, or why there are none to read.
pub fn catalog(settings: &Settings) -> Result<(Vec<PathBuf>, Vec<Game>)> {
    if !settings.enabled {
        bail!("Steam integration is disabled");
    }
    let roots = butterpollo_win_io::steam::roots();
    if roots.is_empty() {
        bail!("Steam installation was not found");
    }
    let games = steam::discover(&roots);
    Ok((roots, games))
}
/// Sync the managed Steam apps now.
pub fn sync(h: &Shared) -> Result<Outcome> {
    let settings = Settings::from_config(&h.config.read().unwrap());
    let (_, games) = butterpollo_win_io::library::in_background(|| catalog(&settings))?;
    sync_beside(h, &settings, &games, crate::playnite::syncs_launchers(h))
}
/// While Playnite syncs the library its export brings the Steam games too, so the Steam
/// sync stands aside as the other launchers' does and no game is added twice.
fn sync_beside(h: &Shared, settings: &Settings, games: &[Game], playnite: bool) -> Result<Outcome> {
    if playnite {
        return Ok(Outcome {
            changed: false,
            games: games.len(),
            importable: games
                .iter()
                .filter(|g| steam::importable(g, settings.include_tools))
                .count(),
        });
    }
    sync_catalog(h, settings, games)
}
fn sync_catalog(h: &Shared, settings: &Settings, games: &[Game]) -> Result<Outcome> {
    let selected = steam::select(games, settings, now());
    let covers: HashMap<u32, PathBuf> = selected
        .iter()
        .filter_map(|game| cover(h, game).map(|path| (game.appid, path)))
        .collect();
    let changed = update_library(h, |apps, document| {
        let dismissed = crate::library::dismissed(document);
        steam::reconcile(apps, &selected, settings, &covers, &dismissed)
    })?;
    if changed {
        tracing::info!(selected = selected.len(), "Steam library synced");
    }
    Ok(Outcome {
        changed,
        games: games.len(),
        importable: games
            .iter()
            .filter(|g| steam::importable(g, settings.include_tools))
            .count(),
    })
}
/// Change the app library as JSON and save it when `change` reports a change.
pub fn update_apps(h: &Shared, change: impl FnOnce(&mut Vec<Value>) -> bool) -> Result<bool> {
    update_library(h, |apps, _| change(apps))
}
/// [`update_apps`] that may also change the app document's other fields, such as the launcher
/// sync's list of deleted games. Its `apps` field is replaced by the changed list.
pub fn update_library(
    h: &Shared,
    change: impl FnOnce(&mut Vec<Value>, &mut Value) -> bool,
) -> Result<bool> {
    // A sync changes the library as it is on disk, not one another program has since replaced.
    h.reload_apps();
    let mut apps = h.apps.write().unwrap();
    let mut document = h.app_document.read().unwrap().clone();
    let mut values = match serde_json::to_value(&*apps)? {
        Value::Array(values) => values,
        _ => vec![],
    };
    if !change(&mut values, &mut document) {
        return Ok(false);
    }
    let mut next: Vec<App> = serde_json::from_value(Value::Array(values))?;
    h.assign_apps(&mut next)?;
    document["apps"] = serde_json::to_value(&next)?;
    h.save_apps(&document)?;
    let dropped = crate::neural::dropped(&apps, &next);
    *apps = next;
    *h.app_document.write().unwrap() = document;
    h.config_version
        .fetch_add(1, std::sync::atomic::Ordering::Release);
    drop(apps);
    // An uninstalled game the sync dropped loses the mod as a deleted one
    // does, after the apps are unlocked: the removal runs a helper process.
    crate::neural::remove_dropped(h, &dropped);
    Ok(true)
}
/// The game's cover as a PNG in the profile's covers folder. A missing or
/// small local portrait is replaced by Steam's 600x900 store image.
fn cover(h: &Shared, game: &Game) -> Option<PathBuf> {
    let folder = h.directory.join("covers");
    let png = folder.join(format!("steam_{}.png", game.appid));
    let mut source = game.artwork().map(Path::to_owned);
    let small = source.as_deref().is_none_or(|path| {
        butterpollo_win_graphics::image::dimensions(path).is_ok_and(|(w, h)| w < 600 || h < 900)
    });
    if small && let Some(download) = store_portrait(&folder, game.appid, source.as_deref()) {
        source = Some(download);
    }
    let Some(source) = source else {
        return png.is_file().then_some(png);
    };
    let meta = folder.join(format!("steam_{}.png.meta", game.appid));
    let stamp = std::fs::metadata(&source).ok().map(|m| {
        let modified = m
            .modified()
            .ok()
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs());
        format!(
            "version=1\npath={}\nsize={}\nmtime={modified}\n",
            source.display(),
            m.len()
        )
    })?;
    if png.is_file() && std::fs::read_to_string(&meta).is_ok_and(|saved| saved == stamp) {
        return Some(png);
    }
    match butterpollo_win_graphics::image::to_png(&source, &png) {
        Ok(_) => {
            let _ = butterpollo_session::state::atomic_write(&meta, stamp.as_bytes());
            Some(png)
        }
        Err(error) => {
            tracing::debug!(error = %format!("{error:#}"), appid = game.appid, "Steam cover conversion failed");
            png.is_file().then_some(png)
        }
    }
}
/// How long a store portrait whose download failed `failures` times in a row is left alone.
/// Steam has none for many tools and older games, so asking every ten minutes finds nothing
/// for as long as the game is installed: ten minutes, an hour, then a day.
pub(crate) fn portrait_retry_after(failures: u32) -> Duration {
    Duration::from_secs(match failures {
        0 | 1 => 600,
        2 => 3600,
        _ => 86_400,
    })
}
/// The failures a `.failed` marker counts. One written before the count was kept is empty: one.
pub(crate) fn portrait_failures(marker: &[u8]) -> u32 {
    std::str::from_utf8(marker)
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(1)
}
/// One client for every artwork download, so a sync of many games sets TLS up once.
static ARTWORK_CLIENT: std::sync::LazyLock<Result<reqwest::Client, String>> =
    std::sync::LazyLock::new(|| {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(10))
            .user_agent("Rubylight-Steam-Artwork/1.0")
            .build()
            .map_err(|error| error.to_string())
    });
/// One artwork download of at most 16 MiB, from a blocking thread of the host's runtime (the
/// syncs run on one), with the shared client: no redirects, three seconds to connect and ten
/// in all.
pub(crate) fn download_artwork(url: &str) -> Result<Vec<u8>> {
    let client = ARTWORK_CLIENT
        .as_ref()
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    let runtime = tokio::runtime::Handle::try_current()
        .map_err(|_| anyhow::anyhow!("artwork downloads need the host's runtime"))?;
    runtime.block_on(async {
        let response = client.get(url).send().await?.error_for_status()?;
        if response.content_length().is_some_and(|n| n > 16 << 20) {
            bail!("The artwork is too large");
        }
        let bytes = response.bytes().await?;
        if bytes.len() > 16 << 20 {
            bail!("The artwork is too large");
        }
        Ok(bytes.to_vec())
    })
}
/// Steam's portrait store image, downloaded once into the covers folder. When
/// the fixed CDN path fails, the hashed folder of the local cover is tried. A
/// failed download is retried after [`portrait_retry_after`].
fn store_portrait(folder: &Path, appid: u32, local: Option<&Path>) -> Option<PathBuf> {
    let file = folder.join(format!("steam_{appid}_600x900_2x.jpg"));
    let valid = |path: &Path| {
        butterpollo_win_graphics::image::dimensions(path).is_ok_and(|(w, h)| w >= 600 && h >= 900)
    };
    if file.is_file() && valid(&file) {
        return Some(file);
    }
    let failed = folder.join(format!("steam_{appid}_600x900_2x.jpg.failed"));
    let failures = std::fs::read(&failed).map_or(0, |marker| portrait_failures(&marker));
    if failures > 0
        && std::fs::metadata(&failed)
            .and_then(|m| m.modified())
            .is_ok_and(|at| {
                at.elapsed()
                    .is_ok_and(|age| age < portrait_retry_after(failures))
            })
    {
        return None;
    }
    let download = download_artwork;
    let hashed = local.and_then(|path| steam::hashed_store_portrait_url(appid, path));
    let bytes = download(&steam::store_portrait_url(appid)).or_else(|error| match &hashed {
        Some(url) => download(url),
        None => Err(error),
    });
    let result = bytes.and_then(|bytes| {
        std::fs::create_dir_all(folder)?;
        butterpollo_session::state::atomic_write(&file, &bytes)?;
        if !valid(&file) {
            let _ = std::fs::remove_file(&file);
            bail!("Steam artwork is smaller than 600x900");
        }
        Ok(())
    });
    match result {
        Ok(()) => {
            let _ = std::fs::remove_file(&failed);
            Some(file)
        }
        Err(error) => {
            tracing::debug!(error = %format!("{error:#}"), appid, "Steam store artwork unavailable");
            let _ = std::fs::create_dir_all(folder);
            // Only an answer counts towards the longer waits: a PC that was offline asks
            // again in ten minutes, as before.
            let answered = error
                .downcast_ref::<reqwest::Error>()
                .is_none_or(|error| error.status().is_some());
            let failures = if answered {
                failures.saturating_add(1)
            } else {
                failures.max(1)
            };
            let _ = std::fs::write(&failed, failures.to_string());
            None
        }
    }
}
/// What the files a sync reads look like now: sizes and times of the
/// library lists, manifests, play history, app cache and artwork folders. Each read a step
/// would repeat is skipped when its input did not change since the last one: the check asks
/// every 30 seconds, and Steam changes these files only when something was installed, removed
/// or signed in.
fn fingerprint(settings: &Settings, roots: &[PathBuf]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    format!("{settings:?}").hash(&mut hasher);
    let mut stamp = |path: &Path| {
        path.hash(&mut hasher);
        if let Ok(metadata) = std::fs::metadata(path) {
            metadata.len().hash(&mut hasher);
            metadata.modified().ok().hash(&mut hasher);
        }
    };
    let mut libraries = vec![];
    for root in roots {
        let Some(apps) = steam::steamapps(root) else {
            continue;
        };
        stamp(&apps.join("libraryfolders.vdf"));
        stamp(&root.join("appcache").join("appinfo.vdf"));
        stamp(&root.join("appcache").join("librarycache"));
        for user in users_of(&root.join("userdata")) {
            stamp(&user.join("config").join("localconfig.vdf"));
        }
        libraries.push(apps.clone());
        libraries.extend(vdf_libraries(&apps));
    }
    for library in libraries {
        for manifest in manifests_of(&library) {
            stamp(&manifest);
        }
    }
    hasher.finish()
}
/// The library paths a `libraryfolders.vdf` listed, as text, with the file's size and time it
/// was read. The file is parsed again only when that moved, which happens when a library folder
/// comes or goes. Only the paths are cached: whether a folder is there is checked on every call,
/// as a drive can appear after the file was read. A read that fails is not cached.
static VDF_LIBRARIES: LazyLock<StampCache<Vec<String>>> = LazyLock::new(Default::default);
fn vdf_libraries(apps: &Path) -> Vec<PathBuf> {
    let file = apps.join("libraryfolders.vdf");
    let stamp = std::fs::metadata(&file)
        .ok()
        .and_then(|metadata| Some((metadata.len(), metadata.modified().ok()?)));
    let Some(stamp) = stamp else {
        // No file to cache by: read it, as before.
        return std::fs::read_to_string(&file)
            .ok()
            .map(|text| existing_libraries(&vdf_listed(&text)))
            .unwrap_or_default();
    };
    let mut read = VDF_LIBRARIES.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((last, listed)) = read.get(apps)
        && *last == stamp
    {
        return existing_libraries(listed);
    }
    let Some(text) = std::fs::read_to_string(&file).ok() else {
        // Unreadable now, so not cached: the next check reads it again.
        return vec![];
    };
    let listed = vdf_listed(&text);
    let libraries = existing_libraries(&listed);
    read.insert(apps.to_owned(), (stamp, listed));
    libraries
}
/// The library paths the text of `libraryfolders.vdf` lists. Old files give the path as the
/// value itself.
fn vdf_listed(text: &str) -> Vec<String> {
    let folders = steam::Vdf::parse(text);
    let folders = folders.get("libraryfolders").unwrap_or(&folders);
    folders
        .entries()
        .iter()
        .filter_map(|(_, folder)| {
            folder
                .get("path")
                .or(Some(folder))
                .and_then(steam::Vdf::text)
        })
        .map(str::to_owned)
        .collect()
}
/// The `steamapps` folders of the listed library paths that exist now.
fn existing_libraries(paths: &[String]) -> Vec<PathBuf> {
    paths
        .iter()
        .filter_map(|path| steam::steamapps(Path::new(path)))
        .collect()
}
/// The user folders a Steam root's `userdata` holds, with the folder's time they were read.
/// The folder's time moves when a user folder comes or goes, so they are read again only then.
/// A folder whose time cannot be read is not cached.
static USERS: LazyLock<TimeCache<Vec<PathBuf>>> = LazyLock::new(Default::default);
fn users_of(userdata: &Path) -> Vec<PathBuf> {
    let Some(modified) = std::fs::metadata(userdata)
        .ok()
        .and_then(|m| m.modified().ok())
    else {
        // No folder to cache by: read it, as before.
        return user_paths(userdata).unwrap_or_default();
    };
    let mut read = USERS.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((last, users)) = read.get(userdata)
        && *last == modified
    {
        return users.clone();
    }
    let Some(users) = user_paths(userdata) else {
        // Unreadable now, so not cached: the next check reads it again.
        return vec![];
    };
    read.insert(userdata.to_owned(), (modified, users.clone()));
    users
}
/// The folder's user folders, `None` when they cannot be read.
fn user_paths(userdata: &Path) -> Option<Vec<PathBuf>> {
    let users = std::fs::read_dir(userdata).ok()?;
    Some(users.flatten().map(|entry| entry.path()).collect())
}
/// The manifest names of a library folder, with the folder's time they were read. The names
/// are read again only when that moved, which happens when a manifest appears, is renamed or
/// goes; their sizes and times, which the fingerprint stamps itself, are unaffected. A folder
/// whose time cannot be read is not cached.
static MANIFESTS: LazyLock<TimeCache<Vec<PathBuf>>> = LazyLock::new(Default::default);
fn manifests_of(library: &Path) -> Vec<PathBuf> {
    let Some(modified) = std::fs::metadata(library)
        .ok()
        .and_then(|m| m.modified().ok())
    else {
        // No folder to cache by: read it, as before.
        return manifest_names(library).unwrap_or_default();
    };
    let mut read = MANIFESTS.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((last, names)) = read.get(library)
        && *last == modified
    {
        return names.clone();
    }
    let Some(names) = manifest_names(library) else {
        // Unreadable now, so not cached: the next check reads it again.
        return vec![];
    };
    read.insert(library.to_owned(), (modified, names.clone()));
    names
}
/// The folder's `.acf` files, in name order, `None` when they cannot be read.
fn manifest_names(library: &Path) -> Option<Vec<PathBuf>> {
    let entries = std::fs::read_dir(library).ok()?;
    let mut manifests: Vec<_> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("acf")))
        .collect();
    manifests.sort();
    Some(manifests)
}
struct Watch {
    fingerprint: u64,
    synced: Option<Instant>,
}
static WATCH: Mutex<Watch> = Mutex::new(Watch {
    fingerprint: 0,
    synced: None,
});
static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Steam's install folders for one housekeeping tick, looked up when the first check asks.
pub type Roots = std::cell::LazyCell<Vec<PathBuf>>;
/// The auto-sync check: sync when the Steam files or settings changed, and
/// at least every five minutes to retry artwork.
pub fn watch(h: &Shared, roots: &Roots) {
    let settings = Settings::from_config(&h.config.read().unwrap());
    if !settings.enabled || !settings.auto_sync {
        return;
    }
    if RUNNING.swap(true, std::sync::atomic::Ordering::AcqRel) {
        return;
    }
    let current = butterpollo_win_io::library::in_background(|| fingerprint(&settings, roots));
    let due = {
        let watch = WATCH.lock().unwrap();
        watch.fingerprint != current
            || watch
                .synced
                .is_none_or(|at| at.elapsed() >= Duration::from_secs(300))
    };
    if due {
        match sync(h) {
            Ok(_) => {
                let mut watch = WATCH.lock().unwrap();
                watch.fingerprint = current;
                watch.synced = Some(Instant::now());
            }
            Err(error) => {
                tracing::debug!(error = %format!("{error:#}"), "Steam auto-sync skipped");
                WATCH.lock().unwrap().synced = Some(Instant::now());
            }
        }
    }
    RUNNING.store(false, std::sync::atomic::Ordering::Release);
}
/// `/api/steam/status`.
pub fn status(h: &Shared) -> Value {
    let settings = Settings::from_config(&h.config.read().unwrap());
    let roots = butterpollo_win_io::steam::roots();
    let games = if roots.is_empty() {
        vec![]
    } else {
        steam::discover(&roots)
    };
    let installed: Vec<&Game> = games.iter().filter(|g| g.installed).collect();
    let importable = installed
        .iter()
        .filter(|g| steam::importable(g, settings.include_tools))
        .count();
    let tools = installed
        .iter()
        .filter(|g| !steam::importable(g, false))
        .count();
    let excluded = installed
        .iter()
        .filter(|g| steam::is_excluded(g, &settings.exclusions))
        .count();
    json!({
        "status": true,
        "provider": "steam",
        "enabled": settings.enabled,
        "forced": false,
        "available": !roots.is_empty(),
        "roots": roots,
        "game_count": installed.len(),
        "importable_game_count": importable,
        "tool_game_count": tools,
        "excluded_game_count": excluded,
        "selected_game_count": steam::select(&games, &settings, now()).len(),
        "exclude_games": settings.exclusions.iter().map(|(id, name)| json!({"id": id, "name": name})).collect::<Vec<_>>(),
        "auto_sync": settings.auto_sync,
        "sync_all_installed": settings.sync_all_installed,
        "recent_games": settings.recent_games,
        "recent_max_age_days": settings.recent_max_age_days,
        "autosync_remove_uninstalled": settings.remove_uninstalled,
        "include_tools": settings.include_tools,
        "playnite_available": false,
    })
}
/// `/api/steam/games`, optionally one app with its cover prepared.
pub fn games(h: &Shared, appid: Option<u32>) -> Result<Value> {
    let settings = Settings::from_config(&h.config.read().unwrap());
    let (_, games) = catalog(&settings)?;
    let selected: Vec<u32> = steam::select(&games, &settings, now())
        .iter()
        .map(|g| g.appid)
        .collect();
    let list: Vec<Value> = games
        .iter()
        .filter(|g| appid.is_none_or(|id| id == g.appid))
        .map(|game| {
            let cover = appid.and_then(|_| cover(h, game));
            let path = |p: &Option<PathBuf>| p.as_ref().map(|p| p.to_string_lossy().into_owned());
            let importable = steam::importable(game, settings.include_tools);
            let excluded = steam::is_excluded(game, &settings.exclusions);
            json!({
                "appid": game.appid,
                "steam_id": game.appid.to_string(),
                "stable_id": format!("steam:{}", game.appid),
                "name": game.name,
                "install_dir": game.install_dir,
                "library_path": game.library_path,
                "icon_path": path(&game.icon_path),
                "header_path": path(&game.header_path),
                "portrait_path": path(&game.portrait_path),
                "artwork_path": game.artwork(),
                "artwork_client_path": cover,
                "app_type": game.app_type,
                "installed": game.installed,
                "last_played": game.last_played,
                "playtime_minutes": game.playtime_minutes,
                "importable": importable,
                "excluded": excluded,
                "selected": selected.contains(&game.appid),
                "launch_uri": format!("steam://rungameid/{}", game.appid),
            })
        })
        .collect();
    Ok(json!({"status": true, "enabled": settings.enabled, "games": list}))
}
/// `/api/steam/launch`: start an installed game on this PC through Steam.
pub fn launch(h: &Shared, appid: u32) -> Result<Value> {
    let _transition = h.launch_transition.lock().unwrap();
    if crate::updater::installing(h) {
        bail!("Rubylight is installing an update");
    }
    let settings = Settings::from_config(&h.config.read().unwrap());
    let (_, games) = catalog(&settings)?;
    let Some(game) = games.iter().find(|g| g.appid == appid && g.installed) else {
        bail!("Steam game {appid} is not installed");
    };
    if !steam::importable(game, settings.include_tools)
        || steam::is_excluded(game, &settings.exclusions)
    {
        bail!("Steam app {appid} is excluded");
    }
    let environment = butterpollo_win_io::process::user_environment()?;
    butterpollo_win_io::process::Process::shell_detached(
        &steam::launch_command(appid),
        None,
        false,
        &environment,
    )?;
    Ok(json!({"status": true, "launch_uri": format!("steam://rungameid/{appid}")}))
}

#[cfg(test)]
mod tests;
