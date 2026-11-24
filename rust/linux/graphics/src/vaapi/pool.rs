//! Surfaces the stream owns: a pool of same-sized surfaces that hands each out as a [`Pooled`]
//! and takes it back when the last holder drops it. Capture converts every frame into one, so a
//! frame stays valid for as long as the stream threads keep it, long after the PipeWire buffer
//! went back to the producer.
use super::{display::Va, ffi::*};
use anyhow::Result;
use std::sync::{
    Arc, Mutex, PoisonError,
    atomic::{AtomicUsize, Ordering},
};

/// Same-sized surfaces of one pixel format.
pub struct Pool {
    va: Arc<Va>,
    rt_format: u32,
    pixel: u32,
    /// The size surfaces are made with: the picture's size rounded up to whole 16-pixel blocks,
    /// as encoders read it.
    pub width: u32,
    pub height: u32,
    free: Mutex<Vec<VASurfaceID>>,
    /// Surfaces made so far, free or in use.
    made: AtomicUsize,
    limit: usize,
}

/// A surface on loan from a [`Pool`].
pub struct Pooled {
    pool: Arc<Pool>,
    id: VASurfaceID,
}

/// Rounds a picture dimension up to a whole number of 16-pixel blocks.
pub fn aligned(size: u32) -> u32 {
    size.next_multiple_of(16)
}

impl Pool {
    /// A pool of at most `limit` surfaces for pictures of `width`x`height`, `first` of them made
    /// now.
    pub fn new(
        va: &Arc<Va>,
        rt_format: u32,
        pixel: u32,
        width: u32,
        height: u32,
        first: usize,
        limit: usize,
    ) -> Result<Arc<Self>> {
        let (width, height) = (aligned(width), aligned(height));
        let free = if first > 0 {
            va.create_surfaces(rt_format, pixel, width, height, first)?
        } else {
            Vec::new()
        };
        Ok(Arc::new(Self {
            va: va.clone(),
            rt_format,
            pixel,
            width,
            height,
            made: AtomicUsize::new(free.len()),
            free: Mutex::new(free),
            limit,
        }))
    }

    /// A free surface, a new one while the pool is under its limit, or `None` when all are in
    /// use.
    pub fn take(self: &Arc<Self>) -> Option<Pooled> {
        let reused = self
            .free
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop();
        let id = match reused {
            Some(id) => id,
            None => {
                if self.made.load(Ordering::Relaxed) >= self.limit {
                    return None;
                }
                let made = self
                    .va
                    .create_surfaces(self.rt_format, self.pixel, self.width, self.height, 1)
                    .inspect_err(|error| tracing::warn!(%error, "a surface could not be made"))
                    .ok()?;
                self.made.fetch_add(1, Ordering::Relaxed);
                made[0]
            }
        };
        Some(Pooled {
            pool: self.clone(),
            id,
        })
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        let mut free =
            std::mem::take(&mut *self.free.lock().unwrap_or_else(PoisonError::into_inner));
        self.va.destroy_surfaces(&mut free);
    }
}

impl Pooled {
    pub fn id(&self) -> VASurfaceID {
        self.id
    }
    pub fn va(&self) -> &Arc<Va> {
        &self.pool.va
    }
}

impl Drop for Pooled {
    fn drop(&mut self) {
        self.pool
            .free
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(self.id);
    }
}
