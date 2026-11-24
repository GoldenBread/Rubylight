//! Windows physical/virtual display topology, arrangement, recovery, and HDR calibration profiles.
#![warn(clippy::undocumented_unsafe_blocks)]
#![warn(clippy::print_stdout, clippy::print_stderr)]

#[cfg(windows)]
pub(crate) mod desktop;
#[cfg(windows)]
pub mod display;
#[cfg(windows)]
pub mod display_arrangement;
#[cfg(windows)]
pub mod display_recovery;
#[cfg(windows)]
pub mod hdr_profile;
#[cfg(windows)]
pub(crate) mod text;

#[cfg(windows)]
/// Acquire a global display mutex, recovering its inner guard if a previous holder panicked
/// so one fault does not poison every later stream.
pub(crate) fn lock_unpoisoned<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

// Linux: the same modules and names, from `rust/linux/display`.
#[cfg(target_os = "linux")]
pub use butterpollo_linux_display::*;
