//! Converted GPU surface cache for unchanged AMF repeat frames.
//!
//! Static desktops and constant-rate capture repeats pass the same capture
//! texture and conversion parameters frame after frame. Keeping a single
//! weak-keyed entry lets AMF re-wrap the already-converted YUV texture
//! without re-running colour conversion or HDR luminance statistics.
use super::*;
use std::sync::{Arc, Weak};
use windows::Win32::Graphics::{Direct3D11::ID3D11Texture2D, Direct3D12::ID3D12Resource};

/// Cursor identity for repeat-frame conversion reuse. Holds only a weak
/// reference to the pixel buffer so the cache never retains old shapes.
struct CachedCursor {
    pixels: Weak<[u8]>,
    width: u32,
    height: u32,
    logic: bool,
    position: [i32; 2],
}
impl CachedCursor {
    fn from_cursor(cursor: &crate::cursor::Cursor) -> Self {
        Self {
            pixels: Arc::downgrade(&cursor.pixels),
            width: cursor.width,
            height: cursor.height,
            logic: cursor.logic,
            position: cursor.position,
        }
    }
    fn matches(&self, cursor: &crate::cursor::Cursor) -> bool {
        self.width == cursor.width
            && self.height == cursor.height
            && self.logic == cursor.logic
            && self.position == cursor.position
            && self
                .pixels
                .upgrade()
                .is_some_and(|pixels| Arc::ptr_eq(&pixels, &cursor.pixels))
    }
}

pub(super) enum CachedTexture {
    D3d12(Arc<ID3D12Resource>),
    D3d11(Arc<ID3D11Texture2D>),
}

