//! Source-pinned UDP `ReplySocket` using `WSARecvMsg` and `IP_PKTINFO`/`IPV6_PKTINFO`.
use super::batch::send_from;
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr, ToSocketAddrs, UdpSocket},
    os::windows::io::{AsRawSocket, AsSocket, BorrowedSocket, RawSocket},
    sync::{
        RwLock,
        atomic::{AtomicBool, Ordering},
    },
};
use windows::Win32::Networking::WinSock::*;

/// Spoofed senders cannot grow a ReplySocket's table without bound; a client
/// still streaming is learned again from its next datagram.
pub(super) const REPLY_SOURCES: usize = 256;

/// A UDP socket that answers each client from the local address the client's
/// datagrams arrived on, as moonlight-common-c's ENet does. Bound to every
/// address, Windows otherwise picks the source by route: on a PC where a VPN
/// such as Tailscale also routes the LAN, replies to a LAN client could leave
/// from the VPN's address, and the client drops them (issue #54).
///
/// Every send through it (`send_to`, `Batch::send`, `send_datagram`) uses
/// what `answer_from` learned for that peer. A peer Windows already routes
/// from the address it reached, as on a plain LAN, is answered as before,
/// with plain sendto and without taking the table's lock while no peer is
/// pinned.
pub struct ReplySocket {
    pub(super) socket: UdpSocket,
    receive_message: unsafe extern "system" fn(
        SOCKET,
        *mut WSAMSG,
        *mut u32,
        *mut windows::Win32::System::IO::OVERLAPPED,
        LPWSAOVERLAPPED_COMPLETION_ROUTINE,
    ) -> i32,
    /// The address Windows sends to a peer from; `routed_source` but in tests.
    pub(super) route: fn(SOCKET, SocketAddr) -> std::io::Result<IpAddr>,
    /// The time; `Instant::now` but in tests.
    pub(super) now: fn() -> std::time::Instant,
    pub(super) sources: RwLock<Sources>,
    /// Whether any peer in `sources` is pinned, so sends skip the lock
    /// while none is.
    pub(super) any_pinned: AtomicBool,
    /// Whether a datagram without its local address was logged.
    missing_reported: AtomicBool,
    /// Whether a failed route lookup was logged.
    route_reported: AtomicBool,
}

/// How often a peer's route is checked again while its datagrams arrive, so
/// a VPN coming up or going down mid-stream is noticed.
pub(super) const ROUTE_CHECK: std::time::Duration = std::time::Duration::from_secs(1);
/// How soon after the last check a failed send has the route checked again.
pub(super) const RECHECK: std::time::Duration = std::time::Duration::from_millis(100);
/// How long an address Windows refused to send to a peer from is not pinned
/// again; a refusal that lasts costs a failed send and its retry this often.
pub(super) const REFUSAL: std::time::Duration = std::time::Duration::from_secs(5);

/// What a ReplySocket learned about one peer.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Reached {
    /// The local address the peer's datagrams arrive on.
    local: IpAddr,
    /// Whether Windows routes replies from another address, so they must be
    /// sent from `local`.
    pinned: bool,
    /// When the route was last checked.
    checked: std::time::Instant,
    /// Whether a send to the peer failed as unreachable since.
    recheck: bool,
}

