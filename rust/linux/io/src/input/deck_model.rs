//! The Steam Deck's controller as a model for [`super::uhid_pad`]: the same device the Windows
//! host serves over USB/IP (`butterpollo_input_policy::steam_deck::SteamDeck`, 28de:1205, its
//! controller interface's report descriptor, 64-byte input report and the feature reports Steam
//! and the kernel's hid-steam driver send it). hid-steam reads the serial number and
//! attributes through feature reports while binding, and gives games the "Steam Deck" gamepad
//! with its trackpads, gyro and back grips; Steam writes its rumble as a feature report.

use super::lizard::{Lizard, Phase};
use super::uhid::ReportType;
use super::uhid_pad::{Identity, PadModel};
use butterpollo_input_policy::{
    input::Input,
    steam_deck::{ENDPOINT, PRODUCT, REPORT_PERIOD, SteamDeck, VENDOR},
    usbip::{Device, Setup},
};
use std::time::Duration;

/// The interface the controller's reports are on.
const INTERFACE: u16 = 2;
/// The rumble feedback packet the client takes (Xbox's: low and high motor, trigger motors).
const RUMBLE_KIND: u16 = 4;
/// The Menu (Options) button's bit in the report's first button byte pair: byte 9, bit 6.
const MENU_BYTE: (usize, u8) = (9, 0x40);
/// ID_SET_DEFAULT_DIGITAL_MAPPINGS: what the driver sends when it turns lizard mode on.
const LIZARD_ON: u8 = 0x85;
/// ID_CLEAR_DIGITAL_MAPPINGS: what it sends when it turns lizard mode off, the hold done.
const LIZARD_OFF: u8 = 0x81;

pub(super) struct DeckModel {
    deck: SteamDeck,
    reports: Vec<Vec<u8>>,
    feedback: Vec<(u16, Vec<u8>)>,
    last_rumble: Option<(u16, u16)>,
    /// Serial and attribute requests the kernel's driver has had answered.
    served: u8,
    /// Leaves hid-steam's lizard mode, which holds the gamepad's input back until Options is
    /// held, as a real Deck's player would ([`Lizard`]).
    lizard: Lizard,
    phase: Phase,
    /// Whether the client holds Menu itself.
    menu: bool,
}

impl DeckModel {
    /// A Deck whose serial number is `serial`, which Steam keeps its settings under.
    /// `leave_lizard`: whether the pad leaves hid-steam's lizard mode itself, which a Deck that
    /// Steam takes over (it opens the driver's hidraw node) needs not.
    pub(super) fn new(serial: &str, leave_lizard: bool) -> Self {
        Self {
            deck: SteamDeck::new(serial),
            reports: Vec::new(),
            feedback: Vec::new(),
            last_rumble: None,
            served: 0,
            lizard: Lizard::new(leave_lizard),
            phase: Phase::Idle,
            menu: false,
        }
    }
    fn setup(request_type: u8, request: u8, value: u16, length: u16) -> Setup {
        Setup {
            request_type,
            request,
            value,
            index: INTERFACE,
            length,
        }
    }
    fn report(&mut self) {
        self.phase = self.lizard.update(self.bound(), self.menu);
        if let Some(mut report) = self.deck.interrupt(ENDPOINT) {
            match self.phase {
                Phase::Idle => {}
                Phase::Showing => report[MENU_BYTE.0] |= MENU_BYTE.1,
                // The driver is switching: it sees no report until it has.
                Phase::Silent => return,
            }
            self.reports.push(report);
        }
    }
}

