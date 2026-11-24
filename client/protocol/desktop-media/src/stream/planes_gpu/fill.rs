//! CPU-to-transfer-buffer plane packing for `planes_gpu::PlanesPass`.

use std::ptr;

/// Whether a picture's chroma is one interleaved plane (NV12, P010).
pub(super) fn semi_planar(info: &crate::media::FrameInfo) -> bool {
    matches!(info.kind, crate::media::FRAME_NV12 | crate::media::FRAME_P010)
}

/// Whether a planar picture's 16-bit samples hold its 10-bit codes in their high bits, as P010's
/// do: NVDEC's 10-bit 4:4:4 copied to memory (`media::FRAME_YUV444P16`).
pub(super) fn high_bit_planes(info: &crate::media::FrameInfo) -> bool {
    info.kind == crate::media::FRAME_YUV444P16
}

/// Copies an NV12 or P010 picture's luma and interleaved chroma rows directly into the transfer
/// buffer without CPU bit shifts or UV deinterleaving (paired with shader flag 256 / 272 and
/// `R8G8_UNORM` / `R16G16_UNORM` chroma texture).
///
/// # Safety
/// `info` describes a live NV12 or P010 frame covering `sizes`, and `mapped` has room at `offsets`.
pub(super) unsafe fn fill_semi_planar(
    info: &crate::media::FrameInfo,
    sizes: [(u32, u32); 2],
    sample_bytes: usize,
    mapped: *mut u8,
    offsets: [usize; 2],
) {
    let row_bytes = [
        sizes[0].0 as usize * sample_bytes,
        sizes[1].0 as usize * sample_bytes * 2,
    ];
    for plane in 0..2 {
        let (_, height) = sizes[plane];
        let row = row_bytes[plane];
        let stride = info.linesize[plane] as usize;
        for y in 0..height as usize {
            // SAFETY: Caller guarantees `info.data[plane]` and `mapped` cover `sizes[plane]`.
            unsafe {
                ptr::copy_nonoverlapping(
                    info.data[plane].add(y * stride),
                    mapped.add(offsets[plane] + y * row),
                    row,
                );
            }
        }
    }
}

