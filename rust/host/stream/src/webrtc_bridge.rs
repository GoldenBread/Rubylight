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
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
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

/// Shared network/port-mapping state for browser guests read by `network` and `web`.
#[derive(Default)]
pub struct GuestNetworkState {
    open: AtomicBool,
    internet_wanted: AtomicBool,
    internet_round: AtomicU64,
    mapping_notify: Notify,
}

impl GuestNetworkState {
    pub fn open(&self) -> bool {
        self.open.load(Ordering::Acquire)
    }

    pub fn set_open(&self, open: bool) {
        self.open.store(open, Ordering::Release);
    }

    pub fn internet_wanted(&self) -> bool {
        self.internet_wanted.load(Ordering::Acquire)
    }

    pub fn set_internet_wanted(&self, wanted: bool) {
        self.internet_wanted.store(wanted, Ordering::Release);
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
