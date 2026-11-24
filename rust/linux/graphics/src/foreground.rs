//! The visible window stack and the foreground window's process. Linux has no window inspection yet: no window is ever reported, selected or found in front.

use crate::capture::Display;
use butterpollo_media_policy::capture_policy::InFront;

/// Follows the app window in front of a display for the encode thread. On Linux it scans
/// nothing and never has a window to report.
#[derive(Default)]
pub struct Tracker(());
impl Tracker {
    /// The program of the window selected on `display`: always none on Linux.
    pub fn poll(&self, owned: &[u32], display: &Display) -> Option<String> {
        self.poll_process(owned, display).map(|(exe, _)| exe)
    }
    /// The program and process of the selected window: always none on Linux.
    pub fn poll_process(&self, _owned: &[u32], _display: &Display) -> Option<(String, u32)> {
        None
    }
}
/// The process of the foreground window: unknown on Linux.
pub fn process() -> Option<u32> {
    None
}
/// The process of the active app window in the foreground: unknown on Linux.
pub fn active_process() -> Option<u32> {
    None
}
/// What sits in front of `display` right now: nothing that Linux can see yet.
pub fn in_front(_display: &str) -> InFront {
    InFront::Nothing
}
/// Whether an app presents in exclusive fullscreen. Never on Linux, where the compositor
/// presents every window.
pub fn exclusive_fullscreen() -> bool {
    false
}
