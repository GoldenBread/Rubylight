//! The live-monitor registry: the one strong owner of every [`Monitor`], plus each monitor
//! id's mode history.
//!
//! From punktfunk's pf-vdisplay `registry.rs` by Enrico Bühler (unom), MIT OR Apache-2.0.
//!
//! A process `static` because the IddCx monitor and mode DDIs receive only an IddCx handle.
//! With one devnode and `ProcessSharingDisabled`, WUDFHost dies with the device, so this is
//! device-scoped already. The lock covers lookups, insert and remove only: a removal hands
//! the `Arc` back and the caller tears it down with the lock released. Lock order is
//! `REGISTRY -> Monitor.*`; workers never take this lock.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use rubylight_vdisplay_proto::modes::{self, IdError, Mode};

use crate::monitor::Monitor;

struct Registry {
    monitors: Vec<Arc<Monitor>>,
    /// The last list of a departed monitor per id, unioned into the next monitor with that
    /// id: Windows pins the settable modes at arrival, so this is what lets a return to an
    /// earlier size be an in-place mode set instead of a hotplug.
    mode_history: Vec<(u32, Vec<Mode>)>,
}

impl Registry {
    const fn new() -> Self {
        Self {
            monitors: Vec::new(),
            mode_history: Vec::new(),
        }
    }

    fn insert(
        &mut self,
        owner: u32,
        session_id: u64,
        preferred_id: u32,
        exact: bool,
        requested_mhz: u32,
        mut modes: Vec<Mode>,
    ) -> Result<Arc<Monitor>, IdError> {
        let live: Vec<(u32, bool)> = self
            .monitors
            .iter()
            .map(|m| (m.id, m.owner == owner))
            .collect();
        let id = modes::choose_id(&live, preferred_id, exact)?;
        if let Some((_, prev)) = self.mode_history.iter().find(|(i, _)| *i == id) {
            modes::union_history(&mut modes, prev, requested_mhz);
        }
        let monitor = Arc::new(Monitor::pending(owner, id, session_id, modes));
        self.monitors.push(monitor.clone());
        Ok(monitor)
    }

    fn remove_entry(&mut self, target: &Arc<Monitor>) -> Option<Arc<Monitor>> {
        let pos = self.monitors.iter().position(|m| Arc::ptr_eq(m, target))?;
        Some(self.monitors.remove(pos))
    }
}

static REGISTRY: Mutex<Registry> = Mutex::new(Registry::new());

/// Lock `m`, recovering the guard on poison (unreachable with `panic = "abort"`).
pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn registry() -> MutexGuard<'static, Registry> {
    lock(&REGISTRY)
}

/// The first live monitor matching `pred`.
pub fn find(pred: impl Fn(&Monitor) -> bool) -> Option<Arc<Monitor>> {
    registry().monitors.iter().find(|m| pred(m)).cloned()
}

/// Snapshot for power transitions; workers are joined after releasing the registry lock.
pub fn all() -> Vec<Arc<Monitor>> {
    registry().monitors.clone()
}

/// Unlink every monitor matching `pred` for the caller to tear down lock-free.
pub fn remove(pred: impl Fn(&Monitor) -> bool) -> Vec<Arc<Monitor>> {
    let mut reg = registry();
    let (gone, keep): (Vec<_>, Vec<_>) = reg.monitors.drain(..).partition(|m| pred(m));
    reg.monitors = keep;
    for m in &gone {
        record_modes(&mut reg, m);
    }
    gone
}

fn record_modes(reg: &mut Registry, monitor: &Monitor) {
    let modes = monitor.modes();
    if let Some(slot) = reg.mode_history.iter_mut().find(|(i, _)| *i == monitor.id) {
        slot.1 = modes;
    } else {
        reg.mode_history.push((monitor.id, modes));
    }
}

/// Unlink the exact registry entry `target` by `Arc::ptr_eq`, leaving any concurrent
/// replacement that reused the same monitor id untouched.
pub fn remove_entry(target: &Arc<Monitor>) -> Option<Arc<Monitor>> {
    registry().remove_entry(target)
}

/// Register `owner`'s pending monitor for `session_id` under the id [`modes::choose_id`]
/// picks, with the id's history unioned into `modes` at no rate above `requested_mhz`.
/// Choosing and inserting share one guard, so two concurrent ADDs never take the same id.
pub fn insert(
    owner: u32,
    session_id: u64,
    preferred_id: u32,
    exact: bool,
    requested_mhz: u32,
    modes: Vec<Mode>,
) -> Result<Arc<Monitor>, IdError> {
    registry().insert(owner, session_id, preferred_id, exact, requested_mhz, modes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remove_entry_matches_arc_ptr_eq_and_preserves_concurrent_same_id_monitor() {
        let mut reg = Registry::new();
        let first = reg.insert(10, 1, 1, true, 60_000, Vec::new()).unwrap();
        assert_eq!(first.id, 1);

        // Another path unlinks `first` before its create/arrival fails, freeing id 1.
        let removed = reg.remove_entry(&first).expect("first monitor removed");
        assert!(Arc::ptr_eq(&removed, &first));

        // A concurrent ADD reuses connector id 1 with a distinct Monitor Arc.
        let second = reg.insert(20, 2, 1, true, 60_000, Vec::new()).unwrap();
        assert_eq!(second.id, 1);
        assert!(!Arc::ptr_eq(&first, &second));

        // The first ADD's error path must NOT unlink `second`.
        assert!(reg.remove_entry(&first).is_none());
        assert_eq!(reg.monitors.len(), 1);
        assert!(Arc::ptr_eq(&reg.monitors[0], &second));

        // Removing `second` by its own Arc succeeds.
        let removed_second = reg.remove_entry(&second).expect("second monitor removed");
        assert!(Arc::ptr_eq(&removed_second, &second));
        assert!(reg.monitors.is_empty());
    }
}
