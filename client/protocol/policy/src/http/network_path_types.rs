//! The route to the host as the host sees it (`/api/network-path`), read as
//! NvHTTP.getNetworkPath and NetworkPathInfo.java do: only the current peer's route category,
//! no peer list or addresses.

use super::{Error, HostApiError};
use serde_json::Value;

/// The largest reply read.
pub const MAX_NETWORK_PATH_BYTES: usize = 512;

/// How this client reaches the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NetworkPath {
    Direct,
    /// Through a relay server (Tailscale DERP and the like).
    Relay,
    /// Through another peer of the overlay network.
    PeerRelay,
    /// A category this client does not know yet.
    Unknown,
}

fn invalid() -> HostApiError {
    HostApiError::Host(Error::Malformed("Invalid route response".into()))
}

impl NetworkPath {
    /// A reply body. A version other than 1 is [`HostApiError::UnsupportedVersion`].
    pub fn parse(json: &str) -> Result<Self, HostApiError> {
        if json.len() > MAX_NETWORK_PATH_BYTES {
            return Err(invalid());
        }
        let root: Value = serde_json::from_str(json).map_err(|_| invalid())?;
        let root = root.as_object().ok_or_else(invalid)?;
        // Any number equal to 1, as org.json compares it; "1" as a string is not a version.
        let version = root
            .get("version")
            .and_then(Value::as_f64)
            .ok_or_else(invalid)?;
        if version != 1.0 {
            return Err(HostApiError::UnsupportedVersion);
        }
        // org.json reads any present value as a string: only the known names map to a route.
        Ok(match root.get("kind").ok_or_else(invalid)?.as_str() {
            Some("direct") => NetworkPath::Direct,
            Some("relay") => NetworkPath::Relay,
            Some("peer_relay") => NetworkPath::PeerRelay,
            _ => NetworkPath::Unknown,
        })
    }
}
