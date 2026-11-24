//! Per-application audio routing on Linux: no program's sound can be moved to another device
//! yet, so a route cannot be made, no program is found playing elsewhere, and there is
//! nothing to put back.

use anyhow::Result;
use butterpollo_config::config::Config;
use std::path::Path;

/// The process hasn't opened its sound yet, so there is no device for it: try again once it
/// has ([`no_sound_yet`]).
#[derive(Debug)]
pub struct NoSoundYet;
impl std::fmt::Display for NoSoundYet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the game has not opened its sound yet")
    }
}
impl std::error::Error for NoSoundYet {}

/// Whether `error` is [`NoSoundYet`]: the route is tried again later.
pub fn no_sound_yet(error: &anyhow::Error) -> bool {
    error.is::<NoSoundYet>()
}

/// Where the game's sound reaches the PC ([`AppRoute::plays_elsewhere`]).
#[derive(Debug, Default, PartialEq)]
pub struct OnThePc {
    /// The game's processes playing there.
    pub pids: Vec<u32>,
    /// One plays on the PC's default communications device, which may be the game's voice
    /// chat, which stays on it.
    pub maybe_voice: bool,
}

/// The game's process played on a virtual sink while this lives. None exists on Linux.
pub struct AppRoute {
    exe: String,
    /// The sink's name for messages.
    pub sink: String,
}
impl AppRoute {
    /// Play process `pid` on a virtual sink. Fails on Linux.
    pub fn acquire(_config: &Config, _directory: &Path, _pid: u32) -> Result<Self> {
        Err(crate::unsupported("Per-application audio routing"))
    }
    /// The routed program's path, in lower case.
    pub fn program(&self) -> &str {
        &self.exe
    }
    /// The device the user chose for the game, which it stays on. None: no route moves it.
    pub fn users_device(&self) -> Option<&str> {
        None
    }
    /// Which of these routes' processes, and of the game's processes left `unrouted`, play
    /// where the PC plays the game. None do on Linux.
    pub fn plays_elsewhere(_routes: &[&AppRoute], _unrouted: &[u32]) -> Result<OnThePc> {
        Ok(OnThePc::default())
    }
}

/// Restore what an interrupted stream left. Nothing is left on Linux.
pub fn recover(_directory: &Path) -> Result<()> {
    Ok(())
}

/// Programs the host stops: routed again once this drops. It holds nothing on Linux.
pub struct Closing(());

/// The host is about to stop the processes `pids` gives: their programs go back to their own
/// devices. Nothing was moved on Linux, so there is nothing to put back.
pub fn put_back(_directory: &Path, _pids: impl FnOnce() -> Result<Vec<u32>>) -> Result<Closing> {
    Ok(Closing(()))
}
