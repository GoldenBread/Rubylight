//! Owner liveness: which host processes are alive, and reaping the monitors of one that is
//! not.
//!
//! From punktfunk's pf-vdisplay `watchdog.rs` by Enrico Bühler (unom), MIT OR Apache-2.0.
//!
//! A monitor belongs to the process whose ADD created it. That owner ends when its last
//! control handle closes ([`evt_file_cleanup`]; a crash closes every handle, so this is
//! immediate) or when it sends no IOCTL for [`WATCHDOG_TIMEOUT_S`] ([`evt_timer`], a hung
//! host). Only that owner's monitors depart. The timer is a child of the device, so no tick
//! outlives it, and [`stop`] waits out a tick that is mid-reap.

use std::sync::Mutex;
use std::time::Duration;

use wdk_iddcx::nt_success;
use wdk_sys::{
    NTSTATUS, ULONG, WDF_OBJECT_ATTRIBUTES, WDF_TIMER_CONFIG, WDFDEVICE, WDFFILEOBJECT, WDFTIMER,
    call_unsafe_wdf_function_binding,
};

use crate::registry::lock;
use crate::worker::Sendable;

/// An owner must send an IOCTL within this window or its monitors are reaped. The host's
/// lease heartbeat runs every second. Reported over `IOCTL_GET_INFO`.
pub const WATCHDOG_TIMEOUT_S: u32 = 10;

const TICK_MS: u32 = 3000;
/// Silent ticks before a reap, rounded up so it never fires short of the timeout.
const TICKS_TO_REAP: u32 = (1000 * WATCHDOG_TIMEOUT_S).div_ceil(TICK_MS);

struct Owner {
    pid: u32,
    pings: u64,
    seen: u64,
    silent: u32,
}

/// Unified owner and control-handle table so handle registration (`ping`), last-handle
/// cleanup (`evt_file_cleanup`) and watchdog ticks (`evt_timer`) serialize atomically.
struct OwnerState {
    owners: Vec<Owner>,
    /// Control handles that sent an IOCTL, with their owning process.
    handles: Vec<(usize, u32)>,
}

impl OwnerState {
    const fn new() -> Self {
        Self {
            owners: Vec::new(),
            handles: Vec::new(),
        }
    }

    fn clear(&mut self) {
        self.owners.clear();
        self.handles.clear();
    }

    fn rebase_all(&mut self) {
        for o in &mut self.owners {
            o.seen = o.pings;
            o.silent = 0;
        }
    }

    fn ping(&mut self, pid: u32, handle_key: usize) {
        match self.owners.iter_mut().find(|o| o.pid == pid) {
            Some(o) => o.pings += 1,
            None => self.owners.push(Owner {
                pid,
                pings: 1,
                seen: 0,
                silent: 0,
            }),
        }
        if !self.handles.iter().any(|(fo, _)| *fo == handle_key) {
            self.handles.push((handle_key, pid));
        }
    }

    /// Remove `handle_key`. Returns `Some(pid)` only when `powered` is true and no other
    /// handle for `pid` remains, removing `pid` from `owners` under the same lock. When
    /// `!powered`, drops the closed handle without departing monitors and leaves `owners`
    /// intact so the watchdog can reap on resume if the owner died while suspended.
    fn close_handle(&mut self, handle_key: usize, powered: bool) -> Option<u32> {
        let pos = self.handles.iter().position(|(fo, _)| *fo == handle_key)?;
        let (_, pid) = self.handles.swap_remove(pos);
        if !powered {
            return None;
        }
        if self.handles.iter().any(|(_, p)| *p == pid) {
            return None;
        }
        self.owners.retain(|o| o.pid != pid);
        Some(pid)
    }

    fn take_stale(&mut self) -> Vec<u32> {
        let mut stale = Vec::new();
        for o in &mut self.owners {
            if o.seen != o.pings {
                o.seen = o.pings;
                o.silent = 0;
                continue;
            }
            o.silent += 1;
            if o.silent >= TICKS_TO_REAP {
                stale.push(o.pid);
            }
        }
        if !stale.is_empty() {
            self.owners.retain(|o| !stale.contains(&o.pid));
            self.handles.retain(|(_, p)| !stale.contains(p));
        }
        stale
    }
}

static OWNER_STATE: Mutex<OwnerState> = Mutex::new(OwnerState::new());

type SendTimer = Sendable<WDFTIMER>;
static TIMER: Mutex<Option<SendTimer>> = Mutex::new(None);