/// What a ReplySocket learned per peer, each table holding at most
/// REPLY_SOURCES entries and evicting the oldest.
#[derive(Default)]
pub(super) struct Sources {
    /// What each peer last reached, and when it was learned.
    pins: HashMap<SocketAddr, (Reached, u64)>,
    /// The local addresses Windows refused to send to a peer from, and when;
    /// not pinned again for REFUSAL, as a VPN that went down may come back.
    pub(super) refused: HashMap<(SocketAddr, IpAddr), (std::time::Instant, u64)>,
    /// Orders both tables' entries.
    clock: u64,
}
impl Sources {
    /// Inserts into `table`, evicting its oldest entry when full; whether
    /// `key` is new.
    fn remember<K: Copy + Eq + std::hash::Hash, V>(
        table: &mut HashMap<K, (V, u64)>,
        clock: &mut u64,
        key: K,
        value: V,
    ) -> bool {
        if table.len() >= REPLY_SOURCES
            && !table.contains_key(&key)
            && let Some(oldest) = table.iter().min_by_key(|(_, (_, at))| *at).map(|(k, _)| *k)
        {
            table.remove(&oldest);
        }
        *clock += 1;
        table.insert(key, (value, *clock)).is_none()
    }
    /// Whether Windows refused to send to `peer` from `local` less than
    /// REFUSAL before `now`.
    fn refusing(&self, peer: SocketAddr, local: IpAddr, now: std::time::Instant) -> bool {
        self.refused
            .get(&(peer, local))
            .is_some_and(|(at, _)| now.duration_since(*at) < REFUSAL)
    }
    /// Whether a datagram from `peer` to `local` at `now` changes nothing:
    /// the same address with a fresh route, or one refused for it.
    fn settled(&self, peer: SocketAddr, local: Option<IpAddr>, now: std::time::Instant) -> bool {
        let reached = self.pins.get(&peer).map(|(reached, _)| reached);
        match local {
            Some(local) if self.refusing(peer, local, now) => reached.is_none(),
            Some(local) => reached.is_some_and(|reached| {
                let age = now.duration_since(reached.checked);
                reached.local == local && age < ROUTE_CHECK && !(reached.recheck && age >= RECHECK)
            }),
            None => reached.is_none(),
        }
    }
}

/// The errors with which Windows refuses to send from a pinned address: one
/// it no longer has (WSAEADDRNOTAVAIL, or WSAEINVAL for one it never had),
/// or one whose interface has no route to the peer (WSAENETUNREACH,
/// WSAEHOSTUNREACH), as on a Hyper-V switch or a Tailscale subnet router.
/// WSAEINVAL is also how a NIC without UDP segmentation fails.
const REFUSED: [WSA_ERROR; 4] = [WSAEINVAL, WSAEADDRNOTAVAIL, WSAENETUNREACH, WSAEHOSTUNREACH];

