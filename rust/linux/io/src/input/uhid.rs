//! The kernel's `/dev/uhid`: a user-space HID device. The process describes a device (a report
//! descriptor and ids), the kernel binds its HID drivers to it as to a USB or Bluetooth device
//! (hid-playstation for a DualSense), and the process then sends input reports and answers the
//! kernel's report requests. Reports a game makes the driver write come back as events.
//! The structures are those of `linux/uhid.h`, packed; a write shorter than the kernel's event
//! is zero-filled by the kernel, so only the used bytes are sent.

use anyhow::{Context, Result};
use std::{fs::File, io::Write, os::fd::AsRawFd, sync::Arc, time::Duration};

const UHID_DESTROY: u32 = 1;
const UHID_START: u32 = 2;
const UHID_STOP: u32 = 3;
const UHID_OPEN: u32 = 4;
const UHID_CLOSE: u32 = 5;
const UHID_OUTPUT: u32 = 6;
const UHID_GET_REPORT: u32 = 9;
const UHID_GET_REPORT_REPLY: u32 = 10;
const UHID_CREATE2: u32 = 11;
const UHID_INPUT2: u32 = 12;
const UHID_SET_REPORT: u32 = 13;
const UHID_SET_REPORT_REPLY: u32 = 14;
/// The most data a report carries.
const DATA_MAX: usize = 4096;
/// `sizeof(struct uhid_event)`: the type and the largest request, `create2` (4372 bytes).
const EVENT_SIZE: usize = 4 + 4372;
/// `BUS_USB`.
pub(super) const BUS_USB: u16 = 3;

/// What a device description names.
pub(super) struct Description<'a> {
    pub(super) name: &'a str,
    pub(super) phys: &'a str,
    pub(super) uniq: &'a str,
    pub(super) bus: u16,
    pub(super) vendor: u32,
    pub(super) product: u32,
    pub(super) version: u32,
    pub(super) descriptor: &'a [u8],
}

/// A report type as `linux/uhid.h` numbers them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReportType {
    Feature,
    Output,
    Input,
}

impl ReportType {
    fn from_raw(raw: u8) -> Self {
        match raw {
            0 => Self::Feature,
            1 => Self::Output,
            _ => Self::Input,
        }
    }
    /// The type as the high byte of a USB HID request's value.
    pub(super) fn usb(self) -> u16 {
        match self {
            Self::Input => 1,
            Self::Output => 2,
            Self::Feature => 3,
        }
    }
}

/// What the kernel tells the device's creator.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Event {
    /// The device is registered with the HID core.
    Start,
    /// The device was removed.
    Stop,
    /// A program opened the device's input or hidraw node.
    Open,
    Close,
    /// A driver wrote an output report; the data starts with the report id.
    Output(Vec<u8>),
    /// A driver wants a report; answer with the same `id`.
    GetReport {
        id: u32,
        number: u8,
        kind: ReportType,
    },
    /// A driver sets a report (feature or output); the data starts with the report id.
    SetReport {
        id: u32,
        number: u8,
        kind: ReportType,
        data: Vec<u8>,
    },
    /// An event this module does not use.
    Other(u32),
}

fn le16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}
fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// Decodes one `struct uhid_event` as read from the device.
pub(super) fn decode(b: &[u8]) -> Option<Event> {
    if b.len() < 4 {
        return None;
    }
    let kind = le32(b, 0);
    let body = |need: usize| (b.len() >= 4 + need).then_some(());
    Some(match kind {
        UHID_START => Event::Start,
        UHID_STOP => Event::Stop,
        UHID_OPEN => Event::Open,
        UHID_CLOSE => Event::Close,
        UHID_OUTPUT => {
            body(DATA_MAX + 3)?;
            let size = usize::from(le16(b, 4 + DATA_MAX)).min(DATA_MAX);
            Event::Output(b[4..4 + size].to_vec())
        }
        UHID_GET_REPORT => {
            body(6)?;
            Event::GetReport {
                id: le32(b, 4),
                number: b[8],
                kind: ReportType::from_raw(b[9]),
            }
        }
        UHID_SET_REPORT => {
            body(8)?;
            let size = usize::from(le16(b, 10)).min(DATA_MAX).min(b.len() - 12);
            Event::SetReport {
                id: le32(b, 4),
                number: b[8],
                kind: ReportType::from_raw(b[9]),
                data: b[12..12 + size].to_vec(),
            }
        }
        other => Event::Other(other),
    })
}

