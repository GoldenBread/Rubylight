//! SIMD and GFNI kernels for GF(2^8) Reed-Solomon arithmetic over polynomial 0x11d.

use super::{EXP_LOG, MUL};

/// Per coefficient `u`, the products of every low nibble (`[0][n] = u * n`) and every high
/// nibble (`[1][n] = u * (n << 4)`). Multiplication distributes over XOR, so
/// `u * b == [0][b & 15] ^ [1][b >> 4]`: the 16-byte tables the SIMD kernels shuffle, like
/// nanors' oblas_lite.
#[cfg_attr(
    not(any(target_arch = "x86_64", target_arch = "aarch64")),
    allow(dead_code)
)]
const fn nibble_tables() -> [[[u8; 16]; 2]; 256] {
    let (exp, log) = EXP_LOG;
    let mut table = [[[0u8; 16]; 2]; 256];
    let mut u = 1;
    while u < 256 {
        let mut n = 1;
        while n < 16 {
            table[u][0][n] = exp[log[u] as usize + log[n] as usize];
            table[u][1][n] = exp[log[u] as usize + log[n << 4] as usize];
            n += 1;
        }
        u += 1;
    }
    table
}

#[cfg_attr(
    not(any(target_arch = "x86_64", target_arch = "aarch64")),
    allow(dead_code)
)]
pub(super) static NIBBLES: [[[u8; 16]; 2]; 256] = nibble_tables();

/// For each coefficient `u`, the 8x8 bit matrix of multiplication by `u` in nanors' field
/// (polynomial 0x11d, not GFNI's built-in 0x11b), formatted for `vgf2p8affineqb`: byte `7 - i`
/// holds the input bit mask that produces output bit `i`.
#[cfg(target_arch = "x86_64")]
const fn affine_matrices() -> [u64; 256] {
    let (exp, log) = EXP_LOG;
    let mut matrices = [0u64; 256];
    let mut u = 1;
    while u < 256 {
        let mut matrix = 0u64;
        let mut input = 0;
        while input < 8 {
            let column = exp[log[u] as usize + log[1usize << input] as usize];
            let mut output = 0;
            while output < 8 {
                if (column >> output) & 1 != 0 {
                    matrix |= 1u64 << (8 * (7 - output) + input);
                }
                output += 1;
            }
            input += 1;
        }
        matrices[u] = matrix;
        u += 1;
    }
    matrices
}

#[cfg(target_arch = "x86_64")]
static AFFINE: [u64; 256] = affine_matrices();

/// The vector unit `axpy`, `axpy_n` and `scal` use; the scalar table loop finishes tail bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kernel {
    /// AArch64 always has NEON; there only the tests run the scalar loop alone.
    #[cfg_attr(target_arch = "aarch64", allow(dead_code))]
    Scalar,
    #[cfg(target_arch = "x86_64")]
    Ssse3,
    #[cfg(target_arch = "x86_64")]
    Avx2,
    #[cfg(target_arch = "x86_64")]
    Avx2Gfni,
    #[cfg(target_arch = "aarch64")]
    Neon,
}

impl Kernel {
    /// The widest kernel this CPU runs. std caches the CPUID probe.
    pub(super) fn best() -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            if is_x86_feature_detected!("avx2") {
                if is_x86_feature_detected!("gfni") {
                    Kernel::Avx2Gfni
                } else {
                    Kernel::Avx2
                }
            } else if is_x86_feature_detected!("ssse3") {
                Kernel::Ssse3
            } else {
                Kernel::Scalar
            }
        }
        // NEON is part of the AArch64 baseline.
        #[cfg(target_arch = "aarch64")]
        {
            Kernel::Neon
        }
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        {
            Kernel::Scalar
        }
    }
}

