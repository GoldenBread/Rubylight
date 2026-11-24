//! The packed 4:4:4 pass against the protocol's reference packing, and its SPIR-V against its
//! source.
use super::*;
use crate::vaapi::{
    BGRX, NV12,
    ffi::{VA_RT_FORMAT_RGB32, VA_RT_FORMAT_YUV420},
    pool::Pool,
};
use std::sync::Arc;

/// The checked-in SPIR-V is `pack.comp` compiled, when glslc is installed to tell.
#[test]
fn the_shader_is_the_compiled_source() {
    let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/packed444");
    let out = std::env::temp_dir().join(format!("pack-{}.spv", std::process::id()));
    let compiled = std::process::Command::new("glslc")
        .args(["--target-env=vulkan1.2", "-O"])
        .arg(folder.join("pack.comp"))
        .arg("-o")
        .arg(&out)
        .status();
    match compiled {
        Ok(status) if status.success() => {
            let fresh = std::fs::read(&out).unwrap();
            let _ = std::fs::remove_file(&out);
            assert!(
                fresh == SHADER,
                "pack.spv is not pack.comp compiled: run the glslc line in pack.comp"
            );
        }
        _ => eprintln!("glslc is not installed: pack.spv was not compared with pack.comp"),
    }
}

/// What the shader writes for one pixel, on the CPU: `lumaCode` and `chromaCode`.
fn codes(rgb: [u8; 3], matrix: u8, full: bool) -> [u8; 3] {
    let k: [f32; 3] = match matrix {
        2 => [0.2627, 0.6780, 0.0593],
        0 => [0.299, 0.587, 0.114],
        _ => [0.2126, 0.7152, 0.0722],
    };
    let [r, g, b] = rgb.map(|c| f32::from(c) / 255.0);
    let y = r * k[0] + g * k[1] + b * k[2];
    let code = |v: f32| (v.clamp(0.0, 255.0) + 0.5).floor() as u8;
    let luma = code(if full { 255.0 * y } else { 16.0 + 219.0 * y });
    let scale = if full { 255.0 } else { 224.0 };
    let cb = code(128.0 + scale * (b - y) / (2.0 * (1.0 - k[2])));
    let cr = code(128.0 + scale * (r - y) / (2.0 * (1.0 - k[0])));
    [luma, cb, cr]
}

