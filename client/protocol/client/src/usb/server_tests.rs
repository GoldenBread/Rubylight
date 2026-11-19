use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rubylight_protocol::usbip::{self as wire, Base, Header, IsoPacket};

use super::*;
use crate::usb::fake::{self, FakeDevice};
use crate::usb::{Blocking, TcpTunnel};

/// The host's driver end of a tunnel.
pub(crate) struct Driver {
    stream: TcpStream,
    seqnum: u32,
}

/// A connected tunnel and the driver end of it.
pub(crate) fn pipe() -> (Arc<TcpTunnel>, Driver) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (driver, _) = listener.accept().unwrap();
    driver
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    (
        Arc::new(TcpTunnel::new(client).unwrap()),
        Driver {
            stream: driver,
            seqnum: 0,
        },
    )
}

pub(crate) fn setup(request_type: u8, request: u8, value: u16, length: u16) -> [u8; 8] {
    let (value, length) = (value.to_le_bytes(), length.to_le_bytes());
    [
        request_type,
        request,
        value[0],
        value[1],
        0,
        0,
        length[0],
        length[1],
    ]
}

impl Driver {
    /// Imports `busid`: the device, or the reply's status.
    pub(crate) fn import(&mut self, busid: &str) -> Result<wire::Device, u32> {
        self.stream
            .write_all(&wire::import_request(busid).unwrap())
            .unwrap();
        let mut head = [0; wire::OP_HEADER_BYTES];
        self.stream.read_exact(&mut head).unwrap();
        let head = wire::OpHeader::decode(&head);
        assert_eq!(head.code, wire::OP_REP_IMPORT);
        if head.status != 0 {
            return Err(head.status);
        }
        let mut device = [0; wire::DEVICE_BYTES];
        self.stream.read_exact(&mut device).unwrap();
        Ok(wire::Device::decode(&device).unwrap())
    }

    /// Submits a transfer and returns its seqnum.
    pub(crate) fn submit(
        &mut self,
        ep: u32,
        direction: u32,
        length: u32,
        setup: [u8; 8],
        data: &[u8],
        iso: &[IsoPacket],
    ) -> u32 {
        self.seqnum += 1;
        let header = Header::Submit {
            base: Base {
                seqnum: self.seqnum,
                devid: 0x0001_0002,
                direction,
                ep,
            },
            flags: 0,
            length,
            start_frame: 0,
            packets: if iso.is_empty() {
                wire::NOT_ISO
            } else {
                iso.len() as u32
            },
            interval: 1,
            setup,
        };
        let mut message = header.encode().to_vec();
        message.extend_from_slice(data);
        for packet in iso {
            message.extend_from_slice(&packet.encode());
        }
        self.stream.write_all(&message).unwrap();
        self.seqnum
    }

    pub(crate) fn unlink(&mut self, target: u32) -> u32 {
        self.seqnum += 1;
        let header = Header::Unlink {
            base: Base {
                seqnum: self.seqnum,
                devid: 0x0001_0002,
                direction: 0,
                ep: 0,
            },
            unlink: target,
        };
        self.stream.write_all(&header.encode()).unwrap();
        self.seqnum
    }

    /// The next reply, its IN data and its packet descriptors. `inbound` says whether the
    /// transfer it answers is IN, as the driver knows from the seqnum.
    pub(crate) fn reply(&mut self, inbound: bool) -> (Header, Vec<u8>, Vec<IsoPacket>) {
        let mut head = [0; wire::HEADER_BYTES];
        self.stream.read_exact(&mut head).unwrap();
        let header = Header::decode(&head).unwrap();
        let (data, packets) = match header {
            Header::RetSubmit {
                length, packets, ..
            } => (if inbound { length } else { 0 }, wire::iso_packets(packets)),
            _ => (0, 0),
        };
        let mut body = vec![0; data as usize + packets as usize * wire::ISO_PACKET_BYTES];
        self.stream.read_exact(&mut body).unwrap();
        let iso = wire::iso_descriptors(&body, packets).unwrap().collect();
        body.truncate(data as usize);
        (header, body, iso)
    }

    /// Whether the client closed the tunnel.
    pub(crate) fn closed(&mut self) -> bool {
        let mut byte = [0];
        matches!(self.stream.read(&mut byte), Ok(0) | Err(_))
    }
}

