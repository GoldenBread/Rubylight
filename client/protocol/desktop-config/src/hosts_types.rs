//! Wake callback type shared across config, UI, media and launcher threads.

use std::sync::Arc;

/// Wakes the UI thread after an event was queued.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// Merges a host's native API capabilities into its classic serverinfo capabilities.
pub fn merge_native(
    info: &mut nightjar_policy::http::ServerInfo,
    native: Option<&nightjar_policy::http::NativeCapabilities>,
) {
    if let Some(native) = native {
        info.capabilities = info.capabilities.with_native(native);
    }
}

/// The name the host lists this client under.
pub fn device_name() -> String {
    let name = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .filter(|n| !n.is_empty())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|n| n.trim().to_owned())
                .filter(|n| !n.is_empty())
        });
    match name {
        Some(name) => format!("Nightjar ({name})"),
        None => "Nightjar".into(),
    }
}