/// Packs a pseudo-random `width`x`height` picture on the GPU and returns the decoded-picture
/// samples (dense planar Y, Cb, Cr of the coded picture) and the reference packing of the same
/// codes.
fn packed(width: u32, height: u32, matrix: u8, full: bool) -> Option<(Vec<u8>, Vec<u8>)> {
    let Some(node) = crate::vaapi::node::nodes().first().cloned() else {
        eprintln!("skipped: no render node");
        return None;
    };
    let va = Arc::new(Va::open(&node.path).expect("VA-API"));
    let mut packer = Packer::new(&va, (width, height), matrix, full).expect("the pass");
    let (coded_width, coded_height) = packer.coded_size();
    let stride = width as usize * 4;
    let mut seed = 0x2545_f491_u32;
    let bgra: Vec<u8> = (0..stride * height as usize)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed >> 24) as u8
        })
        .collect();
    let sources = Pool::new_linear(&va, VA_RT_FORMAT_RGB32, BGRX, width, height, 1, 1).unwrap();
    let targets = Pool::new_linear(
        &va,
        VA_RT_FORMAT_YUV420,
        NV12,
        coded_width,
        coded_height,
        1,
        1,
    )
    .unwrap();
    let source = sources.take().unwrap();
    let target = targets.take().unwrap();
    va.upload_bgra(source.id(), width, height, stride, &bgra)
        .unwrap();
    va.sync(source.id()).unwrap();
    packer.pack(&source, &target).expect("packing");
    // Again, reusing both imports: the pass's own time, from submission to its fence.
    let mut times: Vec<f64> = (0..20)
        .map(|_| {
            let started = std::time::Instant::now();
            packer.pack(&source, &target).expect("packing again");
            started.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    times.sort_by(f64::total_cmp);
    eprintln!(
        "{width}x{height}: a pass takes {:.3} ms (median of 20)",
        times[10]
    );

    let exported = dmabuf::export_surface(&target).unwrap();
    // SAFETY: a seek on the exported descriptor.
    let size = unsafe { libc::lseek(exported.fd.as_raw_fd(), 0, libc::SEEK_END) } as usize;
    // SAFETY: a read-only shared mapping of the whole DMA-BUF, unmapped below.
    let mapped = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            size,
            libc::PROT_READ,
            libc::MAP_SHARED,
            exported.fd.as_raw_fd(),
            0,
        )
    };
    assert_ne!(mapped, libc::MAP_FAILED, "mapping the target");
    // SAFETY: `size` mapped bytes.
    let bytes = unsafe { std::slice::from_raw_parts(mapped.cast::<u8>(), size) };
    let (pitch, chroma) = (
        exported.planes[0].pitch as usize,
        exported.planes[1].offset as usize,
    );
    let (w, h) = (coded_width as usize, coded_height as usize);
    let mut decoded = Vec::with_capacity(w * h * 3 / 2);
    for row in 0..h {
        decoded.extend_from_slice(&bytes[row * pitch..row * pitch + w]);
    }
    for plane in 0..2 {
        for row in 0..h / 2 {
            let line = &bytes[chroma + row * pitch..chroma + row * pitch + w];
            decoded.extend(line.iter().skip(plane).step_by(2));
        }
    }
    // SAFETY: the mapping made above.
    unsafe { libc::munmap(mapped, size) };

    let count = (width * height) as usize;
    let mut planes = vec![vec![0u8; count]; 3];
    for i in 0..count {
        let [b, g, r] = [bgra[i * 4], bgra[i * 4 + 1], bgra[i * 4 + 2]];
        for (plane, code) in codes([r, g, b], matrix, full).into_iter().enumerate() {
            planes[plane][i] = code;
        }
    }
    let geometry = Geometry::new(width, height).unwrap();
    let mut reference = vec![0u8; geometry.samples()];
    assert!(geometry.pack([&planes[0], &planes[1], &planes[2]], &mut reference));
    Some((decoded, reference))
}

/// The GPU's codes are the reference packing's: every sample where Geometry::pack puts it,
/// with at most a code's difference, in at most 0.1% of samples, where the GPU's fused float
/// arithmetic rounds a code the other way (25 of 6,220,800 at 1080p on an RX 7800 XT).
#[test]
#[ignore = "needs a GPU with VA-API and Vulkan"]
fn the_pass_writes_the_reference_packing() {
    for (width, height, matrix, full) in [
        (1920, 1080, 1, false),
        (130, 70, 0, true),
        (1366, 768, 2, false),
    ] {
        let Some((decoded, reference)) = packed(width, height, matrix, full) else {
            return;
        };
        assert_eq!(decoded.len(), reference.len(), "{width}x{height}");
        let differing: Vec<(usize, u8, u8)> = decoded
            .iter()
            .zip(&reference)
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .map(|(i, (&a, &b))| (i, a, b))
            .collect();
        let worst = differing
            .iter()
            .map(|&(_, a, b)| a.abs_diff(b))
            .max()
            .unwrap_or(0);
        assert!(
            worst <= 1 && differing.len() * 1_000 <= reference.len(),
            "{width}x{height} matrix {matrix} full {full}: {} of {} samples differ, by up to \
             {worst}; first {:?}",
            differing.len(),
            reference.len(),
            &differing[..differing.len().min(8)]
        );
        eprintln!(
            "{width}x{height}: {} of {} samples a code off",
            differing.len(),
            reference.len()
        );
    }
}
