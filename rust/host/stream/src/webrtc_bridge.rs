//! Trait seams between `butterpollo-stream`, `butterpollo-webrtc`, and `butterpollo-web` so
//! the stream runtime never depends on str0m, WebRTC signalling, or the web console.

use crate::state::{Session, Shared};
use anyhow::Result;
use axum::{extract::WebSocketUpgrade, http::HeaderMap, response::Response};
use butterpollo_config::config::Config;
use butterpollo_media_policy::{audio::OpusLayout, performance::Claims};
use butterpollo_session::{
    state::Client,
    webrtc::{
        guests::{Invite, InviteError, Permission},
        protocol::ErrorCode,
    },
};
use butterpollo_win_graphics::encoder::Encoded;
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    any::Any,
    collections::HashMap,
    net::SocketAddr,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Notify;

/// Guests at once. Guest `n` (0-based) gets UDP port `webrtc_media_port + 1 + n`.
pub const MAX_GUESTS: u16 = 4;
/// The WebSocket subprotocol of the console's own stream page.
pub const CONSOLE_PROTOCOL: &str = "rubylight.signal.v2";
/// The WebSocket subprotocol of the play page's signalling route on the guest port.
pub const PLAY_PROTOCOL: &str = "rubylight.play.v2";
/// The guest page's WebSocket subprotocol.
pub const GUEST_PROTOCOL: &str = "rubylight.guest.v1";
/// How long an answered WebRTC session may take to connect plus HDR check grace.
const DEFAULT_HELD_LIMIT: Duration = Duration::from_secs(30);

/// Result of queueing an audio packet onto a WebRTC media transport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Submit {
    Queued,
    Dropped,
}

/// An encoded Opus packet handed from `stream::audio` to the WebRTC media thread.
pub struct AudioPacket {
    /// One Opus packet (or one multistream packet), sent as is.
    pub data: Vec<u8>,
    /// When the audio thread handed the packet over. Only str0m's NTP pairing uses it.
    pub capture: Instant,
    /// Where this packet starts in the audio thread's own stream, in 48 kHz samples.
    pub position: u64,
}

/// Audio sink abstraction implemented by `butterpollo_webrtc::media::AudioSender`.
pub trait WebRtcAudioSink: Send {
    /// The multistream layout the encoder must use.
    fn layout(&self) -> &OpusLayout;
    /// Never blocks; a full queue drops the packet.
    fn submit(&self, packet: AudioPacket) -> Submit;
}

/// Video sink abstraction implemented by `butterpollo_webrtc::session::Sender`.
pub trait WebRtcVideoSink: Send {
    fn peer(&self) -> Option<SocketAddr>;
    fn simulated_losses(&self) -> Option<u64>;
    fn submit(
        &self,
        session: &Session,
        frames: Vec<Encoded>,
        call_latency: Duration,
        processing_started: Instant,
        claims: &mut Claims,
    ) -> Result<()>;
}

/// Media link held between signalling setup and stream worker startup.
pub trait WebRtcMediaLink: Send + Sync {
    fn audio_sender(&self) -> Option<Box<dyn WebRtcAudioSink>>;
    fn into_video_sink(self: Box<Self>) -> Box<dyn WebRtcVideoSink>;
}

/// Registry of WebRTC media links keyed by launch ID, plus session ID allocator.
#[derive(Default)]
pub struct WebRtcLinks {
    media: Mutex<HashMap<String, Box<dyn WebRtcMediaLink>>>,
    ids: AtomicU64,
    held_limit_ms: AtomicU64,
}