fn status_of(header: &Header) -> (u32, i32) {
    match *header {
        Header::RetSubmit { base, status, .. } | Header::RetUnlink { base, status } => {
            (base.seqnum, status)
        }
        other => panic!("not a reply: {other:?}"),
    }
}

fn serving(device: FakeDevice) -> (Driver, std::thread::JoinHandle<Result<(), ServeError>>) {
    let (tunnel, driver) = pipe();
    let served = std::thread::spawn(move || serve(tunnel, Arc::new(Blocking::new(device))));
    (driver, served)
}

#[test]
fn a_served_device_imports_and_carries_control_bulk_and_interrupt_transfers() {
    let device = FakeDevice::new("1-2");
    let (mut driver, served) = serving(device.clone());
    let described = driver.import("1-2").unwrap();
    assert_eq!(
        (
            described.busid.as_str(),
            described.vendor,
            described.product,
            described.speed
        ),
        ("1-2", fake::VENDOR, fake::PRODUCT, 3)
    );

    let get = driver.submit(0, wire::DIR_IN, 64, setup(0x80, 6, 0x0100, 64), &[], &[]);
    let (header, data, _) = driver.reply(true);
    assert_eq!(status_of(&header), (get, 0));
    assert_eq!(data, fake::DEVICE_DESCRIPTOR);
    let set = driver.submit(0, wire::DIR_OUT, 0, setup(0x00, 9, 1, 0), &[], &[]);
    assert_eq!(status_of(&driver.reply(false).0), (set, 0));
    assert_eq!(device.configuration(), 1);
    // A request it does not know stalls.
    let odd = driver.submit(0, wire::DIR_IN, 8, setup(0xc0, 0x77, 0, 8), &[], &[]);
    assert_eq!(status_of(&driver.reply(true).0), (odd, wire::EPIPE));

    // An interrupt IN waits for a report while bulk goes on beside it.
    let report = driver.submit(fake::INTERRUPT as u32, wire::DIR_IN, 8, [0; 8], &[], &[]);
    let block: Vec<u8> = (0..100_000u32).map(|at| at as u8).collect();
    let out = driver.submit(
        fake::BULK as u32,
        wire::DIR_OUT,
        block.len() as u32,
        [0; 8],
        &block,
        &[],
    );
    let (header, data, _) = driver.reply(false);
    assert_eq!(status_of(&header), (out, 0));
    assert!(data.is_empty());
    assert!(matches!(
        header,
        Header::RetSubmit {
            length: 100_000,
            ..
        }
    ));
    let back = driver.submit(fake::BULK as u32, wire::DIR_IN, 200_000, [0; 8], &[], &[]);
    let (header, data, _) = driver.reply(true);
    assert_eq!(status_of(&header), (back, 0));
    assert!(data == block, "the loopback reads back what went out");
    device.push_report(vec![1, 2, 3]);
    let (header, data, _) = driver.reply(true);
    assert_eq!((status_of(&header), data), ((report, 0), vec![1, 2, 3]));

    drop(driver);
    assert!(
        served.join().unwrap().is_ok(),
        "the host closing is a clean end"
    );
    assert_eq!(device.closes(), 1, "and lets go of the device");
}

