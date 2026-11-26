//! Sans-IO str0m [`Engine`], RTP timestamping, DataChannel lifecycle and AV1 OBU stripping.

use super::{DISCONNECT_TIMEOUT, Datagram, IdrReason, Outgoing, TransportEvent, VideoFrame};
use crate::sdp::{
    AUDIO_PT, CONTROL_LABEL, GAMEPAD_LABEL, INPUT_LABEL, OfferSpec, SessionIds, VIDEO_PT,
    VideoCodec,
};
use anyhow::{Context, Result, anyhow, bail};
use butterpollo_media_policy::network_pacing;
use butterpollo_stream::webrtc_bridge::AudioPacket;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use str0m::IceConnectionState;
use str0m::channel::{ChannelId, Reliability};
use str0m::media::{Frequency, MediaTime, Pt};
use str0m::net::{Protocol, Receive};
use str0m::{Event, Input, Output, Rtc};

/// RTP, not RTCP, STUN or DTLS (RFC 7983 demultiplexing, rtcp-mux).
pub(super) fn is_media_rtp(data: &[u8]) -> bool {
    data.len() > 12 && (128..192).contains(&data[0]) && !(192..224).contains(&data[1])
}

fn ticks(since: Duration, hz: u64) -> u64 {
    (since.as_nanos() * u128::from(hz) / 1_000_000_000) as u64
}

fn read_leb128(data: &[u8]) -> Option<(usize, usize)> {
    let mut value = 0usize;
    for (i, byte) in data.iter().take(8).enumerate() {
        value |= usize::from(byte & 0x7f) << (7 * i);
        if byte & 0x80 == 0 {
            return Some((value, i + 1));
        }
    }
    None
}

/// Drop the temporal-delimiter OBUs: RFC 9628 says not to send them, and the receiver adds its
/// own. Other OBUs pass byte for byte, moved down in the frame's own buffer.
pub fn strip_av1_temporal_delimiters(mut data: Vec<u8>) -> Result<Vec<u8>> {
    const OBU_TEMPORAL_DELIMITER: u8 = 2;
    // Where the next kept OBU goes.
    let mut kept = 0;
    let mut at = 0;
    while at < data.len() {
        let header = data[at];
        if header & 0x80 != 0 {
            bail!("AV1 OBU with the forbidden bit set at byte {at}");
        }
        let obu_type = (header >> 3) & 0x0f;
        let header_len = 1 + usize::from(header & 0x04 != 0);
        let (payload, size_len) = if header & 0x02 != 0 {
            let size = data.get(at + header_len..).and_then(read_leb128);
            size.ok_or_else(|| anyhow!("AV1 OBU size is cut off at byte {at}"))?
        } else {
            (data.len().saturating_sub(at + header_len), 0)
        };
        let end = at + header_len + size_len + payload;
        if end > data.len() {
            bail!(
                "AV1 OBU at byte {at} runs {} bytes past the access unit",
                end - data.len()
            );
        }
        if obu_type != OBU_TEMPORAL_DELIMITER {
            if kept != at {
                data.copy_within(at..end, kept);
            }
            kept += end - at;
        }
        at = end;
    }
    data.truncate(kept);
    Ok(data)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Control,
    Gamepad,
    Input,
}

