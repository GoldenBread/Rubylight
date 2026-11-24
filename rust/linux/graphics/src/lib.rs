//! Linux capture, colour conversion, hardware encoding and GPU telemetry: the Linux side of `butterpollo-win-graphics`, module for module.
//!
//! The host names the Windows crate; on Linux that crate re-exports this one. A module here
//! offers what the host uses of its Windows namesake, under the same names and signatures.
//! What Linux cannot do yet answers honestly: a query reports nothing, an action returns an
//! error that says so.
#![cfg(target_os = "linux")]
#![warn(clippy::undocumented_unsafe_blocks)]
#![warn(clippy::print_stdout, clippy::print_stderr)]

pub mod amf;
pub mod capture;
pub mod codec_probe;
pub mod compute;
pub mod device_loss;
pub mod encoder;
pub mod encoder_clock;
pub mod ffmpeg;
pub mod foreground;
pub mod frame_pool;
pub mod gpu_probe;
pub mod image;
pub mod nvidia_enhance;
pub mod nvidia_profile;
pub mod overlay;
pub mod packed444;
pub mod present_timing;
pub mod pyrowave;
pub mod radeon;
pub mod rtx_profiles;
pub mod rtx_video;
pub mod telemetry;
pub mod timing;
pub mod truehdr;
pub mod user_helper;
pub mod vaapi;
pub mod vulkan;

/// The error of an action Linux cannot carry out yet. `what` names the action in the words a
/// log line or the console can show.
pub fn unsupported(what: &str) -> anyhow::Error {
    anyhow::anyhow!("{what} is not available on a Linux host yet")
}
