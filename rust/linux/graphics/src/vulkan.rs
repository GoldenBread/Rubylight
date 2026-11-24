//! Registration and activation of the Vulkan HDR layer. The layer is not available on a Linux host yet: nothing is ever registered, turning it on is an error, and a lease activates nothing.

use anyhow::Result;
use serde_json::{Value, json};

/// Registers the layer for Vulkan applications, or removes the registration. Turning it on
/// is an error on Linux; turning it off succeeds, since nothing is registered.
pub fn register(enabled: bool) -> Result<()> {
    if enabled {
        return Err(crate::unsupported("The Vulkan HDR layer"));
    }
    Ok(())
}
/// Brings the registration in line with the setting: an error while the setting asks for
/// the layer, nothing to do otherwise.
pub fn reconcile(enabled: bool) -> Result<()> {
    if enabled {
        register(enabled)?;
    }
    Ok(())
}
/// The layer's state for the console: not installed, not available and not active on Linux,
/// with no manifest path. `enabled` is the setting as given.
pub fn status(enabled: bool) -> Value {
    json!({"status":true,"installed":false,"enabled":enabled,
        "available":false,"manifest":"","active":false})
}
/// Held by an HDR display session to keep the layer active. On Linux it holds nothing.
pub struct Lease;
impl Lease {
    pub fn acquire() -> Result<Self> {
        Ok(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_layer_is_reported_absent_and_cannot_be_turned_on() {
        for enabled in [false, true] {
            let status = status(enabled);
            assert_eq!(status["enabled"], enabled);
            for key in ["installed", "available", "active"] {
                assert_eq!(status[key], false, "{key}");
            }
        }
        assert!(register(true).is_err() && reconcile(true).is_err());
        assert!(register(false).is_ok() && reconcile(false).is_ok());
    }
}
