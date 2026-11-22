//! Peer address, connection, and HTTP reply helpers for guest portal tests.
use axum::http::{HeaderMap, StatusCode};
use butterpollo_stream::{
    state::{Shared, test_support::Fixture},
    tls::Connection,
};
use butterpollo_webrtc::WebRtcRuntime;
use serde_json::Value;

pub(super) fn with_webrtc(f: &Fixture) -> Shared {
    WebRtcRuntime::for_host(&f.host);
    f.host.clone()
}

/// A peer on the LAN and one on the internet.
pub(super) const LAN: &str = "192.168.4.20:50000";
pub(super) const WAN: &str = "203.0.113.7:50000";

/// The PC's Tailscale address, where Tailscale peers arrive.
pub(super) const TAILNET_HOST: &str = "100.90.1.2:47991";

/// A connection from `peer` to the PC's address it reaches: the Tailscale one for a 100.64/10
/// peer, the LAN one otherwise.
pub(super) fn connection(peer: &str) -> Connection {
    let tailscale = peer.starts_with("100.");
    connection_to(
        peer,
        if tailscale {
            TAILNET_HOST
        } else {
            "192.168.4.69:47990"
        },
    )
}

pub(super) fn connection_to(peer: &str, local: &str) -> Connection {
    Connection {
        peer: peer.parse().unwrap(),
        local: local.parse().unwrap(),
        certificate: None,
        tls: true,
        second_screen: false,
    }
}

pub(super) struct Reply {
    pub(super) status: StatusCode,
    pub(super) headers: HeaderMap,
    pub(super) body: Vec<u8>,
}

impl Reply {
    pub(super) fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_default()
    }
    pub(super) fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
    pub(super) fn error(&self) -> String {
        self.json()["error"].as_str().unwrap_or_default().to_owned()
    }
}
