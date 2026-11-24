//! The visible top-level windows with a title and the process each belongs to, for the host
//! to tell a launcher showing a prompt from a game. Linux has no window listing yet, so the
//! list is always empty.

/// A visible top-level window with a title.
pub struct TopWindow {
    pub pid: u32,
    pub title: String,
    /// Whether it is the window in front.
    pub foreground: bool,
    /// Whether it is minimised.
    pub minimized: bool,
    /// Whether the window has an owner, such as a dialog or popup.
    pub owned: bool,
    /// Whether it is a tool window, which the taskbar does not show.
    pub tool: bool,
    /// Whether the compositor hides the window though it counts as visible.
    pub cloaked: bool,
}

/// The visible windows that have a title. Always empty on Linux, where windows are not
/// listed yet.
pub fn visible() -> Vec<TopWindow> {
    Vec::new()
}
