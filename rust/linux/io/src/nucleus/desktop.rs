//! The signed-in user's desktop as a Nucleus Co-op session uses it, on Linux: the host
//! reads and moves no window here yet. Processes are listed as the host lists them; every
//! list of windows, monitors and message boxes is empty, and every step that would read or
//! change one window is an error. The type keeps its Windows name, which the host uses.
use super::instances::{Desktop, MessageBox};
use anyhow::Result;
use butterpollo_session::nucleus::window::{Rect, Window};
use butterpollo_session::steam::Process;
use std::collections::BTreeSet;

/// The desktop of the signed-in user. It holds nothing on Linux.
#[derive(Default)]
pub struct Win32 {
    _nothing: (),
}

/// What every step that would read or change one window answers with on Linux.
fn no_windows() -> anyhow::Error {
    crate::unsupported("Reading or changing a game's window")
}

impl Desktop for Win32 {
    fn processes(&mut self) -> Result<Vec<Process>> {
        crate::process::processes()
    }
    fn image_path(&mut self, pid: u32) -> Option<String> {
        crate::process::image_path(pid)
    }
    fn windows(&mut self) -> Result<Vec<Window>> {
        Ok(Vec::new())
    }
    fn monitors(&mut self) -> Result<Vec<Rect>> {
        Ok(Vec::new())
    }
    fn virtual_screen(&mut self) -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        }
    }
    fn place(&mut self, _window: u64, _to: Rect) -> Result<()> {
        Err(no_windows())
    }
    fn unframe(&mut self, _window: u64) -> Result<()> {
        Err(no_windows())
    }
    fn unthrottle(&mut self, _processes: &BTreeSet<u32>) {}
    fn label(&mut self, _processes: &BTreeSet<u32>, _text: &str) -> Result<Option<u64>> {
        Ok(None)
    }
    fn button(&mut self, _processes: &BTreeSet<u32>, _name: &str) -> Result<Option<u64>> {
        Ok(None)
    }
    fn click(&mut self, _label: u64) -> Result<()> {
        Err(no_windows())
    }
    fn to_front(&mut self, _window: u64) -> bool {
        false
    }
    /// Never idle on Linux: whether somebody uses the PC is not read, so no window is taken
    /// to the front on that ground.
    fn pc_idle(&mut self) -> bool {
        false
    }
    fn message_boxes(&mut self, _processes: &BTreeSet<u32>) -> Result<Vec<MessageBox>> {
        Ok(Vec::new())
    }
    fn close_forms(&mut self, _processes: &BTreeSet<u32>) -> Result<Vec<MessageBox>> {
        Ok(Vec::new())
    }
    fn owner(&mut self, _window: u64) -> Result<u32> {
        Err(no_windows())
    }
    fn forced_aslr(&mut self, _pid: u32) -> Result<bool> {
        Ok(false)
    }
}