#[test]
fn an_unlinked_transfer_is_answered_once_and_never_completes() {
    let device = FakeDevice::new("1-2");
    let (mut driver, served) = serving(device.clone());
    driver.import("1-2").unwrap();
    let waiting = driver.submit(fake::INTERRUPT as u32, wire::DIR_IN, 8, [0; 8], &[], &[]);
    // It runs, waiting for a report, when the unlink comes.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while device.seen().is_empty() {
        assert!(
            std::time::Instant::now() < deadline,
            "the transfer never ran"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let unlink = driver.unlink(waiting);
    assert_eq!(
        status_of(&driver.reply(false).0),
        (unlink, wire::ECONNRESET)
    );
    // The report it would have read goes to the next one, and nothing answers the first.
    device.push_report(vec![9]);
    let next = driver.submit(fake::INTERRUPT as u32, wire::DIR_IN, 8, [0; 8], &[], &[]);
    let (header, data, _) = driver.reply(true);
    assert_eq!((status_of(&header), data), ((next, 0), vec![9]));
    // An unlink after the answer cancels nothing.
    let late = driver.unlink(next);
    assert_eq!(status_of(&driver.reply(false).0), (late, 0));
    drop(driver);
    served.join().unwrap().unwrap();
}

#[test]
fn isochronous_in_comes_packed_with_its_descriptors_and_failures_keep_them() {
    let device = FakeDevice::new("1-2");
    let (mut driver, served) = serving(device.clone());
    driver.import("1-2").unwrap();
    let asked: Vec<IsoPacket> = (0..3)
        .map(|at| IsoPacket {
            offset: at * 64,
            length: 64,
            actual_length: 0,
            status: 0,
        })
        .collect();
    let iso = driver.submit(
        fake::ISOCHRONOUS as u32,
        wire::DIR_IN,
        192,
        [0; 8],
        &[],
        &asked,
    );
    let (header, data, packets) = driver.reply(true);
    assert_eq!(status_of(&header), (iso, 0));
    assert!(matches!(
        header,
        Header::RetSubmit {
            length: 12,
            packets: 3,
            ..
        }
    ));
    assert_eq!(data, [0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2]);
    assert!(packets
        .iter()
        .zip(&asked)
        .all(|(got, asked)| got.offset == asked.offset && got.actual_length == 4));

    device.unplug();
    let gone = driver.submit(
        fake::ISOCHRONOUS as u32,
        wire::DIR_IN,
        192,
        [0; 8],
        &[],
        &asked,
    );
    let (header, data, packets) = driver.reply(true);
    assert_eq!(status_of(&header), (gone, wire::ENODEV));
    assert!(data.is_empty());
    assert_eq!(
        packets.len(),
        3,
        "a failure keeps the packets the driver reads"
    );
    assert!(packets
        .iter()
        .all(|p| p.actual_length == 0 && p.status == wire::ENODEV));
    drop(driver);
    served.join().unwrap().unwrap();
}

#[test]
fn another_devices_import_is_refused() {
    let device = FakeDevice::new("1-2");
    let (mut driver, served) = serving(device.clone());
    assert_eq!(driver.import("1-3").err(), Some(1));
    assert!(matches!(
        served.join().unwrap(),
        Err(ServeError::Protocol(_))
    ));
    assert_eq!(device.closes(), 1);
}

/// Answers every transfer with more than it asked for.
struct Greedy;
impl UsbDevice for Greedy {
    fn describe(&self) -> wire::Device {
        wire::Device {
            busid: "1-1".into(),
            ..wire::Device::default()
        }
    }
    fn submit(&self, transfer: Transfer, done: Completion) {
        done.complete(Reply::read(vec![0; transfer.length as usize + 1]));
    }
    fn unlink(&self, _: u32) {}
    fn close(&self) {}
}

#[test]
fn a_reply_that_does_not_fit_its_transfer_closes_the_tunnel() {
    let (tunnel, mut driver) = pipe();
    let served = std::thread::spawn(move || serve(tunnel, Arc::new(Greedy)));
    driver.import("1-1").unwrap();
    driver.submit(1, wire::DIR_IN, 8, [0; 8], &[], &[]);
    assert!(
        driver.closed(),
        "nothing that does not fit reaches the driver"
    );
    assert!(matches!(served.join().unwrap(), Err(ServeError::Device(_))));
}

#[test]
fn a_dropped_completion_fails_its_transfer() {
    struct Forgetful;
    impl UsbDevice for Forgetful {
        fn describe(&self) -> wire::Device {
            wire::Device {
                busid: "1-1".into(),
                ..wire::Device::default()
            }
        }
        fn submit(&self, _: Transfer, done: Completion) {
            drop(done);
        }
        fn unlink(&self, _: u32) {}
        fn close(&self) {}
    }
    let (tunnel, mut driver) = pipe();
    let served = std::thread::spawn(move || serve(tunnel, Arc::new(Forgetful)));
    driver.import("1-1").unwrap();
    let lost = driver.submit(1, wire::DIR_IN, 8, [0; 8], &[], &[]);
    assert_eq!(status_of(&driver.reply(true).0), (lost, wire::ESHUTDOWN));
    drop(driver);
    served.join().unwrap().unwrap();
}

#[test]
fn an_unlink_is_answered_only_once_the_device_stopped_the_transfer() {
    /// Keeps every transfer; stopping one takes a while.
    struct Slow {
        kept: Mutex<Vec<Completion>>,
        stopped: AtomicBool,
    }
    impl UsbDevice for Slow {
        fn describe(&self) -> wire::Device {
            wire::Device {
                busid: "1-1".into(),
                ..wire::Device::default()
            }
        }
        fn submit(&self, _: Transfer, done: Completion) {
            self.kept.lock().unwrap().push(done);
        }
        fn unlink(&self, _: u32) {
            std::thread::sleep(Duration::from_millis(200));
            self.stopped.store(true, Ordering::SeqCst);
        }
        fn close(&self) {
            self.kept.lock().unwrap().clear();
        }
    }
    let device = Arc::new(Slow {
        kept: Mutex::new(Vec::new()),
        stopped: AtomicBool::new(false),
    });
    let (tunnel, mut driver) = pipe();
    let served = {
        let device = device.clone();
        std::thread::spawn(move || serve(tunnel, device))
    };
    driver.import("1-1").unwrap();
    let waiting = driver.submit(1, wire::DIR_IN, 8, [0; 8], &[], &[]);
    let unlink = driver.unlink(waiting);
    assert_eq!(
        status_of(&driver.reply(false).0),
        (unlink, wire::ECONNRESET)
    );
    assert!(
        device.stopped.load(Ordering::SeqCst),
        "the driver hears only once the device let go of it"
    );
    drop(driver);
    served.join().unwrap().unwrap();
}

#[test]
fn a_blocking_unlink_waits_for_the_slice_that_still_runs() {
    /// Sees the cancel flag only after a slice that takes a report anyway, as libusb's
    /// synchronous read does.
    struct Late {
        started: AtomicBool,
        took: AtomicBool,
    }
    impl crate::usb::BlockingDevice for Late {
        fn describe(&self) -> wire::Device {
            wire::Device {
                busid: "1-1".into(),
                ..wire::Device::default()
            }
        }
        fn transfer(&self, _: &Transfer, cancelled: &AtomicBool) -> Reply {
            self.started.store(true, Ordering::SeqCst);
            while !cancelled.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
            // The slice that was running when the flag went up ends with data.
            std::thread::sleep(Duration::from_millis(200));
            self.took.store(true, Ordering::SeqCst);
            Reply::read(vec![1])
        }
    }
    let device = Arc::new(Blocking::new(Late {
        started: AtomicBool::new(false),
        took: AtomicBool::new(false),
    }));
    let (tunnel, mut driver) = pipe();
    let served = {
        let device = device.clone();
        std::thread::spawn(move || serve(tunnel, device))
    };
    driver.import("1-1").unwrap();
    let waiting = driver.submit(1, wire::DIR_IN, 8, [0; 8], &[], &[]);
    // Unlinked once it runs, so the cancel flag is what stops it.
    while !device.device().started.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(5));
    }
    let unlink = driver.unlink(waiting);
    assert_eq!(
        status_of(&driver.reply(false).0),
        (unlink, wire::ECONNRESET)
    );
    assert!(
        device.device().took.load(Ordering::SeqCst),
        "the unlink is answered only after the transfer took its last data"
    );
    drop(driver);
    served.join().unwrap().unwrap();
}

