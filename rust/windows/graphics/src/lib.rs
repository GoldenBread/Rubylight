//! Windows capture, Direct3D colour conversion, AMF/NVENC/PyroWave encoding, and GPU telemetry.
#![warn(clippy::undocumented_unsafe_blocks)]
#![warn(clippy::print_stdout, clippy::print_stderr)]

#[cfg(windows)]
pub(crate) mod adlx_system;
#[cfg(windows)]
pub mod amf;
#[cfg(windows)]
pub mod capture;
#[cfg(windows)]
pub mod codec_probe;
#[cfg(windows)]
pub mod color;
#[cfg(windows)]
pub mod compute;
#[cfg(windows)]
mod cursor;
#[cfg(windows)]
pub mod device_loss;
#[cfg(windows)]
pub mod encoder;
#[cfg(windows)]
pub mod ffmpeg;
#[cfg(windows)]
pub mod foreground;
#[cfg(windows)]
pub mod frame_pool;
#[cfg(windows)]
mod gpu_color;
#[cfg(windows)]
pub mod gpu_priority;
#[cfg(windows)]
pub mod gpu_probe;
#[cfg(windows)]
pub mod image;
#[cfg(windows)]
pub mod nvenc;
#[cfg(windows)]
pub mod nvidia_enhance;
#[cfg(windows)]
pub mod nvidia_profile;
#[cfg(windows)]
pub mod nvidia_settings;
#[cfg(windows)]
pub mod overlay;
#[cfg(windows)]
pub mod present_timing;
#[cfg(windows)]
pub(crate) mod process_info;
#[cfg(windows)]
pub mod pyrowave;
#[cfg(windows)]
pub mod radeon;
#[cfg(windows)]
pub mod rtx_profiles;
#[cfg(windows)]
pub mod rtx_video;
#[cfg(windows)]
pub mod telemetry;
#[cfg(windows)]
pub(crate) mod text;
#[cfg(windows)]
pub mod timing;
#[cfg(windows)]
pub mod truehdr;
#[cfg(windows)]
pub mod user_helper;
#[cfg(windows)]
pub mod vulkan;

// Linux: the same modules and names, from `rust/linux/graphics`.
#[cfg(target_os = "linux")]
pub use butterpollo_linux_graphics::*;
