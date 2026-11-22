//! Release cache pins ([`Using`]) and test stall hooks ([`Stall`]) for Neural Rendering.
use super::{Hold, cache_root, cached};
use crate::state::Shared;
use std::path::{Path, PathBuf};
#[cfg(any(test, feature = "test-support"))]
use std::sync::{Condvar, Mutex, PoisonError};

/// A cached release kept while an install copies it ([`super::fetch_using`]).
/// The last one of a release another download replaced meanwhile takes it
/// out of the cache: no fetch hands it out again, and launches use the newest.
pub struct Using {
    pub(super) h: Shared,
    pub(super) folder: PathBuf,
    /// Open until the install's copy is done: the cache and its data root were
    /// checked before the release was read, and this keeps them where they
    /// were checked. Taken before the release is removed: Windows refuses to
    /// delete a folder whose open handle does not share deletion.
    pub(super) held: Option<Hold>,
}

#[cfg(any(test, feature = "test-support"))]
impl Using {
    pub fn for_test(h: Shared, folder: PathBuf, held: Option<Hold>) -> Self {
        Self { h, folder, held }
    }
}

impl Drop for Using {
    fn drop(&mut self) {
        // Released before the release may be removed: a held folder cannot be
        // deleted, and taking the value here drops it now rather than at the
        // end of this call.
        self.held = None;
        let last = {
            let mut state = self.h.neural.lock();
            let Some(count) = state.using.get_mut(&self.folder) else {
                return;
            };
            *count -= 1;
            *count == 0 && state.using.remove(&self.folder).is_some()
        };
        if last && cached(&cache_root(&self.h)).is_some_and(|(newest, _)| newest != self.folder) {
            match std::fs::remove_dir_all(&self.folder) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                // The next download's clearing tries again.
                Err(error) => {
                    tracing::warn!(folder = %self.folder.display(), %error, "Neural Rendering: an older release could not be taken out of the cache")
                }
            }
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
static STALLED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
#[cfg(any(test, feature = "test-support"))]
static UNSTALLED: Condvar = Condvar::new();

/// A folder held as a share that stalls until dropped.
#[cfg(any(test, feature = "test-support"))]
pub struct Stall(PathBuf);

#[cfg(any(test, feature = "test-support"))]
impl Stall {
    pub fn new(folder: &Path) -> Self {
        STALLED.lock().unwrap().push(folder.to_path_buf());
        Self(folder.to_path_buf())
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Drop for Stall {
    fn drop(&mut self) {
        STALLED
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|folder| *folder != self.0);
        UNSTALLED.notify_all();
    }
}

/// Where a look into a game's folder may wait (a network share that stalls).
#[cfg(not(any(test, feature = "test-support")))]
pub(super) fn stalls(_: &Path) {}

#[cfg(any(test, feature = "test-support"))]
pub(super) fn stalls(path: &Path) {
    let mut stalled = STALLED.lock().unwrap_or_else(PoisonError::into_inner);
    while stalled.iter().any(|folder| path.starts_with(folder)) {
        stalled = UNSTALLED
            .wait(stalled)
            .unwrap_or_else(PoisonError::into_inner);
    }
}
