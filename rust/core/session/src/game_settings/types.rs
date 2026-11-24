//! Journal, swap state, report, and error types for per-device game settings.
use super::paths::normal;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Where a swapped file is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    /// The original is kept; the device's copy may be going in. Finishing puts the original back.
    Placing,
    /// The file is the device's (its copy, or on its first launch the file as it was). Finishing
    /// saves it as the device's copy, then puts the original back.
    Playing,
    /// Saved as the device's copy; the original goes back.
    Saved,
    Done,
}

/// A file of a swap.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSwap {
    /// As the app lists it.
    pub written: String,
    /// Expanded.
    pub path: PathBuf,
    pub key: String,
    pub step: Step,
}

/// A launch's swap, until every file is back.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Swap {
    pub id: String,
    pub app: String,
    #[serde(default)]
    pub app_name: String,
    pub device: String,
    #[serde(default)]
    pub device_name: String,
    pub files: Vec<FileSwap>,
    /// The game's processes seen while it ran: the files stay while one runs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub processes: Vec<Watched>,
    /// Why the last try to finish it failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
impl Swap {
    pub(super) fn holds(&self, path: &Path) -> bool {
        let path = normal(&path.to_string_lossy());
        self.files
            .iter()
            .any(|file| file.step != Step::Done && normal(&file.path.to_string_lossy()) == path)
    }
    pub(super) fn who(&self) -> String {
        let device = if self.device_name.is_empty() {
            &self.device
        } else {
            &self.device_name
        };
        format!("{} from {device}", self.app_name)
    }
}

/// The swaps not finished yet.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Journal {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub swaps: Vec<Swap>,
}

/// What a launch or a finish did, by file as listed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// The device's copy went in.
    pub placed: Vec<String>,
    /// The device had no copy: the file stays and becomes its copy at the end.
    pub first: Vec<String>,
    /// Saved as the device's copy.
    pub saved: Vec<String>,
    /// The original is back.
    pub restored: Vec<String>,
    /// Done, but worth saying loudly (a file the game removed).
    pub warnings: Vec<String>,
    /// The game was never seen, so its close could not be waited for: why, for the console.
    pub unwatched: Option<String>,
}

/// A launch's swap, begun.
#[derive(Debug)]
pub struct Begun {
    pub id: String,
    pub report: Report,
}

/// A process of a swap's game.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Watched {
    pub pid: u32,
    /// Its creation time, which tells a later process with the same id apart.
    pub started: u64,
    /// Its program, for messages.
    pub program: String,
}

/// Which of a swap's processes still runs, if one does: its program, or why that is not known.
pub type Running = Box<dyn Fn(&[Watched]) -> Option<String> + Send + Sync>;

/// Originals put back that stay as backups, newest first: `original/<key>`, then `.1`, `.2`.
pub const BACKUPS: usize = 3;

/// How a swap ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mode {
    /// Its app stopped: the files are saved as the device's copies.
    Finish,
    /// Its launch failed before the game ran: only the originals go back.
    Cancel,
    /// The user puts the files back although the game may still run.
    Release,
}

/// Marks a finish refused because the swap's game still runs.
#[derive(Debug)]
pub struct StillRunning;
impl std::fmt::Display for StillRunning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the game still runs")
    }
}
impl std::error::Error for StillRunning {}
/// Whether a finish was refused because the swap's game still runs.
pub fn still_running(error: &anyhow::Error) -> bool {
    error.downcast_ref::<StillRunning>().is_some()
}

/// A [`Store::read_game`] error: the message, and whether the file does not exist.
#[derive(Debug)]
pub(super) struct Unread {
    pub(super) missing: bool,
    pub(super) message: String,
}
impl std::fmt::Display for Unread {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for Unread {}
pub(super) fn is_missing(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<Unread>()
        .is_some_and(|unread| unread.missing)
}
