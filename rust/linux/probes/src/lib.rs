//! Capture and hardware encoding on Linux, measured for the Linux host spike
//! (docs/linux-host-spike.md). Nothing here is used by the host.
#![cfg(target_os = "linux")]
pub mod args;
pub mod hevc;
pub mod hevc_headers;
pub mod pattern;
pub mod stats;
pub mod va;
pub mod vpp;
