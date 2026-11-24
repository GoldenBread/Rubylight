//! Keeps a background stream's game windows on the stream's display without activating them.
//! Linux cannot move or address another program's windows yet, so a pass finds no window and
//! moves none.
use butterpollo_session::background::Rect;
use std::collections::BTreeSet;

/// Moves a background stream's game windows, one [`Keeper::pass`] at a time. Holds nothing on
/// Linux.
#[derive(Default)]
pub struct Keeper {
    _kept: (),
}

/// What one pass did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Pass {
    /// Visible windows of the game, wherever they are.
    pub windows: usize,
    pub moved: usize,
    /// Windows not yet observed on the target, including moves still queued.
    pub pending: usize,
    /// Windows that kept leaving the stream's display and are left alone.
    pub given_up: usize,
    /// The game's largest window already on the target and its process.
    pub main: Option<(isize, u32)>,
    /// The game held a cursor clip that reached the stream's display, which
    /// would trap the PC user's mouse; it was released.
    pub released_clip: bool,
}

impl Keeper {
    /// Move every visible top-level window of `processes` that is off `target` onto it.
    /// On Linux no window is found, so the pass reports none and moves none.
    pub fn pass(&mut self, processes: &BTreeSet<u32>, target: Rect) -> Pass {
        let _ = (processes, target);
        Pass::default()
    }
}

/// Tell `window` it is the active window, though another program keeps the real focus.
/// Does nothing on Linux: [`Keeper::pass`] never names a window to tell.
pub fn fake_focus(window: isize, processes: &BTreeSet<u32>, target: Rect) {
    let _ = (window, processes, target);
}