impl ReplySocket {
    /// Ask Windows for each datagram's local address on `socket`.
    pub fn new(socket: UdpSocket) -> std::io::Result<Self> {
        let raw = SOCKET(socket.as_raw_socket() as usize);
        let ipv6 = socket.local_addr()?.is_ipv6();
        let mut options = Vec::with_capacity(2);
        // An IPv6-only socket receives no IPv4 datagrams.
        if !ipv6 || !socket2::SockRef::from(&socket).only_v6()? {
            options.push((IPPROTO_IP, IP_PKTINFO));
        }
        if ipv6 {
            options.push((IPPROTO_IPV6, IPV6_PKTINFO));
        }
        for (level, option) in options {
            // SAFETY: the borrowed socket stays open, and the option takes this 4-byte BOOL.
            if unsafe { setsockopt(raw, level.0, option, Some(&1u32.to_ne_bytes())) }
                == SOCKET_ERROR
            {
                return Err(last_error());
            }
        }
        let mut receive_message: LPFN_WSARECVMSG = None;
        let mut returned = 0;
        // SAFETY: the input is the GUID and the output the function pointer's own storage, both
        // outliving this synchronous call on the open socket.
        if unsafe {
            WSAIoctl(
                raw,
                SIO_GET_EXTENSION_FUNCTION_POINTER,
                Some((&WSAID_WSARECVMSG as *const windows::core::GUID).cast()),
                std::mem::size_of::<windows::core::GUID>() as u32,
                Some((&mut receive_message as *mut LPFN_WSARECVMSG).cast()),
                std::mem::size_of::<LPFN_WSARECVMSG>() as u32,
                &mut returned,
                None,
                None,
            )
        } == SOCKET_ERROR
        {
            return Err(last_error());
        }
        Ok(Self {
            socket,
            receive_message: receive_message
                .ok_or_else(|| std::io::Error::other("Winsock returned no WSARecvMsg"))?,
            sources: RwLock::default(),
            route: routed_source,
            now: std::time::Instant::now,
            any_pinned: AtomicBool::new(false),
            missing_reported: AtomicBool::new(false),
            route_reported: AtomicBool::new(false),
        })
    }
    pub fn bind(address: impl ToSocketAddrs) -> std::io::Result<Self> {
        Self::new(UdpSocket::bind(address)?)
    }
    /// One datagram: its size, its sender, and the local address it arrived
    /// on. A datagram larger than `buffer` is lost with WSAEMSGSIZE, as
    /// recv_from loses it.
    pub fn receive(
        &self,
        buffer: &mut [u8],
    ) -> std::io::Result<(usize, SocketAddr, Option<IpAddr>)> {
        #[repr(C, align(8))]
        struct Control([u8; 128]);
        let mut control = Control([0; 128]);
        let mut data = WSABUF {
            len: buffer.len().try_into().unwrap_or(u32::MAX),
            buf: windows::core::PSTR(buffer.as_mut_ptr()),
        };
        // SAFETY: WSARecvMsg writes at most `length` bytes of sender address into the storage
        // try_init provides and the length it wrote; `data`, `control` and `message` outlive this
        // synchronous call on the open socket.
        let ((size, flags, control_len), peer) = unsafe {
            socket2::SockAddr::try_init(|storage, length| {
                let mut message = WSAMSG {
                    name: storage.cast(),
                    namelen: *length,
                    lpBuffers: &mut data,
                    dwBufferCount: 1,
                    Control: WSABUF {
                        len: control.0.len() as u32,
                        buf: windows::core::PSTR(control.0.as_mut_ptr()),
                    },
                    dwFlags: 0,
                };
                let mut size = 0;
                if (self.receive_message)(
                    SOCKET(self.socket.as_raw_socket() as usize),
                    &mut message,
                    &mut size,
                    std::ptr::null_mut(),
                    None,
                ) == SOCKET_ERROR
                {
                    return Err(last_error());
                }
                *length = message.namelen;
                Ok((size as usize, message.dwFlags, message.Control.len as usize))
            })?
        };
        if flags & MSG_PARTIAL != 0 {
            return Err(std::io::Error::from_raw_os_error(WSAEMSGSIZE.0));
        }
        let peer = peer
            .as_socket()
            .ok_or_else(|| std::io::Error::other("a datagram from a non-IP sender"))?;
        let local = local_address(&control.0[..control_len.min(control.0.len())]);
        if local.is_none() && !self.missing_reported.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                %peer,
                "Windows did not report which address a datagram arrived on; replies to it leave from the address Windows routes by"
            );
        }
        Ok((size, peer, local))
    }
    /// Answer `peer` from `local`, the address its datagram arrived on,
    /// unless Windows refused that within REFUSAL. Asks which address Windows routes
    /// to `peer` from when `local` is new, and again at most every
    /// ROUTE_CHECK, here on the receiving thread rather than on a send.
    pub fn answer_from(&self, peer: SocketAddr, local: Option<IpAddr>) {
        let now = (self.now)();
        if self.sources.read().unwrap().settled(peer, local, now) {
            return;
        }
        let local = local.filter(|local| !self.sources.read().unwrap().refusing(peer, *local, now));
        let Some(local) = local else {
            let mut sources = self.sources.write().unwrap();
            sources.pins.remove(&peer);
            self.note_pins(&sources);
            return;
        };
        let previous = self
            .sources
            .read()
            .unwrap()
            .pins
            .get(&peer)
            .filter(|(reached, _)| reached.local == local)
            .map(|(reached, _)| reached.pinned);
        let pinned = self.routes_elsewhere(peer, local, previous);
        let mut sources = self.sources.write().unwrap();
        let Sources {
            pins,
            refused,
            clock,
        } = &mut *sources;
        refused.remove(&(peer, local));
        Sources::remember(
            pins,
            clock,
            peer,
            Reached {
                local,
                pinned,
                checked: now,
                recheck: false,
            },
        );
        self.note_pins(&sources);
    }
    /// Forget what `peer` reached, once its connection or session ended.
    pub fn forget(&self, peer: SocketAddr) {
        let mut sources = self.sources.write().unwrap();
        sources.pins.remove(&peer);
        sources.refused.retain(|(refused, _), _| *refused != peer);
        self.note_pins(&sources);
    }
    /// Records whether any peer is pinned, after `sources` changed under
    /// its write lock.
    fn note_pins(&self, sources: &Sources) {
        let any = sources.pins.values().any(|(reached, _)| reached.pinned);
        self.any_pinned.store(any, Ordering::Release);
    }
    /// The address replies to `peer` must leave from; None leaves it to
    /// Windows, which routes them from the address the peer reached or
    /// knows nothing better.
    pub fn source(&self, peer: SocketAddr) -> Option<IpAddr> {
        if !self.any_pinned.load(Ordering::Acquire) {
            return None;
        }
        self.sources
            .read()
            .unwrap()
            .pins
            .get(&peer)
            .filter(|(reached, _)| reached.pinned)
            .map(|(reached, _)| reached.local)
    }
    /// The senders at `ip` and the address each reached, for logs.
    pub fn sources_from(&self, ip: IpAddr) -> Vec<(SocketAddr, IpAddr)> {
        let ip = ip.to_canonical();
        let mut sources: Vec<_> = self
            .sources
            .read()
            .unwrap()
            .pins
            .iter()
            .filter(|(peer, _)| peer.ip().to_canonical() == ip)
            .map(|(peer, (reached, _))| (*peer, reached.local))
            .collect();
        sources.sort();
        sources
    }
    /// One datagram to `peer`, from the address it reached.
    pub fn send_to(&self, buffer: &[u8], peer: SocketAddr) -> std::io::Result<usize> {
        let Ok((result, _)) = self.send_pinned(peer, self.source(peer), |source| {
            Ok::<_, std::convert::Infallible>(send_from(&self.socket, buffer, peer, source))
        });
        result.map_err(|error| std::io::Error::from_raw_os_error(error.0))
    }
    /// Sends with `send` from `source`, the address `peer` reached, and once
    /// more from the address Windows routes by when Windows refuses it. Only
    /// that second send succeeding shows the pin was at fault: a network
    /// that is down, or a NIC without UDP segmentation, keeps it, and the
    /// second send's error is the one the caller handles. Any other refusal
    /// has the route checked again soon (`reroute`). Returns the result and
    /// the source later sends to `peer` use.
    pub(super) fn send_pinned<T, E>(
        &self,
        peer: SocketAddr,
        source: Option<IpAddr>,
        mut send: impl FnMut(
            Option<IpAddr>,
        ) -> std::result::Result<std::result::Result<T, WSA_ERROR>, E>,
    ) -> std::result::Result<(std::result::Result<T, WSA_ERROR>, Option<IpAddr>), E> {
        let result = send(source)?;
        let Err(error) = &result else {
            return Ok((result, source));
        };
        let error = *error;
        if !REFUSED.contains(&error) {
            return Ok((result, source));
        }
        let Some(pinned) = source else {
            self.reroute(peer);
            return Ok((result, source));
        };
        match send(None)? {
            Ok(sent) => {
                self.refuse(peer, pinned, error);
                Ok((Ok(sent), None))
            }
            Err(retry) => {
                self.reroute(peer);
                Ok((Err(retry), source))
            }
        }
    }
    /// Whether Windows routes replies to `peer` from an address other than
    /// `local`. When Windows cannot say, `previous` stands, and a new peer is
    /// answered from `local`, the address it expects.
    fn routes_elsewhere(&self, peer: SocketAddr, local: IpAddr, previous: Option<bool>) -> bool {
        match (self.route)(SOCKET(self.socket.as_raw_socket() as usize), peer) {
            Ok(routed) => routed != local,
            Err(error) => {
                if !self.route_reported.swap(true, Ordering::Relaxed) {
                    tracing::warn!(
                        %peer,
                        %local,
                        %error,
                        "Windows did not say which address it routes this client by; replying from the address the client reached"
                    );
                }
                previous.unwrap_or(true)
            }
        }
    }
    /// Asks the receiving thread to check `peer`'s route again with its next
    /// datagram, at most every RECHECK, after a send to it failed, as when a
    /// VPN came up or went down. The sending thread asks Windows nothing.
    fn reroute(&self, peer: SocketAddr) {
        let flagged = |sources: &Sources| {
            sources
                .pins
                .get(&peer)
                .is_none_or(|(reached, _)| reached.recheck)
        };
        if flagged(&self.sources.read().unwrap()) {
            return;
        }
        if let Some((reached, _)) = self.sources.write().unwrap().pins.get_mut(&peer) {
            reached.recheck = true;
        }
    }
    /// Stop answering `peer` from `source`, which Windows refused, as after a
    /// VPN disconnects or when its interface has no route back; for REFUSAL,
    /// replies leave from the address Windows routes by.
    fn refuse(&self, peer: SocketAddr, source: IpAddr, error: WSA_ERROR) {
        let now = (self.now)();
        let new = {
            let mut sources = self.sources.write().unwrap();
            let new = !sources.refusing(peer, source, now);
            let Sources {
                pins,
                refused,
                clock,
            } = &mut *sources;
            if pins
                .get(&peer)
                .is_some_and(|(reached, _)| reached.local == source)
            {
                pins.remove(&peer);
            }
            Sources::remember(refused, clock, (peer, source), now);
            self.note_pins(&sources);
            new
        };
        if new {
            tracing::warn!(
                %peer,
                %source,
                code = error.0,
                "Windows refused to send from the address the client reached; replying to this client from the address Windows routes by"
            );
        }
    }
}