/// `a ^= u * b`
pub(super) fn axpy(kernel: Kernel, a: &mut [u8], b: &[u8], u: u8) {
    let len = a.len().min(b.len());
    let (a, b) = (&mut a[..len], &b[..len]);
    match u {
        0 => {}
        1 => {
            // SAFETY: `best` (or a test that probed the CPU) picked a kernel this CPU runs, and
            // `a` and `b` have the same length.
            let done = match kernel {
                Kernel::Scalar => 0,
                #[cfg(target_arch = "x86_64")]
                Kernel::Ssse3 => unsafe { x86::xor_sse2(a, b) },
                #[cfg(target_arch = "x86_64")]
                Kernel::Avx2 | Kernel::Avx2Gfni => unsafe { x86::xor_avx2(a, b) },
                #[cfg(target_arch = "aarch64")]
                Kernel::Neon => unsafe { neon::xor(a, b) },
            };
            a[done..]
                .iter_mut()
                .zip(&b[done..])
                .for_each(|(a, b)| *a ^= b);
        }
        _ => {
            // SAFETY: `best` (or a test that probed the CPU) picked a kernel this CPU runs, and
            // `a` and `b` have the same length.
            let done = match kernel {
                Kernel::Scalar => 0,
                #[cfg(target_arch = "x86_64")]
                Kernel::Ssse3 => unsafe { x86::axpy_ssse3(a, b, &NIBBLES[u as usize]) },
                #[cfg(target_arch = "x86_64")]
                Kernel::Avx2 => unsafe { x86::axpy_avx2(a, b, &NIBBLES[u as usize]) },
                #[cfg(target_arch = "x86_64")]
                Kernel::Avx2Gfni => unsafe { x86::axpy_gfni(a, b, AFFINE[u as usize]) },
                #[cfg(target_arch = "aarch64")]
                Kernel::Neon => unsafe { neon::axpy(a, b, &NIBBLES[u as usize]) },
            };
            let row = &MUL[u as usize];
            a[done..]
                .iter_mut()
                .zip(&b[done..])
                .for_each(|(a, b)| *a ^= row[*b as usize]);
        }
    }
}

/// For each `r in 0..N`: `outs[r][..src.len()] ^= coeffs[r] * src`, reading `src` once.
pub(super) fn axpy_n<const N: usize>(
    kernel: Kernel,
    outs: &mut [&mut [u8]; N],
    src: &[u8],
    coeffs: [u8; N],
) {
    if coeffs.iter().all(|&u| u == 0) {
        return;
    }
    let len = src.len();
    for out in outs.iter() {
        assert!(out.len() >= len);
    }
    // SAFETY: `best` (or a test that probed the CPU) picked a kernel this CPU runs, every
    // `outs[r]` is disjoint and at least `src.len()` bytes long.
    let done = match kernel {
        Kernel::Scalar => 0,
        #[cfg(target_arch = "x86_64")]
        Kernel::Ssse3 => unsafe { x86::axpy_n_ssse3(outs, src, coeffs) },
        #[cfg(target_arch = "x86_64")]
        Kernel::Avx2 => unsafe { x86::axpy_n_avx2(outs, src, coeffs) },
        #[cfg(target_arch = "x86_64")]
        Kernel::Avx2Gfni => unsafe { x86::axpy_n_gfni(outs, src, coeffs) },
        #[cfg(target_arch = "aarch64")]
        Kernel::Neon => unsafe { neon::axpy_n(outs, src, coeffs) },
    };
    let tail = &src[done..len];
    for r in 0..N {
        let u = coeffs[r];
        if u == 0 {
            continue;
        }
        let out = &mut outs[r][done..len];
        if u == 1 {
            out.iter_mut().zip(tail).for_each(|(a, b)| *a ^= b);
        } else {
            let row = &MUL[u as usize];
            out.iter_mut()
                .zip(tail)
                .for_each(|(a, b)| *a ^= row[*b as usize]);
        }
    }
}

/// `a *= u`
pub(super) fn scal(kernel: Kernel, a: &mut [u8], u: u8) {
    if u < 2 {
        return;
    }
    // SAFETY: as in `axpy`.
    let done = match kernel {
        Kernel::Scalar => 0,
        #[cfg(target_arch = "x86_64")]
        Kernel::Ssse3 => unsafe { x86::scal_ssse3(a, &NIBBLES[u as usize]) },
        #[cfg(target_arch = "x86_64")]
        Kernel::Avx2 => unsafe { x86::scal_avx2(a, &NIBBLES[u as usize]) },
        #[cfg(target_arch = "x86_64")]
        Kernel::Avx2Gfni => unsafe { x86::scal_gfni(a, AFFINE[u as usize]) },
        #[cfg(target_arch = "aarch64")]
        Kernel::Neon => unsafe { neon::scal(a, &NIBBLES[u as usize]) },
    };
    let row = &MUL[u as usize];
    a[done..].iter_mut().for_each(|a| *a = row[*a as usize]);
}

