//! Windows audio, input, process/service lifecycle, launchers, networking, and system I/O.
#![warn(clippy::undocumented_unsafe_blocks)]
#![warn(clippy::print_stdout, clippy::print_stderr)]

#[cfg(windows)]
pub mod audio;
#[cfg(windows)]
pub mod audio_app_route;
#[cfg(windows)]
pub mod audio_route;
#[cfg(windows)]
pub mod background_write;
#[cfg(windows)]
pub mod child_session;
#[cfg(windows)]
pub mod clipboard;
#[cfg(windows)]
pub mod crash;
#[cfg(windows)]
pub mod files;
#[cfg(windows)]
pub mod firewall;
#[cfg(windows)]
pub mod hip;
#[cfg(windows)]
pub mod hotkey;
#[cfg(windows)]
pub mod input;
#[cfg(windows)]
pub mod ipc;
#[cfg(windows)]
pub mod keylayout;
#[cfg(windows)]
pub mod launcher;
#[cfg(windows)]
pub mod library;
#[cfg(windows)]
pub mod limiter;
#[cfg(windows)]
pub mod local_input;
#[cfg(windows)]
pub mod local_time;
#[cfg(windows)]
pub mod lossless;
#[cfg(windows)]
pub mod mic;
#[cfg(windows)]
pub mod net;
#[cfg(windows)]
pub mod net_adapter;
#[cfg(windows)]
pub mod nucleus;
#[cfg(windows)]
pub(crate) mod os_helpers;
#[cfg(windows)]
pub mod playnite;
#[cfg(windows)]
pub mod power;
#[cfg(windows)]
pub mod process;
#[cfg(windows)]
pub mod rtss;
#[cfg(windows)]
pub mod service;
#[cfg(windows)]
pub mod session_end;
#[cfg(windows)]
pub mod shared_folder;
#[cfg(windows)]
pub mod steam;
#[cfg(windows)]
mod text;
#[cfg(windows)]
pub mod text_focus;
#[cfg(windows)]
pub mod tray;
#[cfg(windows)]
pub mod user_folder;
#[cfg(windows)]
pub mod wake;
#[cfg(windows)]
pub mod wifi_scan;
#[cfg(windows)]
pub mod window_list;
#[cfg(windows)]
pub mod window_placement;
#[cfg(windows)]
pub mod wmi;

// Linux: the same modules and names, from `rust/linux/io`.
#[cfg(target_os = "linux")]
pub use butterpollo_linux_io::*;
