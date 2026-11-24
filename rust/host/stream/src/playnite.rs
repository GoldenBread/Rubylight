//! Playnite, as in Vibepollo: the plugin's games become apps, and an app
//! linked to a Playnite game is started through Playnite with the stream's
//! environment, ending when Playnite reports the game stopped.
use crate::state::Shared;
use anyhow::{Context, Result, bail, ensure};
#[cfg(test)]
use butterpollo_session::game_settings::Watched;
use butterpollo_session::playnite::{self, Artwork, Game, Message, Settings};
use butterpollo_session::version;
use butterpollo_win_io::playnite::Pipe;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

mod launch;
mod launch_state;
mod plugin;

#[cfg(test)]
use launch::launch_command;
pub use launch::{Launch, close_fullscreen};
#[cfg(test)]
use launch_state::{
    EXIT_GRACE, ExitGrace, GameWatch, LaunchState, Phase, START_TIMEOUT, WATCH_LATER, WATCH_SOON,
    WATCH_START,
};
use plugin::{PLUGIN_FILES, installed_plugin, version};
pub use plugin::{
    helper_exit_code, install_plugin, install_worker, packaged_plugin, uninstall_plugin,
    uninstall_worker,
};
#[cfg(test)]
use plugin::{needs_update, plugin_present};

