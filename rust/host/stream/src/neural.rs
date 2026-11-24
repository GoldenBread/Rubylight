//! Neural Rendering: a per-app switch that puts frame generation into the
//! game's folder and takes it out again.
//!
//! It is ANR FG by Apex Compute (https://github.com/apex-compute/ANR, no
//! licence published), a DLSS frame generation provider that OptiScaler
//! (GPL-3.0) loads in place of the game's own. Rubylight never hosts,
//! bundles, mirrors or modifies its files: when the user turns the switch
//! on, this PC downloads the release Rubylight pins
//! ([`butterpollo_session::neural::PINNED`]): its full package and add-on,
//! straight from that GitHub release ([`butterpollo_session::neural::asset`]
//! says which), checks them against the pin, GitHub's digests and the
//! release's SHA256SUMS.txt, keeps them in a cache and copies the files
//! unmodified into the game's folder ([`folder`]). A launch checks them and
//! finishes an update cut short; no other release is ever taken on its own.
//!
//! Rubylight 3.0 offered AMDNR (neural upscaling) here instead. Its
//! installs stay until the user removes them ([`LEGACY_KEY`]); turning frame
//! generation on for such a game takes AMDNR out first.

mod cache;
pub mod folder;
mod game_check;
mod install;
mod launch;
mod pending;
mod probe;
mod process;
mod record;
mod stall;
mod status;

pub use butterpollo_session::neural::{
    ALREADY_ON, DOWNLOAD_FAILED, GameCheck, Job, KEY, LEGACY_KEY, Pending, Probe, Refusal, Removed,
    SETTINGS_FILE,
};
pub use cache::{
    cache_root, cached, clear_legacy_cache, data_root, download, extract, fetch_locked,
    fetch_using, http, launch_cache, legacy_cache_root, pinned, serves, trusted_root,
    trusted_root_with, verified, verify_cache,
};
pub use game_check::{
    app, find_game, game_command, install, keep_key, legacy_exe, placed_exe, placed_exes, set_key,
    steam_refusal, steam_roots,
};
#[cfg(any(test, feature = "test-support"))]
pub use install::before_delete;
pub use install::{
    Removing, disable, enable, fail_job, finish_install, removal, remove, remove_earlier, reserve,
    running, start_job, update_job,
};
pub use launch::before_launch;
pub use pending::{
    clear_pending, dropped, pending, record_pending, removal_due, removal_due_at, remove_dropped,
    remove_pending, try_pending,
};
pub use probe::{capability, offered, probe_main, vc_missing, without};
pub use process::as_system;
pub use record::{accept, accepted, now, settings};
#[cfg(any(test, feature = "test-support"))]
pub use stall::Stall;
pub use stall::Using;
pub use status::{found, status, status_waiting};

use butterpollo_session::{neural, state::App};
use butterpollo_win_io::process::Hold;
use probe::GpuSetting;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{Condvar, Mutex, PoisonError, RwLock},
    time::{Duration, Instant},
};

/// The hidden flag that makes the host exe report what the compute runtime sees.
#[cfg_attr(
    any(test, feature = "test-support"),
    allow(dead_code, reason = "tests answer for the probe")
)]
const PROBE_FLAG: &str = "--neural-probe";
const PROBE_ANSWER: &str = "NEURAL-PROBE ";
/// The record's acceptance of this release's notice. Rubylight 3.0's
/// `accepted_at` was AMDNR's and does not count.
const ACCEPTED: &str = "anr_accepted_at";
/// A failed probe is asked again after this; a driver update needs no restart.
pub const PROBE_RETRY: Duration = Duration::from_secs(300);
/// The record's list of removals still to do.
const PENDING: &str = "pending_removals";
/// How long a game's own check ([`game_check::game_check`]) holds.
pub const GAME_RECHECK: Duration = Duration::from_secs(120);
/// How long a status read waits for a game's own check; one that takes
/// longer (a network share that stalls) answers the next read.
pub const GAME_WAIT: Duration = Duration::from_secs(if cfg!(any(test, feature = "test-support")) {
    30
} else {
    3
});
/// How often a pending removal is tried again.
const PENDING_RETRY: Duration = Duration::from_secs(600);

