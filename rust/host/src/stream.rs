use crate::state::{Launch, Session, Shared};
mod app_window;
pub(crate) mod audio;
pub(crate) mod audio_encoder;
mod audio_haptics;
mod audio_sink;
mod background;
mod capture;
pub(crate) mod clipboard_sync;
mod config;
mod control;
mod couch_audio;
mod device_recovery;
#[cfg(test)]
mod encoder_tests;
pub(crate) mod frame_warp;
mod game_audio;
mod instance_audio;
mod keyboard_layout;
mod qos;
mod roaming;
pub(crate) mod slices;
mod video;
use audio_sink::{AudioPace, AudioSink, GameStreamAudio, WebRtcAudio};
use butterpollo_media_policy::audio_passthrough::Codec;
#[cfg(test)]
use config::{apply_overrides, sets_peak};
pub(crate) use config::{
    cap_stream_resolution, device_config, effective_config, launch_app, launched_through_steam,
    report_stream_resolution_cap,
};
#[cfg(test)]
use control::{
    CONTROL_EVENT_LIMIT, ControlSocket, FEEDBACK_CHANNEL, INPUT_OVERFLOW, INPUT_QUEUE,
    MOTION_EVENT_REQUEST, PadSends, PeerConfig, RUMBLE_MARKER, RumbleForwarded,
    SWALLOWED_PER_RECEIVE, ScaleLookup, UNATTENDED_SWEEP, UnattendedSweep, control_wait_timeout,
    drop_uninjectable_input, end_unattended_sessions, expects_enet_control, expire_controllers,
    first_steam_attributes_refusal, haptics_negotiated, hold_pad_input, motion_requests,
    pad_reports, queue_input, server_command,
};
pub(crate) use control::{
    PadsWait, RUMBLE_DATA, RUMBLE_TRIGGER_DATA, attach_pads, controllers_back, controllers_wait,
    feedback_packets,
};
pub(crate) use instance_audio::{InstanceRoutes, instance_routes, plays_on_pc, put_back};
#[cfg(test)]
use video::{
    COMPUTE_FAILURE_WINDOW, COMPUTE_RETRY_QUIET, ComputeDemotion, ENCODER_RECOVERY, EncoderOutput,
    EncoderRecovery, OUTPUT_POLL, QueryTrace, encoder_progress, next_output_poll,
    observe_foreground_game, poll_fits, wait_for_output,
};
#[cfg(any(test, feature = "test-harness"))]
pub mod reconnect_harness;
#[cfg(test)]
pub(crate) mod reconnect_tests;
use anyhow::{Context, Result, bail};
use butterpollo_config::config::Config;
use butterpollo_crypto::crypto;
use butterpollo_input_policy::{
    controller_audio, haptics,
    input::{self, required_permission},
    text_input::Watch as TextWatch,
};
use butterpollo_media_policy::{
    encoder_backlog::{self, Backpressure},
    packet::{AudioPacketizer, VideoPacketizer},
};
use butterpollo_session::session::Role;
use butterpollo_windows::device_loss::DeviceLost;
/// A WebRTC HDR stream carries the display's HDR or nothing: SDR content labelled HDR, or HDR
/// tone-mapped to SDR, is not what the client asked for, so the session ends with an error.
/// `display_in_hdr` is the capture's reading of the output's colour space (HDR10 or not); the
/// frames' pixel format cannot stand in for it, since FP16 frames come from SDR outputs too.
/// GameStream keeps its own behaviour for this case. `running` tells a display that left HDR
/// from one that never was in it.
pub(crate) fn require_hdr_source(
    s: &Session,
    display_in_hdr: impl FnOnce() -> Option<bool>,
    running: bool,
) -> Result<()> {
    if s.launch.transport != butterpollo_session::session::Transport::WebRtc || !s.config.hdr {
        return Ok(());
    }
    match display_in_hdr() {
        Some(true) => Ok(()),
        Some(false) if running => bail!(
            "the display left HDR during the stream; the HDR stream ended instead of showing SDR as HDR. Keep Windows HDR on for this display, or start an SDR stream"
        ),
        Some(false) => bail!(
            "HDR was requested but the display is not in HDR; turn on HDR for this display in Windows settings (or set the HDR policy to auto in the host's display settings), or start an SDR stream"
        ),
        // A reading that fails mid-stream is the capture's to report (it ends on a colour space
        // error itself); only a display seen leaving HDR ends the stream here.
        None if running => Ok(()),
        None => bail!(
            "HDR was requested but Windows did not report the display's colour space, so the stream cannot tell HDR from SDR; start an SDR stream"
        ),
    }
}

/// Whether a non-WebRTC stream configured for HDR must fall back to 10-bit SDR
/// because capture shows the display stayed in SDR (`Some(false)`) with an SDR
/// surface and RTX HDR is not generating HDR from it.
pub(crate) fn should_fallback_hdr_to_sdr(
    transport: butterpollo_session::session::Transport,
    stream_hdr: bool,
    source_is_bgra8: bool,
    display_in_hdr: Option<bool>,
    rtx_hdr_active: bool,
) -> bool {
    transport != butterpollo_session::session::Transport::WebRtc
        && stream_hdr
        && source_is_bgra8
        && display_in_hdr == Some(false)
        && !rtx_hdr_active
}