/// Last converted YUV texture for unchanged repeat frames. Holds only a weak
/// reference to the capture texture so ring and pool slots are never pinned.
pub(super) struct CachedConverted {
    source: Weak<ID3D11Texture2D>,
    acquired: Instant,
    origin: u64,
    constants: [u32; 20],
    cursor: Option<CachedCursor>,
    pub(super) texture: CachedTexture,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn repeat_source_matches(
    repeat: bool,
    same_texture: bool,
    cached_acquired: Instant,
    image_acquired: Instant,
    cached_origin: u64,
    image_origin: u64,
    image_repeat: bool,
    same_constants: bool,
    same_cursor: bool,
) -> bool {
    repeat
        && same_texture
        && same_constants
        && same_cursor
        && (cached_acquired == image_acquired
            || (image_repeat && cached_origin != 0 && cached_origin == image_origin))
}

impl CachedConverted {
    pub(super) fn new(image: &GpuImage, constants: [u32; 20], texture: CachedTexture) -> Self {
        Self {
            source: Arc::downgrade(&image.texture),
            acquired: image.acquired,
            origin: image.origin,
            constants,
            cursor: image.cursor.as_ref().map(CachedCursor::from_cursor),
            texture,
        }
    }
    pub(super) fn matches(&self, image: &GpuImage, constants: &[u32; 20], repeat: bool) -> bool {
        let same_texture = self
            .source
            .upgrade()
            .is_some_and(|source| Arc::ptr_eq(&source, &image.texture));
        let same_cursor = match (&self.cursor, &image.cursor) {
            (None, None) => true,
            (Some(cached), Some(cursor)) => cached.matches(cursor),
            _ => false,
        };
        repeat_source_matches(
            repeat,
            same_texture,
            self.acquired,
            image.acquired,
            self.origin,
            image.origin,
            image.repeat,
            self.constants == *constants,
            same_cursor,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeat_conversion_cache_matches_only_unchanged_repeats() {
        let t0 = Instant::now();
        let t1 = t0 + Duration::from_millis(16);
        // Static repeat of the same capture texture and timestamp reuses conversion.
        assert!(repeat_source_matches(
            true, true, t0, t0, 0, 0, false, true, true
        ));
        // Constant-rate capture repeat (`image_repeat` + matching nonzero `origin`)
        // also reuses conversion even though `acquired` advances per slot.
        assert!(repeat_source_matches(
            true, true, t0, t1, 42, 42, true, true, true
        ));
        // Any mismatch or fresh frame forces a fresh conversion.
        assert!(!repeat_source_matches(
            false, true, t0, t0, 0, 0, false, true, true
        ));
        assert!(!repeat_source_matches(
            true, false, t0, t0, 0, 0, false, true, true
        ));
        assert!(!repeat_source_matches(
            true, true, t0, t1, 0, 0, false, true, true
        ));
        assert!(!repeat_source_matches(
            true, true, t0, t1, 0, 0, true, true, true
        ));
        assert!(!repeat_source_matches(
            true, true, t0, t1, 42, 43, true, true, true
        ));
        assert!(!repeat_source_matches(
            true, true, t0, t1, 42, 42, false, true, true
        ));
        assert!(!repeat_source_matches(
            true, true, t0, t0, 0, 0, false, false, true
        ));
        assert!(!repeat_source_matches(
            true, true, t0, t0, 0, 0, false, true, false
        ));
    }

    #[test]
    #[ignore = "requires a live AMD adapter with H.264, HEVC and AV1 encoding"]
    fn native_repeat_frames_reuse_converted_surface_across_codecs_and_paths() -> Result<()> {
        let _com = crate::capture::ComGuard::new()?;
        let device = Device::new("")?;
        let compute = crate::compute::Compute::for_device(&device.device)?;
        for (compute_on, label) in [(true, "d3d12"), (false, "d3d11")] {
            let options = butterpollo_config::config::Config::parse(&format!(
                "gpu_compute_conversion={compute_on}\namd_vpe=false\nhdr_dynamic_metadata=true\n"
            ))?;
            for (codec, hdr) in [(0, false), (1, true), (2, true)] {
                let config = butterpollo_media_policy::rtsp::Negotiated {
                    width: 1920,
                    height: 1080,
                    codec,
                    hdr,
                    fps: 60,
                    bitrate_kbps: 20_000,
                    ..Default::default()
                };
                let pixel = if hdr {
                    crate::capture::Pixel::RgbaF16
                } else {
                    crate::capture::Pixel::Bgra8
                };
                let bpp = if hdr { 8 } else { 4 };
                let make_frame = |val: u8| -> Result<GpuImage> {
                    let bytes = if hdr {
                        let half = half::f16::from_f32(f32::from(val) / 64.)
                            .to_bits()
                            .to_le_bytes();
                        let mut out = Vec::with_capacity(
                            config.width as usize * config.height as usize * bpp,
                        );
                        for _ in 0..config.width as usize * config.height as usize {
                            out.extend_from_slice(&half);
                            out.extend_from_slice(&half);
                            out.extend_from_slice(&half);
                            out.extend_from_slice(&half::f16::ONE.to_bits().to_le_bytes());
                        }
                        out
                    } else {
                        vec![val; config.width as usize * config.height as usize * bpp]
                    };
                    GpuImage::upload_settled(
                        &device,
                        &Image {
                            width: config.width,
                            height: config.height,
                            stride: config.width as usize * bpp,
                            bytes,
                            captured: Instant::now(),
                            pixel,
                        },
                    )
                };
                let frame_a = make_frame(64)?;
                let frame_b = make_frame(192)?;
                let mut encoder = Encoder::new_gpu(
                    &config,
                    device.clone(),
                    &options,
                    compute_on.then(|| compute.clone()),
                )?;
                assert_eq!(
                    encoder.converts_on_compute(),
                    compute_on,
                    "{label} codec {codec}: unexpected conversion queue"
                );
                let mut output = Vec::new();
                let drain = |encoder: &mut Encoder, output: &mut Vec<Encoded>| -> Result<()> {
                    let deadline = Instant::now() + Duration::from_secs(2);
                    while encoder.pending() && Instant::now() < deadline {
                        output.extend(encoder.poll()?);
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    anyhow::ensure!(!encoder.pending(), "encoder did not drain");
                    Ok(())
                };
                // Frame 1: fresh IDR populates cache.
                encoder.set_repeat(false);
                output.extend(encoder.encode_gpu(&frame_a, true, config.bitrate_kbps)?);
                assert_eq!(encoder.conversion_reuses(), 0);
                // Frames 2..=4: three repeats submitted back-to-back without draining first,
                // exercising concurrent AMF surfaces wrapping the same cached texture.
                for _ in 0..3 {
                    let mut repeat = frame_a.clone();
                    repeat.captured = Instant::now();
                    encoder.set_repeat(true);
                    output.extend(encoder.encode_gpu(&repeat, false, config.bitrate_kbps)?);
                }
                assert_eq!(
                    encoder.conversion_reuses(),
                    3,
                    "{label} codec {codec}: back-to-back repeats should reuse"
                );
                drain(&mut encoder, &mut output)?;
                // Frames 5..=20: steady-state repeats.
                for _ in 0..16 {
                    let mut repeat = frame_a.clone();
                    repeat.captured = Instant::now();
                    encoder.set_repeat(true);
                    output.extend(encoder.encode_gpu(&repeat, false, config.bitrate_kbps)?);
                    drain(&mut encoder, &mut output)?;
                }
                assert_eq!(
                    encoder.conversion_reuses(),
                    19,
                    "{label} codec {codec}: steady repeats should reuse"
                );
                // Luminance change invalidates cache even if repeat is set.
                encoder.luminance = [200., 1.];
                let mut repeat = frame_a.clone();
                repeat.captured = Instant::now();
                encoder.set_repeat(true);
                output.extend(encoder.encode_gpu(&repeat, false, config.bitrate_kbps)?);
                assert_eq!(
                    encoder.conversion_reuses(),
                    19,
                    "{label} codec {codec}: luminance change must re-convert"
                );
                // Next repeat at the new luminance reuses the new cached conversion.
                let mut repeat = frame_a.clone();
                repeat.captured = Instant::now();
                encoder.set_repeat(true);
                output.extend(encoder.encode_gpu(&repeat, false, config.bitrate_kbps)?);
                assert_eq!(
                    encoder.conversion_reuses(),
                    20,
                    "{label} codec {codec}: repeat after luminance change should reuse"
                );
                // Overlay invalidates cache and disables caching while open.
                encoder.overlay = Some(crate::overlay::Overlay::new(
                    1,
                    [0, 0],
                    [64, 64],
                    1,
                    Arc::from(vec![255u8; 64 * 64 * 4]),
                ));
                let mut repeat = frame_a.clone();
                repeat.captured = Instant::now();
                encoder.set_repeat(true);
                output.extend(encoder.encode_gpu(&repeat, false, config.bitrate_kbps)?);
                assert_eq!(
                    encoder.conversion_reuses(),
                    20,
                    "{label} codec {codec}: overlay must re-convert"
                );
                // Closing overlay invalidates and re-converts cleanly without menu.
                encoder.overlay = None;
                let mut repeat = frame_a.clone();
                repeat.captured = Instant::now();
                encoder.set_repeat(true);
                output.extend(encoder.encode_gpu(&repeat, false, config.bitrate_kbps)?);
                assert_eq!(
                    encoder.conversion_reuses(),
                    20,
                    "{label} codec {codec}: closing overlay must re-convert"
                );
                // New fresh capture texture invalidates cache.
                encoder.set_repeat(false);
                output.extend(encoder.encode_gpu(&frame_b, false, config.bitrate_kbps)?);
                assert_eq!(encoder.conversion_reuses(), 20);
                let mut repeat_b = frame_b.clone();
                repeat_b.captured = Instant::now();
                encoder.set_repeat(true);
                output.extend(encoder.encode_gpu(&repeat_b, false, config.bitrate_kbps)?);
                assert_eq!(encoder.conversion_reuses(), 21);
                drain(&mut encoder, &mut output)?;
                assert_eq!(
                    output.len(),
                    26,
                    "{label} codec {codec}: every frame returned"
                );
                assert!(output.iter().all(|f| !f.bytes.is_empty()));
                assert!(output[0].idr);
                println!(
                    "repeat_reuse_ok path={label} codec={codec} hdr={hdr} frames={} reuses={}",
                    output.len(),
                    encoder.conversion_reuses()
                );
            }
        }
        Ok(())
    }
}
