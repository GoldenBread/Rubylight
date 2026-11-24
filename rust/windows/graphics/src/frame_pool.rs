//! Reused buffers for large encoded frames.
//!
//! From around a megabyte, the Windows heap maps every block with its own
//! VirtualAlloc and returns it with VirtualFree: a new keyframe or PyroWave
//! picture of that size costs a page fault per 4 KB as it is written, and the
//! free costs again on the sender's thread that drops it. [`Encoded`] gives
//! its buffer back here when it is dropped, and the encoders copy their
//! output into one taken from here, so the same few regions carry the stream.
//!
//! [`Encoded`]: crate::encoder::Encoded
use std::sync::{Mutex, PoisonError};

/// Smaller buffers come from the heap as ever: it serves them from its own
/// segments without mapping them.
pub const POOLED: usize = 512 * 1024;
/// More than one frame encoding, one waiting and one being sent.
const KEPT: usize = 4;
/// A buffer past this is freed rather than kept: an exceptional frame must
/// not hold its memory for the rest of the process.
const LARGEST: usize = 16 * 1024 * 1024;

static POOL: Mutex<Pool> = Mutex::new(Pool(Vec::new()));

/// An empty buffer that holds at least `len` bytes, reused when `len` is
/// large enough to be worth it.
pub fn take(len: usize) -> Vec<u8> {
    if len < POOLED {
        return Vec::with_capacity(len);
    }
    let mut buffer = shared(len).unwrap_or_else(|| Vec::with_capacity(len));
    buffer.clear();
    buffer
}

/// A buffer of `len` bytes for a writer that overwrites them all, such as
/// PyroWave's packetizer: a reused one keeps its last frame's bytes, so only
/// what it never held is zeroed, where [`take`] and a resize zeroed it all.
pub fn take_filled(len: usize) -> Vec<u8> {
    filled(if len < POOLED { None } else { shared(len) }, len)
}

fn shared(len: usize) -> Option<Vec<u8>> {
    POOL.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take(len)
}

/// `reused`, or a new buffer, holding `len` bytes.
fn filled(reused: Option<Vec<u8>>, len: usize) -> Vec<u8> {
    let mut buffer = reused.unwrap_or_else(|| Vec::with_capacity(len));
    if buffer.len() >= len {
        buffer.truncate(len);
    } else {
        buffer.resize(len, 0);
    }
    buffer
}

/// `bytes` in a buffer from [`take`].
pub fn copy(bytes: &[u8]) -> Vec<u8> {
    let mut buffer = take(bytes.len());
    buffer.extend_from_slice(bytes);
    buffer
}

/// `bytes` in a buffer from [`take`] with room for at least `headroom` more
/// bytes, so a caller that splices or appends metadata does not reallocate.
pub fn copy_with_headroom(bytes: &[u8], headroom: usize) -> Vec<u8> {
    let mut buffer = take(bytes.len().saturating_add(headroom));
    buffer.extend_from_slice(bytes);
    buffer
}

/// `bytes` in a buffer from [`take`] with `prefix` spliced in at `at` and
/// `suffix` appended at the end, written linearly in one pass without
/// reallocation or shifting. `at` past `bytes.len()` is clamped to the end.
pub fn copy_spliced(bytes: &[u8], at: usize, prefix: &[u8], suffix: &[u8]) -> Vec<u8> {
    let at = at.min(bytes.len());
    let total = bytes
        .len()
        .saturating_add(prefix.len())
        .saturating_add(suffix.len());
    let mut buffer = take(total);
    buffer.extend_from_slice(&bytes[..at]);
    buffer.extend_from_slice(prefix);
    buffer.extend_from_slice(&bytes[at..]);
    buffer.extend_from_slice(suffix);
    buffer
}

/// Keep `buffer` for a later [`take`], if it is large and the pool has room;
/// otherwise it is freed.
pub fn give(buffer: Vec<u8>) {
    if !(POOLED..=LARGEST).contains(&buffer.capacity()) {
        return;
    }
    // Freed, if it is, outside the lock.
    let _freed = POOL
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .give(buffer);
}

/// Free every kept buffer, as a stream ends: an idle host holds none.
pub fn clear() {
    let freed = std::mem::take(&mut POOL.lock().unwrap_or_else(PoisonError::into_inner).0);
    drop(freed);
}

/// [`clear`] once no stream is left to reuse the buffers. A session calls it
/// after its [`StreamingScope`] has ended and its encoder and senders have
/// given their buffers back, so the last stream to end empties the pool and
/// one ending beside another leaves the other its buffers.
///
/// [`StreamingScope`]: crate::timing::StreamingScope
pub fn clear_after_stream() {
    clear_unless_streaming(&POOL, crate::timing::StreamingScope::active());
}