/// Create the timer, stopped, as a child of `device`, before any IOCTL can arrive.
pub fn create(device: WDFDEVICE) -> NTSTATUS {
    // AutomaticSerialization stays FALSE: the tick joins swap-chain workers, which must not
    // queue in front of the device's other callbacks.
    let mut cfg = WDF_TIMER_CONFIG {
        Size: core::mem::size_of::<WDF_TIMER_CONFIG>() as ULONG,
        EvtTimerFunc: Some(evt_timer),
        Period: TICK_MS,
        ..Default::default()
    };
    let mut attr = WDF_OBJECT_ATTRIBUTES {
        Size: core::mem::size_of::<WDF_OBJECT_ATTRIBUTES>() as ULONG,
        ParentObject: device.cast(),
        ExecutionLevel: wdk_sys::_WDF_EXECUTION_LEVEL::WdfExecutionLevelInheritFromParent,
        SynchronizationScope:
            wdk_sys::_WDF_SYNCHRONIZATION_SCOPE::WdfSynchronizationScopeInheritFromParent,
        ..Default::default()
    };
    let mut timer: WDFTIMER = core::ptr::null_mut();
    // SAFETY: `cfg` and `attr` are initialised locals; `timer` receives the handle.
    let status = unsafe {
        call_unsafe_wdf_function_binding!(WdfTimerCreate, &mut cfg, &mut attr, &mut timer)
    };
    dbglog!("[rlvd] watchdog WdfTimerCreate -> {status:#x}");
    if nt_success(status) {
        *lock(&TIMER) = Some(Sendable(timer));
    }
    status
}

/// Arm the watchdog once monitors can exist. Re-arming re-bases every owner's sample, so the
/// first tick after it never counts as silence.
pub fn start() {
    let held = lock(&TIMER);
    let Some(timer) = held.as_ref().map(|t| t.0) else {
        return;
    };
    if !crate::adapter::powered() {
        return;
    }
    lock(&OWNER_STATE).rebase_all();
    // SAFETY: `timer` is the live device-parented timer; a negative due time is relative.
    let _armed = unsafe {
        call_unsafe_wdf_function_binding!(WdfTimerStart, timer, -(i64::from(TICK_MS) * 10_000))
    };
}

/// Sleep suspends the clock, not the owners. Serialize start/stop so an async
/// adapter completion cannot rearm the timer after D0Exit paused it.
pub fn pause() {
    let held = lock(&TIMER);
    if let Some(timer) = held.as_ref().map(|t| t.0) {
        // SAFETY: the device-parented timer is live; wait for an active reap.
        let _ = unsafe { call_unsafe_wdf_function_binding!(WdfTimerStop, timer, 1) };
    }
}

/// Disarm on device cleanup and wait for a running tick, so no reap races the cleanup.
pub fn stop() {
    let taken = lock(&TIMER).take();
    let Some(Sendable(timer)) = taken else {
        return;
    };
    // SAFETY: the timer is still live (cleanup runs before the framework deletes children);
    // Wait = TRUE returns only after a running tick has returned.
    let _was_queued = unsafe { call_unsafe_wdf_function_binding!(WdfTimerStop, timer, 1) };
    lock(&OWNER_STATE).clear();
    dbglog!("[rlvd] watchdog: device cleanup, timer stopped");
}

/// Any IOCTL from `pid` proves it alive; remember the handle it came on.
pub fn ping(pid: u32, file_object: WDFFILEOBJECT) {
    lock(&OWNER_STATE).ping(pid, file_object as usize);
}

/// `EvtFileCleanup`: a control handle is closing. If it was its process's last while the
/// device is powered, that process is gone and its monitors depart now.
pub unsafe extern "C" fn evt_file_cleanup(file_object: WDFFILEOBJECT) {
    let _power = crate::adapter::POWER_GATE
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let powered = crate::adapter::powered();
    let key = file_object as usize;
    let reaped = {
        let mut state = lock(&OWNER_STATE);
        state.close_handle(key, powered).map(|pid| {
            // Hold OWNER_STATE across monitor selection (lock order: POWER_GATE ->
            // OWNER_STATE -> REGISTRY) so a concurrent same-PID handle open + ping +
            // IOCTL_ADD cannot insert a fresh monitor that this cleanup then departs.
            let removed = crate::monitor::take_owner_monitors(pid, Duration::ZERO);
            (pid, removed)
        })
    };
    if let Some((pid, removed)) = reaped {
        let n = removed.len();
        crate::monitor::depart(removed);
        dbglog!("[rlvd] owner {pid}: last control handle closed; departed {n} monitor(s)");
    }
}

