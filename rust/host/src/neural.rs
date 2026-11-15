//! Neural Rendering: a per-app switch that puts frame generation into the
//! game's folder and takes it out again.
//!
//! It is ANR FG by Apex Compute (https://github.com/apex-compute/ANR, no
//! licence published), a DLSS frame generation provider that OptiScaler
//! (GPL-3.0) loads in place of the game's own. Rubylight never hosts,
//! bundles, mirrors or modifies its files: when the user turns the switch
//! on, this PC downloads the release Rubylight pins
//! ([`butterpollo_core::neural::PINNED`]): its full package and add-on,
//! straight from that GitHub release ([`butterpollo_core::neural::asset`]
//! says which), checks them against the pin, GitHub's digests and the
//! release's SHA256SUMS.txt, keeps them in a cache and copies the files
//! unmodified into the game's folder ([`folder`]). A launch checks them and
//! finishes an update cut short; no other release is ever taken on its own.
//!
//! Rubylight 3.0 offered AMDNR (neural upscaling) here instead. Its
//! installs stay until the user removes them ([`LEGACY_KEY`]); turning frame
//! generation on for such a game takes AMDNR out first.
pub mod folder;
#[cfg(test)]
mod tests;

use crate::state::Shared;
use anyhow::{Context, Result, anyhow, bail, ensure};
use butterpollo_core::{
    neural::{self, Download, Gpu, Placed, Release},
    state::App,
};
use folder::{PACKAGE, Package, Request, Response, Writer};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{BufRead, Read, Write},
    path::{Path, PathBuf},
    sync::{Condvar, Mutex, PoisonError, RwLock, mpsc},
    time::{Duration, Instant},
};

/// The app's setting: `{"exe", "release", "proxy", "mode", "switches"}`
/// once the files are in its folder. Only the host writes it.
pub const KEY: &str = "neural-anr";
/// Rubylight 3.0's setting for the AMDNR it installed (`{"exe", "release",
/// "proxy"}`): kept until the files are removed, never set again.
pub const LEGACY_KEY: &str = "neural-rendering";
/// The hidden flag that makes the host exe report what the compute runtime sees.
#[cfg_attr(test, allow(dead_code, reason = "tests answer for the probe"))]
const PROBE_FLAG: &str = "--neural-probe";
const PROBE_ANSWER: &str = "NEURAL-PROBE ";
/// The user's acceptance of the notice and the pending removals.
const SETTINGS_FILE: &str = "neural.json";
/// The record's acceptance of this release's notice. Rubylight 3.0's
/// `accepted_at` was AMDNR's and does not count.
const ACCEPTED: &str = "anr_accepted_at";
/// A failed probe is asked again after this; a driver update needs no restart.
const PROBE_RETRY: Duration = Duration::from_secs(300);
/// The record's list of removals still to do.
const PENDING: &str = "pending_removals";
/// How long a game's own check ([`game_check`]) holds.
const GAME_RECHECK: Duration = Duration::from_secs(120);
/// How long a status read waits for a game's own check; one that takes
/// longer (a network share that stalls) answers the next read.
const GAME_WAIT: Duration = Duration::from_secs(if cfg!(test) { 30 } else { 3 });
/// How often a pending removal is tried again.
const PENDING_RETRY: Duration = Duration::from_secs(600);
/// What the console shows for a failed download; the host's log has why.
const DOWNLOAD_FAILED: &str = "the download or its check failed; Rubylight's log says why";

#[derive(Default)]
pub struct Neural {
    state: Mutex<State>,
    /// Told when a game's own check ends ([`game_check`]).
    checked: Condvar,
    /// One download into the cache at a time.
    fetching: tokio::sync::Mutex<()>,
    /// One writer of the record at a time.
    record: Mutex<()>,
    /// One pass over the pending removals at a time.
    removing: Mutex<()>,
    /// Copies out of the cache hold it to read; replacing or clearing a
    /// release in the cache holds it to write.
    cache: RwLock<()>,
    #[cfg(test)]
    pub probe: Mutex<Option<Probe>>,
    /// A test's streaming GPU in place of this PC's.
    #[cfg(test)]
    pub streams: Mutex<Option<neural::Adapter>>,
    /// A test's GPUs in place of this PC's; the streaming GPU alone if none.
    #[cfg(test)]
    pub adapters: Mutex<Option<Vec<neural::Adapter>>>,
    /// A test's missing Visual C++ runtime files in place of this PC's.
    #[cfg(test)]
    pub vc_missing: Mutex<Vec<&'static str>>,
    /// A test's server in place of GitHub.
    #[cfg(test)]
    pub origin: Mutex<Option<String>>,
    /// A test's release in place of [`neural::PINNED`].
    #[cfg(test)]
    pub pin: Mutex<Option<neural::Pin<'static>>>,
    /// A test's Steam folders in place of this PC's.
    #[cfg(test)]
    pub steam_roots: Mutex<Option<Vec<PathBuf>>>,
    /// The GPU a test streams with can't be found.
    #[cfg(test)]
    pub streams_unknown: Mutex<bool>,
    /// A game's own check waits for this test to send or hang up.
    #[cfg(test)]
    pub game_gate: Mutex<Option<mpsc::Receiver<()>>>,
    /// How many games' own checks ran in this test.
    #[cfg(test)]
    pub game_checks: std::sync::atomic::AtomicUsize,
    /// A test's wait for a game's own check in place of [`GAME_WAIT`].
    #[cfg(test)]
    pub game_wait: Mutex<Option<Duration>>,
    /// The GPU check waits for this test to send or hang up, once.
    #[cfg(test)]
    pub probe_gate: Mutex<Option<mpsc::Receiver<()>>>,
}

#[derive(Default)]
struct State {
    /// The last GPU check's answer ([`gpu_capability`]), when it came, the
    /// settings it holds for, and whether it held the GPU that streams.
    probe: Option<(Instant, GpuSetting, Result<neural::Capable, String>, bool)>,
    /// A GPU check asks again in the background while a read is answered
    /// with the last one.
    probing: bool,
    /// Whether one of the PC's GPUs is AMD's or NVIDIA's ([`offered`]),
    /// and the settings it holds for.
    offered: Option<(GpuSetting, bool)>,
    jobs: BTreeMap<String, Job>,
    /// Each app's game folder as [`game_check`] last looked: when, for
    /// which record, and the Visual C++ runtime files beside the game.
    lookups: BTreeMap<String, Lookup>,
    /// Each app's own check ([`game_check`]): when, for which record, and
    /// what it found (None: the check itself failed).
    games: BTreeMap<String, GameChecked>,
    /// The apps whose own check runs now.
    checking: BTreeSet<String>,
    /// Each app's count of job changes: a check that began before one is
    /// not kept.
    generation: BTreeMap<String, u64>,
    /// When the pending removals are tried next.
    removal_at: Option<Instant>,
    /// The cached release this host start checked file by file.
    verified: Option<PathBuf>,
    /// Cached releases an install is about to copy, and how many: another
    /// release fetched meanwhile leaves them to the next fetch to clear.
    using: BTreeMap<PathBuf, usize>,
}

type GameChecked = (Instant, GameKey, Option<GameCheck>);

/// What of an app's record a game's own check depends on, all read without
/// touching the disk: its command's program, its install, and the programs
/// the files sit beside now and sat beside in Rubylight 3.0.
type GameKey = [Option<PathBuf>; 4];
fn game_key(app: &App) -> GameKey {
    [
        command_exe(app),
        install(app),
        legacy_exe(app),
        placed_exe(app),
    ]
}

/// A look at an app's game folder ([`game_check`]).
struct Lookup {
    at: Instant,
    key: GameKey,
    /// The Visual C++ runtime files beside the game's program.
    beside: Vec<&'static str>,
}

/// What a status read holds of an app's game folder ([`game_check`]).
#[derive(Default)]
struct Looked {
    /// The runtime files beside the game's program, once looked for.
    beside: Option<Vec<&'static str>>,
    check: Option<GameCheck>,
}

/// What a game's own check found before any switch: what the install asks
/// of the game itself ([`find_game`]), without the download.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GameCheck {
    Fits,
    /// Refused: `why` is the code a device says in its words
    /// (`anti_cheat`, `no_frame_generation`, `game`).
    Refused {
        why: &'static str,
        reason: String,
    },
    /// The host can't tell which program is the game's.
    Choose(String),
}

/// A cached release kept while an install copies it ([`fetch_using`]).
/// The last one of a release another download replaced meanwhile takes it
/// out of the cache: no fetch hands it out again, and launches use the newest.
struct Using {
    h: Shared,
    folder: PathBuf,
}
impl Drop for Using {
    fn drop(&mut self) {
        let last = {
            let mut state = self.h.neural.lock();
            let Some(count) = state.using.get_mut(&self.folder) else {
                return;
            };
            *count -= 1;
            *count == 0 && state.using.remove(&self.folder).is_some()
        };
        if last && cached(&cache_root(&self.h)).is_some_and(|(newest, _)| newest != self.folder) {
            match std::fs::remove_dir_all(&self.folder) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                // The next download's clearing tries again.
                Err(error) => {
                    tracing::warn!(folder = %self.folder.display(), %error, "Neural Rendering: an older release could not be taken out of the cache")
                }
            }
        }
    }
}

/// What a switch is doing for one app, or how it ended.
#[derive(Clone, Debug, Serialize)]
pub struct Job {
    pub action: &'static str,
    /// `download`, `place`, `remove`, `done` or `failed`.
    pub phase: &'static str,
    pub done: u64,
    pub total: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The game's .exe could not be found: the user picks it.
    pub choose_exe: bool,
    /// Placed files that changed since and stayed in the folder.
    pub kept: Vec<String>,
}

/// What `--neural-probe` prints.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Probe {
    pub gpus: Vec<Gpu>,
    pub error: Option<String>,
}

impl Neural {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
    fn reading(&self) -> std::sync::RwLockReadGuard<'_, ()> {
        self.cache.read().unwrap_or_else(PoisonError::into_inner)
    }
    fn writing(&self) -> std::sync::RwLockWriteGuard<'_, ()> {
        self.cache.write().unwrap_or_else(PoisonError::into_inner)
    }
}