/// Keeps every transfer until the test completes it.
#[derive(Default)]
struct Keeper {
    kept: Mutex<Vec<Completion>>,
}
impl UsbDevice for Keeper {
    fn describe(&self) -> wire::Device {
        wire::Device {
            busid: "1-1".into(),
            ..wire::Device::default()
        }
    }
    fn submit(&self, _: Transfer, done: Completion) {
        self.kept.lock().unwrap().push(done);
    }
    fn unlink(&self, _: u32) {}
    fn close(&self) {
        self.kept.lock().unwrap().clear();
    }
}

fn keeping() -> (
    Arc<Keeper>,
    Driver,
    std::thread::JoinHandle<Result<(), ServeError>>,
) {
    let device = Arc::new(Keeper::default());
    let (tunnel, mut driver) = pipe();
    let served = {
        let device = device.clone();
        std::thread::spawn(move || serve(tunnel, device))
    };
    driver.import("1-1").unwrap();
    (device, driver, served)
}

#[test]
fn a_driver_that_queues_too_many_transfers_hears_enomem_until_one_completes() {
    let (device, mut driver, served) = keeping();
    for _ in 0..MAX_IN_FLIGHT {
        driver.submit(1, wire::DIR_IN, 8, [0; 8], &[], &[]);
    }
    let over = driver.submit(1, wire::DIR_IN, 8, [0; 8], &[], &[]);
    assert_eq!(status_of(&driver.reply(true).0), (over, wire::ENOMEM));
    assert_eq!(device.kept.lock().unwrap().len(), MAX_IN_FLIGHT);

    let first = device.kept.lock().unwrap().remove(0);
    let seqnum = first.seqnum();
    first.complete(Reply::read(vec![5]));
    let (header, data, _) = driver.reply(true);
    assert_eq!((status_of(&header), data), ((seqnum, 0), vec![5]));
    // Room for one: it reaches the device, and the one after is refused again.
    driver.submit(1, wire::DIR_IN, 8, [0; 8], &[], &[]);
    let again = driver.submit(1, wire::DIR_IN, 8, [0; 8], &[], &[]);
    assert_eq!(status_of(&driver.reply(true).0), (again, wire::ENOMEM));
    assert_eq!(device.kept.lock().unwrap().len(), MAX_IN_FLIGHT);
    drop(driver);
    served.join().unwrap().unwrap();
}

