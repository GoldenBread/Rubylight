//! PyroWave FEC block planning, parity calculation, and static-detail FEC control.

use super::{Record, records};
use anyhow::{Result, bail};
use std::{collections::HashMap, hash::BuildHasherDefault, time::Instant};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct FecBlock {
    pub data: usize,
    pub percentage: usize,
}
pub fn parity(data: usize, percentage: usize, minimum: usize) -> usize {
    fec_parameters(data, percentage, minimum).0
}
/// The parity count must be exactly derivable from the percentage on the wire:
/// it also selects the receiver's Reed-Solomon matrix, not just its packet count.
pub(crate) fn fec_parameters(data: usize, percentage: usize, minimum: usize) -> (usize, usize) {
    if data == 0 || percentage == 0 {
        return (0, percentage);
    }
    let fec = (data * percentage).div_ceil(100);
    let minimum = minimum_parity(data, minimum);
    if fec >= minimum {
        return (fec, percentage);
    }
    let mut effective = 100 * minimum / data;
    if (data * effective).div_ceil(100) < minimum {
        effective += 1;
    }
    ((data * effective).div_ceil(100), effective)
}
/// A client's minimum parity, capped where the 8-bit FEC percentage that
/// carries it can still describe it: 3 parity shards for 1 data shard would
/// need 300% and spill into the shard index bits.
pub fn minimum_parity(data: usize, minimum: usize) -> usize {
    minimum.min(data * 255 / 100)
}
fn split_into(push: &mut impl FnMut(FecBlock), data: usize, slots: usize) {
    if data == 0 {
        return;
    }
    let count = data.div_ceil(255).clamp(1, slots);
    let size = data.div_ceil(count);
    for i in 0..count {
        push(FecBlock {
            data: if i + 1 == count {
                data - size * i
            } else {
                size
            },
            percentage: 0,
        });
    }
}
#[cfg(test)]
pub(super) fn split(blocks: &mut Vec<FecBlock>, data: usize, slots: usize) {
    split_into(&mut |block| blocks.push(block), data, slots);
}
pub fn plan(
    total: usize,
    critical: usize,
    rate: usize,
    minimum: usize,
    detail: usize,
    extra: usize,
) -> Result<Vec<FecBlock>> {
    plan_inline(total, critical, rate, minimum, detail, extra).map(|blocks| blocks.to_vec())
}
/// [`plan`] into a stack-allocated [`crate::packet::InlineBlocks`], avoiding
/// heap allocations in the inner candidate loop and return value.
pub(crate) fn plan_inline(
    total: usize,
    critical: usize,
    rate: usize,
    minimum: usize,
    detail: usize,
    extra: usize,
) -> Result<crate::packet::InlineBlocks<FecBlock>> {
    if total == 0 || total > 4000 || rate > 255 || minimum > 255 {
        bail!("invalid PyroWave FEC plan");
    }
    let critical = critical.min(total);
    let mut out = crate::packet::InlineBlocks::new();
    if critical == 0 || rate == 0 || critical + parity(critical, rate, minimum) > 255 {
        split_into(&mut |b| out.push(b), total, 4);
    } else {
        out.push(FecBlock {
            data: critical,
            percentage: rate,
        });
        let fine = total - critical;
        let detail = detail.min(50);
        let mut extra = extra.min(parity(fine, detail, 0));
        let capacity = |rate| {
            (1..=255)
                .rev()
                .find(|&data| data + parity(data, rate, minimum) <= 255)
                .unwrap_or(0)
        };
        for percentage in (1..=detail).rev() {
            let cap = capacity(percentage);
            if cap == 0 || fine == 0 {
                continue;
            }
            let count = fine.div_ceil(cap);
            if count > 3 {
                continue;
            }
            let mut candidate = [FecBlock::default(); 3];
            for (i, slot) in candidate.iter_mut().take(count).enumerate() {
                *slot = FecBlock {
                    data: fine / count + usize::from(i < fine % count),
                    percentage,
                };
            }
            // Each candidate block's parity is computed once, for both its fit
            // and its sum.
            let mut parities = [0usize; 3];
            for (slot, block) in parities.iter_mut().zip(&candidate[..count]) {
                *slot = parity(block.data, block.percentage, minimum);
            }
            if candidate[..count]
                .iter()
                .zip(parities)
                .all(|(block, parity)| block.data + parity <= 255)
                && parities[..count].iter().sum::<usize>() <= extra
            {
                for &block in &candidate[..count] {
                    out.push(block);
                }
                return Ok(out);
            }
        }
        let base = parity(critical, rate, minimum);
        if detail > 0 && extra > 0 {
            // The critical block grows into the wire room `extra` leaves, one
            // shard at a time: each candidate shard's parity is computed once,
            // for both conditions and the room it takes.
            let limit = total.min(capacity(rate));
            let mut leading = critical;
            let mut at_leading = base;
            let mut next = parity(critical + 1, rate, minimum);
            while leading < limit && leading + 1 + next <= 255 && next.saturating_sub(base) <= extra
            {
                leading += 1;
                at_leading = next;
                next = parity(leading + 1, rate, minimum);
            }
            out[0].data = leading;
            extra -= at_leading.saturating_sub(base);
            let cap = capacity(detail);
            let mut remaining = total - leading;
            while remaining > 0 && out.len() < 4 {
                // The first block that fits the room its parity leaves closes
                // the block; its parity is computed once for the walk, the
                // limit and the room it takes.
                let mut data = remaining.min(cap);
                let mut at_data = if data > 0 {
                    parity(data, detail, minimum)
                } else {
                    0
                };
                while data > 0 && (at_data > extra || data + at_data > 255) {
                    data -= 1;
                    at_data = if data > 0 {
                        parity(data, detail, minimum)
                    } else {
                        0
                    };
                }
                if data == 0 || remaining - data > (3 - out.len()) * 1023 {
                    break;
                }
                out.push(FecBlock {
                    data,
                    percentage: detail,
                });
                extra -= at_data;
                remaining -= data;
            }
            if remaining > 0 {
                let slots = 4 - out.len();
                split_into(&mut |b| out.push(b), remaining, slots);
            }
        } else {
            split_into(&mut |b| out.push(b), fine, 3);
        }
    }
    if out.len() > 4
        || out.iter().any(|b| {
            b.data > 1023
                || (b.percentage > 0 && b.data + parity(b.data, b.percentage, minimum) > 255)
        })
    {
        bail!("PyroWave frame exceeds FEC capacity");
    }
    Ok(out)
}
/// A record's contents without the sequence bits of its fourth byte, which
/// change every frame. Eight bytes a step: a byte at a time took 0.9 ms of a
/// 0.83 MB frame (800 Mbps at 120 fps) before its first packet could leave.
pub(super) fn record_hash(bytes: &[u8]) -> u64 {
    let mix = |h: u64, word: u64| {
        (h ^ word)
            .wrapping_mul(0x9e37_79b9_7f4a_7c15)
            .rotate_left(29)
    };
    let (words, rest) = bytes.as_chunks::<8>();
    let mut h = 0xcbf2_9ce4_8422_2325u64 ^ bytes.len() as u64;
    for (i, word) in words.iter().enumerate() {
        let word = u64::from_le_bytes(*word);
        h = mix(h, if i == 0 { word & !(0x70 << 24) } else { word });
    }
    let start = words.len() * 8;
    for (i, &b) in rest.iter().enumerate() {
        h = mix(h, u64::from(if start + i == 3 { b & 0x8f } else { b }));
    }
    h
}
/// No idle credit: detail protection is bounded by this frame's allowance.
pub struct DetailFec {
    nominal: f64,
    previous: Option<Instant>,
    interval: f64,
    stable: f64,
    /// The last late frame's record hashes by index, and the one being built:
    /// kept and swapped instead of a fresh map for every late frame, whose
    /// table a 4K picture's records size to about a megabyte.
    seen: RecordMap,
    building: RecordMap,
}
/// Record hashes by index, hashed as [`IndexHasher`] hashes them.
type RecordMap = HashMap<u32, (u64, usize), BuildHasherDefault<IndexHasher>>;
/// A record index hashed as itself spread by a Fibonacci multiplier: two
/// lookups a record, where SipHash spends its full word round on each.
#[derive(Default)]
struct IndexHasher(u64);
impl std::hash::Hasher for IndexHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for (at, &byte) in bytes.iter().enumerate() {
            self.0 = self
                .0
                .wrapping_mul(31)
                .wrapping_add(u64::from(byte) + at as u64);
        }
    }
    fn write_u32(&mut self, index: u32) {
        self.0 = u64::from(index).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    }
}
impl DetailFec {
    pub fn new(fps_millihz: u32) -> Self {
        Self {
            nominal: 1000. / f64::from(fps_millihz.max(1)),
            previous: None,
            interval: 0.,
            stable: 0.,
            seen: RecordMap::default(),
            building: RecordMap::default(),
        }
    }
    pub fn observe(
        &mut self,
        frame: &[u8],
        records: Option<&[Record]>,
        now: Instant,
        kbps: u32,
    ) -> Result<(usize, usize)> {
        let elapsed = self
            .previous
            .replace(now)
            .map(|p| now.saturating_duration_since(p).as_secs_f64())
            .unwrap_or(0.);
        // Require >15% slowdown past nominal cadence so normal ~1-5% frame-time
        // jitter at high refresh rates (e.g. 83 us at 120 Hz) never triggers a
        // full-frame hash pass on moving gameplay.
        if elapsed <= self.nominal * 1.15 {
            self.seen.clear();
            self.stable = 0.;
            self.interval = elapsed;
            return Ok((0, 0));
        }
        // The codec's own records when the packetizer has them from the
        // framing pass; a picture that arrives without its framing is parsed.
        let parsed;
        let records: &[Record] = match records {
            Some(records) => records,
            None => {
                parsed = self::records(frame)?;
                &parsed
            }
        };
        if self.building.capacity() < records.len() {
            self.building.reserve(records.len());
        }
        self.building.clear();
        let (mut total, mut unchanged) = (0, 0);
        for r in records.iter() {
            let hash = record_hash(&frame[r.offset..r.offset + r.size]);
            if self.seen.get(&r.index) == Some(&(hash, r.size)) {
                unchanged += r.size;
            }
            total += r.size;
            self.building.insert(r.index, (hash, r.size));
        }
        std::mem::swap(&mut self.seen, &mut self.building);
        self.interval = if self.interval > 0. {
            self.interval * 0.8 + elapsed * 0.2
        } else {
            elapsed
        };
        if total == 0 || unchanged * 4 < total * 3 {
            self.stable = 0.;
            return Ok((0, 0));
        }
        self.stable += elapsed;
        if self.stable < 0.25 {
            return Ok((0, 0));
        }
        Ok((
            (100. * (self.interval / self.nominal - 1.)).clamp(0., 50.) as usize,
            (f64::from(kbps) * 125. * elapsed.min(self.interval).min(0.05)) as usize,
        ))
    }
}
