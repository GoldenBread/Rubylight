//! WebRTC sessions on the stream pipeline: `start-session`, `stop-session`, `switch-session` and
//! `stop-host-session` (`docs/webrtc-design.md`).
//!
//! One actor per signalling connection runs the requests in order. It validates a request
//! against the host's capabilities, launches the app and display through the same back half as a
//! Moonlight launch (`nvhttp::launch_webrtc`), offers the str0m session over the socket, and on
//! the client's answer starts the media transport and the stream worker. The worker takes the
//! transport from [`Links`] and sends each encoded access unit through a [`Sender`]; the
//! GameStream video, audio and ENet paths do not run for the session.
//!
//! A session ends when the client sends `stop-session`, the signalling socket closes, the media
//! transport closes, or the stream fails; the actor says `idle` (or `error`) when it notices.

use super::{
    input::InputHandle,
    media::{Hooks, IdrReason, MediaHandle, Submit, TransportEvent, VideoFrame},
};
use anyhow::{Result, ensure};
use butterpollo_media_policy::performance::{Claims, Timing};
use butterpollo_session::webrtc::protocol::{
    AudioSettings, ControlMessage, ServerMessage, VideoSettings, classify_control,
};
use butterpollo_stream::{
    state::{Session, Shared},
    stream::Media,
    webrtc_bridge::{WebRtcAudioSink, WebRtcMediaLink, WebRtcVideoSink},
};
use butterpollo_win_graphics::encoder::{Encoded, Unit};
use std::{
    cell::Cell,
    net::{IpAddr, SocketAddr},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::mpsc::{self, UnboundedSender, channel};

mod actor;
mod live;
#[cfg(test)]
mod tests;

/// The permission to start and end streams, as for a Moonlight launch.
const LAUNCH: u32 = 1 << 26;
/// How long the client has to answer the offer. The launch it holds expires after 30 s.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(15);
/// How long an answered session may take to connect before it is ended.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// How much longer than [`CONNECT_TIMEOUT`] a connected session may wait for the worker to vouch
/// for an HDR source before it is ended: a worker stuck before its first frame must not leave the
/// client in "starting" for good.
const HELD_EXTRA: Duration = Duration::from_secs(10);
/// How long a new session waits for the media port of the one before it.
const BIND_WAIT: Duration = Duration::from_secs(3);
/// How often the actor looks for a session that ended.
const WATCH: Duration = Duration::from_millis(100);
/// Capture instants of consecutive frames differ by at least one 90 kHz tick (11.1 us).
const MIN_STEP: Duration = Duration::from_micros(12);

fn micros(duration: Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}

/// A capture instant no earlier than one RTP tick after the last one: the transport maps the
/// instant to the 90 kHz timestamp and does not repair a frame that steps back or repeats.
fn monotonic(last: &Cell<Option<Instant>>, capture: Instant) -> Instant {
    let capture = match last.get() {
        Some(previous) => capture.max(previous + MIN_STEP),
        None => capture,
    };
    last.set(Some(capture));
    capture
}

/// The stream worker's end of a WebRTC session: hands each encoded access unit to the transport,
/// which packetizes, paces and sends it from its own thread. Dropping it closes the transport.
pub struct Sender {
    media: MediaHandle,
    last: Cell<Option<Instant>>,
}

impl Sender {
    pub fn new(media: MediaHandle) -> Self {
        Self {
            media,
            last: Cell::new(None),
        }
    }

    /// The client's address on the pair ICE selected, once it has answered.
    pub fn peer(&self) -> Option<SocketAddr> {
        self.media.peer()
    }

    /// Video datagrams the loss switch has thrown away so far; None while it is off.
    pub fn simulated_losses(&self) -> Option<u64> {
        self.media.simulated_losses()
    }

    /// Queue frames the encoder returned. A frame the transport cannot take is dropped and a
    /// keyframe requested by the transport; the stream goes on.
    pub fn submit(
        &self,
        session: &Session,
        frames: Vec<Encoded>,
        call_latency: Duration,
        processing_started: Instant,
        claims: &mut Claims,
    ) -> Result<()> {
        if frames.is_empty() {
            return Ok(());
        }
        if !self.media.is_running() {
            // Closing the transport stops the session first; anything else is a failure.
            ensure!(
                session.stopping(),
                "the WebRTC transport stopped without closing"
            );
            return Ok(());
        }
        let polled = Instant::now();
        for mut frame in frames {
            ensure!(
                matches!(frame.unit, Unit::Whole),
                "WebRTC takes whole access units, not slices"
            );
            let latency = frame.latency.unwrap_or(call_latency);
            let claimed = polled.checked_sub(latency).unwrap_or(polled);
            let claim = claims.take(frame.presentation);
            let capture = monotonic(&self.last, frame.presentation.unwrap_or(claimed));
            let bytes = frame.bytes.len() as u64;
            let submitted = self.media.submit_video(VideoFrame {
                // Encoded returns its buffer to the encoders' pool on drop;
                // this one leaves with the frame instead.
                data: std::mem::take(&mut frame.bytes).into(),
                capture,
                keyframe: frame.idr,
            });
            let stats = &session.stats;
            match submitted {
                Submit::Queued => {
                    let sent_at = Instant::now();
                    stats.latency_us.store(micros(latency), Ordering::Relaxed);
                    stats.frames.fetch_add(1, Ordering::Relaxed);
                    stats.bytes.fetch_add(bytes, Ordering::Relaxed);
                    let age = claim.as_ref().map_or(0, |claim| {
                        micros(claim.claimed.saturating_duration_since(claim.presented))
                    });
                    stats.performance.lock().unwrap().record_timing(
                        sent_at,
                        Timing {
                            period: session.stream_mode().period(),
                            encode: micros(latency),
                            host: micros(polled.saturating_duration_since(processing_started)),
                            age,
                            sent: micros(sent_at.saturating_duration_since(claimed)),
                        },
                        bytes,
                    );
                }
                Submit::Dropped => {
                    stats.frames_replaced.fetch_add(1, Ordering::Relaxed);
                    tracing::debug!(bytes, keyframe = frame.idr, "WebRTC video frame dropped");
                }
            }
        }
        Ok(())
    }
}

impl WebRtcVideoSink for Sender {
    fn peer(&self) -> Option<SocketAddr> {
        Sender::peer(self)
    }

    fn simulated_losses(&self) -> Option<u64> {
        Sender::simulated_losses(self)
    }

    fn submit(
        &self,
        session: &Session,
        frames: Vec<Encoded>,
        call_latency: Duration,
        processing_started: Instant,
        claims: &mut Claims,
    ) -> Result<()> {
        Sender::submit(
            self,
            session,
            frames,
            call_latency,
            processing_started,
            claims,
        )
    }
}

impl WebRtcMediaLink for MediaHandle {
    fn audio_sender(&self) -> Option<Box<dyn WebRtcAudioSink>> {
        self.audio_sender()
            .map(|sender| Box::new(sender) as Box<dyn WebRtcAudioSink>)
    }

    fn into_video_sink(self: Box<Self>) -> Box<dyn WebRtcVideoSink> {
        Box::new(Sender::new(*self))
    }
}

/// What the actor learns from the transport.
enum Event {
    Connected,
}

/// The transport's hooks run on its threads and know the session only through this: the session
/// is set once it exists, which is after the transport started.
struct Bridge {
    session: Mutex<Option<Weak<Session>>>,
    /// The transport closed before the session was bound.
    closed: AtomicBool,
    /// The transport connected, which may be before the session was bound.
    connected: AtomicBool,
    events: UnboundedSender<Event>,
    /// The session's input worker, attached before the transport starts so no frame the client
    /// sends early is lost.
    input: Mutex<Option<InputHandle>>,
}

impl Bridge {
    fn new(events: UnboundedSender<Event>) -> Arc<Self> {
        Arc::new(Self {
            session: Mutex::new(None),
            closed: AtomicBool::new(false),
            connected: AtomicBool::new(false),
            events,
            input: Mutex::new(None),
        })
    }

    fn attach_input(&self, input: InputHandle) {
        *self.input.lock().unwrap() = Some(input);
    }

    fn input(&self) -> Option<InputHandle> {
        self.input.lock().unwrap().clone()
    }

    fn bind(&self, session: &Arc<Session>) {
        *self.session.lock().unwrap() = Some(Arc::downgrade(session));
        if let Some(input) = self.input() {
            input.bind(session.clone());
        }
        if self.closed.load(Ordering::SeqCst) {
            session.stop();
        } else if self.connected.load(Ordering::SeqCst) {
            // Its keyframe request had no session to reach.
            session.request_host_keyframe();
        }
    }

    fn session(&self) -> Option<Arc<Session>> {
        self.session.lock().unwrap().as_ref()?.upgrade()
    }

    /// A PLI or FIR is the client's keyframe request, like Moonlight's; the transport's own
    /// reasons are the host's.
    fn idr(&self, reason: IdrReason) {
        let Some(session) = self.session() else {
            return;
        };
        match reason {
            IdrReason::Pli | IdrReason::Fir => session.request_client_keyframe(),
            IdrReason::Connected => session.request_host_keyframe(),
            IdrReason::Reconnected
            | IdrReason::QueueFull
            | IdrReason::PacerBacklog
            | IdrReason::WaitingForKeyframe
            | IdrReason::WriteFailed => session.request_idr(),
        }
    }

    fn event(&self, event: TransportEvent) {
        match event {
            TransportEvent::Connected => {
                self.connected.store(true, Ordering::SeqCst);
                let _ = self.events.send(Event::Connected);
            }
            TransportEvent::Closed => {
                self.closed.store(true, Ordering::SeqCst);
                tracing::info!("WebRTC transport closed; ending the session");
                if let Some(session) = self.session() {
                    session.stop();
                }
                // Releases what the client still holds.
                if let Some(input) = self.input() {
                    input.closed();
                }
            }
            TransportEvent::Disconnected => {
                tracing::warn!("WebRTC path down; the session ends if it does not come back")
            }
            TransportEvent::Reconnected => tracing::info!("WebRTC path restored"),
            TransportEvent::Control {
                data,
                binary: false,
            } => {
                match std::str::from_utf8(&data).map(classify_control) {
                    Ok(ControlMessage::KeyframeRequest) => {
                        if let Some(session) = self.session() {
                            session.request_client_keyframe();
                        }
                    }
                    // Reference-style clients announce their pads here; the
                    // snapshots that follow on `gamepad` are ignored without it.
                    Ok(ControlMessage::GamepadAnnouncement) => {
                        if let Some(input) = self.input() {
                            input.gamepad(data, false, Instant::now());
                        }
                    }
                    _ => {}
                }
            }
            TransportEvent::Gamepad { data, binary } => {
                if let Some(input) = self.input() {
                    input.gamepad(data, binary, Instant::now());
                }
            }
            TransportEvent::Input { data, binary } => {
                if let Some(input) = self.input() {
                    input.input(data, binary);
                }
            }
            TransportEvent::ChannelClosed { label } => {
                if let Some(input) = self.input() {
                    input.channel_closed(&label);
                }
            }
            TransportEvent::ChannelOpen(_) | TransportEvent::ChannelRejected(_) => {}
            TransportEvent::Control { .. } => {}
        }
    }

    fn hooks(self: &Arc<Self>) -> Hooks {
        let (idr, event) = (self.clone(), self.clone());
        Hooks::new(move |reason| idr.idr(reason), move |e| event.event(e))
    }
}

/// A request to start or switch to an app.
pub struct Request {
    pub app_id: String,
    pub video: VideoSettings,
    pub audio: AudioSettings,
}

pub enum Command {
    Start(Request),
    Switch(Request),
    Stop,
    StopHost,
    Answer { session_id: u64, sdp: String },
}

pub(crate) const COMMAND_CAP: usize = 64;

/// The connection's end of its session actor. Dropping it ends the actor and stops its session.
pub struct Commands(mpsc::Sender<Command>);

impl Commands {
    pub fn send(&self, command: Command) -> bool {
        self.0.try_send(command).is_ok()
    }

    #[cfg(test)]
    pub(crate) fn test_channel(cap: usize) -> (Self, mpsc::Receiver<Command>) {
        let (tx, rx) = channel(cap);
        (Self(tx), rx)
    }
}

/// Run the session actor of one signalling connection. Its replies arrive on `out`. `peer` is the
/// signalling peer and `local` the address it connected to, which the media socket binds. `this_pc`
/// says the browser runs on this PC ([`browser_on_this_pc`]).
pub fn spawn(
    h: Shared,
    media: Arc<Media>,
    client_id: String,
    peer: SocketAddr,
    local: IpAddr,
    this_pc: bool,
    out: mpsc::Sender<ServerMessage>,
) -> Commands {
    let (commands, inbox) = channel(COMMAND_CAP);
    let actor = actor::Actor {
        h,
        media,
        client_id,
        peer,
        local,
        this_pc,
        inbox,
        out,
        live: None,
    };
    tokio::spawn(actor.run());
    Commands(commands)
}
