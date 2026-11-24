//! A virtual DualSense on `/dev/uhid`. The Windows host builds the same pad as a USB device
//! (`butterpollo_input_policy::dualsense::DualSense`, which answers a USB host's control and
//! interrupt requests); here the kernel's HID layer is the host: its report descriptor and
//! feature reports come from that device's control answers, its input reports from its interrupt
//! endpoint, and the output reports the kernel's hid-playstation driver writes (rumble, lightbar,
//! adaptive triggers) go in through its output endpoint. hid-playstation turns it into a
//! gamepad, motion sensors and touchpad that games and SDL read as a real DualSense.
//! Whatever the game asks of the pad leaves as the client's feedback, as on Windows.

use super::{
    PadReport,
    feedback::Sink,
    uhid::{self, Description, Event, ReportType, Uhid},
};
use anyhow::{Context, Result, bail};
use butterpollo_input_policy::{
    dualsense::DualSense,
    input::Input,
    usbip::{Device, Setup},
};
use std::{
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU8, AtomicU16, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

pub(super) const VENDOR: u16 = 0x054c;
pub(super) const PRODUCT: u16 = 0x0ce6;
/// The interface the device answers HID requests on.
const HID_INTERFACE: u16 = 3;
/// How long the kernel's driver gets to read the pad's feature reports at creation.
const PROBE: Duration = Duration::from_millis(2500);
/// How long a thread waits in one read of the uhid device.
const POLL: Duration = Duration::from_millis(50);
/// The feature reports hid-playstation reads while binding: calibration, pairing info and
/// firmware info, as bits of [`Shared::served`].
const PROBE_REPORTS: [(u8, u8); 3] = [(5, 1), (9, 2), (0x20, 4)];

/// Whether `/dev/uhid` can be written, so virtual DualSense pads can be made.
pub(super) fn available() -> bool {
    // SAFETY: a valid NUL-terminated path.
    unsafe { libc::access(c"/dev/uhid".as_ptr(), libc::W_OK | libc::R_OK) == 0 }
}

/// Slots of the process's pads: each is in the pad's Bluetooth address, which the kernel's
/// driver requires to be unique among the DualSenses it drives.
static SLOTS: AtomicU16 = AtomicU16::new(0);

/// A pad's place among the process's uhid pads.
pub(super) struct Slot(pub(super) u8);
impl Slot {
    pub(super) fn reserve() -> Result<Self> {
        loop {
            let taken = SLOTS.load(Ordering::Acquire);
            let Some(slot) = (0..16u8).find(|slot| taken & (1 << slot) == 0) else {
                bail!("all 16 virtual HID pad slots are in use");
            };
            if SLOTS
                .compare_exchange(
                    taken,
                    taken | (1 << slot),
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                return Ok(Self(slot));
            }
        }
    }
}
impl Drop for Slot {
    fn drop(&mut self) {
        SLOTS.fetch_and(!(1 << self.0), Ordering::AcqRel);
    }
}

struct Shared {
    id: u16,
    device: Mutex<DualSense>,
    uhid: Uhid,
    sink: Arc<Sink>,
    stop: AtomicBool,
    /// Which of [`PROBE_REPORTS`] the kernel has read.
    served: AtomicU8,
    /// The pad's address as the pairing report gives it: the slot, then this process.
    address: [u8; 3],
}

/// One virtual DualSense.
pub(super) struct HidPad {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    /// The device's `phys` name, with which a test finds its nodes.
    pub(super) phys: String,
    _slot: Slot,
}

impl Shared {
    fn device(&self) -> std::sync::MutexGuard<'_, DualSense> {
        self.device.lock().unwrap_or_else(PoisonError::into_inner)
    }
    /// What the pad answers the kernel's request for a report.
    fn get_report(&self, kind: ReportType, number: u8) -> Option<Vec<u8>> {
        let setup = Setup {
            request_type: 0xa1,
            request: 1,
            value: (kind.usb() << 8) | u16::from(number),
            index: HID_INTERFACE,
            length: 4096,
        };
        let mut report = self.device().control(&setup, &[])?;
        if kind == ReportType::Feature
            && let Some(bit) = PROBE_REPORTS.iter().find(|r| r.0 == number).map(|r| r.1)
        {
            self.served.fetch_or(bit, Ordering::AcqRel);
        }
        if kind == ReportType::Feature && number == 9 && report.len() >= 7 {
            // The pairing report's address, 6 bytes after the id: the slot, "R", this
            // process's id, then the locally administered marker the device ends with.
            report[1] = self.address[0];
            report[3] = self.address[1];
            report[4] = self.address[2];
        }
        Some(report)
    }
    fn set_report(&self, kind: ReportType, number: u8, data: &[u8]) -> bool {
        let setup = Setup {
            request_type: 0x21,
            request: 9,
            value: (kind.usb() << 8) | u16::from(number),
            index: HID_INTERFACE,
            length: data.len() as u16,
        };
        self.device().control(&setup, data).is_some()
    }
    /// Tells the client of the feedback the pad's last output report changed.
    fn forward_feedback(&self) {
        let feedback = self.device().take_feedback();
        for (kind, data) in feedback {
            self.sink.push(PadReport::Feedback {
                id: self.id,
                kind,
                data,
            });
        }
    }
    fn handle(&self, event: Event) {
        match event {
            Event::GetReport { id, number, kind } => {
                let report = self.get_report(kind, number);
                if let Err(error) = self
                    .uhid
                    .send(&uhid::encode_get_report_reply(id, report.as_deref()))
                {
                    tracing::debug!(%error, "uhid report reply failed");
                }
            }
            Event::SetReport {
                id,
                number,
                kind,
                data,
            } => {
                let accepted = self.set_report(kind, number, &data);
                if let Err(error) = self.uhid.send(&uhid::encode_set_report_reply(id, accepted)) {
                    tracing::debug!(%error, "uhid report reply failed");
                }
                self.forward_feedback();
            }
            Event::Output(data) => {
                // Endpoint 3 is the pad's interrupt OUT endpoint.
                self.device().output(3, &data);
                self.forward_feedback();
            }
            _ => {}
        }
    }
    /// Sends the pad's current input report.
    fn send_input(&self, device: &mut DualSense) -> std::io::Result<()> {
        match device.interrupt(4) {
            Some(report) => self.uhid.send(&uhid::encode_input(&report)),
            None => Ok(()),
        }
    }
}

