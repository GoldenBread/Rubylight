//! The external frame limiter on Linux: no limiter provider exists here, so none is usable,
//! a stream's lease holds nothing and caps nothing, and there is nothing to recover.

use anyhow::Result;
use butterpollo_config::config::Config;
use butterpollo_media_policy::framegen::{Policy, Provider, Rate};
use serde_json::{Value as Json, json};
use std::path::Path;

pub mod hooks;

/// The driver settings an older host left behind after a crash, named for the user. There
/// are none on Linux.
pub fn take_stale_nvidia(_directory: &Path) -> Option<String> {
    None
}

/// Put back what a crashed stream left of the limiter. Nothing is left on Linux.
pub fn recover(_directory: &Path) -> Result<()> {
    Ok(())
}

/// Whether the configured limiter can apply a cap now. None can on Linux.
pub fn provider_usable(_config: &Config) -> bool {
    false
}

/// Says once per host run that a virtual display's automatic cap needs a limiter. Says
/// nothing on Linux, where no limiter is looked for.
pub fn warn_automatic_cap_needs_rtss_once(_config: &Config) {}

/// A stream's hold on the frame limiter. It holds nothing on Linux.
pub struct Lease;
impl Lease {
    /// What the console should say when the limiter did not do what the policy asks.
    pub fn warning(&self, _policy: &Policy, _explicit: bool) -> Option<String> {
        None
    }
    /// Take the limiter for a stream; it caps nothing on Linux.
    pub fn acquire(_directory: &Path, _config: &Config, _policy: &Policy) -> Result<Self> {
        Ok(Self)
    }
}

/// The stream's display refreshes at `display` now. There is no cap to follow on Linux.
pub fn follow_display(_display: Option<Rate>) {}

/// Whether a cap follows one stream's display now. None does on Linux.
pub fn following() -> bool {
    false
}

/// Whether the stream reads its display's mode at this live tick: with every timing window,
/// and between windows while a cap follows the display.
pub fn display_read_due(window_due: bool, following: bool) -> bool {
    window_due || following
}

/// The rate games are held to while a stream holds the frame limiter. None on Linux.
pub fn active_rate() -> Option<Rate> {
    None
}

/// The limiter's state for the console.
pub fn status(config: &Config) -> Json {
    let configured = config.get("rtss_install_path", "");
    json!({"status":true,"enabled":config.boolean("frame_limiter_enable",false),
        "configured_provider":Provider::parse(config.get("frame_limiter_provider","auto")).name(),
        "active_provider":"none",
        "rtss_available":false,"configured_path":configured,"path_configured":!configured.is_empty(),
        "resolved_path":"","path_exists":false,"hooks_found":false,
        "rtss_version":Json::Null,"profile_found":false,
        "can_bootstrap_profile":false,"process_running":false,
        "message":"No external frame limiter is available on a Linux host yet."})
}