/// Split-nibble `pshufb` and GFNI `vgf2p8affineqb` kernels. Each handles whole vector blocks and
/// returns how many bytes it did; callers guarantee the CPU feature and slice lengths.
#[cfg(target_arch = "x86_64")]
mod x86 {
    use std::arch::x86_64::*;

    use super::{AFFINE, NIBBLES};

    #[inline]
    #[target_feature(enable = "ssse3")]
    unsafe fn mul128(x: __m128i, lo: __m128i, hi: __m128i) -> __m128i {
        let mask = _mm_set1_epi8(0x0f);
        let l = _mm_and_si128(x, mask);
        let h = _mm_and_si128(_mm_srli_epi64::<4>(x), mask);
        _mm_xor_si128(_mm_shuffle_epi8(lo, l), _mm_shuffle_epi8(hi, h))
    }

    #[inline]
    #[target_feature(enable = "avx2")]
    unsafe fn mul256(x: __m256i, lo: __m256i, hi: __m256i) -> __m256i {
        let mask = _mm256_set1_epi8(0x0f);
        let l = _mm256_and_si256(x, mask);
        let h = _mm256_and_si256(_mm256_srli_epi64::<4>(x), mask);
        _mm256_xor_si256(_mm256_shuffle_epi8(lo, l), _mm256_shuffle_epi8(hi, h))
    }

    #[target_feature(enable = "ssse3")]
    pub unsafe fn xor_sse2(a: &mut [u8], b: &[u8]) -> usize {
        // SAFETY: caller guarantees SSSE3 (hence SSE2) and `a.len() == b.len()`.
        unsafe {
            let end = a.len() & !15;
            let (pa, pb) = (a.as_mut_ptr(), b.as_ptr());
            let mut i = 0;
            while i < end {
                let x = _mm_loadu_si128(pb.add(i).cast());
                let y = _mm_loadu_si128(pa.add(i).cast());
                _mm_storeu_si128(pa.add(i).cast(), _mm_xor_si128(y, x));
                i += 16;
            }
            end
        }
    }

    #[target_feature(enable = "avx2")]
    pub unsafe fn xor_avx2(a: &mut [u8], b: &[u8]) -> usize {
        // SAFETY: caller guarantees AVX2 and `a.len() == b.len()`.
        unsafe {
            let end = a.len() & !31;
            let (pa, pb) = (a.as_mut_ptr(), b.as_ptr());
            let mut i = 0;
            while i < end {
                let x = _mm256_loadu_si256(pb.add(i).cast());
                let y = _mm256_loadu_si256(pa.add(i).cast());
                _mm256_storeu_si256(pa.add(i).cast(), _mm256_xor_si256(y, x));
                i += 32;
            }
            end + xor_sse2(&mut a[end..], &b[end..])
        }
    }

    #[target_feature(enable = "ssse3")]
    pub unsafe fn axpy_ssse3(a: &mut [u8], b: &[u8], t: &[[u8; 16]; 2]) -> usize {
        // SAFETY: caller guarantees SSSE3 is supported and `a.len() == b.len()`.
        unsafe {
            let lo = _mm_loadu_si128(t[0].as_ptr().cast());
            let hi = _mm_loadu_si128(t[1].as_ptr().cast());
            let end = a.len() & !15;
            let (pa, pb) = (a.as_mut_ptr(), b.as_ptr());
            let mut i = 0;
            while i < end {
                let x = _mm_loadu_si128(pb.add(i).cast());
                let y = _mm_loadu_si128(pa.add(i).cast());
                _mm_storeu_si128(pa.add(i).cast(), _mm_xor_si128(y, mul128(x, lo, hi)));
                i += 16;
            }
            end
        }
    }

