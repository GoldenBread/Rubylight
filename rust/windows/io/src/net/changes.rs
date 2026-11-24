//! IP Helper notifications for Windows network interface and address changes.
use std::sync::{Mutex, PoisonError};
use windows::Win32::{
    NetworkManagement::IpHelper::{
        CancelMibChangeNotify2, MIB_IPINTERFACE_ROW, MIB_NOTIFICATION_TYPE,
        MIB_UNICASTIPADDRESS_ROW, NotifyIpInterfaceChange, NotifyUnicastIpAddressChange,
    },
    Networking::WinSock::AF_UNSPEC,
};

/// Windows telling this PC's IP interfaces or addresses changed: an interface came or went, a
/// parameter of one (its state among them) moved, or an address was added, removed or changed
/// (the interface notification does not report addresses). A watcher re-reads the interfaces on
/// a change instead of on a short timer, so the reading runs only when there was one.
pub struct InterfaceChanges {
    /// The registrations `CancelMibChangeNotify2` takes: interfaces, then unicast addresses.
    handles: Vec<windows::Win32::Foundation::HANDLE>,
    /// The context the OS callback signals, kept alive until after the cancel: a callback that
    /// races it finds its context valid.
    context: std::sync::Arc<Signal>,
    worker: Option<std::thread::JoinHandle<()>>,
}

/// What the callback and the worker tell each other. The callback only notes the change and
/// wakes the worker: it runs on a Windows thread, where no other work happens.
#[derive(Default)]
struct Signal {
    state: Mutex<SignalState>,
    wake: std::sync::Condvar,
}
#[derive(Default)]
struct SignalState {
    changed: bool,
    stopped: bool,
}

/// How long a change holds the window open for the ones that follow it: whatever the watcher
/// did about the first has happened by the end of it, and later changes within it are covered.
const QUIET: std::time::Duration = std::time::Duration::from_secs(2);

impl InterfaceChanges {
    /// Call `changed` when Windows reports an interface change, and `quiet` once none followed
    /// within a short window. `None` when Windows refuses the notification: the caller then
    /// falls back to its own timer.
    pub fn register(
        changed: impl Fn() + Send + 'static,
        quiet: impl Fn() + Send + 'static,
    ) -> Option<Self> {
        Self::register_with(false, changed, quiet)
    }
    /// The registration for the test below, which also reports every interface there is now, so
    /// the callback arrives without an interface actually changing.
    #[cfg(test)]
    pub fn register_initial(
        changed: impl Fn() + Send + 'static,
        quiet: impl Fn() + Send + 'static,
    ) -> Option<Self> {
        Self::register_with(true, changed, quiet)
    }
    fn register_with(
        initial: bool,
        changed: impl Fn() + Send + 'static,
        quiet: impl Fn() + Send + 'static,
    ) -> Option<Self> {
        use windows::Win32::Foundation::ERROR_SUCCESS;
        let context = std::sync::Arc::new(Signal::default());
        let pointer = std::sync::Arc::as_ptr(&context) as *const core::ffi::c_void;
        let mut interfaces = windows::Win32::Foundation::HANDLE::default();
        // SAFETY: `context` outlives the registration, as Drop (or the cancel below) cancels it
        // before the field drops, and `interfaces` is a live out-pointer for the call.
        let registered = unsafe {
            NotifyIpInterfaceChange(
                AF_UNSPEC,
                Some(interface_changed),
                Some(pointer),
                initial,
                &mut interfaces,
            )
        };
        if registered != ERROR_SUCCESS {
            return None;
        }
        let mut addresses = windows::Win32::Foundation::HANDLE::default();
        // SAFETY: As above, for the address registration.
        let registered = unsafe {
            NotifyUnicastIpAddressChange(
                AF_UNSPEC,
                Some(address_changed),
                Some(pointer),
                initial,
                &mut addresses,
            )
        };
        if registered != ERROR_SUCCESS {
            cancel(&[interfaces]);
            return None;
        }
        let handles = vec![interfaces, addresses];
        let signal = context.clone();
        let worker = std::thread::Builder::new()
            .name("interface-watch".into())
            .spawn(move || watch_interfaces(signal, changed, quiet));
        let Ok(worker) = worker else {
            // No callback may outlive the context it signals.
            cancel(&handles);
            return None;
        };
        Some(Self {
            handles,
            context,
            worker: Some(worker),
        })
    }
}

