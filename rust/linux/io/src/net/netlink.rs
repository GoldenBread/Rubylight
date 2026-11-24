//! The kernel's routing socket (rtnetlink) as far as the host asks it: the route to one address,
//! and a socket that becomes readable when an interface or an address changes.
use std::{
    io::Read,
    net::IpAddr,
    os::fd::AsRawFd,
    time::Duration,
};

const NLMSG_ERROR: u16 = 2;
const RTM_NEWROUTE: u16 = 24;
const RTM_GETROUTE: u16 = 26;
const RTM_GETLINK: u16 = 18;
const NLM_F_REQUEST: u16 = 1;
const NLM_F_DUMP: u16 = 0x300;
const RTA_DST: u16 = 1;
const RTA_OIF: u16 = 4;
const RTA_GATEWAY: u16 = 5;
const RTA_METRICS: u16 = 8;
const RTA_VIA: u16 = 18;
const RTAX_MTU: u16 = 2;
/// Route types that deliver: to a neighbour or a router (unicast), or to this PC (local).
const RTN_UNICAST: u8 = 1;
const RTN_LOCAL: u8 = 2;
/// Multicast groups: links, IPv4 addresses, IPv6 addresses.
const GROUPS: [i32; 3] = [1, 5, 9];
/// The lengths of `struct nlmsghdr` and `struct rtmsg`.
const HEADER: usize = 16;
const ROUTE: usize = 12;

/// How the kernel routes to one address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Route {
    /// The index of the interface the route leaves by.
    pub(super) interface: u32,
    /// Whether the route goes through a router rather than straight to the address.
    pub(super) gateway: bool,
    /// Whether the route delivers at all (not an unreachable or blackhole one).
    pub(super) delivers: bool,
    /// The route's own MTU, when it has one: a setting of the route, or what the path
    /// reported for this address.
    pub(super) mtu: Option<u32>,
}

fn socket() -> std::io::Result<socket2::Socket> {
    socket2::Socket::new(
        socket2::Domain::from(libc::AF_NETLINK),
        socket2::Type::RAW,
        Some(socket2::Protocol::from(libc::NETLINK_ROUTE)),
    )
}

fn header(length: usize, kind: u16, flags: u16) -> Vec<u8> {
    let mut message = Vec::with_capacity(length);
    message.extend_from_slice(&(length as u32).to_ne_bytes());
    message.extend_from_slice(&kind.to_ne_bytes());
    message.extend_from_slice(&flags.to_ne_bytes());
    // The sequence number; the sender's port stays 0 for the kernel to fill in.
    message.extend_from_slice(&1u32.to_ne_bytes());
    message.extend_from_slice(&0u32.to_ne_bytes());
    message
}

fn attribute(message: &mut Vec<u8>, kind: u16, data: &[u8]) {
    message.extend_from_slice(&((4 + data.len()) as u16).to_ne_bytes());
    message.extend_from_slice(&kind.to_ne_bytes());
    message.extend_from_slice(data);
    message.resize(message.len().next_multiple_of(4), 0);
}

/// The request for the route to `address`; `scope` is a link-local IPv6 address's interface.
fn route_request(address: IpAddr, scope: u32) -> Vec<u8> {
    let (family, bytes): (i32, Vec<u8>) = match address {
        IpAddr::V4(address) => (libc::AF_INET, address.octets().to_vec()),
        IpAddr::V6(address) => (libc::AF_INET6, address.octets().to_vec()),
    };
    let mut message = header(0, RTM_GETROUTE, NLM_F_REQUEST);
    // struct rtmsg: the family and the destination's prefix length, the rest unset.
    message.extend_from_slice(&[family as u8, (bytes.len() * 8) as u8, 0, 0, 0, 0, 0, 0]);
    message.extend_from_slice(&0u32.to_ne_bytes());
    attribute(&mut message, RTA_DST, &bytes);
    if scope != 0 {
        attribute(&mut message, RTA_OIF, &scope.to_ne_bytes());
    }
    let length = (message.len() as u32).to_ne_bytes();
    message[..4].copy_from_slice(&length);
    message
}

fn number<const N: usize>(bytes: &[u8], at: usize) -> Option<[u8; N]> {
    bytes.get(at..at + N)?.try_into().ok()
}

