//! The host's NVIDIA driver profile, which keeps the encoder's GPU at full clocks under light load. Linux has no such driver profile to write: applying the setting does nothing.

/// Applies `nvenc_latency_over_power` to the driver profile. Nothing to apply on Linux.
pub fn apply_configured(_config: &butterpollo_config::config::Config) {
    tracing::debug!("no NVIDIA driver profile on Linux; the NVIDIA power profile is not set");
}
