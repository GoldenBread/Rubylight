//! Windows physical/virtual display topology, arrangement, recovery, and HDR calibration profiles.
#![cfg(windows)]
#![warn(clippy::undocumented_unsafe_blocks)]
#![warn(clippy::print_stdout, clippy::print_stderr)]

pub(crate) mod desktop;
pub mod display;
pub mod display_arrangement;
pub mod display_recovery;
pub mod hdr_profile;
pub(crate) mod text;
