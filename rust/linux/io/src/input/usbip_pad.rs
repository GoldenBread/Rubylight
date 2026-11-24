//! The USB/IP driver that plugs a client's forwarded USB device into the PC. Linux has no
//! backend for it yet (the kernel's `vhci-hcd` is not driven), so no driver is found and
//! nothing can be attached.
use anyhow::{Result, bail};

/// The USB/IP driver for USB forwarding: attaches a client's device that the host relays on
/// a loopback port. None is found on Linux yet.
pub struct UsbipDriver {
    _found: (),
}

impl UsbipDriver {
    /// None when no USB/IP driver is installed: always on Linux, where none is used yet.
    pub fn find() -> Option<Self> {
        None
    }
    /// Attaches `busid`, served on 127.0.0.1:`tcp_port`; returns the hub port. Fails on
    /// Linux, where no USB/IP driver is used yet.
    pub fn attach(&self, tcp_port: u16, busid: &str) -> Result<u32> {
        let _ = (tcp_port, busid);
        Err(crate::unsupported("Attaching a forwarded USB device"))
    }
    /// Unplugs hub `port` and stops the driver attaching the device again. Fails on Linux,
    /// where no USB/IP driver is used yet.
    pub fn detach(&self, port: u32, tcp_port: u16, busid: &str) -> Result<()> {
        let _ = (port, tcp_port, busid);
        Err(crate::unsupported("Detaching a forwarded USB device"))
    }
    /// Stops the driver attaching `busid` from 127.0.0.1:`tcp_port` again, for an attach
    /// whose hub port is unknown. Succeeds: nothing was ever attached, so there is nothing
    /// to stop.
    pub fn stop(&self, tcp_port: u16, busid: &str) -> Result<()> {
        let _ = (tcp_port, busid);
        Ok(())
    }
    /// Whether the connection from `peer` to the USB/IP server at `server` is the driver's.
    /// Never on Linux: no driver connects, so whatever reached the port is another process.
    pub fn connected(peer: std::net::SocketAddr, server: std::net::SocketAddr) -> Result<bool> {
        let (std::net::SocketAddr::V4(_), std::net::SocketAddr::V4(_)) = (peer, server) else {
            bail!("the USB/IP server listens on IPv4 only, not {server} from {peer}");
        };
        Ok(false)
    }
}
