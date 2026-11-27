//! Suspending and resuming one process by identity, for Quick Suspend. Not available on a
//! Linux host yet: nothing is suspended, so nothing is ever resumed, and the host does not
//! offer the feature (`extensions::native_for_platform`).
use butterpollo_session::quick_suspend::{self, Resumed};

/// Why a process could not be suspended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SuspendError {
    /// It exited, or its id now belongs to another process.
    Gone,
    /// The system protects it.
    Protected,
    /// Any other failure, in words.
    Os(String),
}

impl std::fmt::Display for SuspendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Gone => f.write_str("the process exited"),
            Self::Protected => f.write_str("the system protects the process"),
            Self::Os(error) => f.write_str(error),
        }
    }
}

/// What a suspend answers on Linux.
const NOT_YET: &str = "Quick Suspend is not available on a Linux host yet";

/// Whether the process `pid` started at `started` runs protected. None does on Linux.
pub fn protected(_pid: u32, _started: u64) -> bool {
    false
}

/// Whether a process runs as the one started at `started`.
pub fn alive(pid: u32, started: u64) -> bool {
    started != 0 && super::creation_time(pid) == started
}

/// Suspend the process `pid` started at `started`: refused on Linux.
pub fn suspend(_pid: u32, _started: u64) -> Result<(), SuspendError> {
    Err(SuspendError::Os(NOT_YET.into()))
}

/// Resume the process `pid` started at `started` once. A Linux host suspended none, so
/// there is nothing to resume.
pub fn resume(_pid: u32, _started: u64) -> Result<Resumed, String> {
    Ok(Resumed::Gone)
}

/// [`resume`] for [`quick_suspend::recover`].
pub struct Thaw;

impl quick_suspend::Thaw for Thaw {
    fn resume(&mut self, pid: u32, started: u64) -> Result<Resumed, String> {
        resume(pid, started)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_suspended_and_this_process_is_alive() {
        let me = std::process::id();
        assert!(matches!(suspend(me, 1), Err(SuspendError::Os(_))));
        assert_eq!(resume(me, 1), Ok(Resumed::Gone));
        assert!(alive(me, super::super::creation_time(me)));
        assert!(!alive(me, 0));
        assert!(!protected(me, 1));
    }
}
