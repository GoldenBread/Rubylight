//! Linux display topology, virtual displays, arrangement and recovery: the Linux side of `butterpollo-win-display`, module for module.
//!
//! The host names the Windows crate; on Linux that crate re-exports this one. A module here
//! offers what the host uses of its Windows namesake, under the same names and signatures.
//! What Linux cannot do yet answers honestly: a query reports nothing, an action returns an
//! error that says so.
#![cfg(target_os = "linux")]
#![warn(clippy::undocumented_unsafe_blocks)]
#![warn(clippy::print_stdout, clippy::print_stderr)]

pub mod display;
pub mod display_arrangement;
pub mod display_recovery;
pub mod hdr_profile;

/// The error of an action Linux cannot carry out yet. `what` names the action in the words a
/// log line or the console can show.
pub fn unsupported(what: &str) -> anyhow::Error {
    anyhow::anyhow!("{what} is not available on a Linux host yet")
}
