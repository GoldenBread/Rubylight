//! Windows capture, Direct3D colour conversion, AMF/NVENC/PyroWave encoding, and GPU telemetry.
#![cfg(windows)]
#![warn(clippy::undocumented_unsafe_blocks)]
#![warn(clippy::print_stdout, clippy::print_stderr)]

pub(crate) mod adlx_system;
pub mod amf;
pub mod capture;
pub mod codec_probe;
pub mod color;
pub mod compute;
mod cursor;
pub mod device_loss;
pub mod encoder;
pub mod ffmpeg;
pub mod foreground;
pub mod frame_pool;
mod gpu_color;
pub mod gpu_priority;
pub mod gpu_probe;
pub mod image;
pub mod nvenc;
pub mod nvidia_enhance;
pub mod nvidia_profile;
pub mod nvidia_settings;
pub mod overlay;
pub mod present_timing;
pub(crate) mod process_info;
pub mod pyrowave;
pub mod radeon;
pub mod rtx_profiles;
pub mod rtx_video;
pub mod telemetry;
pub(crate) mod text;
pub mod timing;
pub mod truehdr;
pub mod user_helper;
pub mod vulkan;
