//! A USB/IP server for one virtual USB device on the loopback interface.
//! A USB/IP client attaches it (on Windows, usbip-win2's signed UDE
//! driver), and the device then looks to the host like one plugged into a
//! USB port, with no driver of ours in the kernel.
//!
//! The protocol is the Linux kernel's (Documentation/usb/usbip_protocol):
//! big-endian `OP_REQ_IMPORT` to attach, then 48-byte `CMD_SUBMIT` and
//! `CMD_UNLINK` headers, answered by `RET_SUBMIT` and `RET_UNLINK`.

mod server;

#[cfg(test)]
use std::io::Read;
use std::{
    collections::VecDeque,
    io::{self, Write},
    net::{Ipv4Addr, Shutdown, TcpListener, TcpStream},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use server::{accept_clients, report};

/// The bus ID the device is exported under.
pub const BUS_ID: &str = "1-1";
const VERSION: u16 = 0x0111;
const OP_REQ_DEVLIST: u16 = 0x8005;
const OP_REQ_IMPORT: u16 = 0x8003;
const CMD_SUBMIT: u32 = 1;
const CMD_UNLINK: u32 = 2;
const RET_SUBMIT: u32 = 3;
const RET_UNLINK: u32 = 4;
const EPIPE: i32 = -32;
const ECONNRESET: i32 = -104;
/// Bound both HID and audio transfer buffers before allocating them.
const MAX_TRANSFER: usize = 65536;
/// How long a client may take to ask for the device once connected.
const IMPORT_WAIT: Duration = Duration::from_secs(5);
/// How often a blocked read looks for [`Export`] being dropped. Windows
/// does not wake a blocked receive when another thread shuts the socket
/// down, so reads time out and look instead.
const READ_SLICE: Duration = Duration::from_millis(100);
/// A client that stops reading is dropped after this long, so the input
/// thread never waits on it.
const WRITE_WAIT: Duration = Duration::from_secs(1);

/// A control request's setup packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Setup {
    pub request_type: u8,
    pub request: u8,
    pub value: u16,
    pub index: u16,
    pub length: u16,
}
impl Setup {
    fn parse(b: &[u8; 8]) -> Self {
        Self {
            request_type: b[0],
            request: b[1],
            value: u16::from_le_bytes([b[2], b[3]]),
            index: u16::from_le_bytes([b[4], b[5]]),
            length: u16::from_le_bytes([b[6], b[7]]),
        }
    }
}

/// What the device list and import replies say about the device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsbDevice {
    /// Linux USB speed: 2 = full speed, 3 = high speed.
    pub speed: u32,
    pub vendor: u16,
    pub product: u16,
    pub release: u16,
    /// Class, subclass and protocol of each interface.
    pub interfaces: Vec<(u8, u8, u8)>,
}
impl UsbDevice {
    /// `usbip_usb_device`: a device in configuration 1 at its declared speed.
    fn encode(&self) -> Vec<u8> {
        let mut b = vec![0u8; 256 + 32];
        let path = format!("/sys/devices/butterpollo/usb1/{BUS_ID}");
        b[..path.len()].copy_from_slice(path.as_bytes());
        b[256..256 + BUS_ID.len()].copy_from_slice(BUS_ID.as_bytes());
        for n in [1u32, 2, self.speed] {
            // bus 1, device 2
            b.extend_from_slice(&n.to_be_bytes());
        }
        for n in [self.vendor, self.product, self.release] {
            b.extend_from_slice(&n.to_be_bytes());
        }
        b.extend_from_slice(&[0, 0, 0, 1, 1, self.interfaces.len() as u8]);
        b
    }
}