/// `butterpollo.exe --neural-probe`: the GPUs the AMD compute runtime
/// finds, as one line.
pub fn probe_main() -> i32 {
    let probe = match butterpollo_windows::hip::gpus() {
        Ok(gpus) => Probe { gpus, error: None },
        Err(error) => Probe {
            gpus: Vec::new(),
            error: Some(format!("{error:#}")),
        },
    };
    let mut out = std::io::stdout().lock();
    let line = serde_json::to_string(&probe).unwrap_or_default();
    match writeln!(out, "{PROBE_ANSWER}{line}").and_then(|()| out.flush()) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

/// The GPU frame generation runs on, or why this PC cannot have it: the
/// GPU a stream uses, else another of the PC's ([`neural::capable`]), an
/// AMD one asked about in a helper process; then the Visual C++ runtime,
/// beside the game's program (`game`, its folder) or in System32, looked for
/// at every call, so an install shows at once. The GPU is asked once, again
/// a while after a failure, and again once the GPU or display a stream uses
/// is set otherwise. Blocks.
pub fn capability(h: &Shared, game: Option<&Path>) -> Result<neural::Capable, String> {
    capability_of(h, game).map_err(|(_, reason)| reason)
}

/// [`capability`], with which half failed: `gpu` or `runtime`.
fn capability_of(
    h: &Shared,
    game: Option<&Path>,
) -> Result<neural::Capable, (&'static str, String)> {
    let capable = gpu_capability(h, false).map_err(|reason| ("gpu", reason))?;
    neural::runtime(&vc_missing(h, game)).map_err(|reason| ("runtime", reason))?;
    Ok(capable)
}

/// [`capability`]'s GPU half, held. An answer made without the GPU that
/// streams is asked again after a while, like a failure, so its note comes
/// once that GPU is found. With `serve`, a read's, the answer held for these
/// settings is given while that asks again in the background; the first
/// answer, and one after the settings changed, is waited for.
fn gpu_capability(h: &Shared, serve: bool) -> Result<neural::Capable, String> {
    let setting = gpu_setting(h);
    {
        let mut state = h.neural.lock();
        if let Some((at, probed, result, whole)) = &state.probe
            && *probed == setting
        {
            let result = result.clone();
            if (result.is_ok() && *whole) || at.elapsed() < PROBE_RETRY {
                return result;
            }
            if serve {
                if !std::mem::replace(&mut state.probing, true) {
                    let h = h.clone();
                    std::thread::spawn(move || {
                        /// Ends the background check however it ends.
                        struct Ending<'a>(&'a Shared);
                        impl Drop for Ending<'_> {
                            fn drop(&mut self) {
                                self.0.neural.lock().probing = false;
                            }
                        }
                        let _ending = Ending(&h);
                        let _ = probe_gpu(&h, setting);
                    });
                }
                return result;
            }
        }
    }
    probe_gpu(h, setting)
}

/// [`gpu_capability`]'s check itself, held for `setting` unless the
/// settings changed meanwhile. Blocks.
fn probe_gpu(h: &Shared, setting: GpuSetting) -> Result<neural::Capable, String> {
    let mut whole = true;
    let result = gpus_with(h, &setting).and_then(|(streams, adapters)| {
        whole = streams.is_ok();
        // Only an AMD GPU needs the compute runtime, which no other driver has.
        let needs_probe = match &streams {
            Ok(streams) => neural::needs_probe(streams, &adapters),
            Err(_) => adapters.iter().any(|adapter| adapter.vendor == neural::AMD),
        };
        let probe = if needs_probe {
            run_probe(h).and_then(|probe| match probe.error {
                Some(error) if probe.gpus.is_empty() => Err(error),
                _ => Ok(probe.gpus),
            })
        } else {
            Err("not asked: no AMD GPU".into())
        };
        let gpus = probe.as_deref().map_err(String::as_str);
        match &streams {
            Ok(streams) => neural::capable(streams, &adapters, gpus),
            // The PC's other GPUs still count, with no note on the one that streams.
            Err(unknown) => neural::capable_any(&adapters, gpus, unknown),
        }
    });
    match &result {
        Err(reason) => tracing::info!(%reason, "Neural Rendering is not available on this PC"),
        Ok(capable) => {
            if let Some(streams) = &capable.streams {
                tracing::info!(gpu = %capable.gpu, %streams, "Neural Rendering runs on a GPU Rubylight does not stream with");
            }
        }
    }
    if gpu_setting(h) == setting {
        h.neural.lock().probe = Some((Instant::now(), setting, result.clone(), whole));
    }
    result
}

/// Whether devices are offered frame generation at all: one of the PC's
/// GPUs is AMD's or NVIDIA's, by its DXGI vendor ID alone, with no probe
/// ([`neural::offered`]). GPUs that can't be listed keep it offered, and
/// the switch says why. Asked again once the GPU or display a stream uses
/// is set otherwise. Blocks.
pub fn offered(h: &Shared) -> bool {
    let setting = gpu_setting(h);
    if let Some((held, offered)) = &h.neural.lock().offered
        && *held == setting
    {
        return *offered;
    }
    match gpus_with(h, &setting) {
        Ok((_, adapters)) => {
            let offered = neural::offered(&adapters);
            h.neural.lock().offered = Some((setting, offered));
            offered
        }
        // Offered while the GPUs are not known, and asked again next time.
        Err(_) => true,
    }
}

/// The settings that pick the GPU a stream uses: the display, and the
/// Graphics card setting's name and PnP identity.
type GpuSetting = [String; 3];
fn gpu_setting(h: &Shared) -> GpuSetting {
    let config = h.config.read().unwrap();
    ["output_name", "adapter_name", "adapter_pnp_id"].map(|key| config.get(key, "").to_string())
}

/// The Visual C++ runtime files OptiScaler needs that this PC lacks: in
/// System32, and beside the game's program (`game`, its folder), where the
/// DLL OptiScaler loads as finds them first. Blocks on the game's folder.
fn vc_missing(h: &Shared, game: Option<&Path>) -> Vec<&'static str> {
    let system = system_missing(h);
    if system.is_empty() {
        return system;
    }
    let beside = game.map(runtime_beside).unwrap_or_default();
    without(system, &beside)
}

/// `missing` less the files in `found`.
fn without(missing: Vec<&'static str>, found: &[&'static str]) -> Vec<&'static str> {
    missing
        .into_iter()
        .filter(|file| !found.contains(file))
        .collect()
}

/// The Visual C++ runtime files OptiScaler needs that System32 lacks.
#[cfg(not(test))]
fn system_missing(_: &Shared) -> Vec<&'static str> {
    let system = std::env::var_os("SystemRoot")
        .map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from)
        .join("System32");
    without(neural::VC_RUNTIME.to_vec(), &runtime_in(&system))
}
/// Tests miss the runtime files they name.
#[cfg(test)]
fn system_missing(h: &Shared) -> Vec<&'static str> {
    h.neural.vc_missing.lock().unwrap().clone()
}

/// The Visual C++ runtime files OptiScaler needs that are in `folder`.
fn runtime_in(folder: &Path) -> Vec<&'static str> {
    neural::VC_RUNTIME
        .into_iter()
        .filter(|file| folder.join(file).is_file())
        .collect()
}

/// [`runtime_in`] the game's folder, which may be on a share that stalls. Blocks.
fn runtime_beside(folder: &Path) -> Vec<&'static str> {
    stalls(folder);
    runtime_in(folder)
}

/// Where a look into a game's folder may wait (a network share that
/// stalls): tests hold a folder of theirs here ([`tests::Stall`]).
#[cfg(not(test))]
fn stalls(_: &Path) {}
#[cfg(test)]
fn stalls(path: &Path) {
    tests::stall_at(path);
}

/// What a status read knows of `app`'s game folder (saved as `uuid`): the
/// Visual C++ runtime beside its program, and with `ask` the game's own
/// check, as turning the switch on asks it before the download: its
/// program, anti-cheat, DLSS frame generation and Valve Anti-Cheat, through
/// the user's helper as the install does. The check is None when it failed:
/// the switch stays usable and the install says why. `system` is what
/// System32 lacks of the runtime; with nothing lacking, the game's folder
/// is not looked into for it.
///
/// Nothing here touches the game's folder, which may be on a share that
/// stalls: one thread per app at a time finds the game's program and looks
/// beside it, then runs the check where it decides something (`ask`, and
/// the runtime is there). A reader waits for it up to `wait`, then answers
/// with what it last found for the app's record, however old, and the
/// thread's answer serves the next read. Both are held for
/// [`GAME_RECHECK`]; a job drops the check's, and one that began before a
/// job is not kept. While `streaming`, no check starts and the last answer
/// is given.
fn game_check(
    h: &Shared,
    uuid: &str,
    app: &App,
    system: &[&'static str],
    wait: Duration,
    ask: bool,
    streaming: bool,
) -> Looked {
    let key = game_key(app);
    let fresh = |at: &Instant| at.elapsed() < GAME_RECHECK;
    fn held_lookup<'a>(state: &'a State, uuid: &str, key: &GameKey) -> Option<&'a Lookup> {
        state.lookups.get(uuid).filter(|lookup| lookup.key == *key)
    }
    fn held_check<'a>(state: &'a State, uuid: &str, key: &GameKey) -> Option<&'a GameChecked> {
        state.games.get(uuid).filter(|(_, held, _)| held == key)
    }
    // Whether what is held answers the read: the check only matters while
    // the runtime is there.
    let current = |state: &State| match held_lookup(state, uuid, &key) {
        Some(lookup) if fresh(&lookup.at) => {
            !ask || system.iter().any(|file| !lookup.beside.contains(file))
                || held_check(state, uuid, &key).is_some_and(|(at, ..)| fresh(at))
        }
        _ => false,
    };
    let mut state = h.neural.lock();
    let quiet = streaming
        && (held_lookup(&state, uuid, &key).is_some() || held_check(&state, uuid, &key).is_some());
    if !current(&state) && !quiet {
        if state.checking.insert(uuid.to_owned()) {
            let generation = state.generation.get(uuid).copied().unwrap_or(0);
            let (h, uuid, app, key) = (h.clone(), uuid.to_owned(), app.clone(), key.clone());
            let system = system.to_vec();
            std::thread::spawn(move || {
                /// Ends the check however it ends, a panic too.
                struct Ending<'a>(&'a Shared, &'a str);
                impl Drop for Ending<'_> {
                    fn drop(&mut self) {
                        self.0.neural.lock().checking.remove(self.1);
                        self.0.neural.checked.notify_all();
                    }
                }
                let _ending = Ending(&h, &uuid);
                let command = game_command(&app);
                let beside = match game_folder_with(&app, command.clone()) {
                    Some(folder) if !system.is_empty() => runtime_beside(&folder),
                    _ => Vec::new(),
                };
                let runtime = system.iter().all(|file| beside.contains(file));
                let lookup = Lookup {
                    at: Instant::now(),
                    key: key.clone(),
                    beside,
                };
                h.neural.lock().lookups.insert(uuid.clone(), lookup);
                h.neural.checked.notify_all();
                if !ask || !runtime {
                    return;
                }
                let exe = legacy_exe(&app).or(command);
                let check = run_game_check(&h, &app, exe, install(&app));
                let mut state = h.neural.lock();
                if state.generation.get(&uuid).copied().unwrap_or(0) == generation {
                    state
                        .games
                        .insert(uuid.clone(), (Instant::now(), key, check));
                }
            });
        }
        let deadline = Instant::now() + wait;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if current(&state) || !state.checking.contains(uuid) || left.is_zero() {
                break;
            }
            state = h
                .neural
                .checked
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
    Looked {
        beside: held_lookup(&state, uuid, &key).map(|lookup| lookup.beside.clone()),
        check: held_check(&state, uuid, &key).and_then(|(.., check)| check.clone()),
    }
}

/// [`game_check`]'s check itself. Blocks.
fn run_game_check(
    h: &Shared,
    app: &App,
    exe: Option<PathBuf>,
    install: Option<PathBuf>,
) -> Option<GameCheck> {
    #[cfg(test)]
    {
        h.neural
            .game_checks
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let gate = h.neural.game_gate.lock().unwrap().take();
        if let Some(gate) = gate {
            let _ = gate.recv();
        }
    }
    match find_game(h, app, exe, install) {
        Ok(Response::Found { .. }) => Some(GameCheck::Fits),
        Ok(Response::Refused { reason }) => Some(GameCheck::Refused {
            why: game_why(&reason),
            reason,
        }),
        Ok(Response::ChooseExe { reason }) => Some(GameCheck::Choose(reason)),
        Ok(other) => {
            tracing::debug!(app = %app.name, answer = ?other, "Neural Rendering: the game's check gave no answer");
            None
        }
        Err(error) => {
            tracing::debug!(app = %app.name, error = %format!("{error:#}"), "Neural Rendering: the game's check failed");
            None
        }
    }
}