/// One session's Rtc with no I/O of its own: datagrams and time go in, datagrams and events come
/// out. Tests drive it with a virtual clock; [`super::MediaHandle`] drives it from a socket.
pub struct Engine {
    rtc: Rtc,
    ids: SessionIds,
    codec: VideoCodec,
    /// The advertised host candidate, which incoming datagrams are addressed to.
    pub(super) local: SocketAddr,
    start: Instant,
    authenticated: Arc<AtomicBool>,
    input: Option<ChannelId>,
    control_open: bool,
    gamepad_open: bool,
    /// A refused channel was logged loudly once; a peer opening more must not flood the log.
    refused_channel: bool,
    outgoing: VecDeque<Outgoing>,
    events: Vec<TransportEvent>,
    idr: Vec<IdrReason>,
    pub(super) connected: bool,
    closed: bool,
    /// Since when the ICE path has been down, while it is.
    pub(super) disconnected_since: Option<Instant>,
    /// RTP timestamp and accumulated wire bytes of the video frame currently being emitted in `poll`.
    emitting_rtp_ts: Option<u32>,
    emitting_frame_bytes: usize,
    /// Wire bytes of the biggest video frame emitted by `advance()` since
    /// [`Engine::take_frame_wire_bytes`].
    frame_wire_bytes: usize,
    /// Datagrams that reached the socket, whoever sent them.
    pub(super) received: u64,
    rejected: u64,
    failed: u64,
    /// RTP time of audio position 0, 48 kHz ticks since `start`: set from the capture instant of
    /// the first packet written, then never again, so wall time cannot move the audio timeline.
    audio_base: Option<u64>,
}

impl Engine {
    /// `start` is the RTP timeline's zero: capture instants map to 90 kHz (video) and 48 kHz
    /// (audio) ticks since it.
    pub fn new(
        rtc: Rtc,
        ids: SessionIds,
        spec: &OfferSpec,
        local: SocketAddr,
        start: Instant,
        authenticated: Arc<AtomicBool>,
    ) -> Self {
        Self {
            rtc,
            ids,
            codec: spec.video.codec,
            local,
            start,
            authenticated,
            input: None,
            control_open: false,
            gamepad_open: false,
            refused_channel: false,
            outgoing: VecDeque::new(),
            events: Vec::new(),
            idr: Vec::new(),
            connected: false,
            closed: false,
            disconnected_since: None,
            emitting_rtp_ts: None,
            emitting_frame_bytes: 0,
            frame_wire_bytes: 0,
            received: 0,
            rejected: 0,
            failed: 0,
            audio_base: None,
        }
    }

    /// Media can flow: connected, and the ICE path is up.
    pub fn is_connected(&self) -> bool {
        self.connected && self.disconnected_since.is_none() && self.rtc.is_alive()
    }

    /// The ICE path is down and the session is waiting for it ([`DISCONNECT_TIMEOUT`]).
    pub fn is_disconnected(&self) -> bool {
        self.disconnected_since.is_some()
    }

    pub fn is_closed(&self) -> bool {
        self.closed || !self.rtc.is_alive()
    }

    /// Wire bytes of the biggest frame written since the last call, for the pacer.
    pub fn take_frame_wire_bytes(&mut self) -> usize {
        std::mem::take(&mut self.frame_wire_bytes)
    }

    /// Datagrams dropped as not the peer's, and ones str0m refused, since the start.
    pub fn dropped_datagrams(&self) -> (u64, u64) {
        (self.rejected, self.failed)
    }

