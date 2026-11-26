//! The Windows host's VHF driver pads as models for [`super::uhid_pad`]: a Switch Pro Controller,
//! a DualShock 4 and a Steam Controller (2026), each the profile `rubylight-vgamepad-proto`
//! shares with the Windows driver, so the descriptors, input reports, feature-report answers
//! and the decoding of the game's output reports are the ones the Windows pad has. The
//! driver's slot state machine ([`DeviceState`]) runs here too; only the transport differs:
//! where the Windows driver hands a report to VHF, this hands it to `/dev/uhid`, which the
//! kernel's hid-nintendo, hid-playstation and hid-steam drivers read.

use super::lizard::{Lizard, Phase};
use super::uhid::ReportType;
use super::uhid_pad::{Identity, PadModel};
use butterpollo_input_policy::{input::Input, steam_feedback::SteamPads, steam_pad};
use rubylight_protocol::steam_controller::SteamAttributes;
#[cfg(test)]
use rubylight_protocol::steam_controller::{LI_CCAP_STEAM_HAPTIC, STEAM_HAPTIC_MESSAGE_TYPE};
use rubylight_vgamepad_proto::{
    lifecycle::{DeviceState, PendingSubmit},
    profiles::{
        self, dualshock4, steam_controller,
        switch_pro::{self, SUB_ENABLE_VIBRATION},
    },
    protocol::{
        BatteryStateRequest, ControllerIdRequest, CreateControllerRequest, InputStateRequest,
        MotionStateRequest, Profile, RequestHeader, TouchStateRequest,
    },
};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

/// The driver's owner token for the one handle the pad has.
const OWNER: usize = 1;
/// The handle the slot's report path is bound to; any non-zero value.
const HANDLE: usize = 1;
/// A real Switch Pro Controller sends a report every 8 ms over USB, and hid-nintendo's
/// subcommand rate limiter wants reports 8 to 17 ms apart.
const SWITCH_PERIOD: Duration = Duration::from_millis(10);
/// The Steam Controller's keep-alive tick (the driver's `SC26_TICK_MS`).
const STEAM_PERIOD: Duration = Duration::from_millis(4);
/// Moonlight's Menu (start) button.
const MENU: u32 = 0x10;
/// The Steam Controller's command for lizard mode on (default mappings), and the output report
/// the driver writes twice as it switches it off, to tell the player (the clear-mappings command
/// comes with it, but a program that has the pad open sends that too).
const LIZARD_ON: u8 = steam_controller::CMD_SET_DEFAULT_DIGITAL_MAPPINGS;
const SWITCHED: u8 = steam_controller::HAPTIC_PULSE_ID;
/// Moonlight's `LI_CCAP_DUAL_TOUCHPAD`.
const CCAP_DUAL_TOUCHPAD: u16 = 0x100;

/// The kernel's request for each feature report hid-playstation reads while binding.
const DS4_PROBE: [u8; 3] = [0x02, 0x12, 0xa3];

pub(super) struct VhfModel {
    state: Box<DeviceState>,
    /// The slot, which also makes the pad's address unique in the kernel's driver.
    id: u32,
    profile: Profile,
    started: Instant,
    capabilities: u16,
    last: InputStateRequest,
    reports: Vec<Vec<u8>>,
    /// The feedback the driver's writes made and the client has not been told.
    feedback: Vec<(u16, Vec<u8>)>,
    last_feedback: Option<(u16, Vec<u8>)>,
    /// Which of [`DS4_PROBE`] the kernel has read.
    served: u8,
    /// Whether the driver has finished its start-up exchange.
    bound: bool,
    /// Finger to contact slot, for the touch surface.
    pointers: BTreeMap<u32, u8>,
    /// Rumble synthesized from a Steam Controller's haptics for a client without pads of its own.
    steam_rumble: SteamPads,
    steam_sent: (u16, u16),
    /// Leaves hid-steam's lizard mode, which holds a Steam Controller's gamepad input back
    /// until Menu is held, as a real one's player would ([`Lizard`]).
    lizard: Lizard,
    phase: Phase,
}

