//! Reed-Solomon erasure coding over GF(2^8), bit-compatible with nanors as moonlight-common-c
//! uses it for video and audio FEC.
//!
//! Ported from nanors (MIT, Joseph Calderon) as vendored by moonlight-common-c (GPL-3.0,
//! Cameron Gutman and contributors). The field polynomial is 0x11d and the parity matrix is the
//! Cauchy matrix `p[j][i] = 1 / ((parity + i) ^ j)`; Moonlight audio replaces it with a fixed
//! OpenFEC matrix ([`ReedSolomon::with_parity_matrix`]).

mod simd;

use std::fmt;
use std::mem::MaybeUninit;

use simd::{Kernel, axpy, axpy_n, scal};

/// nanors limit on data + parity shards.
pub const MAX_SHARDS: usize = 255;
/// Most erasures a decode solves for: no more than either the data or the parity shards.
const MAX_ERASURES: usize = MAX_SHARDS / 2;

/// The parity matrix GFE and Sunshine use for the 4+2 audio FEC blocks.
pub const AUDIO_PARITY_MATRIX: [u8; 8] = [0x77, 0x40, 0x38, 0x0e, 0xc7, 0xa7, 0x0d, 0x6c];

const fn exp_log() -> ([u8; 512], [u8; 256]) {
    let mut exp = [0u8; 512];
    let mut log = [0u8; 256];
    let mut x: u16 = 1;
    let mut i = 0;
    while i < 255 {
        exp[i] = x as u8;
        exp[i + 255] = x as u8;
        log[x as usize] = i as u8;
        x <<= 1;
        if x & 0x100 != 0 {
            x ^= 0x11d;
        }
        i += 1;
    }
    exp[510] = exp[255];
    (exp, log)
}

const EXP_LOG: ([u8; 512], [u8; 256]) = exp_log();

const fn mul_table() -> [[u8; 256]; 256] {
    let (exp, log) = EXP_LOG;
    let mut table = [[0u8; 256]; 256];
    let mut a = 1;
    while a < 256 {
        let mut b = 1;
        while b < 256 {
            table[a][b] = exp[log[a] as usize + log[b] as usize];
            b += 1;
        }
        a += 1;
    }
    table
}

const fn inverse_table() -> [u8; 256] {
    let (exp, log) = EXP_LOG;
    let mut inv = [0u8; 256];
    let mut a = 1;
    while a < 256 {
        inv[a] = exp[255 - log[a] as usize];
        a += 1;
    }
    inv
}

static MUL: [[u8; 256]; 256] = mul_table();
/// `INV[0]` is 0, as in nanors.
static INV: [u8; 256] = inverse_table();

/// Multiplies two field elements.
pub fn gf_mul(a: u8, b: u8) -> u8 {
    MUL[a as usize][b as usize]
}

/// Mutable `shards[to]` and shared `shards[from]`; `to != from`.
fn pair<S: AsMut<[u8]> + AsRef<[u8]>>(
    shards: &mut [S],
    to: usize,
    from: usize,
) -> (&mut [u8], &[u8]) {
    debug_assert_ne!(to, from);
    if to < from {
        let (low, high) = shards.split_at_mut(from);
        (low[to].as_mut(), high[0].as_ref())
    } else {
        let (low, high) = shards.split_at_mut(to);
        (high[0].as_mut(), low[from].as_ref())
    }
}