/// The code of a game's refusal ([`GameCheck::Refused`]).
fn game_why(reason: &str) -> &'static str {
    if reason.starts_with(folder::NO_DLSS) {
        "no_frame_generation"
    } else if reason.contains(folder::ANTI_CHEAT) {
        "anti_cheat"
    } else {
        "game"
    }
}

/// The folder of the game's program as far as the app tells without the
/// helper: where the files are, else where the earlier upscaling was, else
/// the app's command when it is a program in the install. Blocks on the
/// game's folder.
fn game_folder(app: &App) -> Option<PathBuf> {
    game_folder_with(app, None).or_else(|| game_folder_with(app, game_command(app)))
}

/// [`game_folder`], with the app's command as [`game_command`] found it.
fn game_folder_with(app: &App, command: Option<PathBuf>) -> Option<PathBuf> {
    placed_exe(app)
        .or_else(|| legacy_exe(app))
        .or(command)
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
}

/// The GPU this host captures and encodes with (the Graphics card
/// setting's, or the one that drives the display it streams), or why it
/// can't be found, and every hardware GPU of the PC, that one among them.
#[cfg(not(test))]
fn gpus_with(_: &Shared, setting: &GpuSetting) -> Result<StreamsAmong, String> {
    let [display, adapter_name, pnp_id] = setting;
    let stream_gpu = butterpollo_windows::capture::stream_gpu(display, adapter_name, pnp_id)
        .map_err(|e| format!("the GPU Rubylight streams with can't be found: {e:#}"));
    let gpus = butterpollo_windows::capture::gpus().map_err(|e| {
        let listed = format!("the GPUs can't be listed: {e:#}");
        match &stream_gpu {
            Err(unknown) => format!("{unknown}; {listed}"),
            Ok(_) => listed,
        }
    })?;
    let luid = |(low, high): (u32, i32)| (u64::from(high as u32) << 32) | u64::from(low);
    let streams = stream_gpu.and_then(|(name, (low, high))| {
        let gpu = gpus
            .iter()
            .find(|gpu| gpu.luid == (low, high))
            .ok_or_else(|| {
                format!("the GPU Rubylight streams with ({name}) can't be identified")
            })?;
        Ok(neural::Adapter {
            name,
            luid: luid((low, high)),
            vendor: gpu.vendor,
            device: gpu.device,
        })
    });
    let adapters = gpus
        .iter()
        .map(|gpu| neural::Adapter {
            name: gpu.name.clone(),
            luid: luid(gpu.luid),
            vendor: gpu.vendor,
            device: gpu.device,
        })
        .collect();
    Ok((streams, adapters))
}
/// The GPU this host streams with, or why it can't be found, and every
/// hardware GPU of the PC ([`gpus_with`]).
type StreamsAmong = (Result<neural::Adapter, String>, Vec<neural::Adapter>);
/// Tests stream with their own GPU, else the probe's first one, among their
/// own GPUs, else alone; or with one that can't be found, among their own.
#[cfg(test)]
fn gpus_with(h: &Shared, _: &GpuSetting) -> Result<StreamsAmong, String> {
    if *h.neural.streams_unknown.lock().unwrap() {
        let adapters = h
            .neural
            .adapters
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_default();
        return Ok((
            Err("the GPU Rubylight streams with can't be found: in this test".into()),
            adapters,
        ));
    }
    let streams = match h.neural.streams.lock().unwrap().clone() {
        Some(streams) => streams,
        None => {
            let probe = run_probe(h)?;
            let gpu = probe.gpus.first().ok_or("no GPU in this test")?;
            neural::Adapter {
                name: gpu.name.clone(),
                luid: gpu.luid.unwrap_or_default(),
                vendor: neural::AMD,
                device: 0,
            }
        }
    };
    let adapters = h
        .neural
        .adapters
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_else(|| vec![streams.clone()]);
    Ok((Ok(streams), adapters))
}

#[cfg(test)]
fn run_probe(h: &Shared) -> Result<Probe, String> {
    let gate = h.neural.probe_gate.lock().unwrap().take();
    if let Some(gate) = gate {
        let _ = gate.recv();
    }
    h.neural
        .probe
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "no probe in this test".into())
}
#[cfg(not(test))]
fn run_probe(_: &Shared) -> Result<Probe, String> {
    let mut answer = None;
    run_child(PROBE_FLAG, "", Duration::from_secs(60), |line| {
        if let Some(json) = line.strip_prefix(PROBE_ANSWER) {
            answer = serde_json::from_str::<Probe>(json).ok();
        }
    })
    .map_err(|e| format!("the GPU check failed: {e:#}"))?;
    answer.ok_or_else(|| "the GPU check gave no answer".into())
}