    #[target_feature(enable = "ssse3")]
    pub unsafe fn scal_ssse3(a: &mut [u8], t: &[[u8; 16]; 2]) -> usize {
        // SAFETY: caller guarantees SSSE3 is supported and `t` is valid.
        unsafe {
            let lo = _mm_loadu_si128(t[0].as_ptr().cast());
            let hi = _mm_loadu_si128(t[1].as_ptr().cast());
            let end = a.len() & !15;
            let pa = a.as_mut_ptr();
            let mut i = 0;
            while i < end {
                let x = _mm_loadu_si128(pa.add(i).cast());
                _mm_storeu_si128(pa.add(i).cast(), mul128(x, lo, hi));
                i += 16;
            }
            end
        }
    }

    /// 32-byte blocks, then one 16-byte block.
    #[target_feature(enable = "avx2")]
    pub unsafe fn axpy_avx2(a: &mut [u8], b: &[u8], t: &[[u8; 16]; 2]) -> usize {
        // SAFETY: caller guarantees AVX2 is supported and `a.len() == b.len()`.
        unsafe {
            let lo = _mm256_broadcastsi128_si256(_mm_loadu_si128(t[0].as_ptr().cast()));
            let hi = _mm256_broadcastsi128_si256(_mm_loadu_si128(t[1].as_ptr().cast()));
            let end = a.len() & !31;
            let (pa, pb) = (a.as_mut_ptr(), b.as_ptr());
            let mut i = 0;
            while i < end {
                let x = _mm256_loadu_si256(pb.add(i).cast());
                let y = _mm256_loadu_si256(pa.add(i).cast());
                _mm256_storeu_si256(pa.add(i).cast(), _mm256_xor_si256(y, mul256(x, lo, hi)));
                i += 32;
            }
            end + axpy_ssse3(&mut a[end..], &b[end..], t)
        }
    }

    /// 32-byte blocks, then one 16-byte block.
    #[target_feature(enable = "avx2")]
    pub unsafe fn scal_avx2(a: &mut [u8], t: &[[u8; 16]; 2]) -> usize {
        // SAFETY: caller guarantees AVX2 is supported.
        unsafe {
            let lo = _mm256_broadcastsi128_si256(_mm_loadu_si128(t[0].as_ptr().cast()));
            let hi = _mm256_broadcastsi128_si256(_mm_loadu_si128(t[1].as_ptr().cast()));
            let end = a.len() & !31;
            let pa = a.as_mut_ptr();
            let mut i = 0;
            while i < end {
                let x = _mm256_loadu_si256(pa.add(i).cast());
                _mm256_storeu_si256(pa.add(i).cast(), mul256(x, lo, hi));
                i += 32;
            }
            end + scal_ssse3(&mut a[end..], t)
        }
    }

    /// 32-byte, 16-byte, and 8-byte blocks using `vgf2p8affineqb`.
    #[target_feature(enable = "avx2,gfni")]
    pub unsafe fn axpy_gfni(a: &mut [u8], b: &[u8], matrix: u64) -> usize {
        // SAFETY: caller guarantees AVX2 + GFNI and `a.len() == b.len()`.
        unsafe {
            let m256 = _mm256_set1_epi64x(matrix as i64);
            let m128 = _mm256_castsi256_si128(m256);
            let len = a.len();
            let (pa, pb) = (a.as_mut_ptr(), b.as_ptr());
            let mut i = 0;
            while i + 32 <= len {
                let x = _mm256_loadu_si256(pb.add(i).cast());
                let p = _mm256_gf2p8affine_epi64_epi8::<0>(x, m256);
                let y = _mm256_loadu_si256(pa.add(i).cast());
                _mm256_storeu_si256(pa.add(i).cast(), _mm256_xor_si256(y, p));
                i += 32;
            }
            if i + 16 <= len {
                let x = _mm_loadu_si128(pb.add(i).cast());
                let p = _mm_gf2p8affine_epi64_epi8::<0>(x, m128);
                let y = _mm_loadu_si128(pa.add(i).cast());
                _mm_storeu_si128(pa.add(i).cast(), _mm_xor_si128(y, p));
                i += 16;
            }
            if i + 8 <= len {
                let x = _mm_loadl_epi64(pb.add(i).cast());
                let p = _mm_gf2p8affine_epi64_epi8::<0>(x, m128);
                let y = _mm_loadl_epi64(pa.add(i).cast());
                _mm_storel_epi64(pa.add(i).cast(), _mm_xor_si128(y, p));
                i += 8;
            }
            i
        }
    }

