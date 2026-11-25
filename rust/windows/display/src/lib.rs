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

/// Acquire a global display mutex, recovering its inner guard if a previous holder panicked
/// so one fault does not poison every later stream.
pub(crate) fn lock_unpoisoned<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