/// Backoff only after reopening fails; resource release is acknowledged.
const RECOVERY_RETRY: Duration = Duration::from_millis(150);
/// How long a capture of the app's window that became unavailable (minimised, closed) waits
/// for the app window worker, which checks every 250 ms, to lay the display back over it.
const WINDOW_TARGET_WAIT: Duration = Duration::from_millis(400);
/// What the stream card says after an Automatic streamed display moved to Desktop
/// Duplication because Windows Graphics Capture could not see an exclusive-fullscreen
/// app: the cause, what it costs and when it ends.
const DUPLICATION_WARNING: &str = "Windows Graphics Capture sent no picture while an exclusive-fullscreen app was in front, so this display is captured with Desktop Duplication for the rest of this session, which reads the display itself. Desktop Duplication costs what it always costs: HDR and capture timing can differ, and a turned monitor or a display another GPU drives cannot be captured that way. The next stream starts on the Capture method setting again.";
/// How long capture waits for the stream's display to come back before it
/// shows another one. Switching a virtual display back on takes about a
/// second; recreating it takes up to the driver's 10 s arrival deadline.
const DISPLAY_RETURN_WAIT: Duration = Duration::from_secs(15);
/// How often a still picture may ask Windows whether an exclusive-fullscreen
/// app is in front. Capture polls continuously, so the shell's answer runs
/// twice a second rather than on every idle poll.
const FULLSCREEN_PROBE: Duration = Duration::from_millis(500);
/// Which display a capture reopen uses.
#[derive(Clone, Copy, Debug, PartialEq)]
enum ReopenOn {
    /// The stream's display.
    Stream,
    /// Not yet: the stream's display is not on the desktop.
    Wait,
    /// The primary display, after the stream's did not return in time.
    Primary,
}
/// Capture follows the stream's display. Windows can switch a virtual display
/// off (a game resetting the layout on exit); the heartbeat switches it back
/// on or recreates it and publishes it, possibly under a new name. Capture
/// waits for that and never quietly shows another desktop: only after
/// `DISPLAY_RETURN_WAIT` does a foreground stream open the primary display.
/// A background stream must never expose the local user's desktop.
fn reopen_on(
    present: bool,
    background: bool,
    missing_since: &mut Option<Instant>,
    now: Instant,
) -> ReopenOn {
    if present {
        *missing_since = None;
        return ReopenOn::Stream;
    }
    let since = *missing_since.get_or_insert(now);
    if !background && now.duration_since(since) >= DISPLAY_RETURN_WAIT {
        ReopenOn::Primary
    } else {
        ReopenOn::Wait
    }
}
/// The backend a capture reopens on: the stream's own (the configured one),
/// again after an ordinary reopen such as the secure desktop's, or Desktop
/// Duplication for the rest of the session once the fullscreen fallback moved
/// a frozen picture there. A backend that fell back for a while, as an
/// unavailable Rubylight display push does to Windows Graphics Capture, is
/// therefore tried again on the next reopen.
fn reopen_backend(kind: &str, switched_to_duplication: bool) -> &str {
    if switched_to_duplication { "ddx" } else { kind }
}
/// Whether a capture on Desktop Duplication reopens on Windows Graphics
/// Capture now that the desktop is visible again: it fell back because WGC
/// cannot capture a secure desktop (a UAC prompt, the lock screen), not
/// because the fullscreen fallback moved a frozen game picture there, which
/// keeps its source for the session, nor because the stream asked for Desktop
/// Duplication itself.
fn returns_to_wgc(
    available: bool,
    was_available: bool,
    capture_backend: &str,
    kind: &str,
    switched_to_duplication: bool,
) -> bool {
    available
        && !was_available
        && !switched_to_duplication
        && capture_backend == "ddx"
        && kind != "ddx"
}
/// Whether the fullscreen probe may ask the shell now: at most every
/// [`FULLSCREEN_PROBE`], however fast a still picture polls capture. The next
/// time is advanced before the answer is used, so a probe that finds nothing
/// waits as long as one that switches. `next` starts at the stream's first
/// frame, which lets the first probe through at once.
fn fullscreen_probe_due(now: Instant, next: &mut Instant) -> bool {
    if now < *next {
        return false;
    }
    *next = now + FULLSCREEN_PROBE;
    true
}
/// How long a stream waits for its capture to start: past
/// [`SAME_WINDOW_WAIT`] only for the try of a window the game opened after a
/// whole helper start went by on the one it replaced (#59).
const CAPTURE_START_WAIT: Duration = Duration::from_secs(12);
/// What a window the game still shows gets, tries included: one that sends no
/// frame in a whole helper start fails with its own error, before a client
/// gives up on seeing video and for the next player's capture to go ahead.
const SAME_WINDOW_WAIT: Duration = Duration::from_secs(10);
/// What a try of a game window's capture takes beyond its WGC helper's start.
const WINDOW_OPEN_MARGIN: Duration = Duration::from_secs(1);
/// Whether a game window's first capture, whose stream began waiting for it
/// at `started`, is tried again at `now`: not once the stream stops, and only
/// while the wait before the try and the try itself, with a WGC helper's
/// start of up to [`butterpollo_windows::capture::WGC_START_TIMEOUT`], end
/// before the stream stops waiting, which is [`CAPTURE_START_WAIT`] when the
/// game has `replaced` the window just tried and [`SAME_WINDOW_WAIT`] when it
/// has not. A try that ran past it would end the stream with a timeout
/// instead of the capture's own error.
fn window_open_again(started: Instant, stop: &AtomicBool, now: Instant, replaced: bool) -> bool {
    let wait = if replaced {
        CAPTURE_START_WAIT
    } else {
        SAME_WINDOW_WAIT
    };
    !stop.load(Ordering::Acquire)
        && now
            + RECOVERY_RETRY
            + butterpollo_windows::capture::WGC_START_TIMEOUT
            + WINDOW_OPEN_MARGIN
            <= started + wait
}
/// [`window_open_again`] now, after the wait between tries when it is.
fn wait_to_open_again(started: Instant, stop: &AtomicBool, replaced: bool) -> bool {
    if !window_open_again(started, stop, Instant::now(), replaced) {
        return false;
    }
    thread::sleep(RECOVERY_RETRY);
    true
}
/// Opens a stream's first capture, on the target `target` names when each
/// try starts. A game window's is tried again while `again` (told the target
/// that failed, it waits between tries) allows: at its start a game replaces
/// its splash window, or resizes its window as the capture opens, and that
/// open fails while the next one works (#56: three of five Nucleus Co-op
/// streams ended at once). A display's first failure is the stream's.
fn open_first<T>(
    mut target: impl FnMut() -> (String, u64),
    mut open: impl FnMut(&(String, u64)) -> Result<T>,
    mut again: impl FnMut(&(String, u64)) -> bool,
) -> Result<(T, (String, u64))> {
    loop {
        let next = target();
        let error = match open(&next) {
            Ok(capture) => return Ok((capture, next)),
            Err(error) => error,
        };
        let window = butterpollo_windows::capture::WindowTarget::parse(&next.0)
            .is_ok_and(|window| window.is_some());
        if !window || !again(&next) {
            return Err(error);
        }
        tracing::warn!(target = %next.0, error = %format!("{error:#}"), "the game window's capture did not open; trying again");
    }
}

use butterpollo_windows::{
    audio::{Loopback, Opus},
    capture::{Capture, ComGuard, GpuImage, Priority},
    encoder::Encoder,
    input::{Injector, PadReport},
    net::ReplySocket,
};
use rusty_enet::{Event, Host, HostSettings, Packet, PacketKind, PeerID, PeerState};
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr, UdpSocket},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

