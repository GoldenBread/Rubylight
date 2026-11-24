//! A virtual USB HID mouse and a virtual Precision Touchpad, served over USB/IP
//! ([`crate::usbip`]) and attached by usbip-win2's signed driver: Windows sees a mouse or
//! touchpad plugged into a USB port, with no driver of ours in its kernel.
//!
//! The mouse has a relative collection (buttons, 16-bit X and Y, a wheel and a horizontal
//! pan, each with a resolution multiplier) and an absolute one (X and Y on 0-32767), and is polled
//! every millisecond. Its reports reach games as a real mouse's do: raw input from a device,
//! never flagged as injected. Foundation Sunshine's virtual mouse (a UMDF driver,
//! `src/platform/windows/virtual_mouse.cpp`) gave the idea and the per-app mouse mode; this
//! device and its descriptors are our own.
//!
//! The touchpad follows Microsoft's Precision Touchpad HID protocol: five fingers in parallel
//! reports with scan time, contact count and the click button, the device capabilities and
//! certification status feature reports, and the configuration collection through which
//! Windows switches it to touchpad input. A tablet's physical touchpad (Moonlight V+'s and
//! Foundation Sunshine's touchpad packets, 0x55000009 and 0x5500000A) replays on it, so
//! Windows' own gestures work. The certification status blob is the one open-source PTP
//! drivers send, from imbushuo's mac-precision-touchpad (`AmtPtpDeviceSpiKm/Hid.h`, MIT,
//! Copyright 2018 Bingxing Wang; `third-party/licenses/mac-precision-touchpad-LICENSE.txt`).

mod descriptors;
mod touchpad;

#[cfg(test)]
use std::time::Instant;
use std::{collections::VecDeque, time::Duration};

use crate::usbip::{Device, Setup, UsbDevice};

use descriptors::{ABSOLUTE_ID, MULTIPLIER_ID, NOTCH, RELATIVE_ID};
pub use descriptors::{
    ABSOLUTE_MAX, Contact, DEFAULT_SIZE_MM, INPUT_MODE_TOUCHPAD, MAX_CONTACTS,
    MOUSE_REPORT_DESCRIPTOR, PTP_CERTIFICATION_BLOB, WHEEL_MULTIPLIER, touchpad_report_descriptor,
};
pub use touchpad::PrecisionTouchpad;

/// pid.codes' open-source vendor ID.
pub const VENDOR: u16 = 0x1209;
pub const MOUSE_PRODUCT: u16 = 0x52b1;
pub const TOUCHPAD_PRODUCT: u16 = 0x52b2;
/// Both devices' interrupt IN endpoint, polled every millisecond (`bInterval` 1 at full speed).
pub const ENDPOINT: u8 = 1;
/// How often the USB/IP server completes a waiting transfer with no change: never sooner than
/// a change, which completes at once.
pub const POLL: Duration = Duration::from_millis(1);
/// Unsent reports kept for a host that stopped polling; older ones go first.
const QUEUE_LIMIT: usize = 256;

/// The HID mouse's button bits.
pub mod button {
    pub const LEFT: u8 = 0x01;
    pub const RIGHT: u8 = 0x02;
    pub const MIDDLE: u8 = 0x04;
    pub const BACK: u8 = 0x08;
    pub const FORWARD: u8 = 0x10;
}

