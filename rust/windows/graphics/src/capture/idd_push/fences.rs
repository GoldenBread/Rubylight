//! Fence-ring synchronization and slot release for `idd_push` captures.
//!
//! [`Fences`] owns the shared written/released/bridge D3D12/D3D11 fences and
//! drains the compute queue on drop so a final `Signal` queued by [`Lent::drop`]
//! completes before the fences, context or imported ring textures are freed.

use super::{Consumer, Device, Section};
use anyhow::{Context, Result};
use rubylight_vdisplay_proto::ring::{Header, MAX_SLOTS, SlotState};
use std::{
    os::windows::io::OwnedHandle,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use windows::{
    Win32::Graphics::{
        Direct3D11::{ID3D11Device5, ID3D11DeviceContext4, ID3D11Fence, ID3D11Texture2D},
        Direct3D12::ID3D12Fence,
    },
    core::Interface,
};

/// A fence ring's fences, and how its slots are freed. The session and every image lent from
/// the ring hold it, so a slot is freed even after the session closed.
pub(super) struct Fences {
    section: Arc<Section>,
    pub(super) compute: Arc<crate::compute::Compute>,
    /// Imports stay available while any lent image can still be converted, even after the
    /// session replaces the ring. Also rolls back a partially constructed ring.
    pub(super) imports: Mutex<Vec<ID3D11Texture2D>>,
    /// A failed ordering operation quarantines this ring. Never free a slot whose GPU
    /// readers may not be covered by its completion fence.
    pub(super) failed: AtomicBool,
    /// The driver signals it with a frame's sequence once its copy is done.
    pub(super) written: ID3D12Fence,
    pub(super) written11: ID3D11Fence,
    /// Signalled with each slot's release value after the host's last read of the slot.
    released: ID3D12Fence,
    released11: ID3D11Fence,
    /// For the driver: `[written, released]`.
    pub(super) handles: [OwnedHandle; 2],
    pub(super) context: ID3D11DeviceContext4,
    /// D3D11 reads of a lent texture reach the compute queue through it, so the released
    /// signal follows them.
    bridge: (ID3D12Fence, ID3D11Fence),
    /// Whose queue signals the released fence: the compute queue when the ring is read in
    /// place, else the capture's D3D11 device. Always the same one, so the values reach the
    /// fence in the order they were handed out.
    on_compute: bool,
    /// The last release and bridge values handed out.
    values: Mutex<(u64, u64)>,
}

// SAFETY: the D3D12 fences and the compute queue are free-threaded, the D3D11 context's device
// is multithread-protected (`capture::Device::create`), and the values are locked.
unsafe impl Send for Fences {}
// SAFETY: as above.
unsafe impl Sync for Fences {}

impl Fences {
    fn fail_ordering(&self) {
        self.failed.store(true, Ordering::Release);
        if self.on_compute {
            // Earlier readers may have queued a written/bridge wait whose matching
            // signal is now uncertain. A fresh D3D11 device alone cannot retire it.
            self.compute.retire();
        }
    }

    pub(super) fn order_d3d11(&self, seq: u32) -> Result<()> {
        // SAFETY: the shared fence is opened on this multithread-protected context's device.
        let waited = unsafe { self.context.Wait(&self.written11, u64::from(seq)) };
        if waited.is_err() {
            self.fail_ordering();
        }
        waited.context("order a driver frame's graphics reader")
    }

    pub(super) fn new(
        gpu: &Device,
        compute: Arc<crate::compute::Compute>,
        section: Arc<Section>,
        on_compute: bool,
    ) -> Result<Self> {
        let device: ID3D11Device5 = gpu.device.cast()?;
        let (written, written11, written_handle) = compute.share_fence(&device)?;
        let (released, released11, released_handle) = compute.share_fence(&device)?;
        let (bridge, bridge11, _) = compute.share_fence(&device)?;
        Ok(Self {
            section,
            compute,
            imports: Mutex::new(Vec::new()),
            failed: AtomicBool::new(false),
            written,
            written11,
            released,
            released11,
            handles: [written_handle, released_handle],
            context: gpu.context.cast()?,
            bridge: (bridge, bridge11),
            on_compute,
            values: Mutex::new((0, 0)),
        })
    }

    /// Free `slot`, claimed for frame `seq`, behind every read of it submitted so far: on the
    /// compute queue for a ring read in place (and on the D3D11 device too when `d3d11_read`),
    /// else on the D3D11 device. When `on_compute` and neither queue read the slot, release it
    /// immediately with fence value `0` without touching any GPU queue.
    pub(super) fn free(
        &self,
        slot: usize,
        seq: u32,
        d3d11_read: bool,
        compute_read: bool,
    ) -> Result<()> {
        let mut values = self.values.lock().unwrap();
        anyhow::ensure!(
            !self.failed.load(Ordering::Acquire),
            "frame ring ordering failed"
        );
        let release = if self.on_compute && !compute_read && !d3d11_read {
            // Never read by either GPU queue: the driver's `claim_fenced` treats
            // `release == 0` as already completed (`0 <= released`), avoiding a
            // compute-queue Signal per discarded high-refresh frame.
            0
        } else {
            values.0 += 1;
            let release = values.0;
            // SAFETY: every fence was created on the compute device and opened on this context's
            // device; the context is multithread-protected and the queue free-threaded.
            let signalled = unsafe {
                if self.on_compute {
                    let bridged = if d3d11_read {
                        values.1 += 1;
                        self.context
                            .Signal(&self.bridge.1, values.1)
                            .map(|()| self.context.Flush())
                            .and_then(|()| self.compute.queue.Wait(&self.bridge.0, values.1))
                    } else {
                        Ok(())
                    };
                    bridged.and_then(|()| self.compute.queue.Signal(&self.released, release))
                } else {
                    self.context
                        .Signal(&self.released11, release)
                        .map(|()| self.context.Flush())
                }
            };
            // A failed signal does not prove the reads completed. Keep the slot owned until
            // Session retires this ring; a later signal must not make an unsafe slot reusable.
            if let Err(error) = signalled {
                self.fail_ordering();
                return Err(error).context("free a Rubylight frame ring slot");
            }
            release
        };
        let freed = Consumer::release(&self.section.header(), slot, seq, release);
        drop(values);
        if !freed {
            self.fail_ordering();
        }
        anyhow::ensure!(
            freed,
            "Rubylight frame ring slot {slot} was not the host's to free"
        );
        Ok(())
    }
    /// Highest driver-published or in-flight `written` fence sequence visible in
    /// the shared header for this ring.
    pub(super) fn latest_written_seq(&self) -> u64 {
        let header = self.section.header();
        let slots = header
            .layout()
            .map_or(MAX_SLOTS, |layout| layout.slot_count as usize);
        latest_written_seq_in(&header, slots)
    }

    /// CPU-wait (bounded by `compute::wait_fence`'s 2 s timeout) for the driver's
    /// latest published GPU copy on this ring to signal `written`, so the host
    /// never releases D3D12 shared textures/fences while WUDFHost's D3D11 GPU
    /// copy is still in flight (Rubylight #148).
    pub(super) fn wait_written(&self) {
        let target = self.latest_written_seq();
        if target == 0 {
            return;
        }
        if let Err(error) = crate::compute::wait_fence(&self.written, target) {
            tracing::debug!(
                target,
                error = %format!("{error:#}"),
                "waiting for driver written fence before releasing frame ring"
            );
        }
    }
}

/// Highest sequence number written or being written into `header` across
/// `W_LATEST`, each slot's `SlotMeta::seq`, and each slot's atomic `SlotState`.
pub(super) fn latest_written_seq_in(header: &Header<'_>, slot_count: usize) -> u64 {
    let mut max_seq = header.latest().map_or(0, |token| u64::from(token.seq));
    for slot in 0..slot_count.min(MAX_SLOTS) {
        max_seq = max_seq.max(header.slot(slot).seq);
        let state_seq = match header.slot_state(slot) {
            SlotState::Writing { seq }
            | SlotState::Published { seq }
            | SlotState::Reading { seq } => u64::from(seq),
            SlotState::Free { .. } | SlotState::Invalid(_) => 0,
        };
        max_seq = max_seq.max(state_seq);
    }
    max_seq
}

impl Drop for Fences {
    fn drop(&mut self) {
        // The last lent image's `Lent::drop` calls `Fences::free`, queuing
        // `compute.queue.Signal(&self.released, release)` (and optionally
        // `Wait(&self.bridge.0, ...)`) right before `Fences` drops. Also wait
        // on the driver's latest `written` fence value before freeing the
        // shared D3D12 textures, fences, context or compute device (#148).
        let mut work = crate::compute::RetainedWork::new(&self.compute, &self.compute.queue);
        let written_target = self.latest_written_seq();
        if written_target > 0 {
            work.waits.push((self.written.clone(), written_target));
        }
        work.keep(&self.written);
        work.keep(&self.written11);
        work.keep(&self.released);
        work.keep(&self.released11);
        work.keep(&self.bridge.0);
        work.keep(&self.bridge.1);
        work.keep(&self.context);
        for texture in self.imports.get_mut().unwrap().drain(..) {
            work.keep(&texture);
            for (d3d11, d3d12) in self.compute.take_forgotten(&texture) {
                work.keep(&d3d11);
                work.keep(&d3d12);
            }
        }
        work.finish(&self.compute);
    }
}

/// A fence-ring slot an image reads in place. The slot is freed when this drops, with the
/// image's last clone.
pub(crate) struct Lent {
    pub(super) fences: Arc<Fences>,
    pub(super) slot: usize,
    pub(super) seq: u32,
    /// Whether a D3D11 reader waited for the driver's copy.
    pub(super) d3d11: Mutex<bool>,
    /// Shared with the image's `Ready`: true once any D3D12 compute reader queued a read.
    pub(super) compute_read: Arc<AtomicBool>,
}

impl Lent {
    /// A bounded reader wait failed. Retain this slot and let capture reopen on the
    /// standard transport without treating the unrelated compute queue as broken.
    pub(crate) fn reject(&self) {
        self.fences.failed.store(true, Ordering::Release);
    }

    /// Make the capture's D3D11 device wait for the driver's copy before any D3D11 read of
    /// the texture. Only readers other than the compute conversion need it.
    pub(crate) fn order_d3d11(&self) -> Result<()> {
        let mut waited = self.d3d11.lock().unwrap();
        if !*waited {
            self.fences.order_d3d11(self.seq)?;
            *waited = true;
        }
        Ok(())
    }
}

impl Drop for Lent {
    fn drop(&mut self) {
        let d3d11 = *self.d3d11.lock().unwrap();
        let compute = self.compute_read.load(Ordering::Acquire);
        if let Err(error) = self.fences.free(self.slot, self.seq, d3d11, compute) {
            tracing::debug!(error = %format!("{error:#}"), slot = self.slot, "Rubylight frame ring slot retained after a GPU ordering error");
        }
    }
}
