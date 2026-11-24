//! UDP socket configuration, readiness polling, transient-error retry budgets, and single-datagram sends.
use super::{batch::send_from, reply::ReplySocket};
use anyhow::Result;
use std::{
    net::{IpAddr, SocketAddr, UdpSocket},
    os::windows::io::AsRawSocket,
};
use windows::Win32::Networking::WinSock::*;

/// Default UDP send buffer applied by [`configure_udp`] (matches WebRTC media).
pub const SEND_BUFFER_DEFAULT: usize = 4 << 20;

pub fn configure_udp(socket: &UdpSocket) -> Result<()> {
    let disabled = 0u32;
    let mut returned = 0;
    // SAFETY: The borrowed socket stays open, `disabled` is the 4-byte input buffer, and the call
    // is synchronous with no output buffer.
    if unsafe {
        WSAIoctl(
            SOCKET(socket.as_raw_socket() as usize),
            SIO_UDP_CONNRESET,
            Some((&disabled as *const u32).cast()),
            4,
            None,
            0,
            &mut returned,
            None,
            None,
        )
    } == SOCKET_ERROR
    {
        // SAFETY: WSAGetLastError only reads this thread's last Winsock error.
        return Err(std::io::Error::from_raw_os_error(unsafe { WSAGetLastError() }.0).into());
    }
    // Vibepollo's video socket buffer: 4 MiB absorbs startup keyframe and
    // initial audio bursts without the non-blocking socket refusing them.
    socket2::SockRef::from(socket).set_send_buffer_size(SEND_BUFFER_DEFAULT)?;
    Ok(())
}

/// Set whether this socket refuses to fragment its later datagrams:
/// `IP_DONTFRAGMENT` and `IPV6_DONTFRAG`, so an oversized probe packet fails
/// with WSAEMSGSIZE at the sender instead of being fragmented and lost whole
/// on a hop that cannot reassemble it. Both options are tried because a
/// dual-stack socket carries both families; the one the socket does not carry
/// refuses its option harmlessly. The PyroWave probe sets it on its own socket
/// and clears it again; the shared video socket never sets it, since the flag
/// would govern every stream using that process-wide socket.
pub fn set_dont_fragment(socket: &UdpSocket, enabled: bool) -> Result<()> {
    let raw = SOCKET(socket.as_raw_socket() as usize);
    let value = u32::from(enabled).to_ne_bytes();
    let mut applied = false;
    let mut last = None;
    for (level, option) in [(IPPROTO_IP, IP_DONTFRAGMENT), (IPPROTO_IPV6, IPV6_DONTFRAG)] {
        // SAFETY: the borrowed socket stays open, and the option takes this 4-byte BOOL.
        if unsafe { setsockopt(raw, level.0, option, Some(&value)) } == SOCKET_ERROR {
            // SAFETY: WSAGetLastError only reads this thread's last Winsock error.
            let error = unsafe { WSAGetLastError() };
            // A family the socket does not carry refuses only its own option.
            if !matches!(error, WSAENOPROTOOPT | WSAEINVAL | WSAEPROTONOSUPPORT) {
                return Err(std::io::Error::from_raw_os_error(error.0).into());
            }
            last = Some(error);
        } else {
            applied = true;
        }
    }
    if applied {
        Ok(())
    } else {
        Err(std::io::Error::from_raw_os_error(last.unwrap_or(WSAEINVAL).0).into())
    }
}

