//! UDP segmentation offload (`WSASendMsg`) batches, runs, and ancillary control buffers.
use super::{
    datagram::{
        SegmentedFailure, WaitBudget, WritableWaits, buffer_available, classify_segmented_error,
        route_unavailable, send_one, send_with_retry, transient,
    },
    reply::ReplySocket,
};
use anyhow::Result;
use std::{
    net::{IpAddr, SocketAddr, UdpSocket},
    os::windows::io::AsRawSocket,
};
use windows::Win32::Networking::WinSock::*;

/// Cumulative drop counters read from [`Batch::drops`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BatchDrops {
    pub dropped: u64,
    pub oversized: u64,
    pub route_unavailable: u64,
}

/// Per-batch or per-frame datagram drop breakdown computed by [`Batch::drops_since`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BatchDropDelta {
    pub dropped: u64,
    pub oversized: u64,
    pub route_unavailable: u64,
    pub refused: u64,
}

impl BatchDropDelta {
    pub fn any(&self) -> bool {
        self.dropped > 0
    }

    pub fn route_only(&self) -> bool {
        self.route_unavailable > 0 && self.refused == 0 && self.oversized == 0
    }
}

/// Per-message segmentation leaves the shared socket's options untouched.
pub struct Batch {
    pub(super) offload: bool,
    pub system_calls: u64,
    /// Datagrams lost to transient send errors (full buffer, route unavailable, oversized).
    pub dropped: u64,
    /// Packets handed to Winsock in the last [`Batch::send`] call.
    pub(super) last_sent: usize,
    /// Datagrams the path refused because they are larger than its MTU with
    /// `IP_DONTFRAGMENT`/`IPV6_DONTFRAG` set; also counted in `dropped`.
    pub oversized: u64,
    /// Datagrams dropped because the PC had no route to the peer; also counted in `dropped`.
    pub route_unavailable: u64,
    /// The sender's socket refuses fragmentation ([`super::set_dont_fragment`]), as
    /// the PyroWave probe's does for a jumbo path, so an oversized datagram is
    /// a lost packet to report, not a send error that ends the stream.
    pub dont_fragment: bool,
    /// Enabled by the sender only for pacing traces.
    pub waits: Option<WritableWaits>,
    /// The frame in flight (see [`Batch::begin_frame`]): its own deadline,
    /// and the deadline its batches share once one met a full socket. `None`
    /// outside a frame, where one batch waits [`super::WRITABLE_WAIT`] on its own.
    pub(super) frame: Option<FrameWait>,
    pub(super) unreported: u64,
    pub(super) reported: Option<std::time::Instant>,
    pub(super) unreported_route: u64,
    pub(super) reported_route: Option<std::time::Instant>,
    /// Scratch WSABUFs filled for each segmented send, so their 1 KiB is not
    /// zeroed anew every batch.
    pub(super) buffers: [WSABUF; 64],
    /// The control data and address of the last segmented send, rebuilt only
    /// when the peer, the first packet's size or the source changes.
    pub(super) segmented: Option<Segmented>,
}
impl Default for Batch {
    fn default() -> Self {
        Self {
            offload: true,
            system_calls: 0,
            dropped: 0,
            last_sent: 0,
            oversized: 0,
            route_unavailable: 0,
            dont_fragment: false,
            waits: None,
            frame: None,
            unreported: 0,
            reported: None,
            unreported_route: 0,
            reported_route: None,
            buffers: [WSABUF {
                len: 0,
                buf: windows::core::PSTR::null(),
            }; 64],
            segmented: None,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct FrameWait {
    until: std::time::Instant,
    pub(super) deadline: Option<std::time::Instant>,
    enobufs_until: Option<std::time::Instant>,
    enobufs_deadline: Option<std::time::Instant>,
}

pub(super) fn segmented_retries_in_place(error: WSA_ERROR) -> bool {
    error != WSAENOBUFS
}

impl Batch {
    /// Start one video frame: its batches share one wait for room in a full
    /// socket, up to `wait` from now or [`super::WRITABLE_WAIT`] from the frame's
    /// first refusal, whichever is later (see [`Batch::frame_wait_spent`]).
    pub fn begin_frame(&mut self, wait: std::time::Duration) {
        let now = std::time::Instant::now();
        self.frame = Some(FrameWait {
            until: now + wait,
            deadline: None,
            enobufs_until: None,
            enobufs_deadline: None,
        });
    }
    /// Start one video frame with a separate shared retry ceiling for
    /// transient `WSAENOBUFS` driver pressure, keeping `WSAEWOULDBLOCK` at
    /// `wait` and adding zero delay when the socket accepts datagrams.
    pub fn begin_frame_with_enobufs(
        &mut self,
        wait: std::time::Duration,
        enobufs_wait: std::time::Duration,
    ) {
        let now = std::time::Instant::now();
        self.frame = Some(FrameWait {
            until: now + wait,
            deadline: None,
            enobufs_until: Some(now + enobufs_wait),
            enobufs_deadline: None,
        });
    }
    /// The frame is over: a later batch outside a frame gets its own wait.
    pub fn end_frame(&mut self) {
        self.frame = None;
    }
    /// The frame in flight met a full socket and its shared wait is over:
    /// its sender gives up the rest of it.
    pub fn frame_wait_spent(&self) -> bool {
        let now = std::time::Instant::now();
        self.frame.is_some_and(|frame| {
            [frame.deadline, frame.enobufs_deadline]
                .into_iter()
                .flatten()
                .any(|deadline| now >= deadline)
        })
    }
    /// How many of the leading `packets` one segmented send carries: equal
    /// sizes, then at most one shorter, at most 64 and 65507 bytes in all.
    /// The shorter tail rides along, as it would otherwise take a paced
    /// iteration and a syscall of its own. An empty datagram is never in a
    /// batch: segmentation would not send it.
    pub fn count(packets: &[Vec<u8>], limit: usize) -> usize {
        let Some(first) = packets.first() else {
            return 0;
        };
        let size = first.len();
        if size == 0 {
            return 1;
        }
        let limit = limit.min(65507);
        let max = (limit / size).clamp(1, 64);
        let (mut count, mut total) = (0, 0);
        for packet in packets.iter().take(max) {
            if packet.len() != size {
                break;
            }
            count += 1;
            total += size;
        }
        if let Some(tail) = packets.get(count) {
            let len = tail.len();
            if count < max && len > 0 && len < size && total + len <= 65507 {
                count += 1;
            }
        }
        count.max(1)
    }
    /// Snapshot cumulative drop counters so callers can separate socket congestion from
    /// route unavailability or MTU refusals.
    pub fn drops(&self) -> BatchDrops {
        BatchDrops {
            dropped: self.dropped,
            oversized: self.oversized,
            route_unavailable: self.route_unavailable,
        }
    }
    /// Compute drops recorded since `before`.
    pub fn drops_since(&self, before: BatchDrops) -> BatchDropDelta {
        let dropped = self.dropped.saturating_sub(before.dropped);
        let oversized = self.oversized.saturating_sub(before.oversized);
        let route_unavailable = self
            .route_unavailable
            .saturating_sub(before.route_unavailable);
        let refused = dropped
            .saturating_sub(oversized)
            .saturating_sub(route_unavailable);
        BatchDropDelta {
            dropped,
            oversized,
            route_unavailable,
            refused,
        }
    }
    /// Count dropped datagrams and report them at most every 5 seconds.
    pub(super) fn dropped(&mut self, count: usize, error: WSA_ERROR) {
        if let Some(trace) = self.waits.as_mut() {
            trace.last_error = error.0;
        }
        self.dropped += count as u64;
        if route_unavailable(error) {
            self.route_unavailable += count as u64;
            self.unreported_route += count as u64;
            if self
                .reported_route
                .is_none_or(|at| at.elapsed() >= std::time::Duration::from_secs(5))
            {
                tracing::warn!(
                    code = error.0,
                    dropped = self.unreported_route,
                    "UDP send failed: the network route to the client is unavailable, so packets were dropped until the connection returns"
                );
                self.unreported_route = 0;
                self.reported_route = Some(std::time::Instant::now());
            }
            return;
        }
        self.unreported += count as u64;
        if self
            .reported
            .is_none_or(|at| at.elapsed() >= std::time::Duration::from_secs(5))
        {
            tracing::warn!(
                code = error.0,
                dropped = self.unreported,
                "UDP send failed: the socket stayed full, so packets were dropped and the client may stutter. The PC could not send fast enough (its CPU or network adapter fell behind); close other work on the PC or lower the bitrate"
            );
            self.unreported = 0;
            self.reported = Some(std::time::Instant::now());
        }
    }
    /// Count datagrams as refused by a full socket without sending them,
    /// for `BUTTERPOLLO_TEST_SEND_OUTAGE`.
    pub fn refuse(&mut self, count: usize) {
        self.last_sent = 0;
        self.dropped(count, WSAEWOULDBLOCK);
    }
    /// Number of packets sent in the last [`Batch::send`] call.
    pub fn sent_packets(&self) -> usize {
        self.last_sent
    }
    /// Count datagrams the path refused for their size with fragmentation
    /// refused: lost like a full buffer's, and reported once every 5 seconds as
    /// the jumbo path's fault rather than a transient socket error.
    pub(super) fn too_large(&mut self, count: usize, error: WSA_ERROR) {
        if let Some(trace) = self.waits.as_mut() {
            trace.last_error = error.0;
        }
        self.dropped += count as u64;
        self.oversized += count as u64;
        if self
            .reported
            .is_none_or(|at| at.elapsed() >= std::time::Duration::from_secs(5))
        {
            tracing::warn!(
                code = error.0,
                dropped = count,
                "video datagrams exceed the path MTU and are dropped: the offered packet size plus 64 bytes of RTP, UDP and IPv6 headers must fit every hop's MTU"
            );
            self.reported = Some(std::time::Instant::now());
        }
    }
    /// Bytes handed to Winsock. Datagrams that hit a transient error are
    /// dropped, as Vibepollo does, rather than ending the stream. The batch
    /// shares one writable wait: under congestion it drops rather than
    /// holding the frame for a wait per datagram. They leave from the
    /// address `peer` reached.
    pub fn send(
        &mut self,
        socket: &ReplySocket,
        packets: &[Vec<u8>],
        peer: SocketAddr,
    ) -> Result<usize> {
        self.last_sent = 0;
        if packets.is_empty() {
            return Ok(0);
        }
        let mut source = socket.source(peer);
        let mut budget = WaitBudget::for_frame_with_enobufs(
            self.frame.map(|frame| frame.until),
            self.frame.and_then(|frame| frame.deadline),
            self.frame.and_then(|frame| frame.enobufs_until),
            self.frame.and_then(|frame| frame.enobufs_deadline),
        );
        let dropped = self.dropped;
        let result = self.send_within(socket, packets, peer, &mut source, &mut budget);
        if let Some(frame) = self.frame.as_mut() {
            // The shared deadline lasts only while the socket stays full: a
            // batch that dropped nothing ends that spell, and a later refusal
            // starts a new one with at least WRITABLE_WAIT.
            frame.deadline = budget.deadline.filter(|_| self.dropped > dropped);
            frame.enobufs_deadline = budget.enobufs_deadline.filter(|_| self.dropped > dropped);
        }
        result
    }
    fn send_within(
        &mut self,
        socket: &ReplySocket,
        packets: &[Vec<u8>],
        peer: SocketAddr,
        source: &mut Option<IpAddr>,
        budget: &mut WaitBudget,
    ) -> Result<usize> {
        if self.offload
            && packets.len() > 1
            && packets.len() <= 64
            && !packets[0].is_empty()
            && packets[..packets.len() - 1]
                .iter()
                .all(|p| p.len() == packets[0].len())
            && (1..=packets[0].len()).contains(&packets.last().unwrap().len())
        {
            let result;
            (result, *source) = socket.send_pinned(peer, *source, |source| {
                self.send_segmented(socket, packets, peer, source, budget)
            })?;
            match result {
                Ok(sent) => {
                    self.last_sent = packets.len();
                    return Ok(sent);
                }
                Err(error) => match classify_segmented_error(error) {
                    // WSAEMSGSIZE here can be the offload's own limit, not the
                    // path's MTU: turn segmentation off and send the datagrams one
                    // by one, where an oversized one is counted as such. Dropping
                    // the batch would leave every later batch dropped as well.
                    SegmentedFailure::DisableOffload => {
                        self.offload = false;
                        tracing::warn!(
                            code = error.0,
                            "UDP segmentation unavailable or this batch could not be segmented; using individual datagrams with more CPU overhead. Update the network driver or lower bitrate if sending stalls"
                        );
                    }
                    // Transient ENOBUFS on a multi-packet GSO buffer can still
                    // leave room for individual datagrams; keep offload enabled
                    // for later batches once driver pressure clears.
                    SegmentedFailure::FallbackDatagrams => {}
                    SegmentedFailure::DropBatch => {
                        self.dropped(packets.len(), error);
                        return Ok(0);
                    }
                    SegmentedFailure::Fatal => {
                        return Err(std::io::Error::from_raw_os_error(error.0).into());
                    }
                },
            }
        }
        let mut sent = 0;
        for (index, packet) in packets.iter().enumerate() {
            let result;
            (result, *source) = socket.send_pinned(peer, *source, |source| {
                self.system_calls += 1;
                send_one(socket, packet, peer, source, budget, self.waits.as_mut())
            })?;
            match result {
                Ok(()) => {
                    sent += packet.len();
                    self.last_sent += 1;
                }
                // A socket that refuses fragmentation reports a datagram larger than the
                // path's MTU this way, as the PyroWave probe does on a jumbo path. Drop
                // it like a path-MTU refusal; ending the probe's or a stream's send is
                // worse than losing that datagram.
                Err(error) if error == WSAEMSGSIZE => {
                    self.too_large(1, error);
                }
                Err(error) if transient(error) => {
                    self.dropped(packets.len() - index, error);
                    break;
                }
                Err(error) => return Err(std::io::Error::from_raw_os_error(error.0).into()),
            }
        }
        Ok(sent)
    }
    /// One WSASendMsg with UDP segmentation from `source`, retried once when
    /// the socket buffer is full and frees up within `budget`. The inner error
    /// is Winsock's.
    pub(super) fn send_segmented(
        &mut self,
        socket: &UdpSocket,
        packets: &[Vec<u8>],
        peer: SocketAddr,
        source: Option<IpAddr>,
        budget: &mut WaitBudget,
    ) -> Result<std::result::Result<usize, WSA_ERROR>> {
        let size = u32::try_from(packets[0].len())?;
        // Rebuild the address and control data only when they changed, then
        // fill the reused scratch buffers and count their bytes in one pass.
        if self
            .segmented
            .as_ref()
            .is_none_or(|s| s.peer != peer || s.size != size || s.source != source)
        {
            let mut control = Ancillary::default();
            control.push(IPPROTO_UDP, UDP_SEND_MSG_SIZE, size);
            if let Some(source) = source {
                control.source(source);
            }
            self.segmented = Some(Segmented {
                peer,
                size,
                source,
                control,
                address: socket2::SockAddr::from(peer),
            });
        }
        let mut expected = 0usize;
        for (buffer, packet) in self.buffers.iter_mut().zip(packets) {
            buffer.len = packet.len() as u32;
            buffer.buf = windows::core::PSTR(packet.as_ptr().cast_mut());
            expected += packet.len();
        }
        let buffers = &self.buffers[..packets.len()];
        let segmented = self.segmented.as_ref().unwrap();
        let system_calls = &mut self.system_calls;
        let result = send_with_retry(
            || {
                *system_calls += 1;
                send_message(socket, buffers, &segmented.address, &segmented.control)
            },
            // Transient ENOBUFS on a multi-packet GSO buffer falls back immediately
            // to individual datagrams (which get the full enobufs_deadline) without
            // spending the recovery budget inside send_segmented or disabling offload.
            |error| {
                segmented_retries_in_place(error)
                    && buffer_available(socket, error, budget, self.waits.as_mut())
            },
        );
        Ok(match result {
            Ok(sent) => {
                anyhow::ensure!(sent == expected, "Winsock returned an incomplete UDP batch");
                Ok(sent)
            }
            Err(error) => Err(error),
        })
    }
}

/// WSABUFs over the first 64 of `packets`, the rest empty. They borrow the
/// packets' bytes without the borrow checker knowing: use them while
/// `packets` lives.
fn wsa_buffers<T: AsRef<[u8]>>(packets: &[T]) -> [WSABUF; 64] {
    let mut buffers = [WSABUF {
        len: 0,
        buf: windows::core::PSTR::null(),
    }; 64];
    for (buffer, packet) in buffers.iter_mut().zip(packets) {
        let packet = packet.as_ref();
        buffer.len = packet.len() as u32;
        buffer.buf = windows::core::PSTR(packet.as_ptr().cast_mut());
    }
    buffers
}

/// Datagrams of any size from a plain socket to one peer, in order and in as
/// few system calls as UDP segmentation allows: each run of equal-sized
/// datagrams, the last of a run maybe shorter, goes in one WSASendMsg. For
/// WebRTC, whose RTP packets are equal but for each frame's last and whose
/// other traffic is any size. Without segmentation they go one by one.
pub struct Segments {
    pub(super) offload: bool,
    pub system_calls: u64,
}
impl Default for Segments {
    fn default() -> Self {
        Self {
            offload: true,
            system_calls: 0,
        }
    }
}

/// What became of one [`Segments::send`].
#[derive(Debug, Default)]
pub struct Sent {
    pub datagrams: usize,
    pub bytes: usize,
    /// Datagrams lost to send errors, and the last such error.
    pub failed: usize,
    pub error: Option<std::io::Error>,
}

impl Segments {
    /// How many of the leading `packets` one segmented send carries: equal
    /// sizes, then at most one shorter, at most 64 and 65507 bytes in all.
    /// An empty datagram is never in a run: segmentation would not send it.
    pub fn run<T: AsRef<[u8]>>(packets: &[T]) -> usize {
        let Some(size) = packets.first().map(|p| p.as_ref().len()) else {
            return 0;
        };
        if size == 0 {
            return 1;
        }
        let (mut count, mut total) = (0, 0);
        for len in packets.iter().take(64).map(|p| p.as_ref().len()) {
            if len > size || len == 0 || total + len > 65507 {
                break;
            }
            count += 1;
            total += len;
            if len < size {
                break;
            }
        }
        count.max(1)
    }
    /// Send `packets` to `peer`. An error loses only the datagrams it hit:
    /// the caller counts them and goes on.
    pub fn send<T: AsRef<[u8]>>(
        &mut self,
        socket: &UdpSocket,
        packets: &[T],
        peer: SocketAddr,
    ) -> Sent {
        let mut sent = Sent::default();
        let mut rest = packets;
        while !rest.is_empty() {
            let (run, tail) = rest.split_at(if self.offload { Self::run(rest) } else { 1 });
            rest = tail;
            if run.len() > 1 {
                let expected: usize = run.iter().map(|p| p.as_ref().len()).sum();
                match self.send_run(socket, run, peer) {
                    Ok(bytes) if bytes == expected => {
                        sent.datagrams += run.len();
                        sent.bytes += bytes;
                        continue;
                    }
                    // A datagram socket sends all or nothing; a short count
                    // would be a Winsock bug. Loud, counted as lost, and not
                    // sent again: some of it may have gone.
                    Ok(bytes) => {
                        tracing::warn!(bytes, expected, "Winsock sent part of a UDP batch");
                        sent.failed += run.len();
                        sent.error =
                            Some(std::io::Error::other("Winsock sent part of a UDP batch"));
                        continue;
                    }
                    Err(error)
                        if [WSAEINVAL, WSAENOPROTOOPT, WSAEOPNOTSUPP, WSAEMSGSIZE]
                            .contains(&error) =>
                    {
                        self.offload = false;
                        tracing::warn!(
                            code = error.0,
                            "UDP segmentation unavailable; WebRTC sends individual datagrams with more CPU overhead. Update the network driver if sending stalls"
                        );
                    }
                    // Nothing of the run went. One by one, as before
                    // segmentation, a full buffer or a route that comes back
                    // loses only the datagrams it hits.
                    Err(_) => {}
                }
            }
            for packet in run {
                self.system_calls += 1;
                match socket.send_to(packet.as_ref(), peer) {
                    Ok(_) => {
                        sent.datagrams += 1;
                        sent.bytes += packet.as_ref().len();
                    }
                    Err(error) => {
                        sent.failed += 1;
                        sent.error = Some(error);
                    }
                }
            }
        }
        sent
    }
    /// One WSASendMsg of a [`Segments::run`], segmented at its first
    /// datagram's size. The error is Winsock's.
    fn send_run<T: AsRef<[u8]>>(
        &mut self,
        socket: &UdpSocket,
        packets: &[T],
        peer: SocketAddr,
    ) -> std::result::Result<usize, WSA_ERROR> {
        let mut control = Ancillary::default();
        // A run is at most 65507 bytes, so its first datagram's size fits.
        control.push(
            IPPROTO_UDP,
            UDP_SEND_MSG_SIZE,
            packets[0].as_ref().len() as u32,
        );
        let buffers = wsa_buffers(packets);
        self.system_calls += 1;
        send_message(
            socket,
            &buffers[..packets.len()],
            &socket2::SockAddr::from(peer),
            &control,
        )
    }
}

/// WSASendMsg control data: whole messages, each padded to 8 bytes as
/// WSA_CMSG_SPACE pads them.
#[repr(C, align(8))]
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
    pub(super) fn push<T: Copy>(&mut self, level: IPPROTO, kind: i32, data: T) {
        let header = std::mem::size_of::<CMSGHDR>();
        let size = header + std::mem::size_of::<T>();
        let end = self.len + size.next_multiple_of(8);
        assert!(end <= self.bytes.len(), "control data too large");
        // SAFETY: the header and `data` end within `bytes` (asserted above), and unaligned writes
        // need no alignment.
        unsafe {
            let at = self.bytes.as_mut_ptr().add(self.len);
            at.cast::<CMSGHDR>().write_unaligned(CMSGHDR {
                cmsg_len: size,
                cmsg_level: level.0,
                cmsg_type: kind,
            });
            at.add(header).cast::<T>().write_unaligned(data);
        }
        self.len = end;
    }
    /// Send from `source`, as Moonlight's ENet does: IP_PKTINFO for IPv4, also
    /// on a dual-stack socket, IPV6_PKTINFO for IPv6.
    pub(super) fn source(&mut self, source: IpAddr) {
        match source.to_canonical() {
            IpAddr::V4(source) => self.push(
                IPPROTO_IP,
                IP_PKTINFO,
                IN_PKTINFO {
                    ipi_addr: IN_ADDR {
                        S_un: IN_ADDR_0 {
                            S_addr: u32::from_ne_bytes(source.octets()),
                        },
                    },
                    ipi_ifindex: 0,
                },
            ),
            IpAddr::V6(source) => self.push(
                IPPROTO_IPV6,
                IPV6_PKTINFO,
                IN6_PKTINFO {
                    ipi6_addr: IN6_ADDR {
                        u: IN6_ADDR_0 {
                            Byte: source.octets(),
                        },
                    },
                    ipi6_ifindex: 0,
                },
            ),
        }
    }
}

/// The control data and address of one segmented send, kept so a frame's
/// batches rebuild them only when the peer, the first packet's size or the
/// source changes.
pub(super) struct Segmented {
    peer: SocketAddr,
    size: u32,
    source: Option<IpAddr>,
    control: Ancillary,
    address: socket2::SockAddr,
}

/// One WSASendMsg of `buffers` to `peer` with `control`. The error is Winsock's.
fn send_message(
    socket: &UdpSocket,
    buffers: &[WSABUF],
    peer: &socket2::SockAddr,
    control: &Ancillary,
) -> std::result::Result<usize, WSA_ERROR> {
    let message = WSAMSG {
        name: peer.as_ptr().cast_mut().cast(),
        namelen: peer.len(),
        // WSASendMsg only reads the buffers.
        lpBuffers: buffers.as_ptr().cast_mut(),
        dwBufferCount: buffers.len() as u32,
        Control: WSABUF {
            len: control.len as u32,
            buf: windows::core::PSTR(control.bytes.as_ptr().cast_mut()),
        },
        dwFlags: 0,
    };
    let mut sent = 0;
    // SAFETY: `message` and all it points to outlive this synchronous call, and dwBufferCount and
    // the control length stay within `buffers` and `control.bytes`.
    let result = unsafe {
        WSASendMsg(
            SOCKET(socket.as_raw_socket() as usize),
            &message,
            0,
            Some(&mut sent),
            None,
            None,
        )
    };
    if result == 0 {
        Ok(sent as usize)
    } else {
        // SAFETY: WSAGetLastError only reads this thread's last Winsock error.
        Err(unsafe { WSAGetLastError() })
    }
}

/// One datagram to `peer`, from `source` when it is known. The error is Winsock's.
pub(super) fn send_from(
    socket: &UdpSocket,
    packet: &[u8],
    peer: SocketAddr,
    source: Option<IpAddr>,
) -> std::result::Result<usize, WSA_ERROR> {
    let Some(source) = source else {
        return socket
            .send_to(packet, peer)
            .map_err(|e| WSA_ERROR(e.raw_os_error().unwrap_or(WSAEINVAL.0)));
    };
    let mut control = Ancillary::default();
    control.source(source);
    let buffer = WSABUF {
        len: packet.len() as u32,
        buf: windows::core::PSTR(packet.as_ptr().cast_mut()),
    };
    send_message(socket, &[buffer], &socket2::SockAddr::from(peer), &control)
}
