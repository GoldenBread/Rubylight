//! The lock the restore of the removed child-session setup takes across processes. On Linux there is no child-session setup to restore, so the lock is taken at once and holds nothing.

use anyhow::Result;
use std::{marker::PhantomData, time::Duration};

/// The name the lock has on Windows.
pub const SETUP_LOCK: &str = r"Global\Rubylight.ChildSessionSetup";
/// How long the host waits for [`setup_lock`].
pub const SETUP_LOCK_TIMEOUT: Duration = Duration::from_secs(120);

/// A named lock this thread holds until it is dropped. It never leaves the thread that took it.
pub struct NamedLock {
    _thread: PhantomData<*const ()>,
}
/// Ends what the value holds, at the point the host drops it. It holds no lock on Linux, so there is nothing to release.
impl Drop for NamedLock {
    fn drop(&mut self) {}
}
impl NamedLock {
    /// Take the lock `name`, waiting up to `timeout`. Nothing else on Linux takes it, so it is
    /// always free.
    pub fn acquire(name: &str, timeout: Duration) -> Result<Option<Self>> {
        let _ = (name, timeout);
        Ok(Some(Self {
            _thread: PhantomData,
        }))
    }
    /// Whether the last holder ended without letting go of it. Never.
    pub fn abandoned(&self) -> bool {
        false
    }
}

/// [`SETUP_LOCK`], for up to `timeout`.
pub fn setup_lock(timeout: Duration) -> Result<NamedLock> {
    let lock = NamedLock::acquire(SETUP_LOCK, timeout)?;
    lock.ok_or_else(|| anyhow::anyhow!("the child-session lock is held elsewhere"))
}