/// Send errors after which the socket still works and only these datagrams
/// are lost: a full send buffer, no buffer space, or a route that is down for
/// the moment. Vibepollo drops the packets and keeps streaming; so do we.
pub(super) fn transient(error: WSA_ERROR) -> bool {
    [
        WSAEWOULDBLOCK,
        WSAENOBUFS,
        WSAENETDOWN,
        WSAENETUNREACH,
        WSAENETRESET,
        WSAEHOSTUNREACH,
        WSAEHOSTDOWN,
        WSAECONNRESET,
        WSAEADDRNOTAVAIL,
    ]
    .contains(&error)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SegmentedFailure {
    DisableOffload,
    FallbackDatagrams,
    DropBatch,
    Fatal,
}

pub(super) fn classify_segmented_error(error: WSA_ERROR) -> SegmentedFailure {
    if [WSAEINVAL, WSAENOPROTOOPT, WSAEOPNOTSUPP, WSAEMSGSIZE].contains(&error) {
        SegmentedFailure::DisableOffload
    } else if error == WSAENOBUFS {
        SegmentedFailure::FallbackDatagrams
    } else if transient(error) {
        SegmentedFailure::DropBatch
    } else {
        SegmentedFailure::Fatal
    }
}

/// A send or receive error that loses only this datagram; the socket keeps
/// working. A receive also reports a datagram too large for its buffer this
/// way (WSAEMSGSIZE) after discarding it.
pub fn datagram_lost(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::WouldBlock
        || error.raw_os_error().is_some_and(|code| {
            let code = WSA_ERROR(code);
            transient(code) || code == WSAEMSGSIZE
        })
}

/// How long one send, or all of one batch's sends together, wait for room in
/// a full socket buffer before dropping. Per datagram, 64 refused datagrams
/// would hold the sender for 256 ms. Within a video frame
/// ([`super::Batch::begin_frame`]) all its batches share one deadline: the later of
/// the frame's own (about a frame period from its start, at most 20 ms) and
/// this long after the frame's first refusal.
pub const WRITABLE_WAIT: std::time::Duration = std::time::Duration::from_millis(4);
pub(super) const BUFFER_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(1);
/// Default shared transient `WSAENOBUFS` retry budget for one audio send pass.
pub const AUDIO_ENOBUFS_WAIT: std::time::Duration = std::time::Duration::from_millis(12);

/// Shared retry deadline across one datagram send pass (such as all audio
/// packets captured in one pass) so a congested socket does not multiply waits
/// per packet beyond one audio interval.
#[derive(Clone, Copy, Debug)]
pub struct DatagramPass {
    until: std::time::Instant,
    enobufs_until: std::time::Instant,
    pub(super) deadline: Option<std::time::Instant>,
    pub(super) enobufs_deadline: Option<std::time::Instant>,
}

impl DatagramPass {
    /// Start a pass capped at [`WRITABLE_WAIT`] for `WSAEWOULDBLOCK` and
    /// `enobufs_wait` for `WSAENOBUFS`.
    pub fn new(enobufs_wait: std::time::Duration) -> Self {
        let now = std::time::Instant::now();
        Self {
            until: now + WRITABLE_WAIT,
            enobufs_until: now + enobufs_wait,
            deadline: None,
            enobufs_deadline: None,
        }
    }
}

/// The writable wait left to one send call, started at its first full buffer.
#[derive(Default)]
pub(super) struct WaitBudget {
    pub(super) deadline: Option<std::time::Instant>,
    /// The frame's own deadline ([`super::Batch::begin_frame`]), if one is in flight.
    pub(super) frame_until: Option<std::time::Instant>,
    /// Optional separate retry window for transient `WSAENOBUFS` pressure so
    /// startup bursts and high-refresh frames recover without slowing down
    /// `WSAEWOULDBLOCK` waits or adding latency to successful sends.
    pub(super) enobufs_deadline: Option<std::time::Instant>,
    pub(super) enobufs_until: Option<std::time::Instant>,
    /// Full-buffer refusals seen by this send call.
    pub(super) refusals: u32,
    pub(super) backoff_timer: Option<crate::os_helpers::Timer>,
}
impl WaitBudget {
    /// A budget shared by every batch of the frame in flight: `deadline` is
    /// the frame's, once a batch of it met a full socket, so a socket that
    /// stays full is not waited on again for each batch and the rest of that
    /// frame is abandoned instead; `until` is the frame's own deadline.
    #[cfg(test)]
    pub(super) fn for_frame(
        until: Option<std::time::Instant>,
        deadline: Option<std::time::Instant>,
    ) -> Self {
        Self::for_frame_with_enobufs(until, deadline, None, None)
    }
    pub(super) fn for_frame_with_enobufs(
        until: Option<std::time::Instant>,
        deadline: Option<std::time::Instant>,
        enobufs_until: Option<std::time::Instant>,
        enobufs_deadline: Option<std::time::Instant>,
    ) -> Self {
        Self {
            deadline,
            frame_until: until,
            enobufs_deadline,
            enobufs_until,
            ..Default::default()
        }
    }
    /// The deadline, set at the first full buffer: [`WRITABLE_WAIT`] from
    /// then, or the frame's own deadline if that is later. A frame that took
    /// most of its period to pace out still gets the usual wait.
    fn deadline_at(&mut self, now: std::time::Instant) -> std::time::Instant {
        let wait = now + WRITABLE_WAIT;
        *self
            .deadline
            .get_or_insert(self.frame_until.map_or(wait, |until| until.max(wait)))
    }
    pub(super) fn deadline_for(
        &mut self,
        now: std::time::Instant,
        enobufs: bool,
    ) -> std::time::Instant {
        if enobufs && (self.enobufs_until.is_some() || self.enobufs_deadline.is_some()) {
            let wait = now + WRITABLE_WAIT;
            *self
                .enobufs_deadline
                .get_or_insert(self.enobufs_until.map_or(wait, |until| until.max(wait)))
        } else {
            self.deadline_at(now)
        }
    }
    /// Count a full-buffer refusal; true from the second on, when the retry
    /// should pause first. A socket can poll writable and still refuse a
    /// large segmented batch, and polling alone would spin on it.
    fn refused_again(&mut self) -> bool {
        self.refusals += 1;
        self.refusals > 1
    }
    /// Milliseconds to wait for a full buffer now; 0 once the budget is spent.
    pub(super) fn take(&mut self, now: std::time::Instant) -> i32 {
        let left = self.deadline_at(now).saturating_duration_since(now);
        left.as_micros().div_ceil(1000) as i32
    }
    /// Reserve a short pause only when it leaves time for the retry. All
    /// datagrams in a batch share this deadline, including time already spent
    /// polling a full socket or sending earlier datagrams.
    pub(super) fn backoff_for(
        &mut self,
        now: std::time::Instant,
        enobufs: bool,
    ) -> Option<std::time::Instant> {
        let retry = now + BUFFER_RETRY_DELAY;
        (retry < self.deadline_for(now, enobufs)).then_some(retry)
    }
    #[cfg(test)]
    pub(super) fn backoff_at(&mut self, now: std::time::Instant) -> Option<std::time::Instant> {
        self.backoff_for(now, false)
    }
}

/// Wait until `socket` has a datagram to read, or `timeout_ms` passes;
/// returns whether one is there. Unlike a fixed sleep, input waiting on the
/// socket is handled the moment it arrives.
pub fn wait_readable(
    socket: std::os::windows::io::RawSocket,
    timeout_ms: i32,
) -> std::io::Result<bool> {
    wait_readable_sockets([socket], timeout_ms)
}

/// Include host feedback in the same wait as incoming control traffic.
pub fn wait_readable_or_wake(
    socket: std::os::windows::io::RawSocket,
    wake: std::os::windows::io::RawSocket,
    timeout_ms: i32,
) -> std::io::Result<bool> {
    wait_readable_sockets([socket, wake], timeout_ms)
}

fn wait_readable_sockets<const N: usize>(
    sockets: [std::os::windows::io::RawSocket; N],
    timeout_ms: i32,
) -> std::io::Result<bool> {
    let mut poll = sockets.map(|socket| WSAPOLLFD {
        fd: SOCKET(socket as usize),
        events: POLLRDNORM,
        revents: WSAPOLL_EVENT_FLAGS(0),
    });
    // SAFETY: `poll` holds N entries and outlives the call; a stale socket only makes
    // WSAPoll fail.
    match unsafe { WSAPoll(poll.as_mut_ptr(), N as u32, timeout_ms) } {
        ready if ready >= 0 => Ok(ready > 0),
        _ => Err(std::io::Error::from_raw_os_error(
            // SAFETY: WSAGetLastError only reads this thread's last Winsock error.
            unsafe { WSAGetLastError() }.0,
        )),
    }
}

#[derive(Clone, Copy, Default)]
pub struct WritableWaits {
    pub count: u64,
    pub elapsed: std::time::Duration,
    /// Last transient socket refusal, including one followed by a successful retry.
    pub last_error: i32,
}

/// ENOBUFS can come from transport/driver allocation pressure while the socket
/// still polls writable. Pause briefly before a retry rather than hot-polling
/// that readiness bit or dropping an entire segmented batch immediately. A
/// repeated WSAEWOULDBLOCK pauses the same way before it polls again.
pub(super) fn buffer_available(
    socket: &UdpSocket,
    error: WSA_ERROR,
    budget: &mut WaitBudget,
    mut trace: Option<&mut WritableWaits>,
) -> bool {
    if let Some(trace) = trace.as_mut() {
        trace.last_error = error.0;
    }
    let again = budget.refused_again();
    if error == WSAEWOULDBLOCK {
        if again && !pause(budget, false, trace.as_deref_mut()) {
            return false;
        }
        return writable(socket, budget, trace);
    }
    debug_assert_eq!(error, WSAENOBUFS);
    pause(budget, true, trace)
}

/// Wait [`BUFFER_RETRY_DELAY`] if the budget leaves time for a retry after
/// it; false when it does not.
fn pause(budget: &mut WaitBudget, enobufs: bool, trace: Option<&mut WritableWaits>) -> bool {
    let started = std::time::Instant::now();
    let Some(retry_at) = budget.backoff_for(started, enobufs) else {
        return false;
    };
    if budget.backoff_timer.is_none() {
        budget.backoff_timer = crate::os_helpers::Timer::new().ok();
    }
    let Some(timer) = budget.backoff_timer.as_ref() else {
        return false;
    };
    timer.until(retry_at);
    let finished = std::time::Instant::now();
    if let Some(trace) = trace {
        trace.count += 1;
        trace.elapsed += finished.saturating_duration_since(started);
    }
    // A delayed timer wake must not revive an expired batch budget.
    finished < budget.deadline_for(started, enobufs)
}

/// A successful UDP send must never be repeated. Retry buffer pressure for as
/// long as the caller's budget allows, so a socket that is full for a moment
/// (a busy CPU, a late transmit completion) carries the batch instead of
/// losing it; `wait` paces the retries ([`buffer_available`] pauses 1 ms
/// before each after the first). All other errors retain their existing
/// drop/fatal handling.
pub(super) fn send_with_retry<T>(
    mut send: impl FnMut() -> std::result::Result<T, WSA_ERROR>,
    mut wait: impl FnMut(WSA_ERROR) -> bool,
) -> std::result::Result<T, WSA_ERROR> {
    loop {
        match send() {
            Err(error) if [WSAEWOULDBLOCK, WSAENOBUFS].contains(&error) && wait(error) => continue,
            result => return result,
        }
    }
}

pub(super) fn writable(
    socket: &UdpSocket,
    budget: &mut WaitBudget,
    trace: Option<&mut WritableWaits>,
) -> bool {
    let timeout_ms = budget.take(std::time::Instant::now());
    if timeout_ms == 0 {
        return false;
    }
    let started = trace.as_ref().map(|_| std::time::Instant::now());
    let mut poll = [WSAPOLLFD {
        fd: SOCKET(socket.as_raw_socket() as usize),
        events: POLLWRNORM,
        revents: WSAPOLL_EVENT_FLAGS(0),
    }];
    // SAFETY: `poll` is a one-element array that outlives the call, for a socket the borrow keeps
    // open.
    let ready = (unsafe { WSAPoll(poll.as_mut_ptr(), 1, timeout_ms) }) > 0
        && poll[0].revents.0 & POLLWRNORM.0 != 0;
    if let (Some(trace), Some(started)) = (trace, started) {
        trace.count += 1;
        trace.elapsed += started.elapsed();
    }
    ready
}

/// One datagram from `source`, retried once when the socket buffer is full
/// and frees up within `budget`. The inner error is Winsock's.
pub(super) fn send_one(
    socket: &UdpSocket,
    packet: &[u8],
    peer: SocketAddr,
    source: Option<IpAddr>,
    budget: &mut WaitBudget,
    trace: Option<&mut WritableWaits>,
) -> Result<std::result::Result<(), WSA_ERROR>> {
    let send = || send_from(socket, packet, peer, source);
    let mut trace = trace;
    let result = send_with_retry(send, |error| {
        buffer_available(socket, error, budget, trace.as_deref_mut())
    });
    Ok(match result {
        Ok(size) => {
            anyhow::ensure!(
                size == packet.len(),
                "Winsock returned an incomplete UDP datagram"
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
        Err(error) => Err(std::io::Error::from_raw_os_error(error.0).into()),
    }
}

/// Shared wait budget across a group of datagrams sent via [`send_datagram_with_wait`].
#[derive(Default)]
pub struct DatagramWait(WaitBudget);

/// One datagram, from the address `peer` reached, sharing `wait` so a stalled
/// socket blocks at most [`WRITABLE_WAIT`] across the whole group; `Ok(false)`
/// when a transient error dropped it.
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
/// `WSAENOBUFS` can recover while persistent refusal fails subsequent packets
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