/// `N` disjoint mutable output shards and one disjoint shared input shard (`N <= 4`).
fn strip_n<const N: usize, S: AsMut<[u8]> + AsRef<[u8]>>(
    shards: &mut [S],
    to: [usize; N],
    from: usize,
) -> ([&mut [u8]; N], &[u8]) {
    let mut order = [(0usize, 0usize); 5];
    let count = N + 1;
    debug_assert!(count <= 5);
    for i in 0..N {
        order[i] = (to[i], i);
    }
    order[N] = (from, N);
    for i in 1..count {
        let mut j = i;
        while j > 0 && order[j - 1].0 > order[j].0 {
            order.swap(j - 1, j);
            j -= 1;
        }
    }
    let mut out: [Option<&mut [u8]>; N] = [const { None }; N];
    let mut src: Option<&[u8]> = None;
    let mut rest = shards;
    let mut base = 0;
    for &(idx, slot) in &order[..count] {
        assert!(idx >= base, "duplicate shard index");
        let (_, tail) = rest.split_at_mut(idx - base);
        let (head, after) = tail.split_first_mut().expect("shard index in bounds");
        if slot == N {
            src = Some((*head).as_ref());
        } else {
            out[slot] = Some(head.as_mut());
        }
        rest = after;
        base = idx + 1;
    }
    (
        out.map(|shard| shard.expect("output shard")),
        src.expect("input shard"),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FecError {
    /// Fewer shards or a shorter shard than the code needs.
    BadShards,
    /// More shards are missing than parity shards were received.
    TooFewShards,
}

impl fmt::Display for FecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            FecError::BadShards => "invalid shard set",
            FecError::TooFewShards => "too few shards to recover the data",
        })
    }
}

impl std::error::Error for FecError {}

/// A code with fixed data and parity shard counts.
#[derive(Clone)]
pub struct ReedSolomon {
    data: usize,
    parity: usize,
    /// True when built with [`ReedSolomon::with_parity_matrix`] rather than the Cauchy matrix.
    custom: bool,
    /// `parity` rows of `data` coefficients.
    matrix: Vec<u8>,
}

impl ReedSolomon {
    /// The nanors code for `data` + `parity` shards; `None` outside nanors' limits
    /// (each count 1..=255, total at most 255).
    pub fn new(data: usize, parity: usize) -> Option<Self> {
        let mut code = Self {
            data: 0,
            parity: 0,
            custom: false,
            matrix: Vec::new(),
        };
        code.renew(data, parity).then_some(code)
    }

    /// Makes this the nanors code for `data` + `parity` shards, in the memory it has. False,
    /// and unchanged, outside nanors' limits.
    pub fn renew(&mut self, data: usize, parity: usize) -> bool {
        if data == 0 || parity == 0 || data + parity > MAX_SHARDS {
            return false;
        }
        if !self.custom && self.data == data && self.parity == parity {
            return true;
        }
        self.custom = false;
        self.data = data;
        self.parity = parity;
        self.matrix.clear();
        self.matrix.resize(data * parity, 0);
        for (j, row) in self.matrix.chunks_exact_mut(data).enumerate() {
            for (i, value) in row.iter_mut().enumerate() {
                *value = INV[(parity + i) ^ j];
            }
        }
        true
    }

    /// A code with an explicit row-major parity matrix (`parity` rows of `data` entries).
    pub fn with_parity_matrix(data: usize, parity: usize, matrix: &[u8]) -> Option<Self> {
        let mut code = Self::new(data, parity)?;
        if matrix.len() != data * parity {
            return None;
        }
        code.matrix.copy_from_slice(matrix);
        code.custom = true;
        Some(code)
    }

    pub fn data_shards(&self) -> usize {
        self.data
    }

    pub fn parity_shards(&self) -> usize {
        self.parity
    }

    fn check<S: AsRef<[u8]>>(&self, shards: &[S], len: usize) -> Result<(), FecError> {
        if len == 0
            || shards.len() < self.data + self.parity
            || shards[..self.data + self.parity]
                .iter()
                .any(|s| s.as_ref().len() < len)
        {
            return Err(FecError::BadShards);
        }
        Ok(())
    }