/// A Moonlight mouse button (1 left, 2 middle, 3 right, 4 X1, 5 X2) as the HID bit.
pub fn button_bit(button: u8) -> Option<u8> {
    Some(match button {
        1 => button::LEFT,
        2 => button::MIDDLE,
        3 => button::RIGHT,
        4 => button::BACK,
        5 => button::FORWARD,
        _ => return None,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MouseReport {
    Relative {
        buttons: u8,
        x: i16,
        y: i16,
        wheel: i8,
        pan: i8,
    },
    /// The absolute collection carries no buttons: they go on the relative one only, so
    /// Windows sees each press once.
    Absolute { x: u16, y: u16 },
}
impl MouseReport {
    fn encode(self) -> Vec<u8> {
        match self {
            Self::Relative {
                buttons,
                x,
                y,
                wheel,
                pan,
            } => {
                let mut r = vec![RELATIVE_ID, buttons];
                r.extend_from_slice(&x.to_le_bytes());
                r.extend_from_slice(&y.to_le_bytes());
                r.extend_from_slice(&[wheel as u8, pan as u8]);
                r
            }
            Self::Absolute { x, y } => {
                let mut r = vec![ABSOLUTE_ID, 0];
                r.extend_from_slice(&x.to_le_bytes());
                r.extend_from_slice(&y.to_le_bytes());
                r
            }
        }
    }
}

/// The virtual mouse: its button state and the reports Windows has not polled yet. Motion
/// and wheel merge into the newest unsent report; a button change always starts a new one,
/// so a click shorter than a poll still reaches Windows as a press and a release.
pub struct HidMouse {
    buttons: u8,
    queue: VecDeque<MouseReport>,
    /// Wheel and pan in 1/120 notches not sent yet.
    scroll: [i32; 2],
    /// The resolution multipliers Windows set, wheel and pan (1 until it does).
    multiplier: [i32; 2],
    serial: String,
}
impl HidMouse {
    pub fn new(serial: &str) -> Self {
        Self {
            buttons: 0,
            queue: VecDeque::new(),
            scroll: [0; 2],
            multiplier: [1; 2],
            serial: serial.chars().take(32).collect(),
        }
    }
    /// The buttons held now.
    pub fn buttons(&self) -> u8 {
        self.buttons
    }
    /// The wheel's (or with `horizontal` the pan's) steps per notch Windows asked for: 1, or
    /// [`WHEEL_MULTIPLIER`].
    pub fn multiplier(&self, horizontal: bool) -> i32 {
        self.multiplier[usize::from(horizontal)]
    }
    fn push(&mut self, report: MouseReport) {
        if self.queue.len() >= QUEUE_LIMIT {
            self.queue.pop_front();
        }
        self.queue.push_back(report);
    }
    /// The newest unsent relative report, made if there is none.
    fn relative(&mut self) -> &mut MouseReport {
        if !matches!(self.queue.back(), Some(MouseReport::Relative { .. })) {
            let buttons = self.buttons;
            self.push(MouseReport::Relative {
                buttons,
                x: 0,
                y: 0,
                wheel: 0,
                pan: 0,
            });
        }
        self.queue.back_mut().unwrap()
    }
    /// Relative motion in mickeys. Returns whether a report is waiting.
    pub fn motion(&mut self, mut dx: i32, mut dy: i32) -> bool {
        while dx != 0 || dy != 0 {
            let MouseReport::Relative { x, y, .. } = self.relative() else {
                unreachable!()
            };
            let take = |sum: &mut i16, rest: &mut i32| {
                let total = (i32::from(*sum) + *rest).clamp(-32767, 32767);
                *rest -= total - i32::from(*sum);
                *sum = total as i16;
            };
            take(x, &mut dx);
            take(y, &mut dy);
            if dx != 0 || dy != 0 {
                // The report is full: the rest goes in the next one.
                let buttons = self.buttons;
                self.push(MouseReport::Relative {
                    buttons,
                    x: 0,
                    y: 0,
                    wheel: 0,
                    pan: 0,
                });
            }
        }
        !self.queue.is_empty()
    }
    /// A button pressed or released (a [`button`] bit). False when nothing changed.
    pub fn button(&mut self, bit: u8, down: bool) -> bool {
        let buttons = if down {
            self.buttons | bit
        } else {
            self.buttons & !bit
        };
        if buttons == self.buttons {
            return false;
        }
        self.buttons = buttons;
        self.push(MouseReport::Relative {
            buttons,
            x: 0,
            y: 0,
            wheel: 0,
            pan: 0,
        });
        true
    }
    /// A wheel turn in 1/120 notches (Windows' `WHEEL_DELTA` scale), positive up or right.
    /// Returns whether a report is waiting; parts of a step wait for the rest.
    pub fn scroll(&mut self, amount: i16, horizontal: bool) -> bool {
        let index = usize::from(horizontal);
        // Both have a resolution multiplier, so a touchpad's fine pan stays fine too.
        let step = NOTCH / self.multiplier[index];
        self.scroll[index] += i32::from(amount);
        let mut steps = self.scroll[index] / step;
        self.scroll[index] -= steps * step;
        while steps != 0 {
            let MouseReport::Relative { wheel, pan, .. } = self.relative() else {
                unreachable!()
            };
            let field = if horizontal { pan } else { wheel };
            let total = (i32::from(*field) + steps).clamp(-127, 127);
            steps -= total - i32::from(*field);
            *field = total as i8;
            if steps != 0 {
                let buttons = self.buttons;
                self.push(MouseReport::Relative {
                    buttons,
                    x: 0,
                    y: 0,
                    wheel: 0,
                    pan: 0,
                });
            }
        }
        !self.queue.is_empty()
    }
    /// The pointer at `x`, `y` on 0-[`ABSOLUTE_MAX`] of the absolute collection's surface.
    pub fn absolute(&mut self, x: u16, y: u16) {
        let report = MouseReport::Absolute {
            x: x.min(ABSOLUTE_MAX),
            y: y.min(ABSOLUTE_MAX),
        };
        match self.queue.back_mut() {
            Some(back @ MouseReport::Absolute { .. }) => *back = report,
            _ => self.push(report),
        }
    }
    /// Releases every button; returns whether one was held.
    pub fn release_all(&mut self) -> bool {
        self.scroll = [0; 2];
        if self.buttons == 0 {
            return false;
        }
        self.buttons = 0;
        self.push(MouseReport::Relative {
            buttons: 0,
            x: 0,
            y: 0,
            wheel: 0,
            pan: 0,
        });
        true
    }
    /// Drops reports Windows did not poll, for a device that is not attached.
    pub fn discard(&mut self) {
        self.queue.clear();
    }
}

impl Device for HidMouse {
    fn describe(&self) -> UsbDevice {
        UsbDevice {
            speed: 2,
            vendor: VENDOR,
            product: MOUSE_PRODUCT,
            release: 0x0100,
            interfaces: vec![(3, 0, 0)],
        }
    }
    fn control(&mut self, setup: &Setup, data: &[u8]) -> Option<Vec<u8>> {
        let strings = ["Rubylight", "Rubylight Virtual Mouse", self.serial.as_str()];
        let reply = match standard(setup, MOUSE_PRODUCT, MOUSE_REPORT_DESCRIPTOR, 16, &strings) {
            Standard::Reply(reply) => reply,
            Standard::Stall => return None,
            Standard::Class => match (setup.request_type, setup.request, setup.value) {
                // GET_REPORT: an input report as it stands, or the multiplier.
                (0xa1, 0x01, value) => match (value >> 8, value as u8) {
                    (1, RELATIVE_ID) => MouseReport::Relative {
                        buttons: self.buttons,
                        x: 0,
                        y: 0,
                        wheel: 0,
                        pan: 0,
                    }
                    .encode(),
                    (1, ABSOLUTE_ID) => MouseReport::Absolute { x: 0, y: 0 }.encode(),
                    (3, MULTIPLIER_ID) => vec![
                        MULTIPLIER_ID,
                        u8::from(self.multiplier[0] > 1) | u8::from(self.multiplier[1] > 1) << 2,
                    ],
                    _ => return None,
                },
                // SET_REPORT of the multipliers (wheel bits 0-1, pan bits 2-3): logical 1 is
                // physical 8.
                (0x21, 0x09, value) if value == 0x0300 | u16::from(MULTIPLIER_ID) => {
                    let bits = data.get(1).copied().unwrap_or(0);
                    for (axis, shift) in [(0, 0), (1, 2)] {
                        self.multiplier[axis] = if (bits >> shift) & 0x03 >= 1 {
                            WHEEL_MULTIPLIER
                        } else {
                            1
                        };
                    }
                    self.scroll = [0; 2];
                    vec![]
                }
                (0x21, 0x09, _) => return None,
                _ => return hid_class(setup),
            },
        };
        Some(reply)
    }
    fn interrupt(&mut self, endpoint: u8) -> Option<Vec<u8>> {
        (endpoint == ENDPOINT)
            .then(|| self.queue.pop_front().map(MouseReport::encode))
            .flatten()
    }
    fn interrupt_ready(&self, endpoint: u8) -> bool {
        endpoint == ENDPOINT && !self.queue.is_empty()
    }
}

enum Standard {
    Reply(Vec<u8>),
    Stall,
    /// A HID class request for the device to answer.
    Class,
}

/// The standard USB requests of a one-interface HID device whose interrupt IN endpoint 0x81
/// carries reports of up to `packet` bytes, polled every millisecond. `strings` are the
/// manufacturer, product and serial.
fn standard(
    setup: &Setup,
    product: u16,
    report: &[u8],
    packet: u8,
    strings: &[&str; 3],
) -> Standard {
    let hid = {
        let [ll, lh] = (report.len() as u16).to_le_bytes();
        [9, 0x21, 0x11, 0x01, 0, 1, 0x22, ll, lh]
    };
    Standard::Reply(match (setup.request_type, setup.request) {
        // GET_STATUS for the device, an interface or an endpoint.
        (0x80..=0x82, 0x00) => vec![0, 0],
        // CLEAR_FEATURE, SET_FEATURE, SET_ADDRESS, SET_CONFIGURATION, SET_INTERFACE.
        (0x00..=0x02, 0x01 | 0x03) | (0x00, 0x05 | 0x09) | (0x01, 0x0b) => vec![],
        (0x80, 0x08) => vec![1],
        (0x81, 0x0a) => vec![0],
        (0x80, 0x06) => match (setup.value >> 8, setup.value as u8) {
            (1, _) => {
                let [vl, vh] = VENDOR.to_le_bytes();
                let [pl, ph] = product.to_le_bytes();
                // USB 2.0, class per interface, 64-byte EP0, release 1.00, strings 1-3, one
                // configuration.
                vec![
                    18, 1, 0x00, 0x02, 0, 0, 0, 64, vl, vh, pl, ph, 0x00, 0x01, 1, 2, 3, 1,
                ]
            }
            (2, _) => {
                let mut d = vec![9, 2, 0, 0, 1, 1, 0, 0xa0, 50];
                d.extend_from_slice(&[9, 4, 0, 0, 1, 3, 0, 0, 0]);
                d.extend_from_slice(&hid);
                d.extend_from_slice(&[7, 5, 0x80 | ENDPOINT, 3, packet, 0, 1]);
                let total = (d.len() as u16).to_le_bytes();
                d[2..4].copy_from_slice(&total);
                d
            }
            (3, 0) => vec![4, 3, 0x09, 0x04],
            (3, n @ 1..=3) => string_descriptor(strings[usize::from(n) - 1]),
            // No other strings, no device qualifier (a full-speed device), no BOS (USB 2.0).
            _ => return Standard::Stall,
        },
        (0x81, 0x06) => match setup.value >> 8 {
            0x21 => hid.to_vec(),
            0x22 => report.to_vec(),
            _ => return Standard::Stall,
        },
        _ => return Standard::Class,
    })
}

/// GET_IDLE, GET_PROTOCOL (report protocol), SET_IDLE and SET_PROTOCOL; anything else stalls.
fn hid_class(setup: &Setup) -> Option<Vec<u8>> {
    match (setup.request_type, setup.request) {
        (0xa1, 0x02) => Some(vec![0]),
        (0xa1, 0x03) => Some(vec![1]),
        (0x21, 0x0a | 0x0b) => Some(vec![]),
        _ => None,
    }
}

fn string_descriptor(s: &str) -> Vec<u8> {
    let mut d = vec![0, 3];
    for unit in s.encode_utf16().take(126) {
        d.extend_from_slice(&unit.to_le_bytes());
    }
    d[0] = d.len() as u8;
    d
}

#[cfg(test)]
mod tests;
