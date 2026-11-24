//! What a Nucleus Co-op session reads and changes on the desktop, on Linux: the seam
//! ([`Desktop`]) and a message box's description are as on Windows. No session runs here,
//! so nothing finds or parks a game instance's window.
use anyhow::Result;
use butterpollo_session::nucleus::window::{Rect, Window};
use butterpollo_session::steam::Process;
use std::collections::BTreeSet;

/// What a session reads and changes on the desktop: the user's desktop on the PC, fakes in
/// tests.
pub trait Desktop: Send {
    fn processes(&mut self) -> Result<Vec<Process>>;
    fn image_path(&mut self, pid: u32) -> Option<String>;
    /// Every top-level window.
    fn windows(&mut self) -> Result<Vec<Window>>;
    fn monitors(&mut self) -> Result<Vec<Rect>>;
    /// The virtual screen: the rectangle around every monitor.
    fn virtual_screen(&mut self) -> Rect;
    /// Move and size a window without activating it.
    fn place(&mut self, window: u64, to: Rect) -> Result<()>;
    /// Take away a window's title bar and sizing border, so its stream shows only the
    /// game.
    fn unframe(&mut self, window: u64) -> Result<()>;
    /// Keep the system from throttling these background processes; the ones no longer
    /// listed get their settings back.
    fn unthrottle(&mut self, processes: &BTreeSet<u32>);
    /// The visible child window with this text in a window of `processes`: the profile's
    /// entry in Nucleus' list. An error when several match.
    fn label(&mut self, processes: &BTreeSet<u32>, text: &str) -> Result<Option<u64>>;
    /// The visible button a window of `processes` calls `name` in its program: Nucleus'
    /// Play shows only a picture.
    fn button(&mut self, processes: &BTreeSet<u32>, name: &str) -> Result<Option<u64>>;
    /// Click it as the mouse would; refused while another window covers it.
    fn click(&mut self, label: u64) -> Result<()>;
    /// Bring a window to the front, activated: whether it is in front.
    fn to_front(&mut self, window: u64) -> bool;
    /// Whether nobody uses the PC: no local input for a while, or only the desktop or
    /// taskbar in front. A window taken to the front then takes nobody's focus.
    fn pc_idle(&mut self) -> bool;
    /// The visible message boxes of `processes`, and those the system shows for any
    /// program: with no processes, those alone.
    fn message_boxes(&mut self, processes: &BTreeSet<u32>) -> Result<Vec<MessageBox>>;
    /// Close the modal form of `processes`, as its close button does. The ones closed,
    /// with what they said.
    fn close_forms(&mut self, processes: &BTreeSet<u32>) -> Result<Vec<MessageBox>>;
    /// The process showing `window`.
    fn owner(&mut self, window: u64) -> Result<u32>;
    /// Whether the system forces process `pid` to load its images at random addresses
    /// (Windows' Mandatory ASLR).
    fn forced_aslr(&mut self, pid: u32) -> Result<bool>;
}

/// A message box on the desktop: its window, its caption and its text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageBox {
    pub window: u64,
    pub caption: String,
    pub text: String,
}

impl MessageBox {
    /// What it says on one line, as a failure quotes it: `caption: text`, spaces collapsed
    /// and cut to a few sentences.
    pub fn summary(&self) -> String {
        const LIMIT: usize = 300;
        let plain = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
        let (caption, text) = (plain(&self.caption), plain(&self.text));
        let line = match (caption.is_empty(), text.is_empty()) {
            (false, false) => format!("{caption}: {text}"),
            (true, _) => text,
            (false, true) => caption,
        };
        match line.char_indices().nth(LIMIT) {
            Some((at, _)) => format!("{}...", line[..at].trim_end()),
            None => line,
        }
    }
}
