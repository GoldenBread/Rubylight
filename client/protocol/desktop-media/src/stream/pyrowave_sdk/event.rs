//! Cached auto-reset Win32 event handles for `ID3D11Fence::SetEventOnCompletion` waits.

use std::cell::Cell;
use std::time::Duration;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Graphics::Direct3D11::ID3D11Fence;
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

struct CachedEvent(Cell<HANDLE>);

impl CachedEvent {
    const fn empty() -> Self {
        Self(Cell::new(HANDLE(std::ptr::null_mut())))
    }

    fn take(&self) -> Option<HANDLE> {
        let h = self.0.replace(HANDLE(std::ptr::null_mut()));
        (!h.is_invalid() && !h.0.is_null()).then_some(h)
    }

    fn put(&self, h: HANDLE) {
        let old = self.0.replace(h);
        if !old.is_invalid() && !old.0.is_null() {
            // SAFETY: Owned event handle created by `CreateEventW`.
            let _ = unsafe { CloseHandle(old) };
        }
    }
}

impl Drop for CachedEvent {
    fn drop(&mut self) {
        let h = self.0.get();
        if !h.is_invalid() && !h.0.is_null() {
            // SAFETY: Owned event handle created by `CreateEventW` on this thread.
            let _ = unsafe { CloseHandle(h) };
        }
    }
}

thread_local! {
    static FENCE_EVENT: CachedEvent = const { CachedEvent::empty() };
}

/// Waits on `fence` to reach `value` within `deadline` using a thread-local cached auto-reset
/// event handle. Events that time out or fail are closed rather than reused so a late signal
/// can never spuriously wake a subsequent wait.
pub(super) fn wait_fence_within(fence: &ID3D11Fence, value: u64, deadline: Duration) -> Result<(), String> {
    // SAFETY: Plain fence calls; event handle is either returned clean after `WAIT_OBJECT_0` or closed.
    unsafe {
        let mut completed = fence.GetCompletedValue();
        if completed < value {
            let event = match FENCE_EVENT.with(CachedEvent::take) {
                Some(event) => event,
                None => CreateEventW(None, false, false, PCWSTR::null())
                    .map_err(|e| format!("cannot wait for the PyroWave decode: {e}"))?,
            };
            let armed = fence.SetEventOnCompletion(value, event);
            let waited = armed
                .is_ok()
                .then(|| WaitForSingleObject(event, deadline.as_millis().min(u128::from(u32::MAX - 1)) as u32));
            if waited == Some(WAIT_OBJECT_0) {
                // Auto-reset event already returned to unsignaled after its signal fired.
                FENCE_EVENT.with(|slot| slot.put(event));
            } else {
                let _ = CloseHandle(event);
            }
            armed.map_err(|e| format!("cannot wait for the PyroWave decode: {e}"))?;
            completed = fence.GetCompletedValue();
            if waited != Some(WAIT_OBJECT_0) && completed < value {
                return Err(format!(
                    "the PyroWave decode (fence value {value}) was not done within {deadline:?} and the fence stands at {completed}: the Vulkan device may be hung or lost, so the D3D11 queue must not wait on it"
                ));
            }
        }
        if completed == u64::MAX {
            return Err(format!("the D3D11 device holding the PyroWave fence was removed while waiting for value {value}"));
        }
    }
    Ok(())
}
