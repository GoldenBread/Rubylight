//! Playback of a client's microphone on Linux, into a PipeWire source named "Rubylight
//! Microphone" that games and chat apps record from. The source is one `Audio/Source` stream
//! node (`mic/device.rs`) made the first time a microphone is set up and kept until the host
//! exits, as the Steam driver's device is on Windows. [`Render`] is its host end: the render
//! thread keeps the node's ring two periods ahead (a period is the graph's quantum), and the
//! node's process callback wakes it once per cycle, so the node adds the queue's two periods
//! (10 to 40 ms by the quantum) to what the playout holds.

use anyhow::{Result, bail};
use butterpollo_session::mic::{Conceal, Playout};
use std::{
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

mod device;
pub(crate) mod graph;
#[cfg(test)]
mod tests;

pub(crate) use device::{Device as Handle, NODE_NAME};

/// The shortest period, in frames (5 ms), whatever the graph's quantum.
const MIN_PERIOD: usize = 240;

static DEVICE: Mutex<Option<Arc<device::Device>>> = Mutex::new(None);

/// The virtual microphone's node, made now when it does not exist (or ended).
pub(crate) fn device() -> Result<Arc<device::Device>> {
    let mut slot = DEVICE.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(device) = slot.as_ref().filter(|device| device.alive()) {
        return Ok(device.clone());
    }
    // A dead node's thread is joined here, before its replacement takes the name.
    *slot = None;
    let device = Arc::new(device::Device::open()?);
    *slot = Some(device.clone());
    Ok(device)
}

/// The node when it exists and runs.
pub(crate) fn existing_device() -> Option<Arc<device::Device>> {
    let slot = DEVICE.lock().unwrap_or_else(PoisonError::into_inner);
    slot.as_ref().filter(|device| device.alive()).cloned()
}

/// The host end of a client's microphone, played into the virtual microphone's source node.
pub struct Render {
    device: Arc<device::Device>,
    scratch: Vec<f32>,
}

impl Render {
    /// Opens the virtual microphone `id` (its node name; any other id is refused) and starts
    /// it playing silence.
    pub fn open(id: &str) -> Result<Self> {
        if id != NODE_NAME {
            bail!("no virtual microphone named {id:?}");
        }
        let device = device()?;
        device.shared.ring().clear();
        let mut render = Self {
            device,
            scratch: Vec::new(),
        };
        // A period of silence so the first cycle has audio.
        render.fill(&mut Playout::default())?;
        Ok(render)
    }
    /// Frames played per wake-up: the graph's quantum.
    pub fn period(&self) -> usize {
        self.device.shared.quantum().max(MIN_PERIOD)
    }
    /// Waits for the device to want more, up to `timeout`.
    pub fn wait(&self, timeout: Duration) {
        self.device
            .shared
            .wait(timeout.clamp(Duration::from_millis(1), Duration::from_secs(1)));
    }
    /// Moves what the device has room for from `playout`, adding silence only when it would
    /// otherwise run dry. Returns the samples written.
    pub fn fill(&mut self, playout: &mut Playout) -> Result<usize> {
        self.write(|buffer, padding, period, out| playout.pull(buffer, padding, period, out))
    }
    /// [`Self::fill`] from a playout shared with the thread that feeds it, with `conceal`
    /// filling a dry spell instead of silence. The playout is locked for the pull only.
    pub fn fill_concealing(
        &mut self,
        playout: &Mutex<Playout>,
        conceal: &mut Conceal,
    ) -> Result<usize> {
        self.write(|buffer, padding, period, out| {
            let mut playout = playout.lock().unwrap_or_else(PoisonError::into_inner);
            playout.pull_concealing(buffer, padding, period, out, conceal)
        })
    }
    /// One wake-up: `pull` gets the buffer size, the ring's padding and the period, and the
    /// scratch to fill, and returns the silence to add after it.
    fn write(
        &mut self,
        pull: impl FnOnce(usize, usize, usize, &mut Vec<f32>) -> usize,
    ) -> Result<usize> {
        if let Some(error) = self.device.shared.failure() {
            bail!("the Rubylight Microphone node ended: {error}");
        }
        let period = self.period();
        let buffer = (4 * period).min(device::RING_CAP);
        let mut ring = self.device.shared.ring();
        let silence = pull(buffer, ring.len(), period, &mut self.scratch);
        ring.extend(self.scratch.iter().copied());
        ring.extend(std::iter::repeat_n(0., silence));
        Ok(self.scratch.len())
    }
}
