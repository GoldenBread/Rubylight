//! Cauchy Reed-Solomon GF(2^8) parity encoding with AVX2, GFNI, and SSSE3 kernels.

use anyhow::{Result, bail};

pub(super) const fn gf_mul(mut a: u8, mut b: u8) -> u8 {
    let mut p = 0;
    while b != 0 {
        if b & 1 != 0 {
            p ^= a;
        }
        let top = a & 0x80;
        a <<= 1;
        if top != 0 {
            a ^= 0x1d;
        }
        b >>= 1;
    }
    p
}
pub(super) const fn inverse(a: u8) -> u8 {
    let mut p = 1;
    let mut base = a;
    let mut exponent = 254;
    while exponent != 0 {
        if exponent & 1 != 0 {
            p = gf_mul(p, base);
        }
        base = gf_mul(base, base);
        exponent >>= 1;
    }
    p
}
const fn nibble_tables() -> [[u8; 32]; 256] {
    let mut tables = [[0; 32]; 256];
    let mut coefficient = 0;
    while coefficient < 256 {
        let mut i = 0;
        while i < 16 {
            tables[coefficient][i] = gf_mul(coefficient as u8, i as u8);
            tables[coefficient][16 + i] = gf_mul(coefficient as u8, (i << 4) as u8);
            i += 1;
        }
        coefficient += 1;
    }
    tables
}
static NIBBLES: [[u8; 32]; 256] = nibble_tables();
const fn inverses() -> [u8; 256] {
    let mut values = [0; 256];
    let mut i = 0;
    while i < 256 {
        values[i] = inverse(i as u8);
        i += 1;
    }
    values
}
pub(super) static INVERSES: [u8; 256] = inverses();
/// For each coefficient, the 8x8 bit matrix of multiplication by it in
/// nanors' field (polynomial 0x11D, not GFNI's own 0x11B, so `gf2p8mulb` does
/// not apply), as `gf2p8affineqb` takes it: byte `7 - i` holds the input bits
/// that make output bit `i`.
const fn affine_matrices() -> [u64; 256] {
    let mut matrices = [0; 256];
    let mut coefficient = 0;
    while coefficient < 256 {
        let mut matrix = 0u64;
        let mut input = 0;
        while input < 8 {
            let column = gf_mul(coefficient as u8, 1 << input);
            let mut output = 0;
            while output < 8 {
                if (column >> output) & 1 != 0 {
                    matrix |= 1 << (8 * (7 - output) + input);
                }
                output += 1;
            }
            input += 1;
        }
        matrices[coefficient] = matrix;
        coefficient += 1;
    }
    matrices
}
#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
pub(super) static AFFINE: [u64; 256] = affine_matrices();
static GFNI: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
/// Whether FEC parity may use GFNI where the CPU has it (`fec_gfni`). Both
/// paths produce the same bytes.
pub fn use_gfni(enabled: bool) {
    GFNI.store(enabled, std::sync::atomic::Ordering::Relaxed);
}
/// FEC parity is computed with GFNI: allowed, and the CPU has it and AVX2.
pub fn gfni_active() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        GFNI.load(std::sync::atomic::Ordering::Relaxed)
            && std::is_x86_feature_detected!("avx2")
            && std::is_x86_feature_detected!("gfni")
    }
    #[cfg(not(target_arch = "x86_64"))]
    false
}
pub(super) fn axpy(out: &mut [u8], input: &[u8], coefficient: u8) {
    #[cfg(target_arch = "x86_64")]
    if std::is_x86_feature_detected!("avx2") {
        // SAFETY: AVX2 is available, and the caller passes equal-length slices.
        unsafe {
            axpy_avx2(out, input, &NIBBLES[coefficient as usize]);
        }
        return;
    }
    #[cfg(target_arch = "x86_64")]
    if std::is_x86_feature_detected!("ssse3") {
        // SAFETY: SSSE3 is available, and the caller passes equal-length slices.
        unsafe {
            axpy_ssse3(out, input, &NIBBLES[coefficient as usize]);
        }
        return;
    }
    let table = &NIBBLES[coefficient as usize];
    for (a, b) in out.iter_mut().zip(input) {
        *a ^= table[(b & 15) as usize] ^ table[16 + (b >> 4) as usize];
    }
}
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
/// `out ^= coefficient * input` in GF(2^8), 32 bytes at a time.
///
/// # Safety
/// The CPU supports AVX2 and `input` is at least as long as `out`.
unsafe fn axpy_avx2(out: &mut [u8], input: &[u8], table: &[u8; 32]) {
    use std::arch::x86_64::*;
    // SAFETY: loads and stores stay below `out.len()`, which `input` covers,
    // and `table` holds the 32 bytes read.
    unsafe {
        let low = _mm256_broadcastsi128_si256(_mm_loadu_si128(table.as_ptr().cast()));
        let high = _mm256_broadcastsi128_si256(_mm_loadu_si128(table.as_ptr().add(16).cast()));
        let mask = _mm256_set1_epi8(15);
        let mut i = 0;
        while i + 32 <= out.len() {
            let b = _mm256_loadu_si256(input.as_ptr().add(i).cast());
            let p = _mm256_xor_si256(
                _mm256_shuffle_epi8(low, _mm256_and_si256(b, mask)),
                _mm256_shuffle_epi8(high, _mm256_and_si256(_mm256_srli_epi16::<4>(b), mask)),
            );
            let a = _mm256_loadu_si256(out.as_ptr().add(i).cast());
            _mm256_storeu_si256(out.as_mut_ptr().add(i).cast(), _mm256_xor_si256(a, p));
            i += 32;
        }
        for (a, b) in out[i..].iter_mut().zip(&input[i..]) {
            *a ^= table[(b & 15) as usize] ^ table[16 + (b >> 4) as usize];
        }
    }
}

