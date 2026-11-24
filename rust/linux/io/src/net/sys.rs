//! The socket calls the senders and the reply socket share on Linux: socket options, `sendmsg`
//! with its control data (UDP segmentation, the source address) and the control data `recvmsg`
//! returns.
use std::{
    io::IoSlice,
    net::{IpAddr, SocketAddr, UdpSocket},
    os::fd::AsRawFd,
};

/// A Linux error number, as a failed send reports it.
pub(super) type Errno = i32;

/// `UDP_SEGMENT`: one `sendmsg` carries several datagrams of this size (Linux 4.18).
const UDP_SEGMENT: i32 = 103;

/// The error number of `error`; `EINVAL` for one the OS did not number.
pub(super) fn errno(error: &std::io::Error) -> Errno {
    error.raw_os_error().unwrap_or(libc::EINVAL)
}

/// Sets an integer socket option.
pub(super) fn set_option(
    socket: &impl AsRawFd,
    level: i32,
    name: i32,
    value: i32,
) -> std::io::Result<()> {
    // SAFETY: the borrowed socket stays open, and the option reads the four bytes of `value`,
    // which outlives the call.
    let result = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            level,
            name,
            (&raw const value).cast(),
            size_of::<i32>() as libc::socklen_t,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Where a control message's level and type are, and where its data begins: `struct cmsghdr`
/// is a `size_t` length and two `int`s, padded as `CMSG_ALIGN` pads it.
const LEVEL: usize = size_of::<usize>();
const KIND: usize = LEVEL + 4;
const HEADER: usize = (KIND + 4).next_multiple_of(ALIGN);
const ALIGN: usize = size_of::<usize>();

/// `sendmsg` control data: whole messages, each padded as `CMSG_SPACE` pads them.
pub(super) struct Ancillary {
    pub(super) bytes: [u8; 64],
    pub(super) len: usize,
}
impl Default for Ancillary {
    fn default() -> Self {
        Self {
            bytes: [0; 64],
            len: 0,
        }
    }
}
impl Ancillary {
    /// Appends one message. The messages this module sends fit together; one that would not
    /// is left out.
    fn push(&mut self, level: i32, kind: i32, data: &[u8]) {
        let size = HEADER + data.len();
        let end = self.len + size.next_multiple_of(ALIGN);
        debug_assert!(end <= self.bytes.len(), "control data too large");
        let Some(message) = self.bytes.get_mut(self.len..end) else {
            return;
        };
        message.fill(0);
        message[..LEVEL].copy_from_slice(&size.to_ne_bytes());
        message[LEVEL..KIND].copy_from_slice(&level.to_ne_bytes());
        message[KIND..KIND + 4].copy_from_slice(&kind.to_ne_bytes());
        message[HEADER..size].copy_from_slice(data);
        self.len = end;
    }
    /// Split the message's bytes into datagrams of `size` bytes, the last maybe shorter.
    pub(super) fn segment(&mut self, size: u16) {
        self.push(libc::SOL_UDP, UDP_SEGMENT, &size.to_ne_bytes());
    }
    /// Send from `source`, as Moonlight's ENet does: `IP_PKTINFO` for IPv4, also on a
    /// dual-stack socket, `IPV6_PKTINFO` for IPv6.
    pub(super) fn source(&mut self, source: IpAddr) {
        match source.to_canonical() {
            IpAddr::V4(source) => {
                // struct in_pktinfo: the interface index (none), the source to send from
                // (ipi_spec_dst) and the header's destination, which only a receive fills.
                let mut info = [0u8; 12];
                info[4..8].copy_from_slice(&source.octets());
                info[8..].copy_from_slice(&source.octets());
                self.push(libc::IPPROTO_IP, libc::IP_PKTINFO, &info);
            }
            IpAddr::V6(source) => {
                // struct in6_pktinfo: the address, then the interface index (none).
                let mut info = [0u8; 20];
                info[..16].copy_from_slice(&source.octets());
                self.push(libc::IPPROTO_IPV6, libc::IPV6_PKTINFO, &info);
            }
        }
    }
}

/// The local address in `recvmsg`'s control data: `IP_PKTINFO` or `IPV6_PKTINFO`.
pub(super) fn local_address(control: &[u8]) -> Option<IpAddr> {
    let number = |bytes: &[u8]| Some(i32::from_ne_bytes(bytes.try_into().ok()?));
    let mut offset = 0;
    while offset + HEADER <= control.len() {
        let message = &control[offset..];
        let length = usize::from_ne_bytes(message[..LEVEL].try_into().ok()?);
        if length < HEADER || length > message.len() {
            return None;
        }
        let level = number(&message[LEVEL..KIND])?;
        let kind = number(&message[KIND..KIND + 4])?;
        let data = &message[HEADER..length];
        if level == libc::IPPROTO_IP && kind == libc::IP_PKTINFO && data.len() >= 12 {
            let address: [u8; 4] = data[8..12].try_into().ok()?;
            return Some(IpAddr::from(address));
        }
        if level == libc::IPPROTO_IPV6 && kind == libc::IPV6_PKTINFO && data.len() >= 20 {
            let address: [u8; 16] = data[..16].try_into().ok()?;
            return Some(IpAddr::from(address).to_canonical());
        }
        offset += length.next_multiple_of(ALIGN);
    }
    None
}

/// One `sendmsg` of `buffers` to `peer` with `control`. The error is the OS's number.
pub(super) fn send_message(
    socket: &UdpSocket,
    buffers: &[IoSlice<'_>],
    peer: &socket2::SockAddr,
    control: &Ancillary,
) -> Result<usize, Errno> {
    let message = socket2::MsgHdr::new()
        .with_addr(peer)
        .with_buffers(buffers)
        .with_control(&control.bytes[..control.len]);
    socket2::SockRef::from(socket)
        .sendmsg(&message, 0)
        .map_err(|error| errno(&error))
}

/// One datagram to `peer`, from `source` when it is known. The error is the OS's number.
pub(super) fn send_from(
    socket: &UdpSocket,
    packet: &[u8],
    peer: SocketAddr,
    source: Option<IpAddr>,
) -> Result<usize, Errno> {
    let Some(source) = source else {
        return socket.send_to(packet, peer).map_err(|error| errno(&error));
    };
    let mut control = Ancillary::default();
    control.source(source);
    send_message(
        socket,
        &[IoSlice::new(packet)],
        &socket2::SockAddr::from(peer),
        &control,
    )
}