    /// Computes the parity shards `shards[data..data + parity]` from the first `len` bytes of
    /// the data shards.
    pub fn encode(&self, shards: &mut [&mut [u8]], len: usize) -> Result<(), FecError> {
        self.check(shards, len)?;
        let kernel = Kernel::best();
        let (data, parity) = shards.split_at_mut(self.data);
        for out in parity.iter_mut() {
            out[..len].fill(0);
        }
        let mut row = 0;
        while row + 4 <= self.parity {
            let r0 = row * self.data;
            let (p0, rest) = parity[row..row + 4].split_first_mut().unwrap();
            let (p1, rest) = rest.split_first_mut().unwrap();
            let (p2, rest) = rest.split_first_mut().unwrap();
            let p3 = &mut rest[0];
            let mut outs = [&mut p0[..len], &mut p1[..len], &mut p2[..len], &mut p3[..len]];
            for (col, input) in data.iter().enumerate() {
                axpy_n(
                    kernel,
                    &mut outs,
                    &input[..len],
                    [
                        self.matrix[r0 + col],
                        self.matrix[r0 + self.data + col],
                        self.matrix[r0 + 2 * self.data + col],
                        self.matrix[r0 + 3 * self.data + col],
                    ],
                );
            }
            row += 4;
        }
        for (r, out) in parity[row..self.parity].iter_mut().enumerate() {
            let coeffs = &self.matrix[(row + r) * self.data..(row + r + 1) * self.data];
            let out = &mut out[..len];
            for (input, &u) in data.iter().zip(coeffs) {
                axpy(kernel, out, &input[..len], u);
            }
        }
        Ok(())
    }

    /// Recovers the data shards marked missing in `missing` (one flag per shard, data first)
    /// from the received data and parity shards, like `reed_solomon_decode`. Missing parity
    /// shards are not rebuilt and need not be sized. Allocates nothing.
    pub fn decode<S: AsMut<[u8]> + AsRef<[u8]>>(
        &self,
        shards: &mut [S],
        missing: &[bool],
        len: usize,
    ) -> Result<(), FecError> {
        let (ds, ts) = (self.data, self.data + self.parity);
        if len == 0
            || shards.len() < ts
            || missing.len() < ts
            || shards[..ts]
                .iter()
                .enumerate()
                .any(|(i, s)| (i < ds || !missing[i]) && s.as_ref().len() < len)
        {
            return Err(FecError::BadShards);
        }
        let mut erasures = [MaybeUninit::<u8>::uninit(); MAX_SHARDS];
        let mut colperm = [MaybeUninit::<u8>::uninit(); MAX_SHARDS];
        let mut rowperm = [MaybeUninit::<u8>::uninit(); MAX_SHARDS];
        let mut gaps = 0;
        for (i, _) in missing[..ds].iter().enumerate().filter(|(_, m)| **m) {
            erasures[gaps].write(i as u8);
            gaps += 1;
        }
        let mut j = 0;
        for slot in &mut colperm[..ds - gaps] {
            while missing[j] {
                j += 1;
            }
            slot.write(j as u8);
            j += 1;
        }
        for i in 0..gaps {
            // SAFETY: `erasures[i]` was initialized above for `i < gaps`.
            let erasure = unsafe { erasures[i].assume_init() };
            colperm[ds - gaps + i].write(erasure);
        }
        let mut j = ds;
        for i in 0..gaps {
            while j < ts && missing[j] {
                j += 1;
            }
            if j >= ts {
                return Err(FecError::TooFewShards);
            }
            rowperm[i].write((j - ds) as u8);
            // SAFETY: `erasures[i]` was initialized above for `i < gaps`.
            let erasure = unsafe { erasures[i].assume_init() } as usize;
            let (to, from) = pair(shards, erasure, j);
            to[..len].copy_from_slice(&from[..len]);
            j += 1;
        }
        // SAFETY: `colperm[..ds]` and `rowperm[..gaps]` were initialized above.
        let colperm = unsafe { std::slice::from_raw_parts(colperm.as_ptr().cast::<u8>(), ds) };
        let rowperm = unsafe { std::slice::from_raw_parts(rowperm.as_ptr().cast::<u8>(), gaps) };
        self.invert(Kernel::best(), shards, ds - gaps, len, colperm, rowperm);
        Ok(())
    }

    /// Eliminates the known columns `known` into the first `N` output erasures of `erased`/`rows`.
    fn eliminate_strip<const N: usize, S: AsMut<[u8]> + AsRef<[u8]>>(
        &self,
        kernel: Kernel,
        dst: &mut [S],
        len: usize,
        known: &[u8],
        erased: &[u8],
        rows: &[u8],
    ) {
        let k = self.data;
        let src = &self.matrix;
        let to: [usize; N] = std::array::from_fn(|r| erased[r] as usize);
        let drs: [usize; N] = std::array::from_fn(|r| rows[r] as usize * k);
        for &col in known {
            let col = col as usize;
            let coeffs: [u8; N] = std::array::from_fn(|r| src[drs[r] + col]);
            let (mut outs, from) = strip_n(dst, to, col);
            axpy_n(kernel, &mut outs, &from[..len], coeffs);
        }
    }