type Latest = capture::Latest<GpuImage>;
struct Source {
    warnings: Arc<butterpollo_session::session::Warnings>,
    latest: Arc<Latest>,
    /// The capture backend that opened, after every reopen: `auto` is planned
    /// as WGC, so the plan cannot say whether frames come from WGC or the
    /// display driver.
    backend: Arc<Mutex<&'static str>>,
    grid: Arc<Mutex<butterpollo_windows::capture::ClaimGrid>>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
fn finish_worker(worker: thread::JoinHandle<()>, name: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !worker.is_finished() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    if worker.is_finished() {
        let _ = worker.join();
    } else {
        tracing::error!(
            worker = name,
            "teardown did not return within five seconds; detaching the driver worker"
        );
    }
}
impl Drop for Source {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.thread.take() {
            finish_worker(worker, "capture");
        }
    }
}
impl Source {
    fn backend(&self) -> &'static str {
        *self.backend.lock().unwrap()
    }
}
impl std::ops::Deref for Source {
    type Target = Latest;
    fn deref(&self) -> &Latest {
        &self.latest
    }
}
/// Each session's video and audio addresses from its pings, by launch ID. The stream and
/// audio threads look theirs up for every frame and packet, so a lookup copies no key.
#[derive(Default)]
pub struct Peers(HashMap<String, [Option<SocketAddr>; 2]>);
impl Peers {
    #[cfg(any(test, feature = "test-harness"))]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn get(&self, id: &str, audio: bool) -> Option<SocketAddr> {
        self.0.get(id).and_then(|pair| pair[usize::from(audio)])
    }
    /// The address it replaced, if any.
    fn insert(&mut self, id: &str, audio: bool, peer: SocketAddr) -> Option<SocketAddr> {
        if let Some(pair) = self.0.get_mut(id) {
            return pair[usize::from(audio)].replace(peer);
        }
        let mut pair = [None; 2];
        pair[usize::from(audio)] = Some(peer);
        self.0.insert(id.to_owned(), pair);
        None
    }
    /// Forgets a session's addresses, returning each as (audio, address).
    fn remove(&mut self, id: &str) -> impl Iterator<Item = (bool, SocketAddr)> + use<> {
        let pair = self.0.remove(id).unwrap_or_default();
        [false, true]
            .into_iter()
            .zip(pair)
            .filter_map(|(audio, peer)| Some((audio, peer?)))
    }
    /// Whether any session's video or audio comes from `peer`.
    fn contains(&self, peer: SocketAddr) -> bool {
        self.0.values().flatten().any(|known| *known == Some(peer))
    }
}
/// The active session a media ping from `peer` is for, and how often it had roamed
/// then: its own ping payload, or a bare "PING" when only one session plays to that
/// IP and, once it roamed, from the address that session already uses.
fn ping_session(
    sessions: &crate::state::Sessions,
    peers: &Mutex<Peers>,
    ping: &[u8],
    peer: SocketAddr,
    is_audio: bool,
) -> Option<(Arc<Session>, u32)> {
    let ip = peer.ip().to_canonical();
    let from_ip = |s: &&Arc<Session>| s.client_ip() == ip;
    let only_one = sessions.active.values().filter(from_ip).count() == 1;
    let s = sessions.active.values().filter(from_ip).find(|s| {
        if ping.len() >= 16 && crypto::equal(&ping[..16], s.launch.ping.as_bytes()) {
            return true;
        }
        // After a roam only the session's own ping proves a new address: behind a
        // CGNAT anyone may share the client's IP.
        ping == b"PING"
            && only_one
            && (s.roams() == 0 || peers.lock().unwrap().get(&s.launch.id, is_audio) == Some(peer))
    })?;
    Some((s.clone(), s.roams()))
}
/// Whether `s`, found by [`ping_session`] when it had roamed `roams` times, is still
/// active and has not roamed since.
fn still_current(sessions: &crate::state::Sessions, s: &Arc<Session>, roams: u32) -> bool {
    sessions
        .active
        .get(&s.launch.id)
        .is_some_and(|active| Arc::ptr_eq(active, s))
        && s.roams() == roams
}
/// Takes a media ping from `peer`, which reached this host on `local`: its session
/// learns the address, and replies to it leave from `local` (`answer_from`). That can
/// look up a route, so it runs with the sessions lock released, which launches, the
/// control worker and roams take. A teardown or roam in between is decided again
/// under the lock, as if the ping came after it. Returns the session whose video
/// now takes a new path.
fn take_media_ping(
    h: &Shared,
    m: &Media,
    socket: &ReplySocket,
    is_audio: bool,
    ping: &[u8],
    peer: SocketAddr,
    local: Option<IpAddr>,
) -> Option<Arc<Session>> {
    let found = ping_session(&h.sessions.lock().unwrap(), &m.peers, ping, peer, is_audio);
    let (found, roams) = found?;
    // Answer from the address the client reached: Windows may route a reply out of a
    // VPN address.
    socket.answer_from(peer, local);
    let sessions = h.sessions.lock().unwrap();
    let s = if still_current(&sessions, &found, roams) {
        found
    } else if let Some((s, _)) = ping_session(&sessions, &m.peers, ping, peer, is_audio) {
        socket.answer_from(peer, local);
        s
    } else {
        // What the answer pinned no longer belongs to anyone.
        if !m.peers.lock().unwrap().contains(peer) {
            socket.forget(peer);
        }
        return None;
    };
    let previous = {
        let mut peers = m.peers.lock().unwrap();
        peers.insert(&s.launch.id, is_audio, peer)
    };
    if previous == Some(peer) {
        return None;
    }
    if let Some(previous) = previous {
        socket.forget(previous);
    }
    tracing::info!(
        client = %s.launch.client.name,
        %peer,
        local = %describe_local(local),
        stream = if is_audio { "audio" } else { "video" },
        "stream peer"
    );
    (!is_audio && s.take_new_path()).then_some(s)
}
pub struct Media {
    video: Arc<ReplySocket>,
    audio: Arc<ReplySocket>,
    control_wake: Arc<UdpSocket>,
    control_wake_receiver: UdpSocket,
    /// The microphone receiver's counts; None when its port could not be bound.
    pub(crate) mic: Option<Arc<crate::mic::Counters>>,
    pub peers: Mutex<Peers>,
    captures: Mutex<HashMap<CaptureKey, Weak<Source>>>,
    control_port: u16,
    bind: IpAddr,
    /// The control worker's answer to [`Media::clients_told_of_shutdown`].
    shutdown_requested: AtomicBool,
    shutdown_told: std::sync::atomic::AtomicUsize,
    shutdown_elapsed_ms: std::sync::atomic::AtomicU64,
    shutdown_done: AtomicBool,
    /// Roaming seqs of the launches, kept across control worker restarts so no launch key
    /// repeats a nonce.
    roam_seqs: butterpollo_session::roaming::SeqMemory,
}
#[derive(Debug, PartialEq, Eq, Hash)]
struct CaptureKey {
    kind: String,
    output: String,
    /// The user's Capture method was Automatic, which the fullscreen fallback
    /// may move to Desktop Duplication. An explicit method never shares its
    /// capture, or the fallback would move it behind the user's back.
    automatic: bool,
    hdr: bool,
    adapter: String,
    adapter_id: String,
    phase: String,
    compute: bool,
    wgc_compute: bool,
    wgc_user_helper: bool,
    wgc_high_rate: bool,
    wgc_drain: bool,
    wgc_helper_scheduling: bool,
    idd_fence_ring: bool,
    background: bool,
    constant_rate: bool,
}
impl CaptureKey {
    fn new(
        kind: &str,
        output: &str,
        hdr: bool,
        config: &Config,
        phase: &str,
        constant_rate: bool,
    ) -> Self {
        Self {
            kind: kind.into(),
            output: output.into(),
            automatic: butterpollo_media_policy::capture_policy::automatic_capture(
                config.get("capture", ""),
            ),
            hdr,
            adapter: config.get("adapter_name", "").into(),
            adapter_id: config.get("adapter_pnp_id", "").into(),
            phase: if config.boolean("wgc_slot_aligned_publish", false) {
                phase
            } else {
                ""
            }
            .into(),
            compute: config.boolean("gpu_compute_conversion", true),
            wgc_compute: config.boolean("wgc_compute_copy", true),
            wgc_user_helper: config.boolean("wgc_user_helper", false),
            wgc_high_rate: !matches!(kind, "ddx" | "dxgi")
                && config.boolean("wgc_high_rate_capture", false),
            wgc_drain: !matches!(kind, "ddx" | "dxgi")
                && config.boolean("wgc_drain_to_newest", false),
            wgc_helper_scheduling: !matches!(kind, "ddx" | "dxgi")
                && config.boolean("wgc_helper_streaming_scope", false),
            idd_fence_ring: config.idd_push_fence_ring(),
            background: butterpollo_session::background::enabled(config),
            constant_rate,
        }
    }
}
/// The first video ping from the path a client roamed to: the client needs a keyframe,
/// and what the link-rate controller learned about the old path no longer holds.
fn restart_on_new_path(s: &Session, c: &Config) {
    s.new_network_path();
    s.request_host_keyframe();
    let mut stream = s.config.clone();
    s.requested_mode()
        .apply_to(&mut stream, c.max_stream_resolution());
    s.rebuild_link_rate(&stream, c.link_rate_adaptation(), max_bitrate_kbps(s, c));
    rebuild_congestion(s, &stream, c);
}
/// The stream's video FEC at its start: the configured percentage, for every
/// frame (`fec_percentage`, PyroWave included unless it keeps a share of its
/// own), unless the client chose its own with `rlFecPercentage`.
fn configure_video_fec(s: &Session, c: &Config) -> Result<()> {
    let requested_fec = c.integer("fec_percentage", 20);
    if requested_fec != requested_fec.clamp(0, 100) {
        s.launch.warnings.set("network_fec_config", format!("FEC percentage {requested_fec} is outside 0-100; using {}%. Correct fec_percentage in Video settings.", requested_fec.clamp(0, 100)));
    }
    let fec_ceiling = if s.config.codec == 3 {
        c.pyrowave_fec_percentage().clamp(0, 255) as usize
    } else {
        requested_fec.clamp(0, 100) as usize
    };
    // A client's `rlFecPercentage` was checked at /launch.
    let choice = butterpollo_media_policy::extensions::fec_choice(&s.launch.options)?;
    let maximum = butterpollo_config::config::FEC_MAX_PERCENTAGE;
    let start = s.configure_fec_choice(choice, false, fec_ceiling, maximum);
    if choice != butterpollo_media_policy::extensions::FecChoice::Host {
        tracing::info!(
            client = %s.launch.client.name,
            choice = %choice.as_param(),
            pyrowave = s.config.codec == 3,
            percent = start,
            maximum,
            "client fec choice"
        );
    }
    Ok(())
}
/// Packet-level congestion control from the client's arrival feedback: `(enabled, shadow)`.
/// `congestion_control` turns it on; `BUTTERPOLLO_CONGESTION_SHADOW=1` runs it measure-only
/// for the A/B (`rust/release/congestion_ab.py`): it logs what it would do, applies nothing.
pub(crate) fn congestion_mode(c: &Config) -> (bool, bool) {
    let shadow = std::env::var("BUTTERPOLLO_CONGESTION_SHADOW").is_ok_and(|v| v == "1");
    (c.congestion_control() || shadow, shadow)
}
/// Congestion control for a stream whose mode or path changed; call after
/// `rebuild_link_rate`.
fn rebuild_congestion(
    s: &Session,
    stream: &butterpollo_media_policy::rtsp::Negotiated,
    c: &Config,
) {
    let (enabled, shadow) = congestion_mode(c);
    s.rebuild_congestion(stream, enabled, max_bitrate_kbps(s, c), shadow);
}
/// The user's `max_bitrate`, within what the stream's codec can be asked for:
/// PyroWave goes to 2 Gbps, the others to 500 Mbps.
fn max_bitrate_kbps(s: &Session, c: &Config) -> u32 {
    let maximum = if s.config.codec == 3 {
        2_000_000
    } else {
        500_000
    };
    c.integer("max_bitrate", 0).clamp(0, maximum) as u32
}
/// The local address a client's datagrams arrived on, for logs.
fn describe_local(local: Option<IpAddr>) -> String {
    local.map_or_else(|| "unknown".into(), |local| local.to_string())
}
fn capture_config(config: &Config) -> Config {
    let mut capture = config.clone();
    // Keep the effective interval choice with the shared capture and its helper.
    // A 1 ms request skipped updates in the 120 FPS motion comparison; use
    // explicit zero at every rate unless the user opts into the legacy limit.
    capture.values.insert(
        "wgc_high_rate_capture".into(),
        config.boolean("wgc_high_rate_capture", false).to_string(),
    );
    capture
}
/// Whether a stream's video socket refuses to fragment its datagrams: never.
/// The video socket is process-wide, so a flag set for one jumbo stream would
/// govern every stream, including a stock client behind a WireGuard or
/// Tailscale tunnel whose 1280-byte path would refuse every 1400-byte
/// datagram. The jumbo offer checks the route's MTU, and the PyroWave UDP
/// probe sets the flag on its own socket to test the path with the stream's
/// packet size before a stream starts.
pub(crate) fn video_refuses_fragmentation() -> bool {
    false
}

