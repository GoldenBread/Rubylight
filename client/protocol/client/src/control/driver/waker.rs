//! Loopback UDP waker that interrupts the control thread's socket poll on demand.

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};

/// Wakes the control thread with a datagram to the control socket; connected to it, so a
/// moved control socket ([`super::ControlHandle::move_to`]) is a reconnect away. Bound to the
/// unspecified address, so it still reaches the control socket after a roam took the old
/// local address away.
pub(super) struct Waker {
    pub(super) socket: UdpSocket,
}

/// Where wakes for the control socket at `enet_local` go. A specific local address keeps its
/// IPv6 scope, which a link-local one needs.
fn wake_target(enet_local: SocketAddr) -> SocketAddr {
    let mut target = enet_local;
    if target.ip().is_unspecified() {
        target.set_ip(match target.ip() {
            IpAddr::V4(_) => Ipv4Addr::LOCALHOST.into(),
            IpAddr::V6(_) => Ipv6Addr::LOCALHOST.into(),
        });
    }
    target
}

/// Where the waker for `target` is bound: no local address, only the family's.
pub(super) fn wake_bind(target: SocketAddr) -> SocketAddr {
    match target {
        SocketAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
        SocketAddr::V6(_) => (Ipv6Addr::UNSPECIFIED, 0).into(),
    }
}

impl Waker {
    pub(super) fn new(enet_local: SocketAddr) -> io::Result<Self> {
        let target = wake_target(enet_local);
        let socket = UdpSocket::bind(wake_bind(target))?;
        socket.set_nonblocking(true)?;
        socket.connect(target)?;
        Ok(Self { socket })
    }

    pub(super) fn retarget(&self, enet_local: SocketAddr) -> io::Result<()> {
        self.socket.connect(wake_target(enet_local))
    }

    pub(super) fn wake(&self) {
        let _ = self.socket.send(&[0]);
    }
}
