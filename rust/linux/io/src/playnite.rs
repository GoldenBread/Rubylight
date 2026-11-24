//! Playnite, on Linux: the program does not run here, so it is never found running or
//! installed, has no extensions folder, cannot be started, and its plugin's pipe never
//! connects. The time zone conversion for the plugin's times and the process list are real.

use anyhow::Result;
use std::{
    path::{Path, PathBuf},
    sync::mpsc,
};

/// The names of Playnite's desktop and fullscreen programs.
pub const PROCESSES: [&str; 2] = ["Playnite.DesktopApp.exe", "Playnite.FullscreenApp.exe"];

/// The processes a Playnite launch is tracked against: every process the host can list.
pub fn session_processes() -> Result<Vec<butterpollo_session::steam::Process>> {
    crate::process::processes()
}

/// The running Playnite process (id and program). Always none on Linux.
pub fn running() -> Option<(u32, PathBuf)> {
    None
}

/// The folder Playnite is installed in. Always none on Linux.
pub fn install_dir() -> Option<PathBuf> {
    None
}

/// A wall-clock time on this PC (Unix seconds as if it were UTC) as Unix seconds, by the
/// time zone's rule on that date: Playnite's times without an offset. None for a time the C
/// library cannot represent.
pub fn local_to_utc(wall: i64) -> Option<i64> {
    let wall = libc::time_t::try_from(wall).ok()?;
    // SAFETY: `tm` is plain integers and a pointer the C library sets or ignores; all zero is
    // a valid value of it.
    let mut local: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers are to locals that outlive the call; `gmtime_r` writes only to
    // `local` and keeps no pointer.
    if unsafe { libc::gmtime_r(&wall, &mut local) }.is_null() {
        return None;
    }
    // The zone's own rule decides whether daylight saving time applies on that date.
    local.tm_isdst = -1;
    // SAFETY: `local` is a valid broken-down time that outlives the call; `mktime` reads the
    // process's time zone under the C library's own lock.
    let utc = unsafe { libc::mktime(&mut local) };
    // -1 is `mktime`'s error, and also one second before 1970 UTC: only that second is lost.
    (utc != -1).then_some(utc as i64)
}

/// Playnite's program in `dir`: the desktop one, else the fullscreen one.
pub fn executable(dir: &Path) -> Option<PathBuf> {
    PROCESSES
        .iter()
        .map(|name| dir.join(name))
        .find(|exe| exe.is_file())
}

/// Where Playnite loads extensions from. Always none on Linux, where it is not installed.
pub fn extensions_dir() -> Option<PathBuf> {
    None
}

/// Start Playnite's program in the user's session. Always an error on Linux.
pub fn launch(
    _program: &Path,
    _args: &[&str],
    _environment: &std::collections::BTreeMap<String, String>,
) -> Result<()> {
    Err(crate::unsupported("Starting Playnite"))
}

/// A connection to Playnite's plugin: newline-separated JSON both ways. None can be made on
/// Linux, so no value of this type exists there.
pub struct Pipe {
    /// The lines the plugin sent.
    pub lines: mpsc::Receiver<String>,
}
impl Pipe {
    /// Connect and introduce this side with `hello`. Always an error on Linux: no plugin
    /// serves the pipe.
    pub fn connect(_hello: &serde_json::Value) -> Result<Self> {
        Err(crate::unsupported("The Playnite plugin"))
    }
    /// Send one message to the plugin. Always an error on Linux.
    pub fn send(&self, _message: &serde_json::Value) -> Result<()> {
        Err(crate::unsupported("The Playnite plugin"))
    }
}