    /// The datagrams str0m wrote since the last call, leaving the queue's allocation for the next.
    pub fn drain_outgoing(&mut self) -> std::collections::vec_deque::Drain<'_, Outgoing> {
        self.outgoing.drain(..)
    }

    #[cfg(test)]
    pub fn take_outgoing(&mut self) -> Vec<Outgoing> {
        self.drain_outgoing().collect()
    }

    pub fn take_events(&mut self) -> Vec<TransportEvent> {
        std::mem::take(&mut self.events)
    }

    pub fn take_idr_requests(&mut self) -> Vec<IdrReason> {
        std::mem::take(&mut self.idr)
    }

    /// A datagram that arrived on the socket. The port is public, so anyone can send to it:
    /// str0m is only asked to handle what it says is for this session (`Rtc::accepts`), and
    /// whatever it still refuses is dropped, counted and logged, never fatal.
    pub fn handle_datagram(&mut self, now: Instant, from: SocketAddr, data: &[u8]) -> Datagram {
        self.received = self.received.saturating_add(1);
        let receive = match Receive::new(Protocol::Udp, from, self.local, data) {
            Ok(receive) => receive,
            Err(e) => {
                self.note_rejected(from, &format!("unparsable: {e}"));
                return Datagram::Rejected;
            }
        };
        let input = Input::Receive(now, receive);
        if !self.rtc.accepts(&input) {
            self.note_rejected(from, "not from the peer");
            return Datagram::Rejected;
        }
        match self.rtc.handle_input(input) {
            Ok(()) => Datagram::Handled,
            Err(e) => {
                if self.failed.is_multiple_of(100) {
                    tracing::warn!(
                        "webrtc: dropped a datagram from {from} ({} so far): {e}",
                        self.failed + 1
                    );
                }
                self.failed += 1;
                Datagram::Failed
            }
        }
    }

    fn note_rejected(&mut self, from: SocketAddr, why: &str) {
        if self.rejected.is_multiple_of(100) {
            tracing::debug!(
                "webrtc: ignored a datagram from {from} ({} so far): {why}",
                self.rejected + 1
            );
        }
        self.rejected += 1;
    }

    /// Advance the clock and collect everything str0m has ready. Returns when it wants to be
    /// called again.
    pub fn advance(&mut self, now: Instant) -> Result<Instant> {
        self.rtc
            .handle_input(Input::Timeout(now))
            .map_err(|e| anyhow!("webrtc transport failed: {e}"))?;
        let wake = self.poll(now)?;
        let Some(since) = self.disconnected_since else {
            return Ok(wake);
        };
        let deadline = since + DISCONNECT_TIMEOUT;
        if now < deadline {
            return Ok(wake.min(deadline));
        }
        if !self.closed {
            tracing::warn!(
                "webrtc: the connection did not come back within {:?}, closing",
                DISCONNECT_TIMEOUT
            );
            self.closed = true;
        }
        Ok(wake)
    }

    fn poll(&mut self, now: Instant) -> Result<Instant> {
        loop {
            match self
                .rtc
                .poll_output()
                .map_err(|e| anyhow!("webrtc transport failed: {e}"))?
            {
                Output::Timeout(at) => return Ok(at.max(now)),
                Output::Transmit(t) => {
                    let data: Vec<u8> = t.contents.into();
                    // Only video waits for the pacer. The payload type sits in the clear.
                    let paced = is_media_rtp(&data) && data[1] & 0x7f != AUDIO_PT;
                    if paced {
                        let ts = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
                        let wire = data.len()
                            + network_pacing::overhead(t.destination.ip().to_canonical().is_ipv6());
                        if self.emitting_rtp_ts == Some(ts) {
                            self.emitting_frame_bytes += wire;
                        } else {
                            self.emitting_rtp_ts = Some(ts);
                            self.emitting_frame_bytes = wire;
                        }
                        self.frame_wire_bytes =
                            self.frame_wire_bytes.max(self.emitting_frame_bytes);
                        if data[1] & 0x80 != 0 {
                            self.emitting_rtp_ts = None;
                        }
                    }
                    self.outgoing.push_back(Outgoing {
                        destination: t.destination,
                        data,
                        paced,
                    });
                }
                Output::Event(event) => self.on_event(now, event),
            }
        }
    }

    pub(super) fn on_event(&mut self, now: Instant, event: Event) {
        match event {
            Event::Connected => {
                self.connected = true;
                self.events.push(TransportEvent::Connected);
                // Nothing was sent before this point, so the picture starts with a keyframe.
                self.idr.push(IdrReason::Connected);
            }
            Event::IceConnectionStateChange(state) => {
                tracing::debug!("webrtc: ICE {state:?}");
                match state {
                    // Before `Connected` these are only ICE not having found a pair yet. After
                    // it, Disconnected is an outage and Checking is ICE searching for a new pair
                    // (a handover): the path is down until a pair is back.
                    IceConnectionState::Disconnected | IceConnectionState::Checking => {
                        if self.connected && self.disconnected_since.is_none() {
                            tracing::warn!("webrtc: the ICE path is down");
                            self.disconnected_since = Some(now);
                            self.events.push(TransportEvent::Disconnected);
                        }
                    }
                    IceConnectionState::Connected | IceConnectionState::Completed
                        if self.disconnected_since.take().is_some() =>
                    {
                        tracing::info!("webrtc: the ICE path is back");
                        self.events.push(TransportEvent::Reconnected);
                        // Whatever was written meanwhile went nowhere: start again from a
                        // keyframe.
                        self.idr.push(IdrReason::Reconnected);
                    }
                    _ => {}
                }
            }
            Event::KeyframeRequest(request) => {
                self.idr.push(match request.kind {
                    str0m::media::KeyframeRequestKind::Fir => IdrReason::Fir,
                    _ => IdrReason::Pli,
                });
            }
            Event::ChannelOpen(id, label) => self.on_channel_open(id, label),
            Event::ChannelData(data) => self.on_channel_data(data.id, data.data, data.binary),
            Event::ChannelClose(id) => {
                let label = match self.role(id) {
                    Some(Role::Control) => {
                        self.control_open = false;
                        CONTROL_LABEL
                    }
                    Some(Role::Gamepad) => {
                        self.gamepad_open = false;
                        GAMEPAD_LABEL
                    }
                    Some(Role::Input) => {
                        self.input = None;
                        INPUT_LABEL
                    }
                    // A channel the host refused, or closed itself.
                    None => return,
                };
                tracing::warn!("webrtc: the {label} channel closed");
                self.events.push(TransportEvent::ChannelClosed {
                    label: label.into(),
                });
            }
            Event::Closed => self.closed = true,
            _ => {}
        }
    }

    /// The `input` channel carries keystrokes and clicks: in order, and never dropped.
    fn ordered_and_reliable(&mut self, id: ChannelId) -> bool {
        self.rtc
            .channel(id)
            .and_then(|channel| {
                channel
                    .config()
                    .map(|c| c.ordered && matches!(c.reliability, Reliability::Reliable))
            })
            .unwrap_or(false)
    }

    fn on_channel_open(&mut self, id: ChannelId, label: String) {
        let input_ok = label == INPUT_LABEL
            && self.input.is_none()
            && self.authenticated.load(Ordering::Acquire);
        if input_ok && !self.ordered_and_reliable(id) {
            tracing::warn!("webrtc: the client's input channel is not ordered and reliable");
            self.rtc.direct_api().close_data_channel(id);
            self.events.push(TransportEvent::ChannelRejected(label));
            return;
        }
        match label.as_str() {
            CONTROL_LABEL if id == self.ids.control => {
                self.control_open = true;
                self.events.push(TransportEvent::ChannelOpen(label));
            }
            GAMEPAD_LABEL if id == self.ids.gamepad => {
                self.gamepad_open = true;
                self.events.push(TransportEvent::ChannelOpen(label));
            }
            INPUT_LABEL if input_ok => {
                self.input = Some(id);
                self.events.push(TransportEvent::ChannelOpen(label));
            }
            _ => {
                if std::mem::replace(&mut self.refused_channel, true) {
                    tracing::debug!("webrtc: refusing the client's {label:?} channel");
                } else {
                    tracing::warn!("webrtc: refusing the client's {label:?} channel");
                }
                self.rtc.direct_api().close_data_channel(id);
                self.events.push(TransportEvent::ChannelRejected(label));
            }
        }
    }

    fn role(&self, id: ChannelId) -> Option<Role> {
        if id == self.ids.control && self.control_open {
            Some(Role::Control)
        } else if id == self.ids.gamepad && self.gamepad_open {
            Some(Role::Gamepad)
        } else if self.input == Some(id) {
            Some(Role::Input)
        } else {
            None
        }
    }

    fn on_channel_data(&mut self, id: ChannelId, data: Vec<u8>, binary: bool) {
        match self.role(id) {
            Some(Role::Control) => self.events.push(TransportEvent::Control { data, binary }),
            Some(Role::Gamepad) => self.events.push(TransportEvent::Gamepad { data, binary }),
            Some(Role::Input) => {
                // Authentication can end while the channel is open (the session was revoked).
                if self.authenticated.load(Ordering::Acquire) {
                    self.events.push(TransportEvent::Input { data, binary });
                } else {
                    self.input = None;
                    self.rtc.direct_api().close_data_channel(id);
                    self.events
                        .push(TransportEvent::ChannelRejected(INPUT_LABEL.into()));
                }
            }
            None => tracing::debug!("webrtc: data on an unknown channel dropped"),
        }
    }

    /// Packetize one access unit. `Ok(false)` means the peer isn't connected yet, so nothing was
    /// sent (str0m would drop it); the caller already asked for a keyframe on `Connected`.
    /// The frame's buffer goes to str0m as it is, not copied.
    pub fn write_video(&mut self, now: Instant, frame: VideoFrame) -> Result<bool> {
        if !self.is_connected() {
            return Ok(false);
        }
        // The conversion str0m's writer makes of a `Vec` anyway; a shared frame has it done.
        let data = frame.data.share(self.codec)?;
        let time = MediaTime::new(
            ticks(frame.capture.saturating_duration_since(self.start), 90_000),
            Frequency::NINETY_KHZ,
        );
        // The playout delay asks the receiver to show the frame as soon as it is whole.
        self.rtc
            .writer(self.ids.video_mid)
            .context("the video media is not negotiated")?
            .playout_delay(MediaTime::ZERO, MediaTime::ZERO)
            .write(
                Pt::new_with_value(VIDEO_PT),
                frame.capture.min(now),
                time,
                data,
            )
            .map_err(|e| anyhow!("video write failed: {e}"))?;
        Ok(true)
    }

    /// One Opus packet. Stereo and multistream payloads are not looked at.
    pub fn write_audio(&mut self, now: Instant, packet: AudioPacket) -> Result<bool> {
        if !self.is_connected() {
            return Ok(false);
        }
        // The audio thread reads whole WASAPI chunks and stamps every packet from one with the
        // same instant, and the device clock drifts against wall time. Receivers place packets by
        // RTP time, so the clock is the thread's own sample position: a packet dropped upstream
        // or a stall that lost no audio leaves it contiguous, and audio Windows lost leaves
        // exactly its length as a gap. Wall time only anchors the first packet, so the NTP
        // pairing starts near the video's.
        let base = *self.audio_base.get_or_insert_with(|| {
            ticks(packet.capture.saturating_duration_since(self.start), 48_000)
                .saturating_sub(packet.position)
        });
        let at = base + packet.position;
        let time = MediaTime::new(at, Frequency::FORTY_EIGHT_KHZ);
        self.rtc
            .writer(self.ids.audio_mid)
            .context("the audio media is not negotiated")?
            .write(
                Pt::new_with_value(AUDIO_PT),
                packet.capture.min(now),
                time,
                packet.data,
            )
            .map_err(|e| anyhow!("audio write failed: {e}"))?;
        Ok(true)
    }

    /// `Ok(false)` when the channel isn't open (yet) or its buffer is full.
    pub fn send_control(&mut self, data: &[u8], binary: bool) -> Result<bool> {
        self.send_channel(self.ids.control, self.control_open, data, binary)
    }

    pub fn send_gamepad(&mut self, data: &[u8], binary: bool) -> Result<bool> {
        self.send_channel(self.ids.gamepad, self.gamepad_open, data, binary)
    }

    fn send_channel(
        &mut self,
        id: ChannelId,
        open: bool,
        data: &[u8],
        binary: bool,
    ) -> Result<bool> {
        if !open {
            return Ok(false);
        }
        match self.rtc.channel(id) {
            Some(mut channel) => channel
                .write(binary, data)
                .map_err(|e| anyhow!("channel write failed: {e}")),
            None => Ok(false),
        }
    }

    pub fn disconnect(&mut self) {
        self.rtc.disconnect();
    }
}