/// A virtual USB device. Control requests are answered at once; interrupt
/// IN transfers wait for [`Device::interrupt`] to have data.
pub trait Device: Send + 'static {
    fn describe(&self) -> UsbDevice;
    /// Answers a control request on endpoint 0 with the data to return (empty
    /// for OUT requests); None stalls it. `data` is what an OUT request sent.
    fn control(&mut self, setup: &Setup, data: &[u8]) -> Option<Vec<u8>>;
    /// The next report on an interrupt IN endpoint. None holds that
    /// endpoint's transfers until the client cancels them.
    fn interrupt(&mut self, endpoint: u8) -> Option<Vec<u8>>;
    /// Whether an interrupt IN endpoint has a report now. A real mouse or touchpad NAKs the
    /// host's polls until it has something to say: false keeps the transfer waiting, without a
    /// periodic wake-up, until [`Export::changed`].
    fn interrupt_ready(&self, _endpoint: u8) -> bool {
        true
    }
    fn output(&mut self, _endpoint: u8, _data: &[u8]) -> bool {
        false
    }
    /// The next IN transfer of at most `length` bytes on a non-control
    /// endpoint; by default [`Device::interrupt`]. A device whose replies can
    /// outgrow one transfer (a bulk endpoint) hands them out a transfer at a
    /// time. None holds the transfer as [`Device::interrupt`] does.
    fn transfer_in(&mut self, endpoint: u8, _length: usize) -> Option<Vec<u8>> {
        self.interrupt(endpoint)
    }
    /// Whether held IN transfers are tried again after an accepted OUT
    /// transfer or [`Export::changed`]: true for a device that answers OUT
    /// commands on an IN endpoint, or starts reporting when told to.
    fn retries_held(&self) -> bool {
        false
    }
    /// Maximum packet size and frame alignment for a 1 ms isochronous endpoint.
    fn iso_layout(&self, _endpoint: u8, _input: bool) -> Option<(usize, usize)> {
        None
    }
    /// An audio deadline was missed or its transport disconnected.
    fn iso_discontinuity(&mut self) {}
    /// One validated USB frame, at its scheduled time. IN returns its data;
    /// OUT consumes data and returns an empty vector. None stalls the packet.
    fn iso(
        &mut self,
        _endpoint: u8,
        _input: bool,
        _data: &[u8],
        _length: usize,
    ) -> Option<Vec<u8>> {
        None
    }
}

#[derive(Clone, Debug)]
struct IsoPacket {
    offset: usize,
    length: usize,
}
struct IsoTransfer {
    seqnum: u32,
    endpoint: u8,
    input: bool,
    length: usize,
    packets: Vec<IsoPacket>,
    data: Vec<u8>,
    due: Instant,
    start_frame: u32,
}
impl IsoTransfer {
    fn reply(&self, status: i32, data: &[u8], lengths: &[usize]) -> Vec<u8> {
        let mut b = ret(
            RET_SUBMIT,
            self.seqnum,
            status,
            self.input.then_some(data),
            if status != 0 {
                0
            } else if self.input {
                data.len()
            } else {
                self.length
            },
        );
        b[28..32].copy_from_slice(&self.start_frame.to_be_bytes());
        b[32..36].copy_from_slice(&(self.packets.len() as u32).to_be_bytes());
        b[36..40].copy_from_slice(
            &(if status == 0 {
                0
            } else {
                self.packets.len() as u32
            })
            .to_be_bytes(),
        );
        for (i, p) in self.packets.iter().enumerate() {
            for n in [
                p.offset as u32,
                p.length as u32,
                if status != 0 {
                    0
                } else if self.input {
                    lengths[i] as u32
                } else {
                    p.length as u32
                },
                status as u32,
            ] {
                b.extend_from_slice(&n.to_be_bytes());
            }
        }
        b
    }
}

