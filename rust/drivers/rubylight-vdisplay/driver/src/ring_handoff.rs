//! Synchronous handoff between control-plane ring clears and the swap-chain worker.
//!
//! When `IOCTL_CLEAR_FRAME_RING` or `IOCTL_CLEAR_FRAME_RING_GENERATION` removes a ring,
//! the host drops its D3D12/D3D11 shared textures and fences immediately after the IOCTL
//! returns. Returning while `rlvd-swapchain` still has GPU commands referencing those
//! resources (or still holds `OpenRing`/`RingHandles`) causes cross-process GPU MMU faults
//! (`nvlddmkm` Event 13 / TDR `153`) on NVIDIA GPUs.
//!
//! [`RingHandoff`] tracks each active swap-chain worker's open host ring generation and the
//! highest monotonic `Monitor::ring_gen` sequence it has fully settled (only updated after
//! draining D3D11 and dropping any superseded `OpenRing` and `RingHandles`). Control-plane
//! clears wait on its condvar until every active worker has settled past the clear's
//! sequence or exited.

use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::registry::lock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WorkerSlot {
    id: u64,
    /// Host `RingHandles::generation` currently open on this worker, if any.
    active_ring_gen: Option<u32>,
    /// Highest monotonic `Monitor::ring_gen` this worker has fully transitioned past
    /// (after draining D3D11 and dropping any earlier `OpenRing`/`RingHandles`).
    settled_ring_seq: u64,
}

#[derive(Debug, Default)]
pub struct ActiveRingState {
    next_worker_id: u64,
    workers: Vec<WorkerSlot>,
}

impl ActiveRingState {
    /// Whether any swap-chain worker is currently active on this monitor.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn worker_active(&self) -> bool {
        !self.workers.is_empty()
    }

    /// Host ring generation currently open on an active worker, if any.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn active_ring_gen(&self) -> Option<u32> {
        self.workers.iter().rev().find_map(|w| w.active_ring_gen)
    }

    /// True once no active worker can still hold a ring from before `cleared_seq`.
    pub fn is_cleared(&self, cleared_seq: u64) -> bool {
        self.workers
            .iter()
            .all(|w| w.settled_ring_seq >= cleared_seq)
    }
}

/// Condvar-backed tracker pairing `Monitor::ring_gen` with active swap-chain workers.
#[derive(Default)]
pub struct RingHandoff {
    state: Mutex<ActiveRingState>,
    changed: Condvar,
}

impl RingHandoff {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a swap-chain worker before its first `monitor.ring()` read.
    pub fn enter_worker(&self) -> u64 {
        let mut state = lock(&self.state);
        state.next_worker_id = state.next_worker_id.wrapping_add(1).max(1);
        let id = state.next_worker_id;
        state.workers.push(WorkerSlot {
            id,
            active_ring_gen: None,
            settled_ring_seq: 0,
        });
        id
    }

    /// Record that `worker_id` has drained D3D11 and dropped any ring from before `seq`,
    /// and now holds `active_ring_gen`. Wakes any waiting clear IOCTL.
    pub fn note_settled(&self, worker_id: u64, active_ring_gen: Option<u32>, seq: u64) {
        let mut state = lock(&self.state);
        if let Some(slot) = state.workers.iter_mut().find(|w| w.id == worker_id) {
            slot.active_ring_gen = active_ring_gen;
            if seq > slot.settled_ring_seq {
                slot.settled_ring_seq = seq;
            }
        }
        self.changed.notify_all();
    }

    /// Unregister `worker_id` after it drained D3D11 and dropped `OpenRing`/`RingHandles`.
    /// Wakes any waiting clear IOCTL.
    pub fn exit_worker(&self, worker_id: u64) {
        let mut state = lock(&self.state);
        state.workers.retain(|w| w.id != worker_id);
        self.changed.notify_all();
    }