#[test]
fn transfers_that_would_buffer_too_much_together_hear_enomem_until_one_is_unlinked() {
    let (device, mut driver, served) = keeping();
    let whole = (MAX_BUFFERED / u64::from(wire::MAX_TRANSFER)) as usize;
    let mut asked = Vec::new();
    for _ in 0..whole {
        asked.push(driver.submit(1, wire::DIR_IN, wire::MAX_TRANSFER, [0; 8], &[], &[]));
    }
    let over = driver.submit(1, wire::DIR_IN, 1, [0; 8], &[], &[]);
    assert_eq!(status_of(&driver.reply(true).0), (over, wire::ENOMEM));
    assert_eq!(device.kept.lock().unwrap().len(), whole);

    let unlink = driver.unlink(asked[0]);
    assert_eq!(
        status_of(&driver.reply(false).0),
        (unlink, wire::ECONNRESET)
    );
    // The unlinked one's bytes are free again.
    driver.submit(1, wire::DIR_IN, wire::MAX_TRANSFER, [0; 8], &[], &[]);
    let again = driver.submit(1, wire::DIR_IN, 1, [0; 8], &[], &[]);
    assert_eq!(status_of(&driver.reply(true).0), (again, wire::ENOMEM));
    assert_eq!(device.kept.lock().unwrap().len(), whole + 1);
    drop(driver);
    served.join().unwrap().unwrap();
}

/// A device the kernel serves: [`UsbDevice::relay`] connects to `server`, counting connections.
struct Relayed {
    server: std::net::SocketAddr,
    connects: AtomicUsize,
    closes: AtomicUsize,
}
impl UsbDevice for Relayed {
    fn describe(&self) -> wire::Device {
        wire::Device {
            busid: "1-2".into(),
            vendor: 0x046d,
            ..wire::Device::default()
        }
    }
    fn submit(&self, _: Transfer, _: Completion) {
        panic!("a relayed device takes no transfers");
    }
    fn unlink(&self, _: u32) {}
    fn close(&self) {
        self.closes.fetch_add(1, Ordering::SeqCst);
    }
    fn relay(&self) -> Option<std::io::Result<Arc<dyn Tunnel>>> {
        self.connects.fetch_add(1, Ordering::SeqCst);
        Some(
            TcpStream::connect(self.server)
                .and_then(TcpTunnel::new)
                .map(|t| Arc::new(t) as Arc<dyn Tunnel>),
        )
    }
}

/// The kernel's end: echoes every byte until the client closes, and returns them.
fn kernel() -> (std::net::SocketAddr, std::thread::JoinHandle<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut got = Vec::new();
        let mut buffer = [0; 1024];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    got.extend_from_slice(&buffer[..n]);
                    stream.write_all(&buffer[..n]).unwrap();
                }
            }
        }
        got
    });
    (address, server)
}

