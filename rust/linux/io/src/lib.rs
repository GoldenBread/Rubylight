//! Linux audio, input, processes, launchers, networking and system I/O: the Linux side of `butterpollo-win-io`, module for module.
//!
//! The host names the Windows crate; on Linux that crate re-exports this one. A module here
//! offers what the host uses of its Windows namesake, under the same names and signatures.
//! What Linux cannot do yet answers honestly: a query reports nothing, an action returns an
//! error that says so.
#![cfg(target_os = "linux")]
#![warn(clippy::undocumented_unsafe_blocks)]
#![warn(clippy::print_stdout, clippy::print_stderr)]

pub mod audio;
pub mod audio_app_route;
pub mod audio_route;
pub mod child_session;
pub mod clipboard;
pub mod crash;
pub mod files;
pub mod firewall;
pub mod gamescope;
pub mod hip;
pub mod hotkey;
pub mod input;
pub mod ipc;
pub mod keylayout;
pub mod launcher;
pub mod library;
pub mod limiter;
pub mod local_input;
pub mod local_time;
pub mod lossless;
pub mod mic;
pub mod net;
pub mod nucleus;
pub mod playnite;
pub mod power;
pub mod process;
pub mod rtss;
pub mod service;
pub mod session_end;
pub mod shared_folder;
pub mod steam;
pub mod text_focus;
pub mod tray;
pub mod user_folder;
pub mod wake;
pub mod wifi_scan;
pub mod window_list;
pub mod window_placement;

/// The error of an action Linux cannot carry out yet. `what` names the action in the words a
/// log line or the console can show.
pub fn unsupported(what: &str) -> anyhow::Error {
    anyhow::anyhow!("{what} is not available on a Linux host yet")
}