    #[target_feature(enable = "avx2,gfni")]
    pub unsafe fn scal_gfni(a: &mut [u8], matrix: u64) -> usize {
        // SAFETY: caller guarantees AVX2 + GFNI.
        unsafe {
            let m256 = _mm256_set1_epi64x(matrix as i64);
            let m128 = _mm256_castsi256_si128(m256);
            let len = a.len();
            let pa = a.as_mut_ptr();
            let mut i = 0;
            while i + 32 <= len {
                let x = _mm256_loadu_si256(pa.add(i).cast());
                _mm256_storeu_si256(pa.add(i).cast(), _mm256_gf2p8affine_epi64_epi8::<0>(x, m256));
                i += 32;
            }
            if i + 16 <= len {
                let x = _mm_loadu_si128(pa.add(i).cast());
                _mm_storeu_si128(pa.add(i).cast(), _mm_gf2p8affine_epi64_epi8::<0>(x, m128));
                i += 16;
            }
            if i + 8 <= len {
                let x = _mm_loadl_epi64(pa.add(i).cast());
                _mm_storel_epi64(pa.add(i).cast(), _mm_gf2p8affine_epi64_epi8::<0>(x, m128));
                i += 8;
            }
            i
        }
    }

    #[target_feature(enable = "ssse3")]
    pub unsafe fn axpy_n_ssse3<const N: usize>(
        outs: &mut [&mut [u8]; N],
        src: &[u8],
        coeffs: [u8; N],
    ) -> usize {
        // SAFETY: caller guarantees SSSE3 and disjoint `outs[r]` of length >= `src.len()`.
        unsafe {
            let mut ptrs = [std::ptr::null_mut::<u8>(); N];
            let mut lo = [_mm_setzero_si128(); N];
            let mut hi = [_mm_setzero_si128(); N];
            for r in 0..N {
                ptrs[r] = outs[r].as_mut_ptr();
                lo[r] = _mm_loadu_si128(NIBBLES[coeffs[r] as usize][0].as_ptr().cast());
                hi[r] = _mm_loadu_si128(NIBBLES[coeffs[r] as usize][1].as_ptr().cast());
            }
            let mask = _mm_set1_epi8(0x0f);
            let pb = src.as_ptr();
            let end = src.len() & !15;
            let mut i = 0;
            while i < end {
                let x = _mm_loadu_si128(pb.add(i).cast());
                let l = _mm_and_si128(x, mask);
                let h = _mm_and_si128(_mm_srli_epi64::<4>(x), mask);
                for r in 0..N {
                    let p = _mm_xor_si128(_mm_shuffle_epi8(lo[r], l), _mm_shuffle_epi8(hi[r], h));
                    let y = _mm_loadu_si128(ptrs[r].add(i).cast());
                    _mm_storeu_si128(ptrs[r].add(i).cast(), _mm_xor_si128(y, p));
                }
                i += 16;
            }
            end
        }
    }