/// Cancel registrations; each cancel waits for a callback still running.
fn cancel(handles: &[windows::Win32::Foundation::HANDLE]) {
    for handle in handles {
        // SAFETY: each handle is a registration the Notify calls returned and nothing has
        // cancelled yet.
        unsafe {
            let _ = CancelMibChangeNotify2(*handle);
        }
    }
}

/// The interface callback: note the change, wake the worker, nothing else.
unsafe extern "system" fn interface_changed(
    context: *const core::ffi::c_void,
    _row: *const MIB_IPINTERFACE_ROW,
    _kind: MIB_NOTIFICATION_TYPE,
) {
    // SAFETY: the context is the `Signal` of the live registration, valid until its cancel
    // returns, and the callback runs no longer than this function.
    unsafe { note_change(context) }
}

/// The address callback: the same as `interface_changed`, for an address row.
unsafe extern "system" fn address_changed(
    context: *const core::ffi::c_void,
    _row: *const MIB_UNICASTIPADDRESS_ROW,
    _kind: MIB_NOTIFICATION_TYPE,
) {
    // SAFETY: as in `interface_changed`.
    unsafe { note_change(context) }
}

/// # Safety
/// `context` is the `Signal` of a live registration.
unsafe fn note_change(context: *const core::ffi::c_void) {
    // SAFETY: the caller's contract.
    let signal = unsafe { &*(context as *const Signal) };
    let mut state = signal.state.lock().unwrap_or_else(PoisonError::into_inner);
    state.changed = true;
    signal.wake.notify_one();
}

/// The worker: hand each change to `changed`, then wait out the window; when it closes with no
/// further change, `quiet` ends what `changed` began. The calls happen off the lock, so an
/// arriving callback never waits behind them, and the loop checks what is pending before every
/// wait, so one that arrived during a call is not slept over.
fn watch_interfaces(signal: std::sync::Arc<Signal>, changed: impl Fn(), quiet: impl Fn()) {
    let mut window: Option<std::time::Instant> = None;
    let mut state = signal.state.lock().unwrap_or_else(PoisonError::into_inner);
    loop {
        if state.stopped {
            return;
        }
        if state.changed {
            state.changed = false;
            // A change within an open window is covered by what the first one began.
            let call = window.is_none();
            window = Some(std::time::Instant::now() + QUIET);
            drop(state);
            if call {
                changed();
            }
            state = signal.state.lock().unwrap_or_else(PoisonError::into_inner);
            continue;
        }
        // Nothing pending: wait for a change, or for the window to end without one.
        let (guard, timed_out) = match window {
            Some(end) => {
                let (guard, result) = signal
                    .wake
                    .wait_timeout(
                        state,
                        end.saturating_duration_since(std::time::Instant::now()),
                    )
                    .unwrap_or_else(PoisonError::into_inner);
                (guard, result.timed_out())
            }
            None => (
                signal
                    .wake
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner),
                false,
            ),
        };
        state = guard;
        if state.stopped {
            return;
        }
        if state.changed {
            continue;
        }
        if timed_out {
            window = None;
            drop(state);
            quiet();
            state = signal.state.lock().unwrap_or_else(PoisonError::into_inner);
        }
    }
}

impl Drop for InterfaceChanges {
    fn drop(&mut self) {
        // Each cancel waits for a callback still running and Windows calls no more once it
        // returns, so the context below stays valid for every callback there may be.
        cancel(&self.handles);
        {
            let mut state = self
                .context
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            state.stopped = true;
        }
        self.context.wake.notify_one();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
