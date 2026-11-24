//! Virtual Precision Touchpad state machine and USB/IP [`Device`] implementation.

use std::{collections::VecDeque, time::Instant};

use crate::usbip::{Device, Setup, UsbDevice};

use super::descriptors::{
    CAPABILITIES_ID, CERTIFICATION_ID, Contact, FUNCTION_SWITCH_ID, INPUT_MODE_ID,
    INPUT_MODE_TOUCHPAD, MAX_CONTACTS, PTP_CERTIFICATION_BLOB, TOUCH_ID, TOUCH_REPORT_BYTES,
    touchpad_extent, touchpad_report_descriptor,
};
use super::{ENDPOINT, QUEUE_LIMIT, Standard, TOUCHPAD_PRODUCT, VENDOR, hid_class, standard};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Finger {
    client: u32,
    x: u16,
    y: u16,
}

/// The virtual Precision Touchpad: fingers on their slots (the HID contact ids), the click
/// button, what Windows configured and the reports it has not polled yet.
pub struct PrecisionTouchpad {
    extent: (u16, u16),
    size_mm: (u16, u16),
    slots: [Option<Finger>; MAX_CONTACTS],
    button: bool,
    queue: VecDeque<[u8; TOUCH_REPORT_BYTES]>,
    input_mode: u8,
    surface: bool,
    buttons: bool,
    epoch: Instant,
    last_scan: Option<u16>,
    serial: String,
    /// Fingers the packet being applied lifts, reused across packets: the report
    /// lists each once.
    lifted: Vec<(usize, Finger, bool)>,
    /// Contacts that came before Windows switched the device to touchpad input.
    pub early_contacts: u64,
}
impl PrecisionTouchpad {
    /// A touchpad of the client's size (0 for unknown), whose scan times start at `epoch`.
    pub fn new(width_mm: u16, height_mm: u16, serial: &str, epoch: Instant) -> Self {
        Self {
            extent: touchpad_extent(width_mm, height_mm),
            size_mm: (width_mm, height_mm),
            slots: [None; MAX_CONTACTS],
            button: false,
            queue: VecDeque::new(),
            input_mode: 0,
            surface: true,
            buttons: true,
            epoch,
            last_scan: None,
            serial: serial.chars().take(32).collect(),
            lifted: Vec::new(),
            early_contacts: 0,
        }
    }
    /// The size the device was made with, as the client reported it.
    pub fn size_mm(&self) -> (u16, u16) {
        self.size_mm
    }
    /// Whether Windows took the device as a Precision Touchpad (input mode 3).
    pub fn active(&self) -> bool {
        self.input_mode == INPUT_MODE_TOUCHPAD
    }
    /// Fingers down now.
    pub fn fingers(&self) -> usize {
        self.slots.iter().flatten().count()
    }
    /// Applies the contacts of one packet (a frame, or one contact) with the touchpad's
    /// buttons (bit 0 the click) at `now`, and queues the report Windows reads. Contacts the
    /// packet does not name stay where they were. Returns whether a report was queued.
    pub fn apply(&mut self, contacts: &[Contact], buttons: u8, now: Instant) -> bool {
        // Slots in this report: (slot, finger, tip, confidence), emptied per packet.
        self.lifted.clear();
        for contact in contacts.iter().take(MAX_CONTACTS) {
            let slot = self
                .slots
                .iter()
                .position(|s| s.is_some_and(|f| f.client == contact.id));
            match contact.event {
                // Down, or a move of a finger whose down was lost.
                1 | 3 => {
                    let finger = Finger {
                        client: contact.id,
                        x: scale(contact.x, self.extent.0),
                        y: scale(contact.y, self.extent.1),
                    };
                    match slot.or_else(|| self.slots.iter().position(Option::is_none)) {
                        Some(slot) => self.slots[slot] = Some(finger),
                        // A sixth finger has no slot.
                        None => continue,
                    }
                }
                // Up lifts the finger; cancel lifts it as not a touch (a palm).
                2 | 4 => {
                    if let Some(slot) = slot {
                        let mut finger = self.slots[slot].take().unwrap();
                        if contact.x.is_finite() && contact.y.is_finite() && contact.event == 2 {
                            finger.x = scale(contact.x, self.extent.0);
                            finger.y = scale(contact.y, self.extent.1);
                        }
                        self.lifted.push((slot, finger, contact.event == 2));
                    }
                }
                7 => {
                    for slot in 0..MAX_CONTACTS {
                        if let Some(finger) = self.slots[slot].take() {
                            self.lifted.push((slot, finger, false));
                        }
                    }
                }
                // Hover, hover leave and button-only events move no finger.
                _ => {}
            }
        }
        let button = buttons & 1 != 0;
        let button_changed = button != self.button;
        self.button = button;
        if !self.active() {
            self.early_contacts += contacts.len() as u64;
            return false;
        }
        if contacts.is_empty() && !button_changed {
            return false;
        }
        let mut report = [0u8; TOUCH_REPORT_BYTES];
        report[0] = TOUCH_ID;
        let mut count = 0usize;
        if self.surface {
            let down = (0..MAX_CONTACTS)
                .filter_map(|slot| self.slots[slot].map(|f| (slot, f, true, true)));
            let up = self
                .lifted
                .iter()
                .map(|&(slot, finger, confident)| (slot, finger, false, confident));
            for (slot, finger, tip, confident) in up.chain(down).take(MAX_CONTACTS) {
                let at = 1 + count * 5;
                report[at] = u8::from(confident) | u8::from(tip) << 1 | (slot as u8) << 2;
                report[at + 1..at + 3].copy_from_slice(&finger.x.to_le_bytes());
                report[at + 3..at + 5].copy_from_slice(&finger.y.to_le_bytes());
                count += 1;
            }
        }
        if count == 0 && !(button_changed && self.buttons) {
            return false;
        }
        let at = 1 + MAX_CONTACTS * 5;
        let scan = self.scan_time(now);
        report[at..at + 2].copy_from_slice(&scan.to_le_bytes());
        report[at + 2] = count as u8;
        report[at + 3] = u8::from(self.buttons && self.button);
        if self.queue.len() >= QUEUE_LIMIT {
            self.queue.pop_front();
        }
        self.queue.push_back(report);
        true
    }
    /// Lifts every finger and releases the button (the client left or its touchpad went
    /// away). Returns whether a report was queued.
    pub fn release_all(&mut self, now: Instant) -> bool {
        let cancel = Contact {
            event: 7,
            id: 0,
            x: 0.,
            y: 0.,
        };
        self.apply(&[cancel], 0, now)
    }
    /// Scan time in 100 us units from `epoch`, wrapping, and never the same twice in a row.
    fn scan_time(&mut self, now: Instant) -> u16 {
        let ticks = (now.saturating_duration_since(self.epoch).as_micros() / 100) as u64;
        let mut scan = (ticks % 65536) as u16;
        if self.last_scan == Some(scan) {
            scan = scan.wrapping_add(1);
        }
        self.last_scan = Some(scan);
        scan
    }
    pub fn discard(&mut self) {
        self.queue.clear();
    }
    fn current(&mut self) -> Vec<u8> {
        let mut report = [0u8; TOUCH_REPORT_BYTES];
        report[0] = TOUCH_ID;
        report.to_vec()
    }
}

