//! Where the encoded audio goes. `Media::audio` captures and encodes the same way for both
//! transports; only the last step differs: GameStream wraps each Opus packet in RTP with FEC and
//! sends it to the client's audio port, WebRTC hands it to the media thread's Opus track.

use super::qos::Tagged;
use crate::state::Session;
use crate::webrtc::media::{AudioPacket, AudioSender, Submit};
use anyhow::Result;
use butterpollo_core::{audio::HostLoss, packet::AudioPacketizer};
use butterpollo_windows::{audio::Loopback, net::ReplySocket};
use std::{
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

pub(super) trait AudioSink {
    /// Whether the far end is known. Audio is encoded either way (the capture must be drained),
    /// and a sink that is not ready when the ping window passes raises [`PingWait::WARNING`].
    fn ready(&mut self) -> bool;
    /// Start one audio capture pass so its datagrams share a single transient-buffer wait.
    fn begin_pass(&mut self) {}
    /// Send one encoded Opus packet. Returns how many packets did not go out, which the caller
    /// counts as loss; only a broken transport is an error. `position` is where the packet starts
    /// in the audio thread's stream, in 48 kHz samples, counting every packet and any audio the
    /// endpoint lost, whether or not it was sent.
    fn send(&mut self, opus: &[u8], captured: Instant, position: u64) -> Result<usize>;
    /// Send one AC3 or E-AC3 frame in its `parts`, which share an RTP timestamp. Only
    /// GameStream negotiates passthrough codecs.
    fn send_frame(
        &mut self,
        _parts: &[Vec<u8>],
        _captured: Instant,
        _position: u64,
    ) -> Result<usize> {
        anyhow::bail!("this transport carries only Opus audio")
    }
}

/// Whether a sink that is not ready means the client's audio pings timed out: no ping within
/// `timeout` of the stream's `start` or its last roam. A session that is ending (it stopped, or
/// the host is stopping) forgets its peers right after it stops, so a pass that started before
/// the stop finds no peer: that is the quit, not a timeout.
pub(super) fn ping_timed_out(
    session: &Session,
    host_stop: &AtomicBool,
    start: Instant,
    timeout: Duration,
) -> bool {
    let stopping = session.stopping() || host_stop.load(Ordering::Acquire);
    !stopping && session.media_wait_start(start).elapsed() > timeout
}

/// The client's audio ping, waited for as long as the session lives. A missed ping window raises
/// a warning but keeps the audio thread: the ping can still come (a ping from a shared address is
/// only accepted once no other session holds it), and the client hears sound from then on.
#[derive(Default)]
pub(super) struct PingWait {
    missing: bool,
}

impl PingWait {
    pub(super) const WARNING: &'static str = "audio_ping";

    /// One pass of the audio thread: `ready` is whether the client's audio address is known,
    /// `timed_out` is [`ping_timed_out`], asked only while the sink is not ready.
    pub(super) fn update(
        &mut self,
        ready: bool,
        timed_out: impl FnOnce() -> bool,
        warnings: &butterpollo_core::session::Warnings,
    ) {
        if ready {
            if std::mem::take(&mut self.missing) {
                warnings.clear(Self::WARNING);
                tracing::info!("client audio ping arrived; audio resumed");
            }
        } else if !self.missing && timed_out() {
            self.missing = true;
            warnings.set(Self::WARNING, "No audio ping from the client; video is running without sound until it arrives. Check the network between this PC and the client if sound does not return.");
        }
    }
}

/// Where the audio thread is in its stream, in 48 kHz samples. It counts what the thread encoded
/// and sent, or would have sent, and the audio that never became a packet (Windows dropped it,
/// this host trimmed it, or the thread was too late to make silence for it), so a receiver
/// placing packets by their position hears a gap exactly where audio was lost and nowhere else.
/// It never reads a clock: the sound card's drifts against wall time, and a stall that lost
/// nothing is no gap.
#[derive(Default)]
pub(super) struct AudioClock {
    position: u64,
    /// The idle endpoint's silence has started: its first packet already counted the wait.
    silent: bool,
}

impl AudioClock {
    const RATE: u128 = 48_000;

    /// The start of the next packet of `frames` samples; the clock moves past it, sent or not.
    pub(super) fn packet(&mut self, frames: usize) -> u64 {
        let at = self.position;
        self.position += frames as u64;
        at
    }

    /// `span` as 48 kHz samples. Whole nanoseconds, so 90 ms is 4320 samples and not 4319.99.
    fn samples(span: Duration) -> u64 {
        (span.as_nanos() * Self::RATE / 1_000_000_000) as u64
    }

    /// `lost` of audio that never became a packet, before the packets about to be read. Returns
    /// it in samples.
    pub(super) fn lost(&mut self, lost: Duration) -> u64 {
        let samples = Self::samples(lost);
        self.position += samples;
        samples
    }

    /// The same, counted in 48 kHz samples.
    pub(super) fn lost_samples(&mut self, samples: u64) {
        self.position += samples;
    }

    /// Audio arrived: the endpoint is no longer idle.
    pub(super) fn heard(&mut self) {
        self.silent = false;
    }

    /// The first silence packet of an idle endpoint is due `waited` after the last audio. The
    /// time before it carried no packets, and is part of the stream. Returns whether this was
    /// that first packet.
    /// `counted` samples of that wait were already added as lost, and are not added twice.
    pub(super) fn idle(&mut self, waited: Duration, counted: u64) -> bool {
        let first = !std::mem::replace(&mut self.silent, true);
        if first {
            self.position += Self::samples(waited).saturating_sub(counted);
        }
        first
    }
}

/// How long an endpoint must be quiet before silence stands in for it. An endpoint capture keeps
/// delivering through silence (its keep-alive render stream); this covers game-only capture and a
/// capture that is gone.
const IDLE: Duration = Duration::from_millis(50);

/// What the audio thread decides each pass, apart from the sound card and the socket: when the
/// next packet is due, when silence replaces a quiet endpoint, and where each packet sits in the
/// stream. Its numbers go to the sink before anything is sent, so a packet the sink drops or the
/// peer never receives still holds its place.
pub(super) struct AudioPace {
    clock: AudioClock,
    interval: Duration,
    frames: usize,
    /// When audio last arrived.
    heard: Instant,
    /// When an idle endpoint's next silence is due.
    next: Instant,
    /// Samples counted as lost since audio last arrived. An idle endpoint's first silence
    /// counts the wait since then, and must not count these twice.
    counted: u64,
    /// When the capture went away, until audio or silence comes after it. The time without a
    /// capture is lost even when it is shorter than `IDLE`.
    gap: Option<Instant>,
}

impl AudioPace {
    pub(super) fn new(now: Instant, interval: Duration, frames: usize) -> Self {
        Self {
            clock: AudioClock::default(),
            interval,
            frames,
            heard: now,
            next: now,
            counted: 0,
            gap: None,
        }
    }

    /// How long the endpoint may stay quiet before silence stands in for it:
    /// at least [`IDLE`], and scaled past two intervals plus jitter for 32 ms
    /// AC3/E-AC3 frames.
    fn idle(&self) -> Duration {
        IDLE.max(self.interval.saturating_mul(2) + Duration::from_millis(10))
    }

    /// When the thread has something to do: the packet deadline, but not before the endpoint has
    /// been quiet for long enough to need silence.
    pub(super) fn due(&self) -> Instant {
        self.next.max(self.heard + self.idle())
    }

    /// Windows reported `skipped` of audio it had no room for. `lost` says whether the read
    /// that found it was late enough to explain it; a skip nothing explains is the endpoint
    /// falling quiet and starting again, and is no gap.
    pub(super) fn windows_dropped(&mut self, lost: bool, skipped: Duration) {
        if lost {
            self.counted += self.clock.lost(skipped);
        }
    }

    /// This host discarded `samples` (48 kHz) of audio Windows had delivered.
    pub(super) fn host_dropped(&mut self, samples: u64) {
        self.clock.lost_samples(samples);
        self.counted += samples;
    }

    /// The capture went away at `now` (a reopen, a failed read). Until audio or silence follows,
    /// the time passing is lost.
    pub(super) fn capture_closed(&mut self, now: Instant) {
        self.gap.get_or_insert(now);
    }

    /// A capture delivered audio at `now`: the time since the last one went away was lost.
    pub(super) fn capture_delivered(&mut self, now: Instant) {
        if let Some(at) = self.gap.take() {
            self.clock.lost(now.saturating_duration_since(at));
        }
    }

    /// [`Self::stamp_into`] for packets of their own. Returns each packet's position.
    #[cfg(test)]
    pub(super) fn stamp(
        &mut self,
        now: Instant,
        packets: &mut Vec<Vec<f32>>,
        silence: &[f32],
    ) -> Vec<u64> {
        let mut kept = Packets {
            samples: std::mem::take(packets),
            ..Packets::default()
        };
        self.stamp_into(now, &mut kept, silence);
        *packets = kept.samples;
        kept.positions
    }

    /// Numbers the packets read at `now`, in order, into their positions in the stream. With
    /// none, and the endpoint quiet for 50 ms, one packet of `silence` is made for it.
    pub(super) fn stamp_into(&mut self, now: Instant, packets: &mut Packets, silence: &[f32]) {
        if !packets.samples.is_empty() {
            self.counted = 0;
            self.heard = now;
            self.next = now + self.interval;
            self.clock.heard();
        } else if now >= self.next && now.saturating_duration_since(self.heard) >= self.idle() {
            // WASAPI delivers roughly 10 ms chunks; allow for scheduling jitter
            // before treating a quiet capture queue as an idle endpoint.
            // Silence counts the whole wait since the last audio, which covers a gap.
            self.gap = None;
            let first = self
                .clock
                .idle(now.saturating_duration_since(self.heard), self.counted);
            self.counted = 0;
            let mut room = packets.room();
            room.extend_from_slice(silence);
            packets.samples.push(room);
            self.next += self.interval;
            if self.next < now {
                // One packet covers one interval; the rest of the late time (a blocked device
                // open, a reopen) is gone from the stream. The first packet's wait is already
                // counted by `idle`.
                if !first {
                    self.clock.lost(now - self.next);
                }
                self.next = now + self.interval;
            }
        }
        let count = packets.samples.len();
        packets.positions.clear();
        packets
            .positions
            .extend((0..count).map(|_| self.clock.packet(self.frames)));
    }
}

/// The audio thread's packets of one pass and their positions in the stream, kept from pass
/// to pass with their room: no packet makes a buffer of its own.
#[derive(Default)]
pub(super) struct Packets {
    pub(super) samples: Vec<Vec<f32>>,
    pub(super) positions: Vec<u64>,
    /// The room of packets sent, for the next ones.
    spare: Vec<Vec<f32>>,
}

impl Packets {
    /// Empties it for the next pass, keeping each packet's room.
    fn clear(&mut self) {
        self.spare.append(&mut self.samples);
        self.positions.clear();
    }
    /// Room for one packet, empty.
    fn room(&mut self) -> Vec<f32> {
        let mut room = self.spare.pop().unwrap_or_default();
        room.clear();
        room
    }
}

/// What the audio thread needs from the sound card's capture; `Loopback` is the real one.
pub(super) trait AudioCapture {
    fn since_drained(&mut self) -> Option<Duration>;
    /// One packet of `frames` into `out`, emptied first: whether a whole one was there.
    fn read_into(&mut self, frames: usize, out: &mut Vec<f32>) -> Result<bool>;
    fn take_delivered(&mut self) -> Option<Duration>;
    fn buffer(&self) -> Duration;
    /// Audio this host discarded after Windows delivered it, in 48 kHz samples.
    fn take_trimmed(&mut self) -> u64;
    /// The same, plus whatever was still queued: what a capture that is being dropped loses.
    fn take_discarded(&mut self) -> u64;
}

impl AudioCapture for Loopback {
    fn since_drained(&mut self) -> Option<Duration> {
        Loopback::since_drained(self)
    }
    fn read_into(&mut self, frames: usize, out: &mut Vec<f32>) -> Result<bool> {
        Loopback::read_into(self, frames, out)
    }
    fn take_delivered(&mut self) -> Option<Duration> {
        Loopback::take_delivered(self)
    }
    fn buffer(&self) -> Duration {
        Loopback::buffer(self)
    }
    fn take_trimmed(&mut self) -> u64 {
        Loopback::take_trimmed(self)
    }
    fn take_discarded(&mut self) -> u64 {
        Loopback::take_discarded(self)
    }
}

/// A capture is going away (a failed read, a sink change, a game-capture swap): its Windows
/// skip, the audio this host trimmed and whatever it still queued are lost, and so is the time
/// until the next capture delivers. `waited` is its `since_drained`.
pub(super) fn retire<C: AudioCapture>(
    old: &mut C,
    waited: Option<Duration>,
    loss: &mut HostLoss,
    pace: &mut AudioPace,
    now: Instant,
) {
    let queued = retire_read(old, waited, loss, pace, now);
    pace.host_dropped(queued);
}

/// The part of [`retire`] that precedes whatever the capture handed over: its Windows skip and
/// the audio this host trimmed. Returns the audio it still queued, which follows what was read.
fn retire_read<C: AudioCapture>(
    old: &mut C,
    waited: Option<Duration>,
    loss: &mut HostLoss,
    pace: &mut AudioPace,
    now: Instant,
) -> u64 {
    let delivered = old.take_delivered();
    if let Some(waited) = waited {
        let lost = loss.read(waited, delivered, old.buffer());
        pace.windows_dropped(lost, delivered.unwrap_or_default());
    }
    pace.host_dropped(old.take_trimmed());
    pace.capture_closed(now);
    old.take_discarded()
}

/// The least time between opening the capture and opening it again, so an endpoint that fails
/// right after it opens is not reopened on every pass.
pub(super) const REOPEN_SPACING: Duration = Duration::from_secs(1);

/// When the audio thread next opens a capture a read error just closed, given the `check` it
/// had planned and when the capture `opened`: at once, unless it opened less than
/// [`REOPEN_SPACING`] ago.
pub(super) fn reopen_check(check: Instant, opened: Instant) -> Instant {
    check.min(opened + REOPEN_SPACING)
}

/// How often the audio thread reads a capture that sends no events: WASAPI
/// shared mode delivers in periods, and the read takes whatever is queued, so
/// no packet waits longer for its read than this.
pub(super) const READ_POLL: Duration = Duration::from_millis(5);

/// How long the audio thread waits before its next pass: until the earliest of
/// the next packet deadline `due` and the next housekeeping deadline `check`,
/// never longer than `poll` (how often a capture without events is read) and
/// never less than 1 ms, so an expired deadline cannot spin the loop. Stop
/// stays prompt: every pass reads the stop flag, and no packet deadline sits
/// further out than the 50 ms silence threshold, so the wait never does either.
pub(super) fn audio_wait(
    now: Instant,
    due: Instant,
    check: Option<Instant>,
    poll: Option<Duration>,
) -> Duration {
    let due = check.map_or(due, |check| due.min(check));
    let wait = due.saturating_duration_since(now);
    poll.map_or(wait, |poll| wait.min(poll))
        .max(Duration::from_millis(1))
}

/// [`collect_into`] for packets of their own.
#[cfg(test)]
pub(super) fn collect<C: AudioCapture>(
    capture: &mut Option<C>,
    frames: usize,
    loss: &mut HostLoss,
    pace: &mut AudioPace,
    silence: &[f32],
    now: impl Fn() -> Instant,
    interrupted: impl FnMut(&anyhow::Error),
) -> (Vec<Vec<f32>>, Vec<u64>) {
    let mut packets = Packets::default();
    collect_into(
        capture,
        frames,
        loss,
        pace,
        silence,
        now,
        interrupted,
        &mut packets,
    );
    (packets.samples, packets.positions)
}

/// One pass of the audio thread's read side, into `packets`: every whole packet the capture
/// has, with where each sits in the stream, or one packet of `silence` for a quiet endpoint.
/// What Windows skipped, what this host trimmed and a capture that fails are all counted
/// before the positions are made. `interrupted` hears each read error; the capture is then
/// gone.
#[allow(clippy::too_many_arguments)]
pub(super) fn collect_into<C: AudioCapture>(
    capture: &mut Option<C>,
    frames: usize,
    loss: &mut HostLoss,
    pace: &mut AudioPace,
    silence: &[f32],
    now: impl Fn() -> Instant,
    mut interrupted: impl FnMut(&anyhow::Error),
    packets: &mut Packets,
) {
    packets.clear();
    // What a failed capture still queued: it comes after the packets it handed over first.
    let mut queued = 0;
    let waited = capture.as_mut().and_then(AudioCapture::since_drained);
    while let Some(capturing) = capture.as_mut() {
        let mut room = packets.room();
        match capturing.read_into(frames, &mut room) {
            Ok(true) => packets.samples.push(room),
            Ok(false) => {
                packets.spare.push(room);
                break;
            }
            Err(error) => {
                packets.spare.push(room);
                interrupted(&error);
                queued += retire_read(capturing, waited, loss, pace, now());
                *capture = None;
            }
        }
    }
    if let Some(capturing) = capture.as_mut() {
        let delivered = capturing.take_delivered();
        if let Some(waited) = waited {
            let lost = loss.read(waited, delivered, capturing.buffer());
            // A skip no late read explains is the endpoint falling quiet and starting again.
            let skipped = delivered.unwrap_or_default();
            // Windows dropped this audio: the RTP clock keeps its length as a gap for the
            // receiver to conceal.
            pace.windows_dropped(lost, skipped);
            if waited > capturing.buffer() || !skipped.is_zero() {
                tracing::debug!(
                    waited_ms = waited.as_secs_f64() * 1000.,
                    skipped_ms = skipped.as_secs_f64() * 1000.,
                    delivered = delivered.is_some(),
                    lost,
                    "audio sender read late or Windows skipped audio"
                );
            }
        }
        // Audio the host itself trimmed after a late wake was delivered, so the skip count
        // above does not hold it; the stream still lost its time.
        pace.host_dropped(capturing.take_trimmed());
    }
    // Packets read before a failure belong to the capture that is gone: they do not end its gap.
    let at = now();
    if capture.is_some() && !packets.samples.is_empty() {
        pace.capture_delivered(at);
    }
    pace.stamp_into(at, packets, silence);
    pace.host_dropped(queued);
}

/// RTP with FEC on the client's audio port.
pub(super) struct GameStreamAudio<'a> {
    socket: &'a Arc<ReplySocket>,
    peers: &'a Mutex<super::Peers>,
    launch: &'a str,
    packetizer: AudioPacketizer,
    qos: Tagged,
    use_qos: bool,
    peer: Option<SocketAddr>,
    /// The last call's packets, handed back to the packetizer next call: no
    /// packet of a stream allocates its own buffer.
    packets: Vec<Vec<u8>>,
    pass: butterpollo_windows::net::DatagramPass,
}

