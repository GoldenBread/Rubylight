//! Lock-guarded list with a relaxed `AtomicBool` fast path for empty checks.

use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

/// Types accepted by `pipeline::lock`.
pub trait Lockable {
    type Guard<'a>
    where
        Self: 'a;

    fn lock_inner(&self) -> Self::Guard<'_>;
}

impl<T> Lockable for Mutex<T> {
    type Guard<'a>
        = MutexGuard<'a, T>
    where
        Self: 'a;

    fn lock_inner(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(|p| p.into_inner())
    }
}

impl<L: Lockable> Lockable for Arc<L> {
    type Guard<'a>
        = L::Guard<'a>
    where
        Self: 'a;

    fn lock_inner(&self) -> L::Guard<'_> {
        (**self).lock_inner()
    }
}

impl<L: Lockable + ?Sized> Lockable for &L {
    type Guard<'a>
        = L::Guard<'a>
    where
        Self: 'a;

    fn lock_inner(&self) -> L::Guard<'_> {
        (**self).lock_inner()
    }
}

/// `Mutex<Vec<T>>` with a relaxed `AtomicBool` guard so hot audio callbacks skip locking when empty.
pub struct GuardedList<T> {
    inner: Mutex<Vec<T>>,
    non_empty: AtomicBool,
}

impl<T> GuardedList<T> {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Vec::new()),
            non_empty: AtomicBool::new(false),
        }
    }

    /// Relaxed empty check; returns `false` when the list is empty without acquiring the mutex.
    pub fn has_any(&self) -> bool {
        self.non_empty.load(Ordering::Relaxed)
    }
}

impl<T> Default for GuardedList<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Guard returned when locking a [`GuardedList`]; refreshes the `AtomicBool` flag on drop.
pub struct ListGuard<'a, T> {
    guard: MutexGuard<'a, Vec<T>>,
    non_empty: &'a AtomicBool,
}

impl<T> Deref for ListGuard<'_, T> {
    type Target = Vec<T>;

    fn deref(&self) -> &Vec<T> {
        &self.guard
    }
}

impl<T> DerefMut for ListGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Vec<T> {
        &mut self.guard
    }
}

impl<T> Drop for ListGuard<'_, T> {
    fn drop(&mut self) {
        self.non_empty.store(!self.guard.is_empty(), Ordering::Release);
    }
}

impl<T> Lockable for GuardedList<T> {
    type Guard<'a>
        = ListGuard<'a, T>
    where
        Self: 'a;

    fn lock_inner(&self) -> ListGuard<'_, T> {
        ListGuard {
            guard: self.inner.lock().unwrap_or_else(|p| p.into_inner()),
            non_empty: &self.non_empty,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guarded_list_tracks_emptiness_across_writes() {
        let list: GuardedList<u32> = GuardedList::new();
        assert!(!list.has_any());
        list.lock_inner().push(7);
        assert!(list.has_any());
        *list.lock_inner() = Vec::new();
        assert!(!list.has_any());
    }
}
