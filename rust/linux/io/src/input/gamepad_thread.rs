//! One client's virtual gamepads and what they hand back to the control stream. A Moonlight
//! controller is a DualSense on `/dev/uhid` when it is a PlayStation controller, has motion
//! sensors or a touchpad, or the `gamepad` setting says so (the kernel's hid-playstation driver
//! gives games its gamepad, motion sensors and touchpad), and an Xbox 360 pad on `/dev/uinput`
//! otherwise, chosen as on Windows by `Policy::controller_profile`. Pads are written to as
//! events arrive (a write is quick, so no thread is needed); each has a thread of its own for
//! what the kernel asks of it, which reports the game's rumble, lights and trigger effects.
//! Speaker and haptics audio are not supported.
use super::{Event, feedback::Sink, hid_pad, uinput};
use anyhow::Result;
use butterpollo_input_policy::{haptics, input_policy};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, PoisonError},
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// The warning for controllers that cannot be had, as when `/dev/uinput` is not writable.
const UNAVAILABLE: &str = "input_gamepad";
/// The warning for a DualSense that fell back to an Xbox pad (Windows' code for it).
const DUALSENSE: &str = "input_dualsense";
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
    policy: input_policy::Policy,
    /// The `gamepad` setting's profile.
    profile: u16,
    /// What the pads' threads have for the client.
    sink: Arc<Sink>,
    /// Whether `/dev/uhid` can be written, so PlayStation pads can be made.
    uhid: bool,
}

/// A virtual pad of either kind.
enum Pad {
    Xbox(uinput::Pad),
    DualSense(hid_pad::HidPad),
}

#[derive(Default)]
struct Pads {
    plugged: BTreeMap<u16, Pad>,
    /// Each controller's arrival: its type and capabilities, which choose its pad.
    arrivals: BTreeMap<u16, (u8, u16)>,
    failed: Option<Instant>,
}

impl Pad {
    /// Everything released and centred.
    fn neutral(&mut self, id: u16) -> Result<()> {
        match self {
            Self::Xbox(pad) => pad.set(uinput::State::default()),
            Self::DualSense(pad) => {
                pad.apply(&Event::Controller {
                    id,
                    active: 0,
                    buttons: 0,
                    left_trigger: 0,
                    right_trigger: 0,
                    sticks: [0; 4],
                });
                Ok(())
            }
        }
    }
}