impl<'a> GameStreamAudio<'a> {
    pub(super) fn new(
        socket: &'a Arc<ReplySocket>,
        peers: &'a Mutex<super::Peers>,
        launch: &'a str,
        packetizer: AudioPacketizer,
        use_qos: bool,
    ) -> Self {
        Self {
            socket,
            peers,
            launch,
            packetizer,
            qos: Tagged::default(),
            use_qos,
            peer: None,
            packets: Vec::new(),
            pass: butterpollo_windows::net::DatagramPass::new(
                butterpollo_windows::net::AUDIO_ENOBUFS_WAIT,
            ),
        }
    }
}

impl AudioSink for GameStreamAudio<'_> {
    fn ready(&mut self) -> bool {
        self.peer = self.peers.lock().unwrap().get(self.launch, true);
        if let (Some(peer), true) = (self.peer, self.use_qos) {
            self.qos.follow(self.socket, peer, true);
        }
        self.peer.is_some()
    }

    fn begin_pass(&mut self) {
        self.pass = butterpollo_windows::net::DatagramPass::new(
            butterpollo_windows::net::AUDIO_ENOBUFS_WAIT,
        );
    }

    fn send(&mut self, opus: &[u8], _captured: Instant, _position: u64) -> Result<usize> {
        let Some(peer) = self.peer else {
            return Ok(1);
        };
        let mut unsent = 0;
        self.packetizer.encode_into(opus, &mut self.packets)?;
        // A lost audio packet is concealed by the client; only a broken socket stops the audio.
        for packet in &self.packets {
            if !butterpollo_windows::net::send_datagram_pass(
                self.socket,
                packet,
                peer,
                &mut self.pass,
            )? {
                unsent += 1;
            }
        }
        Ok(unsent)
    }

    fn send_frame(
        &mut self,
        parts: &[Vec<u8>],
        _captured: Instant,
        _position: u64,
    ) -> Result<usize> {
        let Some(peer) = self.peer else {
            return Ok(parts.len());
        };
        let mut unsent = 0;
        for (index, part) in parts.iter().enumerate() {
            self.packetizer
                .encode_part_into(part, index + 1 == parts.len(), &mut self.packets)?;
            for packet in &self.packets {
                if !butterpollo_windows::net::send_datagram_pass(
                    self.socket,
                    packet,
                    peer,
                    &mut self.pass,
                )? {
                    unsent += 1;
                }
            }
        }
        Ok(unsent)
    }
}

/// The WebRTC Opus track. The media thread sends nothing until the peer connects, and a full
/// queue drops the packet, which Opus conceals; neither is an error here.
pub(super) struct WebRtcAudio(pub(super) AudioSender);

impl AudioSink for WebRtcAudio {
    fn ready(&mut self) -> bool {
        true
    }

    fn send(&mut self, opus: &[u8], captured: Instant, position: u64) -> Result<usize> {
        Ok(
            match self.0.submit(AudioPacket {
                data: opus.to_vec(),
                capture: captured,
                position,
            }) {
                Submit::Queued => 0,
                Submit::Dropped => 1,
            },
        )
    }
}

#[cfg(test)]
mod tests;