impl VhfModel {
    /// A pad of `profile` in slot `id` for a client controller with `capabilities`.
    /// `address` is the DualShock 4's pairing address, which the kernel's driver wants unique.
    /// `leave_lizard`: whether a Steam Controller leaves hid-steam's lizard mode itself, which
    /// one that Steam takes over (it opens the driver's hidraw node) needs not.
    pub(super) fn new(
        profile: Profile,
        id: u32,
        capabilities: u16,
        address: [u8; 6],
        leave_lizard: bool,
    ) -> Self {
        let mut state = Box::new(DeviceState::new());
        state.powered = true;
        let request = CreateControllerRequest::new(id, profile as u16);
        let generation = state.slots[id as usize].generation;
        state
            .begin_create(OWNER, &request, generation, false)
            .expect("a free slot of a shipped profile");
        state.slots[id as usize].ds4.features.address = address;
        state.publish_vhf(id, HANDLE);
        assert!(state.commit_start(id, OWNER, true));
        let mut model = Self {
            state,
            id,
            profile,
            started: Instant::now(),
            capabilities,
            last: InputStateRequest {
                header: RequestHeader::new(InputStateRequest::WIRE_SIZE as u32),
                controller_id: id,
                ..Default::default()
            },
            reports: Vec::new(),
            feedback: Vec::new(),
            last_feedback: None,
            served: 0,
            bound: false,
            pointers: BTreeMap::new(),
            steam_rumble: SteamPads::default(),
            steam_sent: (0, 0),
            lizard: Lizard::new(leave_lizard && profile == Profile::SteamController),
            phase: Phase::Idle,
        };
        if profile == Profile::SteamController {
            // The firmware a real Steam Controller reports gets no update offer from Steam;
            // a client's real one replaces it ([`PadModel::apply`]).
            model.set_attributes(&steam_pad::virtual_attributes(id as u8));
        }
        // Reports go out as they are made: the report path is always ready for the next.
        model.drain(None);
        model
    }
    fn now(&self) -> u64 {
        self.started.elapsed().as_micros() as u64
    }
    fn set_attributes(&mut self, attributes: &SteamAttributes) {
        let bytes = steam_pad::attributes_request(self.id, attributes);
        if let Some(request) =
            rubylight_vgamepad_proto::protocol::SteamControllerAttributesRequest::from_bytes(&bytes)
        {
            let _ = self.state.set_sc_attributes(OWNER, &request);
        }
    }
    /// Moves the reports the pump holds, `first` and then each it releases, to the outbox.
    fn drain(&mut self, first: Option<PendingSubmit>) {
        let mut pending = first;
        loop {
            if let Some(report) = pending.take() {
                let data = &self.state.slots[self.id as usize].pump.in_flight.data;
                // While the driver switches out of lizard mode it sees no report.
                if self.phase != Phase::Silent {
                    self.reports.push(data[..report.length as usize].to_vec());
                }
                // Taken: the report reached uhid, or was dropped on purpose while silent.
                self.state.submit_finish(self.id, true);
            }
            let now = self.now();
            match self.state.on_vhf_ready_for_next_report(self.id, now).0 {
                Some(next) => pending = Some(next),
                None => break,
            }
        }
    }
    fn request(&mut self, event: &Input) -> Option<InputStateRequest> {
        let Input::Controller {
            buttons,
            left_trigger,
            right_trigger,
            sticks,
            ..
        } = event
        else {
            return None;
        };
        let buttons = if self.profile == Profile::SteamController {
            steam_pad::buttons(*buttons, self.capabilities)
        } else {
            // Only the Steam Controller has the Switch 2 Pro's C button's bit.
            *buttons & !butterpollo_input_policy::switch2::BUTTON_C
        };
        Some(InputStateRequest {
            header: RequestHeader::new(InputStateRequest::WIRE_SIZE as u32),
            controller_id: self.id,
            buttons,
            left_x: sticks[0],
            left_y: sticks[1],
            right_x: sticks[2],
            right_y: sticks[3],
            left_trigger: *left_trigger,
            right_trigger: *right_trigger,
            reserved: 0,
        })
    }
    fn submit(&mut self, request: InputStateRequest) {
        self.last = request;
        let mut request = request;
        if self.phase == Phase::Showing {
            request.buttons |= MENU;
        }
        let now = self.now();
        if let Ok((pending, _)) = self.state.submit_input_state(OWNER, &request, now, true) {
            self.drain(pending);
        }
    }
    fn dual(&self) -> bool {
        self.capabilities & CCAP_DUAL_TOUCHPAD != 0
    }
    /// The driver's touch request for a controller touch, with the finger-to-contact
    /// bookkeeping the Windows host does for its single touch surface.
    fn touch_request(&mut self, event: &Input) -> Option<(TouchStateRequest, u32)> {
        let Input::ControllerTouch {
            event,
            touchpad,
            pointer,
            x,
            y,
            pressure,
            ..
        } = event
        else {
            return None;
        };
        let steam = self.profile == Profile::SteamController;
        let dual = self.dual();
        if *touchpad > u8::from(dual) || self.profile == Profile::SwitchPro {
            return None;
        }
        let event = match event {
            0..=4 => *event,
            6 => 4,
            7 => 5,
            _ => return None,
        };
        // Finger ids are small; the top bit tells the second touchpad's apart.
        let pointer = (*pointer & 0x7fff_ffff) | (u32::from(*touchpad) << 31);
        let slot = if event == 5 {
            0
        } else if let Some(slot) = self.pointers.get(&pointer) {
            *slot
        } else if matches!(event, 0 | 1) {
            if steam {
                // The Steam Controller's contact is its pad: one finger each.
                let pad = steam_pad::touch_pad(*x, *touchpad, dual);
                if self.pointers.values().any(|used| *used == pad) {
                    return None;
                }
                pad
            } else {
                (0..2).find(|slot| !self.pointers.values().any(|used| used == slot))?
            }
        } else {
            return None;
        };
        let x = if steam {
            steam_pad::touch_x(*x, slot, dual)
        } else if dual {
            x.clamp(0., 1.) / 2. + f32::from(*touchpad) / 2.
        } else {
            *x
        };
        let word = |f: f32| (f.clamp(0., 1.) * 65535.) as u16;
        Some((
            TouchStateRequest {
                header: RequestHeader::new(TouchStateRequest::WIRE_SIZE as u32),
                controller_id: self.id,
                contact_index: slot,
                event_type: event,
                x: word(x),
                y: word(*y),
                pressure: word(*pressure),
                reserved: 0,
            },
            pointer,
        ))
    }
    /// The driver's pending feedback, routed as the Windows host routes it.
    fn collect_feedback(&mut self) {
        let request = ControllerIdRequest::new(self.id);
        while let Ok(event) = self.state.poll_feedback(OWNER, &request) {
            let kind = event.feedback_type;
            let data = event.payload[..usize::from(event.payload_size).min(32)].to_vec();
            if let Some(routed) = steam_pad::route(self.id as u16, kind, &data, self.capabilities) {
                // The pulses with which the driver tells the player it switched are no game's.
                let routed = routed.filter(|_| !self.lizard.quiet());
                match routed {
                    Some(steam_pad::Feedback::Forward(kind, data)) => {
                        self.feedback.push((kind, data));
                    }
                    Some(steam_pad::Feedback::Rumble(haptic)) => {
                        self.steam_rumble.apply(&haptic, Instant::now());
                    }
                    None => {}
                }
                continue;
            }
            // A game that writes the same lights and rumble again changes nothing.
            if self.last_feedback.as_ref() != Some(&(kind, data.clone())) {
                self.last_feedback = Some((kind, data.clone()));
                self.feedback.push((kind, data));
            }
        }
        self.synthesized_rumble();
    }
    /// Steam haptics for a client without a Steam Controller's pads, as plain rumble.
    fn synthesized_rumble(&mut self) {
        for (id, low, high) in self.steam_rumble.changes(Instant::now()) {
            if u32::from(id) == self.id {
                self.steam_sent = (low, high);
                let mut data = low.to_le_bytes().to_vec();
                data.extend_from_slice(&high.to_le_bytes());
                data.extend_from_slice(&[0; 4]);
                self.feedback.push((4, data));
            }
        }
    }
    /// Moves the pad's own hold of Menu along, and sends the report that shows a change of
    /// it: the client's state with Menu held, or, once the driver has switched, as it is.
    fn show_hold(&mut self) {
        let phase = self
            .lizard
            .update(self.bound, self.last.buttons & MENU != 0);
        if phase != self.phase {
            self.phase = phase;
            let last = self.last;
            self.submit(last);
        }
    }
    fn written(&mut self, number: u8, data: &[u8]) -> bool {
        let now = self.now();
        let (status, pending, _) = self.state.on_write_report(self.id, number, data, now);
        self.drain(pending);
        self.collect_feedback();
        status.is_success()
    }
}