/// The local address Windows sends to `peer` from, by route.
fn routed_source(socket: SOCKET, peer: SocketAddr) -> std::io::Result<IpAddr> {
    let peer = socket2::SockAddr::from(peer);
    // SAFETY: Winsock writes at most the storage's length into it, and the
    // input is the peer's own sockaddr, both outliving this synchronous call
    // on the open socket.
    let ((), routed) = unsafe {
        socket2::SockAddr::try_init(|storage, length| {
            let mut returned = 0;
            if WSAIoctl(
                socket,
                SIO_ROUTING_INTERFACE_QUERY,
                Some(peer.as_ptr().cast()),
                peer.len() as u32,
                Some(storage.cast()),
                *length as u32,
                &mut returned,
                None,
                None,
            ) == SOCKET_ERROR
            {
                return Err(last_error());
            }
            *length = returned as _;
            Ok(())
        })
    }?;
    routed
        .as_socket()
        .map(|routed| routed.ip().to_canonical())
        .ok_or_else(|| std::io::Error::other("Windows returned no IP address for the route"))
}

impl std::ops::Deref for ReplySocket {
    type Target = UdpSocket;
    fn deref(&self) -> &UdpSocket {
        &self.socket
    }
}
impl AsRawSocket for ReplySocket {
    fn as_raw_socket(&self) -> RawSocket {
        self.socket.as_raw_socket()
    }
}
impl AsSocket for ReplySocket {
    fn as_socket(&self) -> BorrowedSocket<'_> {
        self.socket.as_socket()
    }
}

