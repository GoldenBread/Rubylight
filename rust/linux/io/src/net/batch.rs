//! A stream's video and audio datagrams on Linux: single sends and batches that one `sendmsg`
//! carries with UDP segmentation (`UDP_SEGMENT`), sharing one wait for room in a full socket.
use super::{
    reply::ReplySocket,
    sys::{Ancillary, Errno, send_from, send_message},
    wait::{DatagramPass, WaitBudget, WritableWaits, buffer_available, send_with_retry, transient},
};
use anyhow::Result;
use std::{
    io::IoSlice,
    net::{IpAddr, SocketAddr, UdpSocket},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SegmentedFailure {
    DisableOffload,
    FallbackDatagrams,
    DropBatch,
    Fatal,
}
/// The errors of a segmented send that say segmentation is not to be had here: a kernel
/// before 4.18 or a size it cannot segment (`EINVAL`, `EMSGSIZE`), a protocol without it, or a
/// route whose device cannot checksum the segments (`EIO`).
pub(super) const NO_SEGMENTATION: [Errno; 5] = [
    libc::EINVAL,
    libc::ENOPROTOOPT,
    libc::EOPNOTSUPP,
    libc::EMSGSIZE,
    libc::EIO,
];
pub(super) fn classify_segmented_error(error: Errno) -> SegmentedFailure {
    if NO_SEGMENTATION.contains(&error) {
        SegmentedFailure::DisableOffload
    } else if error == libc::ENOBUFS {
        SegmentedFailure::FallbackDatagrams
    } else if transient(error) {
        SegmentedFailure::DropBatch
    } else {
        SegmentedFailure::Fatal
    }
}
/// One datagram from `source`, retried once when the socket buffer is full
/// and frees up within `budget`. The inner error is the OS's number.
pub(super) fn send_one(
    socket: &UdpSocket,
    packet: &[u8],
    peer: SocketAddr,
    source: Option<IpAddr>,
    budget: &mut WaitBudget,
    trace: Option<&mut WritableWaits>,
) -> Result<Result<(), Errno>> {
    let send = || send_from(socket, packet, peer, source);
    let mut trace = trace;
    let result = send_with_retry(send, |error| {
        buffer_available(socket, error, budget, trace.as_deref_mut())
    });
    Ok(match result {
        Ok(size) => {
            anyhow::ensure!(
                size == packet.len(),
                "Linux sent an incomplete UDP datagram"
            );
            Ok(())
        }
        Err(error) => Err(error),
    })
}
fn send_datagram_budget(
    socket: &ReplySocket,
    packet: &[u8],
    peer: SocketAddr,
    budget: &mut WaitBudget,
) -> Result<bool> {
    let (result, _) = socket.send_pinned(peer, socket.source(peer), |source| {
        send_one(socket, packet, peer, source, budget, None)
    })?;
    match result {
        Ok(()) => Ok(true),
        Err(error) if transient(error) => Ok(false),
        Err(error) => Err(std::io::Error::from_raw_os_error(error).into()),
    }
}
/// Shared wait budget across a group of datagrams sent via [`send_datagram_with_wait`].
#[derive(Default)]
pub struct DatagramWait(WaitBudget);
/// One datagram, from the address `peer` reached, sharing `wait` so a stalled
/// socket blocks at most [`WRITABLE_WAIT`](super::WRITABLE_WAIT) across the whole group;
/// `Ok(false)` when a transient error dropped it.
pub fn send_datagram_with_wait(
    socket: &ReplySocket,
    packet: &[u8],
    peer: SocketAddr,
    wait: &mut DatagramWait,
) -> Result<bool> {
    send_datagram_budget(socket, packet, peer, &mut wait.0)
}
/// One datagram, from the address `peer` reached; `Ok(false)` when a
/// transient error dropped it.
pub fn send_datagram(socket: &ReplySocket, packet: &[u8], peer: SocketAddr) -> Result<bool> {
    send_datagram_with_wait(socket, packet, peer, &mut DatagramWait::default())
}
/// One datagram using a [`DatagramPass`] shared across a capture pass so
/// `ENOBUFS` can recover while persistent refusal fails subsequent packets
/// immediately without multiplying retry sleeps.
pub fn send_datagram_pass(
    socket: &ReplySocket,
    packet: &[u8],
    peer: SocketAddr,
    pass: &mut DatagramPass,
) -> Result<bool> {
    let mut budget = WaitBudget::for_frame_with_enobufs(
        Some(pass.until),
        pass.deadline,
        Some(pass.enobufs_until),
        pass.enobufs_deadline,
    );
    let ok = send_datagram_budget(socket, packet, peer, &mut budget)?;
    pass.deadline = budget.deadline.filter(|_| !ok);
    pass.enobufs_deadline = budget.enobufs_deadline.filter(|_| !ok);
    Ok(ok)
}
/// Per-message segmentation leaves the shared socket's options untouched.
pub struct Batch {
    pub(super) offload: bool,
    pub system_calls: u64,
    /// Datagrams lost to transient send errors.
    pub dropped: u64,
    /// Packets handed to the socket in the last [`Batch::send`] call.
    last_sent: usize,
    /// Datagrams the path refused because they are larger than its MTU with
    /// fragmentation refused; also counted in `dropped`.
    pub oversized: u64,
    /// The sender's socket refuses fragmentation
    /// ([`set_dont_fragment`](super::set_dont_fragment)), as the PyroWave probe's does for a
    /// jumbo path, so an oversized datagram is a lost packet to report, not a send error that
    /// ends the stream.
    pub dont_fragment: bool,
    /// Enabled by the sender only for pacing traces.
    pub waits: Option<WritableWaits>,
    /// The frame in flight (see [`Batch::begin_frame`]): its own deadline,
    /// and the deadline its batches share once one met a full socket. `None`
    /// outside a frame, where one batch waits [`WRITABLE_WAIT`](super::WRITABLE_WAIT) on its
    /// own.
    pub(super) frame: Option<FrameWait>,
    unreported: u64,
    reported: Option<Instant>,
    /// The control data and address of the last segmented send, rebuilt only
    /// when the peer, the first packet's size or the source changes.
    segmented: Option<Segmented>,
}
impl Default for Batch {
    fn default() -> Self {
        Self {
            offload: true,
            system_calls: 0,
            dropped: 0,
            last_sent: 0,
            oversized: 0,
            dont_fragment: false,
            waits: None,
            frame: None,
            unreported: 0,
            reported: None,
            segmented: None,
        }
    }
}
#[derive(Clone, Copy)]
pub(super) struct FrameWait {
    until: Instant,
    pub(super) deadline: Option<Instant>,
    enobufs_until: Option<Instant>,
    enobufs_deadline: Option<Instant>,
}
pub(super) fn segmented_retries_in_place(error: Errno) -> bool {
    error != libc::ENOBUFS
}
impl Batch {
    /// Start one video frame: its batches share one wait for room in a full
    /// socket, up to `wait` from now or [`WRITABLE_WAIT`](super::WRITABLE_WAIT) from the
    /// frame's first refusal, whichever is later (see [`Batch::frame_wait_spent`]).
    pub fn begin_frame(&mut self, wait: Duration) {
        let now = Instant::now();
        self.frame = Some(FrameWait {
            until: now + wait,
            deadline: None,
            enobufs_until: None,
            enobufs_deadline: None,
        });
    }
    /// Start one video frame with a separate shared retry ceiling for
    /// transient `ENOBUFS` driver pressure, keeping a full buffer's wait at
    /// `wait` and adding zero delay when the socket accepts datagrams.
    pub fn begin_frame_with_enobufs(&mut self, wait: Duration, enobufs_wait: Duration) {
        let now = Instant::now();
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
        let now = Instant::now();
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
    /// Count dropped datagrams and report them at most every 5 seconds.
    fn dropped(&mut self, count: usize, error: Errno) {
        if let Some(trace) = self.waits.as_mut() {
            trace.last_error = error;
        }
        self.dropped += count as u64;
        self.unreported += count as u64;
        if self
            .reported
            .is_none_or(|at| at.elapsed() >= Duration::from_secs(5))
        {
            tracing::warn!(
                code = error,
                dropped = self.unreported,
                "UDP send failed: the socket stayed full, so packets were dropped and the client may stutter. The PC could not send fast enough (its CPU or network adapter fell behind); close other work on the PC or lower the bitrate"
            );
            self.unreported = 0;
            self.reported = Some(Instant::now());
        }
    }
    /// Count datagrams as refused by a full socket without sending them,
    /// for `BUTTERPOLLO_TEST_SEND_OUTAGE`.
    pub fn refuse(&mut self, count: usize) {
        self.last_sent = 0;
        self.dropped(count, libc::EAGAIN);
    }
    /// Number of packets sent in the last [`Batch::send`] call.
    pub fn sent_packets(&self) -> usize {
        self.last_sent
    }
    /// Count datagrams the path refused for their size with fragmentation
    /// refused: lost like a full buffer's, and reported once every 5 seconds as
    /// the jumbo path's fault rather than a transient socket error.
    pub(super) fn too_large(&mut self, count: usize, error: Errno) {
        if let Some(trace) = self.waits.as_mut() {
            trace.last_error = error;
        }
        self.dropped += count as u64;
        self.oversized += count as u64;
        if self
            .reported
            .is_none_or(|at| at.elapsed() >= Duration::from_secs(5))
        {
            tracing::warn!(
                code = error,
                dropped = count,
                "video datagrams exceed the path MTU and are dropped: the offered packet size plus 64 bytes of RTP, UDP and IPv6 headers must fit every hop's MTU"
            );
            self.reported = Some(Instant::now());
        }
    }
    /// Bytes handed to the socket. Datagrams that hit a transient error are
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
            && packets
                .last()
                .is_some_and(|last| (1..=packets[0].len()).contains(&last.len()))
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
                    // EINVAL or EMSGSIZE here can be segmentation's own limit, not the
                    // path's MTU: turn segmentation off and send the datagrams one
                    // by one, where an oversized one is counted as such. Dropping
                    // the batch would leave every later batch dropped as well.
                    SegmentedFailure::DisableOffload => {
                        self.offload = false;
                        tracing::warn!(
                            code = error,
                            "UDP segmentation unavailable or this batch could not be segmented; using individual datagrams with more CPU overhead. Update the kernel or the network driver, or lower bitrate if sending stalls"
                        );
                    }
                    // Transient ENOBUFS on a multi-packet segmented buffer can still
                    // leave room for individual datagrams; keep offload enabled
                    // for later batches once the pressure clears.
                    SegmentedFailure::FallbackDatagrams => {}
                    SegmentedFailure::DropBatch => {
                        self.dropped(packets.len(), error);
                        return Ok(0);
                    }
                    SegmentedFailure::Fatal => {
                        return Err(std::io::Error::from_raw_os_error(error).into());
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
                Err(error) if error == libc::EMSGSIZE => {
                    self.too_large(1, error);
                }
                Err(error) if transient(error) => {
                    self.dropped(packets.len() - index, error);
                    break;
                }
                Err(error) => return Err(std::io::Error::from_raw_os_error(error).into()),
            }
        }
        Ok(sent)
    }
    /// One `sendmsg` with UDP segmentation from `source`, retried once when
    /// the socket buffer is full and frees up within `budget`. The inner error
    /// is the OS's number.
    pub(super) fn send_segmented(
        &mut self,
        socket: &UdpSocket,
        packets: &[Vec<u8>],
        peer: SocketAddr,
        source: Option<IpAddr>,
        budget: &mut WaitBudget,
    ) -> Result<Result<usize, Errno>> {
        // A datagram this long is no UDP datagram: sent alone, it is counted as too large.
        let Some(size) = packets.first().and_then(|p| u16::try_from(p.len()).ok()) else {
            return Ok(Err(libc::EMSGSIZE));
        };
        // Rebuild the address and control data only when they changed.
        if self
            .segmented
            .as_ref()
            .is_none_or(|s| s.peer != peer || s.size != size || s.source != source)
        {
            let mut control = Ancillary::default();
            control.segment(size);
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
        let Some(segmented) = self.segmented.as_ref() else {
            return Ok(Err(libc::EINVAL));
        };
        let mut buffers = [IoSlice::new(&[]); 64];
        let mut expected = 0usize;
        for (buffer, packet) in buffers.iter_mut().zip(packets) {
            *buffer = IoSlice::new(packet);
            expected += packet.len();
        }
        let buffers = &buffers[..packets.len().min(64)];
        let system_calls = &mut self.system_calls;
        let result = send_with_retry(
            || {
                *system_calls += 1;
                send_message(socket, buffers, &segmented.address, &segmented.control)
            },
            // Transient ENOBUFS on a multi-packet segmented buffer falls back immediately
            // to individual datagrams (which get the full enobufs_deadline) without
            // spending the recovery budget inside send_segmented or disabling offload.
            |error| {
                segmented_retries_in_place(error)
                    && buffer_available(socket, error, budget, self.waits.as_mut())
            },
        );
        Ok(match result {
            Ok(sent) => {
                anyhow::ensure!(sent == expected, "Linux sent an incomplete UDP batch");
                Ok(sent)
            }
            Err(error) => Err(error),
        })
    }
}
/// The control data and address of one segmented send, kept so a frame's
/// batches rebuild them only when the peer, the first packet's size or the
/// source changes.
struct Segmented {
    peer: SocketAddr,
    size: u16,
    source: Option<IpAddr>,
    control: Ancillary,
    address: socket2::SockAddr,
}
