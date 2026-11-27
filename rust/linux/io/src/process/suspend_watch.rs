//! The Quick Suspend watchdog and the recovery of a journal a host left behind. A Linux host
//! suspends nothing yet ([`super::suspend`]), so there is no helper to start, and no journal
//! of its own to recover.
use anyhow::{Result, bail};
use butterpollo_session::quick_suspend::{Journal, Recovery};
use std::path::Path;

/// The journal's lock: held while it is read and changed. Only this host writes the journal
/// on Linux, and only to clear it.
pub struct JournalLock(());

/// Take the lock of the journal in `profile`.
pub fn lock(_profile: &Path) -> Result<JournalLock> {
    Ok(JournalLock(()))
}

/// This process's identity: id and start time.
pub fn me() -> (u32, u64) {
    let pid = std::process::id();
    (pid, super::creation_time(pid))
}

/// Start this host's helper for `profile`. There is none on Linux, and a suspend that cannot
/// start it does not suspend.
pub fn watch(_profile: &Path) -> Result<()> {
    bail!("Quick Suspend is not available on a Linux host yet")
}

/// Tell this host's helper that nothing is suspended any more. There is none on Linux.
pub fn release() {}

/// The helper's work. A Linux host starts no helper, so there is nothing to wait for.
pub fn wait_and_recover(_host_pid: u32, _profile: &Path) -> Result<Option<Recovery>> {
    Ok(None)
}

/// At a host's start: resume what a journal of a host that no longer runs names. A Linux
/// host writes none.
pub fn recover_at_start(_profile: &Path) -> Result<Option<(Journal, Recovery)>> {
    Ok(None)
}