/// Accumulate `ROWS` parity rows from every source shard at once.
///
/// Reuses each input load and nibble split across the rows. A row is only
/// 1–2 KiB for Moonlight, so the active parity set stays in L1 instead of
/// rereading the whole input block for every row.
///
/// # Safety
/// The CPU supports AVX2; `destination` holds `ROWS` disjoint buffers, and
/// every source and destination buffer has the same length, at least `OFFSET`.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn parity_avx2<const ROWS: usize, const OFFSET: usize>(
    source: &[Vec<u8>],
    destination: &mut [Vec<u8>],
    first: usize,
    parity: usize,
) {
    use std::arch::x86_64::*;
    // SAFETY: every pointer offset stays below the common shard length the
    // caller guarantees, and each destination row is written through its own
    // pointer only.
    unsafe {
        let size = source[0].len() - OFFSET;
        let pointers: [*mut u8; ROWS] =
            std::array::from_fn(|i| destination[i].as_mut_ptr().add(OFFSET));
        let mask = _mm256_set1_epi8(15);
        for (column, input) in source.iter().enumerate() {
            let input = &input[OFFSET..];
            let coefficients: [u8; ROWS] =
                std::array::from_fn(|row| INVERSES[(parity + column) ^ (first + row)]);
            let low: [__m256i; ROWS] = std::array::from_fn(|row| {
                _mm256_broadcastsi128_si256(_mm_loadu_si128(
                    NIBBLES[coefficients[row] as usize].as_ptr().cast(),
                ))
            });
            let high: [__m256i; ROWS] = std::array::from_fn(|row| {
                _mm256_broadcastsi128_si256(_mm_loadu_si128(
                    NIBBLES[coefficients[row] as usize].as_ptr().add(16).cast(),
                ))
            });
            let mut position = 0;
            while position + 32 <= size {
                let value = _mm256_loadu_si256(input.as_ptr().add(position).cast());
                let lo = _mm256_and_si256(value, mask);
                let hi = _mm256_and_si256(_mm256_srli_epi16::<4>(value), mask);
                for row in 0..ROWS {
                    let product = _mm256_xor_si256(
                        _mm256_shuffle_epi8(low[row], lo),
                        _mm256_shuffle_epi8(high[row], hi),
                    );
                    let output = _mm256_loadu_si256(pointers[row].add(position).cast());
                    _mm256_storeu_si256(
                        pointers[row].add(position).cast(),
                        _mm256_xor_si256(output, product),
                    );
                }
                position += 32;
            }
            // Leftover 16 and 8 bytes, with the 128-bit halves of the tables.
            if position + 16 <= size {
                let value = _mm_loadu_si128(input.as_ptr().add(position).cast());
                let lo = _mm_and_si128(value, _mm256_castsi256_si128(mask));
                let hi = _mm_and_si128(_mm_srli_epi16::<4>(value), _mm256_castsi256_si128(mask));
                for row in 0..ROWS {
                    let product = _mm_xor_si128(
                        _mm_shuffle_epi8(_mm256_castsi256_si128(low[row]), lo),
                        _mm_shuffle_epi8(_mm256_castsi256_si128(high[row]), hi),
                    );
                    let output = _mm_loadu_si128(pointers[row].add(position).cast());
                    _mm_storeu_si128(
                        pointers[row].add(position).cast(),
                        _mm_xor_si128(output, product),
                    );
                }
                position += 16;
            }
            if position + 8 <= size {
                let value = _mm_loadl_epi64(input.as_ptr().add(position).cast());
                let lo = _mm_and_si128(value, _mm256_castsi256_si128(mask));
                let hi = _mm_and_si128(_mm_srli_epi16::<4>(value), _mm256_castsi256_si128(mask));
                for row in 0..ROWS {
                    let product = _mm_xor_si128(
                        _mm_shuffle_epi8(_mm256_castsi256_si128(low[row]), lo),
                        _mm_shuffle_epi8(_mm256_castsi256_si128(high[row]), hi),
                    );
                    let output = _mm_loadl_epi64(pointers[row].add(position).cast());
                    _mm_storel_epi64(
                        pointers[row].add(position).cast(),
                        _mm_xor_si128(output, product),
                    );
                }
                position += 8;
            }
            for (position, value) in input.iter().enumerate().skip(position) {
                for row in 0..ROWS {
                    let table = &NIBBLES[coefficients[row] as usize];
                    *pointers[row].add(position) ^=
                        table[(value & 15) as usize] ^ table[16 + (value >> 4) as usize];
                }
            }
        }
    }
}
/// [`parity_avx2`] with one `gf2p8affineqb` per row in place of two nibble
/// lookups and their masks: the same bytes.
///
/// # Safety
/// As for [`parity_avx2`], and the CPU supports GFNI.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,gfni")]
unsafe fn parity_gfni<const ROWS: usize, const OFFSET: usize>(
    source: &[Vec<u8>],
    destination: &mut [Vec<u8>],
    first: usize,
    parity: usize,
) {
    use std::arch::x86_64::*;
    // SAFETY: as in `parity_avx2`.
    unsafe {
        let size = source[0].len() - OFFSET;
        let pointers: [*mut u8; ROWS] =
            std::array::from_fn(|i| destination[i].as_mut_ptr().add(OFFSET));
        for (column, input) in source.iter().enumerate() {
            let input = &input[OFFSET..];
            let coefficients: [u8; ROWS] =
                std::array::from_fn(|row| INVERSES[(parity + column) ^ (first + row)]);
            let matrices: [__m256i; ROWS] = std::array::from_fn(|row| {
                _mm256_set1_epi64x(AFFINE[coefficients[row] as usize] as i64)
            });
            let mut position = 0;
            while position + 32 <= size {
                let value = _mm256_loadu_si256(input.as_ptr().add(position).cast());
                for row in 0..ROWS {
                    let product = _mm256_gf2p8affine_epi64_epi8::<0>(value, matrices[row]);
                    let output = _mm256_loadu_si256(pointers[row].add(position).cast());
                    _mm256_storeu_si256(
                        pointers[row].add(position).cast(),
                        _mm256_xor_si256(output, product),
                    );
                }
                position += 32;
            }
            // Leftover 16 and 8 bytes, with the 128-bit half of each matrix.
            if position + 16 <= size {
                let value = _mm_loadu_si128(input.as_ptr().add(position).cast());
                for row in 0..ROWS {
                    let matrix = _mm256_castsi256_si128(matrices[row]);
                    let product = _mm_gf2p8affine_epi64_epi8::<0>(value, matrix);
                    let output = _mm_loadu_si128(pointers[row].add(position).cast());
                    _mm_storeu_si128(
                        pointers[row].add(position).cast(),
                        _mm_xor_si128(output, product),
                    );
                }
                position += 16;
            }
            if position + 8 <= size {
                let value = _mm_loadl_epi64(input.as_ptr().add(position).cast());
                for row in 0..ROWS {
                    let matrix = _mm256_castsi256_si128(matrices[row]);
                    let product = _mm_gf2p8affine_epi64_epi8::<0>(value, matrix);
                    let output = _mm_loadl_epi64(pointers[row].add(position).cast());
                    _mm_storel_epi64(
                        pointers[row].add(position).cast(),
                        _mm_xor_si128(output, product),
                    );
                }
                position += 8;
            }
            for (position, value) in input.iter().enumerate().skip(position) {
                for row in 0..ROWS {
                    let table = &NIBBLES[coefficients[row] as usize];
                    *pointers[row].add(position) ^=
                        table[(value & 15) as usize] ^ table[16 + (value >> 4) as usize];
                }
            }
        }
    }
}
/// One group of `ROWS` parity rows, with GFNI or AVX2.
///
/// # Safety
/// As for [`parity_avx2`], and with `gfni` the CPU supports GFNI.
#[cfg(target_arch = "x86_64")]
unsafe fn parity_rows<const ROWS: usize, const OFFSET: usize>(
    gfni: bool,
    source: &[Vec<u8>],
    destination: &mut [Vec<u8>],
    first: usize,
    parity: usize,
) {
    // SAFETY: the caller's guarantees.
    unsafe {
        if gfni {
            parity_gfni::<ROWS, OFFSET>(source, destination, first, parity);
        } else {
            parity_avx2::<ROWS, OFFSET>(source, destination, first, parity);
        }
    }
}
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "ssse3")]
/// `out ^= coefficient * input` in GF(2^8), 16 bytes at a time.
///
/// # Safety
/// The CPU supports SSSE3 and `input` is at least as long as `out`.
unsafe fn axpy_ssse3(out: &mut [u8], input: &[u8], table: &[u8; 32]) {
    use std::arch::x86_64::*;
    // SAFETY: loads and stores stay below `out.len()`, which `input` covers,
    // and `table` holds the 32 bytes read.
    unsafe {
        let low = _mm_loadu_si128(table.as_ptr().cast());
        let high = _mm_loadu_si128(table.as_ptr().add(16).cast());
        let mask = _mm_set1_epi8(15);
        let mut i = 0;
        while i + 16 <= out.len() {
            let b = _mm_loadu_si128(input.as_ptr().add(i).cast());
            let p = _mm_xor_si128(
                _mm_shuffle_epi8(low, _mm_and_si128(b, mask)),
                _mm_shuffle_epi8(high, _mm_and_si128(_mm_srli_epi16::<4>(b), mask)),
            );
            let a = _mm_loadu_si128(out.as_ptr().add(i).cast());
            _mm_storeu_si128(out.as_mut_ptr().add(i).cast(), _mm_xor_si128(a, p));
            i += 16;
        }
        for (a, b) in out[i..].iter_mut().zip(&input[i..]) {
            *a ^= table[(b & 15) as usize] ^ table[16 + (b >> 4) as usize];
        }
    }
}