fn relaying(
    server: std::net::SocketAddr,
) -> (
    Arc<Relayed>,
    Driver,
    std::thread::JoinHandle<Result<(), ServeError>>,
) {
    let device = Arc::new(Relayed {
        server,
        connects: AtomicUsize::new(0),
        closes: AtomicUsize::new(0),
    });
    let (tunnel, driver) = pipe();
    let served = {
        let device = device.clone();
        std::thread::spawn(move || serve(tunnel, device))
    };
    (device, driver, served)
}

#[test]
fn a_relayed_device_answers_the_import_itself_and_relays_every_byte_after() {
    let (address, server) = kernel();
    let (device, mut driver, served) = relaying(address);
    let imported = driver.import("1-2").unwrap();
    assert_eq!(imported.vendor, 0x046d, "the device as it describes itself");
    driver.stream.write_all(b"urb bytes").unwrap();
    let mut echoed = [0; 9];
    driver.stream.read_exact(&mut echoed).unwrap();
    assert_eq!(&echoed, b"urb bytes");
    drop(driver);
    served.join().unwrap().unwrap();
    assert_eq!(
        server.join().unwrap(),
        b"urb bytes",
        "the kernel sees only what follows the import"
    );
    assert_eq!(device.closes.load(Ordering::SeqCst), 1);
}

#[test]
fn a_relayed_import_of_another_device_never_reaches_the_kernel() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (device, mut driver, served) = relaying(listener.local_addr().unwrap());
    assert_eq!(driver.import("1-3").err(), Some(1));
    assert!(matches!(
        served.join().unwrap(),
        Err(ServeError::Protocol(_))
    ));
    assert_eq!(device.connects.load(Ordering::SeqCst), 0);
    assert_eq!(device.closes.load(Ordering::SeqCst), 1);
}

#[test]
fn a_relay_that_cannot_connect_refuses_the_import() {
    let address = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let (device, mut driver, served) = relaying(address);
    assert_eq!(driver.import("1-2").err(), Some(1));
    assert!(matches!(served.join().unwrap(), Err(ServeError::Device(_))));
    assert_eq!(device.closes.load(Ordering::SeqCst), 1);
}

#[test]
fn the_kernel_closing_ends_the_relay_and_closes_the_tunnel() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    // The device went away: the kernel closes the connection at once.
    let server = std::thread::spawn(move || drop(listener.accept().unwrap()));
    let (device, mut driver, served) = relaying(address);
    driver.import("1-2").unwrap();
    server.join().unwrap();
    assert!(driver.closed(), "the host sees the tunnel close");
    served.join().unwrap().unwrap();
    assert_eq!(device.closes.load(Ordering::SeqCst), 1);
}

/// A tunnel whose writes wait while `held`, as a PC that stops reading leaves them.
struct Held {
    inner: Arc<TcpTunnel>,
    held: Mutex<bool>,
    released: std::sync::Condvar,
    waiting: AtomicUsize,
    closed: AtomicBool,
}
impl Tunnel for Held {
    fn read(&self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(bytes)
    }
    fn write_all(&self, bytes: &[u8]) -> std::io::Result<()> {
        let mut held = self.held.lock().unwrap();
        if *held {
            self.waiting.fetch_add(1, Ordering::SeqCst);
            while *held && !self.closed.load(Ordering::SeqCst) {
                held = self.released.wait(held).unwrap();
            }
        }
        drop(held);
        if self.closed.load(Ordering::SeqCst) {
            return Err(std::io::ErrorKind::BrokenPipe.into());
        }
        self.inner.write_all(bytes)
    }
    fn close(&self) {
        // A closed connection fails a held write, as a shutdown fails a blocked one.
        self.closed.store(true, Ordering::SeqCst);
        self.inner.close();
        let _held = self.held.lock().unwrap();
        self.released.notify_all();
    }
}

impl Held {
    fn new(inner: Arc<TcpTunnel>) -> Arc<Self> {
        Arc::new(Self { inner, held: Mutex::new(false), released: std::sync::Condvar::new(), waiting: AtomicUsize::new(0),
            closed: AtomicBool::new(false) })
    }

    fn hold(&self) {
        *self.held.lock().unwrap() = true;
    }

    fn release(&self) {
        *self.held.lock().unwrap() = false;
        self.released.notify_all();
    }
}