#[derive(Default)]
pub struct Neural {
    state: Mutex<State>,
    /// Told when a game's own check ends ([`game_check::game_check`]).
    checked: Condvar,
    /// One download into the cache at a time.
    pub fetching: tokio::sync::Mutex<()>,
    /// One writer of the record at a time.
    record: Mutex<()>,
    /// One pass over the pending removals at a time.
    removing: Mutex<()>,
    /// Copies out of the cache hold it to read; replacing or clearing a
    /// release in the cache holds it to write.
    cache: RwLock<()>,
    #[cfg(any(test, feature = "test-support"))]
    pub fake_hardware: std::sync::atomic::AtomicBool,
    #[cfg(any(test, feature = "test-support"))]
    pub probe: Mutex<Option<Probe>>,
    /// A test's streaming GPU in place of this PC's.
    #[cfg(any(test, feature = "test-support"))]
    pub streams: Mutex<Option<neural::Adapter>>,
    /// A test's GPUs in place of this PC's; the streaming GPU alone if none.
    #[cfg(any(test, feature = "test-support"))]
    pub adapters: Mutex<Option<Vec<neural::Adapter>>>,
    /// A test's missing Visual C++ runtime files in place of this PC's.
    #[cfg(any(test, feature = "test-support"))]
    pub vc_missing: Mutex<Vec<&'static str>>,
    /// A test's server in place of GitHub.
    #[cfg(any(test, feature = "test-support"))]
    pub origin: Mutex<Option<String>>,
    /// A test's release in place of [`neural::PINNED`].
    #[cfg(any(test, feature = "test-support"))]
    pub pin: Mutex<Option<neural::Pin<'static>>>,
    /// A test's Steam folders in place of this PC's.
    #[cfg(any(test, feature = "test-support"))]
    pub steam_roots: Mutex<Option<Vec<PathBuf>>>,
    /// The GPU a test streams with can't be found.
    #[cfg(any(test, feature = "test-support"))]
    pub streams_unknown: Mutex<bool>,
    /// A game's own check waits for this test to send or hang up.
    #[cfg(any(test, feature = "test-support"))]
    pub game_gate: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    /// How many games' own checks ran in this test.
    #[cfg(any(test, feature = "test-support"))]
    pub game_checks: std::sync::atomic::AtomicUsize,
    /// A test's wait for a game's own check in place of [`GAME_WAIT`].
    #[cfg(any(test, feature = "test-support"))]
    pub game_wait: Mutex<Option<Duration>>,
    /// The GPU check waits for this test to send or hang up, once.
    #[cfg(any(test, feature = "test-support"))]
    pub probe_gate: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
}

#[derive(Default)]
pub struct State {
    /// The last GPU check's answer, when it came, the settings it holds for,
    /// and whether it held the GPU that streams.
    pub probe: Option<(Instant, GpuSetting, Result<neural::Capable, String>, bool)>,
    /// A GPU check asks again in the background while a read is answered
    /// with the last one.
    pub probing: bool,
    /// Whether one of the PC's GPUs is AMD's or NVIDIA's ([`offered`]),
    /// and the settings it holds for.
    pub offered: Option<(GpuSetting, bool)>,
    pub jobs: BTreeMap<String, Job>,
    /// Each app's game folder as [`game_check::game_check`] last looked: when,
    /// for which record, and the Visual C++ runtime files beside the game.
    pub lookups: BTreeMap<String, Lookup>,
    /// Each app's own check ([`game_check::game_check`]): when, for which
    /// record, and what it found (None: the check itself failed).
    pub games: BTreeMap<String, GameChecked>,
    /// The apps whose own check runs now.
    pub checking: BTreeSet<String>,
    /// Each app's count of job changes: a check that began before one is
    /// not kept.
    pub generation: BTreeMap<String, u64>,
    /// When the pending removals are tried next.
    pub removal_at: Option<Instant>,
    /// The cached release this host start checked file by file.
    pub verified: Option<PathBuf>,
    /// Cached releases an install is about to copy, and how many: another
    /// release fetched meanwhile leaves them to the next fetch to clear.
    pub using: BTreeMap<PathBuf, usize>,
}

pub type GameChecked = (Instant, GameKey, Option<GameCheck>);

/// What of an app's record a game's own check depends on, all read without
/// touching the disk: its command's program, its install, and the programs
/// the files sit beside now and sat beside in Rubylight 3.0.
pub type GameKey = [Option<PathBuf>; 4];

pub(self) fn game_key(app: &App) -> GameKey {
    [
        game_check::command_exe(app),
        install(app),
        legacy_exe(app),
        placed_exe(app),
    ]
}

/// A look at an app's game folder ([`game_check::game_check`]).
pub struct Lookup {
    pub at: Instant,
    pub key: GameKey,
    /// The Visual C++ runtime files beside the game's program.
    pub beside: Vec<&'static str>,
}

/// What a status read holds of an app's game folder ([`game_check::game_check`]).
#[derive(Default)]
pub(self) struct Looked {
    /// The runtime files beside the game's program, once looked for.
    pub beside: Option<Vec<&'static str>>,
    pub check: Option<GameCheck>,
}

impl Neural {
    pub fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
    pub fn reading(&self) -> std::sync::RwLockReadGuard<'_, ()> {
        self.cache.read().unwrap_or_else(PoisonError::into_inner)
    }
    fn writing(&self) -> std::sync::RwLockWriteGuard<'_, ()> {
        self.cache.write().unwrap_or_else(PoisonError::into_inner)
    }
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
