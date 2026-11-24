//! The host's side of game settings files per device (issues #72 and #69; the rules are in
//! `butterpollo_session::game_settings`): when a swap begins and ends, its retries and the console.
//!
//! A stream's launch swaps the files in just before the app's process starts; a launch the
//! files cannot be swapped for fails with the reason, so the game never starts with the wrong
//! device's settings. The swap ends when the app stops ([`Swap`] goes with the
//! `RunningApp`, after its processes are closed): End session, Close game, the game exiting,
//! another app replacing it. A Disconnect keeps the app, so it keeps the swap. A launch from the
//! console has no device, so it swaps nothing.
//!
//! Only one app runs at a time. A second device joining the running game plays with the
//! launching device's settings; a device starting another app ends the first app, and with it
//! its swap, before its own begins. A game moved with Continue here keeps its files as they
//! are, which become the previous device's copies once the new device's stream runs; what the
//! game holds when the app stops is saved for the device that played it last ([`hand_over`]).
//!
//! While the game runs, its programs are recorded in the journal (the app's processes, the
//! Steam game, a store game the host noticed; not launchers or shells). When the app stops but
//! one of them still runs (a detached launcher's game, a game the host lost track of, a host
//! that crashed), nothing is saved or put back until it closes. A game the host never saw at
//! all is not waited for.
//!
//! What a crash or a failure left is finished by a thread of its own: at host start, then every
//! 30 seconds while something is left, as the Windows user once someone is signed in. While
//! anything is left, the console shows a warning with the reason.
//!
//! The devices' copies stay in the host's config folder when a device is unpaired or the app
//! is removed; pairing the device again, or adding the app back with the same uuid, finds them.
use crate::state::{Host, Shared};
use butterpollo_session::{
    game_files,
    game_settings::{self, Report, Store, Watched},
    state::App,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{Condvar, Mutex, MutexGuard, PoisonError, Weak},
    time::Duration,
};

/// The console warning while a swap is left unfinished.
const WARNING: &str = "game_settings";
/// The console warning for a launch the files could not be swapped for.
const LAUNCH_WARNING: &str = "game_settings_launch";
/// The console warning for a swapped game the host cannot wait for.
pub const UNWATCHED_WARNING: &str = "game_settings_unwatched";
/// How often what is left is tried again.
const RETRY: Duration = Duration::from_secs(30);

/// How the files are reached: as the signed-in Windows user; the tests reach theirs as the
/// test process.
const ACCESS: game_files::Access = if cfg!(test) {
    game_files::as_host
} else {
    as_user
};

#[cfg_attr(test, allow(dead_code))]
fn as_user(work: &mut dyn FnMut()) -> std::io::Result<()> {
    butterpollo_win_io::user_folder::as_user(work)
        .map_err(|error| std::io::Error::other(error.to_string()))
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The swaps of apps running in this run: the retries leave them alone.
static LIVE: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
/// Wakes the retry thread early, after a swap failed to finish.
static NUDGE: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());

fn nudge() {
    *locked(&NUDGE.0) = true;
    NUDGE.1.notify_all();
}

fn store(directory: &Path) -> Store {
    Store::new(directory, ACCESS).running(Box::new(still_running))
}

/// The tests' running processes (id, creation time).
#[cfg(test)]
static RUNNING: Mutex<BTreeSet<(u32, u64)>> = Mutex::new(BTreeSet::new());

/// Processes recorded for a swap, not journaled yet: [`Swap::watch`] runs on the stream's
/// thread with the app held, so the journal is written off it ([`flush`]).
static PENDING: Mutex<Vec<(PathBuf, String, Vec<Watched>)>> = Mutex::new(Vec::new());

/// Journal what [`Swap::watch`] recorded, for swap `only` or all.
fn flush(only: Option<&str>) {
    let pending: Vec<_> = {
        let mut pending = locked(&PENDING);
        let (taken, kept) = std::mem::take(&mut *pending)
            .into_iter()
            .partition(|(_, id, _)| only.is_none_or(|only| only == id));
        *pending = kept;
        taken
    };
    for (directory, id, processes) in pending {
        if let Err(error) = store(&directory).watch(&id, &processes) {
            tracing::warn!(
                error = format!("{error:#}"),
                "the game's processes could not be recorded for its settings files; the files may go back before it closes"
            );
        }
    }
}