impl PadModel for VhfModel {
    fn identity(&self) -> Identity {
        match self.profile {
            Profile::SwitchPro => Identity {
                name: "Nintendo Switch Pro Controller",
                vendor: switch_pro::VENDOR_ID,
                product: switch_pro::PRODUCT_ID,
                version: switch_pro::VERSION,
                phys: "sw",
                driver: "hid_nintendo",
            },
            Profile::DualShock4 => Identity {
                name: "Sony Interactive Entertainment DualShock 4 Controller",
                vendor: dualshock4::VENDOR_ID,
                product: dualshock4::PRODUCT_ID,
                version: dualshock4::VERSION,
                phys: "ds4",
                driver: "hid_playstation",
            },
            _ => Identity {
                name: "Valve Steam Controller",
                vendor: steam_controller::VENDOR_ID,
                product: steam_controller::PRODUCT_ID,
                version: steam_controller::VERSION,
                phys: "sc",
                driver: "hid_steam",
            },
        }
    }
    fn descriptor(&mut self) -> Vec<u8> {
        profiles::find_profile(self.profile)
            .map(|definition| definition.report_descriptor.to_vec())
            .unwrap_or_default()
    }
    fn get_report(&mut self, kind: ReportType, number: u8) -> Option<Vec<u8>> {
        let mut buffer = [0u8; 256];
        let now = self.now();
        let written = match kind {
            ReportType::Feature => self.state.on_get_feature(self.id, number, &mut buffer),
            ReportType::Input => self
                .state
                .on_get_input_report(self.id, number, &mut buffer, now),
            ReportType::Output => return None,
        }
        .ok()?;
        if kind == ReportType::Feature
            && let Some(at) = DS4_PROBE.iter().position(|id| *id == number)
        {
            self.served |= 1 << at;
            self.bound |= self.profile == Profile::DualShock4 && self.served == 7;
        }
        Some(buffer[..written].to_vec())
    }
    fn set_report(&mut self, kind: ReportType, number: u8, data: &[u8]) -> bool {
        match kind {
            ReportType::Feature => {
                let accepted = self.state.on_set_feature(self.id, number, data);
                self.collect_feedback();
                if self.profile == Profile::SteamController {
                    match data.get(1) {
                        // hid-steam's last step in registering is loading the default
                        // settings, as it turns lizard mode on.
                        Some(&steam_controller::CMD_LOAD_DEFAULT_SETTINGS) => self.bound = true,
                        Some(&LIZARD_ON) => self.lizard.driver_enabled(),
                        _ => {}
                    }
                }
                accepted.is_success()
            }
            ReportType::Output => self.written(number, data),
            ReportType::Input => false,
        }
    }
    fn output(&mut self, data: &[u8]) {
        let Some(&number) = data.first() else {
            return;
        };
        // hid-nintendo's last step is enabling the vibration.
        if self.profile == Profile::SwitchPro
            && number == switch_pro::RUMBLE_SUBCOMMAND_ID
            && data.get(10) == Some(&SUB_ENABLE_VIBRATION)
        {
            self.written(number, data);
            self.bound = true;
            return;
        }
        if self.profile == Profile::SteamController && number == SWITCHED {
            // The driver left lizard mode: the reports go on, Menu let go.
            self.lizard.driver_disabled();
            self.written(number, data);
            self.show_hold();
            return;
        }
        self.written(number, data);
    }
    fn apply(&mut self, event: &Input) {
        let now = self.now();
        match event {
            Input::Controller { .. } => {
                if let Some(request) = self.request(event) {
                    self.submit(request);
                }
            }
            Input::Motion { kind, xyz, .. } => {
                let milli = |f: f32| {
                    (f64::from(f) * 1000.).clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
                };
                let request = MotionStateRequest {
                    header: RequestHeader::new(MotionStateRequest::WIRE_SIZE as u32),
                    controller_id: self.id,
                    motion_type: *kind,
                    reserved0: [0; 3],
                    x_milli: milli(xyz[0]),
                    y_milli: milli(xyz[1]),
                    z_milli: milli(xyz[2]),
                };
                if let Ok(pending) = self.state.submit_motion_state(OWNER, &request, now, true) {
                    self.drain(pending);
                }
            }
            Input::ControllerTouch { .. } => {
                if let Some((request, pointer)) = self.touch_request(event)
                    && let Ok(pending) = self.state.submit_touch_state(OWNER, &request, now)
                {
                    self.drain(pending);
                    match request.event_type {
                        5 => self.pointers.clear(),
                        2 | 4 => {
                            self.pointers.remove(&pointer);
                        }
                        _ => {
                            self.pointers.insert(pointer, request.contact_index);
                        }
                    }
                }
            }
            Input::Battery { state, percent, .. } => {
                let request = BatteryStateRequest {
                    header: RequestHeader::new(BatteryStateRequest::WIRE_SIZE as u32),
                    controller_id: self.id,
                    percent: *percent,
                    flags: *state,
                    reserved: 0,
                };
                if let Ok(pending) = self.state.submit_battery_state(OWNER, &request, now) {
                    self.drain(pending);
                }
            }
            Input::SteamAttributes(attributes) if self.profile == Profile::SteamController => {
                self.set_attributes(attributes);
            }
            _ => {}
        }
    }
    fn period(&self) -> Option<Duration> {
        match self.profile {
            Profile::SwitchPro => Some(SWITCH_PERIOD),
            Profile::SteamController => Some(STEAM_PERIOD),
            _ => None,
        }
    }
    fn tick(&mut self) {
        let now = self.now();
        match self.profile {
            Profile::SwitchPro => {
                // The controller reports its state at its own pace, changed or not.
                let last = self.last;
                self.submit(last);
            }
            _ => {
                self.show_hold();
                let (pending, count, ..) = self.state.sc26_keepalive_collect(now);
                for report in pending.into_iter().take(count).flatten() {
                    self.drain(Some(report));
                }
                self.synthesized_rumble();
            }
        }
    }
    fn take_reports(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.reports)
    }
    fn take_feedback(&mut self) -> Vec<(u16, Vec<u8>)> {
        std::mem::take(&mut self.feedback)
    }
    fn bound(&self) -> bool {
        self.bound
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(profile: Profile, capabilities: u16) -> VhfModel {
        VhfModel::new(profile, 2, capabilities, [2, 0x52, 1, 2, 0x59, 2], true)
    }
    fn state(buttons: u32, left: u8, sticks: [i16; 4]) -> Input {
        Input::Controller {
            id: 2,
            active: 4,
            buttons,
            left_trigger: left,
            right_trigger: 0,
            sticks,
        }
    }

    #[test]
    fn the_switch_pro_answers_hid_nintendos_start_up() {
        let mut pad = model(Profile::SwitchPro, 0x30);
        // USB handshake, then the subcommand replies the driver waits for.
        pad.output(&[0x80, 0x02]);
        let reply = pad.take_reports();
        assert_eq!(&reply[0][..2], &[0x81, 0x02]);
        let mut command = vec![0u8; 49];
        command[0] = 1;
        command[10] = 0x02; // the controller's info
        pad.output(&command);
        let reply = pad.take_reports();
        assert_eq!((reply[0][0], reply[0][14]), (0x21, 0x02), "{reply:?}");
        assert!(!pad.bound());
        command[10] = SUB_ENABLE_VIBRATION;
        command[11] = 1;
        pad.output(&command);
        assert!(pad.bound());
    }

    #[test]
    fn a_switch_pro_reports_its_state_and_its_rumble_reaches_the_client() {
        let mut pad = model(Profile::SwitchPro, 0x30);
        pad.take_reports();
        pad.apply(&state(0x1000, 0, [0; 4]));
        let reports = pad.take_reports();
        let report = reports.last().unwrap();
        assert_eq!(report[0], 0x30);
        assert_ne!(report[3] & 0x04, 0, "B, the south button");
        // The pad repeats its state on its own, as the real controller does.
        pad.tick();
        assert_eq!(pad.take_reports().len(), 1);
        // A rumble-only output report with the amplitudes hid-nintendo writes.
        let mut rumble = vec![0u8; 10];
        rumble[0] = 0x10;
        rumble[3] = 68 << 1;
        rumble[5] = 0x40;
        rumble[9] = 0x40;
        pad.output(&rumble);
        let feedback = pad.take_feedback();
        assert_eq!(feedback.len(), 1, "{feedback:?}");
        assert_eq!(feedback[0].0, 5);
    }

    #[test]
    fn the_dualshock_4_is_bound_once_the_kernel_read_its_feature_reports() {
        let mut pad = model(Profile::DualShock4, 0x38);
        assert!(!pad.bound());
        let calibration = pad.get_report(ReportType::Feature, 0x02).unwrap();
        assert_eq!((calibration[0], calibration.len()), (2, 37));
        let pairing = pad.get_report(ReportType::Feature, 0x12).unwrap();
        assert_eq!(&pairing[1..7], &[2, 0x52, 1, 2, 0x59, 2]);
        assert!(pad.get_report(ReportType::Feature, 0x77).is_none());
        assert!(!pad.bound());
        pad.get_report(ReportType::Feature, 0xa3).unwrap();
        assert!(pad.bound());
    }

    #[test]
    fn a_dualshock_4_output_report_is_rumble_and_light_for_the_client() {
        let mut pad = model(Profile::DualShock4, 0);
        // Report 5: flags 1 (rumble) and 2 (lightbar), weak and strong motor, colour.
        let mut report = vec![0u8; 32];
        report[0] = 5;
        report[1] = 0x07;
        report[4] = 0x40;
        report[5] = 0x80;
        report[6..9].copy_from_slice(&[1, 2, 3]);
        pad.output(&report);
        let feedback = pad.take_feedback();
        assert_eq!(feedback.len(), 1, "{feedback:?}");
        assert_eq!(feedback[0].0, 5);
        // The same report again changes nothing.
        pad.output(&report);
        assert!(pad.take_feedback().is_empty());
    }

    #[test]
    fn touch_and_motion_reach_a_dualshock_4_report() {
        let mut pad = model(Profile::DualShock4, 0x38);
        pad.apply(&state(0, 0, [0; 4]));
        pad.take_reports();
        pad.apply(&Input::ControllerTouch {
            id: 2,
            event: 1,
            touchpad: 0,
            pointer: 7,
            x: 0.5,
            y: 0.5,
            pressure: 1.0,
        });
        let reports = pad.take_reports();
        assert!(!reports.is_empty());
        assert_eq!(reports.last().unwrap()[0], 1);
        pad.apply(&Input::Motion {
            id: 2,
            kind: 2,
            xyz: [1.0, 0.0, 0.0],
        });
        assert_eq!(pad.take_reports().len(), 1);
        // Lifting the finger frees its contact.
        pad.apply(&Input::ControllerTouch {
            id: 2,
            event: 2,
            touchpad: 0,
            pointer: 7,
            x: 0.5,
            y: 0.5,
            pressure: 0.0,
        });
        assert!(pad.pointers.is_empty());
    }

    #[test]
    fn a_steam_controller_keeps_reporting_and_forwards_haptics() {
        let mut pad = model(Profile::SteamController, LI_CCAP_STEAM_HAPTIC);
        pad.apply(&state(0x1000, 0, [0; 4]));
        let reports = pad.take_reports();
        assert_eq!(
            reports.last().unwrap()[0],
            steam_controller::INPUT_REPORT_ID
        );
        // A haptic report from Steam: the driver queues it and the client gets it.
        let mut haptic = vec![0u8; 10];
        haptic[0] = steam_controller::HAPTIC_RUMBLE_ID;
        haptic[1] = 8;
        pad.output(&haptic);
        assert!(
            pad.take_feedback()
                .iter()
                .any(|f| f.0 == STEAM_HAPTIC_MESSAGE_TYPE),
            "the haptic report is forwarded"
        );
    }
}