/// Waits up to 3 s for `done`.
fn within(done: impl Fn() -> bool) -> bool {
    let started = std::time::Instant::now();
    while !done() {
        if started.elapsed() > Duration::from_secs(3) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    true
}

#[test]
fn an_unlink_answered_while_the_pc_reads_nothing_does_not_stop_the_serve_loop() {
    let device = Arc::new(Keeper::default());
    let (inner, mut driver) = pipe();
    let tunnel = Held::new(inner);
    let served = {
        let (tunnel, device) = (tunnel.clone(), device.clone());
        std::thread::spawn(move || serve(tunnel, device))
    };
    driver.import("1-1").unwrap();
    let first = driver.submit(1, wire::DIR_IN, 8, [0; 8], &[], &[]);
    assert!(within(|| device.kept.lock().unwrap().len() == 1));
    // Nothing waits to go out; the PC stops reading, then gives up on the transfer.
    tunnel.hold();
    let unlink = driver.unlink(first);
    assert!(within(|| tunnel.waiting.load(Ordering::SeqCst) > 0), "the unlink was never answered");
    driver.submit(1, wire::DIR_IN, 8, [0; 8], &[], &[]);
    let took = within(|| device.kept.lock().unwrap().len() == 2);
    tunnel.release();
    assert!(took, "the serve loop waited behind its own answer");
    assert_eq!(status_of(&driver.reply(false).0), (unlink, wire::ECONNRESET));
    drop(driver);
    served.join().unwrap().unwrap();
}

#[test]
fn a_completion_made_inside_submit_while_the_pc_reads_nothing_does_not_stop_the_serve_loop() {
    /// Completes every transfer inside `submit`, on the serve loop's thread.
    #[derive(Default)]
    struct AtOnce {
        submitted: AtomicUsize,
    }
    impl UsbDevice for AtOnce {
        fn describe(&self) -> wire::Device {
            wire::Device {
                busid: "1-1".into(),
                ..wire::Device::default()
            }
        }
        fn submit(&self, _: Transfer, done: Completion) {
            self.submitted.fetch_add(1, Ordering::SeqCst);
            done.complete(Reply::read(vec![3]));
        }
        fn unlink(&self, _: u32) {}
        fn close(&self) {}
    }
    let device = Arc::new(AtOnce::default());
    let (inner, mut driver) = pipe();
    let tunnel = Held::new(inner);
    let served = {
        let (tunnel, device) = (tunnel.clone(), device.clone());
        std::thread::spawn(move || serve(tunnel, device))
    };
    driver.import("1-1").unwrap();
    tunnel.hold();
    let first = driver.submit(1, wire::DIR_IN, 8, [0; 8], &[], &[]);
    assert!(within(|| tunnel.waiting.load(Ordering::SeqCst) > 0), "the completion was never written");
    let second = driver.submit(1, wire::DIR_IN, 8, [0; 8], &[], &[]);
    let took = within(|| device.submitted.load(Ordering::SeqCst) == 2);
    tunnel.release();
    assert!(took, "the serve loop waited behind a completion it made");
    for seqnum in [first, second] {
        let (header, data, _) = driver.reply(true);
        assert_eq!((status_of(&header), data), ((seqnum, 0), vec![3]));
    }
    drop(driver);
    served.join().unwrap().unwrap();
}

#[test]
fn a_completion_whose_write_waits_on_the_pc_does_not_stop_the_serve_loop() {
    let device = Arc::new(Keeper::default());
    let (inner, mut driver) = pipe();
    let tunnel = Held::new(inner);
    let served = {
        let (tunnel, device) = (tunnel.clone(), device.clone());
        std::thread::spawn(move || serve(tunnel, device))
    };
    driver.import("1-1").unwrap();
    let first = driver.submit(1, wire::DIR_IN, 8, [0; 8], &[], &[]);
    let second = driver.submit(1, wire::DIR_IN, 8, [0; 8], &[], &[]);
    let kept = |count: usize| {
        let started = std::time::Instant::now();
        while device.kept.lock().unwrap().len() < count {
            if started.elapsed() > Duration::from_secs(3) {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        true
    };
    assert!(kept(2));
    // The PC stops reading: the first completion's write waits.
    *tunnel.held.lock().unwrap() = true;
    let completion = device.kept.lock().unwrap().remove(0);
    assert_eq!(completion.seqnum(), first);
    let completing = std::thread::spawn(move || completion.complete(Reply::read(vec![1])));
    while tunnel.waiting.load(Ordering::SeqCst) == 0 {
        std::thread::sleep(Duration::from_millis(5));
    }
    // The serve loop still takes a submit, an unlink and a submit after it.
    driver.submit(1, wire::DIR_IN, 8, [0; 8], &[], &[]);
    let unlink = driver.unlink(second);
    driver.submit(1, wire::DIR_IN, 8, [0; 8], &[], &[]);
    let took = kept(3);
    *tunnel.held.lock().unwrap() = false;
    tunnel.released.notify_all();
    assert!(took, "the serve loop waited behind the blocked write");
    completing.join().unwrap();
    // The unlink's answer comes after the completion it followed.
    let (header, data, _) = driver.reply(true);
    assert_eq!((status_of(&header), data), ((first, 0), vec![1]));
    assert_eq!(status_of(&driver.reply(false).0), (unlink, wire::ECONNRESET));
    drop(driver);
    served.join().unwrap().unwrap();
}

/// A served device whose PC reads nothing, with [`MAX_IN_FLIGHT`] answers waiting for it and the
/// serve loop waiting for room to answer an unlink. Returns the transfer still in flight.
type Served = std::thread::JoinHandle<Result<(), ServeError>>;

fn full_queue() -> (Arc<Held>, Driver, Served, Completion) {
    let device = Arc::new(Keeper::default());
    let (inner, mut driver) = pipe();
    let tunnel = Held::new(inner);
    let served = {
        let (tunnel, device) = (tunnel.clone(), device.clone());
        std::thread::spawn(move || serve(tunnel, device))
    };
    driver.import("1-1").unwrap();
    tunnel.hold();
    let first = driver.submit(1, wire::DIR_IN, 8, [0; 8], &[], &[]);
    for _ in 1..MAX_IN_FLIGHT {
        driver.submit(1, wire::DIR_IN, 8, [0; 8], &[], &[]);
    }
    assert!(within(|| device.kept.lock().unwrap().len() == MAX_IN_FLIGHT));
    let mut kept = std::mem::take(&mut *device.kept.lock().unwrap());
    // The writer takes the first answer and waits in its write; the rest queue behind it.
    kept.remove(0).complete(Reply::read(vec![1]));
    assert!(within(|| tunnel.waiting.load(Ordering::SeqCst) > 0));
    for done in kept {
        done.complete(Reply::read(vec![1]));
    }
    // One more is taken, the next refused: its ENOMEM fills the queue.
    driver.submit(1, wire::DIR_IN, 8, [0; 8], &[], &[]);
    assert!(within(|| device.kept.lock().unwrap().len() == 1));
    let in_flight = device.kept.lock().unwrap().remove(0);
    driver.submit(1, wire::DIR_IN, 8, [0; 8], &[], &[]);
    // An unlink of a transfer already answered: its answer would go past the queue's room,
    // so the serve loop waits. Only that unlink can make it wait: the refusal before it fit.
    driver.unlink(first);
    assert!(within(|| in_flight.shared.state().waiting_for_room), "the serve loop never waited for room");
    (tunnel, driver, served, in_flight)
}

/// Waits up to 3 s for `served` to end.
fn ended(served: &Served) -> bool {
    within(|| served.is_finished())
}

#[test]
fn a_tunnel_closed_while_the_serve_loop_waits_for_room_ends_serving() {
    let (tunnel, _driver, served, in_flight) = full_queue();
    assert!(!served.is_finished(), "the serve loop does not wait for room");
    tunnel.close();
    let ended = ended(&served);
    drop(in_flight);
    assert!(ended, "serve never returned");
    assert!(matches!(served.join().unwrap(), Err(ServeError::Io(_))));
}

#[test]
fn a_bad_reply_while_the_serve_loop_waits_for_room_ends_serving() {
    let (tunnel, _driver, served, in_flight) = full_queue();
    assert!(!served.is_finished(), "the serve loop does not wait for room");
    // More data than the 8 bytes asked fails serving and closes the tunnel.
    in_flight.complete(Reply::read(vec![0; 9]));
    let ended = ended(&served);
    tunnel.close();
    assert!(ended, "serve never returned");
    assert!(matches!(served.join().unwrap(), Err(ServeError::Device(_))));
}