fn now() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// What the plugin last reported.
#[derive(Default)]
struct Catalog {
    games: Vec<Game>,
    categories: Vec<(String, String)>,
    plugins: Vec<(String, String)>,
    at: Option<Instant>,
}
static CATALOG: Mutex<Catalog> = Mutex::new(Catalog {
    games: vec![],
    categories: vec![],
    plugins: vec![],
    at: None,
});
/// One library connection to the plugin at a time: a new one replaces the
/// plugin's previous connection.
static CONNECTION: Mutex<()> = Mutex::new(());
/// Ask the plugin for its library.
fn snapshot() -> Result<()> {
    let _connection = CONNECTION.lock().unwrap();
    let pipe = Pipe::connect(&json!({"type":"hello","role":"sunshine","pid":std::process::id()}))
        .context("connecting to the Playnite plugin for library sync")?;
    let mut games = vec![];
    let (mut categories, mut plugins) = (vec![], vec![]);
    let (mut started, mut asked, mut complete) = (false, false, false);
    let opened = Instant::now();
    let mut last = Instant::now();
    while !complete {
        match pipe.lines.recv_timeout(Duration::from_millis(250)) {
            Ok(line) => {
                last = Instant::now();
                match playnite::parse_with(&line, &butterpollo_win_io::playnite::local_to_utc) {
                    Message::SnapshotStart => {
                        started = true;
                        games.clear();
                    }
                    Message::SnapshotComplete => complete = true,
                    Message::Games(batch) => games.extend(batch),
                    Message::Categories(list) => categories = list,
                    Message::Plugins(list) => plugins = list,
                    _ => {}
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                bail!("the Playnite plugin closed the connection")
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }
        // The plugin sends its library on connecting; ask if it has not.
        if !started && !asked && opened.elapsed() > Duration::from_secs(3) {
            pipe.send(&json!({"type":"command","command":"snapshot"}))?;
            asked = true;
        }
        // Older plugins send no end marker.
        if !started && !games.is_empty() && last.elapsed() > Duration::from_secs(3) {
            complete = true;
        }
        if opened.elapsed() > Duration::from_secs(60) {
            bail!("the Playnite library did not arrive in time");
        }
    }
    tracing::debug!(
        games = games.len(),
        elapsed_ms = opened.elapsed().as_millis(),
        "Playnite library snapshot received"
    );
    *CATALOG.lock().unwrap() = Catalog {
        games,
        categories,
        plugins,
        at: Some(Instant::now()),
    };
    Ok(())
}
/// Make an uploaded cover (`covers/KEY.png`) the game's cover in Playnite,
/// then sync so its app shows it, as Vibepollo's `/api/playnite/cover`.
pub fn set_cover(h: &Shared, id: &str, key: &str) -> Result<PathBuf> {
    let id = uuid::Uuid::parse_str(id.trim().trim_matches(['{', '}']))
        .context("invalid Playnite game ID")?
        .to_string();
    ensure!(
        !key.is_empty()
            && key.len() <= 128
            && key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
        "invalid cover key"
    );
    let cover = h.directory.join("covers").join(format!("{key}.png"));
    ensure!(cover.is_file(), "the uploaded cover was not found");
    ensure!(
        Settings::from_config(&h.config.read().unwrap()).enabled,
        "Playnite integration is disabled"
    );
    {
        let _connection = CONNECTION.lock().unwrap();
        let pipe =
            Pipe::connect(&json!({"type":"hello","role":"sunshine","pid":std::process::id()}))
                .context("connecting to the Playnite plugin")?;
        let request = format!("cover-{}", uuid::Uuid::new_v4());
        pipe.send(
            &json!({"type":"command","command":"set-cover","requestId":request,
                          "id":id,"path":cover.to_string_lossy().replace('\\', "/")}),
        )?;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let line = match pipe
                .lines
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(line) => line,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    bail!("Playnite did not confirm the cover in time")
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    bail!("the Playnite plugin closed the connection")
                }
            };
            if let Message::CommandResult {
                request_id,
                success,
                error,
                ..
            } = playnite::parse(&line)
                && request_id == request
            {
                ensure!(
                    success,
                    "Playnite did not take the cover: {}",
                    if error.is_empty() {
                        "no reason given"
                    } else {
                        &error
                    }
                );
                break;
            }
        }
    }
    tracing::info!(id, cover = %cover.display(), "set the game's cover in Playnite");
    sync(h)?;
    Ok(cover)
}
/// Start Playnite, closing it first if it runs, as Vibepollo's
/// `/api/playnite/launch`.
pub fn restart() -> Result<()> {
    let program = butterpollo_win_io::playnite::install_dir()
        .and_then(|dir| butterpollo_win_io::playnite::executable(&dir))
        .context("no Playnite executable found; open Playnite once or repair its installation")?;
    let running: BTreeMap<u32, u64> = butterpollo_win_io::playnite::session_processes()?
        .iter()
        .filter(|p| {
            p.name.eq_ignore_ascii_case("Playnite.DesktopApp.exe")
                || p.name.eq_ignore_ascii_case("Playnite.FullscreenApp.exe")
        })
        .map(|p| (p.pid, p.started))
        .collect();
    butterpollo_win_io::process::stop_processes(&running, Duration::from_secs(10));
    butterpollo_win_io::playnite::launch(&program, &[], &BTreeMap::new())?;
    tracing::info!(executable = %program.display(), restarted = !running.is_empty(), "Playnite started from the console");
    Ok(())
}
/// A PNG of `source` in the covers folder, converted again only when the
/// source changes.
fn png(h: &Shared, source: &str, name: &str) -> Option<PathBuf> {
    let source = Path::new(source);
    let metadata = std::fs::metadata(source).ok()?;
    let folder = h.directory.join("covers");
    let png = folder.join(format!("{name}.png"));
    let stamp = format!(
        "{}|{}|{:?}",
        source.display(),
        metadata.len(),
        metadata.modified().ok()
    );
    let record = folder.join(format!("{name}.png.src"));
    if png.is_file() && std::fs::read_to_string(&record).is_ok_and(|saved| saved == stamp) {
        return Some(png);
    }
    match butterpollo_win_graphics::image::to_png(source, &png) {
        Ok(_) => {
            let _ = butterpollo_session::state::atomic_write(&record, stamp.as_bytes());
            Some(png)
        }
        Err(error) => {
            tracing::debug!(error = %format!("{error:#}"), source = %source.display(), "Playnite artwork conversion failed");
            png.is_file().then_some(png)
        }
    }
}
pub struct Outcome {
    pub changed: bool,
    pub games: usize,
}
/// Fetch Playnite's library and bring the apps in line with it.
pub fn sync(h: &Shared) -> Result<Outcome> {
    let settings = Settings::from_config(&h.config.read().unwrap());
    if !settings.enabled {
        bail!("Playnite integration is disabled");
    }
    if butterpollo_win_io::playnite::running().is_none() {
        bail!("Playnite is not running");
    }
    snapshot()?;
    let games = CATALOG.lock().unwrap().games.clone();
    let artwork: Artwork = games
        .iter()
        .map(|game| {
            let id = game.id.to_ascii_lowercase();
            let cover = (!game.box_art_path.is_empty())
                .then(|| png(h, &game.box_art_path, &format!("playnite_{id}")))
                .flatten();
            let icon = (!game.icon_path.is_empty())
                .then(|| png(h, &game.icon_path, &format!("playnite_icon_{id}")))
                .flatten();
            (id, (cover, icon))
        })
        .collect();
    let changed = crate::steam::update_library(h, |apps, document| {
        let dismissed = crate::library::dismissed(document);
        let synced = playnite::reconcile(
            apps,
            &games,
            &settings,
            now(),
            &artwork,
            settings.auto_sync,
            &dismissed,
        );
        playnite::fullscreen_entry(apps, settings.fullscreen_entry) | synced
    })?;
    if changed {
        tracing::info!(games = games.len(), "Playnite library synced");
    }
    Ok(Outcome {
        changed,
        games: games.len(),
    })
}
struct Watch {
    playnite: Option<u32>,
    settings: String,
    synced: Option<Instant>,
}
static WATCH: Mutex<Watch> = Mutex::new(Watch {
    playnite: None,
    settings: String::new(),
    synced: None,
});
static RUNNING: AtomicBool = AtomicBool::new(false);
fn sync_if_due(
    watch: &mut Watch,
    pid: u32,
    settings: String,
    at: Instant,
    sync: impl FnOnce() -> Result<Outcome>,
) -> Result<()> {
    if watch.playnite == Some(pid)
        && watch.settings == settings
        && watch
            .synced
            .is_some_and(|last| at.duration_since(last) < Duration::from_secs(600))
    {
        return Ok(());
    }
    sync()?;
    *watch = Watch {
        playnite: Some(pid),
        settings,
        synced: Some(at),
    };
    Ok(())
}
/// The auto-sync check, every 30 seconds: keep the fullscreen entry as set,
/// and sync when Playnite starts, the settings change, or every ten minutes.
pub fn watch(h: &Shared) {
    let settings = Settings::from_config(&h.config.read().unwrap());
    if RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }
    let wanted = settings.enabled && settings.fullscreen_entry;
    // Compare before cloning: the entry step is skipped when it would change nothing, so the
    // tick does not take the app library's write lock and serialize it for the common case.
    // `reload_apps` first, as the step itself would, so the check reads the library as it is on
    // disk.
    h.reload_apps();
    if playnite::fullscreen_entry_due(&h.apps.read().unwrap(), wanted)
        && let Err(error) =
            crate::steam::update_apps(h, |apps| playnite::fullscreen_entry(apps, wanted))
    {
        tracing::warn!(%error, "the Playnite fullscreen app could not be updated");
    }
    if settings.enabled
        && settings.auto_sync
        && let Some((pid, _)) = butterpollo_win_io::playnite::running()
        && let Err(error) = sync_if_due(
            &mut WATCH.lock().unwrap(),
            pid,
            format!("{settings:?}"),
            Instant::now(),
            || sync(h),
        )
    {
        tracing::warn!(error = %format!("{error:#}"), "Playnite library sync failed; retrying on the next check");
    }
    RUNNING.store(false, Ordering::Release);
}

