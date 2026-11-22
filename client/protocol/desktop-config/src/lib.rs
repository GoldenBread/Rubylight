//! Nightjar desktop configuration, localized UI strings, keyboard/gamepad bindings,
//! PIN lock policy and pure state/math types shared across the desktop crates without
//! pulling `egui`, `sdl3-sys`, `windows`, `ring` or native media SDKs.

pub const VERSION: &str = env!("NIGHTJAR_VERSION");

pub mod app_types;
pub mod config;
pub mod gamepad_types;
pub mod hosts_types;
pub mod latency_bench_state;
pub mod network_types;
pub mod pads_view;
pub mod pin_lock;
pub mod power_types;
pub mod shortcuts;
pub mod stream_types;
pub mod strings;
pub mod version_name;
pub mod window_place_types;
pub mod wol_types;

use std::path::{Path, PathBuf};

/// The platform Downloads folder.
pub fn downloads_dir() -> Option<PathBuf> {
    directories::UserDirs::new().and_then(|dirs| dirs.download_dir().map(Path::to_path_buf))
}

static INTERNET_HOOK: std::sync::OnceLock<fn(bool)> = std::sync::OnceLock::new();

/// Registers the tunnel toggle invoked whenever settings are loaded or saved.
pub fn register_internet_hook(hook: fn(bool)) {
    let _ = INTERNET_HOOK.set(hook);
}

pub(crate) fn notify_internet_enabled(on: bool) {
    if let Some(hook) = INTERNET_HOOK.get() {
        hook(on);
    }
}

impl From<config::HostDisplay> for nightjar_policy::http::HostDisplay {
    fn from(value: config::HostDisplay) -> Self {
        match value {
            config::HostDisplay::HostDefault => Self::HostDefault,
            config::HostDisplay::Exclusive => Self::Exclusive,
            config::HostDisplay::ExtendedPrimary => Self::ExtendedPrimary,
            config::HostDisplay::Extended => Self::Extended,
            config::HostDisplay::ExtendedIsolated => Self::ExtendedIsolated,
            config::HostDisplay::ExtendedPrimaryIsolated => Self::ExtendedPrimaryIsolated,
            config::HostDisplay::Physical => Self::Physical,
        }
    }
}