/// nanors uses a Cauchy matrix; the Vandermonde matrix used by many RS crates is not wire compatible.
pub fn cauchy_encode(shards: &mut [Vec<u8>], data: usize, parity: usize) -> Result<()> {
    cauchy_encode_offset::<0>(shards, data, parity)
}
pub(super) fn cauchy_encode_offset<const OFFSET: usize>(
    shards: &mut [Vec<u8>],
    data: usize,
    parity: usize,
) -> Result<()> {
    cauchy_encode_with::<OFFSET>(shards, data, parity, gfni_active())
}
/// [`cauchy_encode_offset`] with GFNI where the CPU has it, or without.
pub(super) fn cauchy_encode_with<const OFFSET: usize>(
    shards: &mut [Vec<u8>],
    data: usize,
    parity: usize,
    gfni: bool,
) -> Result<()> {
    if data == 0 || data + parity != shards.len() || data + parity > 255 {
        bail!("invalid Cauchy shard count");
    }
    let size = shards[0].len();
    if size < OFFSET || shards.iter().any(|s| s.len() != size) {
        bail!("unequal shard sizes");
    }
    let (source, dest) = shards.split_at_mut(data);
    for out in dest.iter_mut() {
        out[OFFSET..].fill(0);
    }
    #[cfg(target_arch = "x86_64")]
    if std::is_x86_feature_detected!("avx2") {
        let gfni = gfni && std::is_x86_feature_detected!("gfni");
        let (rows, remainder) = dest.as_chunks_mut::<4>();
        for (batch, out) in rows.iter_mut().enumerate() {
            // SAFETY: AVX2 is available; validation above guarantees disjoint,
            // equally sized buffers and at most 255 rows and columns, with
            // OFFSET within each allocation. Each group writes only its own rows.
            // GFNI, when used, is available too.
            unsafe {
                parity_rows::<4, OFFSET>(gfni, source, out, batch * 4, parity);
            }
        }
        let first = parity - remainder.len();
        // SAFETY: as for the full groups above.
        unsafe {
            match remainder.len() {
                1 => parity_rows::<1, OFFSET>(gfni, source, remainder, first, parity),
                2 => parity_rows::<2, OFFSET>(gfni, source, remainder, first, parity),
                3 => parity_rows::<3, OFFSET>(gfni, source, remainder, first, parity),
                _ => (),
            }
        }
        return Ok(());
    }
    #[cfg(not(target_arch = "x86_64"))]
    let _ = gfni;
    for (row, out) in dest.iter_mut().enumerate() {
        for (i, input) in source.iter().enumerate() {
            let base = (parity + i) as u8 ^ row as u8;
            axpy(
                &mut out[OFFSET..],
                &input[OFFSET..],
                INVERSES[base as usize],
            );
        }
    }
    Ok(())
}
