//! How long a send waits for room in a full socket buffer on Linux before its datagrams are
//! dropped: the budgets one send call, one video frame and one audio pass share.
use super::sys::Errno;
use std::{
    net::UdpSocket,
    os::fd::AsRawFd,
    time::{Duration, Instant},
};

/// Send errors after which the socket still works and only these datagrams are lost: a full
/// send buffer, no buffer space, a route that is down for the moment, or a call a signal
/// interrupted. `ECONNREFUSED` is how Linux reports the ICMP answer of a closed port on a
/// connected socket. Vibepollo drops the packets and keeps streaming; so do we.
pub(super) fn transient(error: Errno) -> bool {
    [
        libc::EAGAIN,
        libc::ENOBUFS,
        libc::ENETDOWN,
        libc::ENETUNREACH,
        libc::ENETRESET,
        libc::EHOSTUNREACH,
        libc::EHOSTDOWN,
        libc::ECONNRESET,
        libc::ECONNREFUSED,
        libc::EADDRNOTAVAIL,
        libc::EINTR,
    ]
    .contains(&error)
}

/// How long one send, or all of one batch's sends together, wait for room in
/// a full socket buffer before dropping. Per datagram, 64 refused datagrams
/// would hold the sender for 256 ms. Within a video frame
/// ([`Batch::begin_frame`](super::Batch::begin_frame)) all its batches share one deadline: the
/// later of the frame's own (about a frame period from its start, at most 20 ms) and
/// this long after the frame's first refusal.
pub const WRITABLE_WAIT: Duration = Duration::from_millis(4);
pub(super) const BUFFER_RETRY_DELAY: Duration = Duration::from_millis(1);
/// Default shared transient `ENOBUFS` retry budget for one audio send pass.
pub const AUDIO_ENOBUFS_WAIT: Duration = Duration::from_millis(12);

/// Shared retry deadline across one datagram send pass (such as all audio
/// packets captured in one pass) so a congested socket does not multiply waits
/// per packet beyond one audio interval.
#[derive(Clone, Copy, Debug)]
pub struct DatagramPass {
    pub(super) until: Instant,
    pub(super) enobufs_until: Instant,
    pub(super) deadline: Option<Instant>,
    pub(super) enobufs_deadline: Option<Instant>,
}

