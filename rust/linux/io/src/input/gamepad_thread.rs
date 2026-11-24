//! One client's virtual gamepads and what they hand back to the control stream. A Moonlight
//! controller becomes the pad the Windows host would give it (`pad_choice`): an Xbox 360 pad on
//! `/dev/uinput`, or with `/dev/uhid` a DualSense or DualShock 4 (the kernel's hid-playstation
//! driver gives games the gamepad, motion sensors and touchpad), a Switch Pro Controller
//! (hid-nintendo), a Steam Deck or a Steam Controller (hid-steam). Pads are written to as
//! events arrive (a write is quick, so no thread is needed); each has a thread of its own for
//! what the kernel asks of it, which reports the game's rumble, lights and trigger effects.
//! Speaker and haptics audio are not supported.
use super::{
    Event,
    deck_model::DeckModel,
    feedback::Sink,
    hid_pad::{self, Slot},
    pad_choice::{self, Chooser, Want},
    uhid_pad::UhidPad,
    uinput,
    vhf_model::VhfModel,
};
use anyhow::Result;
use butterpollo_input_policy::{haptics, input_policy};
use rubylight_vgamepad_proto::protocol::Profile;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, PoisonError},
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// The warning for controllers that cannot be had, as when `/dev/uinput` is not writable.
const UNAVAILABLE: &str = "input_gamepad";
/// The warning for a Switch 2 Pro Controller that no kernel driver binds.
const SWITCH2: &str = "input_switch2";
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
    /// Whether `/dev/uhid` can be written, so the pads with a kernel driver can be made.
    uhid: bool,
    /// Whether Steam runs on this PC, which two choices of pad ask.
    steam_running: fn() -> bool,
}

/// A virtual pad of any kind.
enum Pad {
    Xbox(uinput::Pad),
    DualSense(hid_pad::HidPad),
    /// A pad of the kind on `/dev/uhid` that [`Want`] says.
    Uhid(UhidPad, Want),
}

#[derive(Default)]
struct Pads {
    plugged: BTreeMap<u16, Pad>,
    /// Each controller's arrival: its type and capabilities, which choose its pad.
    arrivals: BTreeMap<u16, (u8, u16)>,
    /// The USB vendor and product each client's controller reported.
    identities: BTreeMap<u16, (u16, u16)>,
    failed: Option<Instant>,
}