    #[target_feature(enable = "avx2")]
    pub unsafe fn axpy_n_avx2<const N: usize>(
        outs: &mut [&mut [u8]; N],
        src: &[u8],
        coeffs: [u8; N],
    ) -> usize {
        // SAFETY: caller guarantees AVX2 and disjoint `outs[r]` of length >= `src.len()`.
        unsafe {
            let mut ptrs = [std::ptr::null_mut::<u8>(); N];
            let mut lo = [_mm256_setzero_si256(); N];
            let mut hi = [_mm256_setzero_si256(); N];
            for r in 0..N {
                ptrs[r] = outs[r].as_mut_ptr();
                lo[r] = _mm256_broadcastsi128_si256(_mm_loadu_si128(
                    NIBBLES[coeffs[r] as usize][0].as_ptr().cast(),
                ));
                hi[r] = _mm256_broadcastsi128_si256(_mm_loadu_si128(
                    NIBBLES[coeffs[r] as usize][1].as_ptr().cast(),
                ));
            }
            let mask = _mm256_set1_epi8(0x0f);
            let mask128 = _mm256_castsi256_si128(mask);
            let pb = src.as_ptr();
            let len = src.len();
            let mut i = 0;
            while i + 32 <= len {
                let x = _mm256_loadu_si256(pb.add(i).cast());
                let l = _mm256_and_si256(x, mask);
                let h = _mm256_and_si256(_mm256_srli_epi64::<4>(x), mask);
                for r in 0..N {
                    let p = _mm256_xor_si256(
                        _mm256_shuffle_epi8(lo[r], l),
                        _mm256_shuffle_epi8(hi[r], h),
                    );
                    let y = _mm256_loadu_si256(ptrs[r].add(i).cast());
                    _mm256_storeu_si256(ptrs[r].add(i).cast(), _mm256_xor_si256(y, p));
                }
                i += 32;
            }
            if i + 16 <= len {
                let x = _mm_loadu_si128(pb.add(i).cast());
                let l = _mm_and_si128(x, mask128);
                let h = _mm_and_si128(_mm_srli_epi64::<4>(x), mask128);
                for r in 0..N {
                    let p = _mm_xor_si128(
                        _mm_shuffle_epi8(_mm256_castsi256_si128(lo[r]), l),
                        _mm_shuffle_epi8(_mm256_castsi256_si128(hi[r]), h),
                    );
                    let y = _mm_loadu_si128(ptrs[r].add(i).cast());
                    _mm_storeu_si128(ptrs[r].add(i).cast(), _mm_xor_si128(y, p));
                }
                i += 16;
            }
            if i + 8 <= len {
                let x = _mm_loadl_epi64(pb.add(i).cast());
                let l = _mm_and_si128(x, mask128);
                let h = _mm_and_si128(_mm_srli_epi64::<4>(x), mask128);
                for r in 0..N {
                    let p = _mm_xor_si128(
                        _mm_shuffle_epi8(_mm256_castsi256_si128(lo[r]), l),
                        _mm_shuffle_epi8(_mm256_castsi256_si128(hi[r]), h),
                    );
                    let y = _mm_loadl_epi64(ptrs[r].add(i).cast());
                    _mm_storel_epi64(ptrs[r].add(i).cast(), _mm_xor_si128(y, p));
                }
                i += 8;
            }
            i
        }
    }

    #[target_feature(enable = "avx2,gfni")]
    pub unsafe fn axpy_n_gfni<const N: usize>(
        outs: &mut [&mut [u8]; N],
        src: &[u8],
        coeffs: [u8; N],
    ) -> usize {
        // SAFETY: caller guarantees AVX2 + GFNI and disjoint `outs[r]` of length >= `src.len()`.
        unsafe {
            let mut ptrs = [std::ptr::null_mut::<u8>(); N];
            let mut matrices = [_mm256_setzero_si256(); N];
            for r in 0..N {
                ptrs[r] = outs[r].as_mut_ptr();
                matrices[r] = _mm256_set1_epi64x(AFFINE[coeffs[r] as usize] as i64);
            }
            let pb = src.as_ptr();
            let len = src.len();
            let mut i = 0;
            while i + 32 <= len {
                let x = _mm256_loadu_si256(pb.add(i).cast());
                for r in 0..N {
                    let p = _mm256_gf2p8affine_epi64_epi8::<0>(x, matrices[r]);
                    let y = _mm256_loadu_si256(ptrs[r].add(i).cast());
                    _mm256_storeu_si256(ptrs[r].add(i).cast(), _mm256_xor_si256(y, p));
                }
                i += 32;
            }
            if i + 16 <= len {
                let x = _mm_loadu_si128(pb.add(i).cast());
                for r in 0..N {
                    let p = _mm_gf2p8affine_epi64_epi8::<0>(x, _mm256_castsi256_si128(matrices[r]));
                    let y = _mm_loadu_si128(ptrs[r].add(i).cast());
                    _mm_storeu_si128(ptrs[r].add(i).cast(), _mm_xor_si128(y, p));
                }
                i += 16;
            }
            if i + 8 <= len {
                let x = _mm_loadl_epi64(pb.add(i).cast());
                for r in 0..N {
                    let p = _mm_gf2p8affine_epi64_epi8::<0>(x, _mm256_castsi256_si128(matrices[r]));
                    let y = _mm_loadl_epi64(ptrs[r].add(i).cast());
                    _mm_storel_epi64(ptrs[r].add(i).cast(), _mm_xor_si128(y, p));
                }
                i += 8;
            }
            i
        }
    }
}