/// `/api/playnite/status`.
/// Whether Playnite keeps the library in step: on, syncing automatically and with its
/// plugin installed. The launcher sync then leaves the other launchers' games to it.
pub fn syncs_launchers(h: &Shared) -> bool {
    let settings = Settings::from_config(&h.config.read().unwrap());
    settings.enabled
        && settings.auto_sync
        && installed_plugin().is_some_and(|t| PLUGIN_FILES.iter().all(|f| t.join(f).is_file()))
}
pub fn status(h: &Shared) -> Value {
    let settings = Settings::from_config(&h.config.read().unwrap());
    let target = installed_plugin();
    let installed_version = target.as_deref().and_then(version);
    let packaged_version = version(&packaged_plugin(h));
    let installed = target
        .as_ref()
        .is_some_and(|t| PLUGIN_FILES.iter().all(|f| t.join(f).is_file()));
    let catalog = CATALOG.lock().unwrap();
    json!({
        "status": true,
        "enabled": settings.enabled,
        "active": butterpollo_win_io::playnite::running().is_some(),
        "available": butterpollo_win_io::playnite::install_dir().is_some(),
        "installed": installed,
        "extensions_dir": target.as_ref().and_then(|t| t.parent()),
        "installed_version": installed_version,
        "packaged_version": packaged_version,
        "update_available": match (&installed_version, &packaged_version) {
            (Some(installed), Some(packaged)) => version::newer(packaged, installed),
            _ => false,
        },
        "auto_sync": settings.auto_sync,
        "game_count": catalog.games.len(),
        "synced_seconds_ago": catalog.at.map(|at| at.elapsed().as_secs()),
    })
}
fn catalog_ready() -> Result<()> {
    let stale = CATALOG
        .lock()
        .unwrap()
        .at
        .is_none_or(|at| at.elapsed() > Duration::from_secs(60));
    if stale && butterpollo_win_io::playnite::running().is_some() {
        snapshot()?;
    }
    Ok(())
}
/// `/api/playnite/games`.
pub fn games() -> Result<Value> {
    catalog_ready()?;
    let catalog = CATALOG.lock().unwrap();
    Ok(Value::Array(
        catalog
            .games
            .iter()
            .map(|g| {
                json!({"id": g.id, "name": g.name, "categories": g.categories, "installed": g.installed,
                       "pluginId": g.plugin_id, "pluginName": g.plugin_name,
                       "lastPlayed": g.last_played.map(playnite::format_time)})
            })
            .collect(),
    ))
}
/// When Playnite says each game it last reported was played, `0` for never, for the
/// native library's `last_played` (`native::Scan::playing`). Nothing before the plugin's
/// first snapshot, nor for a time that didn't parse.
#[cfg_attr(feature = "test-support", allow(dead_code))]
pub fn last_played() -> Vec<(String, u64)> {
    let catalog = CATALOG.lock().unwrap();
    catalog
        .games
        .iter()
        .filter(|game| !game.last_played_unknown)
        .map(|game| {
            let at = u64::try_from(game.last_played.unwrap_or(0)).unwrap_or(0);
            (butterpollo_session::native::playnite_key(&game.id), at)
        })
        .collect()
}
/// How long Playnite says each game it last reported was played, in minutes, for the native
/// library's `playtime_minutes` (`native::Scan::played_for`). Nothing before the plugin's first
/// snapshot, nor for a game whose time the plugin didn't send.
#[cfg_attr(feature = "test-support", allow(dead_code))]
pub fn playtime() -> Vec<(String, u64)> {
    let catalog = CATALOG.lock().unwrap();
    catalog
        .games
        .iter()
        .filter_map(|game| {
            let minutes = game.playtime_minutes?;
            Some((butterpollo_session::native::playnite_key(&game.id), minutes))
        })
        .collect()
}
/// The categories of the games the plugin last reported, by [`butterpollo_session::native::playnite_key`].
#[cfg(not(any(test, feature = "test-support")))]
pub fn collections() -> Vec<(String, Vec<String>)> {
    let catalog = CATALOG.lock().unwrap();
    catalog
        .games
        .iter()
        .filter(|game| !game.categories.is_empty())
        .map(|game| {
            (
                butterpollo_session::native::playnite_key(&game.id),
                game.categories.clone(),
            )
        })
        .collect()
}
/// `/api/playnite/categories` (and the plugins seen, for the console).
pub fn categories() -> Result<Value> {
    catalog_ready()?;
    let catalog = CATALOG.lock().unwrap();
    let (names, plugins) = playnite::names(&catalog.games);
    let mut categories: BTreeMap<String, String> = catalog
        .categories
        .iter()
        .map(|(id, name)| (name.clone(), id.clone()))
        .collect();
    for name in names {
        categories.entry(name).or_default();
    }
    let plugins: BTreeSet<(String, String)> =
        catalog.plugins.iter().cloned().chain(plugins).collect();
    Ok(json!({
        "status": true,
        "categories": categories.into_iter().map(|(name, id)| json!({"id": id, "name": name})).collect::<Vec<_>>(),
        "plugins": plugins.into_iter().map(|(id, name)| json!({"id": id, "name": name})).collect::<Vec<_>>(),
    }))
}

#[cfg(test)]
mod tests;
