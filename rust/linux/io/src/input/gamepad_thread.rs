//! One client's virtual gamepads and what they hand back to the control stream. Each Moonlight
//! controller is an Xbox 360 pad on `/dev/uinput`, written to as events arrive (a uinput write
//! is quick, so no thread is needed). Rumble, lights, motion and audio are not supported: no
//! feedback is ever reported.
use super::{Event, uinput};
use anyhow::Result;
use butterpollo_input_policy::haptics;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, PoisonError},
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// The warning for controllers that cannot be had, as when `/dev/uinput` is not writable.
const UNAVAILABLE: &str = "input_gamepad";
/// How long after a pad failed to plug in the next try waits, so input does not retry per event.
const RETRY: Duration = Duration::from_secs(1);

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

/// One client's virtual gamepads, a pad per controller number.
pub struct GamepadThread {
    warnings: Mutex<Arc<butterpollo_session::session::Warnings>>,
    pads: Mutex<Pads>,
}

#[derive(Default)]
struct Pads {
    plugged: BTreeMap<u16, uinput::Pad>,
    failed: Option<Instant>,
}

impl GamepadThread {
    /// The controllers of one client with the `gamepad` profile and input policy. No pad is
    /// plugged in until a controller arrives or sends its state; the profile and policy
    /// make no difference on Linux, where every pad is an Xbox 360 pad.
    pub fn new(
        profile: u16,
        policy: butterpollo_input_policy::input_policy::Policy,
        warnings: Arc<butterpollo_session::session::Warnings>,
    ) -> std::io::Result<Self> {
        let _ = (profile, policy);
        Ok(Self {
            warnings: Mutex::new(warnings),
            pads: Mutex::default(),
        })
    }
    /// The warnings of the stream these controllers now belong to.
    pub fn set_warnings(&self, warnings: Arc<butterpollo_session::session::Warnings>) {
        *self.warnings.lock().unwrap_or_else(PoisonError::into_inner) = warnings;
    }
    /// The sysfs name of pad `id`'s device, for the device tests.
    #[cfg(test)]
    pub(super) fn sysname(&self, id: u16) -> Option<String> {
        Some(self.pads().plugged.get(&id)?.sysname.clone())
    }
    fn pads(&self) -> std::sync::MutexGuard<'_, Pads> {
        self.pads.lock().unwrap_or_else(PoisonError::into_inner)
    }
    /// Plugs in controller `id` unless it is, saying in the warnings why it could not be.
    fn plug<'a>(&self, pads: &'a mut Pads, id: u16) -> Option<&'a mut uinput::Pad> {
        if !pads.plugged.contains_key(&id) {
            if pads.failed.is_some_and(|at| at.elapsed() < RETRY) {
                return None;
            }
            match uinput::Pad::create(id) {
                Ok(pad) => {
                    pads.failed = None;
                    pads.plugged.insert(id, pad);
                    self.warnings
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .clear(UNAVAILABLE);
                }
                Err(error) => {
                    pads.failed = Some(Instant::now());
                    self.warnings
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .set(
                            UNAVAILABLE,
                            format!("{error:#}; controller input is ignored."),
                        );
                    return None;
                }
            }
        }
        pads.plugged.get_mut(&id)
    }
    /// Hand an event to the pads: an arrival plugs a pad in, a state updates it and unplugs
    /// the pads whose controller is no longer active. Other events are dropped.
    pub fn send(&self, event: Event) {
        let mut pads = self.pads();
        match event {
            Event::Arrival { id, .. } if id < 16 => {
                // A controller that arrives again is a fresh pad.
                pads.plugged.remove(&u16::from(id));
                self.plug(&mut pads, u16::from(id));
            }
            Event::Controller {
                id,
                active,
                buttons,
                left_trigger,
                right_trigger,
                sticks,
            } => {
                pads.plugged.retain(|pad, _| active & (1 << *pad) != 0);
                if id < 16 && active & (1 << id) != 0 {
                    let state =
                        uinput::State::from_report(buttons, left_trigger, right_trigger, &sticks);
                    if let Some(pad) = self.plug(&mut pads, id)
                        && let Err(error) = pad.set(state)
                    {
                        tracing::warn!(%error, id, "virtual gamepad write failed");
                    }
                }
            }
            _ => {}
        }
    }
    /// Feedback and motion requests the pads produced since the last call: none on Linux.
    pub fn reports(&self) -> impl Iterator<Item = PadReport> + '_ {
        std::iter::empty()
    }
    /// Audio for a controller's speaker or haptics. Dropped: the virtual pads have none.
    pub fn audio(&self, id: u16, kind: u16, payload: &[u8]) {
        let _ = (id, kind, payload);
    }
    /// Whether controller `id` has a USB audio endpoint attached: never on Linux.
    pub fn controller_audio_ready(&self, id: u16) -> bool {
        let _ = id;
        false
    }
    /// Release the controls while the client is away: every pad goes neutral and stays plugged in.
    pub fn park(&self) {
        for pad in self.pads().plugged.values_mut() {
            if let Err(error) = pad.set(uinput::State::default()) {
                tracing::debug!(%error, "virtual gamepad release failed");
            }
        }
    }
    /// Take the controllers up again for a client. Pads follow the next state they get.
    pub fn activate(&self) {}
    /// Whether the controllers in `mask` are plugged in, waiting up to `timeout` for them.
    /// Pads plug in as events arrive, so this waits (up to `timeout`) only for events not
    /// yet sent.
    pub fn wait_ready(&self, mask: u16, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            let ready = self.pads().plugged.keys().fold(0u16, |m, id| m | (1 << id));
            if ready & mask == mask {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    /// How many of its pads are plugged in.
    pub fn plugged_pads(&self) -> u32 {
        self.pads().plugged.len() as u32
    }
    /// Unplug the pads. Nothing runs on a thread, so this is done at once.
    pub fn request_stop(&self) {
        self.pads().plugged.clear();
    }
    /// Unplug the pads; there is no thread to wait for.
    pub fn stop(&self) -> Option<JoinHandle<()>> {
        self.request_stop();
        None
    }
    /// Unplug the pads, which is done when this returns.
    pub fn stop_within(&self, timeout: Duration) -> Result<()> {
        let _ = timeout;
        self.request_stop();
        Ok(())
    }
}