impl GamepadThread {
    /// The controllers of one client with the `gamepad` profile and input policy. No pad is
    /// plugged in until a controller arrives or sends its state.
    pub fn new(
        profile: u16,
        policy: input_policy::Policy,
        warnings: Arc<butterpollo_session::session::Warnings>,
    ) -> std::io::Result<Self> {
        Ok(Self {
            warnings: Mutex::new(warnings),
            pads: Mutex::default(),
            policy,
            profile: if profile == input_policy::VHF_AUTO {
                0
            } else {
                profile
            },
            sink: Arc::default(),
            uhid: hid_pad::available(),
        })
    }
    /// The socket to wake when the pads have feedback for the client.
    pub(super) fn set_feedback_waker(&self, waker: Arc<std::net::UdpSocket>) {
        self.sink.set_waker(waker);
    }
    /// The warnings of the stream these controllers now belong to.
    pub fn set_warnings(&self, warnings: Arc<butterpollo_session::session::Warnings>) {
        *self.warnings.lock().unwrap_or_else(PoisonError::into_inner) = warnings;
    }
    /// The sysfs name of pad `id`'s device, for the device tests.
    #[cfg(test)]
    pub(super) fn sysname(&self, id: u16) -> Option<String> {
        match self.pads().plugged.get(&id)? {
            Pad::Xbox(pad) => Some(pad.sysname.clone()),
            Pad::DualSense(_) => None,
        }
    }
    /// The `phys` name of DualSense `id`'s uhid device, for the device tests.
    #[cfg(test)]
    pub(super) fn phys(&self, id: u16) -> Option<String> {
        match self.pads().plugged.get(&id)? {
            Pad::DualSense(pad) => Some(pad.phys.clone()),
            Pad::Xbox(_) => None,
        }
    }
    fn pads(&self) -> std::sync::MutexGuard<'_, Pads> {
        self.pads.lock().unwrap_or_else(PoisonError::into_inner)
    }
    /// Whether controller `id` is a DualSense, given the type and capabilities it arrived
    /// with: as on Windows, by the `gamepad` setting and then by what the client's controller
    /// is.
    fn wants_dualsense(&self, arrival: Option<(u8, u16)>) -> bool {
        let (kind, capabilities) = arrival.unwrap_or_default();
        let mut available = 1 << (input_policy::VHF_XBOX_SERIES - 1);
        if self.uhid {
            available |= 1 << (input_policy::VHF_DUALSENSE - 1);
        }
        // The PlayStation pads share the DualSense; the Xbox ones are the uinput pad.
        let configured = match self.profile {
            input_policy::USBIP_DUALSENSE | input_policy::VHF_DS4 | input_policy::VIGEM_DS4 => {
                input_policy::VHF_DUALSENSE
            }
            input_policy::VHF_XBOX_ONE | input_policy::VIGEM_X360 => input_policy::VHF_XBOX_SERIES,
            other => other,
        };
        self.policy
            .controller_profile(configured, kind, capabilities, available, false)
            == Some(input_policy::VHF_DUALSENSE)
    }
    /// Plugs in controller `id` unless it is, saying in the warnings why it could not be.
    fn plug<'a>(&self, pads: &'a mut Pads, id: u16) -> Option<&'a mut Pad> {
        if !pads.plugged.contains_key(&id) {
            if pads.failed.is_some_and(|at| at.elapsed() < RETRY) {
                return None;
            }
            let arrival = pads.arrivals.get(&id).copied();
            let created = self.create(id, arrival);
            match created {
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
    /// A new pad for controller `id`. A DualSense that cannot be had falls back to an Xbox
    /// pad, with a warning that says why the controller's motion and touchpad are lost.
    fn create(&self, id: u16, arrival: Option<(u8, u16)>) -> Result<Pad> {
        if self.wants_dualsense(arrival) {
            match hid_pad::HidPad::create(id, arrival.unwrap_or_default().1, self.sink.clone()) {
                Ok(pad) => {
                    self.warnings
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .clear(DUALSENSE);
                    return Ok(Pad::DualSense(pad));
                }
                Err(error) => {
                    self.warnings
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .set(
                            DUALSENSE,
                            format!(
                                "The virtual DualSense could not be made ({error:#}); an Xbox 360 pad takes its place, without motion sensors or touchpad."
                            ),
                        );
                }
            }
        }
        uinput::Pad::create(id, self.sink.clone()).map(Pad::Xbox)
    }
    /// Hand an event to the pads: an arrival plugs a pad in, a state updates it and unplugs
    /// the pads whose controller is no longer active, motion, touch and battery events go to
    /// a DualSense. Other events are dropped.
    pub fn send(&self, event: Event) {
        let mut pads = self.pads();
        match &event {
            Event::Arrival {
                id,
                kind,
                capabilities,
                ..
            } if *id < 16 => {
                let id = u16::from(*id);
                pads.arrivals.insert(id, (*kind, *capabilities));
                // A controller that arrives again is a fresh pad.
                pads.plugged.remove(&id);
                if let Some(Pad::DualSense(_)) = self.plug(&mut pads, id) {
                    // Ask the client for the sensors its controller has.
                    self.sink.push(PadReport::Motion {
                        id: id as u8,
                        capabilities: *capabilities,
                    });
                }
            }
            Event::Controller {
                id,
                active,
                buttons,
                left_trigger,
                right_trigger,
                sticks,
            } => {
                let (id, active) = (*id, *active);
                pads.plugged.retain(|pad, _| active & (1 << *pad) != 0);
                pads.arrivals.retain(|pad, _| active & (1 << *pad) != 0);
                if id < 16 && active & (1 << id) != 0 {
                    let result = match self.plug(&mut pads, id) {
                        Some(Pad::Xbox(pad)) => pad.set(uinput::State::from_report(
                            *buttons,
                            *left_trigger,
                            *right_trigger,
                            sticks,
                        )),
                        Some(Pad::DualSense(pad)) => {
                            pad.apply(&event);
                            Ok(())
                        }
                        None => Ok(()),
                    };
                    if let Err(error) = result {
                        tracing::warn!(%error, id, "virtual gamepad write failed");
                    }
                }
            }
            Event::Motion { id, .. }
            | Event::ControllerTouch { id, .. }
            | Event::Battery { id, .. } => {
                if let Some(Pad::DualSense(pad)) = pads.plugged.get(&u16::from(*id)) {
                    pad.apply(&event);
                }
            }
            _ => {}
        }
    }
    /// Feedback and motion requests the pads produced since the last call.
    pub fn reports(&self) -> impl Iterator<Item = PadReport> + '_ {
        self.sink.take().into_iter()
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
        for (id, pad) in &mut self.pads().plugged {
            if let Err(error) = pad.neutral(*id) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn pads(profile: u16, uhid: bool) -> GamepadThread {
        let policy =
            input_policy::Policy::resolve(&butterpollo_config::config::Config::default()).unwrap();
        let mut pads =
            GamepadThread::new(profile, policy, Arc::default()).expect("a gamepad thread");
        pads.uhid = uhid;
        pads
    }

    #[test]
    fn the_pad_follows_the_controller_like_on_windows() {
        let auto = pads(0, true);
        // An Xbox controller with no sensors stays an Xbox pad.
        assert!(!auto.wants_dualsense(None));
        assert!(!auto.wants_dualsense(Some((1, 0))));
        // A PlayStation controller, or one with an accelerometer or gyroscope or a touchpad,
        // is a DualSense so its sensors work.
        assert!(auto.wants_dualsense(Some((2, 0))));
        assert!(auto.wants_dualsense(Some((1, 0x10))));
        assert!(auto.wants_dualsense(Some((1, 0x20))));
        assert!(auto.wants_dualsense(Some((0, 8))));
        // Without /dev/uhid every controller is an Xbox pad.
        assert!(!pads(0, false).wants_dualsense(Some((2, 0x38))));
        // The setting decides first: Xbox pads stay Xbox, PlayStation pads are a DualSense.
        for xbox in [input_policy::VHF_XBOX_ONE, input_policy::VHF_XBOX_SERIES] {
            assert!(!pads(xbox, true).wants_dualsense(Some((2, 0x38))));
        }
        for sony in [
            input_policy::VHF_DUALSENSE,
            input_policy::USBIP_DUALSENSE,
            input_policy::VHF_DS4,
        ] {
            assert!(pads(sony, true).wants_dualsense(Some((1, 0))));
        }
        // `vhf` is automatic.
        assert!(pads(input_policy::VHF_AUTO, true).wants_dualsense(Some((2, 0))));
        assert!(!pads(input_policy::VHF_AUTO, true).wants_dualsense(Some((1, 0))));
    }

    #[test]
    fn the_sensors_of_a_dualsense_are_asked_of_the_client() {
        // Reports of a pad that was never made are none.
        assert_eq!(pads(0, true).reports().count(), 0);
    }
}