/// Run the host exe with `flag`, `input` as its first line, and hand each
/// line it prints to `line`. As the signed-in user when this host is SYSTEM.
#[cfg_attr(
    test,
    allow(dead_code, reason = "tests run the folder work in-process")
)]
fn run_child(flag: &str, input: &str, timeout: Duration, mut line: impl FnMut(&str)) -> Result<()> {
    use butterpollo_windows::process::{Process, Target};
    enum Child {
        User(Process),
        Own(std::process::Child),
    }
    let program = std::env::current_exe()?;
    let (mut child, stdin, stdout): (Child, Box<dyn Write>, Box<dyn Read + Send>) =
        if butterpollo_windows::process::is_system() {
            let mut piped = Process::spawn_piped(
                &program,
                &[flag.into()],
                Target::User { elevated: false },
            )
            .context(
                "cannot start the helper as the signed-in user; is anyone signed in to Windows?",
            )?;
            let stdin = piped.stdin.take().context("no helper input")?;
            let stdout = piped.stdout.take().context("no helper output")?;
            (
                Child::User(piped.process),
                Box::new(stdin),
                Box::new(stdout),
            )
        } else {
            use std::os::windows::process::CommandExt;
            let mut own = std::process::Command::new(&program)
                .arg(flag)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
                .spawn()
                .context("cannot start the helper")?;
            let stdin = own.stdin.take().context("no helper input")?;
            let stdout = own.stdout.take().context("no helper output")?;
            (Child::Own(own), Box::new(stdin), Box::new(stdout))
        };
    let mut stdin = stdin;
    // A helper that died early closed its input; its output says why.
    let _ = stdin.write_all(format!("{input}\n").as_bytes());
    drop(stdin);
    let (sender, lines) = mpsc::channel();
    std::thread::spawn(move || {
        for text in std::io::BufReader::new(stdout)
            .lines()
            .map_while(Result::ok)
        {
            if sender.send(text).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + timeout;
    loop {
        match lines.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(text) => line(&text),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                match &mut child {
                    Child::User(process) => drop(process.terminate()),
                    Child::Own(own) => drop(own.kill()),
                }
                bail!("the helper did not finish within {} s", timeout.as_secs());
            }
        }
    }
    match &mut child {
        Child::User(process) => drop(process.wait(Duration::from_secs(10))),
        Child::Own(own) => drop(own.wait()),
    }
    Ok(())
}

/// Run one folder request: in a helper with the signed-in user's token when
/// this host is SYSTEM (game folders are the user's), here otherwise. A
/// folder the user may not write goes to [`as_system`].
fn run_folder(request: Request, progress: &dyn Fn(u64, u64)) -> Result<Response> {
    let system = butterpollo_windows::process::is_system();
    let answer = if !system || cfg!(test) {
        folder::handle(request.clone(), Writer::User, progress)
    } else {
        let mut answer = None;
        run_child(
            folder::FLAG,
            &serde_json::to_string(&request)?,
            Duration::from_secs(30 * 60),
            |line| {
                if let Some(json) = line.strip_prefix(folder::ANSWER) {
                    answer = serde_json::from_str(json).ok();
                } else if let Some((done, total)) = line
                    .strip_prefix(folder::PROGRESS)
                    .and_then(|rest| rest.split_once(' '))
                {
                    progress(done.parse().unwrap_or(0), total.parse().unwrap_or(0));
                }
            },
        )?;
        answer.context("the Neural Rendering helper gave no answer")?
    };
    Ok(as_system(answer, system, || {
        folder::handle(request, Writer::System, progress)
    }))
}

/// The helper's answer, but for a folder the signed-in user may not write
/// (a game in Program Files) this SYSTEM host's own: it writes the folder
/// only when nobody but administrators may change it, and refuses
/// otherwise ([`Writer::System`]). A host that is not SYSTEM refuses.
fn as_system(answer: Response, system: bool, again: impl FnOnce() -> Response) -> Response {
    match answer {
        Response::NotWritable { folder } if system => {
            tracing::info!(folder = %folder.display(), "Neural Rendering: only administrators may write the game's folder; the service checks it and writes it itself");
            again()
        }
        Response::NotWritable { folder } => Response::Refused {
            reason: format!(
                "only administrators may change {}, and this Rubylight doesn't run as its service, which could",
                folder.display()
            ),
        },
        answer => answer,
    }
}

// ---- The user's acceptance and the record ----

/// The record, empty only when there is none yet: one that can't be read
/// is an error, never taken for empty and written over (it holds the
/// acceptance and the pending removals).
fn read_settings(h: &Shared) -> Result<Value> {
    let path = h.directory.join(SETTINGS_FILE);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(json!({})),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    let value: Value =
        serde_json::from_slice(&bytes).with_context(|| format!("{} is damaged", path.display()))?;
    ensure!(
        value.is_object(),
        "{} is damaged: no object",
        path.display()
    );
    Ok(value)
}
/// [`read_settings`] for a reader, empty and logged when it can't be read.
fn settings(h: &Shared) -> Value {
    read_settings(h).unwrap_or_else(|error| {
        tracing::warn!(error = %format!("{error:#}"), "Neural Rendering: its record can't be read");
        json!({})
    })
}
/// Change the record and write it; nothing is written when `change` fails.
fn update_settings(h: &Shared, change: impl FnOnce(&mut Value) -> Result<()>) -> Result<()> {
    let _one = h
        .neural
        .record
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let mut value = read_settings(h)?;
    change(&mut value)?;
    butterpollo_core::state::write_json(&h.directory.join(SETTINGS_FILE), &value)
}
/// Whether the user accepted the notice that comes before the first download.
pub fn accepted(h: &Shared) -> bool {
    settings(h)[ACCEPTED].as_u64().is_some()
}
pub fn accept(h: &Shared) -> Result<()> {
    update_settings(h, |value| {
        value[ACCEPTED] = json!(now());
        Ok(())
    })?;
    tracing::info!("Neural Rendering: the user accepted the third-party frame generation notice");
    Ok(())
}
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

// ---- The cache ----

/// `%ProgramData%\Butterpollo\neural-anr` for the installed host, the
/// profile's own `neural-anr` folder for any other.
pub fn cache_root(h: &Shared) -> PathBuf {
    legacy_cache_root(h).with_file_name("neural-anr")
}

/// Where Rubylight 3.0 kept its AMDNR download: `neural` beside the cache.
fn legacy_cache_root(h: &Shared) -> PathBuf {
    if h.directory == butterpollo_core::paths::installed_profile() {
        butterpollo_core::paths::data_root().join("neural")
    } else {
        h.directory.join("neural")
    }
}

/// Delete Rubylight 3.0's AMDNR download, which nothing installs from any
/// more, once the host starts: only from a cache this host trusts, never
/// through a link on the way. Game folders keep their copies. Blocks.
pub fn clear_legacy_cache(h: &Shared) {
    let root = legacy_cache_root(h);
    if !root.exists() {
        return;
    }
    let cleared = trusted_root(&root, butterpollo_windows::process::is_system())
        .and_then(|()| std::fs::remove_dir_all(&root).map_err(anyhow::Error::from));
    match cleared {
        Ok(()) => {
            tracing::info!(folder = %root.display(), "Neural Rendering: deleted the earlier upscaling download")
        }
        Err(error) => {
            tracing::warn!(folder = %root.display(), error = %format!("{error:#}"), "Neural Rendering: the earlier upscaling download could not be deleted")
        }
    }
}

/// The cache folder, readable by users (the folder helper copies from it as
/// the signed-in user) and writable only by administrators.
fn ensure_root(root: &Path) -> Result<()> {
    if root.is_dir() {
        return Ok(());
    }
    if butterpollo_windows::process::elevated()? {
        match butterpollo_windows::process::create_folder_administrators_readable(root) {
            Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => {
                return Err(e).with_context(|| format!("cannot create {}", root.display()));
            }
            _ => {}
        }
    } else {
        std::fs::create_dir_all(root)
            .with_context(|| format!("cannot create {}", root.display()))?;
    }
    Ok(())
}

/// Refuse a cache that is not this host's own: a link on the way to it, or
/// for a SYSTEM host, which copies from it into game folders and deletes in
/// it, a cache anyone but administrators may change, or a folder above it
/// they don't own or whose folders they may move away and replace. A release
/// planted there would reach every game.
fn trusted_root(root: &Path, system: bool) -> Result<()> {
    trusted_root_with(root, system, folder::administrators_hold)
}

/// [`trusted_root`], with `hold` in place of [`folder::administrators_hold`].
fn trusted_root_with(
    root: &Path,
    system: bool,
    hold: impl Fn(&Path, bool) -> Result<(), String>,
) -> Result<()> {
    use std::os::windows::fs::MetadataExt;
    for path in root.ancestors().filter(|path| !path.as_os_str().is_empty()) {
        let meta = std::fs::symlink_metadata(path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        ensure!(
            meta.file_attributes() & 0x400 == 0,
            "{} is a link to elsewhere, and Neural Rendering keeps no download behind one",
            path.display()
        );
        if system {
            hold(path, path == root).map_err(|problem| {
                anyhow!(
                    "Neural Rendering keeps its download only where nobody but administrators may change it: {problem}"
                )
            })?;
        }
    }
    Ok(())
}

/// The release Rubylight installs: [`neural::PINNED`], or a test's.
fn pinned(h: &Shared) -> neural::Pin<'static> {
    #[cfg(test)]
    if let Some(pin) = *h.neural.pin.lock().unwrap() {
        return pin;
    }
    let _ = h;
    neural::PINNED
}

/// Whether the cached `package` is the `pin`ned release: the same archives
/// by name and SHA-256, not only the same tag.
fn serves(package: &Package, pin: &neural::Pin) -> bool {
    let wanted: BTreeMap<String, String> = [pin.full, pin.addon]
        .into_iter()
        .map(|file| (file.asset.to_owned(), file.sha256.to_owned()))
        .collect();
    package.tag == pin.tag && package.archives == wanted
}

/// The cached release's files against its package.json, once per host
/// start: a file changed in the cache must not reach a game. Blocks.
fn verify_cache(h: &Shared, folder: &Path, package: &Package) -> Result<()> {
    if h.neural.lock().verified.as_deref() == Some(folder) {
        return Ok(());
    }
    for (path, placed) in &package.files {
        let file = folder.join("files").join(path);
        let (sha256, size) = folder::sha256_file(&file)
            .with_context(|| format!("cannot read {}", file.display()))?;
        ensure!(
            sha256 == placed.sha256 && size == placed.size,
            "the downloaded {path} in {} changed since it was checked",
            folder.display()
        );
    }
    h.neural.lock().verified = Some(folder.to_path_buf());
    Ok(())
}

/// The pinned release for a launch to finish an update cut short with,
/// from a cache this host trusts. None when it is not cached. Not rehashed:
/// an update copies each file checked against the release as it goes, and
/// most launches copy nothing.
fn launch_cache(h: &Shared) -> Result<Option<folder::Cached>> {
    let root = cache_root(h);
    if !root.exists() {
        return Ok(None);
    }
    trusted_root(&root, butterpollo_windows::process::is_system())?;
    let pin = pinned(h);
    Ok(cached(&root)
        .filter(|(_, package)| serves(package, &pin))
        .map(|(folder, package)| package.with(folder)))
}

/// Where GitHub's release list and the release files are: GitHub, or a
/// test's own server.
fn origins(h: &Shared) -> (String, String) {
    #[cfg(test)]
    if let Some(base) = h.neural.origin.lock().unwrap().clone() {
        return (format!("{base}/api"), base);
    }
    let _ = h;
    ("https://api.github.com".into(), neural::GITHUB.to_owned())
}

/// The updater's client for GitHub (HTTPS only, a User-Agent, which
/// GitHub's API requires), a plain one for a test's own server.
fn http(h: &Shared, timeout: Duration) -> Result<reqwest::Client> {
    #[cfg(test)]
    if h.neural.origin.lock().unwrap().is_some() {
        return Ok(reqwest::Client::builder().timeout(timeout).build()?);
    }
    let _ = h;
    crate::updater::client(timeout)
}

/// The release in the cache, the newest when a crash left two.
pub fn cached(root: &Path) -> Option<(PathBuf, Package)> {
    std::fs::read_dir(root)
        .ok()?
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter(|entry| !entry.file_name().to_string_lossy().ends_with(".partial"))
        .filter_map(|entry| {
            let folder = entry.path();
            let modified = std::fs::metadata(folder.join(PACKAGE))
                .and_then(|m| m.modified())
                .ok()?;
            let package = Package::read(&folder).ok()?;
            Some((modified, folder, package))
        })
        .max_by_key(|(modified, ..)| *modified)
        .map(|(_, folder, package)| (folder, package))
}

/// The pinned release with the whole mod for an install, in the cache:
/// downloaded and checked unless it is there already, and kept there until
/// the [`Using`] goes. A download gives up when a stream starts
/// ([`StreamStarted`]).
async fn fetch_using(h: &Shared, progress: &(dyn Fn(u64, u64) + Sync)) -> Result<(Using, Package)> {
    // Older releases go only under this lock: the release is kept before
    // another fetch may clear it.
    let _one = h.neural.fetching.lock().await;
    let (folder, package) = fetch_locked(h, progress).await?;
    *h.neural.lock().using.entry(folder.clone()).or_default() += 1;
    Ok((
        Using {
            h: h.clone(),
            folder,
        },
        package,
    ))
}

async fn fetch_locked(
    h: &Shared,
    progress: &(dyn Fn(u64, u64) + Sync),
) -> Result<(PathBuf, Package)> {
    let root = cache_root(h);
    ensure_root(&root)?;
    trusted_root(&root, butterpollo_windows::process::is_system())?;
    let pin = pinned(h);
    // Its files as checked when they came, or it goes and comes again.
    if let Some(have) = cached(&root).filter(|(_, package)| serves(package, &pin)) {
        let h = h.clone();
        let checked = tokio::task::spawn_blocking(move || {
            let (folder, package) = have;
            verify_cache(&h, &folder, &package).map(|()| (folder, package))
        })
        .await?;
        match checked {
            Ok(have) => return Ok(have),
            Err(error) => {
                tracing::warn!(error = %format!("{error:#}"), "Neural Rendering: the cached release is damaged; it is downloaded again")
            }
        }
    }
    let (api_origin, files_origin) = origins(h);
    let api = http(h, Duration::from_secs(35))?;
    let listed: Value = api
        .get(format!(
            "{api_origin}/repos/{}/releases/tags/{}",
            neural::REPOSITORY,
            pin.tag
        ))
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let release = neural::pinned(&listed, &files_origin, &pin).map_err(anyhow::Error::msg)?;
    // The release's checksum list (itself pinned) names both archives with
    // the digests GitHub reports.
    let sums_path = root.join(format!("{}.partial", release.sums.asset));
    let text = match download(h, &release.sums, &sums_path, &|_, _| {}).await {
        Ok(()) => std::fs::read_to_string(&sums_path)
            .with_context(|| format!("the release's {} is unreadable", release.sums.asset)),
        Err(error) => Err(error),
    };
    let _ = std::fs::remove_file(&sums_path);
    let sums = neural::sums(&text?).map_err(anyhow::Error::msg)?;
    let archives = [&release.full, &release.addon];
    neural::check_archives(&archives, &sums).map_err(|e| anyhow!("{e}; nothing was downloaded"))?;
    let paths: Vec<PathBuf> = archives
        .iter()
        .map(|archive| root.join(format!("{}.partial", archive.asset)))
        .collect();
    let discard = || {
        for path in &paths {
            let _ = std::fs::remove_file(path);
        }
    };
    let total = archives.iter().map(|archive| archive.size).sum();
    let mut before = 0;
    for (archive, path) in archives.iter().zip(&paths) {
        let progress = |done: u64, _: u64| progress(before + done, total);
        if let Err(error) = download(h, archive, path, &progress).await {
            discard();
            return Err(error);
        }
        before += archive.size;
    }
    let (folder, package) = {
        let (h, root, release, paths) = (h.clone(), root.clone(), release.clone(), paths.clone());
        tokio::task::spawn_blocking(move || {
            // No copy reads the cache while a release in it is replaced
            // or older ones go.
            let _writing = h.neural.writing();
            let (folder, package) = extract(&paths, &root, &release)?;
            // Games have their own copies; older releases in the cache serve
            // nothing once no install is about to copy them.
            let using = h.neural.lock().using.clone();
            for entry in std::fs::read_dir(&root).into_iter().flatten().flatten() {
                if entry.path() != folder && !using.contains_key(&entry.path()) {
                    let _ = if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                        std::fs::remove_dir_all(entry.path())
                    } else {
                        std::fs::remove_file(entry.path())
                    };
                }
            }
            h.neural.lock().verified = Some(folder.clone());
            Ok::<_, anyhow::Error>((folder, package))
        })
        .await?
    }
    .inspect_err(|_| discard())?;
    discard();
    tracing::info!(release = %release.tag, folder = %folder.display(), "Neural Rendering: downloaded and checked the frame generation release");
    Ok((folder, package))
}

async fn download(
    h: &Shared,
    release: &Download,
    path: &Path,
    progress: &(dyn Fn(u64, u64) + Sync),
) -> Result<()> {
    // `pinned` offers nothing else; this keeps it so.
    ensure!(
        neural::allowed_asset(&release.asset),
        "{} is not a file Rubylight fetches",
        release.asset
    );
    let _ = std::fs::remove_file(path);
    let mut response = http(h, Duration::from_secs(1800))?
        .get(&release.url)
        .send()
        .await?
        .error_for_status()?;
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .await
        .with_context(|| format!("cannot create {}", path.display()))?;
    let mut hash = Sha256::new();
    let mut size = 0;
    while let Some(chunk) = response.chunk().await? {
        if crate::updater::streaming(h) {
            return Err(StreamStarted.into());
        }
        size += chunk.len() as u64;
        ensure!(
            size <= release.size,
            "{} is larger than its release says",
            release.asset
        );
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
        hash.update(&chunk);
        progress(size, release.size);
    }
    file.sync_all().await?;
    verified(size, &hex::encode(hash.finalize()), release)
}

