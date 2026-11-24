//! A UDP socket on Linux that answers each client from the local address the client's
//! datagrams arrived on (`IP_PKTINFO`, `IPV6_PKTINFO`).
use super::sys::{Errno, local_address, send_from, set_option};
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr, ToSocketAddrs, UdpSocket},
    os::fd::{AsFd, AsRawFd, BorrowedFd, RawFd},
    sync::{
        RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// Spoofed senders cannot grow a ReplySocket's table without bound; a client
/// still streaming is learned again from its next datagram.
pub(super) const REPLY_SOURCES: usize = 256;
/// A UDP socket that answers each client from the local address the client's
/// datagrams arrived on, as moonlight-common-c's ENet does. Bound to every
/// address, Linux otherwise picks the source by route: on a PC where a VPN
/// such as Tailscale also routes the LAN, replies to a LAN client could leave
/// from the VPN's address, and the client drops them (issue #54).
///
/// Every send through it (`send_to`, `Batch::send`, `send_datagram`) uses
/// what `answer_from` learned for that peer. A peer Linux already routes
/// from the address it reached, as on a plain LAN, is answered as before,
/// with plain sendto and without taking the table's lock while no peer is
/// pinned.
pub struct ReplySocket {
    pub(super) socket: UdpSocket,
    /// The address Linux sends to a peer from; `routed_source` but in tests.
    pub(super) route: fn(SocketAddr) -> std::io::Result<IpAddr>,
    /// The time; `Instant::now` but in tests.
    pub(super) now: fn() -> Instant,
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
pub(super) const ROUTE_CHECK: Duration = Duration::from_secs(1);
/// How soon after the last check a failed send has the route checked again.
pub(super) const RECHECK: Duration = Duration::from_millis(100);
/// How long an address Linux refused to send to a peer from is not pinned
/// again; a refusal that lasts costs a failed send and its retry this often.
pub(super) const REFUSAL: Duration = Duration::from_secs(5);
/// What a ReplySocket learned about one peer.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Reached {
    /// The local address the peer's datagrams arrive on.
    local: IpAddr,
    /// Whether Linux routes replies from another address, so they must be
    /// sent from `local`.
    pinned: bool,
    /// When the route was last checked.
    checked: Instant,
    /// Whether a send to the peer failed as unreachable since.
    recheck: bool,
}
/// What a ReplySocket learned per peer, each table holding at most
/// REPLY_SOURCES entries and evicting the oldest.
#[derive(Default)]
pub(super) struct Sources {
    /// What each peer last reached, and when it was learned.
    pins: HashMap<SocketAddr, (Reached, u64)>,
    /// The local addresses Linux refused to send to a peer from, and when;
    /// not pinned again for REFUSAL, as a VPN that went down may come back.
    pub(super) refused: HashMap<(SocketAddr, IpAddr), (Instant, u64)>,
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
    /// Whether Linux refused to send to `peer` from `local` less than
    /// REFUSAL before `now`.
    fn refusing(&self, peer: SocketAddr, local: IpAddr, now: Instant) -> bool {
        self.refused
            .get(&(peer, local))
            .is_some_and(|(at, _)| now.duration_since(*at) < REFUSAL)
    }
    /// Whether a datagram from `peer` to `local` at `now` changes nothing:
    /// the same address with a fresh route, or one refused for it.
    fn settled(&self, peer: SocketAddr, local: Option<IpAddr>, now: Instant) -> bool {
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
/// The errors with which Linux refuses to send from a pinned address: one
/// it does not have (`EINVAL`, or `EADDRNOTAVAIL`), or one whose interface has no
/// route to the peer (`ENETUNREACH`, `EHOSTUNREACH`), as on a Tailscale subnet router.
/// `EINVAL` is also how a route without UDP segmentation fails.
const REFUSED: [Errno; 4] = [
    libc::EINVAL,
    libc::EADDRNOTAVAIL,
    libc::ENETUNREACH,
    libc::EHOSTUNREACH,
];
impl ReplySocket {
    /// Ask Linux for each datagram's local address on `socket`.
    pub fn new(socket: UdpSocket) -> std::io::Result<Self> {
        let ipv6 = socket.local_addr()?.is_ipv6();
        if ipv6 {
            set_option(&socket, libc::IPPROTO_IPV6, libc::IPV6_RECVPKTINFO, 1)?;
            // A dual-stack socket reports an IPv4 datagram's address as an IPv4-mapped one
            // already; the IPv4 option only adds its own copy, so a refusal changes nothing.
            if !socket2::SockRef::from(&socket).only_v6()? {
                let _ = set_option(&socket, libc::IPPROTO_IP, libc::IP_PKTINFO, 1);
            }
        } else {
            set_option(&socket, libc::IPPROTO_IP, libc::IP_PKTINFO, 1)?;
        }
        Ok(Self {
            socket,
            sources: RwLock::default(),
            route: routed_source,
            now: Instant::now,
            any_pinned: AtomicBool::new(false),
            missing_reported: AtomicBool::new(false),
            route_reported: AtomicBool::new(false),
        })
    }
    pub fn bind(address: impl ToSocketAddrs) -> std::io::Result<Self> {
        Self::new(UdpSocket::bind(address)?)
    }
    /// One datagram: its size, its sender, and the local address it arrived
    /// on. A datagram larger than `buffer` is lost with `EMSGSIZE`.
    pub fn receive(
        &self,
        buffer: &mut [u8],
    ) -> std::io::Result<(usize, SocketAddr, Option<IpAddr>)> {
        let mut control = [0u8; 128];
        let mut data = libc::iovec {
            iov_base: buffer.as_mut_ptr().cast(),
            iov_len: buffer.len(),
        };
        // SAFETY: recvmsg writes at most `length` bytes of sender address into the storage
        // try_init provides, at most `buffer.len()` bytes of data and at most the control
        // buffer's length; `data`, `control` and `message` outlive this call on the open
        // socket, and an all-zero msghdr is a valid one.
        let ((size, flags, control_len), peer) = unsafe {
            socket2::SockAddr::try_init(|storage, length| {
                let mut message: libc::msghdr = std::mem::zeroed();
                message.msg_name = storage.cast();
                message.msg_namelen = *length;
                message.msg_iov = &mut data;
                message.msg_iovlen = 1;
                message.msg_control = control.as_mut_ptr().cast();
                message.msg_controllen = control.len() as _;
                let size = libc::recvmsg(self.socket.as_raw_fd(), &mut message, 0);
                if size < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                *length = message.msg_namelen;
                Ok((
                    size as usize,
                    message.msg_flags,
                    message.msg_controllen as usize,
                ))
            })?
        };
        if flags & libc::MSG_TRUNC != 0 {
            return Err(std::io::Error::from_raw_os_error(libc::EMSGSIZE));
        }
        let peer = peer
            .as_socket()
            .ok_or_else(|| std::io::Error::other("a datagram from a non-IP sender"))?;
        let local = local_address(&control[..control_len.min(control.len())]);
        if local.is_none() && !self.missing_reported.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                %peer,
                "Linux did not report which address a datagram arrived on; replies to it leave from the address Linux routes by"
            );
        }
        Ok((size, peer, local))
    }
    /// Answer `peer` from `local`, the address its datagram arrived on,
    /// unless Linux refused that within REFUSAL. Asks which address Linux routes
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
    /// Linux, which routes them from the address the peer reached or
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
        result.map_err(std::io::Error::from_raw_os_error)
    }
    /// Sends with `send` from `source`, the address `peer` reached, and once
    /// more from the address Linux routes by when Linux refuses it. Only
    /// that second send succeeding shows the pin was at fault: a network
    /// that is down, or a route without UDP segmentation, keeps it, and the
    /// second send's error is the one the caller handles. Any other refusal
    /// has the route checked again soon (`reroute`). Returns the result and
    /// the source later sends to `peer` use.
    pub(super) fn send_pinned<T, E>(
        &self,
        peer: SocketAddr,
        source: Option<IpAddr>,
        mut send: impl FnMut(Option<IpAddr>) -> Result<Result<T, Errno>, E>,
    ) -> Result<(Result<T, Errno>, Option<IpAddr>), E> {
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
    /// Whether Linux routes replies to `peer` from an address other than
    /// `local`. When Linux cannot say, `previous` stands, and a new peer is
    /// answered from `local`, the address it expects.
    fn routes_elsewhere(&self, peer: SocketAddr, local: IpAddr, previous: Option<bool>) -> bool {
        match (self.route)(peer) {
            Ok(routed) => routed != local,
            Err(error) => {
                if !self.route_reported.swap(true, Ordering::Relaxed) {
                    tracing::warn!(
                        %peer,
                        %local,
                        %error,
                        "Linux did not say which address it routes this client by; replying from the address the client reached"
                    );
                }
                previous.unwrap_or(true)
            }
        }
    }
    /// Asks the receiving thread to check `peer`'s route again with its next
    /// datagram, at most every RECHECK, after a send to it failed, as when a
    /// VPN came up or went down. The sending thread asks Linux nothing.
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
    /// Stop answering `peer` from `source`, which Linux refused, as after a
    /// VPN disconnects or when its interface has no route back; for REFUSAL,
    /// replies leave from the address Linux routes by.
    fn refuse(&self, peer: SocketAddr, source: IpAddr, error: Errno) {
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
                code = error,
                "Linux refused to send from the address the client reached; replying to this client from the address Linux routes by"
            );
        }
    }
}
/// The local address Linux sends to `peer` from, by route: the one a socket connected to it
/// gets. A UDP connect sends nothing; it only picks the route.
fn routed_source(peer: SocketAddr) -> std::io::Result<IpAddr> {
    let peer = SocketAddr::new(peer.ip().to_canonical(), peer.port());
    let socket = socket2::Socket::new(
        socket2::Domain::for_address(peer),
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )?;
    socket.connect(&peer.into())?;
    socket
        .local_addr()?
        .as_socket()
        .map(|routed| routed.ip().to_canonical())
        .ok_or_else(|| std::io::Error::other("Linux returned no IP address for the route"))
}
impl std::ops::Deref for ReplySocket {
    type Target = UdpSocket;
    fn deref(&self) -> &UdpSocket {
        &self.socket
    }
}
impl AsRawFd for ReplySocket {
    fn as_raw_fd(&self) -> RawFd {
        self.socket.as_raw_fd()
    }
}
impl AsFd for ReplySocket {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.socket.as_fd()
    }
}

