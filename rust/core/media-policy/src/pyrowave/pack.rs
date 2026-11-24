//! Reusable scratch buffers, flat intrusive-list first-fit packing, and
//! linear-time band counting sort for PyroWave record framing.
use super::{Record, Sink};
use anyhow::{Context, Result};
use std::cell::RefCell;

/// Reusable scratch storage kept per thread across PyroWave frames so record
/// framing allocates no `VecDeque` buckets, segment trees, placement maps, or
/// sort buffers per frame.
#[derive(Default)]
struct FrameScratch {
    /// Bucket heads for the flat intrusive singly-linked list (index into `ordinary`).
    head: Vec<usize>,
    /// Bucket tails used while building the linked list.
    tail: Vec<usize>,
    /// Intrusive next pointers, parallel to `ordinary`.
    next: Vec<usize>,
    /// Iterative min-segment tree over `head[..size]`.
    tree: Vec<usize>,
    /// Placement bitset used only when row-order framing has records over one packet.
    placed: Vec<bool>,
    /// Oversized records deferred behind the ordinary records ranked before them.
    bigs: Vec<(usize, Record)>,
    /// Step histogram / write offsets for counting sort.
    counts: Vec<usize>,
    /// Bucket boundaries `(start, end)` per step when input was not in index order.
    bounds: Vec<(usize, usize)>,
    /// Temporary buffer for counting-sorted fine records.
    sorted: Vec<Record>,
}

thread_local! {
    static SCRATCH: RefCell<FrameScratch> = RefCell::new(FrameScratch::default());
}

impl FrameScratch {
    /// Populate the flat intrusive linked list and segment tree for `records`.
    /// Preserves strict FIFO encoder order within every size bucket.
    fn build_fits(&mut self, records: &[Record]) -> usize {
        let size = (records.iter().map(|r| r.size / 4).max().unwrap_or(0) + 1).next_power_of_two();
        self.head.clear();
        self.head.resize(size, usize::MAX);
        self.tail.clear();
        self.tail.resize(size, usize::MAX);
        self.next.clear();
        self.next.resize(records.len(), usize::MAX);
        for (i, r) in records.iter().enumerate() {
            let bucket = r.size / 4;
            let prev = self.tail[bucket];
            if prev == usize::MAX {
                self.head[bucket] = i;
            } else {
                self.next[prev] = i;
            }
            self.tail[bucket] = i;
        }
        self.tree.clear();
        self.tree.resize(size * 2, usize::MAX);
        self.tree[size..size * 2].copy_from_slice(&self.head[..size]);
        for i in (1..size).rev() {
            self.tree[i] = self.tree[2 * i].min(self.tree[2 * i + 1]);
        }
        size
    }

    #[inline]
    fn fitting(&self, size: usize, words: usize) -> usize {
        let (mut lo, mut hi, mut result) = (size, size + words.min(size - 1) + 1, usize::MAX);
        while lo < hi {
            if lo & 1 != 0 {
                result = result.min(self.tree[lo]);
                lo += 1;
            }
            if hi & 1 != 0 {
                hi -= 1;
                result = result.min(self.tree[hi]);
            }
            lo /= 2;
            hi /= 2;
        }
        result
    }

    #[inline]
    fn exact(&self, size: usize, words: usize) -> usize {
        if words < size {
            self.tree[size + words]
        } else {
            usize::MAX
        }
    }

    #[inline]
    fn remove(&mut self, size: usize, words: usize) {
        let front = self.head[words];
        let nxt = self.next[front];
        self.head[words] = nxt;
        let mut at = size + words;
        self.tree[at] = nxt;
        while at > 1 {
            at /= 2;
            let first = self.tree[2 * at].min(self.tree[2 * at + 1]);
            if self.tree[at] == first {
                break;
            }
            self.tree[at] = first;
        }
    }