fn clear_unless_streaming(pool: &Mutex<Pool>, streams: usize) {
    if streams == 0 {
        let freed = std::mem::take(&mut pool.lock().unwrap_or_else(PoisonError::into_inner).0);
        drop(freed);
    }
}

struct Pool(Vec<Vec<u8>>);

impl Pool {
    /// The smallest kept buffer that fits, so a keyframe's stays for keyframes.
    fn take(&mut self, len: usize) -> Option<Vec<u8>> {
        let index = self
            .0
            .iter()
            .enumerate()
            .filter(|(_, buffer)| buffer.capacity() >= len)
            .min_by_key(|(_, buffer)| buffer.capacity())
            .map(|(index, _)| index)?;
        Some(self.0.swap_remove(index))
    }
    /// Keeps `buffer` with the bytes it holds ([`take_filled`] reuses them;
    /// [`take`] empties it); returns the buffer that does not fit, if any.
    /// A full pool keeps its larger buffers: a smaller frame fits in them,
    /// not the reverse.
    fn give(&mut self, mut buffer: Vec<u8>) -> Option<Vec<u8>> {
        if self.0.len() < KEPT {
            self.0.push(buffer);
            return None;
        }
        if let Some(smallest) = self
            .0
            .iter_mut()
            .min_by_key(|kept| kept.capacity())
            .filter(|kept| kept.capacity() < buffer.capacity())
        {
            std::mem::swap(smallest, &mut buffer);
        }
        Some(buffer)
    }
}

