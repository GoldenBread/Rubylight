//! Vectorized FP32-to-FP16 row packing for PyroWave CPU wavelet coefficient frames.

/// Packs one row of `(Y, Cb, Cr, 1.0)` FP32 wavelet coefficients into RGBA16F (`[u16; 4]`).
#[inline]
pub(super) fn pack_row(row: &mut [[u16; 4]], planes: [&[f32]; 3], fast_x86: bool) {
    #[cfg(target_arch = "x86_64")]
    if fast_x86 {
        // SAFETY: Caller verified `is_x86_feature_detected!("f16c")`.
        unsafe { pack_half_row(row, planes) };
        return;
    }
    #[cfg(not(target_arch = "x86_64"))]
    let _ = fast_x86;
    #[cfg(target_arch = "aarch64")]
    {
        pack_half_row_neon(row, planes);
        return;
    }
    #[allow(unreachable_code)]
    for (x, pixel) in row.iter_mut().enumerate() {
        *pixel = [half(planes[0][x]), half(planes[1][x]), half(planes[2][x]), 0x3c00];
    }
}

/// Packs one luma-only row `(Y, 0.0, 0.0, 1.0)` for inactive 4:2:0 bottom-half chroma rows.
#[inline]
pub(super) fn pack_luma_row(row: &mut [[u16; 4]], luma: &[f32], fast_x86: bool) {
    #[cfg(target_arch = "x86_64")]
    if fast_x86 {
        // SAFETY: Caller verified `is_x86_feature_detected!("f16c")`.
        unsafe { pack_half_luma_row(row, luma) };
        return;
    }
    #[cfg(not(target_arch = "x86_64"))]
    let _ = fast_x86;
    #[cfg(target_arch = "aarch64")]
    {
        pack_half_luma_row_neon(row, luma);
        return;
    }
    #[allow(unreachable_code)]
    for (x, pixel) in row.iter_mut().enumerate() {
        *pixel = [half(luma[x]), 0, 0, 0x3c00];
    }
}

/// Vectorized 4-pixel-per-iteration F16C/AVX row packer (`Y, Cb, Cr, 1.0` -> RGBA16F).
///
/// # Safety
/// Caller must guarantee the CPU supports `f16c` (which implies `avx` on x86_64).
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "f16c")]
pub(super) unsafe fn pack_half_row(row: &mut [[u16; 4]], planes: [&[f32]; 3]) {
    use std::arch::x86_64::*;
    let len = row.len().min(planes[0].len()).min(planes[1].len()).min(planes[2].len());
    let mut x = 0;
    let alpha = _mm_set1_ps(1.0);
    // SAFETY: Caller guarantees F16C/AVX; `x + 4 <= len` keeps every load and store in bounds.
    unsafe {
        while x + 4 <= len {
            let vy = _mm_loadu_ps(planes[0].as_ptr().add(x));
            let vcb = _mm_loadu_ps(planes[1].as_ptr().add(x));
            let vcr = _mm_loadu_ps(planes[2].as_ptr().add(x));
            let t0 = _mm_unpacklo_ps(vy, vcb);
            let t1 = _mm_unpackhi_ps(vy, vcb);
            let t2 = _mm_unpacklo_ps(vcr, alpha);
            let t3 = _mm_unpackhi_ps(vcr, alpha);
            let p01 = _mm256_insertf128_ps::<1>(_mm256_castps128_ps256(_mm_movelh_ps(t0, t2)), _mm_movehl_ps(t2, t0));
            let p23 = _mm256_insertf128_ps::<1>(_mm256_castps128_ps256(_mm_movelh_ps(t1, t3)), _mm_movehl_ps(t3, t1));
            _mm_storeu_si128(row.as_mut_ptr().add(x).cast(), _mm256_cvtps_ph::<0>(p01));
            _mm_storeu_si128(row.as_mut_ptr().add(x + 2).cast(), _mm256_cvtps_ph::<0>(p23));
            x += 4;
        }
        while x < len {
            let values = _mm_set_ps(1.0, planes[2][x], planes[1][x], planes[0][x]);
            _mm_storel_epi64(row[x].as_mut_ptr().cast(), _mm_cvtps_ph::<0>(values));
            x += 1;
        }
    }
}

/// Vectorized 4-pixel-per-iteration F16C luma-only row packer (`Y, 0, 0, 1.0` -> RGBA16F).
///
/// # Safety
/// Caller must guarantee the CPU supports `f16c`.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "f16c")]
unsafe fn pack_half_luma_row(row: &mut [[u16; 4]], luma: &[f32]) {
    use std::arch::x86_64::*;
    let len = row.len().min(luma.len());
    let mut x = 0;
    let zero = _mm_setzero_si128();
    let za = _mm_unpacklo_epi16(zero, _mm_set1_epi16(0x3c00u16 as i16));
    // SAFETY: Caller guarantees F16C; `x + 4 <= len` keeps every load and store in bounds.
    unsafe {
        while x + 4 <= len {
            let hy = _mm_cvtps_ph::<0>(_mm_loadu_ps(luma.as_ptr().add(x)));
            let yz = _mm_unpacklo_epi16(hy, zero);
            _mm_storeu_si128(row.as_mut_ptr().add(x).cast(), _mm_unpacklo_epi32(yz, za));
            _mm_storeu_si128(row.as_mut_ptr().add(x + 2).cast(), _mm_unpackhi_epi32(yz, za));
            x += 4;
        }
        while x < len {
            let values = _mm_set_ps(1.0, 0.0, 0.0, luma[x]);
            _mm_storel_epi64(row[x].as_mut_ptr().cast(), _mm_cvtps_ph::<0>(values));
            x += 1;
        }
    }
}