impl Media {
    /// The address the host listens on: unspecified unless it was bound to one.
    pub fn bind(&self) -> IpAddr {
        self.bind
    }

    /// Host shutdown: the control worker tells every client that its stream ends, its
    /// forwarded USB devices detaching first, through the path a normal stop uses. Wakes
    /// the worker and waits up to `wait` for it. Returns the clients told and how long
    /// telling them took, in milliseconds; `None` when the worker had not finished then.
    pub fn clients_told_of_shutdown(&self, wait: Duration) -> Option<(usize, u64)> {
        self.shutdown_requested.store(true, Ordering::Release);
        let _ = self.control_wake.send(&[1]);
        let deadline = Instant::now() + wait;
        while !self.shutdown_done.load(Ordering::Acquire) {
            if Instant::now() >= deadline {
                return None;
            }
            thread::sleep(Duration::from_millis(2));
        }
        Some((
            self.shutdown_told.load(Ordering::Acquire),
            self.shutdown_elapsed_ms.load(Ordering::Acquire),
        ))
    }

    pub fn new(h: Shared, bind: IpAddr) -> Result<Arc<Self>> {
        let ports = h.config.read().unwrap().ports()?;
        let video = ReplySocket::new(crate::network::udp((bind, ports.video).into())?)?;
        let audio = ReplySocket::new(crate::network::udp((bind, ports.audio).into())?)?;
        butterpollo_windows::net::configure_udp(&video)?;
        butterpollo_windows::net::configure_udp(&audio)?;
        video.set_nonblocking(true)?;
        audio.set_nonblocking(true)?;
        let control_wake_receiver = UdpSocket::bind("127.0.0.1:0")?;
        let control_wake = UdpSocket::bind("127.0.0.1:0")?;
        control_wake.connect(control_wake_receiver.local_addr()?)?;
        control_wake_receiver.connect(control_wake.local_addr()?)?;
        control_wake.set_nonblocking(true)?;
        control_wake_receiver.set_nonblocking(true)?;
        // A microphone port taken by another program only costs the microphone.
        let mic = crate::network::udp((bind, ports.mic).into())
            .and_then(|socket| crate::mic::spawn(h.clone(), socket))
            .inspect_err(|error| {
                tracing::warn!(port = ports.mic, error = %format!("{error:#}"), "client microphones unavailable")
            })
            .ok();
        let m = Arc::new(Self {
            video: Arc::new(video),
            audio: Arc::new(audio),
            control_wake: Arc::new(control_wake),
            control_wake_receiver,
            mic,
            peers: Mutex::default(),
            captures: Mutex::new(HashMap::new()),
            control_port: ports.control,
            bind,
            shutdown_requested: AtomicBool::new(false),
            shutdown_told: std::sync::atomic::AtomicUsize::new(0),
            shutdown_elapsed_ms: std::sync::atomic::AtomicU64::new(0),
            shutdown_done: AtomicBool::new(false),
            roam_seqs: Default::default(),
        });
        for is_audio in [false, true] {
            let socket = if is_audio {
                m.audio.clone()
            } else {
                m.video.clone()
            };
            let m = m.clone();
            let h = h.clone();
            thread::Builder::new()
                .name(if is_audio { "audio-ping" } else { "video-ping" }.into())
                .spawn(move || {
                    let mut b = [0; 2048];
                    let raw_socket = std::os::windows::io::AsRawSocket::as_raw_socket(&*socket);
                    while !h.stop.load(Ordering::Acquire) {
                        match butterpollo_windows::net::wait_readable(raw_socket, 250) {
                            Ok(true) => {}
                            Ok(false) => continue,
                            Err(e) => {
                                tracing::warn!(error=%e,"media ping wait failed");
                                thread::sleep(Duration::from_millis(250));
                                continue;
                            }
                        }
                        match socket.receive(&mut b) {
                            Ok((n, peer, local)) => {
                                let roamed = take_media_ping(
                                    &h,
                                    &m,
                                    &socket,
                                    is_audio,
                                    &b[..n],
                                    peer,
                                    local,
                                );
                                if let Some(s) = roamed {
                                    restart_on_new_path(&s, &h.config.read().unwrap());
                                }
                            }
                            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                            Err(e) => {
                                tracing::warn!(error=%e,"media ping receive failed");
                                thread::sleep(Duration::from_millis(10));
                            }
                        }
                    }
                })?;
        }
        let control = m.clone();
        thread::Builder::new()
            .name("control".into())
            .spawn(move || {
                // Input and control for every session pass through this
                // worker: restart it rather than leave later sessions without.
                while !h.stop.load(Ordering::Acquire) {
                    match control.control(h.clone()) {
                        Ok(()) => break,
                        Err(e) => {
                            tracing::error!(error = %format!("{e:#}"), "control worker stopped; restarting");
                            thread::sleep(Duration::from_secs(1));
                        }
                    }
                }
            })?;
        Ok(m)
    }
    fn capture(
        &self,
        hdr: bool,
        config: &Config,
        rate: butterpollo_media_policy::framegen::Rate,
        phase: &str,
        prepared: Arc<crate::display_session::Ready>,
    ) -> Result<Arc<Source>> {
        // One read of the stream's policy: the method and the repeat mode
        // must agree, or a capture is keyed under one and opened as the other.
        let (kind, constant_rate) = prepared.capture_method();
        let output = prepared.output();
        let aligned = config.boolean("wgc_slot_aligned_publish", false);
        // The user's Capture method, not the resolved backend: an explicit method is
        // never moved to another backend (`capture_policy::automatic_capture`).
        let automatic =
            butterpollo_media_policy::capture_policy::automatic_capture(config.get("capture", ""));
        let capture_config = capture_config(config);
        let key = CaptureKey::new(&kind, &output, hdr, &capture_config, phase, constant_rate);
        let mut captures = self.captures.lock().unwrap();
        captures.retain(|_, source| source.strong_count() > 0);
        if let Some(existing) = captures.get(&key).and_then(Weak::upgrade)
            && existing.check().is_ok()
        {
            return Ok(existing);
        }
        let latest = Arc::new(Latest::new());
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker = latest.clone();
        let warnings = Arc::new(butterpollo_session::session::Warnings::default());
        let capture_warnings = warnings.clone();
        let grid = Arc::new(Mutex::new(butterpollo_windows::capture::ClaimGrid {
            anchor: Instant::now(),
            period: rate.period(),
        }));
        let worker_grid = grid.clone();
        let backend = Arc::new(Mutex::new(""));
        let worker_backend = backend.clone();
        let (started_tx, started_rx) = std::sync::mpsc::sync_channel(1);
        // Before the wait below begins: the capture's tries end within it.
        let started = Instant::now();
        let thread = thread::Builder::new()
            .name("capture".into())
            .spawn(move || {
                let result = (|| -> Result<()> {
                    let _com = ComGuard::new()?;
                    let _priority = Priority::new();
                    let _display_awake = butterpollo_windows::timing::DisplayAwake::enter()
                        .inspect_err(|error| tracing::warn!(%error, "capture cannot keep the display awake"))
                        .ok();
                    let timer = butterpollo_windows::timing::Timer::new()?;
                    // Duplicate the desktop that is showing, including the secure
                    // desktop of a UAC prompt or the lock screen.
                    butterpollo_windows::input::follow_input_desktop();
                    let opened = open_first(
                        || prepared.capture_target(),
                        |target| Capture::open_for_stream_reported(&target.0, &kind, hdr, &capture_config, capture_warnings.clone()),
                        |tried| {
                            let replaced = prepared.capture_target() != *tried;
                            wait_to_open_again(started, &worker_stop, replaced)
                        },
                    );
                    let (mut capture, mut target) = match opened {
                        Ok(opened) => opened,
                        Err(e) => {
                            let _ = started_tx.send(Err(format!("{e:#}")));
                            return Ok(());
                        }
                    };
                    // The stream's display (and its revision) when the capture last opened on a
                    // new device: a window target switches on that device only while it stays.
                    let mut opened_display = prepared.display_target();
                    capture.set_claim_grid(worker_grid.clone(), aligned);
                    *worker_backend.lock().unwrap() = capture.backend();
                    tracing::info!(requested=%kind, backend=capture.backend(), output=%target.0, constant_rate = constant_rate, "capture backend opened");
                    // Re-create the capture on a new device. Windows keeps handing
                    // out the stale adapter while anything holds the old device, so
                    // every duplication made then loses access at once (a display
                    // arriving for another client does this). The old capture is
                    // dropped and consumers acknowledge releasing their frames
                    // and encoders before the new device is made.
                    // The capture, whether it shows the primary display in
                    // place of the stream's, which is gone, and the stream's
                    // display it was opened for. The display can be recreated
                    // while capture opens; the next check then sees that.
                    // `chosen` is normally the stream's own backend; a display whose
                    // Windows Graphics Capture picture stopped asks for Desktop
                    // Duplication instead. A refused switch keeps the working one.
                    let reopen = |lost: Capture, target: &(String, u64), chosen: &str| -> Result<(Capture, bool, (String, u64))> {
                        let recovery_started = Instant::now();
                        // Desktop Duplication reports the pointer's shape only
                        // when it changes; the new capture starts from this one.
                        let mut pointer = lost.pointer();
                        worker.begin_recovery()?;
                        drop(lost);
                        let deadline = Instant::now() + Duration::from_secs(30);
                        while !worker.consumers_released() {
                            worker.check()?;
                            if worker_stop.load(Ordering::Acquire) {
                                bail!("capture stopped while releasing resources");
                            }
                            anyhow::ensure!(Instant::now() < deadline, "streams did not release the lost capture device");
                            timer.until(Instant::now() + Duration::from_millis(2));
                        }
                        let release_ms = recovery_started.elapsed().as_millis();
                        let mut missing_since = None;
                        loop {
                            worker.check()?;
                            anyhow::ensure!(Instant::now() < deadline, "capture did not recover within 30 seconds");
                            if worker_stop.load(Ordering::Acquire) {
                                bail!("capture stopped while recovering");
                            }
                            let next = prepared.capture_target();
                            // A UAC prompt or the lock screen switches the input
                            // desktop; duplication must be made on that desktop.
                            butterpollo_windows::input::follow_input_desktop();
                            let background = butterpollo_session::background::enabled(&capture_config);
                            let on = reopen_on(butterpollo_windows::capture::display_present(&next.0), background, &mut missing_since, Instant::now());
                            let opened = match on {
                                ReopenOn::Wait => {
                                    if background {
                                        capture_warnings.set("capture_display", "The background display disappeared. Waiting for it to recover; the PC's monitor is not captured.");
                                    }
                                    thread::sleep(RECOVERY_RETRY);
                                    continue;
                                }
                                ReopenOn::Stream => match Capture::open_for_stream_reported(&next.0, chosen, hdr, &capture_config, capture_warnings.clone()) {
                                    Ok(opened) => Ok(opened),
                                    // Keep the backend that was working: Desktop
                                    // Duplication cannot capture this display (another
                                    // vendor's GPU drives it, or it is turned), and a
                                    // frozen picture is worse than a picture. The
                                    // warning says so after the reopen: opening the
                                    // stream's own backend clears it otherwise.
                                    Err(error) if chosen != kind.as_str() => {
                                        match Capture::open_for_stream_reported(&next.0, &kind, hdr, &capture_config, capture_warnings.clone()) {
                                            Ok(opened) => {
                                                if opened.backend() == "wgc" {
                                                    capture_warnings.set("capture_backend", format!("Capturing with Windows Graphics Capture: Desktop Duplication cannot capture this display ({error:#})."));
                                                }
                                                Ok(opened)
                                            }
                                            Err(failure) => Err(failure),
                                        }
                                    }
                                    Err(error) => Err(error),
                                },
                                // By name: the WGC helper checks it opened the display asked for.
                                ReopenOn::Primary => butterpollo_windows::capture::primary_display()
                                    .context("no display is on the desktop")
                                    .and_then(|primary| Capture::open_for_stream_reported(&primary, &kind, hdr, &capture_config, capture_warnings.clone())),
                            };
                            match opened {
                                Ok(mut recovered) => {
                                    if on == ReopenOn::Primary {
                                        capture_warnings.set("capture_display", format!("The stream's display {} disappeared and did not return within {} s; showing the primary display until it returns. A game or Windows changed the display layout; reconnect if the picture does not return.", next.0, DISPLAY_RETURN_WAIT.as_secs()));
                                    } else {
                                        capture_warnings.clear("capture_display");
                                    }
                                    if next != *target {
                                        tracing::info!(output = %next.0, "capture moved to the recreated display");
                                    }
                                    if let Some(pointer) = pointer.take()
                                        && let Err(error) = recovered.resume_pointer(pointer)
                                    {
                                        tracing::warn!(%error, "the pointer appears once it moves or changes");
                                    }
                                    tracing::info!(release_ms, elapsed_ms=recovery_started.elapsed().as_millis(), backend=recovered.backend(), "capture reopened after resource release");
                                    return Ok((recovered, on == ReopenOn::Primary, next));
                                }
                                Err(error) if Instant::now() >= deadline => return Err(error),
                                Err(_) => thread::sleep(RECOVERY_RETRY),
                            }
                        }
                    };
                    // A stream that stopped waiting for its capture has none to feed.
                    if started_tx.send(Ok(())).is_err() {
                        return Ok(());
                    }
                    let progress = crate::stall_watch::register("capture", &output, None);
                    let mut check_target = Instant::now();
                    let mut user_desktop = kind == "wgc"
                        && butterpollo_windows::capture::wgc_desktop_available();
                    let mut on_primary = false;
                    let poll_interval = Duration::from_micros(
                        capture_config.integer("capture_poll_interval_us", 500).clamp(100, 1000) as u64,
                    );
                    let mut last_frame = Instant::now();
                    // When to ask again whether a frozen display's picture is one
                    // Windows Graphics Capture cannot see; see the `Ok(None)` arm.
                    let mut stall_check = Instant::now();
                    // One attempt per capture worker, and whether it moved the stream
                    // to Desktop Duplication for good: from then on it reopens there.
                    let mut duplication_tried = false;
                    let mut switched_to_duplication = false;
                    // Constant-rate capture (`capture = wgcc`): one picture per
                    // stream slot, the last one again when nothing new was
                    // composed for it. `origin` names the content of a fresh
                    // picture so its repeats reuse the converted picture.
                    let mut constant = constant_rate
                        .then(butterpollo_media_policy::capture_policy::ConstantRate::default);
                    let mut origin = 0u64;
                    while !worker_stop.load(Ordering::Acquire) {
                        if Instant::now() >= check_target {
                            check_target = Instant::now() + Duration::from_millis(100);
                            let next = prepared.capture_target();
                            let available = kind == "wgc"
                                && butterpollo_windows::capture::wgc_desktop_available();
                            let return_to_wgc = returns_to_wgc(available, user_desktop, capture.backend(), &kind, switched_to_duplication);
                            user_desktop = available;
                            // Back from the primary display once the stream's returns.
                            let returned = on_primary && butterpollo_windows::capture::display_present(&next.0);
                            let restart = worker.take_device_restart()?;
                            // The app's window found, minimised or restored on the same display
                            // and not recreated: the new target opens on the capture's own device
                            // and the encoder keeps going. A device problem, another display or
                            // backend reopens on a new device below, releasing the encoder.
                            let in_place = !restart && !return_to_wgc && !returned && next != target
                                && prepared.display_target() == opened_display;
                            if in_place
                                && let Some(switched) = capture.switch_on_device(&next.0, &kind, hdr, &capture_config, capture_warnings.clone())
                            {
                                match switched {
                                    Ok(switched) => {
                                        drop(std::mem::replace(&mut capture, switched));
                                        tracing::info!(from = %target.0, to = %next.0, "capture switched target on its device");
                                        target = next.clone();
                                        capture.set_claim_grid(worker_grid.clone(), aligned);
                                        *worker_backend.lock().unwrap() = capture.backend();
                                        last_frame = Instant::now();
                                        continue;
                                    }
                                    Err(error) => tracing::info!(error = %format!("{error:#}"), to = %next.0, "the capture could not switch target on its device; reopening it"),
                                }
                            }
                            if restart || next != target || return_to_wgc || returned {
                                let lost = std::mem::replace(&mut capture, Capture::Closed);
                                progress.mark(crate::stall_watch::Phase::Reopen);
                                (capture, on_primary, target) = match reopen(lost, &next, reopen_backend(&kind, switched_to_duplication)) {
                                    Ok(capture) => capture,
                                    Err(_) if worker_stop.load(Ordering::Acquire) => return Ok(()),
                                    Err(error) => return Err(error),
                                };
                                if switched_to_duplication && capture.backend() == "ddx" {
                                    capture_warnings.set("capture_backend", DUPLICATION_WARNING);
                                }
                                opened_display = prepared.display_target();
                                capture.set_claim_grid(worker_grid.clone(), aligned);
                                *worker_backend.lock().unwrap() = capture.backend();
                                last_frame = Instant::now();
                            }
                        }
                        // Reset before the helper's announcements are read: one
                        // that comes after them ends the wait below.
                        if let Some(signal) = capture.frame_signal() {
                            signal.reset()?;
                        }
                        progress.mark(crate::stall_watch::Phase::Capture);
                        // The publication identity this picture carries when the
                        // capture repeats (`capture = wgcc`): the worker's
                        // counter, which a repeat keeps. 0 when the capture
                        // does not repeat; a picture the capture did not return
                        // must not spend one.
                        let publication = constant
                            .as_ref()
                            .map(|_| origin.wrapping_add(1).max(1))
                            .unwrap_or(0);
                        let captured = (|| {
                            #[cfg(debug_assertions)]
                            crate::soak_fault::check("DXGI_ERROR_ACCESS_LOST")?;
                            capture.next_gpu(publication)
                        })();
                        match captured {
                            Ok(Some(image)) => {
                                last_frame = Instant::now();
                                capture_warnings.clear("capture_recovery");
                                let (captured, focus) = (image.captured, image.focus);
                                if let Some(rate) = &mut constant {
                                    origin = publication;
                                    rate.published(image.published);
                                }
                                worker.publish_focus(image, captured, focus)?;
                            }
                            Ok(None) => {
                                // Windows Graphics Capture reads the compositor's output. An
                                // app in exclusive fullscreen presents past it, so the picture
                                // stops with no error while the display keeps changing; Desktop
                                // Duplication reads what the display shows (issue #123). Only
                                // Automatic moves: an explicit Capture method is the user's,
                                // including for the one-reconnect comparison.
                                let now = Instant::now();
                                let idle = now.saturating_duration_since(last_frame);
                                let display = butterpollo_windows::capture::WindowTarget::parse(&target.0).ok().flatten().is_none();
                                // The shell's own answer first, at most twice a second: enumerating
                                // the windows and the displays behind it is the expensive part of
                                // the probe, and most still pictures have no exclusive-fullscreen
                                // app.
                                if automatic
                                    && !duplication_tried
                                    && capture.backend() == "wgc"
                                    && display
                                    && idle >= butterpollo_media_policy::capture_policy::FULLSCREEN_STALL
                                    && fullscreen_probe_due(now, &mut stall_check)
                                    && butterpollo_windows::foreground::exclusive_fullscreen()
                                {
                                    // The display's rectangle as it is now: a game that
                                    // changed its mode after the capture opened (issue #123)
                                    // would not match the rectangle the capture opened on.
                                    let in_front = butterpollo_windows::foreground::in_front(&target.0);
                                    if butterpollo_media_policy::capture_policy::duplication_for_fullscreen(
                                        automatic,
                                        capture.backend(),
                                        display,
                                        idle,
                                        in_front,
                                        true,
                                    ) {
                                        duplication_tried = true;
                                        tracing::info!(idle_ms = idle.as_millis(), output = %target.0, "an exclusive-fullscreen app is in front and no picture came; capturing the display with Desktop Duplication");
                                        let lost = std::mem::replace(&mut capture, Capture::Closed);
                                        progress.mark(crate::stall_watch::Phase::Reopen);
                                        (capture, on_primary, target) = match reopen(lost, &target, "ddx") {
                                            Ok(capture) => capture,
                                            Err(_) if worker_stop.load(Ordering::Acquire) => return Ok(()),
                                            Err(error) => return Err(error),
                                        };
                                        if capture.backend() == "ddx" {
                                            switched_to_duplication = true;
                                            capture_warnings.set("capture_backend", DUPLICATION_WARNING);
                                        }
                                        opened_display = prepared.display_target();
                                        capture.set_claim_grid(worker_grid.clone(), aligned);
                                        *worker_backend.lock().unwrap() = capture.backend();
                                        last_frame = Instant::now();
                                        continue;
                                    }
                                }
                                // The slot's period is the fastest stream using
                                // this capture, so a rate change and a shared
                                // capture both follow it; the grid's period is
                                // the fallback before a stream declares one.
                                let constant_period = constant.as_ref().map(|_| {
                                    let fallback = worker_grid.lock().unwrap().period;
                                    worker.repeat_period(fallback)
                                });
                                // Nothing composed for this slot: publish the last
                                // picture again, stamped now, so the encoder keeps a
                                // steady cadence instead of repeating a still one.
                                if let (Some(rate), Some(period)) = (constant.as_mut(), constant_period) {
                                    let now = Instant::now();
                                    if matches!(
                                        rate.decide(now, period, false),
                                        Some(butterpollo_media_policy::capture_policy::Publish::Repeat)
                                    ) {
                                        match worker.current()? {
                                            Some(picture) => {
                                                let mut repeat = (*picture).clone();
                                                repeat.captured = now;
                                                repeat.acquired = now;
                                                repeat.published = now;
                                                repeat.wgc_stamp = None;
                                                repeat.focus = None;
                                                repeat.repeat = true;
                                                repeat.origin = picture.origin;
                                                // A refused repeat (a capture
                                                // restart) leaves the slot for the
                                                // next try.
                                                if worker.publish_repeat(Arc::new(repeat), now)? {
                                                    rate.repeated(now, period);
                                                }
                                            }
                                            // No picture to repeat (the capture is
                                            // being recreated): wait a period for
                                            // the next slot instead of spinning on
                                            // this one.
                                            None => rate.published(now),
                                        }
                                    }
                                }
                                let interval = if capture_config.boolean("capture_predictive_poll", false)
                                    && capture.backend() == "ddx" {
                                    worker.poll_interval(poll_interval)
                                } else { poll_interval };
                                let interval = capture.wait_interval(last_frame.elapsed(), interval);
                                let now = Instant::now();
                                let deadline = now + interval;
                                // Never sleep past the next constant-rate slot.
                                // One already due could not be published (a
                                // capture restart): the next poll tries again.
                                let due = constant_period
                                    .and_then(|period| constant.as_ref().and_then(|rate| rate.due(period)));
                                let until = capture.publication_deadline().unwrap_or(deadline).min(deadline);
                                let until = due
                                    .filter(|due| *due > now)
                                    .map_or(until, |due| until.min(due));
                                match capture.frame_signal() {
                                    // Frame announcements interrupt both active polling
                                    // and the longer wait used by a static source.
                                    Some(signal) => {
                                        timer.until_or_signal(until, signal)?;
                                    }
                                    None => timer.until(until),
                                }
                            }
                            Err(e) => {
                                // The app's window shown over the display was minimised or closed:
                                // its worker lays the display back within its check, and the next
                                // target check switches to it on this device, without releasing
                                // the encoder. Any other failure reopens on a new device.
                                if DeviceLost::from_error(&e).is_none()
                                    && target != prepared.display_target()
                                    && butterpollo_windows::capture::WindowTarget::parse(&target.0).ok().flatten().is_some()
                                {
                                    let deadline = Instant::now() + WINDOW_TARGET_WAIT;
                                    while prepared.capture_target() == target
                                        && Instant::now() < deadline
                                        && !worker_stop.load(Ordering::Acquire)
                                    {
                                        timer.until(Instant::now() + Duration::from_millis(10));
                                    }
                                    if prepared.capture_target() != target {
                                        tracing::info!(error = %format!("{e:#}"), "the app's window is unavailable; the capture follows its target");
                                        check_target = Instant::now();
                                        continue;
                                    }
                                }
                                if let Some(loss) = DeviceLost::from_error(&e) {
                                    worker.device_lost(loss, &capture_warnings, Instant::now())?;
                                    worker.take_device_restart()?;
                                } else {
                                    capture_warnings.set("capture_recovery", format!("Capture interrupted ({e:#}); reopening capture, with a frozen picture until frames resume. If this repeats, keep the display mode stable and check the WGC helper and graphics driver."));
                                }
                                let lost = std::mem::replace(&mut capture, Capture::Closed);
                                progress.mark(crate::stall_watch::Phase::Reopen);
                                (capture, on_primary, target) = match reopen(lost, &target, reopen_backend(&kind, switched_to_duplication)) {
                                    Ok(capture) => capture,
                                    Err(_) if worker_stop.load(Ordering::Acquire) => return Ok(()),
                                    Err(error) => return Err(error),
                                };
                                if switched_to_duplication && capture.backend() == "ddx" {
                                    capture_warnings.set("capture_backend", DUPLICATION_WARNING);
                                }
                                opened_display = prepared.display_target();
                                capture.set_claim_grid(worker_grid.clone(), aligned);
                                *worker_backend.lock().unwrap() = capture.backend();
                                last_frame = Instant::now();
                            }
                        }
                    }
                    Ok(())
                })();
                if let Err(e) = result {
                    if DeviceLost::from_error(&e).is_some() {
                        capture_warnings.set("gpu_recovery", format!("{e:#}"));
                    }
                    let _ = worker.fail(format!("{e:#}"));
                    tracing::error!(error=%format!("{e:#}"),"capture worker stopped");
                }
            })?;
        let latest = Arc::new(Source {
            warnings,
            latest,
            backend,
            grid,
            stop,
            thread: Some(thread),
        });
        started_rx
            .recv_timeout(CAPTURE_START_WAIT)?
            .map_err(|e| anyhow::anyhow!(e))?;
        captures.insert(key, Arc::downgrade(&latest));
        Ok(latest)
    }
    pub fn start(self: &Arc<Self>, h: Shared, s: Arc<Session>) {
        // Raised first, so a session that ends at once can't end before it started.
        h.session_webhook("session.started", &s);
        let result = self.spawn_session(&h, &s);
        if let Err(e) = result {
            let reason = format!("the stream could not start: {e:#}");
            h.sessions.lock().unwrap().fail_start(&s, reason);
            h.record_failure(&s);
            h.record_session(
                &s,
                butterpollo_session::session_history::REASON_START_FAILED,
            );
            tracing::error!(error=%e,"could not start session worker");
            h.session_webhook("session.ended", &s);
        }
    }
    fn open_session_capture<'a>(
        &self,
        s: &Arc<Session>,
        c: &Config,
        prepared: &Arc<crate::display_session::Ready>,
        latest: &'a mut Option<Arc<Source>>,
    ) -> Result<(&'a mut Arc<Source>, Arc<capture::Consumer>)> {
        let latest = latest.insert(self.capture(
            s.config.hdr,
            c,
            butterpollo_media_policy::framegen::Rate(s.config.fps_millihz()),
            &s.launch.id,
            prepared.clone(),
        )?);
        *s.capture_warnings.write().unwrap() = latest.warnings.clone();
        let capture_wake = latest.subscribe(Arc::downgrade(s))?;
        capture_wake.wake_on_recovery(s);
        Ok((latest, capture_wake))
    }
    fn set_display_hdr_metadata(
        &self,
        s: &Session,
        display: butterpollo_media_policy::hdr::Metadata,
    ) -> butterpollo_media_policy::hdr::Metadata {
        let hdr = s.hdr_active();
        let previous = s.hdr_metadata.read().unwrap().map(|m| m.wire(hdr));
        let metadata = s.set_display_hdr_metadata(display);
        if previous != Some(metadata.wire(hdr)) {
            let _ = self.control_wake.send(&[1]);
        }
        metadata
    }
    /// Drops a GameStream session out of HDR to 10-bit SDR when Windows left the
    /// captured display in SDR with a BGRA8 surface and RTX HDR is not running,
    /// waking control so clients receive `0x010e` (`enabled=false`).
    fn fallback_hdr_if_sdr(
        &self,
        s: &Session,
        stream: &mut butterpollo_media_policy::rtsp::Negotiated,
        image: &GpuImage,
        rtx_hdr_active: bool,
    ) -> bool {
        let display_in_hdr = image.gpu.display_in_hdr();
        if !should_fallback_hdr_to_sdr(
            s.launch.transport,
            stream.hdr,
            image.pixel == butterpollo_windows::capture::Pixel::Bgra8,
            display_in_hdr,
            rtx_hdr_active,
        ) {
            return false;
        }
        butterpollo_media_policy::stream_policy::stream_sdr_10bit(stream);
        if s.disable_hdr() {
            let _ = self.control_wake.send(&[1]);
        }
        tracing::warn!(
            client = %s.launch.client.name,
            output = %image.gpu.display.display_name,
            source_pixel = ?image.pixel,
            "display HDR did not stay active; falling back to 10-bit SDR for this stream"
        );
        s.launch.warnings.set(
            "display_hdr",
            "Requested display HDR state was not applied; capture source HDR is off, so this stream fell back to SDR. Check Windows HDR and the display policy, or reconnect with HDR disabled.",
        );
        true
    }
    fn configure_encoder(
        &self,
        s: &Session,
        c: &Config,
        first: &GpuImage,
        prepared: &crate::display_session::Ready,
        encoder: &mut Option<Encoder>,
    ) -> Result<()> {
        let stream = s.effective_config();
        *encoder = Some(Encoder::new_gpu_reported(
            &stream,
            first,
            c,
            &s.launch.warnings,
        )?);
        *s.encoder.write().unwrap() = encoder.as_ref().unwrap().backend().into();
        let source_refresh_hz = butterpollo_windows::display::mode(&first.gpu.display.display_name)
            .ok()
            .map(|mode| mode.dmDisplayFrequency);
        let (requested_capture, constant_rate) = prepared.capture_method();
        tracing::info!(width=stream.width,height=stream.height,display_width=stream.display_size().0,display_height=stream.display_size().1,capped=stream.capped(),fps=f64::from(stream.fps_millihz())/1000.,requested_fps=stream.fps,bitrate_kbps=s.bitrate.load(Ordering::Relaxed),configured_bitrate_kbps=stream.configured_bitrate_kbps,codec=stream.codec,hdr=stream.hdr,ten_bit=stream.ten_bit(),full_range=stream.full_range(),color_matrix=stream.color_matrix(),vrr=stream.vrr_low_latency,packet_size=stream.packet_size,pyrowave_records=stream.pyrowave_records,pyrowave_row_order=stream.pyrowave_row_order,requested_capture=%requested_capture,constant_rate=constant_rate,requested_encoder=c.get("encoder","auto"),encoder=encoder.as_ref().unwrap().backend(),adapter=%first.gpu.display.adapter,source_width=first.width,source_height=first.height,source_refresh_hz,source_pixel=?first.pixel,"stream configured");
        if stream.chroma_packing != 0 {
            let (coded_width, coded_height) = stream.coded_size();
            tracing::info!(
                packing_version = stream.chroma_packing,
                codec = stream.codec,
                coded_width,
                coded_height,
                bit_depth = if stream.ten_bit() { 10 } else { 8 },
                "packed 4:4:4 stream"
            );
        }
        // The same floor link-rate adaptation stops at, measured on the project's test scenes.
        // Whole frames per second, as the client's minimum is, so 59.94 compares as 60.
        let minimum = butterpollo_media_policy::link_rate::pyrowave_floor_kbps(&stream);
        let recommended = butterpollo_media_policy::link_rate::pyrowave_recommended_kbps(&stream);
        let bitrate = s.bitrate.load(Ordering::Relaxed);
        if stream.codec == 3 && bitrate < minimum {
            tracing::warn!(
                bitrate_kbps = bitrate,
                minimum_kbps = minimum,
                recommended_kbps = recommended,
                "PyroWave bitrate is below the level where the project's test scenes kept detail; text and textures may lose detail. Raise the bitrate in Moonlight with network headroom, lower the resolution or frame rate, or use HEVC or AV1"
            );
        } else if stream.codec == 3 && bitrate < recommended {
            tracing::warn!(
                bitrate_kbps = bitrate,
                minimum_kbps = minimum,
                recommended_kbps = recommended,
                "PyroWave bitrate is below recommended: text and textures may lose detail. Quality depends on the picture; raise the bitrate in Moonlight with network headroom, or use HEVC or AV1"
            );
        }
        // HDR metadata describes the captured source, independent of receiver
        // brightness. RTX HDR's picture is generated at its own peak, not the
        // SDR monitor's; its enhancer reports the same from its first frame.
        let mut display_metadata = first.gpu.hdr_metadata();
        if let Some(peak) = crate::nvidia_enhance::generated_peak(c, stream.hdr, first.pixel) {
            display_metadata = display_metadata.generated(peak);
        }
        let metadata = self.set_display_hdr_metadata(s, display_metadata);
        if let Some(encoder) = encoder.as_mut() {
            encoder.set_hdr_metadata(metadata);
            encoder.set_hdr_target(s.hdr_target_nits());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