/// The file is exactly what GitHub published for the release.
fn verified(size: u64, sha256: &str, release: &Download) -> Result<()> {
    ensure!(
        size == release.size,
        "{} is {size} bytes, its release says {}",
        release.asset,
        release.size
    );
    ensure!(
        sha256 == release.digest,
        "{} does not match its release's SHA-256 digest; nothing was installed",
        release.asset
    );
    Ok(())
}

/// Unpack the release into the cache: from each archive (the full package,
/// then the add-on, as downloaded and checked) what [`neural::Archive`]
/// takes, under `files/<prefix>/`. Blocks.
fn extract(archives: &[PathBuf], root: &Path, release: &Release) -> Result<(PathBuf, Package)> {
    let staging = root.join(format!("{}.partial", release.tag));
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }
    let into = staging.join("files");
    std::fs::create_dir_all(&into)?;
    let [full, addon] = archives else {
        bail!("the release's two archives are not there to unpack");
    };
    let mut total = 0u64;
    let mut files = BTreeMap::new();
    for (path, kind, download) in [
        (full, neural::Archive::Full, &release.full),
        (addon, neural::Archive::Addon, &release.addon),
    ] {
        files.extend(unpack(path, &download.asset, kind, &into, &mut total)?);
    }
    neural::check_whole(&files)
        .map_err(|e| anyhow!("{}: {e}; nothing was installed", release.tag))?;
    let package = Package {
        tag: release.tag.clone(),
        archives: [&release.full, &release.addon]
            .into_iter()
            .map(|download| (download.asset.clone(), download.digest.clone()))
            .collect(),
        files,
    };
    butterpollo_core::state::write_json(&staging.join(PACKAGE), &package)?;
    let folder = root.join(&release.tag);
    // The same release there before (damaged, or changed on GitHub) goes
    // aside whole, never half deleted under an install about to copy it;
    // the cache's clearing takes it out after.
    let aside = root.join(format!("{}.replaced.partial", release.tag));
    if aside.exists() {
        std::fs::remove_dir_all(&aside)?;
    }
    let replacing = folder.exists();
    if replacing {
        std::fs::rename(&folder, &aside)
            .with_context(|| format!("cannot replace {}", folder.display()))?;
    }
    if let Err(error) = std::fs::rename(&staging, &folder) {
        let back = if !replacing || std::fs::rename(&aside, &folder).is_ok() {
            ""
        } else {
            ", and the release before it stays aside"
        };
        return Err(error).with_context(|| format!("cannot place {}{back}", folder.display()));
    }
    Ok((folder, package))
}

/// Unpack the entries of one archive that `kind` takes into
/// `into/<prefix>/`, with their SHA-256, keyed by that cache path. Every
/// entry must name a path inside the folder, taken or not. Blocks.
fn unpack(
    archive: &Path,
    asset: &str,
    kind: neural::Archive,
    into: &Path,
    total: &mut u64,
) -> Result<BTreeMap<String, Placed>> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(archive)?)
        .with_context(|| format!("{asset} is no zip archive"))?;
    let mut files = BTreeMap::new();
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        let name = entry.name().to_owned();
        let path = neural::entry_path(&name)
            .with_context(|| format!("{asset} has a file outside its folder: {name:?}"))?;
        if entry.is_dir() || !kind.takes(&path) {
            continue;
        }
        let cached = format!("{}/{path}", kind.prefix());
        ensure!(!files.contains_key(&cached), "{asset} has {path} twice");
        let target = into.join(&cached);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut output = std::fs::File::create_new(&target)?;
        let mut hash = Sha256::new();
        let mut buffer = vec![0; 1 << 20];
        let mut size = 0u64;
        loop {
            let read = entry.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            size += read as u64;
            *total += read as u64;
            ensure!(
                *total <= 4 * neural::MAX_ARCHIVE,
                "{asset} unpacks to more than it should"
            );
            hash.update(&buffer[..read]);
            output.write_all(&buffer[..read])?;
        }
        output.sync_all()?;
        files.insert(
            cached,
            Placed {
                sha256: hex::encode(hash.finalize()),
                size,
            },
        );
    }
    Ok(files)
}

// ---- Apps ----

fn app(h: &Shared, uuid: &str) -> Option<App> {
    h.apps
        .read()
        .unwrap()
        .iter()
        .find(|app| app.extra.get("uuid").and_then(Value::as_str) == Some(uuid))
        .cloned()
}

/// The folder the app's launcher installed the game in, when it says.
fn install(app: &App) -> Option<PathBuf> {
    ["steam-install-dir", "launcher-install-dir"]
        .iter()
        .find_map(|key| app.extra.get(*key)?.as_str())
        .map(str::trim)
        .filter(|folder| !folder.is_empty())
        .map(PathBuf::from)
}

/// Why the game is refused when Steam lists Valve Anti-Cheat for it, which
/// no file in its install shows, or can't say ([`steam::valve_anti_cheat`]
/// with Steam's `roots`): asked for each of `paths` ([`reached_by`]), as a
/// game's folder moved elsewhere and linked back into Steam's library is a
/// plain folder where the link leads.
///
/// [`steam::valve_anti_cheat`]: butterpollo_core::steam::valve_anti_cheat
fn steam_refusal(app: &App, paths: &[PathBuf], roots: &[PathBuf]) -> Option<String> {
    let appid = serde_json::to_value(app)
        .ok()
        .and_then(|app| butterpollo_core::steam::game_of(&app));
    // A Steam game's link is asked once: its flag names no file.
    let paths = if appid.is_some() {
        &paths[..paths.len().min(1)]
    } else {
        paths
    };
    paths.iter().find_map(|exe| {
        match butterpollo_core::steam::valve_anti_cheat(appid, exe, "", roots) {
            Ok(false) => None,
            Ok(true) => Some(format!(
                "Steam lists Valve Anti-Cheat for the game, and {}",
                folder::ANTI_CHEAT
            )),
            Err(message) => Some(format!(
                "Rubylight can't tell whether the game uses Valve Anti-Cheat ({message})"
            )),
        }
    })
}

/// The game's .exe `exe` (where a linked game folder leads, as the helper
/// works there) and the paths it was reached by before the link was
/// followed: the chosen program `requested`, the app's command, and `exe`
/// in the app's install as named.
fn reached_by(app: &App, requested: Option<&Path>, exe: &Path) -> Vec<PathBuf> {
    let named = install(app).and_then(|install| {
        let relative = folder::real(&install)
            .and_then(|real| exe.strip_prefix(real).ok().map(Path::to_path_buf))
            .or_else(|| exe.strip_prefix(&install).ok().map(Path::to_path_buf))
            .or_else(|| exe.file_name().map(PathBuf::from))?;
        Some(install.join(relative))
    });
    let mut paths = vec![exe.to_path_buf()];
    for path in [requested.map(Path::to_path_buf), command_exe(app), named]
        .into_iter()
        .flatten()
    {
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    paths
}

/// Steam's folders on this PC, or a test's.
fn steam_roots(h: &Shared) -> Vec<PathBuf> {
    #[cfg(test)]
    if let Some(roots) = h.neural.steam_roots.lock().unwrap().clone() {
        return roots;
    }
    let _ = h;
    butterpollo_windows::steam::roots()
}

/// The game's program as an install places it ([`Request::Find`]), or why
/// not: Steam lists Valve Anti-Cheat for the game at it or at a path it was
/// reached by ([`steam_refusal`]). Blocks.
fn find_game(
    h: &Shared,
    app: &App,
    exe: Option<PathBuf>,
    install: Option<PathBuf>,
) -> Result<Response> {
    let requested = exe.clone();
    let exe = match run_folder(Request::Find { exe, install }, &|_, _| {})? {
        Response::Found { exe } => exe,
        other => return Ok(other),
    };
    let paths = reached_by(app, requested.as_deref(), &exe);
    Ok(match steam_refusal(app, &paths, &steam_roots(h)) {
        Some(reason) => Response::Refused { reason },
        None => Response::Found { exe },
    })
}

/// The app's command when it starts a program directly.
fn command_exe(app: &App) -> Option<PathBuf> {
    let target = PathBuf::from(butterpollo_windows::process::command_target(&app.cmd));
    (target.is_absolute()
        && target
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("exe")))
    .then_some(target)
}

/// The app's command when it starts the game itself: in the game's install,
/// which the app must name, and no launcher, interpreter or program of
/// Windows. Otherwise the game's .exe is found in its install, or the user
/// picks it: nothing outside a known install is taken unasked.
fn game_command(app: &App) -> Option<PathBuf> {
    let install = install(app)?;
    command_exe(app).filter(|exe| {
        stalls(exe);
        exe.is_file() && folder::inside(exe, &install) && folder::not_the_game(exe).is_none()
    })
}

/// The .exe the files sit beside, once they are in.
fn placed_exe(app: &App) -> Option<PathBuf> {
    exe_of(app, KEY)
}

/// The .exe Rubylight 3.0 placed AMDNR beside, until it is removed.
fn legacy_exe(app: &App) -> Option<PathBuf> {
    exe_of(app, LEGACY_KEY)
}

fn exe_of(app: &App, key: &str) -> Option<PathBuf> {
    app.extra.get(key)?.get("exe")?.as_str().map(PathBuf::from)
}

/// Every folder the host placed files in for the app: this release's and
/// Rubylight 3.0's, each once.
fn placed_exes(app: &App) -> Vec<PathBuf> {
    let mut exes: Vec<PathBuf> = placed_exe(app).into_iter().collect();
    if let Some(legacy) = legacy_exe(app).filter(|legacy| !exes.contains(legacy)) {
        exes.push(legacy);
    }
    exes
}

/// Whether the host placed files for the app, now or in Rubylight 3.0.
fn has_files(app: &App) -> bool {
    app.extra.contains_key(KEY) || app.extra.contains_key(LEGACY_KEY)
}

/// Set or clear the app's `key` and save the apps.
fn set_key(h: &Shared, uuid: &str, key: &str, value: Option<Value>) -> Result<()> {
    h.reload_apps();
    let mut apps = h.apps.write().unwrap();
    let mut next = apps.clone();
    let app = next
        .iter_mut()
        .find(|app| app.extra.get("uuid").and_then(Value::as_str) == Some(uuid))
        .context("application not found")?;
    match value {
        Some(value) => app.extra.insert(key.into(), value),
        None => app.extra.remove(key),
    };
    h.assign_apps(&mut next)?;
    let mut doc = h.app_document.read().unwrap().clone();
    doc["apps"] = serde_json::to_value(&next)?;
    h.save_apps(&doc)?;
    *apps = next;
    *h.app_document.write().unwrap() = doc;
    h.config_version
        .fetch_add(1, std::sync::atomic::Ordering::Release);
    Ok(())
}

/// A saved app keeps the host's keys: the console never sets them.
pub fn keep_key(stored: Option<&App>, app: &mut App) {
    for key in [KEY, LEGACY_KEY] {
        app.extra.remove(key);
        if let Some(value) = stored.and_then(|stored| stored.extra.get(key)) {
            app.extra.insert(key.into(), value.clone());
        }
    }
}

// ---- The switch ----

/// Why the switch cannot be turned on or off now.
#[derive(Debug)]
pub enum Refusal {
    NotAccepted,
    Unavailable(String),
    NotFound,
    Busy,
    /// A stream runs. Turning on, the download would take its bandwidth and
    /// the copy into the game's folder its disk; turning off, the streamed
    /// game may hold the provider's DLL loaded, which can't be deleted.
    Streaming,
}