/// The local address in WSARecvMsg's control data: IP_PKTINFO or IPV6_PKTINFO.
pub(super) fn local_address(control: &[u8]) -> Option<IpAddr> {
    let header = std::mem::size_of::<CMSGHDR>();
    let mut offset = 0;
    while offset + header <= control.len() {
        // SAFETY: the header lies within `control`, and an unaligned read needs no alignment.
        let message = unsafe {
            control
                .as_ptr()
                .add(offset)
                .cast::<CMSGHDR>()
                .read_unaligned()
        };
        if message.cmsg_len < header || offset + message.cmsg_len > control.len() {
            return None;
        }
        let data = &control[offset + header..offset + message.cmsg_len];
        if message.cmsg_level == IPPROTO_IP.0
            && message.cmsg_type == IP_PKTINFO
            && data.len() >= std::mem::size_of::<IN_PKTINFO>()
        {
            // SAFETY: `data` holds an IN_PKTINFO (checked above), read unaligned; every bit
            // pattern is a valid address.
            let address = unsafe {
                data.as_ptr()
                    .cast::<IN_PKTINFO>()
                    .read_unaligned()
                    .ipi_addr
                    .S_un
                    .S_addr
            };
            return Some(std::net::Ipv4Addr::from(address.to_ne_bytes()).into());
        }
        if message.cmsg_level == IPPROTO_IPV6.0
            && message.cmsg_type == IPV6_PKTINFO
            && data.len() >= std::mem::size_of::<IN6_PKTINFO>()
        {
            // SAFETY: `data` holds an IN6_PKTINFO (checked above), read unaligned; every bit
            // pattern is a valid address.
            let address = unsafe {
                data.as_ptr()
                    .cast::<IN6_PKTINFO>()
                    .read_unaligned()
                    .ipi6_addr
                    .u
                    .Byte
            };
            return Some(IpAddr::from(address).to_canonical());
        }
        offset += message.cmsg_len.next_multiple_of(8);
    }
    None
}

fn last_error() -> std::io::Error {
    // SAFETY: WSAGetLastError only reads this thread's last Winsock error.
    std::io::Error::from_raw_os_error(unsafe { WSAGetLastError() }.0)
}