/// An interrupt IN transfer waiting for data.
#[derive(Clone, Copy, Debug)]
struct Transfer {
    seqnum: u32,
    endpoint: u8,
    length: usize,
}
#[derive(Default)]
struct Link {
    /// Whether the client imported the device and URBs flow.
    imported: bool,
    /// When the client first asked for an IN transfer on a non-control endpoint since it
    /// imported the device: its driver started and reads reports (a HID mouse is read about
    /// 250 ms after the import).
    polled: Option<Instant>,
    /// Transfers on endpoints that report, oldest first.
    pending: VecDeque<Transfer>,
    /// Transfers on endpoints that never report, kept until unlinked.
    held: Vec<Transfer>,
    iso: VecDeque<IsoTransfer>,
    iso_next: [Option<Instant>; 16],
    /// Whether the device changed since its last report.
    changed: bool,
    /// Every waiting interrupt transfer was NAKed ([`Device::interrupt_ready`]) and nothing
    /// changed since: the report thread sleeps until a change or a new transfer.
    idle: bool,
}
impl Link {
    /// Puts held transfers back in line, to be served at once.
    fn retry_held(&mut self) {
        let held = std::mem::take(&mut self.held);
        self.pending.extend(held);
        self.changed = true;
        self.idle = false;
    }
}
struct Shared<D> {
    device: Mutex<D>,
    link: Mutex<Link>,
    /// The connected client's write socket, locked only for socket I/O after `device`
    /// and `link` are dropped so a 1 s blocking TCP write never stalls device mutations.
    writer: Mutex<Option<TcpStream>>,
    wake: Condvar,
    stop: AtomicBool,
    period: Duration,
    epoch: Instant,
    /// How late an OUT transfer may complete and still play its audio.
    late: Duration,
}
impl<D> Shared<D> {
    fn send(&self, b: &[u8]) {
        self.send_batch(std::slice::from_ref(&b));
    }
    fn send_batch<B: AsRef<[u8]>>(&self, replies: &[B]) {
        if replies.is_empty() {
            return;
        }
        let mut writer = self.writer.lock().unwrap();
        let Some(stream) = writer.as_mut() else {
            return;
        };
        for b in replies {
            if stream.write_all(b.as_ref()).is_err() {
                let _ = stream.shutdown(Shutdown::Both);
                *writer = None;
                drop(writer);
                let mut link = self.link.lock().unwrap();
                link.imported = false;
                link.polled = None;
                return;
            }
        }
    }
}

/// Runs first on each of an [`Export`]'s threads; what it returns lives as
/// long as the thread. The Windows host raises their priority with it, as it
/// does its other input threads: the core stays OS-independent.
pub type ThreadStart = fn() -> Box<dyn std::any::Any>;