/// The attributes in `bytes`: each one's type and data.
fn attributes(mut bytes: &[u8]) -> Vec<(u16, &[u8])> {
    let mut found = Vec::new();
    while let (Some(length), Some(kind)) = (number(bytes, 0), number(bytes, 2)) {
        let length = usize::from(u16::from_ne_bytes(length));
        if length < 4 || length > bytes.len() {
            break;
        }
        // The top bits flag a nested attribute and its byte order.
        found.push((u16::from_ne_bytes(kind) & 0x3fff, &bytes[4..length]));
        bytes = bytes.get(length.next_multiple_of(4)..).unwrap_or_default();
    }
    found
}

/// The kernel's answer to a [`route_request`].
pub(super) fn parse_route(reply: &[u8]) -> std::io::Result<Route> {
    let malformed = || std::io::Error::other("the kernel's route answer is malformed");
    let length = u32::from_ne_bytes(number(reply, 0).ok_or_else(malformed)?) as usize;
    let kind = u16::from_ne_bytes(number(reply, 4).ok_or_else(malformed)?);
    let message = reply.get(HEADER..length).ok_or_else(malformed)?;
    if kind == NLMSG_ERROR {
        // The error is the kernel's negative error number, such as ENETUNREACH.
        let code = i32::from_ne_bytes(number(message, 0).ok_or_else(malformed)?);
        return Err(std::io::Error::from_raw_os_error(-code));
    }
    if kind != RTM_NEWROUTE || message.len() < ROUTE {
        return Err(malformed());
    }
    let mut route = Route {
        interface: 0,
        gateway: false,
        delivers: matches!(message[7], RTN_UNICAST | RTN_LOCAL),
        mtu: None,
    };
    for (kind, data) in attributes(&message[ROUTE..]) {
        match kind {
            RTA_OIF => route.interface = number(data, 0).map_or(0, u32::from_ne_bytes),
            RTA_GATEWAY | RTA_VIA => route.gateway = true,
            RTA_METRICS => {
                route.mtu = attributes(data)
                    .into_iter()
                    .find(|(kind, _)| *kind == RTAX_MTU)
                    .and_then(|(_, data)| number(data, 0))
                    .map(u32::from_ne_bytes)
            }
            _ => {}
        }
    }
    Ok(route)
}

/// How the kernel routes to `address` now: one question to the routing socket, no network
/// traffic. `scope` names the interface of a link-local IPv6 address, 0 otherwise.
pub(super) fn route(address: IpAddr, scope: u32) -> std::io::Result<Route> {
    let socket = socket()?;
    // The kernel answers at once; the timeout only keeps a broken socket from holding a thread.
    socket.set_read_timeout(Some(Duration::from_secs(1)))?;
    socket.send(&route_request(address, scope))?;
    let mut reply = [0u8; 4096];
    let size = (&socket).read(&mut reply)?;
    parse_route(&reply[..size])
}

/// A socket that becomes readable whenever a link or an IPv4 or IPv6 address of this PC
/// changes. With `initial` the kernel also lists every link there is now, so there is
/// something to read without a change.
pub(super) fn changes(initial: bool) -> std::io::Result<socket2::Socket> {
    let socket = socket()?;
    for group in GROUPS {
        // SAFETY: the socket is open, and the option reads the four bytes of `group`, which
        // outlives the call.
        let joined = unsafe {
            libc::setsockopt(
                socket.as_raw_fd(),
                libc::SOL_NETLINK,
                libc::NETLINK_ADD_MEMBERSHIP,
                (&raw const group).cast(),
                size_of::<i32>() as libc::socklen_t,
            )
        };
        if joined != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    socket.set_nonblocking(true)?;
    if initial {
        // struct ifinfomsg, all unset: every link.
        let mut request = header(HEADER + 16, RTM_GETLINK, NLM_F_REQUEST | NLM_F_DUMP);
        request.resize(HEADER + 16, 0);
        socket.send(&request)?;
    }
    Ok(socket)
}

/// Reads away everything waiting on a [`changes`] socket. False when the socket is broken and
/// will report nothing more.
pub(super) fn drain(socket: &socket2::Socket) -> bool {
    let mut buffer = [0u8; 8192];
    loop {
        match (&*socket).read(&mut buffer) {
            Ok(0) => return false,
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return true,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            // More changes than the socket's buffer held: some were lost, which is a change.
            Err(error) if error.raw_os_error() == Some(libc::ENOBUFS) => {}
            Err(_) => return false,
        }
    }
}