/// The first of `processes` that still runs: the same id with the same creation time, so a
/// later process of the same program is not taken for the game. When that cannot be told,
/// it is said instead, so nothing goes back under a game that might still run.
fn still_running(processes: &[Watched]) -> Option<String> {
    #[cfg(test)]
    {
        let running = locked(&RUNNING);
        processes
            .iter()
            .find(|process| running.contains(&(process.pid, process.started)))
            .map(|process| process.program.clone())
    }
    #[cfg(not(test))]
    {
        use butterpollo_win_io::process;
        // Read once, only when a process cannot be opened (anti-cheat denies the query).
        let mut listed: Option<Option<BTreeSet<u32>>> = None;
        processes
            .iter()
            .find(|watched| {
                let started = process::creation_time(watched.pid);
                alive(watched, started, || {
                    listed
                        .get_or_insert_with(|| {
                            process::processes()
                                .ok()
                                .map(|list| list.iter().map(|p| p.pid).collect())
                        })
                        .as_ref()
                        .is_none_or(|pids| pids.contains(&watched.pid))
                })
            })
            .map(|watched| format!("{} (process {})", watched.program, watched.pid))
    }
}

/// Whether `watched` still runs, its id's creation time now being `started` (0: the process
/// cannot be opened, or is gone). One that cannot be opened but is still `listed` (by id in
/// the process list, or the list cannot be read) counts as running: a game guarded by
/// anti-cheat denies the query.
fn alive(watched: &Watched, started: u64, listed: impl FnOnce() -> bool) -> bool {
    watched.started != 0 && (started == watched.started || (started == 0 && listed()))
}

/// A running app's swap: finished when it drops.
pub struct Swap {
    id: String,
    app: String,
    directory: PathBuf,
    host: Weak<Host>,
    /// The files swapped, expanded.
    files: Vec<PathBuf>,
    /// The game's processes recorded so far.
    watched: BTreeSet<(u32, u64)>,
    /// The launch failed before the game ran: only the originals go back.
    cancelled: bool,
}

impl Swap {
    /// Record the game's processes: the files stay while one runs, also after a crash of the
    /// host. Journaled off the caller's thread, at once.
    pub fn watch(&mut self, processes: impl IntoIterator<Item = Watched>) {
        let new: Vec<Watched> = processes
            .into_iter()
            .filter(|process| process.started != 0)
            .filter(|process| self.watched.insert((process.pid, process.started)))
            .collect();
        if new.is_empty() {
            return;
        }
        locked(&PENDING).push((self.directory.clone(), self.id.clone(), new));
        nudge();
    }
    /// The files swapped, expanded.
    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }
    /// The launch failed after the files went in, before the game ran.
    pub fn cancel(mut self) {
        self.cancelled = true;
    }
    /// What [`hand_over`] needs, taken under the app's lock and used after it.
    pub fn handle(&self) -> Handle {
        Handle {
            id: self.id.clone(),
            app: self.app.clone(),
            directory: self.directory.clone(),
        }
    }
}

/// A running app's swap, for [`hand_over`].
pub struct Handle {
    id: String,
    app: String,
    directory: PathBuf,
}

/// The running game moved to `device` (`name` for messages) with Continue here and its
/// stream runs: the files as they are now are saved for the device that played before, and
/// what the game holds when the app stops is saved for this one
/// ([`game_settings::Store::hand_over`]). When that fails the swap stays the previous
/// device's, which then keeps everything rather than lose its part.
pub fn hand_over(handle: &Handle, device: &str, name: &str) {
    match store(&handle.directory).hand_over(&handle.id, device, name) {
        Ok(report) => {
            for warning in &report.warnings {
                tracing::warn!(app = %handle.app, "{warning}");
            }
            if !report.saved.is_empty() {
                tracing::info!(app = %handle.app, device = %name, saved = ?report.saved, "the game moved to another device: its settings files are saved for the device that played before, and for this one when the game ends");
            }
        }
        Err(error) => {
            tracing::warn!(app = %handle.app, device = %name, error = format!("{error:#}"), "the game moved to another device, but its settings files stay with the device that played before");
        }
    }
}