    fn first_fit_inner(
        &mut self,
        out: &mut impl Sink,
        bitstream: &[u8],
        ordinary: &[Record],
        use_bigs: bool,
        payload: usize,
    ) {
        let size = self.build_fits(ordinary);
        // Only a frame with records over a packet in row order needs to know what is placed.
        let has_bigs = use_bigs && !self.bigs.is_empty();
        self.placed.clear();
        if has_bigs {
            self.placed.resize(ordinary.len(), false);
        }
        let (mut front, mut next_big) = (0, 0);
        for _ in 0..ordinary.len() {
            if has_bigs {
                while front < self.placed.len() && self.placed[front] {
                    front += 1;
                }
                while next_big < self.bigs.len() && self.bigs[next_big].0 <= front {
                    let r = self.bigs[next_big].1;
                    big(out, bitstream, &r, payload);
                    next_big += 1;
                }
            }
            loop {
                let room = payload - (out.written() + 8) % payload;
                let mut chosen = self.fitting(size, room / 4);
                if chosen != usize::MAX && room - ordinary[chosen].size == 4 {
                    chosen = self.exact(size, room / 4).min(if room >= 16 {
                        self.fitting(size, (room - 8) / 4)
                    } else {
                        usize::MAX
                    });
                }
                if chosen == usize::MAX {
                    out.pad(room);
                    continue;
                }
                let r = ordinary[chosen];
                self.remove(size, r.size / 4);
                if has_bigs {
                    self.placed[chosen] = true;
                }
                out.copy(bitstream, r.offset, r.size);
                break;
            }
        }
        if has_bigs {
            for i in next_big..self.bigs.len() {
                let r = self.bigs[i].1;
                big(out, bitstream, &r, payload);
            }
        }
    }
}

pub(super) fn big(out: &mut impl Sink, bitstream: &[u8], r: &Record, payload: usize) {
    let room = payload - (out.written() + 8) % payload;
    if r.size % payload == room - 4 {
        out.pad(8);
    }
    out.copy(bitstream, r.offset, r.size);
}

/// Packs `ordinary` first-fit using the thread's reusable scratch buffers.
pub(super) fn first_fit(
    out: &mut impl Sink,
    bitstream: &[u8],
    ordinary: &[Record],
    payload: usize,
) {
    SCRATCH.with(|cell| {
        cell.borrow_mut()
            .first_fit_inner(out, bitstream, ordinary, false, payload);
    });
}

/// Sorts the fine records (`r.index >= coarse`) by `(steps[r.index], r.index)`
/// in O(N + bands) via counting sort (preserving exact stable order even for
/// unsorted inputs) and packs them with first-fit.
pub(super) fn pack_row_order(
    out: &mut impl Sink,
    bitstream: &[u8],
    all: &[Record],
    coarse: u32,
    steps: &[u16],
    ordinary: &mut Vec<Record>,
    payload: usize,
) -> Result<()> {
    SCRATCH.with(|cell| {
        let mut scratch = cell.borrow_mut();
        let FrameScratch {
            counts,
            bounds,
            sorted,
            bigs,
            ..
        } = &mut *scratch;
        counts.clear();
        let mut max_step = 0usize;
        let mut fine_count = 0usize;
        let mut prev_index = 0u32;
        let mut by_index = true;
        for r in all.iter().filter(|r| r.index >= coarse) {
            let step = *steps
                .get(r.index as usize)
                .context("PyroWave block index exceeds the picture")?
                as usize;
            if fine_count > 0 && r.index < prev_index {
                by_index = false;
            }
            prev_index = r.index;
            fine_count += 1;
            if step >= counts.len() {
                counts.resize(step + 1, 0);
            }
            counts[step] += 1;
            max_step = max_step.max(step);
        }
        if fine_count == 0 {
            return Ok(());
        }
        let bands = max_step + 1;
        bounds.clear();
        bounds.reserve(bands);
        let mut running = 0usize;
        for count in counts.iter_mut().take(bands) {
            let c = *count;
            *count = running;
            bounds.push((running, running + c));
            running += c;
        }
        sorted.clear();
        sorted.resize(
            fine_count,
            Record {
                offset: 0,
                size: 0,
                index: 0,
            },
        );
        for &r in all.iter().filter(|r| r.index >= coarse) {
            let step = steps[r.index as usize] as usize;
            let dst = counts[step];
            counts[step] = dst + 1;
            sorted[dst] = r;
        }
        if !by_index {
            for &(start, end) in bounds.iter() {
                if end - start > 1 && !sorted[start..end].is_sorted_by_key(|r| r.index) {
                    sorted[start..end].sort_by_key(|r| r.index);
                }
            }
        }
        ordinary.clear();
        bigs.clear();
        for &r in sorted.iter() {
            if r.size > payload - 8 {
                bigs.push((ordinary.len(), r));
            } else {
                ordinary.push(r);
            }
        }
        scratch.first_fit_inner(out, bitstream, ordinary, true, payload);
        Ok(())
    })
}
