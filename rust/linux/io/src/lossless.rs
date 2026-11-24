//! Lossless Scaling, on Linux: the program does not run here, so it is never found, it has no
//! settings file, and the window and keyboard steps that start scaling a game do nothing.
//! Only the processor count the game scoring uses is read for real.

use std::collections::BTreeSet;
use std::path::PathBuf;

/// The names Lossless Scaling's process has had.
pub const PROCESSES: [&str; 2] = ["LosslessScaling.exe", "Lossless Scaling.exe"];

/// Inputs to the game scoring: the number of processors, and the system folder whose
/// processes it penalizes. Linux has no such folder, so none is given.
pub fn scoring_environment() -> (u32, Option<String>) {
    let processors = std::thread::available_parallelism()
        .ok()
        .and_then(|count| u32::try_from(count.get()).ok())
        .unwrap_or(1);
    (processors.max(1), None)
}

/// The Lossless Scaling program. Always none on Linux, where it is not installed, also for a
/// configured path.
pub fn program(_configured: &str) -> Option<PathBuf> {
    None
}

/// The signed-in user's Lossless Scaling settings file. None on Linux.
pub fn settings_path() -> Option<PathBuf> {
    None
}

/// Visible top-level windows of `pid`, by window id. The host reads no windows on Linux yet,
/// so the list is empty.
pub fn windows_of(_pid: u32) -> Vec<isize> {
    vec![]
}

/// Minimize `pid`'s windows without activating them. Does nothing on Linux.
pub fn minimize(_pid: u32) {}

/// The top-level windows minimised now, by window id. Empty on Linux.
pub fn minimized_windows() -> BTreeSet<isize> {
    BTreeSet::new()
}

/// Bring back `pid`'s windows that a display's departure minimised; true when there was one.
/// Nothing is brought back on Linux, so this is false.
pub fn restore_minimized(_pid: u32, _before: &BTreeSet<isize>) -> bool {
    false
}

/// Bring `pid`'s main window to the front; true when it is in front. Always false on Linux,
/// where the host moves no window yet.
pub fn focus(_pid: u32) -> bool {
    false
}

/// Bring `pid`'s minimised or main titled window to the front without a key tap; true when
/// it is in front. Always false on Linux.
pub fn focus_quietly(_pid: u32) -> bool {
    false
}

/// Bring the window with the id `window` to the front without a key tap; true when it is in
/// front. Always false on Linux.
pub fn to_front_quietly(_window: isize) -> bool {
    false
}

/// Press `modifiers` and `code`, then release them in reverse; true when every key was sent.
/// No key is sent on Linux, so this is false.
pub fn press(_modifiers: &[u16], _code: u16) -> bool {
    false
}