impl Drop for Swap {
    fn drop(&mut self) {
        flush(Some(&self.id));
        let store = store(&self.directory);
        let result = if self.cancelled {
            store.cancel(&self.id)
        } else {
            store.finish(&self.id)
        };
        locked(&LIVE).remove(&self.id);
        match result {
            Ok(report) => {
                log_finished(&self.app, &report);
                if !self.cancelled
                    && let Some(h) = self.host.upgrade()
                {
                    match &report.unwatched {
                        Some(why) => h.warnings.set(
                            UNWATCHED_WARNING,
                            format!("{}: game settings files: {why}.", self.app),
                        ),
                        None => h.warnings.clear(UNWATCHED_WARNING),
                    }
                }
            }
            Err(error) => {
                let waiting = game_settings::still_running(&error);
                let error = format!("{error:#}");
                if waiting {
                    tracing::warn!(app = %self.app, %error, "the game still runs after the app stopped; its settings files stay until it closes");
                } else {
                    tracing::error!(app = %self.app, %error, "the game settings files could not all be saved and put back; they stay as they are and the host tries again");
                }
                if let Some(h) = self.host.upgrade() {
                    raise(&h, &error);
                }
                nudge();
            }
        }
    }
}

fn log_finished(app: &str, report: &Report) {
    for warning in &report.warnings {
        tracing::error!(%app, "{warning}");
    }
    tracing::info!(%app, saved = ?report.saved, restored = ?report.restored, "game settings files saved for the device and the PC's own put back");
}

fn raise(h: &Host, error: &str) {
    h.warnings.set(
        WARNING,
        format!("Game settings files are not back yet: {error}. The host tries again every 30 seconds; until then the files may hold a device's settings."),
    );
}