/// Vectorized 4-pixel-per-iteration NEON row packer (`Y, Cb, Cr, 1.0` -> RGBA16F).
#[cfg(target_arch = "aarch64")]
fn pack_half_row_neon(row: &mut [[u16; 4]], planes: [&[f32]; 3]) {
    use std::arch::aarch64::*;
    let len = row.len().min(planes[0].len()).min(planes[1].len()).min(planes[2].len());
    let mut x = 0;
    // SAFETY: NEON is mandatory on aarch64; `x + 4 <= len` keeps every load and store in bounds.
    unsafe {
        let alpha = vdup_n_u16(0x3c00);
        while x + 4 <= len {
            let y = vreinterpret_u16_f16(vcvt_f16_f32(vld1q_f32(planes[0].as_ptr().add(x))));
            let cb = vreinterpret_u16_f16(vcvt_f16_f32(vld1q_f32(planes[1].as_ptr().add(x))));
            let cr = vreinterpret_u16_f16(vcvt_f16_f32(vld1q_f32(planes[2].as_ptr().add(x))));
            vst4_u16(row.as_mut_ptr().add(x).cast(), uint16x4x4_t(y, cb, cr, alpha));
            x += 4;
        }
    }
    while x < len {
        row[x] = [half(planes[0][x]), half(planes[1][x]), half(planes[2][x]), 0x3c00];
        x += 1;
    }
}

/// Vectorized 4-pixel-per-iteration NEON luma-only row packer (`Y, 0, 0, 1.0` -> RGBA16F).
#[cfg(target_arch = "aarch64")]
fn pack_half_luma_row_neon(row: &mut [[u16; 4]], luma: &[f32]) {
    use std::arch::aarch64::*;
    let len = row.len().min(luma.len());
    let mut x = 0;
    // SAFETY: NEON is mandatory on aarch64; `x + 4 <= len` keeps every load and store in bounds.
    unsafe {
        let zero = vdup_n_u16(0);
        let alpha = vdup_n_u16(0x3c00);
        while x + 4 <= len {
            let y = vreinterpret_u16_f16(vcvt_f16_f32(vld1q_f32(luma.as_ptr().add(x))));
            vst4_u16(row.as_mut_ptr().add(x).cast(), uint16x4x4_t(y, zero, zero, alpha));
            x += 4;
        }
    }
    while x < len {
        row[x] = [half(luma[x]), 0, 0, 0x3c00];
        x += 1;
    }
}

/// Scalar round-to-nearest-even FP32-to-FP16 conversion.
pub(super) fn half(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = (bits >> 16) as u16 & 0x8000;
    let exponent = ((bits >> 23) & 255) as i32 - 112;
    let mantissa = bits & 0x7f_ffff;
    if exponent <= 0 {
        if exponent < -10 { return sign; }
        let mantissa = mantissa | 0x80_0000;
        let shift = 14 - exponent;
        return sign | ((mantissa + (1 << (shift - 1)) - 1 + ((mantissa >> shift) & 1)) >> shift) as u16;
    }
    let rounded = mantissa + 0xfff + ((mantissa >> 13) & 1);
    sign | (((exponent as u32) << 10) + (rounded >> 13)) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vectorized_pack_matches_scalar_for_full_and_luma_rows_including_tails() {
        let len = 19; // exercises 4-pixel SIMD loops plus a 3-pixel tail
        let y: Vec<f32> = (0..len).map(|i| (i as f32 - 9.0) * 0.125).collect();
        let cb: Vec<f32> = (0..len).map(|i| (i as f32 - 5.0) * 0.25).collect();
        let cr: Vec<f32> = (0..len).map(|i| -(i as f32) * 0.0625).collect();
        #[cfg(target_arch = "x86_64")]
        let fast = std::is_x86_feature_detected!("f16c");
        #[cfg(not(target_arch = "x86_64"))]
        let fast = false;
        let mut full = vec![[0u16; 4]; len];
        pack_row(&mut full, [&y, &cb, &cr], fast);
        for i in 0..len {
            assert_eq!(full[i], [half(y[i]), half(cb[i]), half(cr[i]), 0x3c00], "full pixel {i}");
        }
        let mut luma = vec![[0u16; 4]; len];
        pack_luma_row(&mut luma, &y, fast);
        for i in 0..len {
            assert_eq!(luma[i], [half(y[i]), 0, 0, 0x3c00], "luma pixel {i}");
        }
    }
}
