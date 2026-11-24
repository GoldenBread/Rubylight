//! Capture copies and colour conversion on a GPU compute queue. On Linux only the settings the stream hands to an encoder and Frame Warp's pass descriptions are here; there is no compute queue yet, so the conversion is never enabled and no pass runs.

pub mod frame_warp;

/// Whether captures and the encoders use the compute queue. Linux has none, so never,
/// whatever `gpu_compute_conversion` says.
pub fn enabled(_config: &butterpollo_config::config::Config) -> bool {
    false
}

/// What the encoder focus passes use for the next conversions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FocusSettings {
    pub mode: butterpollo_media_policy::encoder_focus::Mode,
    pub levels: butterpollo_media_policy::encoder_focus::Levels,
    /// The cursor in target pixels, while it is in the picture.
    pub cursor: Option<(i32, i32)>,
}
