//! Optional WebRTC server for browser and TV clients (`docs/webrtc-design.md`). The signalling
//! server, discovery and device registry run when enabled; `session` starts sessions on the stream
//! pipeline over the str0m media transport.
#![allow(dead_code)]

pub mod clients;
pub mod discovery;
pub mod guests;
pub mod input;
pub mod media;
pub mod sdp;
pub mod server;
pub mod session;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

use axum::{extract::WebSocketUpgrade, http::HeaderMap, response::Response};
use butterpollo_config::config::Config;
use butterpollo_session::{
    state::Client,
    webrtc::{
        guests::{InviteError, Permission},
        protocol::ErrorCode,
    },
};
use butterpollo_stream::{
    state::Shared,
    stream::Media,
    webrtc_bridge::{ConsoleSignIn, InviteInfo, WebRtcWebService},
};
use serde_json::Value;
use std::{
    any::Any,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

/// Live WebRTC server state attached to [`butterpollo_stream::state::Host`].
#[derive(Clone)]
pub struct WebRtcRuntime {
    pub clients: Arc<clients::Registry>,
    pub guests: Arc<guests::Guests>,
    media: Arc<Mutex<Option<Arc<Media>>>>,
}

impl WebRtcRuntime {
    fn create(h: &Shared, media: Option<Arc<Media>>) -> Self {
        Self {
            clients: Arc::new(clients::Registry::with_changed(
                h.webrtc_clients_epoch.clone(),
            )),
            guests: Arc::new(guests::Guests::with_network(h.guest_net.clone())),
            media: Arc::new(Mutex::new(media)),
        }
    }

    /// Returns the [`WebRtcRuntime`] installed on `h`, creating and attaching a default one on
    /// first use (used by standalone tests and helpers that didn't call [`Self::install`]).
    pub fn for_host(h: &Shared) -> Self {
        let svc = h.get_or_init_webrtc_service(|| Arc::new(Self::create(h, None)));
        svc.as_any()
            .downcast_ref::<Self>()
            .expect("host webrtc_service must be WebRtcRuntime")
            .clone()
    }

    /// Installs (or updates the media pipeline on) a [`WebRtcRuntime`] attached to `h`.
    pub fn install(h: &Shared, media: Option<Arc<Media>>) -> Self {
        let rt = Self::for_host(h);
        if let Some(media) = media {
            *rt.media.lock().unwrap() = Some(media);
        }
        rt
    }
}

impl WebRtcWebService for WebRtcRuntime {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn console_device(&self, h: &Shared, username: &str) -> Result<Client, ErrorCode> {
        self.clients.console_device(h, username)
    }

    fn console_upgrade(
        &self,
        h: Shared,
        upgrade: WebSocketUpgrade,
        sign_in: ConsoleSignIn,
    ) -> Response {
        let media = self.media.lock().unwrap().clone();
        server::Console::new(media).upgrade(h, upgrade, sign_in)
    }

    fn play_upgrade(
        &self,
        h: Shared,
        upgrade: WebSocketUpgrade,
        peer: SocketAddr,
        local: SocketAddr,
        headers: &HeaderMap,
    ) -> Response {
        let media = self.media.lock().unwrap().clone();
        server::Console::new(media).play(h, upgrade, peer, local, headers)
    }

    fn guest_upgrade(
        &self,
        h: Shared,
        upgrade: WebSocketUpgrade,
        peer: SocketAddr,
        local: SocketAddr,
    ) -> Response {
        guests::upgrade(h, self.guests.clone(), upgrade, peer, local)
    }

    fn guests_status(&self) -> Value {
        serde_json::to_value(self.guests.status()).unwrap_or_default()
    }

    fn guests_prepare_internet(&self) {
        let _ = self.guests.prepare_internet();
    }

    fn guests_create(
        &self,
        permission: Permission,
        duration: Duration,
        address: Option<String>,
        internet: bool,
    ) -> Result<(InviteInfo, String), InviteError> {
        let (invite, token) = self
            .guests
            .create_for(permission, duration, address, internet)?;
        Ok((InviteInfo::from_invite(&invite, internet), token))
    }

    fn guests_revoke(&self, id: &str) -> bool {
        self.guests.revoke(id)
    }

    fn guests_kick(&self, id: u64) -> bool {
        self.guests.kick(id)
    }

    fn status_json(&self, config: &Config) -> Value {
        self.clients.status_json(config)
    }
}

#[cfg(any(test, feature = "test-support"))]
impl WebRtcWebService for server::Console {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn console_device(&self, h: &Shared, username: &str) -> Result<Client, ErrorCode> {
        WebRtcRuntime::for_host(h).console_device(h, username)
    }

    fn console_upgrade(
        &self,
        h: Shared,
        upgrade: WebSocketUpgrade,
        sign_in: ConsoleSignIn,
    ) -> Response {
        self.upgrade(h, upgrade, sign_in)
    }

    fn play_upgrade(
        &self,
        h: Shared,
        upgrade: WebSocketUpgrade,
        peer: SocketAddr,
        local: SocketAddr,
        headers: &HeaderMap,
    ) -> Response {
        self.play(h, upgrade, peer, local, headers)
    }

    fn guest_upgrade(
        &self,
        h: Shared,
        upgrade: WebSocketUpgrade,
        peer: SocketAddr,
        local: SocketAddr,
    ) -> Response {
        WebRtcRuntime::for_host(&h).guest_upgrade(h, upgrade, peer, local)
    }

    fn guests_status(&self) -> Value {
        Value::Null
    }

    fn guests_prepare_internet(&self) {}

    fn guests_create(
        &self,
        _permission: Permission,
        _duration: Duration,
        _address: Option<String>,
        _internet: bool,
    ) -> Result<(InviteInfo, String), InviteError> {
        Err(InviteError::Full)
    }

    fn guests_revoke(&self, _id: &str) -> bool {
        false
    }

    fn guests_kick(&self, _id: u64) -> bool {
        false
    }

    fn status_json(&self, _config: &Config) -> Value {
        Value::Null
    }
}