impl WebRtcLinks {
    pub fn held_limit(&self) -> Duration {
        match self.held_limit_ms.load(Ordering::Relaxed) {
            0 => DEFAULT_HELD_LIMIT,
            ms => Duration::from_millis(ms),
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn limit_held(&self, limit: Duration) {
        let ms = limit.as_millis().min(u128::from(u64::MAX)) as u64;
        self.held_limit_ms.store(ms, Ordering::Relaxed);
    }

    pub fn next_session_id(&self) -> u64 {
        self.ids.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub fn put(&self, launch: String, media: Box<dyn WebRtcMediaLink>) {
        let replaced = self.media.lock().unwrap().insert(launch, media);
        drop(replaced);
    }

    pub fn take(&self, launch: &str) -> Option<Box<dyn WebRtcMediaLink>> {
        self.media
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(launch)
    }

    pub fn audio_sender(&self, launch: &str) -> Option<Box<dyn WebRtcAudioSink>> {
        self.media.lock().unwrap().get(launch)?.audio_sender()
    }

    pub fn discard(&self, launch: &str) {
        drop(self.take(launch));
    }

    pub fn len(&self) -> usize {
        self.media.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Snapshot of active browser-guest counts and invite/prepare expiry deadlines so
/// `GuestNetworkState` evaluates expiry live without waiting for a mutation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GuestNetDeadlines {
    pub joined_any: bool,
    pub joined_internet: bool,
    /// Latest expiry (unix seconds) among live invites, if any.
    pub open_until_unix: Option<u64>,
    /// Latest expiry (unix seconds) among live internet invites, if any.
    pub internet_until_unix: Option<u64>,
    /// Monotonic deadline of `prepare_internet()` ("On the internet" dialog).
    pub prepared_until: Option<Instant>,
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn unix_deadline(exp: u64, now_sys: SystemTime, now: Instant) -> Option<Instant> {
    let target = UNIX_EPOCH + Duration::from_secs(exp);
    let remaining = target.duration_since(now_sys).ok()?;
    (!remaining.is_zero()).then(|| now + remaining)
}

/// Shared network/port-mapping state for browser guests read by `network` and `web`.
#[derive(Default)]
pub struct GuestNetworkState {
    deadlines: Mutex<GuestNetDeadlines>,
    internet_round: AtomicU64,
    mapping_notify: Notify,
}

impl GuestNetworkState {
    pub fn update(&self, deadlines: GuestNetDeadlines) {
        *self
            .deadlines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = deadlines;
    }

    pub fn open(&self) -> bool {
        self.open_at(unix_now())
    }

    pub fn open_at(&self, now_unix: u64) -> bool {
        let d = *self
            .deadlines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        d.joined_any || d.open_until_unix.is_some_and(|exp| now_unix < exp)
    }

    pub fn internet_wanted(&self) -> bool {
        self.internet_wanted_at(unix_now(), Instant::now())
    }

    pub fn internet_wanted_at(&self, now_unix: u64, now: Instant) -> bool {
        let d = *self
            .deadlines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        d.joined_internet
            || d.internet_until_unix.is_some_and(|exp| now_unix < exp)
            || d.prepared_until.is_some_and(|until| now < until)
    }

    /// Earliest future instant when an invite or `prepare_internet()` window
    /// expires and can change [`Self::open`] or [`Self::internet_wanted`].
    pub fn next_expiry(&self) -> Option<Instant> {
        self.next_expiry_at(SystemTime::now(), Instant::now())
    }

    pub fn next_expiry_at(&self, now_sys: SystemTime, now: Instant) -> Option<Instant> {
        let d = *self
            .deadlines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut earliest: Option<Instant> = None;
        let mut consider = |at: Option<Instant>| {
            if let Some(at) = at.filter(|&t| t > now) {
                earliest = Some(earliest.map_or(at, |cur| cur.min(at)));
            }
        };
        if !d.joined_any {
            consider(
                d.open_until_unix
                    .and_then(|exp| unix_deadline(exp, now_sys, now)),
            );
        }
        if !d.joined_internet {
            consider(d.prepared_until);
            consider(
                d.internet_until_unix
                    .and_then(|exp| unix_deadline(exp, now_sys, now)),
            );
        }
        earliest
    }

    pub fn internet_round(&self) -> u64 {
        self.internet_round.load(Ordering::Acquire)
    }

    pub fn bump_internet_round(&self) {
        self.internet_round.fetch_add(1, Ordering::AcqRel);
    }

    pub async fn mapping_changed(&self) {
        self.mapping_notify.notified().await;
    }

    pub fn notify_mapping_changed(&self) {
        self.mapping_notify.notify_one();
    }
}

/// Console sign-in proof passed across the WebRTC websocket upgrade seam.
pub struct ConsoleSignIn {
    pub client_id: String,
    pub name: String,
    pub peer: SocketAddr,
    pub local: SocketAddr,
    pub this_pc: bool,
    pub signed_in: Box<dyn Fn() -> bool + Send + Sync>,
}

/// Summary of an invite returned to `/api/guests/invite` without exposing the token.
#[derive(Clone, Debug, Serialize)]
pub struct InviteInfo {
    pub id: String,
    pub permission: Permission,
    pub created: u64,
    pub expires: u64,
    pub internet: bool,
}

impl InviteInfo {
    pub fn from_invite(invite: &Invite, internet: bool) -> Self {
        Self {
            id: invite.id.clone(),
            permission: invite.permission,
            created: invite.created,
            expires: invite.expires,
            internet,
        }
    }
}

/// Default WebRTC status JSON when the WebRTC runtime is not installed (e.g. standalone web tests).
pub fn default_status_json(config: &Config) -> Value {
    json!({
        "enabled": false,
        "running": false,
        "discovery": false,
        "port": config.webrtc_port(),
        "media_port": config.webrtc_media_port(),
        "error": Value::Null,
    })
}

/// WebRTC services exposed to `butterpollo-web` without depending on `butterpollo-webrtc`.
pub trait WebRtcWebService: Send + Sync {
    fn as_any(&self) -> &dyn Any;
    fn console_device(&self, h: &Shared, username: &str) -> Result<Client, ErrorCode>;
    fn console_upgrade(
        &self,
        h: Shared,
        upgrade: WebSocketUpgrade,
        sign_in: ConsoleSignIn,
    ) -> Response;
    fn play_upgrade(
        &self,
        h: Shared,
        upgrade: WebSocketUpgrade,
        peer: SocketAddr,
        local: SocketAddr,
        headers: &HeaderMap,
    ) -> Response;
    fn guest_upgrade(
        &self,
        h: Shared,
        upgrade: WebSocketUpgrade,
        peer: SocketAddr,
        local: SocketAddr,
    ) -> Response;
    fn guests_status(&self) -> Value;
    fn guests_prepare_internet(&self);
    fn guests_create(
        &self,
        permission: Permission,
        duration: Duration,
        address: Option<String>,
        internet: bool,
    ) -> Result<(InviteInfo, String), InviteError>;
    fn guests_revoke(&self, id: &str) -> bool;
    fn guests_kick(&self, id: u64) -> bool;
    fn status_json(&self, config: &Config) -> Value;
}

/// Dummy `WebRtcMediaLink` for unit tests inside `butterpollo-stream` that only test
/// `webrtc_links.len() != 0` (`power_action::tests`, `wake::tests`).
#[cfg(test)]
pub(crate) struct DummyMediaLink;

#[cfg(test)]
impl WebRtcMediaLink for DummyMediaLink {
    fn audio_sender(&self) -> Option<Box<dyn WebRtcAudioSink>> {
        None
    }

    fn into_video_sink(self: Box<Self>) -> Box<dyn WebRtcVideoSink> {
        struct DummyVideoSink;
        impl WebRtcVideoSink for DummyVideoSink {
            fn peer(&self) -> Option<SocketAddr> {
                None
            }
            fn simulated_losses(&self) -> Option<u64> {
                None
            }
            fn submit(
                &self,
                _session: &Session,
                _frames: Vec<Encoded>,
                _call_latency: Duration,
                _processing_started: Instant,
                _claims: &mut Claims,
            ) -> Result<()> {
                Ok(())
            }
        }
        Box::new(DummyVideoSink)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guest_network_state_evaluates_invite_and_prepare_deadlines_live() {
        let net = GuestNetworkState::default();
        let now = Instant::now();
        let sys = UNIX_EPOCH + Duration::from_secs(1_000);
        assert!(!net.open_at(1_000));
        assert!(!net.internet_wanted_at(1_000, now));
        assert_eq!(net.next_expiry_at(sys, now), None);

        // Prepared window alone expires automatically without any state mutation.
        let prep_end = now + Duration::from_secs(120);
        net.update(GuestNetDeadlines {
            prepared_until: Some(prep_end),
            ..Default::default()
        });
        assert!(!net.open_at(1_000));
        assert!(net.internet_wanted_at(1_000, now));
        assert_eq!(net.next_expiry_at(sys, now), Some(prep_end));
        assert!(!net.internet_wanted_at(1_000, prep_end));
        assert_eq!(net.next_expiry_at(sys, prep_end), None);

        // Live invite + internet invite both flip closed at expiry unless a guest watches.
        net.update(GuestNetDeadlines {
            open_until_unix: Some(1_060),
            internet_until_unix: Some(1_030),
            ..Default::default()
        });
        assert!(net.open_at(1_029));
        assert!(net.internet_wanted_at(1_029, now));
        assert_eq!(
            net.next_expiry_at(sys, now),
            Some(now + Duration::from_secs(30))
        );
        assert!(net.open_at(1_030));
        assert!(!net.internet_wanted_at(1_030, now));
        assert!(!net.open_at(1_060));
        assert_eq!(
            net.next_expiry_at(UNIX_EPOCH + Duration::from_secs(1_060), now),
            None
        );

        // Joined guests keep open/internet wanted past link expiry with no spurious wakeup.
        net.update(GuestNetDeadlines {
            joined_any: true,
            joined_internet: true,
            open_until_unix: Some(1_060),
            internet_until_unix: Some(1_030),
            ..Default::default()
        });
        assert!(net.open_at(2_000));
        assert!(net.internet_wanted_at(2_000, now));
        assert_eq!(net.next_expiry_at(sys, now), None);
    }
}