impl HidPad {
    /// Creates pad `id` for the kernel to bind its driver to, with the client's controller
    /// `capabilities`. Returns once the driver has read the pad's feature reports, or after a
    /// moment when no driver asks for them.
    pub(super) fn create(id: u16, capabilities: u16, sink: Arc<Sink>) -> Result<Self> {
        let slot = Slot::reserve()?;
        let uhid = Uhid::open()?;
        let mut device = DualSense::new(slot.0, id);
        device.capabilities(capabilities);
        let pid = std::process::id() as u16;
        let shared = Arc::new(Shared {
            id,
            device: Mutex::new(device),
            uhid,
            sink,
            stop: AtomicBool::new(false),
            served: AtomicU8::new(0),
            address: [slot.0, (pid & 0xff) as u8, (pid >> 8) as u8],
        });
        // The descriptor is what the pad answers a USB host's request for it.
        let descriptor = shared
            .device()
            .control(
                &Setup {
                    request_type: 0x81,
                    request: 6,
                    value: 0x2200,
                    index: HID_INTERFACE,
                    length: 4096,
                },
                &[],
            )
            .context("the DualSense has no report descriptor")?;
        let phys = format!("rubylight-{}/ds{}", std::process::id(), slot.0);
        let reader = shared.clone();
        let thread = std::thread::Builder::new()
            .name(format!("pad {id} uhid"))
            .spawn(move || {
                while !reader.stop.load(Ordering::Acquire) {
                    match reader.uhid.read(POLL) {
                        Ok(Some(event)) => reader.handle(event),
                        Ok(None) => {}
                        Err(error) => {
                            tracing::debug!(%error, "uhid device ended");
                            break;
                        }
                    }
                }
            })
            .context("the uhid thread did not start")?;
        let pad = Self {
            shared,
            thread: Some(thread),
            phys,
            _slot: slot,
        };
        let description = Description {
            name: "Sony Interactive Entertainment DualSense Wireless Controller",
            phys: &pad.phys,
            uniq: "",
            bus: uhid::BUS_USB,
            vendor: u32::from(VENDOR),
            product: u32::from(PRODUCT),
            version: 0x0100,
            descriptor: &descriptor,
        };
        // On failure the pad's drop stops the thread.
        pad.shared
            .uhid
            .create(&description)
            .context("cannot create the virtual DualSense on /dev/uhid")?;
        let wanted: u8 = PROBE_REPORTS.iter().fold(0, |m, r| m | r.1);
        let deadline = Instant::now() + PROBE;
        while pad.shared.served.load(Ordering::Acquire) & wanted != wanted
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(5));
        }
        let served = pad.shared.served.load(Ordering::Acquire);
        if served & wanted != wanted {
            tracing::warn!(
                served,
                "the kernel's DualSense driver did not read the virtual pad's reports; is hid_playstation loaded?"
            );
        }
        pad.apply(&Input::Controller {
            id,
            active: 0,
            buttons: 0,
            left_trigger: 0,
            right_trigger: 0,
            sticks: [0; 4],
        });
        Ok(pad)
    }
    /// Applies a controller state, motion, touch or battery event and sends the new report.
    pub(super) fn apply(&self, event: &Input) {
        let mut device = self.shared.device();
        device.apply(event);
        if let Err(error) = self.shared.send_input(&mut device) {
            tracing::debug!(%error, "virtual DualSense report failed");
        }
    }
}