/// One tick: depart the monitors of every owner silent for the whole window.
unsafe extern "C" fn evt_timer(_timer: WDFTIMER) {
    if !crate::adapter::powered() {
        return;
    }
    let reaped: Vec<_> = {
        let mut state = lock(&OWNER_STATE);
        state
            .take_stale()
            .into_iter()
            .map(|pid| {
                let removed = crate::monitor::take_owner_monitors(pid, Duration::from_secs(3));
                (pid, removed)
            })
            .collect()
    };
    for (pid, removed) in reaped {
        let n = removed.len();
        crate::monitor::depart(removed);
        dbglog!(
            "[rlvd] watchdog: no IOCTL from owner {pid} in {WATCHDOG_TIMEOUT_S} s; departed {n} monitor(s)"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Barrier, RwLock};
    use std::thread;

    #[test]
    fn close_handle_unpowered_drops_handle_retains_owner_and_skips_reap() {
        let mut state = OwnerState::new();
        state.ping(42, 0x100);
        assert_eq!(state.close_handle(0x100, false), None);
        assert!(state.handles.is_empty());
        // Owner remains tracked so the watchdog can still reap after resume.
        assert_eq!(state.owners.len(), 1);
        assert_eq!(state.owners[0].pid, 42);
        // Closing an already-removed handle is a no-op.
        assert_eq!(state.close_handle(0x100, true), None);
        // After resume, silent ticks reap the dead owner.
        state.rebase_all();
        for _ in 0..TICKS_TO_REAP - 1 {
            assert!(state.take_stale().is_empty());
        }
        assert_eq!(state.take_stale(), vec![42]);
        assert!(state.owners.is_empty());
    }

    #[test]
    fn close_handle_only_reaps_when_last_handle_for_pid_closes() {
        let mut state = OwnerState::new();
        state.ping(7, 0x10);
        state.ping(7, 0x20);
        state.ping(8, 0x30);

        assert_eq!(state.close_handle(0x10, true), None);
        assert_eq!(state.owners.len(), 2);

        assert_eq!(state.close_handle(0x20, true), Some(7));
        assert_eq!(state.owners.len(), 1);
        assert_eq!(state.owners[0].pid, 8);
    }

    #[test]
    fn serialized_cleanup_never_reaps_concurrent_same_pid_add_on_new_handle() {
        // Models the exact lock protocol between `evt_file_cleanup` and `control::dispatch`
        // (`ping` -> `registry::insert`) under `OWNER_STATE -> REGISTRY`.
        for _ in 0..200 {
            let owner_state = Arc::new(Mutex::new(OwnerState::new()));
            let monitors = Arc::new(Mutex::new(Vec::<(u32, u64)>::new()));
            owner_state.lock().unwrap().ping(99, 0xA);
            monitors.lock().unwrap().push((99, 1)); // old monitor from handle 0xA

            let start = Arc::new(Barrier::new(2));
            let s_clean = owner_state.clone();
            let m_clean = monitors.clone();
            let b_clean = start.clone();
            let cleanup = thread::spawn(move || {
                b_clean.wait();
                let mut state = lock(&s_clean);
                state
                    .close_handle(0xA, true)
                    .map(|pid| {
                        let mut reg = lock(&m_clean);
                        let (gone, keep): (Vec<_>, Vec<_>) =
                            reg.drain(..).partition(|(owner, _)| *owner == pid);
                        *reg = keep;
                        gone
                    })
                    .unwrap_or_default()
            });

            start.wait();
            // Concurrent same-PID IOCTL_ADD on a fresh handle 0xB: ping then insert.
            lock(&owner_state).ping(99, 0xB);
            lock(&monitors).push((99, 2));

            let departed = cleanup.join().unwrap();
            assert!(
                !departed.contains(&(99, 2)),
                "fresh monitor from new handle 0xB was departed by old handle 0xA cleanup"
            );
            assert!(
                lock(&monitors).contains(&(99, 2)),
                "fresh monitor from new handle 0xB must remain registered"
            );
        }
    }

    #[test]
    fn power_gate_serializes_cleanup_against_suspend_and_skips_unpowered_depart() {
        let gate = Arc::new(RwLock::new(()));
        let powered = Arc::new(AtomicBool::new(true));
        let owner_state = Arc::new(Mutex::new(OwnerState::new()));
        lock(&owner_state).ping(55, 0x500);

        // Hold POWER_GATE write lock to simulate D0Exit / ReleaseHardware.
        let power_write = gate.write().unwrap();

        let g = gate.clone();
        let p = powered.clone();
        let s = owner_state.clone();
        let cleanup_started = Arc::new(AtomicBool::new(false));
        let started_flag = cleanup_started.clone();
        let cleanup = thread::spawn(move || {
            started_flag.store(true, Ordering::Release);
            let _read = g.read().unwrap();
            let is_powered = p.load(Ordering::Acquire);
            lock(&s).close_handle(0x500, is_powered)
        });

        while !cleanup_started.load(Ordering::Acquire) {
            thread::yield_now();
        }
        powered.store(false, Ordering::Release);
        drop(power_write);

        // Once unblocked, cleanup observes `!powered`, removes the handle, and skips depart.
        assert_eq!(cleanup.join().unwrap(), None);
        let state = lock(&owner_state);
        assert!(state.handles.is_empty());
        assert_eq!(state.owners.len(), 1);
    }
}