/// Split-nibble `tbl` kernels on 16-byte blocks; same contract as the x86 ones.
#[cfg(target_arch = "aarch64")]
mod neon {
    use std::arch::aarch64::*;

    use super::NIBBLES;

    #[inline]
    unsafe fn mul(x: uint8x16_t, lo: uint8x16_t, hi: uint8x16_t) -> uint8x16_t {
        // SAFETY: caller guarantees NEON is available on aarch64.
        unsafe {
            let l = vandq_u8(x, vdupq_n_u8(0x0f));
            let h = vshrq_n_u8::<4>(x);
            veorq_u8(vqtbl1q_u8(lo, l), vqtbl1q_u8(hi, h))
        }
    }

    pub unsafe fn xor(a: &mut [u8], b: &[u8]) -> usize {
        // SAFETY: caller guarantees NEON is available and `a.len() == b.len()`.
        unsafe {
            let end = a.len() & !15;
            let (pa, pb) = (a.as_mut_ptr(), b.as_ptr());
            let mut i = 0;
            while i < end {
                let x = vld1q_u8(pb.add(i));
                let y = vld1q_u8(pa.add(i));
                vst1q_u8(pa.add(i), veorq_u8(y, x));
                i += 16;
            }
            end
        }
    }

    pub unsafe fn axpy(a: &mut [u8], b: &[u8], t: &[[u8; 16]; 2]) -> usize {
        // SAFETY: caller guarantees NEON is available and `a.len() == b.len()`.
        unsafe {
            let lo = vld1q_u8(t[0].as_ptr());
            let hi = vld1q_u8(t[1].as_ptr());
            let end = a.len() & !15;
            let (pa, pb) = (a.as_mut_ptr(), b.as_ptr());
            let mut i = 0;
            while i < end {
                let x = vld1q_u8(pb.add(i));
                let y = vld1q_u8(pa.add(i));
                vst1q_u8(pa.add(i), veorq_u8(y, mul(x, lo, hi)));
                i += 16;
            }
            end
        }
    }

    pub unsafe fn axpy_n<const N: usize>(
        outs: &mut [&mut [u8]; N],
        src: &[u8],
        coeffs: [u8; N],
    ) -> usize {
        // SAFETY: caller guarantees NEON and disjoint `outs[r]` of length >= `src.len()`.
        unsafe {
            let mut ptrs = [std::ptr::null_mut::<u8>(); N];
            let mut lo = [vdupq_n_u8(0); N];
            let mut hi = [vdupq_n_u8(0); N];
            for r in 0..N {
                ptrs[r] = outs[r].as_mut_ptr();
                lo[r] = vld1q_u8(NIBBLES[coeffs[r] as usize][0].as_ptr());
                hi[r] = vld1q_u8(NIBBLES[coeffs[r] as usize][1].as_ptr());
            }
            let mask = vdupq_n_u8(0x0f);
            let pb = src.as_ptr();
            let end = src.len() & !15;
            let mut i = 0;
            while i < end {
                let x = vld1q_u8(pb.add(i));
                let l = vandq_u8(x, mask);
                let h = vshrq_n_u8::<4>(x);
                for r in 0..N {
                    let p = veorq_u8(vqtbl1q_u8(lo[r], l), vqtbl1q_u8(hi[r], h));
                    let y = vld1q_u8(ptrs[r].add(i));
                    vst1q_u8(ptrs[r].add(i), veorq_u8(y, p));
                }
                i += 16;
            }
            end
        }
    }

    pub unsafe fn scal(a: &mut [u8], t: &[[u8; 16]; 2]) -> usize {
        // SAFETY: caller guarantees NEON is available.
        unsafe {
            let lo = vld1q_u8(t[0].as_ptr());
            let hi = vld1q_u8(t[1].as_ptr());
            let end = a.len() & !15;
            let pa = a.as_mut_ptr();
            let mut i = 0;
            while i < end {
                let x = vld1q_u8(pa.add(i));
                vst1q_u8(pa.add(i), mul(x, lo, hi));
                i += 16;
            }
            end
        }
    }
}
