//! A Nucleus Co-op session, on Linux: none starts, since Nucleus Co-op is a Windows program
//! and none of the pads, windows and settings it needs exist here. Its window is never
//! opened and it is never found running. What starts a session ([`Start`]), what a session
//! tells about its instances ([`Told`]) and the texts the host shows are as on Windows.
use super::pads::Dropped;
use crate::input::PadReport;
use anyhow::Result;
use butterpollo_input_policy::input::Input;
use butterpollo_session::audio_scope::Identity;
use butterpollo_session::nucleus::coop;
use butterpollo_session::nucleus::elevation::{Account, Refused, Run};
use butterpollo_session::nucleus::handler::Handler;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

/// How long Nucleus may take to show the instances, counted again from each step it makes.
pub const START_TIMEOUT: Duration = Duration::from_secs(60);
/// The Windows build process loopback capture of one process tree came with.
pub const PROCESS_AUDIO_BUILD: u32 = 20348;
/// The stream's warning for a game Rubylight's handler starts without ProtoInput's hooks.
pub const UNHOOKED: &str = "nucleus_unhooked";

/// What every step of a session answers with on Linux.
fn no_session() -> anyhow::Error {
    crate::unsupported("A Nucleus Co-op session")
}

/// What Nucleus Co-op starts with for a game found from Steam app `steam_app`: Steam's own
/// launch sets these.
pub fn steam_environment(steam_app: Option<u32>) -> BTreeMap<String, String> {
    steam_app
        .into_iter()
        .flat_map(|app| ["SteamAppId", "SteamGameId"].map(|key| (key.to_string(), app.to_string())))
        .collect()
}

/// Why per-player audio cannot be captured on the Windows build `build`. An unknown build,
/// as on Linux, is taken as new enough.
pub fn audio_limit(build: Option<u32>) -> Option<String> {
    let build = build?;
    (build < PROCESS_AUDIO_BUILD).then(|| {
        format!(
            "Per-player audio needs Windows build {PROCESS_AUDIO_BUILD} or newer and this PC runs build {build}: update Windows to hear each player's own game"
        )
    })
}

/// What a session tells about its instances as it runs.
pub trait Told: Send + Sync {
    /// Each slot's instance processes (its root and every process it started) after every
    /// pass, from the pass that finds the instance on.
    fn found(&self, trees: &[BTreeSet<Identity>]);
    /// The instances' processes `pids`, still running, are about to be stopped with the
    /// session.
    fn stopping(&self, pids: &[u32]);
    /// ...and were stopped.
    fn stopped(&self);
}
pub type Instances = Arc<dyn Told>;

/// What starts a session.
pub struct Start<'a> {
    pub id: u32,
    pub kind: coop::Kind,
    /// The host's Nucleus Co-op folder.
    pub folder: &'a Path,
    /// The signed-in user's own copy ([`super::user_copy`]) when the session runs it as
    /// them without administrator rights; None runs `folder` elevated.
    pub user_copy: Option<&'a Path>,
    pub handler: &'a Handler,
    /// The game's .exe as Nucleus starts it: the user's added game's, or the one
    /// Rubylight's handler is for.
    pub exe_path: &'a str,
    /// The Steam app the game was found from, for Rubylight's own handler.
    pub steam_app: Option<u32>,
    /// The game as the stream names it.
    pub title: &'a str,
    pub players: usize,
    /// The first player's device id and name.
    pub owner: (&'a str, &'a str),
    /// The first player's stream size.
    pub size: (u32, u32),
    /// Told the instances as the session finds them.
    pub instances: Option<Instances>,
}

/// A running session. None starts on Linux, so no value of this type exists there; its
/// calls answer as a session without instances would.
pub struct Running {
    session: Mutex<coop::Session>,
}

/// Start a session on the PC. Always an error on Linux: no pad is plugged and Nucleus
/// Co-op is not started.
pub fn start_on_pc(
    _start: &Start,
    _policy: butterpollo_input_policy::input_policy::Policy,
    _warnings: Arc<butterpollo_session::session::Warnings>,
) -> Result<Running> {
    Err(no_session())
}

/// What starting a session needs that doesn't need Nucleus Co-op closed. Always an error
/// on Linux, where a session cannot start.
pub fn ready(_folder: &Path, _run: Run) -> Result<()> {
    Err(no_session())
}

impl Running {
    /// Read or change the session: players joining, leaving, kicked, ended.
    pub fn with_session<R>(&self, f: impl FnOnce(&mut coop::Session) -> R) -> R {
        f(&mut self.session.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Each slot's game window while the session runs.
    pub fn windows(&self) -> Vec<Option<u64>> {
        self.with_session(|session| session.slots.iter().map(|s| s.window).collect())
    }

    /// Slot `slot`'s capture target on the GPU of `display`. Always an error on Linux.
    pub fn capture_target(&self, _slot: usize, _display: &str) -> Result<String> {
        Err(no_session())
    }

    /// Park slot `slot`'s window at its player's stream size. Always an error on Linux.
    pub fn resize(&self, _slot: usize, _size: (u32, u32)) -> Result<()> {
        Err(no_session())
    }

    /// A device's input, for its slot's pad. No pad exists on Linux: the event is dropped.
    pub fn input(&self, _device: &str, _event: Input) -> Result<(), Dropped> {
        Err(Dropped::Ignored)
    }

    /// Rumble and other feedback since the last call, per slot. None on Linux.
    pub fn feedback(&self) -> Vec<(usize, PadReport)> {
        Vec::new()
    }

    /// Slot `slot`'s instance, for its audio. Always an error on Linux.
    pub fn audio(&self, _slot: usize) -> Result<crate::process::AudioProcess> {
        Err(no_session())
    }

    /// End the session. Nothing runs on Linux, so there is nothing to end.
    pub fn stop(&self) {}
}

/// Open Nucleus Co-op's own window on the PC's screen for the user. Always an error on
/// Linux.
pub fn open(_folder: &Path) -> Result<Opened> {
    Err(crate::unsupported("Opening Nucleus Co-op"))
}

fn refusal(account: Account, refused: Refused) -> String {
    format!(
        "Nucleus Co-op needs administrator rights, and {}",
        account.no_elevation(refused)
    )
}

/// What [`open`] refuses with on Windows when the signed-in `account` has no elevated
/// token, with what to do or what still runs.
pub fn needs_administrator(account: Account) -> String {
    refusal(account, Refused::Open)
}

/// The Nucleus Co-op [`open`] started. None is started on Linux, so no value of this type
/// exists there.
pub struct Opened(());
impl Opened {
    /// The process [`open`] started.
    pub fn pid(&self) -> u32 {
        0
    }
    /// Whether it still runs.
    pub fn running(&self) -> Result<bool> {
        Ok(false)
    }
    /// Ask its windows to close and wait up to `timeout`. Nothing is open on Linux.
    pub fn close(&self, _timeout: Duration) -> Result<()> {
        Ok(())
    }
}

/// A Nucleus Co-op process running on the PC, whoever started it. Always none on Linux,
/// where it does not run.
pub fn running() -> Result<Option<u32>> {
    Ok(None)
}

/// Why a session does not start while Nucleus Co-op process `pid` runs.
pub fn already_running(pid: u32) -> String {
    format!(
        "Nucleus Co-op is already running on the PC (process {pid}): close it, then start the stream again"
    )
}