fn put(buffer: &mut [u8], at: usize, bytes: &[u8]) {
    buffer[at..at + bytes.len()].copy_from_slice(bytes);
}

/// `UHID_CREATE2` for `description`.
pub(super) fn encode_create(description: &Description) -> Vec<u8> {
    let mut e = vec![0u8; EVENT_SIZE];
    put(&mut e, 0, &UHID_CREATE2.to_le_bytes());
    // Names and ids are cut to the fields, leaving the last byte of each text a NUL.
    let text = |s: &str, max: usize| s.bytes().take(max - 1).collect::<Vec<_>>();
    put(&mut e, 4, &text(description.name, 128));
    put(&mut e, 4 + 128, &text(description.phys, 64));
    put(&mut e, 4 + 192, &text(description.uniq, 64));
    let size = description.descriptor.len().min(DATA_MAX);
    put(&mut e, 4 + 256, &(size as u16).to_le_bytes());
    put(&mut e, 4 + 258, &description.bus.to_le_bytes());
    put(&mut e, 4 + 260, &description.vendor.to_le_bytes());
    put(&mut e, 4 + 264, &description.product.to_le_bytes());
    put(&mut e, 4 + 268, &description.version.to_le_bytes());
    // The country code, at 272, is 0.
    put(&mut e, 4 + 276, &description.descriptor[..size]);
    e
}

/// `UHID_INPUT2`: one input report, starting with its report id.
pub(super) fn encode_input(report: &[u8]) -> Vec<u8> {
    let size = report.len().min(DATA_MAX);
    let mut e = vec![0u8; 4 + 2 + size];
    put(&mut e, 0, &UHID_INPUT2.to_le_bytes());
    put(&mut e, 4, &(size as u16).to_le_bytes());
    put(&mut e, 6, &report[..size]);
    e
}

/// `UHID_GET_REPORT_REPLY`: the report (starting with its id) or, with `None`, an error.
pub(super) fn encode_get_report_reply(id: u32, report: Option<&[u8]>) -> Vec<u8> {
    let data = report.unwrap_or_default();
    let size = data.len().min(DATA_MAX);
    let mut e = vec![0u8; 4 + 8 + size];
    put(&mut e, 0, &UHID_GET_REPORT_REPLY.to_le_bytes());
    put(&mut e, 4, &id.to_le_bytes());
    // EIO for a request the device does not serve.
    put(
        &mut e,
        8,
        &u16::from(report.is_none()).wrapping_mul(5).to_le_bytes(),
    );
    put(&mut e, 10, &(size as u16).to_le_bytes());
    put(&mut e, 12, &data[..size]);
    e
}

/// `UHID_SET_REPORT_REPLY`.
pub(super) fn encode_set_report_reply(id: u32, accepted: bool) -> Vec<u8> {
    let mut e = vec![0u8; 4 + 6];
    put(&mut e, 0, &UHID_SET_REPORT_REPLY.to_le_bytes());
    put(&mut e, 4, &id.to_le_bytes());
    put(
        &mut e,
        8,
        &u16::from(!accepted).wrapping_mul(5).to_le_bytes(),
    );
    e
}

/// An open `/dev/uhid`, shared by the thread that reads its events and the one that writes
/// reports.
#[derive(Clone)]
pub(super) struct Uhid {
    file: Arc<File>,
}

