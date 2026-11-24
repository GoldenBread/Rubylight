//! Playback of a client's microphone on Linux: there is no virtual microphone device yet, so
//! opening a render always fails and the host forwards no microphone.

use anyhow::Result;
use butterpollo_session::mic::{Conceal, Playout};
use std::{sync::Mutex, time::Duration};

/// The host end of a client's microphone, played into a virtual microphone's endpoint. It
/// cannot be opened on Linux.
pub struct Render(());

impl Render {
    /// Opens the endpoint `id` and starts it playing silence. Fails on Linux.
    pub fn open(_id: &str) -> Result<Self> {
        Err(crate::unsupported("Microphone playback"))
    }
    /// Frames played per wake-up.
    pub fn period(&self) -> usize {
        0
    }
    /// Waits for the device to want more, up to `timeout`.
    pub fn wait(&self, timeout: Duration) {
        std::thread::sleep(timeout.min(Duration::from_secs(1)));
    }
    /// Moves what the device has room for from `playout`; returns the samples written.
    pub fn fill(&mut self, _playout: &mut Playout) -> Result<usize> {
        Ok(0)
    }
    /// [`Self::fill`] from a playout shared with the thread that feeds it, with `conceal`
    /// filling a dry spell.
    pub fn fill_concealing(
        &mut self,
        _playout: &Mutex<Playout>,
        _conceal: &mut Conceal,
    ) -> Result<usize> {
        Ok(0)
    }
}