/// One device served on `127.0.0.1` at [`Export::port`] under [`BUS_ID`].
/// A client that disconnects may attach again. Dropping it disconnects the
/// client and stops the server.
pub struct Export<D: Device> {
    shared: Arc<Shared<D>>,
    port: u16,
    threads: Vec<JoinHandle<()>>,
}
impl<D: Device> Export<D> {
    /// Serves `device`, completing an interrupt transfer at least every
    /// `period` while the client waits for one, and at once after
    /// [`Export::changed`].
    pub fn listen(device: D, period: Duration) -> io::Result<Self> {
        Self::listen_started(device, period, || Box::new(()))
    }
    /// As [`Export::listen`], with `start` run first on the server's threads.
    pub fn listen_started(device: D, period: Duration, start: ThreadStart) -> io::Result<Self> {
        Self::listen_with(device, period, crate::haptics::MAX_AGE, start)
    }
    /// As [`Export::listen_started`], playing the audio of OUT transfers that complete at most
    /// `late` after they were due.
    fn listen_with(
        device: D,
        period: Duration,
        late: Duration,
        start: ThreadStart,
    ) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let shared = Arc::new(Shared {
            device: Mutex::new(device),
            link: Mutex::default(),
            writer: Mutex::new(None),
            wake: Condvar::new(),
            stop: AtomicBool::new(false),
            period,
            epoch: Instant::now(),
            late,
        });
        let mut threads = vec![];
        let accept = shared.clone();
        threads.push(
            std::thread::Builder::new()
                .name("usbip-accept".into())
                .spawn(move || {
                    let _started = start();
                    accept_clients(&accept, &listener)
                })?,
        );
        let tick = shared.clone();
        let ticker = std::thread::Builder::new()
            .name("usbip-reports".into())
            .spawn(move || {
                let _started = start();
                report(&tick)
            });
        let mut export = Self {
            shared,
            port,
            threads,
        };
        export.threads.push(ticker?);
        Ok(export)
    }
    pub fn port(&self) -> u16 {
        self.port
    }
    /// Whether a client has the device attached.
    pub fn imported(&self) -> bool {
        self.shared.link.lock().unwrap().imported
    }
    /// Whether the attached client reads the device's reports: it asked for an interrupt IN
    /// transfer. Until then a report waits unread, so input meant for the device can go
    /// another way.
    pub fn polled(&self) -> bool {
        self.polled_for().is_some()
    }
    /// How long ago the attached client first asked for an interrupt IN transfer; None until
    /// it has.
    pub fn polled_for(&self) -> Option<Duration> {
        let link = self.shared.link.lock().unwrap();
        link.polled.filter(|_| link.imported).map(|at| at.elapsed())
    }
    pub fn with<R>(&self, f: impl FnOnce(&mut D) -> R) -> R {
        f(&mut self.shared.device.lock().unwrap())
    }
    /// Runs `f` on the device and reports the new state without waiting for the
    /// period, taking the device lock once: `changed` reads nothing else from
    /// the device.
    pub fn with_changed<R>(&self, f: impl FnOnce(&mut D) -> R) -> R {
        let (result, retry) = {
            let mut device = self.shared.device.lock().unwrap();
            let result = f(&mut device);
            (result, device.retries_held())
        };
        self.report_change(retry);
        result
    }
    /// Reports the device's new state without waiting for the period.
    pub fn changed(&self) {
        let retry = self.shared.device.lock().unwrap().retries_held();
        self.report_change(retry);
    }
    /// Flags the device as changed and wakes the report thread. The device lock is
    /// released before the link lock is taken, as everywhere: the report thread
    /// holds the link lock while it takes the device's.
    fn report_change(&self, retry: bool) {
        let mut link = self.shared.link.lock().unwrap();
        link.changed = true;
        link.idle = false;
        if retry {
            link.retry_held();
        }
        // The report thread can serve a change only while an interrupt transfer waits
        // to be served, or it NAKed every one it has; with none pending it sleeps
        // until a transfer arrives, and the arrival wakes it, so a notify here
        // would only be a spurious wake.
        let waiting = !link.pending.is_empty();
        drop(link);
        if waiting {
            self.shared.wake.notify_all();
        }
    }
}
impl<D: Device> Drop for Export<D> {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        {
            let mut link = self.shared.link.lock().unwrap();
            link.imported = false;
            link.polled = None;
        }
        if let Some(stream) = self.shared.writer.lock().unwrap().take() {
            let _ = stream.shutdown(Shutdown::Both);
        }
        self.shared.wake.notify_all();
        // Shutdown must not depend on another loopback connection succeeding.
        // An unpark token also wakes an accept worker that has not parked yet.
        for thread in &self.threads {
            thread.thread().unpark();
        }
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

fn be32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(b[at..at + 4].try_into().unwrap())
}
fn op_reply(code: u16, status: u32) -> Vec<u8> {
    let mut b = VERSION.to_be_bytes().to_vec();
    b.extend_from_slice(&code.to_be_bytes());
    b.extend_from_slice(&status.to_be_bytes());
    b
}
/// `RET_SUBMIT` or `RET_UNLINK`: 48 bytes, then any IN data.
fn ret(command: u32, seqnum: u32, status: i32, data: Option<&[u8]>, length: usize) -> Vec<u8> {
    let mut b = Vec::with_capacity(48 + data.map_or(0, <[u8]>::len));
    b.extend_from_slice(&command.to_be_bytes());
    b.extend_from_slice(&seqnum.to_be_bytes());
    b.extend_from_slice(&[0; 12]);
    b.extend_from_slice(&status.to_be_bytes());
    if command == RET_SUBMIT {
        b.extend_from_slice(&(length as u32).to_be_bytes());
        // start frame, number of packets (none: not isochronous), errors
        b.extend_from_slice(&0u32.to_be_bytes());
        b.extend_from_slice(&u32::MAX.to_be_bytes());
        b.extend_from_slice(&0u32.to_be_bytes());
    }
    b.resize(48, 0);
    if let Some(data) = data {
        b.extend_from_slice(data);
    }
    b
}

#[cfg(test)]
mod tests;
