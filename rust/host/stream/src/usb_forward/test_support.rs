//! Test-harness fake VHCI driver for USB/IP forwarding tests.
use super::{Vhci, wire};
use anyhow::{Context, Result, bail};
use std::{
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

/// How a [`FakeVhci`] misbehaves.
#[derive(Default)]
pub(crate) struct Script {
    /// Another process reaches the relay before the driver does.
    pub(crate) stranger: bool,
    /// usbip.exe fails after the driver imported the device.
    pub(crate) attach_fails: bool,
    /// Detaches that fail before one works.
    pub(crate) failed_detaches: usize,
    /// Whether the driver's connection is still open when it detaches.
    pub(crate) watch_driver: bool,
}

pub fn client(allow_usb: bool) -> butterpollo_session::state::Client {
    let mut extra = std::collections::BTreeMap::new();
    if allow_usb {
        extra.insert("allow_usb".to_owned(), serde_json::Value::Bool(true));
    }
    butterpollo_session::state::Client {
        name: "phone".into(),
        cert: "cert-phone".into(),
        uuid: "uuid-phone".into(),
        perm: 0,
        enabled: true,
        extra,
    }
}

/// A driver that imports the device like usbip-win2 does and hands its connection to the test.
pub struct FakeVhci {
    ready: bool,
    script: Script,
    /// How often the install check ran.
    pub(crate) checks: AtomicUsize,
    connections: Mutex<mpsc::Sender<std::net::TcpStream>>,
    /// Local ports of the driver's connections and of a stranger's, and the ones turned away.
    pub(crate) drivers: Mutex<Vec<u16>>,
    strangers: Mutex<Vec<u16>>,
    pub(crate) rejected: Mutex<Vec<u16>>,
    pub(crate) stranger: Mutex<Option<std::net::TcpStream>>,
    watched: Mutex<Option<std::net::TcpStream>>,
    pub(crate) open_at_detach: Mutex<Vec<bool>>,
    failed_detaches: AtomicUsize,
    detached: Mutex<Vec<(u32, String)>>,
    stopped: Mutex<Vec<String>>,
}
impl FakeVhci {
    pub fn new(ready: bool) -> (Arc<Self>, mpsc::Receiver<std::net::TcpStream>) {
        Self::scripted(ready, Script::default())
    }
    pub(crate) fn scripted(
        ready: bool,
        script: Script,
    ) -> (Arc<Self>, mpsc::Receiver<std::net::TcpStream>) {
        let (sender, receiver) = mpsc::channel();
        let vhci = Arc::new(Self {
            ready,
            failed_detaches: AtomicUsize::new(script.failed_detaches),
            script,
            checks: AtomicUsize::default(),
            connections: Mutex::new(sender),
            drivers: Mutex::default(),
            strangers: Mutex::default(),
            rejected: Mutex::default(),
            stranger: Mutex::default(),
            watched: Mutex::default(),
            open_at_detach: Mutex::default(),
            detached: Mutex::default(),
            stopped: Mutex::default(),
        });
        (vhci, receiver)
    }
    pub fn detached(&self) -> Vec<(u32, String)> {
        self.detached.lock().unwrap().clone()
    }
    #[cfg(test)]
    pub(crate) fn stopped(&self) -> Vec<String> {
        self.stopped.lock().unwrap().clone()
    }
    /// Waits up to 5 s for `done`.
    fn wait(what: &str, mut done: impl FnMut() -> bool) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done() {
            if Instant::now() >= deadline {
                bail!("timed out waiting for {what}");
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(())
    }
}
impl Vhci for FakeVhci {
    fn ready(&self) -> bool {
        self.checks.fetch_add(1, Ordering::Relaxed);
        self.ready
    }
    fn is_driver(&self, peer: SocketAddr, relay: SocketAddr) -> Result<bool> {
        assert!(relay.ip().is_loopback());
        let mut known = None;
        Self::wait("a known peer", || {
            let port = peer.port();
            if self.drivers.lock().unwrap().contains(&port) {
                known = Some(true);
            } else if self.strangers.lock().unwrap().contains(&port) {
                self.rejected.lock().unwrap().push(port);
                known = Some(false);
            }
            known.is_some()
        })?;
        Ok(known == Some(true))
    }
    fn attach(&self, tcp_port: u16, busid: &str) -> Result<u32> {
        if self.script.stranger {
            let stranger = std::net::TcpStream::connect((Ipv4Addr::LOCALHOST, tcp_port))?;
            let port = stranger.local_addr()?.port();
            self.strangers.lock().unwrap().push(port);
            *self.stranger.lock().unwrap() = Some(stranger);
            // The driver connects once the relay turned the stranger away.
            Self::wait("the stranger's rejection", || {
                self.rejected.lock().unwrap().contains(&port)
            })?;
        }
        let mut driver = std::net::TcpStream::connect((Ipv4Addr::LOCALHOST, tcp_port))?;
        self.drivers
            .lock()
            .unwrap()
            .push(driver.local_addr()?.port());
        driver.write_all(&wire::import_request(busid).context("busid")?)?;
        let mut head = [0; wire::OP_HEADER_BYTES];
        driver.read_exact(&mut head)?;
        let header = wire::OpHeader::decode(&head);
        if header.status != 0 {
            bail!("import refused: {}", header.status);
        }
        let mut device = [0; wire::DEVICE_BYTES];
        driver.read_exact(&mut device)?;
        if self.script.watch_driver {
            *self.watched.lock().unwrap() = Some(driver.try_clone()?);
        }
        self.connections.lock().unwrap().send(driver)?;
        if self.script.attach_fails {
            bail!("usbip.exe attach did not finish");
        }
        Ok(3)
    }
    fn detach(&self, port: u32, _tcp_port: u16, busid: &str) -> Result<()> {
        if let Some(watched) = &*self.watched.lock().unwrap() {
            watched.set_nonblocking(true)?;
            let open = match watched.peek(&mut [0]) {
                Ok(read) => read > 0,
                Err(error) => error.kind() == std::io::ErrorKind::WouldBlock,
            };
            self.open_at_detach.lock().unwrap().push(open);
        }
        self.detached.lock().unwrap().push((port, busid.to_owned()));
        if self
            .failed_detaches
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |left| {
                left.checked_sub(1)
            })
            .is_ok()
        {
            bail!("usbip.exe detach --port {port} failed");
        }
        Ok(())
    }
    fn stop(&self, _tcp_port: u16, busid: &str) -> Result<()> {
        self.stopped.lock().unwrap().push(busid.to_owned());
        Ok(())
    }
}

/// An installed driver for tests that never attach.
pub fn installed() -> Arc<dyn Vhci> {
    FakeVhci::new(true).0
}