impl DatagramPass {
    /// Start a pass capped at [`WRITABLE_WAIT`] for a full buffer (`EAGAIN`) and
    /// `enobufs_wait` for `ENOBUFS`.
    pub fn new(enobufs_wait: Duration) -> Self {
        let now = Instant::now();
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
    pub(super) deadline: Option<Instant>,
    /// The frame's own deadline ([`Batch::begin_frame`](super::Batch::begin_frame)), if one is
    /// in flight.
    frame_until: Option<Instant>,
    /// Optional separate retry window for transient `ENOBUFS` pressure so
    /// startup bursts and high-refresh frames recover without slowing down
    /// full-buffer waits or adding latency to successful sends.
    pub(super) enobufs_deadline: Option<Instant>,
    enobufs_until: Option<Instant>,
    /// Full-buffer refusals seen by this send call.
    refusals: u32,
}
impl WaitBudget {
    /// A budget shared by every batch of the frame in flight: `deadline` is
    /// the frame's, once a batch of it met a full socket, so a socket that
    /// stays full is not waited on again for each batch and the rest of that
    /// frame is abandoned instead; `until` is the frame's own deadline.
    #[cfg(test)]
    pub(super) fn for_frame(until: Option<Instant>, deadline: Option<Instant>) -> Self {
        Self::for_frame_with_enobufs(until, deadline, None, None)
    }
    pub(super) fn for_frame_with_enobufs(
        until: Option<Instant>,
        deadline: Option<Instant>,
        enobufs_until: Option<Instant>,
        enobufs_deadline: Option<Instant>,
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
    fn deadline_at(&mut self, now: Instant) -> Instant {
        let wait = now + WRITABLE_WAIT;
        *self
            .deadline
            .get_or_insert(self.frame_until.map_or(wait, |until| until.max(wait)))
    }
    pub(super) fn deadline_for(&mut self, now: Instant, enobufs: bool) -> Instant {
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
    pub(super) fn take(&mut self, now: Instant) -> i32 {
        let left = self.deadline_at(now).saturating_duration_since(now);
        left.as_micros().div_ceil(1000) as i32
    }
    /// Reserve a short pause only when it leaves time for the retry. All
    /// datagrams in a batch share this deadline, including time already spent
    /// polling a full socket or sending earlier datagrams.
    pub(super) fn backoff_for(&mut self, now: Instant, enobufs: bool) -> Option<Instant> {
        let retry = now + BUFFER_RETRY_DELAY;
        (retry < self.deadline_for(now, enobufs)).then_some(retry)
    }
    #[cfg(test)]
    pub(super) fn backoff_at(&mut self, now: Instant) -> Option<Instant> {
        self.backoff_for(now, false)
    }
}

/// The waits a sender spent on a full socket, counted only when its pacing is traced.
#[derive(Clone, Copy, Default)]
pub struct WritableWaits {
    pub count: u64,
    pub elapsed: Duration,
    /// Last transient socket refusal (a Linux error number), including one followed by a
    /// successful retry.
    pub last_error: i32,
}

/// `ENOBUFS` can come from allocation pressure in the network stack or the driver while the
/// socket still polls writable. Pause briefly before a retry rather than hot-polling
/// that readiness bit or dropping an entire segmented batch immediately. A
/// repeated `EAGAIN` pauses the same way before it polls again.
pub(super) fn buffer_available(
    socket: &UdpSocket,
    error: Errno,
    budget: &mut WaitBudget,
    mut trace: Option<&mut WritableWaits>,
) -> bool {
    if let Some(trace) = trace.as_mut() {
        trace.last_error = error;
    }
    let again = budget.refused_again();
    if error == libc::EAGAIN {
        if again && !pause(budget, false, trace.as_deref_mut()) {
            return false;
        }
        return writable(socket, budget, trace);
    }
    debug_assert_eq!(error, libc::ENOBUFS);
    pause(budget, true, trace)
}

/// Wait [`BUFFER_RETRY_DELAY`] if the budget leaves time for a retry after
/// it; false when it does not.
fn pause(budget: &mut WaitBudget, enobufs: bool, trace: Option<&mut WritableWaits>) -> bool {
    let started = Instant::now();
    let Some(retry_at) = budget.backoff_for(started, enobufs) else {
        return false;
    };
    std::thread::sleep(retry_at.saturating_duration_since(started));
    let finished = Instant::now();
    if let Some(trace) = trace {
        trace.count += 1;
        trace.elapsed += finished.saturating_duration_since(started);
    }
    // A late wake must not revive an expired batch budget.
    finished < budget.deadline_for(started, enobufs)
}

/// A successful UDP send must never be repeated. Retry buffer pressure for as
/// long as the caller's budget allows, so a socket that is full for a moment
/// (a busy CPU, a late transmit completion) carries the batch instead of
/// losing it; `wait` paces the retries ([`buffer_available`] pauses 1 ms
/// before each after the first). All other errors retain their existing
/// drop/fatal handling.
pub(super) fn send_with_retry<T>(
    mut send: impl FnMut() -> Result<T, Errno>,
    mut wait: impl FnMut(Errno) -> bool,
) -> Result<T, Errno> {
    loop {
        match send() {
            Err(error) if [libc::EAGAIN, libc::ENOBUFS].contains(&error) && wait(error) => continue,
            result => return result,
        }
    }
}

/// Waits for room in `socket`'s send buffer for what is left of `budget`; whether there is
/// room.
pub(super) fn writable(
    socket: &UdpSocket,
    budget: &mut WaitBudget,
    trace: Option<&mut WritableWaits>,
) -> bool {
    let timeout_ms = budget.take(Instant::now());
    if timeout_ms == 0 {
        return false;
    }
    let started = trace.as_ref().map(|_| Instant::now());
    let mut poll = [libc::pollfd {
        fd: socket.as_raw_fd(),
        events: libc::POLLOUT,
        revents: 0,
    }];
    // SAFETY: `poll` is a one-element array that outlives the call, for a socket the borrow
    // keeps open.
    let ready = (unsafe { libc::poll(poll.as_mut_ptr(), 1, timeout_ms) }) > 0
        && poll[0].revents & libc::POLLOUT != 0;
    if let (Some(trace), Some(started)) = (trace, started) {
        trace.count += 1;
        trace.elapsed += started.elapsed();
    }
    ready
}
