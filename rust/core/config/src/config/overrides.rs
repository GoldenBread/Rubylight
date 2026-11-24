//! The settings a device or an app may override.
use super::catalog_gen::OVERRIDE_ALLOWED;

/// Whether a device or an app may override `key` for its streams. As in
/// Vibepollo, overrides cover stream, input, display and encoder behaviour;
/// identity, network, paths and integrations stay host-wide.
pub fn override_allowed(key: &str) -> bool {
    OVERRIDE_ALLOWED.contains(&key)
}