    /// Wait up to `budget` until no active worker can still hold the ring cleared at
    /// `cleared_seq`. Returns `true` when cleared, `false` on timeout.
    pub fn wait_cleared(&self, cleared_seq: u64, budget: Duration) -> bool {
        let deadline = Instant::now() + budget;
        let mut state = lock(&self.state);
        while !state.is_cleared(cleared_seq) {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            if remaining.is_zero() {
                return false;
            }
            let (guard, result) = self
                .changed
                .wait_timeout(state, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state = guard;
            if result.timed_out() && !state.is_cleared(cleared_seq) {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;

    #[test]
    fn clear_returns_immediately_when_no_worker_is_active() {
        let handoff = RingHandoff::new();
        let started = Instant::now();
        assert!(handoff.wait_cleared(1, Duration::from_secs(2)));
        assert!(started.elapsed() < Duration::from_millis(100));
    }

    #[test]
    fn clear_waits_until_worker_drains_and_settles_past_clear_seq() {
        let handoff = Arc::new(RingHandoff::new());
        let worker_id = handoff.enter_worker();
        handoff.note_settled(worker_id, Some(7), 1);

        let drained = Arc::new(AtomicBool::new(false));
        let worker_handoff = handoff.clone();
        let worker_drained = drained.clone();
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(25));
            worker_drained.store(true, Ordering::Release);
            worker_handoff.note_settled(worker_id, None, 2);
        });

        assert!(handoff.wait_cleared(2, Duration::from_secs(2)));
        assert!(drained.load(Ordering::Acquire));
        let state = lock(&handoff.state);
        assert!(state.worker_active());
        assert_eq!(state.active_ring_gen(), None);
        drop(state);
        handle.join().unwrap();
    }

    #[test]
    fn clear_waits_when_worker_entered_before_opening_ring() {
        let handoff = Arc::new(RingHandoff::new());
        let worker_id = handoff.enter_worker();

        let settled = Arc::new(AtomicBool::new(false));
        let worker_handoff = handoff.clone();
        let worker_settled = settled.clone();
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            // Worker first observes the old ring at seq 1 (must not unblock clear at seq 2).
            worker_handoff.note_settled(worker_id, Some(1), 1);
            thread::sleep(Duration::from_millis(20));
            // Then observes clear at seq 2 after draining and dropping the old ring.
            worker_settled.store(true, Ordering::Release);
            worker_handoff.note_settled(worker_id, None, 2);
        });

        assert!(handoff.wait_cleared(2, Duration::from_secs(2)));
        assert!(settled.load(Ordering::Acquire));
        handle.join().unwrap();
    }

    #[test]
    fn clear_wakes_immediately_on_worker_exit_teardown() {
        let handoff = Arc::new(RingHandoff::new());
        let worker_id = handoff.enter_worker();
        handoff.note_settled(worker_id, Some(3), 1);

        let worker_handoff = handoff.clone();
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            worker_handoff.exit_worker(worker_id);
        });

        assert!(handoff.wait_cleared(2, Duration::from_secs(2)));
        assert!(!lock(&handoff.state).worker_active());
        handle.join().unwrap();
    }

    #[test]
    fn concurrent_reinstall_even_with_same_generation_unblocks_prior_clear() {
        let handoff = Arc::new(RingHandoff::new());
        let worker_id = handoff.enter_worker();
        handoff.note_settled(worker_id, Some(1), 1);

        // Clear bumps to seq 2; a concurrent SET_*_RING immediately bumps to seq 3 with
        // generation 1 again. Worker drains/drops the old ring, settles at seq 3, then opens
        // the new ring.
        let worker_handoff = handoff.clone();
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            worker_handoff.note_settled(worker_id, None, 3);
            worker_handoff.note_settled(worker_id, Some(1), 3);
        });

        assert!(handoff.wait_cleared(2, Duration::from_secs(2)));
        assert_eq!(lock(&handoff.state).active_ring_gen(), Some(1));
        handle.join().unwrap();
    }

    #[test]
    fn overlapping_workers_must_both_settle_before_clear_completes() {
        let handoff = Arc::new(RingHandoff::new());
        let w1 = handoff.enter_worker();
        let w2 = handoff.enter_worker();
        handoff.note_settled(w1, Some(1), 1);
        handoff.note_settled(w2, None, 2);

        let w1_done = Arc::new(AtomicBool::new(false));
        let worker_handoff = handoff.clone();
        let worker_done = w1_done.clone();
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(25));
            worker_done.store(true, Ordering::Release);
            worker_handoff.exit_worker(w1);
        });

        assert!(handoff.wait_cleared(2, Duration::from_secs(2)));
        assert!(w1_done.load(Ordering::Acquire));
        handle.join().unwrap();
    }

    #[test]
    fn clear_wait_is_bounded_on_hung_worker() {
        let handoff = RingHandoff::new();
        let worker_id = handoff.enter_worker();
        handoff.note_settled(worker_id, Some(5), 1);

        assert!(!handoff.wait_cleared(2, Duration::from_millis(30)));
    }
}
