//! Thread-safe handle tables for JNI bridges (`nightjar-android`).
//!
//! Handles are monotonically assigned `i64` tokens starting at `1`, never raw pointers. A call
//! racing `destroy` sees `None` instead of freed memory.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

/// Acquires a mutex without staying poisoned across a caught panic.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Concurrent handle table storing shared `Arc<T>` entries (no outer mutex on `T`).
pub(crate) struct Slab<T> {
    entries: Mutex<BTreeMap<i64, Arc<T>>>,
    next_handle: AtomicI64,
}

impl<T> Slab<T> {
    pub(crate) const fn new() -> Self {
        Self {
            entries: Mutex::new(BTreeMap::new()),
            next_handle: AtomicI64::new(1),
        }
    }

    pub(crate) fn insert(&self, value: T) -> i64 {
        self.insert_arc(Arc::new(value))
    }

    pub(crate) fn insert_arc(&self, value: Arc<T>) -> i64 {
        let handle = self.next_handle.fetch_add(1, Ordering::Relaxed);
        lock(&self.entries).insert(handle, value);
        handle
    }

    pub(crate) fn get(&self, handle: i64) -> Option<Arc<T>> {
        if handle <= 0 {
            return None;
        }
        lock(&self.entries).get(&handle).cloned()
    }

    pub(crate) fn remove(&self, handle: i64) -> bool {
        if handle <= 0 {
            return false;
        }
        lock(&self.entries).remove(&handle).is_some()
    }
}

/// Handle table for single-threaded or short-lived mutable state behind an inner `Mutex`.
pub(crate) struct MutexSlab<T> {
    inner: Slab<Mutex<T>>,
}

impl<T> MutexSlab<T> {
    pub(crate) const fn new() -> Self {
        Self { inner: Slab::new() }
    }

    pub(crate) fn insert(&self, value: T) -> i64 {
        self.inner.insert(Mutex::new(value))
    }

    pub(crate) fn with<R>(&self, handle: i64, f: impl FnOnce(&mut T) -> R) -> Option<R> {
        let entry = self.inner.get(handle)?;
        let mut guard = lock(&entry);
        Some(f(&mut guard))
    }

    pub(crate) fn remove(&self, handle: i64) -> bool {
        self.inner.remove(handle)
    }
}
