//! Host send-loss tracking and test-only send fault injection.

use std::time::{Duration, Instant};

/// Packets of one frame the host itself failed to send, against what its FEC
/// can repair. On a host whose Wi-Fi drops out, the socket refuses packets;
/// once a block loses more than its parity, the client cannot decode the frame
/// or any frame after it until a keyframe. Moonlight asks for one only after
/// its next complete frame and a round trip; the host knows at once.
pub struct SendLoss {
    /// Data and parity shards of each FEC block, in send order.
    blocks: crate::packet::InlineBlocks<(usize, usize)>,
    /// Upper bound of lost packets in each block.
    lost: crate::packet::InlineBlocks<usize>,
    /// Packets handed to the socket so far, and how many it refused.
    offered: usize,
    refused: usize,
}
impl SendLoss {
    pub fn new(blocks: impl IntoIterator<Item = (usize, usize)>) -> Self {
        let blocks: crate::packet::InlineBlocks<_> = blocks.into_iter().collect();
        Self {
            lost: crate::packet::InlineBlocks::zeros(blocks.len()),
            blocks,
            offered: 0,
            refused: 0,
        }
    }
    /// A frame whose blocks are not all known yet, as when it is sent while
    /// the encoder still produces it. Add each with [`Self::push_block`]
    /// before its packets are recorded; the result then matches `new` with
    /// the whole layout.
    pub fn streamed() -> Self {
        Self::new([])
    }
    /// The next block of a streamed frame: its data and parity shards.
    pub fn push_block(&mut self, data: usize, parity: usize) {
        self.blocks.push((data, parity));
        self.lost.push(0);
    }
    /// Record a batch of the next `packets` packets, of which `dropped` were
    /// refused. Which ones is not known, so each block covered by the batch
    /// is charged as if they all fell in it. Returns whether the frame is
    /// beyond repair.
    pub fn sent(&mut self, packets: usize, dropped: usize) -> bool {
        let (first, last) = (self.offered, self.offered + packets);
        self.offered = last;
        self.refused += dropped.min(packets);
        if dropped > 0 {
            let mut start = 0;
            for ((data, parity), lost) in self.blocks.iter().zip(self.lost.iter_mut()) {
                let end = start + data + parity;
                let overlap = last.min(end).saturating_sub(first.max(start));
                *lost = (*lost + dropped.min(overlap)).min(data + parity);
                start = end;
            }
        }
        self.unrecoverable()
    }
    /// Whether the socket took any packet of this frame.
    pub fn reached_socket(&self) -> bool {
        self.refused < self.offered
    }
    pub fn unrecoverable(&self) -> bool {
        self.blocks
            .iter()
            .zip(self.lost.iter())
            .any(|((_, parity), lost)| lost > parity)
    }
}
/// Whether a frame lost on the host's own send makes the next frame a
/// keyframe. A frame refused from its first packet (the radio is out)
/// always does: a keyframe costs the link nothing while it is refused, and
/// the first frame through afterwards must be one. A frame lost after part
/// of it got in (a congested link) does too, unless a keyframe started this
/// way within the hold has not yet been sent whole: a keyframe every frame
/// would only add its size to the backlog. The client then asks for one
/// itself once a frame gets through, as Moonlight does to avoid congestion
/// collapse.
#[derive(Default)]
pub struct SendLossRecovery {
    /// When the last keyframe started this way was asked for, until one is
    /// sent whole.
    pending: Option<Instant>,
}
impl SendLossRecovery {
    pub fn lost(&mut self, now: Instant, period: Duration, reached_socket: bool) -> bool {
        let hold = (period * 4).max(Duration::from_millis(40));
        if reached_socket
            && self
                .pending
                .is_some_and(|at| now.saturating_duration_since(at) < hold)
        {
            return false;
        }
        self.pending = Some(now);
        true
    }
    /// A frame went out whole; a keyframe ends the hold.
    pub fn delivered(&mut self, keyframe: bool) {
        if keyframe {
            self.pending = None;
        }
    }
}
/// Test-only radio outage on the host's video socket, from
/// `BUTTERPOLLO_TEST_SEND_OUTAGE=every_ms:length_ms[:air]`: for `length` of
/// every `every`, starting one `every` into the stream, each video datagram is
/// refused as by a full socket. Loopback never loses packets, so this is how
/// a host A/B exercises recovery from a host-side Wi-Fi dropout. With `:air`
/// the datagrams are lost after the socket instead, as on a client's Wi-Fi:
/// the host counts them sent and only the client notices. With `:stall` there
/// is no schedule: a large frame meets a network adapter that stops draining
/// (see [`AdapterStall`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SendOutage {
    every: Duration,
    length: Duration,
    air: bool,
    stall: bool,
}
impl SendOutage {
    pub fn parse(value: &str) -> Option<Self> {
        let mut fields = value.trim().split(':').map(str::trim);
        let every = Duration::from_millis(fields.next()?.parse().ok()?);
        let length = Duration::from_millis(fields.next()?.parse().ok()?);
        let (air, stall) = match fields.next() {
            None => (false, false),
            Some("air") => (true, false),
            Some("stall") => (false, true),
            Some(_) => return None,
        };
        (fields.next().is_none() && length > Duration::ZERO && every > length).then_some(Self {
            every,
            length,
            air,
            stall,
        })
    }
    /// Lost in the air: the host sent it and does not know it was lost.
    pub fn in_air(&self) -> bool {
        self.air
    }
    /// The adapter stall a large frame starts, for `:stall`.
    pub fn stall(&self) -> Option<AdapterStall> {
        self.stall.then_some(AdapterStall {
            every: self.every,
            length: self.length,
            started: None,
            left: 0,
        })
    }
    pub fn from_env() -> Option<Self> {
        let outage = Self::parse(&std::env::var("BUTTERPOLLO_TEST_SEND_OUTAGE").ok()?);
        if let Some(outage) = outage {
            tracing::warn!(
                every_ms = outage.every.as_millis(),
                length_ms = outage.length.as_millis(),
                air = outage.air,
                stall = outage.stall,
                "BUTTERPOLLO_TEST_SEND_OUTAGE drops video datagrams on a schedule; unset it outside tests"
            );
        }
        outage
    }
    /// Whether the schedule refuses datagrams now; never for `:stall`.
    pub fn active(&self, started: Instant, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(started);
        !self.stall
            && elapsed >= self.every
            && Duration::from_nanos((elapsed.as_nanos() % self.every.as_nanos()) as u64)
                < self.length
    }
}
/// The video socket's send buffer (`SO_SNDBUF`, `butterpollo_win_io::net`).
pub const VIDEO_SOCKET_BUFFER: usize = 1 << 20;
/// Test-only adapter stall, from `BUTTERPOLLO_TEST_SEND_OUTAGE=every:length:stall`:
/// the #59 field event, a large frame meeting a network adapter that stopped
/// draining while a game started. At most once every `every`, a frame with more
/// than [`VIDEO_SOCKET_BUFFER`] on the wire starts a `length` stall. The socket
/// buffer takes the first [`VIDEO_SOCKET_BUFFER`] bytes sent in it; after that
/// the socket is full until the stall ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdapterStall {
    every: Duration,
    length: Duration,
    started: Option<Instant>,
    /// Bytes handed to the socket since the stall started.
    left: usize,
}
impl AdapterStall {
    /// A frame with `wire_bytes` on the wire so far is being sent: it starts
    /// a stall when it overflows the socket buffer and none started within
    /// `every`. Returns whether it did.
    pub fn frame(&mut self, wire_bytes: usize, now: Instant) -> bool {
        if wire_bytes <= VIDEO_SOCKET_BUFFER
            || self
                .started
                .is_some_and(|at| now.saturating_duration_since(at) < self.every)
        {
            return false;
        }
        self.started = Some(now);
        self.left = 0;
        true
    }
    /// A batch of `bytes` about to be sent at `now`: `None` when the socket
    /// takes it, or the stall's end while the socket is full.
    pub fn full_until(&mut self, bytes: usize, now: Instant) -> Option<Instant> {
        let end = self.started? + self.length;
        if now >= end {
            return None;
        }
        if self.left + bytes <= VIDEO_SOCKET_BUFFER {
            self.left += bytes;
            return None;
        }
        Some(end)
    }
}
/// Test-only Wi-Fi stalls on the host's video socket, from
/// `BUTTERPOLLO_TEST_SEND_JITTER=every_ms:max_ms[:seed]`: a stall starts
/// `every` times U(0.5, 1.5) after the one before (the first that far into
/// the stream) and lasts U(max/4, max), drawn from a generator seeded with
/// `seed` (1 unless given), so two runs stall alike. `max` may not exceed
/// `every`: longer stalls would outrun the gaps and hold the link for good.
/// While one lasts, video
/// datagrams wait in a [`JitterLine`] and go out in order as a burst when it
/// ends, as from an access point that held them. A wired link, or an access
/// point that isolates flows, never does that, so this is how a host A/B
/// gives a client a jittery link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SendJitter {
    every: Duration,
    max: Duration,
    state: u64,
    /// The current or next stall, from the stream's start.
    from: Duration,
    until: Duration,
}
impl SendJitter {
    pub fn parse(value: &str) -> Option<Self> {
        let mut fields = value.trim().split(':').map(str::trim);
        let every = Duration::from_millis(fields.next()?.parse().ok()?);
        let max = Duration::from_millis(fields.next()?.parse().ok()?);
        let state = match fields.next() {
            None => 1,
            Some(seed) => seed.parse().ok()?,
        };
        if fields.next().is_some() || every.is_zero() || max.is_zero() || max > every {
            return None;
        }
        let mut jitter = Self {
            every,
            max,
            state,
            from: Duration::ZERO,
            until: Duration::ZERO,
        };
        jitter.advance();
        Some(jitter)
    }
    /// The schedule in `BUTTERPOLLO_TEST_SEND_JITTER`, if any. A value that
    /// does not parse is warned about, not silently ignored.
    pub fn from_env() -> Option<Self> {
        let value = std::env::var("BUTTERPOLLO_TEST_SEND_JITTER").ok()?;
        let jitter = Self::parse(&value);
        if jitter.is_none() {
            tracing::warn!(
                value,
                "BUTTERPOLLO_TEST_SEND_JITTER is not every_ms:max_ms[:seed] with 0 < max_ms <= every_ms; ignored"
            );
        }
        jitter
    }
    /// Warns that the knob is set: it stalls the paced video sender, or, with
    /// `ignored_by` naming another send path, nothing at all.
    pub fn warn(&self, ignored_by: Option<&str>) {
        match ignored_by {
            None => tracing::warn!(
                every_ms = self.every.as_millis(),
                max_ms = self.max.as_millis(),
                "BUTTERPOLLO_TEST_SEND_JITTER holds video datagrams on a schedule; unset it outside tests"
            ),
            Some(path) => tracing::warn!(
                path,
                "BUTTERPOLLO_TEST_SEND_JITTER only stalls the paced video sender; this stream sends another way and ignores it"
            ),
        }
    }
    /// The end of the stall `elapsed` into the stream, or `None` outside one.
    /// Times must not go backwards.
    pub fn stalled_until(&mut self, elapsed: Duration) -> Option<Duration> {
        while elapsed >= self.until {
            self.advance();
        }
        (elapsed >= self.from).then_some(self.until)
    }
    fn advance(&mut self) {
        let every = self.every.as_nanos() as u64;
        let gap = every / 2 + split_mix(&mut self.state) % (every + 1);
        let max = self.max.as_nanos() as u64;
        let length = max / 4 + split_mix(&mut self.state) % (max - max / 4 + 1);
        // Back to back when a stall outlasts the gap to the next.
        self.from = (self.from + Duration::from_nanos(gap)).max(self.until);
        self.until = self.from + Duration::from_nanos(length);
    }
}
/// [`SendJitter`] as the access point that holds what reaches it during a
/// stall: sends while one lasts wait here and all go out, in the order they
/// came, the moment it ends (back-to-back stalls hold them through both).
/// The sender counts them as sent, so its pace, backlog and claims carry on
/// through the stall as they would on the real link.
#[derive(Clone, Debug)]
pub struct JitterLine<T> {
    jitter: SendJitter,
    held: std::collections::VecDeque<T>,
    /// When the held sends go out: the end of the stall that holds them.
    release: Duration,
}
impl<T> JitterLine<T> {
    pub fn new(jitter: SendJitter) -> Self {
        Self {
            jitter,
            held: Default::default(),
            release: Duration::ZERO,
        }
    }
    /// Whether a send `elapsed` into the stream has to wait: a stall lasts,
    /// or sends held before it have not gone yet. Times must not go
    /// backwards.
    pub fn holds(&mut self, elapsed: Duration) -> bool {
        self.stalled(elapsed) || !self.held.is_empty()
    }
    /// Queue a send that [`Self::holds`] said has to wait.
    pub fn hold(&mut self, send: T) {
        self.held.push_back(send);
    }
    /// The held sends, in order, once no stall lasts `elapsed` into the
    /// stream; none while one does.
    pub fn release(&mut self, elapsed: Duration) -> std::collections::vec_deque::Drain<'_, T> {
        let over = self.held.is_empty() || !self.stalled(elapsed);
        let end = if over { self.held.len() } else { 0 };
        self.held.drain(..end)
    }
    /// When the held sends go out, while any wait.
    pub fn release_at(&self) -> Option<Duration> {
        (!self.held.is_empty()).then_some(self.release)
    }
    fn stalled(&mut self, elapsed: Duration) -> bool {
        let stalled = self.jitter.stalled_until(elapsed);
        if let Some(end) = stalled {
            self.release = end;
        }
        stalled.is_some()
    }
}
/// SplitMix64, the test knobs' seeded generator.
fn split_mix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
/// Test-only random loss on the host's video socket, from
/// `BUTTERPOLLO_TEST_SEND_LOSS=percent:seed[:air]`: each video datagram is
/// lost with that probability (in percent, fractions allowed), decided by a
/// generator seeded with `seed`, so two runs lose the same datagrams in the
/// same order. Like [`SendOutage`], lost datagrams are refused as by a full
/// socket, or with `:air` counted sent and lost after the socket.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SendDrop {
    /// Loss probability in millionths.
    millionths: u64,
    state: u64,
    air: bool,
}
impl SendDrop {
    pub fn parse(value: &str) -> Option<Self> {
        let mut fields = value.trim().split(':').map(str::trim);
        let percent: f64 = fields.next()?.parse().ok()?;
        let seed: u64 = fields.next()?.parse().ok()?;
        let air = match fields.next() {
            None => false,
            Some("air") => true,
            Some(_) => return None,
        };
        (fields.next().is_none() && percent > 0. && percent <= 100.).then_some(Self {
            millionths: (percent * 10_000.).round() as u64,
            state: seed,
            air,
        })
    }
    pub fn from_env() -> Option<Self> {
        let loss = Self::parse(&std::env::var("BUTTERPOLLO_TEST_SEND_LOSS").ok()?);
        if let Some(loss) = &loss {
            tracing::warn!(
                loss_millionths = loss.millionths,
                air = loss.air,
                "BUTTERPOLLO_TEST_SEND_LOSS drops video datagrams at random; unset it outside tests"
            );
        }
        loss
    }
    /// Lost in the air: the host sent it and does not know it was lost.
    pub fn in_air(&self) -> bool {
        self.air
    }
    /// Whether the next datagram is lost.
    pub fn drops(&mut self) -> bool {
        split_mix(&mut self.state) % 1_000_000 < self.millionths
    }
}