/// Held by a test while it takes from or gives to the shared pool: tests run side by side, and one
/// that counts on what the pool holds must not have another take it.
#[cfg(test)]
pub(crate) fn alone() -> std::sync::MutexGuard<'static, ()> {
    static ALONE: Mutex<()> = Mutex::new(());
    ALONE.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_large_buffer_is_reused_by_the_next_frame_that_fits() {
        let _alone = alone();
        let mut pool = Pool(Vec::new());
        let frame = copy(&vec![1; POOLED + 10]);
        assert_eq!(frame, vec![1; POOLED + 10]);
        let address = frame.as_ptr();
        assert!(pool.give(frame).is_none());
        assert!(pool.take(POOLED + 20_000_000).is_none());
        let again = pool.take(POOLED + 5).unwrap();
        // Kept with its bytes, for `take_filled`; `take` empties it.
        assert_eq!(again, vec![1; POOLED + 10]);
        assert!(again.capacity() >= POOLED + 5);
        assert_eq!(again.as_ptr(), address);
        assert!(pool.take(1).is_none());
    }

    #[test]
    fn the_smallest_fitting_buffer_is_taken_and_a_full_pool_keeps_its_largest() {
        let mut pool = Pool(Vec::new());
        let sizes = [POOLED * 4, POOLED, POOLED * 2, POOLED * 3, POOLED * 5];
        for size in sizes {
            pool.give(Vec::with_capacity(size));
        }
        assert_eq!(pool.0.len(), KEPT);
        assert!(pool.0.iter().all(|kept| kept.capacity() >= POOLED * 2));
        assert_eq!(pool.take(POOLED * 2 + 1).unwrap().capacity(), POOLED * 3);
        // A full pool frees a buffer smaller than all it keeps.
        pool.give(Vec::with_capacity(POOLED * 6));
        let refused = pool.give(Vec::with_capacity(POOLED)).unwrap();
        assert_eq!(refused.capacity(), POOLED);
    }

    #[test]
    fn a_dropped_frame_returns_its_buffer_until_the_pool_is_cleared() {
        let _alone = alone();
        // Larger than any other test's frame, so only this one fits it.
        let size = LARGEST - 4096;
        let frame = crate::encoder::Encoded {
            framing: None,
            bytes: Vec::with_capacity(size),
            unit: crate::encoder::Unit::Whole,
            idr: false,
            after_invalidation: false,
            frame_cap_bytes: None,
            reencoded: false,
            frame_cap_target: None,
            latency: None,
            presentation: None,
        };
        let address = frame.bytes.as_ptr();
        drop(frame);
        let reused = take(size);
        assert_eq!(reused.as_ptr(), address);
        give(reused);
        clear();
        assert!(
            POOL.lock()
                .unwrap()
                .0
                .iter()
                .all(|kept| kept.capacity() < size)
        );
    }

    #[test]
    fn a_stream_ending_beside_another_leaves_the_pool_until_the_last_one_ends() {
        let pool = Mutex::new(Pool(Vec::new()));
        assert!(
            pool.lock()
                .unwrap()
                .give(Vec::with_capacity(POOLED * 2))
                .is_none()
        );
        clear_unless_streaming(&pool, 1);
        let kept = pool.lock().unwrap().take(POOLED * 2).unwrap();
        assert!(pool.lock().unwrap().give(kept).is_none());
        clear_unless_streaming(&pool, 0);
        assert!(pool.lock().unwrap().take(POOLED).is_none());
    }

    #[test]
    fn a_filled_buffer_keeps_the_last_frames_bytes_and_zeroes_only_new_ones() {
        let mut pool = Pool(Vec::new());
        let size = POOLED * 3;
        let mut first = filled(pool.take(size), size);
        assert_eq!(first.len(), size);
        assert!(first.iter().all(|&byte| byte == 0));
        first.fill(7);
        let address = first.as_ptr();
        assert!(pool.give(first).is_none());
        // Shorter: the same memory, its bytes as written.
        let shorter = filled(pool.take(size - 100), size - 100);
        assert_eq!(shorter.as_ptr(), address);
        assert_eq!(shorter.len(), size - 100);
        assert!(shorter.iter().all(|&byte| byte == 7));
        assert!(pool.give(shorter).is_none());
        // Longer than it held: only the new tail is zeroed.
        let longer = filled(pool.take(size - 50), size - 50);
        assert_eq!(longer.as_ptr(), address);
        assert!(longer[..size - 100].iter().all(|&byte| byte == 7));
        assert!(longer[size - 100..].iter().all(|&byte| byte == 0));
        assert!(pool.give(longer).is_none());
        // Small ones come from the heap, zeroed.
        assert_eq!(take_filled(16), [0; 16]);
    }

    #[test]
    fn small_frames_are_not_pooled() {
        let small = take(16);
        assert!(small.capacity() >= 16 && small.capacity() < POOLED);
        assert_eq!(copy(&[7; 16]), [7; 16]);
    }

    #[test]
    fn copy_with_headroom_reserves_extra_capacity_and_reuses_pooled_buffers() {
        let _alone = alone();
        let mut small = copy_with_headroom(b"hello", 128);
        assert_eq!(small, b"hello");
        assert!(small.capacity() >= 5 + 128);
        let ptr = small.as_ptr();
        small.extend_from_slice(&[9u8; 128]);
        assert_eq!(small.as_ptr(), ptr, "no realloc within headroom");

        // Large buffer reuses the pool with room for bytes + headroom.
        clear();
        let size = POOLED + 1024;
        let seeded = Vec::with_capacity(size + 256);
        let seeded_ptr = seeded.as_ptr();
        give(seeded);
        let payload = vec![0x42; size];
        let mut large = copy_with_headroom(&payload, 256);
        assert_eq!(large.as_ptr(), seeded_ptr);
        assert_eq!(large, payload);
        assert!(large.capacity() >= size + 256);
        large.extend_from_slice(&[0x55; 256]);
        assert_eq!(large.as_ptr(), seeded_ptr, "pooled headroom avoids realloc");
        give(large);
        clear();
    }

    #[test]
    fn copy_spliced_writes_linearly_across_all_positions_and_reuses_pool() {
        let _alone = alone();
        assert_eq!(copy_spliced(b"abcdef", 0, b"PP", b"SS"), b"PPabcdefSS");
        assert_eq!(copy_spliced(b"abcdef", 2, b"PP", b"SS"), b"abPPcdefSS");
        assert_eq!(copy_spliced(b"abcdef", 6, b"PP", b"SS"), b"abcdefPPSS");
        assert_eq!(copy_spliced(b"abcdef", 99, b"PP", b"SS"), b"abcdefPPSS");
        assert_eq!(copy_spliced(b"abcdef", 3, b"", b"SS"), b"abcdefSS");
        assert_eq!(copy_spliced(b"abcdef", 3, b"PP", b""), b"abcPPdef");
        assert_eq!(copy_spliced(b"", 5, b"PP", b"SS"), b"PPSS");

        clear();
        let size = POOLED + 512;
        let seeded = Vec::with_capacity(size + 64);
        let seeded_ptr = seeded.as_ptr();
        give(seeded);
        let payload: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
        let prefix = [0xAA; 20];
        let suffix = [0xBB; 30];
        let out = copy_spliced(&payload, 17, &prefix, &suffix);
        assert_eq!(out.as_ptr(), seeded_ptr);
        assert_eq!(&out[..17], &payload[..17]);
        assert_eq!(&out[17..37], &prefix);
        assert_eq!(&out[37..size + 20], &payload[17..]);
        assert_eq!(&out[size + 20..], &suffix);
        give(out);
        clear();
    }
}
