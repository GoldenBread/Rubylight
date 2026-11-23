//! Typed host address slots (`(address, port)`) and `"<address> <port>"` pinned-address
//! formatting/parsing matching Android `ComputerDetails.AddressTuple` and `PinnedAddresses`.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A host address paired with its HTTP port (distinct per slot: local, remote, manual, IPv6,
/// or pinned).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EndpointSlot {
    pub address: String,
    pub port: u16,
}

impl EndpointSlot {
    /// Builds an [`EndpointSlot`] if `address` is non-empty and `port` is in `1..=65535`,
    /// stripping surrounding `[...]` brackets on IPv6 literals.
    pub fn new(address: &str, port: u16) -> Option<Self> {
        if port == 0 {
            return None;
        }
        let trimmed = address.trim();
        if trimmed.is_empty() {
            return None;
        }
        let unbracketed = trimmed
            .strip_prefix('[')
            .and_then(|s| s.strip_suffix(']'))
            .unwrap_or(trimmed);
        if unbracketed.is_empty() {
            return None;
        }
        Some(Self {
            address: unbracketed.to_owned(),
            port,
        })
    }
}

impl fmt::Display for EndpointSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.address.contains(':') {
            write!(f, "[{}]:{}", self.address, self.port)
        } else {
            write!(f, "{}:{}", self.address, self.port)
        }
    }
}

/// Pure `"<address> <port>"` codec matching Android's `PinnedAddresses` preference storage.
pub struct PinnedAddress;

impl PinnedAddress {
    pub fn parse(stored: &str) -> Option<EndpointSlot> {
        let split = stored.rfind(' ')?;
        if split == 0 {
            return None;
        }
        let addr = &stored[..split];
        let port: u16 = stored[split + 1..].parse().ok()?;
        EndpointSlot::new(addr, port)
    }

    pub fn format(slot: &EndpointSlot) -> String {
        format!("{} {}", slot.address, slot.port)
    }
}