/// The stream's app (expanded) is about to start for the device in `args` (`clientUuid`,
/// `clientName`): swap its game settings files in. `environment` is the Windows user's, for
/// `%VAR%`. None when the app lists no files or the launch has no device.
pub fn launching(
    h: &Shared,
    app: &App,
    args: &HashMap<String, String>,
    environment: &BTreeMap<String, String>,
) -> anyhow::Result<Option<Swap>> {
    let written = game_settings::paths(&app.extra)?;
    if written.is_empty() {
        return Ok(None);
    }
    let Some(device) = args.get("clientUuid").filter(|device| !device.is_empty()) else {
        tracing::info!(app = %app.name, "game settings files are swapped per device; this launch has no device, so the files stay as they are");
        return Ok(None);
    };
    let device_name = args
        .get("clientName")
        .filter(|name| !name.is_empty())
        .unwrap_or(device);
    let id = uuid::Uuid::new_v4().to_string();
    let result = (|| {
        let files = written
            .iter()
            .map(|path| {
                let expanded = game_settings::expand(path, |name| {
                    environment
                        .iter()
                        .find(|(key, _)| key.eq_ignore_ascii_case(name))
                        .map(|(_, value)| value.clone())
                })?;
                Ok((path.clone(), expanded))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        // Live before it is journaled: the retry thread never takes it for a crash's.
        locked(&LIVE).insert(id.clone());
        store(&h.directory)
            .begin(
                &id,
                &game_settings::app_key(app),
                &app.name,
                device,
                device_name,
                &files,
                &|| locked(&LIVE).clone(),
            )
            .map(|begun| (begun, files.into_iter().map(|(_, path)| path).collect()))
    })();
    match result {
        Ok((begun, files)) => {
            h.warnings.clear(LAUNCH_WARNING);
            tracing::info!(app = %app.name, device = %device_name, placed = ?begun.report.placed, first = ?begun.report.first, "game settings files swapped in for the device (first: kept as they are, saved for it when the game ends)");
            Ok(Some(Swap {
                id: begun.id,
                app: app.name.clone(),
                directory: h.directory.clone(),
                host: std::sync::Arc::downgrade(h),
                files,
                watched: BTreeSet::new(),
                cancelled: false,
            }))
        }
        Err(error) => {
            locked(&LIVE).remove(&id);
            let error = error.context(format!(
                "{} did not start: its game settings files could not be swapped for {device_name}",
                app.name
            ));
            tracing::error!(error = format!("{error:#}"), "game settings files");
            h.warnings.set(LAUNCH_WARNING, format!("{error:#}."));
            Err(error)
        }
    }
}

/// Whether the host runs as the service at the sign-in screen, with no user to act as.
fn nobody_signed_in() -> bool {
    use butterpollo_win_io::process;
    process::is_system() && !process::user_signed_in()
}

/// Finish what earlier runs or failed finishes left, and show what is still left. True while
/// something is.
fn retry(h: &Host) -> bool {
    let store = store(&h.directory);
    let results = match store.recover(&|| locked(&LIVE).clone()) {
        Ok(results) => results,
        Err(error) => {
            let error = format!("{error:#}");
            tracing::error!(%error, "the game settings journal could not be read; nothing is swapped until it can be");
            raise(h, &error);
            return true;
        }
    };
    let mut left = Vec::new();
    for (swap, result) in results {
        match result {
            Ok(report) => log_finished(&swap.app_name, &report),
            Err(error) => {
                let error = format!("{error:#}");
                tracing::error!(%error, "game settings files left by an earlier session could not be put back yet");
                left.push(error);
            }
        }
    }
    if left.is_empty() {
        h.warnings.clear(WARNING);
        false
    } else {
        raise(h, &left.join("; "));
        true
    }
}

/// Start the retry thread; its first pass finishes what a crashed run left. At host start,
/// before any stream.
pub fn start(h: &Shared) {
    let host = std::sync::Arc::downgrade(h);
    if let Err(error) = std::thread::Builder::new()
        .name("game-settings".into())
        .spawn(move || retrier(&host))
    {
        tracing::error!(%error, "no thread for game settings files; what a crash left is put back at the next launch of the game");
    }
}

fn retrier(host: &Weak<Host>) {
    loop {
        let Some(h) = host.upgrade() else {
            return;
        };
        flush(None);
        let journal = h
            .directory
            .join(game_settings::FOLDER)
            .join(game_settings::JOURNAL_FILE);
        if journal.exists() && !nobody_signed_in() {
            retry(&h);
        }
        drop(h);
        let nudged = locked(&NUDGE.0);
        let (mut nudged, _) = NUDGE
            .1
            .wait_timeout_while(nudged, RETRY, |nudged| !*nudged)
            .unwrap_or_else(PoisonError::into_inner);
        *nudged = false;
    }
}

/// The console's Put back now for swap `id`, which waits for a game that may not be its own:
/// the files are saved for the device and the PC's put back without waiting. Refused while
/// the swap's app runs in this host.
pub fn release(h: &Host, id: &str) -> anyhow::Result<Report> {
    if locked(&LIVE).contains(id) {
        anyhow::bail!("the game is still streaming; end its session first");
    }
    flush(Some(id));
    let report = store(&h.directory).release(id)?;
    log_finished("(put back from the console)", &report);
    retry(h);
    Ok(report)
}

/// The paths of `app`'s game settings files for the signed-in Windows user, as far as they can
/// be told (nobody signed in, a %NAME% not set: left out). For the checks at save.
pub fn expanded(app: &App) -> Vec<PathBuf> {
    let Ok(written) = game_settings::paths(&app.extra) else {
        return Vec::new();
    };
    if written.is_empty() {
        return Vec::new();
    }
    #[cfg(test)]
    let environment: BTreeMap<String, String> = std::env::vars().collect();
    #[cfg(not(test))]
    let environment = butterpollo_win_io::process::user_environment().unwrap_or_default();
    written
        .iter()
        .filter_map(|path| {
            game_settings::expand(path, |name| {
                environment
                    .iter()
                    .find(|(key, _)| key.eq_ignore_ascii_case(name))
                    .map(|(_, value)| value.clone())
            })
            .ok()
        })
        .collect()
}

/// The console's view of an app's game settings files (`GET /api/game-settings?uuid=`): each
/// paired device with the files it has a saved copy of, and the swaps still in place.
pub fn status(h: &Host, uuid: &str) -> anyhow::Result<Value> {
    let app = h
        .apps
        .read()
        .unwrap()
        .iter()
        .find(|app| app.extra.get("uuid").and_then(Value::as_str) == Some(uuid))
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("no app {uuid}"))?;
    let written = game_settings::paths(&app.extra)?;
    let key = game_settings::app_key(&app);
    let store = store(&h.directory);
    let devices: Vec<Value> = h
        .paired
        .read()
        .unwrap()
        .clients
        .iter()
        .map(|client| {
            let saved: Vec<&String> = written
                .iter()
                .filter(|path| store.saved(&key, &client.uuid, path))
                .collect();
            json!({
                "uuid": client.uuid,
                "name": client.name,
                "saved": saved,
            })
        })
        .collect();
    let live = locked(&LIVE).clone();
    let pending: Vec<Value> = store
        .journal()?
        .swaps
        .into_iter()
        .filter(|swap| swap.app == key)
        .map(|swap| {
            json!({
                "id": swap.id,
                "device": swap.device,
                "deviceName": swap.device_name,
                "running": live.contains(&swap.id),
                "error": swap.error,
            })
        })
        .collect();
    Ok(json!({
        "files": written,
        "devices": devices,
        "pending": pending,
        "folder": h.directory.join(game_settings::FOLDER),
    }))
}

#[cfg(test)]
mod tests;
