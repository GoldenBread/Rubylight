//! One client's virtual gamepads and what they hand back to the control stream. Linux has no
//! virtual gamepad backend yet: a [`GamepadThread`] plugs in no pad, drops the controller
//! input it is sent and says so in the stream's warnings, and never has feedback to report.
use super::Event;
use anyhow::Result;
use butterpollo_input_policy::haptics;
use std::{
    sync::{Arc, Mutex, PoisonError},
    thread::JoinHandle,
    time::Duration,
};

/// The warning for controllers that cannot be had, as when a host has no gamepad driver.
const UNAVAILABLE: &str = "input_gamepad";

/// What the gamepad thread hands back to the control stream.
#[derive(Debug, PartialEq)]
pub enum PadReport {
    Haptics(haptics::Frame),
    Speaker(haptics::Frame),
    /// New rumble, lights or trigger effects for a controller.
    Feedback {
        id: u16,
        kind: u16,
        data: Vec<u8>,
    },
    /// A controller arrived on a profile with motion sensors; the client's
    /// arrival capabilities say which sensors to ask for.
    Motion {
        id: u8,
        capabilities: u16,
    },
}

/// One client's virtual gamepads. On Linux it runs no thread and holds no pad: it only keeps
/// the stream's warnings, to say that controller input is ignored.
pub struct GamepadThread {
    warnings: Mutex<Arc<butterpollo_session::session::Warnings>>,
}

impl GamepadThread {
    /// The controllers of one client with the `gamepad` profile and input policy. Succeeds
    /// on Linux without plugging anything in: no pad follows, and the first controller input
    /// raises the warning.
    pub fn new(
        profile: u16,
        policy: butterpollo_input_policy::input_policy::Policy,
        warnings: Arc<butterpollo_session::session::Warnings>,
    ) -> std::io::Result<Self> {
        let _ = (profile, policy);
        Ok(Self {
            warnings: Mutex::new(warnings),
        })
    }
    /// The warnings of the stream these controllers now belong to.
    pub fn set_warnings(&self, warnings: Arc<butterpollo_session::session::Warnings>) {
        *self.warnings.lock().unwrap_or_else(PoisonError::into_inner) = warnings;
    }
    /// Hand an event to the pads. On Linux there are none: the event is dropped and the
    /// stream warns that controllers are unavailable.
    pub fn send(&self, event: Event) {
        drop(event);
        self.warnings
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .set(
                UNAVAILABLE,
                format!(
                    "{:#}; controller input is ignored.",
                    crate::unsupported("A virtual gamepad")
                ),
            );
    }
    /// Feedback and motion requests the pads produced since the last call: none on Linux.
    pub fn reports(&self) -> impl Iterator<Item = PadReport> + '_ {
        std::iter::empty()
    }
    /// Audio for a controller's speaker or haptics. Dropped on Linux, which has no pad to
    /// play it.
    pub fn audio(&self, id: u16, kind: u16, payload: &[u8]) {
        let _ = (id, kind, payload);
    }
    /// Whether controller `id` has a USB audio endpoint attached: never on Linux.
    pub fn controller_audio_ready(&self, id: u16) -> bool {
        let _ = id;
        false
    }
    /// Release the controls while the client is away. Nothing is held on Linux.
    pub fn park(&self) {}
    /// Take the controllers up again for a client. Nothing to take up on Linux.
    pub fn activate(&self) {}
    /// Whether the controllers in `mask` are plugged in, waiting up to `timeout` for them.
    /// On Linux no pad ever plugs in, so this answers at once: true only for an empty mask.
    pub fn wait_ready(&self, mask: u16, timeout: Duration) -> bool {
        let _ = timeout;
        mask == 0
    }
    /// How many of its pads are plugged in: none on Linux.
    pub fn plugged_pads(&self) -> u32 {
        0
    }
    /// Tell the pads to unplug, without waiting. Nothing is plugged in on Linux.
    pub fn request_stop(&self) {}
    /// Stop the pads and return their thread to wait for. There is no thread on Linux.
    pub fn stop(&self) -> Option<JoinHandle<()>> {
        None
    }
    /// Stop the pads and wait up to `timeout` for them to unplug. Succeeds at once on Linux,
    /// where nothing is plugged in.
    pub fn stop_within(&self, timeout: Duration) -> Result<()> {
        let _ = timeout;
        Ok(())
    }
}