/// An app delete refused while the switch is busy with the app: the
/// install's files would have no app left to be removed with.
#[derive(Debug)]
pub struct Busy(pub String);
impl std::fmt::Display for Busy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Neural Rendering is busy with {}; try again once it is done",
            self.0
        )
    }
}
impl std::error::Error for Busy {}

/// An app delete refused while a stream runs: the app's game may be the
/// streamed one, with the provider's DLL loaded.
#[derive(Debug)]
pub struct StreamRunning(pub String);
impl std::fmt::Display for StreamRunning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} may be running with Neural Rendering loaded: remove it when no stream is running",
            self.0
        )
    }
}
impl std::error::Error for StreamRunning {}

/// A download given up as a stream started: it would take the stream's
/// bandwidth.
#[derive(Debug)]
pub struct StreamStarted;
impl std::fmt::Display for StreamStarted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a stream started, so the download stopped; turn frame generation on again once the stream ends")
    }
}
impl std::error::Error for StreamStarted {}

fn running(state: &State, uuid: &str) -> bool {
    state
        .jobs
        .get(uuid)
        .is_some_and(|job| !matches!(job.phase, "done" | "failed"))
}

pub(crate) fn start_job(
    h: &Shared,
    uuid: &str,
    action: &'static str,
    phase: &'static str,
) -> Result<(), Refusal> {
    let mut state = h.neural.lock();
    if running(&state, uuid) {
        return Err(Refusal::Busy);
    }
    forget_game_check(&mut state, uuid);
    state.jobs.insert(
        uuid.into(),
        Job {
            action,
            phase,
            done: 0,
            total: 0,
            error: None,
            choose_exe: false,
            kept: Vec::new(),
        },
    );
    Ok(())
}
fn update_job(h: &Shared, uuid: &str, change: impl FnOnce(&mut Job)) {
    let mut state = h.neural.lock();
    forget_game_check(&mut state, uuid);
    if let Some(job) = state.jobs.get_mut(uuid) {
        change(job);
    }
}
/// A job changes the game's folder: its own check is asked again, and one
/// that runs now is not kept ([`game_check`]).
fn forget_game_check(state: &mut State, uuid: &str) {
    state.games.remove(uuid);
    *state.generation.entry(uuid.to_owned()).or_default() += 1;
}
fn fail_job(h: &Shared, uuid: &str, error: String, choose_exe: bool) {
    update_job(h, uuid, |job| {
        job.phase = "failed";
        job.error = Some(neural::shown(&error));
        job.choose_exe = choose_exe;
    });
}

/// What a second install for a game that has it is told.
const ALREADY_ON: &str = "frame generation is already on for this game; turn it off first";

/// Turn the switch on: download the release unless it is cached, then place
/// it beside the game's .exe (`exe`, the user's pick, or found). A game with
/// Rubylight 3.0's AMDNR has it taken out first, once the game passed every
/// check and the download is in. Runs on.
pub fn enable(h: &Shared, uuid: &str, exe: Option<PathBuf>) -> Result<(), Refusal> {
    if !accepted(h) {
        return Err(Refusal::NotAccepted);
    }
    let app = app(h, uuid);
    // The runtime beside the program the user picked, else the one the app names.
    let folder = exe
        .as_deref()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .or_else(|| app.as_ref().and_then(game_folder));
    capability(h, folder.as_deref()).map_err(Refusal::Unavailable)?;
    let app = app.ok_or(Refusal::NotFound)?;
    // A console that still shows the switch off may name another program: the files
    // beside the first would stay in the game's folder with no record left of them.
    if placed_exe(&app).is_some() {
        return Err(Refusal::Unavailable(ALREADY_ON.into()));
    }
    if crate::updater::streaming(h) {
        return Err(Refusal::Streaming);
    }
    start_job(h, uuid, "install", "download")?;
    let (h, uuid) = (h.clone(), uuid.to_owned());
    tokio::spawn(async move {
        // The earlier upscaling's folder is the game's.
        let exe = exe
            .or_else(|| legacy_exe(&app))
            .or_else(|| game_command(&app));
        let install = install(&app);
        // The game's program, its DLSS frame generation and Valve Anti-Cheat
        // before the download: a game that is refused never waits for it.
        // Asked again after it, as the game may have changed meanwhile.
        let checked = {
            let (h, app, exe, install) = (h.clone(), app.clone(), exe.clone(), install.clone());
            tokio::task::spawn_blocking(move || find_game(&h, &app, exe, install)).await
        };
        match checked {
            Ok(Ok(Response::Found { .. })) => {}
            Ok(response) => return finish_install(&h, &uuid, &app, None, response),
            Err(error) => {
                return fail_job(&h, &uuid, format!("the install stopped: {error}"), false);
            }
        }
        let progress = |done, total| {
            update_job(&h, &uuid, |job| {
                job.done = done;
                job.total = total;
            })
        };
        let (using, package) = match fetch_using(&h, &progress).await {
            Ok(cached) => cached,
            Err(error) => {
                // The details name the release's files; the console says what happened.
                tracing::warn!(app = %app.name, error = %format!("{error:#}"), "Neural Rendering: the download failed");
                let shown = if error.is::<StreamStarted>() {
                    StreamStarted.to_string()
                } else {
                    DOWNLOAD_FAILED.into()
                };
                return fail_job(&h, &uuid, shown, false);
            }
        };
        update_job(&h, &uuid, |job| {
            (job.phase, job.done, job.total) = ("place", 0, 0)
        });
        let package = package.with(using.folder.clone());
        let worker = h.clone();
        let task_uuid = uuid.clone();
        let finished = tokio::task::spawn_blocking(move || {
            let progress = |done, total| {
                update_job(&worker, &task_uuid, |job| {
                    (job.done, job.total) = (done, total)
                })
            };
            let mut recorded = None;
            let response = (|| {
                let _reading = worker.neural.reading();
                let _using = using;
                let exe = match find_game(&worker, &app, exe, install.clone())? {
                    Response::Found { exe } => exe,
                    other => return Ok(other),
                };
                // Rubylight 3.0's AMDNR goes first: the two can't share a
                // game. Every refusal the install gives is asked before, with
                // AMDNR taken as gone, so a game refused keeps it. Once it is
                // out, the app no longer names it, whatever the install does
                // next; a file of it that stayed in the folder the install
                // goes to stops the install. AMDNR beside another program
                // (the launcher, with the game's own .exe chosen now) is
                // judged in its own folder, and what stays there is named.
                if let Some(legacy) = legacy_exe(&app) {
                    let check = Request::Check {
                        exe: Some(exe.clone()),
                        install: install.clone(),
                        package: package.clone(),
                        legacy: Some(legacy.clone()),
                    };
                    match run_folder(check, &|_, _| {})? {
                        Response::Found { .. } => {}
                        other => return Ok(other),
                    }
                    update_job(&worker, &task_uuid, |job| job.phase = "remove");
                    match remove_beside(&app.name, &legacy, Some(&exe), false)? {
                        // Left in another folder: its record stays, so the
                        // console still offers its removal, which names what
                        // stays then.
                        (Removed::Done(_), true) => {}
                        (Removed::Done(kept), false) if !kept.is_empty() => {
                            set_key(&worker, &task_uuid, LEGACY_KEY, None)?;
                            update_job(&worker, &task_uuid, |job| {
                                job.kept = kept.iter().map(|path| neural::shown(path)).collect();
                            });
                            return Ok(Response::Error {
                                message: folder::kept_text(&kept, true),
                            });
                        }
                        (Removed::Done(_), false) => set_key(&worker, &task_uuid, LEGACY_KEY, None)?,
                        (Removed::Unreachable(folder), _) => {
                            return Ok(Response::Error {
                                message: format!(
                                    "{folder} can't be reached, so the earlier upscaling could not be removed; connect its drive and try again"
                                ),
                            });
                        }
                    }
                    update_job(&worker, &task_uuid, |job| job.phase = "place");
                }
                // The game's program first: its removal is recorded before
                // anything is written, so an install cut short (a timeout,
                // a host restart) is taken out by the pending removals.
                let pending = Pending {
                    name: app.name.clone(),
                    exe: exe.clone(),
                    uuid: Some(task_uuid.clone()),
                };
                record_pending(&worker, std::slice::from_ref(&pending))?;
                recorded = Some(exe.clone());
                let request = Request::Install {
                    exe: Some(exe),
                    install,
                    package,
                };
                run_folder(request, &progress)
            })();
            // Saving the app and any taking back block too.
            finish_install(&worker, &task_uuid, &app, recorded.as_deref(), response);
        })
        .await;
        if let Err(error) = finished {
            fail_job(&h, &uuid, format!("the install stopped: {error}"), false);
        }
    });
    Ok(())
}

/// What the app's key says once the files are in.
fn key_value(exe: &Path, release: &str, proxy: &str, mode: neural::Mode) -> Value {
    json!({
        "exe": exe,
        "release": release,
        "proxy": proxy,
        "mode": mode,
        "switches": [neural::Switch::FrameGeneration],
    })
}

/// The install's end: the app saved with the files, or the job failed. The
/// removal `recorded` before the install goes once the files are in and
/// saved, or nothing was written; otherwise it takes out what is left. Blocks.
fn finish_install(
    h: &Shared,
    uuid: &str,
    app: &App,
    recorded: Option<&Path>,
    response: Result<Response>,
) {
    let clear = || {
        if let Some(exe) = recorded {
            clear_pending(h, exe);
        }
    };
    match response {
        Ok(Response::Placed {
            exe,
            release,
            proxy,
            mode,
            ..
        }) => {
            if let Err(error) = set_key(h, uuid, KEY, Some(key_value(&exe, &release, &proxy, mode)))
            {
                // The app went while the job ran (a library sync dropped it)
                // or could not be saved: nothing would remove the files later.
                tracing::warn!(app = %app.name, error = %format!("{error:#}"), "Neural Rendering: the app could not be saved with frame generation; taking it out again");
                let after = match take_back(h, uuid, &app.name, &exe) {
                    Ok(true) => {
                        clear();
                        "frame generation was taken out again".to_owned()
                    }
                    Ok(false) => "frame generation's removal is pending until the game's folder can be changed".to_owned(),
                    Err(error) => format!("{error:#}"),
                };
                return fail_job(
                    h,
                    uuid,
                    format!("the app could not be saved ({error:#}); {after}"),
                    false,
                );
            }
            clear();
            tracing::info!(app = %app.name, folder = %exe.parent().unwrap_or(&exe).display(), %release, %proxy, ?mode, "Neural Rendering: frame generation installed");
            update_job(h, uuid, |job| job.phase = "done");
        }
        // Refused before anything was written.
        Ok(Response::ChooseExe { reason }) => {
            clear();
            tracing::info!(app = %app.name, %reason, "Neural Rendering needs the game's program chosen");
            fail_job(h, uuid, reason, true);
        }
        Ok(Response::Refused { reason }) => {
            clear();
            tracing::warn!(app = %app.name, %reason, "Neural Rendering refused the game");
            fail_job(h, uuid, reason, false);
        }
        Ok(Response::Error { message }) => {
            tracing::warn!(app = %app.name, error = %message, "Neural Rendering could not be installed");
            fail_job(h, uuid, message, false);
        }
        Ok(
            Response::Removed { .. }
            | Response::Left { .. }
            | Response::Unreachable { .. }
            | Response::NotWritable { .. }
            | Response::Found { .. },
        ) => fail_job(h, uuid, "unexpected helper answer".into(), false),
        Err(error) => {
            tracing::warn!(app = %app.name, error = %format!("{error:#}"), "Neural Rendering could not be installed");
            fail_job(h, uuid, format!("{error:#}"), false);
        }
    }
}