impl Uhid {
    /// Opens `/dev/uhid`.
    pub(super) fn open() -> Result<Self> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/uhid")
            .context("cannot open /dev/uhid; virtual PlayStation controllers need access to it (a udev rule giving the host's user write access)")?;
        Ok(Self {
            file: Arc::new(file),
        })
    }
    /// A device on an already open file, for tests that do not use the kernel.
    #[cfg(test)]
    pub(super) fn from_file(file: File) -> Self {
        Self {
            file: Arc::new(file),
        }
    }
    /// Sends an encoded request.
    pub(super) fn send(&self, request: &[u8]) -> std::io::Result<()> {
        (&*self.file).write_all(request)
    }
    pub(super) fn create(&self, description: &Description) -> std::io::Result<()> {
        self.send(&encode_create(description))
    }
    pub(super) fn destroy(&self) {
        let _ = self.send(&UHID_DESTROY.to_le_bytes());
    }
    /// The next event, or None when none came within `timeout`. An error means the device is
    /// gone.
    pub(super) fn read(&self, timeout: Duration) -> std::io::Result<Option<Event>> {
        let mut poll = libc::pollfd {
            fd: self.file.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid pollfd.
        let ready = unsafe { libc::poll(&raw mut poll, 1, timeout.as_millis() as i32) };
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            return if error.kind() == std::io::ErrorKind::Interrupted {
                Ok(None)
            } else {
                Err(error)
            };
        }
        if ready == 0 {
            return Ok(None);
        }
        if poll.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            return Err(std::io::Error::other("the uhid device is gone"));
        }
        let mut buffer = vec![0u8; EVENT_SIZE + 16];
        // SAFETY: reads into `buffer`, whose length is passed.
        let n = unsafe {
            libc::read(
                self.file.as_raw_fd(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
            )
        };
        if n < 0 {
            let error = std::io::Error::last_os_error();
            return if error.kind() == std::io::ErrorKind::WouldBlock {
                Ok(None)
            } else {
                Err(error)
            };
        }
        Ok(decode(&buffer[..n as usize]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_has_the_kernels_layout() {
        let e = encode_create(&Description {
            name: "Pad",
            phys: "rubylight/1",
            uniq: "ab:cd",
            bus: BUS_USB,
            vendor: 0x054c,
            product: 0x0ce6,
            version: 0x0100,
            descriptor: &[5, 1, 9, 5],
        });
        assert_eq!(e.len(), 4376);
        assert_eq!(le32(&e, 0), 11);
        assert_eq!(&e[4..7], b"Pad");
        assert_eq!(e[7], 0);
        assert_eq!(&e[132..143], b"rubylight/1");
        assert_eq!(&e[196..201], b"ab:cd");
        assert_eq!(le16(&e, 260), 4, "rd_size");
        assert_eq!(le16(&e, 262), 3, "bus");
        assert_eq!(le32(&e, 264), 0x054c);
        assert_eq!(le32(&e, 268), 0x0ce6);
        assert_eq!(le32(&e, 272), 0x0100);
        assert_eq!(&e[280..284], &[5, 1, 9, 5]);
    }

    #[test]
    fn replies_and_input_have_the_kernels_layout() {
        let input = encode_input(&[1, 2, 3]);
        assert_eq!((le32(&input, 0), le16(&input, 4)), (12, 3));
        assert_eq!(&input[6..], &[1, 2, 3]);
        let reply = encode_get_report_reply(7, Some(&[9, 8]));
        assert_eq!(le32(&reply, 0), 10);
        assert_eq!(le32(&reply, 4), 7);
        assert_eq!((le16(&reply, 8), le16(&reply, 10)), (0, 2));
        assert_eq!(&reply[12..], &[9, 8]);
        assert_eq!(le16(&encode_get_report_reply(7, None), 8), 5);
        let set = encode_set_report_reply(3, true);
        assert_eq!((le32(&set, 0), le32(&set, 4), le16(&set, 8)), (14, 3, 0));
        assert_eq!(le16(&encode_set_report_reply(3, false), 8), 5);
    }

    #[test]
    fn events_are_decoded() {
        assert_eq!(decode(&2u32.to_le_bytes()), Some(Event::Start));
        let mut output = vec![0u8; EVENT_SIZE];
        output[..4].copy_from_slice(&6u32.to_le_bytes());
        output[4..7].copy_from_slice(&[2, 0xaa, 0xbb]);
        output[4 + DATA_MAX..6 + DATA_MAX].copy_from_slice(&3u16.to_le_bytes());
        assert_eq!(decode(&output), Some(Event::Output(vec![2, 0xaa, 0xbb])));
        let mut get = vec![0u8; EVENT_SIZE];
        get[..4].copy_from_slice(&9u32.to_le_bytes());
        get[4..8].copy_from_slice(&42u32.to_le_bytes());
        get[8] = 0x20;
        get[9] = 0;
        assert_eq!(
            decode(&get),
            Some(Event::GetReport {
                id: 42,
                number: 0x20,
                kind: ReportType::Feature
            })
        );
        let mut set = vec![0u8; EVENT_SIZE];
        set[..4].copy_from_slice(&13u32.to_le_bytes());
        set[4..8].copy_from_slice(&5u32.to_le_bytes());
        set[8] = 2;
        set[9] = 1;
        set[10..12].copy_from_slice(&2u16.to_le_bytes());
        set[12..14].copy_from_slice(&[2, 7]);
        assert_eq!(
            decode(&set),
            Some(Event::SetReport {
                id: 5,
                number: 2,
                kind: ReportType::Output,
                data: vec![2, 7]
            })
        );
        assert_eq!(decode(&99u32.to_le_bytes()), Some(Event::Other(99)));
        assert_eq!(decode(&[1]), None);
    }
}