impl Pad {
    /// Everything released and centred.
    fn neutral(&mut self, id: u16) -> Result<()> {
        match self {
            Self::Xbox(pad) => pad.set(uinput::State::default()),
            Self::DualSense(_) | Self::Uhid(..) => {
                self.apply(&Event::Controller {
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
    /// Hands a state, motion, touch or battery event to a pad on `/dev/uhid`.
    fn apply(&self, event: &Event) {
        match self {
            Self::DualSense(pad) => pad.apply(event),
            Self::Uhid(pad, _) => pad.apply(event),
            Self::Xbox(_) => {}
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
            steam_running: pad_choice::steam_running,
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
            Pad::DualSense(_) | Pad::Uhid(..) => None,
        }
    }
    /// The `phys` name of pad `id`'s uhid device, for the device tests.
    #[cfg(test)]
    pub(super) fn phys(&self, id: u16) -> Option<String> {
        match self.pads().plugged.get(&id)? {
            Pad::DualSense(pad) => Some(pad.phys.clone()),
            Pad::Uhid(pad, _) => Some(pad.phys.clone()),
            Pad::Xbox(_) => None,
        }
    }
    /// The kind of pad `id` is, for the device tests.
    #[cfg(test)]
    pub(super) fn kind(&self, id: u16) -> Option<Want> {
        Some(match self.pads().plugged.get(&id)? {
            Pad::Xbox(_) => Want::Xbox,
            Pad::DualSense(_) => Want::DualSense,
            Pad::Uhid(_, want) => *want,
        })
    }
    fn pads(&self) -> std::sync::MutexGuard<'_, Pads> {
        self.pads.lock().unwrap_or_else(PoisonError::into_inner)
    }
    /// The choice of pad for this PC and policy.
    fn chooser(&self) -> Chooser<'_> {
        Chooser {
            profile: self.profile,
            policy: &self.policy,
            uhid: self.uhid,
            steam_running: &self.steam_running,
        }
    }
    /// Plugs in controller `id` unless it is, saying in the warnings why it could not be.
    fn plug<'a>(&self, pads: &'a mut Pads, id: u16) -> Option<&'a mut Pad> {
        if !pads.plugged.contains_key(&id) {
            if pads.failed.is_some_and(|at| at.elapsed() < RETRY) {
                return None;
            }
            let arrival = pads.arrivals.get(&id).copied();
            let identity = pads.identities.get(&id).copied();
            let created = self.create(id, arrival, identity);
            match created {
                Ok(pad) => {
                    pads.failed = None;
                    pads.plugged.insert(id, pad);
                    self.warn(|warnings| warnings.clear(UNAVAILABLE));
                }
                Err(error) => {
                    pads.failed = Some(Instant::now());
                    self.warn(|warnings| {
                        warnings.set(
                            UNAVAILABLE,
                            format!("{error:#}; controller input is ignored."),
                        );
                    });
                    return None;
                }
            }
        }
        pads.plugged.get_mut(&id)
    }
    fn warn(&self, f: impl FnOnce(&butterpollo_session::session::Warnings)) {
        f(&self.warnings.lock().unwrap_or_else(PoisonError::into_inner));
    }
    /// A new pad for controller `id`. A pad of a kind the kernel will not bind (its driver is
    /// missing, or `/dev/uhid` refuses it) gives way to the next the policy would give, with a
    /// warning that says why the controller's sensors, touchpad or lights may be lost.
    fn create(
        &self,
        id: u16,
        arrival: Option<(u8, u16)>,
        identity: Option<(u16, u16)>,
    ) -> Result<Pad> {
        let arrival = arrival.unwrap_or_default();
        let mut refused = Vec::new();
        loop {
            let want = self.chooser().want(arrival, identity, &refused);
            let (key, name) = want.warning();
            match self.make(want, id, arrival.1) {
                Ok(pad) => {
                    self.warn(|warnings| {
                        warnings.clear(key);
                        if want == Want::Switch
                            && input_policy::wants_switch2(self.profile, identity)
                        {
                            warnings.set(
                                SWITCH2,
                                "Switch 2 Pro Controller unavailable: the Linux kernel has no HID driver for it (hid-nintendo binds the Switch Pro Controller, 057e:2009, not 057e:2069), so the controller arrives as a Switch Pro Controller instead, without its extra buttons.",
                            );
                        } else {
                            warnings.clear(SWITCH2);
                        }
                    });
                    if want == Want::Switch && input_policy::wants_switch2(self.profile, identity) {
                        tracing::warn!(
                            controller = id,
                            "no kernel driver binds a Switch 2 Pro Controller over uhid; using a Switch Pro Controller"
                        );
                    }
                    return Ok(pad);
                }
                Err(error) if want == Want::Xbox => return Err(error),
                Err(error) => {
                    tracing::warn!(controller = id, error = %format!("{error:#}"), "virtual {name} could not be made");
                    self.warn(|warnings| {
                        warnings.set(
                            key,
                            format!(
                                "The virtual {name} could not be made ({error:#}); another virtual controller takes its place, which may lack its motion sensors, touchpad or lights."
                            ),
                        );
                    });
                    refused.push(want);
                }
            }
        }
    }
    /// A pad of kind `want` for controller `id` whose client reported `capabilities`.
    fn make(&self, want: Want, id: u16, capabilities: u16) -> Result<Pad> {
        let sink = self.sink.clone();
        let profile = match want {
            Want::Xbox => return uinput::Pad::create(id, sink).map(Pad::Xbox),
            Want::DualSense => {
                return hid_pad::HidPad::create(id, capabilities, sink).map(Pad::DualSense);
            }
            Want::DualShock4 => Profile::DualShock4,
            Want::Switch => Profile::SwitchPro,
            Want::SteamController => Profile::SteamController,
            Want::Deck => {
                let slot = Slot::reserve()?;
                // Steam takes a Deck over itself; without Steam the pad leaves the driver's
                // lizard mode, which holds its gamepad input back, as the player would.
                let model =
                    DeckModel::new(&format!("BUTTERPOLLO{}", slot.0), !(self.steam_running)());
                return UhidPad::create_in(id, slot, Box::new(model), sink)
                    .map(|pad| Pad::Uhid(pad, want));
            }
        };
        let slot = Slot::reserve()?;
        // The kernel's PlayStation driver wants every pad's address unique, also among hosts.
        let pid = std::process::id() as u16;
        let address = [slot.0, 0x52, (pid & 0xff) as u8, (pid >> 8) as u8, 0x59, 2];
        let model = VhfModel::new(
            profile,
            u32::from(slot.0),
            capabilities,
            address,
            !(self.steam_running)(),
        );
        UhidPad::create_in(id, slot, Box::new(model), sink).map(|pad| Pad::Uhid(pad, want))
    }
    /// The buttons and triggers a pad of kind `want` gets: only a Steam Controller has back
    /// grips of its own, so on any other pad each presses what it is mapped to.
    fn mapped(&self, want: Want, event: &Event) -> Event {
        let Event::Controller {
            id,
            active,
            buttons,
            left_trigger,
            right_trigger,
            sticks,
        } = event
        else {
            return event.clone();
        };
        let (mut buttons, left_trigger, right_trigger) =
            if matches!(want, Want::SteamController | Want::Deck) {
                (*buttons, *left_trigger, *right_trigger)
            } else {
                self.policy
                    .map_back_grips(*buttons, *left_trigger, *right_trigger)
            };
        if self.profile == input_policy::VIGEM_DS4
            && want == Want::DualShock4
            && self.policy.ds4_back_as_touchpad_click
            && buttons & 0x20 != 0
        {
            buttons |= 0x10_0000;
        }
        Event::Controller {
            id: *id,
            active: *active,
            buttons,
            left_trigger,
            right_trigger,
            sticks: *sticks,
        }
    }
    /// Hand an event to the pads: an arrival plugs a pad in, a state updates it and unplugs
    /// the pads whose controller is no longer active, motion, touch and battery events go to
    /// the pads that have them. Other events are dropped.
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
                if let Some(Pad::DualSense(_) | Pad::Uhid(..)) = self.plug(&mut pads, id) {
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
                pads.identities.retain(|pad, _| active & (1 << *pad) != 0);
                if id < 16 && active & (1 << id) != 0 {
                    let result = match self.plug(&mut pads, id) {
                        Some(Pad::Xbox(pad)) => pad.set(uinput::State::from_report(
                            *buttons,
                            *left_trigger,
                            *right_trigger,
                            sticks,
                        )),
                        Some(Pad::DualSense(pad)) => {
                            pad.apply(&self.mapped(Want::DualSense, &event));
                            Ok(())
                        }
                        Some(Pad::Uhid(pad, want)) => {
                            pad.apply(&self.mapped(*want, &event));
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
                if let Some(pad) = pads.plugged.get(&u16::from(*id)) {
                    pad.apply(&event);
                }
            }
            Event::SteamAttributes(attributes) => {
                // Only from a Steam Controller that announced them.
                if let Some(pad @ Pad::Uhid(_, Want::SteamController)) =
                    pads.plugged.get(&u16::from(attributes.controller))
                {
                    pad.apply(&event);
                }
            }
            Event::ControllerIdentity(identity) => {
                pads.identities.insert(
                    u16::from(identity.controller),
                    (identity.vendor, identity.product),
                );
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
    fn the_sensors_of_a_dualsense_are_asked_of_the_client() {
        // Reports of a pad that was never made are none.
        assert_eq!(pads(0, true).reports().count(), 0);
    }
}