/// Copies a picture's planes into the transfer buffer as the tightly packed planes `sizes` gives.
/// A planar picture's rows go as they are. An NV12 or P010 picture's interleaved chroma is split
/// into its Cb and Cr planes, and P010's samples, in the high 10 bits of each word, move to the low
/// bits, where the shader reads FFmpeg's 10-bit planes (flag 64); so do the samples of a planar
/// picture with high-bit planes (`high_bit_planes`).
///
/// # Safety
/// `info` describes a live frame with at least `sizes` samples in each plane, and `mapped` has
/// room for the planes at `offsets`.
pub(super) unsafe fn fill_planes(
    info: &crate::media::FrameInfo,
    sizes: [(u32, u32); 3],
    sample_bytes: usize,
    mapped: *mut u8,
    offsets: [usize; 3],
) {
    if !semi_planar(info) {
        let high_bits = sample_bytes == 2 && high_bit_planes(info);
        for (plane, &(width, height)) in sizes.iter().enumerate() {
            let row = width as usize * sample_bytes;
            for y in 0..height as usize {
                // SAFETY: Caller guarantees `info.data[plane]` and `mapped` cover `sizes` at `linesize`/`offsets`.
                unsafe {
                    let source = info.data[plane].add(y * info.linesize[plane] as usize);
                    let target = mapped.add(offsets[plane] + y * row);
                    if high_bits {
                        let source = std::slice::from_raw_parts(source, row);
                        let target = std::slice::from_raw_parts_mut(target, row);
                        for (target, source) in target.as_chunks_mut::<2>().0.iter_mut().zip(source.as_chunks::<2>().0) {
                            target.copy_from_slice(&(u16::from_le_bytes([source[0], source[1]]) >> 6).to_le_bytes());
                        }
                    } else {
                        ptr::copy_nonoverlapping(source, target, row);
                    }
                }
            }
        }
        return;
    }
    let p010 = info.kind == crate::media::FRAME_P010;
    let put = |target: &mut [u8], source: &[u8]| {
        if p010 {
            target.copy_from_slice(&(u16::from_le_bytes([source[0], source[1]]) >> 6).to_le_bytes());
        } else {
            target.copy_from_slice(source);
        }
    };
    let (width, height) = sizes[0];
    let row = width as usize * sample_bytes;
    for y in 0..height as usize {
        // SAFETY: Caller guarantees `info.data[0]` and `mapped` cover luma `sizes[0]` at `linesize[0]`/`offsets[0]`.
        let (source, target) = unsafe {
            (
                std::slice::from_raw_parts(info.data[0].add(y * info.linesize[0] as usize), row),
                std::slice::from_raw_parts_mut(mapped.add(offsets[0] + y * row), row),
            )
        };
        if p010 {
            target.as_chunks_mut::<2>().0.iter_mut().zip(source.as_chunks::<2>().0).for_each(|(target, source)| put(target, source));
        } else {
            target.copy_from_slice(source);
        }
    }
    let (width, height) = sizes[1];
    let row = width as usize * sample_bytes;
    for y in 0..height as usize {
        // SAFETY: Caller guarantees `info.data[1]` and `mapped` cover chroma `sizes[1]`/`sizes[2]` at `linesize[1]`/`offsets`.
        let (source, cb, cr) = unsafe {
            (
                std::slice::from_raw_parts(info.data[1].add(y * info.linesize[1] as usize), 2 * row),
                std::slice::from_raw_parts_mut(mapped.add(offsets[1] + y * row), row),
                std::slice::from_raw_parts_mut(mapped.add(offsets[2] + y * row), row),
            )
        };
        for (pair, (cb, cr)) in source.chunks_exact(2 * sample_bytes).zip(cb.chunks_exact_mut(sample_bytes).zip(cr.chunks_exact_mut(sample_bytes))) {
            put(cb, &pair[..sample_bytes]);
            put(cr, &pair[sample_bytes..]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_semi_planar_copies_p010_rows_without_shifting_or_deinterleaving() {
        let mut y_src = vec![0xAAu8; 2 * 16];
        for (i, w) in [64u16 << 6, 512 << 6, 940 << 6, 1023 << 6].into_iter().enumerate() {
            y_src[i * 2..i * 2 + 2].copy_from_slice(&w.to_le_bytes());
            y_src[16 + i * 2..16 + i * 2 + 2].copy_from_slice(&(w ^ 0x40).to_le_bytes());
        }
        let mut uv_src = vec![0xBBu8; 16];
        for (i, w) in [256u16 << 6, 768 << 6, 400 << 6, 600 << 6].into_iter().enumerate() {
            uv_src[i * 2..i * 2 + 2].copy_from_slice(&w.to_le_bytes());
        }
        let mut info: crate::media::FrameInfo = unsafe { std::mem::zeroed() };
        info.kind = crate::media::FRAME_P010;
        info.width = 4;
        info.height = 2;
        info.data[0] = y_src.as_mut_ptr();
        info.data[1] = uv_src.as_mut_ptr();
        info.linesize[0] = 16;
        info.linesize[1] = 16;
        let mut dst = vec![0u8; 16 + 8];
        // SAFETY: Buffers cover 4x2 luma and 2x1 interleaved UV at stride 16.
        unsafe { fill_semi_planar(&info, [(4, 2), (2, 1)], 2, dst.as_mut_ptr(), [0, 16]) };
        assert_eq!(&dst[0..8], &y_src[0..8]);
        assert_eq!(&dst[8..16], &y_src[16..24]);
        assert_eq!(&dst[16..24], &uv_src[0..8]);
    }
}