/// Turn the switch off: remove what was placed, frame generation or
/// Rubylight 3.0's upscaling. Runs on.
pub fn disable(h: &Shared, uuid: &str) -> Result<(), Refusal> {
    removal(h, uuid, false)
}

/// Remove only Rubylight 3.0's upscaling, which frame generation installed
/// beside another program left a record of: frame generation stays. Runs on.
pub fn remove_earlier(h: &Shared, uuid: &str) -> Result<(), Refusal> {
    removal(h, uuid, true)
}

fn removal(h: &Shared, uuid: &str, earlier: bool) -> Result<(), Refusal> {
    let app = app(h, uuid).ok_or(Refusal::NotFound)?;
    // The streamed game may be the one: it holds the DLLs loaded, so the
    // removal would stop at the first and ask to close the game mid-stream.
    if crate::updater::streaming(h) {
        return Err(Refusal::Streaming);
    }
    start_job(h, uuid, "remove", "remove")?;
    let (h, uuid) = (h.clone(), uuid.to_owned());
    let removed = move |app: &App| match legacy_exe(app).filter(|_| earlier) {
        Some(legacy) => remove_beside(&app.name, &legacy, None, true).map(|(removed, _)| removed),
        None if earlier => Ok(Removed::Done(Vec::new())),
        None => remove(app),
    };
    tokio::task::spawn_blocking(move || match removed(&app) {
        Ok(Removed::Done(kept)) => {
            let off = if earlier {
                Ok(())
            } else {
                set_key(&h, &uuid, KEY, None)
            };
            match off.and_then(|()| set_key(&h, &uuid, LEGACY_KEY, None)) {
                Ok(()) => update_job(&h, &uuid, |job| {
                    job.phase = "done";
                    job.kept = kept.iter().map(|path| neural::shown(path)).collect();
                }),
                Err(error) => fail_job(&h, &uuid, format!("{error:#}"), false),
            }
        }
        Ok(Removed::Unreachable(folder)) => {
            tracing::warn!(app = %app.name, %folder, "Neural Rendering could not be removed: the game's folder can't be reached");
            fail_job(
                &h,
                &uuid,
                format!("{folder} can't be reached; connect its drive and turn it off again"),
                false,
            )
        }
        Err(error) => fail_job(&h, &uuid, format!("{error:#}"), false),
    });
    Ok(())
}

/// How a removal from a game folder ended.
#[derive(Debug, PartialEq, Eq)]
enum Removed {
    /// The files that changed since, or that someone else put in the
    /// folders the host created, and stayed.
    Done(Vec<String>),
    /// The folder's drive or share is not there now.
    Unreachable(String),
}

/// Remove what the host placed in the app's game folders. Blocks.
fn remove(app: &App) -> Result<Removed> {
    let mut kept = Vec::new();
    for exe in placed_exes(app) {
        match remove_at(&app.name, &exe)? {
            Removed::Done(more) => kept.extend(more),
            unreachable => return Ok(unreachable),
        }
    }
    Ok(Removed::Done(kept))
}

/// Remove whatever the host placed beside `exe`: frame generation, and
/// Rubylight 3.0's AMDNR, each by its manifest. Blocks.
fn remove_at(name: &str, exe: &Path) -> Result<Removed> {
    remove_beside(name, exe, None, false).map(|(removed, _)| removed)
}

/// [`remove_at`], and whether AMDNR's record stays for what it left beside
/// another program than `beside`, the one an install goes beside
/// ([`Response::Left`]); with `earlier`, only AMDNR. Blocks.
fn remove_beside(
    name: &str,
    exe: &Path,
    beside: Option<&Path>,
    earlier: bool,
) -> Result<(Removed, bool)> {
    let request = Request::Remove {
        exe: exe.into(),
        beside: beside.map(Path::to_path_buf),
        earlier,
    };
    let (kept, record) = match run_folder(request, &|_, _| {})? {
        Response::Removed { kept } => (kept, false),
        Response::Left { kept } => (kept, true),
        Response::Unreachable { folder } => return Ok((Removed::Unreachable(folder), false)),
        Response::Error { message }
        | Response::Refused { reason: message }
        | Response::ChooseExe { reason: message } => {
            tracing::warn!(app = %name, error = %message, "Neural Rendering could not be removed");
            bail!("{message}")
        }
        Response::Placed { .. } | Response::NotWritable { .. } | Response::Found { .. } => {
            bail!("unexpected helper answer")
        }
    };
    let folder = exe.parent().unwrap_or(exe).display().to_string();
    if kept.is_empty() {
        tracing::info!(app = %name, %folder, "Neural Rendering removed");
    } else {
        tracing::warn!(app = %name, %folder, kept = ?kept, "Neural Rendering removed; files that changed since, are not Rubylight's or were held open were left");
    }
    Ok((Removed::Done(kept), record))
}

/// Before an app is deleted: the switch's slot for it, held until the caller
/// drops the [`Removing`], once the app is gone. A delete of several apps
/// takes every slot before any file is removed, so one busy app refuses the
/// delete with nothing changed. Then [`Removing::remove_mod`].
pub fn reserve(h: &Shared, app: &App) -> Result<Removing> {
    // As `disable`: the streamed game may hold the DLLs loaded, so the
    // removal would stop at the first and ask to close the game mid-stream.
    // Refused before any slot is taken or file touched; the app stays.
    if has_files(app) && crate::updater::streaming(h) {
        return Err(StreamRunning(app.name.clone()).into());
    }
    let uuid = app.extra.get("uuid").and_then(Value::as_str);
    // An install not finished has no key to remove by yet, and a launch's
    // update would race the removal.
    Ok(Removing::take(h, uuid).map_err(|_| Busy(app.name.clone()))?)
}

/// One app's [`reserve`] and [`Removing::remove_mod`].
#[cfg(test)]
pub fn before_delete(h: &Shared, app: &App) -> Result<Removing> {
    let slot = reserve(h, app)?;
    slot.remove_mod(app)?;
    Ok(slot)
}

/// The switch's slot taken for an app's removal, so no install or launch
/// update writes its folder meanwhile; done when dropped. A failure the
/// slot held before (an install that failed, its removal pending) shows
/// again after, not a removal the user never asked for.
pub struct Removing {
    h: Shared,
    uuid: Option<String>,
    failed: Option<Job>,
}
impl Removing {
    fn take(h: &Shared, uuid: Option<&str>) -> Result<Self, Refusal> {
        let failed = uuid.and_then(|uuid| {
            h.neural
                .lock()
                .jobs
                .get(uuid)
                .filter(|job| job.phase == "failed")
                .cloned()
        });
        if let Some(uuid) = uuid {
            start_job(h, uuid, "remove", "remove")?;
        }
        Ok(Self {
            h: h.clone(),
            uuid: uuid.map(str::to_owned),
            failed,
        })
    }

    /// For an app with files placed: take them out of its folder, or keep
    /// the app (an error) so nothing is left behind unnoticed. A folder on a
    /// drive that is not connected becomes a pending removal instead, so a
    /// game on a drive that is gone for good can still be deleted. Blocks.
    pub fn remove_mod(&self, app: &App) -> Result<()> {
        if !has_files(app) {
            return Ok(());
        }
        let kept =
            "Neural Rendering could not be removed from the game's folder, so the app was kept";
        let removed = (|| {
            if let Removed::Unreachable(folder) = remove(app).context(kept)? {
                record_pending(&self.h, &Pending::of(app)).context(kept)?;
                tracing::warn!(app = %app.name, %folder, "Neural Rendering: the game's folder can't be reached; its removal is pending");
            }
            anyhow::Ok(())
        })()
        .map_err(|error| {
            // The app delete's answer names no provider path; the log has it.
            let full = format!("{error:#}");
            tracing::warn!(app = %app.name, error = %full, "Neural Rendering could not be removed for the app's delete");
            anyhow!(neural::shown(&full))
        });
        if let (Err(error), Some(uuid)) = (&removed, &self.uuid) {
            fail_job(&self.h, uuid, error.to_string(), false);
        }
        removed
    }
}
impl Drop for Removing {
    fn drop(&mut self) {
        if let Some(uuid) = &self.uuid {
            let failed = self.failed.take();
            update_job(&self.h, uuid, |job| {
                if job.phase == "remove" {
                    match failed {
                        Some(failed) => *job = failed,
                        None => job.phase = "done",
                    }
                }
            });
        }
    }
}

// ---- Removals left for later ----

/// A game folder the files are still to be removed from: its app left the
/// library while the folder could not be reached or the removal failed.
/// Rubylight 3.0's records name AMDNR's folders the same way.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Pending {
    name: String,
    exe: PathBuf,
    /// The app's, whose switch slot the removal takes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    uuid: Option<String>,
}
impl Pending {
    /// One for each folder the host placed files in for the app.
    fn of(app: &App) -> Vec<Self> {
        let uuid = app
            .extra
            .get("uuid")
            .and_then(Value::as_str)
            .map(str::to_owned);
        placed_exes(app)
            .into_iter()
            .map(|exe| Self {
                name: app.name.clone(),
                exe,
                uuid: uuid.clone(),
            })
            .collect()
    }
}

/// The record's removals, none when it has no list yet. A list that can't
/// be read is an error, never taken for empty and written over.
fn pending_in(record: &Value) -> Result<Vec<Pending>> {
    match &record[PENDING] {
        Value::Null => Ok(Vec::new()),
        list => serde_json::from_value(list.clone())
            .with_context(|| format!("{SETTINGS_FILE}'s pending removals are damaged")),
    }
}

/// [`pending_in`] for a reader, empty and logged when it can't be read.
fn pending(h: &Shared) -> Vec<Pending> {
    pending_in(&settings(h)).unwrap_or_else(|error| {
        tracing::warn!(error = %format!("{error:#}"), "Neural Rendering: its pending removals can't be read");
        Vec::new()
    })
}

/// Take the files placed beside `exe` out again (true), or leave their
/// removal pending when that fails (false); Err only when neither worked.
/// Blocks.
fn take_back(h: &Shared, uuid: &str, name: &str, exe: &Path) -> Result<bool> {
    let why = match remove_at(name, exe) {
        Ok(Removed::Done(_)) => return Ok(true),
        Ok(Removed::Unreachable(folder)) => format!("{folder} can't be reached"),
        Err(error) => format!("{error:#}"),
    };
    let pending = Pending {
        name: name.into(),
        exe: exe.into(),
        uuid: Some(uuid.into()),
    };
    record_pending(h, std::slice::from_ref(&pending)).with_context(|| {
        format!(
            "frame generation could not be taken out of {} ({why}), and its removal could not be saved",
            exe.parent().unwrap_or(exe).display()
        )
    })?;
    tracing::warn!(app = %name, reason = %why, "Neural Rendering: taking the files out again failed; their removal is pending");
    Ok(false)
}

/// Drop the removal beside `exe` from the record: the files are meant to be
/// there, or nothing was written.
fn clear_pending(h: &Shared, exe: &Path) {
    let saved = update_settings(h, |value| {
        let mut list = pending_in(value)?;
        list.retain(|removal| removal.exe != exe);
        value[PENDING] = json!(list);
        Ok(())
    });
    if let Err(error) = saved {
        tracing::warn!(error = %format!("{error:#}"), "Neural Rendering: the pending removals could not be saved");
    }
}