/// A position on the surface (0-1) on a logical axis of `max`.
fn scale(v: f32, max: u16) -> u16 {
    if !v.is_finite() {
        return 0;
    }
    (v.clamp(0.0, 1.0) * f32::from(max)).round() as u16
}

impl Device for PrecisionTouchpad {
    fn describe(&self) -> UsbDevice {
        UsbDevice {
            speed: 2,
            vendor: VENDOR,
            product: TOUCHPAD_PRODUCT,
            release: 0x0100,
            interfaces: vec![(3, 0, 0)],
        }
    }
    fn control(&mut self, setup: &Setup, data: &[u8]) -> Option<Vec<u8>> {
        let descriptor = touchpad_report_descriptor(self.extent.0 / 10, self.extent.1 / 10);
        let strings = [
            "Rubylight",
            "Rubylight Virtual Touchpad",
            self.serial.as_str(),
        ];
        let reply = match standard(setup, TOUCHPAD_PRODUCT, &descriptor, 64, &strings) {
            Standard::Reply(reply) => reply,
            Standard::Stall => return None,
            Standard::Class => match (setup.request_type, setup.request) {
                (0xa1, 0x01) => match (setup.value >> 8, setup.value as u8) {
                    (1, TOUCH_ID) => self.current(),
                    (3, CAPABILITIES_ID) => {
                        // Five contacts; pad type 0, a click pad (its whole surface clicks).
                        vec![CAPABILITIES_ID, MAX_CONTACTS as u8]
                    }
                    (3, CERTIFICATION_ID) => {
                        let mut r = vec![CERTIFICATION_ID];
                        r.extend_from_slice(&PTP_CERTIFICATION_BLOB);
                        r
                    }
                    (3, INPUT_MODE_ID) => vec![INPUT_MODE_ID, self.input_mode],
                    (3, FUNCTION_SWITCH_ID) => vec![
                        FUNCTION_SWITCH_ID,
                        u8::from(self.surface) | u8::from(self.buttons) << 1,
                    ],
                    _ => return None,
                },
                (0x21, 0x09) => match (setup.value >> 8, setup.value as u8) {
                    (3, INPUT_MODE_ID) => {
                        self.input_mode = data.get(1).copied().unwrap_or(0);
                        if !self.active() {
                            self.queue.clear();
                        }
                        vec![]
                    }
                    (3, FUNCTION_SWITCH_ID) => {
                        let switches = data.get(1).copied().unwrap_or(0b11);
                        self.surface = switches & 1 != 0;
                        self.buttons = switches & 2 != 0;
                        vec![]
                    }
                    _ => return None,
                },
                _ => return hid_class(setup),
            },
        };
        Some(reply)
    }
    fn interrupt(&mut self, endpoint: u8) -> Option<Vec<u8>> {
        (endpoint == ENDPOINT)
            .then(|| self.queue.pop_front().map(|r| r.to_vec()))
            .flatten()
    }
    fn interrupt_ready(&self, endpoint: u8) -> bool {
        endpoint == ENDPOINT && !self.queue.is_empty()
    }
}
