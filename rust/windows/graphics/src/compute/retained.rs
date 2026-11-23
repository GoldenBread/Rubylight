//! Retention of unfinished D3D12 compute/copy work across teardown.
//!
//! When a queue fails to drain within its teardown budget, [`RetainedWork`]
//! keeps its fences, command queues, shaders, textures and capture leases alive
//! until the GPU reaches a tail marker or the device is removed, and gates new
//! compute generations on that adapter until it clears.

use super::{Compute, completed_value, wait_fence};
use anyhow::Result;
use std::sync::Mutex;
use windows::{
    Win32::{
        Foundation::LUID,
        Graphics::Direct3D12::{
            D3D12_FENCE_FLAG_NONE, ID3D12CommandQueue, ID3D12Device, ID3D12Fence,
        },
    },
    core::{IUnknown, Interface},
};

/// Resources whose last GPU access could not be proved complete. In particular,
/// an ExecuteCommandLists followed by a failed Signal has no usable old fence
/// value. A fresh tail marker covers that work too; if even that signal fails,
/// only device removal makes releasing its references safe.
pub(crate) struct RetainedWork {
    device: ID3D12Device,
    pub(super) adapter: (u32, i32),
    pub(super) tail: Option<ID3D12Fence>,
    pub(super) waits: Vec<(ID3D12Fence, u64)>,
    objects: Vec<IUnknown>,
    pub(super) captures: Vec<crate::capture::GpuImage>,
}
// SAFETY: The erased interfaces are D3D12 objects, multithread-protected D3D11
// objects, or fences; they are only retained/released, never used via IUnknown.
unsafe impl Send for RetainedWork {}

pub(super) static RETAINED: Mutex<Vec<RetainedWork>> = Mutex::new(Vec::new());

/// Whether teardown should block on `tail` before retaining unfinished work:
/// once an adapter already has retained work behind a retired queue, another
/// drop on the same stuck queue parks its resources right away instead of
/// stacking another two-second wait.
pub(super) fn should_wait_tail(complete: bool, queue_stuck: bool, adapter_retained: bool) -> bool {
    !complete && !(queue_stuck && adapter_retained)
}

fn has_retained(adapter: (u32, i32)) -> bool {
    RETAINED
        .lock()
        .unwrap()
        .iter()
        .any(|work| work.adapter == adapter)
}

impl RetainedWork {
    pub(crate) fn new(compute: &Compute, queue: &ID3D12CommandQueue) -> Self {
        // SAFETY: Both objects are live and on the same device. This final
        // marker submits no new resource access, including on a retired queue.
        let (luid, tail) = unsafe {
            (
                compute.device.GetAdapterLuid(),
                compute
                    .device
                    .CreateFence::<ID3D12Fence>(0, D3D12_FENCE_FLAG_NONE)
                    .and_then(|fence| queue.Signal(&fence, 1).map(|()| fence))
                    .ok(),
            )
        };
        let mut work = Self {
            device: compute.device.clone(),
            adapter: (luid.LowPart, luid.HighPart),
            tail,
            waits: Vec::new(),
            objects: Vec::new(),
            captures: Vec::new(),
        };
        work.keep(queue);
        work.keep(&compute.fence);
        work
    }

    pub(crate) fn keep(&mut self, object: &impl Interface) {
        // SAFETY: Every caller supplies a COM D3D11/D3D12 interface, whose
        // vtable starts with IUnknown. Transfer a cloned reference without a
        // fallible QueryInterface call during failure cleanup; this erased
        // reference is only retained and released, never used as a GPU object.
        self.objects
            .push(unsafe { IUnknown::from_raw(object.clone().into_raw()) });
    }

    fn complete(&self) -> bool {
        if crate::device_loss::DeviceLost::d3d12(&self.device).is_some() {
            return true;
        }
        // SAFETY: Every queried fence is retained by this bundle. Device
        // removal is handled above, not mistaken for a completed UINT64_MAX.
        unsafe {
            self.tail.as_ref().is_some_and(|fence| {
                completed_value(fence.GetCompletedValue()).is_ok_and(|done| done >= 1)
            }) && self.waits.iter().all(|(fence, value)| {
                completed_value(fence.GetCompletedValue()).is_ok_and(|done| done >= *value)
            })
        }
    }

    pub(crate) fn finish(self, compute: &Compute) {
        if should_wait_tail(
            self.complete(),
            compute.is_stuck(),
            has_retained(self.adapter),
        ) && let Some(fence) = &self.tail
        {
            let _ = wait_fence(fence, 1);
        }
        if !self.complete() {
            compute.retire();
            tracing::warn!(
                retained_objects = self.objects.len(),
                "retaining unfinished GPU resources; compute recovery waits for this adapter to drain"
            );
            RETAINED.lock().unwrap().push(self);
        }
    }
}

/// Do not accumulate a fresh set of video-memory allocations on every retry
/// behind a permanently hung queue. Existing users have bounded pools; no new
/// compute generation is admitted on this adapter until its retained work ends.
pub(super) fn allow_compute_generation(luid: LUID) -> Result<()> {
    let (complete, blocked) = {
        let mut retained = RETAINED.lock().unwrap();
        let mut complete = Vec::new();
        let mut index = 0;
        while index < retained.len() {
            if retained[index].complete() {
                complete.push(retained.swap_remove(index));
            } else {
                index += 1;
            }
        }
        let blocked = retained
            .iter()
            .any(|work| work.adapter == (luid.LowPart, luid.HighPart));
        (complete, blocked)
    };
    // Releasing a capture lease can run its own teardown; never do it under
    // the retention lock.
    drop(complete);
    anyhow::ensure!(
        !blocked,
        "unfinished GPU resources on this adapter; compute conversion is unavailable until they drain"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::should_wait_tail;

    #[test]
    fn retaining_on_an_already_stuck_adapter_skips_redundant_tail_waits() {
        // Already complete: never waits.
        for queue_stuck in [false, true] {
            for adapter_retained in [false, true] {
                assert!(!should_wait_tail(true, queue_stuck, adapter_retained));
            }
        }
        // Incomplete on a healthy queue, or the first drop on a retired queue: waits once.
        assert!(should_wait_tail(false, false, false));
        assert!(should_wait_tail(false, true, false));
        assert!(should_wait_tail(false, false, true));
        // Incomplete once the queue is stuck AND the adapter already retained work: skips another wait.
        assert!(!should_wait_tail(false, true, true));
    }
}