/// Add removals to the record (once each).
fn record_pending(h: &Shared, removals: &[Pending]) -> Result<()> {
    update_settings(h, |value| {
        let mut list = pending_in(value)?;
        for removal in removals {
            if !list.iter().any(|old| old.exe == removal.exe) {
                list.push(removal.clone());
            }
        }
        value[PENDING] = json!(list);
        Ok(())
    })
}

/// The apps with files placed that a library change left out: an
/// uninstalled game the launcher sync dropped.
pub fn dropped(before: &[App], after: &[App]) -> Vec<App> {
    let uuid = |app: &App| {
        app.extra
            .get("uuid")
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    before
        .iter()
        .filter(|app| has_files(app))
        .filter(|app| {
            uuid(app).is_some_and(|id| {
                !after
                    .iter()
                    .any(|other| uuid(other).as_deref() == Some(&id))
            })
        })
        .cloned()
        .collect()
}

/// Take the files out of the folders of apps the library sync dropped, as a
/// delete does. Each is recorded first; one that fails (a folder on a drive
/// that is not connected) stays recorded for [`remove_pending`]. Blocks.
pub fn remove_dropped(h: &Shared, apps: &[App]) {
    let removals: Vec<Pending> = apps.iter().flat_map(Pending::of).collect();
    if removals.is_empty() {
        return;
    }
    for removal in &removals {
        tracing::info!(app = %removal.name, "Neural Rendering: the library sync dropped the game; removing its files");
    }
    if let Err(error) = record_pending(h, &removals) {
        tracing::warn!(error = %format!("{error:#}"), "Neural Rendering: the pending removals could not be saved");
    }
    try_pending(h, &removals);
}

/// Whether the maintenance loop should try the pending removals again.
pub fn removal_due(h: &Shared) -> bool {
    {
        let mut state = h.neural.lock();
        if state.removal_at.is_some_and(|at| Instant::now() < at) {
            return false;
        }
        state.removal_at = Some(Instant::now() + PENDING_RETRY);
    }
    !pending(h).is_empty()
}

/// Try the recorded removals again. Blocks.
pub fn remove_pending(h: &Shared) {
    try_pending(h, &[]);
}

/// Try the recorded removals and `also` (in case the record could not be
/// saved); keep the ones that are still to do.
fn try_pending(h: &Shared, also: &[Pending]) {
    let _one = h
        .neural
        .removing
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let mut removals = pending(h);
    for removal in also {
        if !removals.iter().any(|old| old.exe == removal.exe) {
            removals.push(removal.clone());
        }
    }
    // The files an app has on stay: such a record is stale. The same folder
    // also when one path goes through a link (a game folder moved and linked
    // back) and the other doesn't.
    let wanted: Vec<PathBuf> = h
        .apps
        .read()
        .unwrap()
        .iter()
        .flat_map(placed_exes)
        .collect();
    let real = |exe: &Path| folder::real(exe.parent().unwrap_or(exe));
    let mut done = Vec::new();
    for removal in &removals {
        let stale = wanted.iter().any(|exe| {
            *exe == removal.exe || real(exe).is_some_and(|at| Some(at) == real(&removal.exe))
        });
        if stale {
            done.push(removal.exe.clone());
            continue;
        }
        let Ok(_slot) = Removing::take(h, removal.uuid.as_deref()) else {
            tracing::info!(app = %removal.name, "Neural Rendering is busy with the game; its removal stays pending");
            continue;
        };
        match remove_at(&removal.name, &removal.exe) {
            Ok(Removed::Done(_)) => done.push(removal.exe.clone()),
            Ok(Removed::Unreachable(folder)) => {
                tracing::info!(app = %removal.name, %folder, "Neural Rendering: the game's folder can't be reached; its removal stays pending");
            }
            Err(error) => {
                tracing::warn!(app = %removal.name, error = %format!("{error:#}"), "Neural Rendering: the removal stays pending");
            }
        }
    }
    if done.is_empty() && also.is_empty() {
        return;
    }
    let saved = update_settings(h, |value| {
        let mut list = pending_in(value)?;
        for removal in also {
            if !list.iter().any(|old| old.exe == removal.exe) {
                list.push(removal.clone());
            }
        }
        list.retain(|removal| !done.contains(&removal.exe));
        value[PENDING] = json!(list);
        Ok(())
    });
    if let Err(error) = saved {
        tracing::warn!(error = %format!("{error:#}"), "Neural Rendering: the pending removals could not be saved");
    }
}

/// Before a launch: the files must still be as placed and the game must not
/// have gained anti-cheat; an update cut short is finished. No other
/// release is applied. Rubylight 3.0's AMDNR is checked the same way, with
/// nothing to finish an update from. Fails the launch loudly otherwise.
/// Blocks.
pub fn before_launch(h: &Shared, app: &App) -> Result<()> {
    let (exe, legacy) = match (placed_exe(app), legacy_exe(app)) {
        (Some(exe), _) => (exe, false),
        (None, Some(exe)) => (exe, true),
        (None, None) => return Ok(()),
    };
    let uuid = app
        .extra
        .get("uuid")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    // The launch's check and update hold the switch's slot: a disable or a
    // delete meanwhile would race the update's files and key.
    if start_job(h, &uuid, "update", "place").is_err() {
        bail!(
            "Neural Rendering is busy with {}; launch it again once that is done",
            app.name
        );
    }
    // The error reaches the client as the launch's status message: it names
    // no provider path, which carries the mod's name; the log has the whole.
    let result = launch_check(h, app, &uuid, exe, legacy).map_err(|error| {
        let full = format!("{error:#}");
        tracing::warn!(app = %app.name, error = %full, "Neural Rendering's launch check failed");
        anyhow!(neural::shown(&full))
    });
    match &result {
        Ok(()) => update_job(h, &uuid, |job| job.phase = "done"),
        Err(error) => fail_job(h, &uuid, error.to_string(), false),
    }
    result
}

fn launch_check(h: &Shared, app: &App, uuid: &str, exe: PathBuf, legacy: bool) -> Result<()> {
    let _reading = h.neural.reading();
    let package = if legacy {
        None
    } else {
        launch_cache(h).context(
            "Neural Rendering's download can't be used; turn frame generation off and on again",
        )?
    };
    let paths = reached_by(app, None, &exe);
    let response = match steam_refusal(app, &paths, &steam_roots(h)) {
        Some(reason) => Response::Refused { reason },
        None => run_folder(
            Request::Prepare {
                exe,
                install: install(app),
                package,
            },
            &|_, _| {},
        )?,
    };
    match response {
        Response::Placed {
            exe,
            release,
            proxy,
            updated,
            mode,
        } => {
            if updated && !legacy {
                tracing::info!(app = %app.name, %release, %proxy, "Neural Rendering updated");
                let value = key_value(&exe, &release, &proxy, mode);
                if let Err(error) = set_key(h, uuid, KEY, Some(value)) {
                    tracing::warn!(app = %app.name, error = %format!("{error:#}"), "Neural Rendering: the updated release could not be saved with the app");
                }
            }
            Ok(())
        }
        Response::Refused { reason } => {
            tracing::warn!(app = %app.name, %reason, "Neural Rendering refused the launch");
            let what = if legacy {
                "the earlier Neural Rendering upscaling is in"
            } else {
                "frame generation is on for"
            };
            bail!(
                "{what} {}, but {reason}; remove it on the game's page",
                app.name
            )
        }
        Response::Error { message } | Response::ChooseExe { reason: message } => {
            tracing::warn!(app = %app.name, error = %message, "Neural Rendering's check failed before the launch");
            bail!("{message}")
        }
        Response::Removed { .. }
        | Response::Left { .. }
        | Response::Unreachable { .. }
        | Response::NotWritable { .. }
        | Response::Found { .. } => {
            bail!("unexpected helper answer")
        }
    }
}

// ---- The console and Nightjar ----

/// The switch as the console and Nightjar show it for one app. Blocks (the
/// first call checks the GPU).
pub fn status(h: &Shared, uuid: &str) -> Value {
    #[cfg(test)]
    let wait = h.neural.game_wait.lock().unwrap().unwrap_or(GAME_WAIT);
    #[cfg(not(test))]
    let wait = GAME_WAIT;
    status_waiting(h, uuid, wait)
}

/// Whether the host has an app `uuid`.
pub fn found(h: &Shared, uuid: &str) -> bool {
    app(h, uuid).is_some()
}

/// [`status`], waiting for the game's own check up to `wait`: a write's
/// answer waits for none.
pub fn status_waiting(h: &Shared, uuid: &str, wait: Duration) -> Value {
    let app = app(h, uuid);
    // Only what is held and System32: the game's folder, which may be on a
    // share that stalls, is looked into by the game's own check's thread.
    let gpu = gpu_capability(h, true);
    let system = system_missing(h);
    // The game's own check, only where it decides something: the switch is
    // off, nothing runs for the game, no stream (the check's helper would
    // take its disk and CPU) and the PC can run frame generation.
    let idle = !h
        .neural
        .lock()
        .jobs
        .get(uuid)
        .is_some_and(|job| !matches!(job.phase, "done" | "failed"));
    let streaming = crate::updater::streaming(h);
    let looked = app.as_ref().map_or_else(Looked::default, |app| {
        let ask = idle && !streaming && gpu.is_ok() && !app.extra.contains_key(KEY);
        game_check(h, uuid, app, &system, wait, ask, streaming)
    });
    // Until the game's folder is looked into, the runtime is not known to
    // be missing: a game may carry it beside its program.
    let missing = looked
        .beside
        .as_deref()
        .map_or_else(Vec::new, |beside| without(system, beside));
    let capability = gpu.map_err(|reason| ("gpu", reason)).and_then(|capable| {
        neural::runtime(&missing)
            .map(|()| capable)
            .map_err(|reason| ("runtime", reason))
    });
    let game = looked.check.filter(|_| {
        idle && capability.is_ok() && app.as_ref().is_some_and(|app| !app.extra.contains_key(KEY))
    });
    let state = h.neural.lock();
    let setting = app.as_ref().and_then(|app| app.extra.get(KEY)).cloned();
    let legacy = app
        .as_ref()
        .and_then(|app| app.extra.get(LEGACY_KEY))
        .cloned();
    json!({
        "accepted": accepted(h),
        "available": capability.is_ok(),
        "gpu": capability.as_ref().ok().map(|capable| &capable.gpu),
        "gpu_note": capability.as_ref().ok().and_then(neural::Capable::note),
        "reason": capability.as_ref().err().map(|(_, reason)| reason),
        // Which half failed: `gpu` or `runtime` (the Visual C++ runtime).
        "unavailable": capability.as_ref().err().map(|(why, _)| why),
        "found": app.is_some(),
        "enabled": setting.is_some(),
        "exe": setting.as_ref().and_then(|s| s.get("exe")).cloned(),
        "release": setting.as_ref().and_then(|s| s.get("release")).cloned(),
        "mode": setting.as_ref().and_then(|s| s.get("mode")).cloned(),
        "game": match game {
            Some(GameCheck::Refused { why, reason }) => json!({"why": why, "reason": reason}),
            Some(GameCheck::Choose(reason)) => json!({"why": "choose_program", "reason": reason}),
            Some(GameCheck::Fits) | None => Value::Null,
        },
        "earlier_upscaling": legacy.map(|legacy| json!({
            "exe": legacy.get("exe"),
            "release": legacy.get("release"),
        })),
        "job": state.jobs.get(uuid),
    })
}