impl Drop for HidPad {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        self.shared.uhid.destroy();
        if let Some(thread) = self.thread.take() {
            drop(thread.join());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_are_unique_and_returned() {
        let a = Slot::reserve().unwrap();
        let b = Slot::reserve().unwrap();
        assert_ne!(a.0, b.0);
        let taken = a.0;
        drop(a);
        let c = Slot::reserve().unwrap();
        assert!(c.0 == taken || c.0 != b.0);
    }

    fn shared() -> Shared {
        Shared {
            id: 2,
            device: Mutex::new(DualSense::new(3, 2)),
            uhid: Uhid::from_file(
                std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open("/dev/null")
                    .unwrap(),
            ),
            sink: Arc::default(),
            stop: AtomicBool::new(false),
            served: AtomicU8::new(0),
            address: [3, 0x34, 0x12],
        }
    }

    #[test]
    fn the_kernel_reads_the_pads_feature_reports() {
        let shared = shared();
        let calibration = shared.get_report(ReportType::Feature, 5).unwrap();
        assert_eq!((calibration[0], calibration.len()), (5, 41));
        let pairing = shared.get_report(ReportType::Feature, 9).unwrap();
        assert_eq!(pairing[0], 9);
        assert_eq!(&pairing[1..7], &[3, 0x52, 0x34, 0x12, 0x59, 2]);
        assert_eq!(
            shared.get_report(ReportType::Feature, 0x20).unwrap().len(),
            64
        );
        assert_eq!(shared.served.load(Ordering::Relaxed), 7);
        assert!(shared.get_report(ReportType::Feature, 0x77).is_none());
    }

    /// An output report that sets both motors, as hid-playstation writes it.
    fn rumble_report(right: u8, left: u8) -> Vec<u8> {
        let mut report = vec![0u8; 63];
        report[0] = 2;
        report[1] = 1;
        report[3] = right;
        report[4] = left;
        report[39] = 4;
        report
    }

    #[test]
    fn an_output_report_becomes_the_clients_rumble_lights_and_triggers() {
        let shared = shared();
        let mut report = rumble_report(0x40, 0x80);
        report[2] = 4; // the lightbar
        report[45..48].copy_from_slice(&[1, 2, 3]);
        report[1] |= 8; // the right trigger's effect
        report[11] = 2;
        report[12] = 9;
        shared.handle(Event::Output(report));
        let sent = shared.sink.take();
        let kinds: Vec<_> = sent
            .iter()
            .map(|r| match r {
                PadReport::Feedback { id, kind, data } => (*id, *kind, data.clone()),
                other => panic!("{other:?}"),
            })
            .collect();
        assert!(
            kinds.contains(&(2, 0x010b, vec![0, 0x80, 0, 0x40])),
            "{kinds:?}"
        );
        assert!(kinds.contains(&(2, 0x5502, vec![1, 2, 3])), "{kinds:?}");
        assert!(
            kinds.iter().any(|k| k.1 == 0x5503 && k.2.len() == 23),
            "{kinds:?}"
        );
        // The same report again changes nothing.
        shared.handle(Event::Output(rumble_report(0x40, 0x80)));
        assert!(shared.sink.take().is_empty());
        // Both motors off.
        shared.handle(Event::Output(rumble_report(0, 0)));
        assert!(shared.sink.take().iter().any(
            |r| matches!(r, PadReport::Feedback { kind: 0x010b, data, .. } if data == &[0; 4])
        ));
    }

    #[test]
    fn sticks_motion_and_touch_are_in_the_input_report() {
        let shared = shared();
        let mut device = shared.device();
        device.capabilities(0x38);
        device.apply(&Input::Controller {
            id: 2,
            active: 4,
            buttons: 0x1000 | 0x8,
            left_trigger: 200,
            right_trigger: 0,
            sticks: [i16::MAX, 0, 0, 0],
        });
        device.apply(&Input::Motion {
            id: 2,
            kind: 2,
            xyz: [1.0, 0.0, -1.0],
        });
        device.apply(&Input::ControllerTouch {
            id: 2,
            event: 1,
            touchpad: 0,
            pointer: 1,
            x: 0.5,
            y: 0.5,
            pressure: 1.0,
        });
        let report = device.interrupt(4).unwrap();
        assert_eq!(report.len(), 64);
        assert_eq!(report[0], 1);
        assert_eq!(report[1], 255, "left stick X");
        assert_eq!(report[5], 200, "left trigger");
        assert_eq!(report[8] & 0xf, 2, "d-pad right");
        assert_ne!(report[8] & 0x20, 0, "cross");
        assert_eq!(
            i16::from_le_bytes([report[16], report[17]]),
            16,
            "gyro pitch"
        );
        assert_eq!(report[33] & 0x80, 0, "a finger on the touchpad");
    }
}