impl PadModel for DeckModel {
    fn identity(&self) -> Identity {
        Identity {
            name: "Valve Software Steam Deck Controller",
            vendor: VENDOR,
            product: PRODUCT,
            version: 0x0100,
            phys: "deck",
            driver: "hid_steam",
        }
    }
    fn descriptor(&mut self) -> Vec<u8> {
        self.deck
            .control(&Self::setup(0x81, 6, 0x2200, 4096), &[])
            .unwrap_or_default()
    }
    fn get_report(&mut self, kind: ReportType, number: u8) -> Option<Vec<u8>> {
        // The controller has no report ids: the kernel's buffer starts with a 0 where the
        // id would be, and the report follows.
        if number != 0 {
            return None;
        }
        let value = (kind.usb() << 8) | u16::from(number);
        let report = self.deck.control(&Self::setup(0xa1, 1, value, 4096), &[])?;
        if kind == ReportType::Feature {
            self.served = self.served.saturating_add(1);
        }
        let mut reply = vec![0];
        reply.extend_from_slice(&report);
        Some(reply)
    }
    fn set_report(&mut self, kind: ReportType, number: u8, data: &[u8]) -> bool {
        if kind != ReportType::Feature || number != 0 {
            return false;
        }
        // The kernel's buffer starts with the (absent) report id.
        let data = data.get(1..).unwrap_or_default();
        let setup = Self::setup(0x21, 9, ReportType::Feature.usb() << 8, data.len() as u16);
        let accepted = self.deck.control(&setup, data).is_some();
        match data.first() {
            Some(&LIZARD_ON) => self.lizard.driver_enabled(),
            // The driver left lizard mode: the reports go on, with Options let go.
            Some(&LIZARD_OFF) => {
                self.lizard.driver_disabled();
                self.report();
            }
            _ => {}
        }
        if let Some(rumble) = self.deck.take_rumble()
            && self.last_rumble.replace(rumble) != Some(rumble)
        {
            let mut packet = rumble.0.to_le_bytes().to_vec();
            packet.extend_from_slice(&rumble.1.to_le_bytes());
            packet.extend_from_slice(&[0; 4]);
            self.feedback.push((RUMBLE_KIND, packet));
        }
        accepted
    }
    fn output(&mut self, _data: &[u8]) {}
    fn apply(&mut self, event: &Input) {
        if let Input::Controller { buttons, .. } = event {
            self.menu = buttons & 0x10 != 0;
        }
        if self.deck.state.apply(event) {
            self.report();
        }
    }
    fn period(&self) -> Option<Duration> {
        Some(REPORT_PERIOD)
    }
    fn tick(&mut self) {
        self.report();
    }
    fn take_reports(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.reports)
    }
    fn take_feedback(&mut self) -> Vec<(u16, Vec<u8>)> {
        std::mem::take(&mut self.feedback)
    }
    fn bound(&self) -> bool {
        // hid-steam reads the serial number and then the attributes.
        self.served >= 2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kernel_reads_the_serial_and_the_attributes_and_steams_rumble_reaches_the_client() {
        let mut pad = DeckModel::new("RUBY1", true);
        assert_eq!(pad.descriptor().len(), 38);
        // hid-steam asks for the serial, then the attributes: a command, then its reply.
        let mut ask = vec![0u8; 65];
        ask[1] = 0xae;
        assert!(pad.set_report(ReportType::Feature, 0, &ask));
        let serial = pad.get_report(ReportType::Feature, 0).unwrap();
        assert_eq!(&serial[..5], &[0, 0xae, 0x14, 0x01, b'R']);
        assert!(!pad.bound());
        ask[1] = 0x83;
        pad.set_report(ReportType::Feature, 0, &ask);
        let attributes = pad.get_report(ReportType::Feature, 0).unwrap();
        assert_eq!(&attributes[..4], &[0, 0x83, 0x2d, 0x01]);
        assert!(pad.bound());
        // A rumble command as hid-steam writes it (the id byte, command, length, intensity,
        // left and right speed, gains), once for the client.
        let mut rumble = vec![0u8; 65];
        rumble[1..12].copy_from_slice(&[0xeb, 9, 0, 0, 0, 0x34, 0x12, 0x78, 0x56, 2, 0]);
        pad.set_report(ReportType::Feature, 0, &rumble);
        assert_eq!(
            pad.take_feedback(),
            vec![(4, vec![0x34, 0x12, 0x78, 0x56, 0, 0, 0, 0])]
        );
        pad.set_report(ReportType::Feature, 0, &rumble);
        assert!(pad.take_feedback().is_empty());
    }

    #[test]
    fn a_state_is_a_report_and_the_deck_reports_on_its_own() {
        let mut pad = DeckModel::new("x", false);
        pad.apply(&Input::Controller {
            id: 0,
            active: 1,
            buttons: 0x1000,
            left_trigger: 0,
            right_trigger: 0,
            sticks: [0; 4],
        });
        let reports = pad.take_reports();
        assert_eq!(reports.len(), 1);
        assert_eq!(&reports[0][..4], &[1, 0, 9, 64]);
        pad.tick();
        assert_eq!(pad.take_reports().len(), 1);
    }
}