    /// nanors' `invert_mat`: eliminates the known columns from the copied parity rows, then
    /// solves the remaining `W x W` system in place.
    fn invert<S: AsMut<[u8]> + AsRef<[u8]>>(
        &self,
        kernel: Kernel,
        dst: &mut [S],
        v0: usize,
        len: usize,
        c: &[u8],
        d: &[u8],
    ) {
        let k = self.data;
        let src = &self.matrix;
        let w = k - v0;
        if w == 0 {
            return;
        }
        // Each erasure took a parity shard, so `w` is at most the smaller shard count.
        debug_assert!(w <= MAX_ERASURES);
        let mut work = [MaybeUninit::<u8>::uninit(); MAX_ERASURES * MAX_ERASURES];
        for i in 0..w {
            let dr = d[i] as usize * k;
            for j in 0..w {
                work[i * w + j].write(src[dr + c[v0 + j] as usize]);
            }
        }
        // SAFETY: `work[..w * w]` was fully initialized above.
        let wrk = unsafe { std::slice::from_raw_parts_mut(work.as_mut_ptr().cast::<u8>(), w * w) };
        let (known, erased) = c.split_at(v0);
        let mut e = 0;
        while e + 4 <= w {
            self.eliminate_strip::<4, _>(kernel, dst, len, known, &erased[e..], &d[e..]);
            e += 4;
        }
        match w - e {
            3 => self.eliminate_strip::<3, _>(kernel, dst, len, known, &erased[e..], &d[e..]),
            2 => self.eliminate_strip::<2, _>(kernel, dst, len, known, &erased[e..], &d[e..]),
            1 => {
                let to = erased[e] as usize;
                let dr = d[e] as usize * k;
                for &col in known {
                    let col = col as usize;
                    let (out, from) = pair(dst, to, col);
                    axpy(kernel, &mut out[..len], &from[..len], src[dr + col]);
                }
            }
            _ => {}
        }
        for x in 0..w {
            let u = INV[wrk[x * w + x] as usize];
            scal(kernel, &mut wrk[x * w + x..x * w + w], u);
            scal(kernel, &mut dst[c[v0 + x] as usize].as_mut()[..len], u);
            for row in x + 1..w {
                let u = wrk[row * w + x];
                let (pivot, rest) = wrk.split_at_mut(row * w);
                axpy(kernel, &mut rest[..w], &pivot[x * w..x * w + w], u);
                let (to, from) = pair(dst, c[v0 + row] as usize, c[v0 + x] as usize);
                axpy(kernel, &mut to[..len], &from[..len], u);
            }
        }
        for x in (0..w).rev() {
            for row in 0..x {
                let u = wrk[row * w + x];
                let (to, from) = pair(dst, c[v0 + row] as usize, c[v0 + x] as usize);
                axpy(kernel, &mut to[..len], &from[..len], u);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::simd::NIBBLES;
    use super::*;

    fn shards(count: usize, len: usize, seed: u32) -> Vec<Vec<u8>> {
        let mut state = seed;
        (0..count)
            .map(|_| {
                (0..len)
                    .map(|_| {
                        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                        (state >> 24) as u8
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn field_tables() {
        assert_eq!(EXP_LOG.0[8], 29);
        assert_eq!(INV[2], 142);
        for a in 1..=255u8 {
            assert_eq!(gf_mul(a, INV[a as usize]), 1);
        }
    }

    /// Every vector kernel this CPU runs.
    fn simd_kernels() -> Vec<Kernel> {
        #[cfg(target_arch = "x86_64")]
        {
            let mut kernels = Vec::new();
            if is_x86_feature_detected!("ssse3") {
                kernels.push(Kernel::Ssse3);
            }
            if is_x86_feature_detected!("avx2") {
                kernels.push(Kernel::Avx2);
                if is_x86_feature_detected!("gfni") {
                    kernels.push(Kernel::Avx2Gfni);
                }
            }
            kernels
        }
        #[cfg(target_arch = "aarch64")]
        {
            vec![Kernel::Neon]
        }
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        {
            Vec::new()
        }
    }

    /// Runs `axpy` and `scal` with `kernel` and with the scalar table loop on the same inputs.
    fn assert_matches_scalar(kernel: Kernel, a: &[u8], b: &[u8], u: u8) {
        let (mut want, mut got) = (a.to_vec(), a.to_vec());
        axpy(Kernel::Scalar, &mut want, b, u);
        axpy(kernel, &mut got, b, u);
        assert_eq!(got, want, "{kernel:?} axpy u {u} len {}", a.len());
        let (mut want, mut got) = (a.to_vec(), a.to_vec());
        scal(Kernel::Scalar, &mut want, u);
        scal(kernel, &mut got, u);
        assert_eq!(got, want, "{kernel:?} scal u {u} len {}", a.len());
    }

    #[test]
    fn nibble_tables_split_every_product() {
        for u in 0..=255usize {
            for b in 0..=255usize {
                let split = NIBBLES[u][0][b & 15] ^ NIBBLES[u][1][b >> 4];
                assert_eq!(split, MUL[u][b], "u {u} b {b}");
            }
        }
    }

    #[test]
    fn simd_matches_scalar_for_every_coefficient() {
        let kernels = simd_kernels();
        #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
        assert!(!kernels.is_empty());
        let set = shards(2, 1392 + 3, 11);
        for kernel in kernels {
            for u in 0..=255u8 {
                for len in [0, 1, 15, 16, 17, 31, 32, 33, 47, 48, 63, 64, 65, 1392] {
                    for offset in [0, 3] {
                        let a = &set[0][offset..offset + len];
                        let b = &set[1][3 - offset..3 - offset + len];
                        assert_matches_scalar(kernel, a, b, u);
                    }
                }
            }
        }
    }

    #[test]
    fn simd_matches_scalar_for_every_length() {
        let set = shards(2, 4100 + 3, 23);
        let mut state = 5u32;
        for kernel in simd_kernels() {
            for len in 0..=4100 {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let u = (state >> 24) as u8;
                let (offset_a, offset_b) = (len % 4, (len / 4) % 3);
                let a = &set[0][offset_a..offset_a + len];
                let b = &set[1][offset_b..offset_b + len];
                assert_matches_scalar(kernel, a, b, u);
            }
        }
    }

    #[test]
    fn recovers_any_erasures_up_to_parity() {
        for (data, parity) in [(1, 1), (4, 2), (10, 3), (40, 8), (200, 55)] {
            let code = ReedSolomon::new(data, parity).unwrap();
            let len = 37;
            let mut set = shards(data + parity, len, (data * 31 + parity) as u32);
            let original = set.clone();
            let mut refs: Vec<&mut [u8]> = set.iter_mut().map(|s| &mut s[..]).collect();
            code.encode(&mut refs, len).unwrap();
            let encoded = set.clone();
            let mut missing = vec![false; data + parity];
            for i in 0..parity {
                let lost = (i * 7 + 3) % (data + parity);
                missing[lost] = true;
                set[lost].fill(0xa5);
            }
            let mut refs: Vec<&mut [u8]> = set.iter_mut().map(|s| &mut s[..]).collect();
            code.decode(&mut refs, &missing, len).unwrap();
            assert_eq!(set[..data], original[..data]);
            assert_eq!(set[..data], encoded[..data]);
        }
    }

    #[test]
    fn rejects_too_many_erasures() {
        let code = ReedSolomon::new(4, 2).unwrap();
        let mut set = shards(6, 8, 1);
        let mut refs: Vec<&mut [u8]> = set.iter_mut().map(|s| &mut s[..]).collect();
        let missing = [true, true, false, false, true, false];
        assert_eq!(
            code.decode(&mut refs, &missing, 8),
            Err(FecError::TooFewShards)
        );
    }

    /// Microseconds per recovery of video-sized shards (1392 bytes, 20 % parity). Run in release:
    /// `cargo test --release -p nightjar-core --lib fec::tests::bench_recovery -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench_recovery() {
        const LEN: usize = 1392;
        for (data, erasures) in [(100, 5), (100, 20), (200, 5), (200, 20)] {
            let parity = data / 5;
            let code = ReedSolomon::new(data, parity).unwrap();
            let mut set = shards(data + parity, LEN, data as u32);
            let mut refs: Vec<&mut [u8]> = set.iter_mut().map(|s| &mut s[..]).collect();
            code.encode(&mut refs, LEN).unwrap();
            let encoded = set.clone();
            let mut missing = vec![false; data + parity];
            for i in 0..erasures {
                missing[i * data / erasures] = true;
            }
            let lost: Vec<usize> = (0..data).filter(|&i| missing[i]).collect();
            let iterations = 4_000_000 / (data * erasures);
            let mut best = f64::MAX;
            for _ in 0..5 {
                let start = std::time::Instant::now();
                for _ in 0..iterations {
                    for &i in &lost {
                        set[i].fill(0);
                    }
                    code.decode(&mut set, &missing, LEN).unwrap();
                }
                let us = start.elapsed().as_secs_f64() * 1e6 / iterations as f64;
                best = best.min(us);
                assert_eq!(set, encoded);
            }
            println!("data {data:3} erasures {erasures:2}: {best:8.1} us per recovery");
        }
    }

    #[test]
    fn limits_match_nanors() {
        assert!(ReedSolomon::new(0, 1).is_none());
        assert!(ReedSolomon::new(1, 0).is_none());
        assert!(ReedSolomon::new(200, 56).is_none());
        assert!(ReedSolomon::new(200, 55).is_some());
        assert!(ReedSolomon::with_parity_matrix(4, 2, &AUDIO_PARITY_MATRIX).is_some());
    }

    #[test]
    fn tiled_axpy_matches_scalar_for_all_kernels_and_widths() {
        let set = shards(5, 1392 + 5, 41);
        for kernel in simd_kernels() {
            for len in [0, 1, 7, 8, 9, 15, 16, 17, 23, 24, 31, 32, 33, 48, 63, 64, 1392] {
                let src = &set[4][1..1 + len];
                for coeffs in [
                    [0u8, 1, 2, 255],
                    [1, 1, 1, 1],
                    [0, 0, 0, 0],
                    [13, 77, 142, 211],
                ] {
                    let mut want: [Vec<u8>; 4] = std::array::from_fn(|r| set[r][..len].to_vec());
                    let mut got = want.clone();
                    for r in 0..4 {
                        axpy(Kernel::Scalar, &mut want[r], src, coeffs[r]);
                    }
                    let [g0, g1, g2, g3] = &mut got;
                    axpy_n(
                        kernel,
                        &mut [
                            g0.as_mut_slice(),
                            g1.as_mut_slice(),
                            g2.as_mut_slice(),
                            g3.as_mut_slice(),
                        ],
                        src,
                        coeffs,
                    );
                    assert_eq!(got, want, "{kernel:?} axpy4 len {len} coeffs {coeffs:?}");
                }
            }
        }
    }

    #[test]
    fn renew_reuses_same_shape_rebuilds_custom_and_decode_ignores_missing_parity_len() {
        let mut rs = ReedSolomon::with_parity_matrix(4, 2, &AUDIO_PARITY_MATRIX).unwrap();
        let custom = rs.matrix.clone();
        assert!(rs.renew(4, 2));
        assert_ne!(rs.matrix, custom, "custom matrix replaced by Cauchy");
        let cauchy = rs.matrix.clone();
        assert!(rs.renew(4, 2));
        assert_eq!(rs.matrix, cauchy);

        let len = 37;
        let mut set = shards(6, len, 99);
        let original = set.clone();
        let mut refs: Vec<&mut [u8]> = set.iter_mut().map(|s| &mut s[..]).collect();
        rs.encode(&mut refs, len).unwrap();
        // Drop data shard 1 and parity shard 5 (leaving parity shard 5 empty).
        let missing = [false, true, false, false, false, true];
        set[1].fill(0);
        set[5].clear();
        rs.decode(&mut set, &missing, len).unwrap();
        assert_eq!(set[..4], original[..4]);
    }
}
