//! Shared AMF VPE A/B test runtime, texture readback and image-quality helpers.

use super::*;

/// An AMF context on the test's device, as the encoder makes one.
pub(super) struct Runtime {
    _library: libloading::Library,
    pub(super) factory: *mut AMFFactory,
    pub(super) context: *mut AMFContext,
}
impl Runtime {
    pub(super) fn new(gpu: &Device) -> Result<Self> {
        // SAFETY: As in `Encoder::create_attempt`: AMFInit's C ABI, owned interfaces released in
        // Drop, and the library outliving them.
        unsafe {
            let library = libloading::Library::new("amfrt64.dll").context("AMD AMF runtime")?;
            let init: libloading::Symbol<
                unsafe extern "C" fn(u64, *mut *mut AMFFactory) -> AMF_RESULT,
            > = library.get(b"AMFInit\0")?;
            let version = ((AMF_VERSION_MAJOR as u64) << 48)
                | ((AMF_VERSION_MINOR as u64) << 32)
                | ((AMF_VERSION_RELEASE as u64) << 16)
                | AMF_VERSION_BUILD_NUM as u64;
            let mut factory = ptr::null_mut();
            check(init(version, &mut factory)).context("AMFInit")?;
            let mut context = ptr::null_mut();
            check(((*(*factory).pVtbl).CreateContext.unwrap())(
                factory,
                &mut context,
            ))?;
            check(((*(*context).pVtbl).InitDX11.unwrap())(
                context,
                gpu.device.as_raw(),
                AMF_DX_VERSION_AMF_DX11_1,
            ))
            .context("AMF InitDX11")?;
            Ok(Self {
                _library: library,
                factory,
                context,
            })
        }
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        // SAFETY: Self owns the context's reference; every converter is dropped first.
        unsafe {
            ((*(*self.context).pVtbl).Terminate.unwrap())(self.context);
            ((*(*self.context).pVtbl).Release.unwrap())(self.context);
        }
    }
}

/// `AMFTextureArrayIndexGUID` (AMF samples' DX11 interop): the slice of a
/// texture array an AMF surface lives in.
pub(super) const ARRAY_INDEX: GUID = GUID::from_values(
    0x2811_5527,
    0xe7c3,
    0x4b66,
    [0x99, 0xd3, 0x4f, 0x2a, 0xe6, 0xb4, 0x7f, 0xaf],
);
/// The D3D11 texture and subresource behind an AMF surface.
pub(super) fn native(surface: &gpu::Surface) -> Result<(ID3D11Texture2D, u32)> {
    // SAFETY: `surface` is a live DX11 surface; its plane's native pointer is a texture the
    // surface keeps alive, cloned (AddRef) before use; the private data read writes four bytes.
    unsafe {
        let v = &*(*surface.0).pVtbl;
        check((v.Convert.unwrap())(
            surface.0,
            AMF_MEMORY_TYPE_AMF_MEMORY_DX11,
        ))?;
        let plane = (v.GetPlaneAt.unwrap())(surface.0, 0);
        anyhow::ensure!(!plane.is_null(), "AMF surface has no plane");
        let raw = ((*(*plane).pVtbl).GetNative.unwrap())(plane);
        let texture = ID3D11Texture2D::from_raw_borrowed(&raw)
            .context("AMF surface has no texture")?
            .clone();
        let mut index = 0u32;
        let mut size = 4u32;
        let _ = texture.GetPrivateData(
            &ARRAY_INDEX,
            &mut size,
            Some(ptr::addr_of_mut!(index).cast()),
        );
        Ok((texture, index))
    }
}
/// The stream's `width` x `height` NV12 or P010 codes from a texture.
pub(super) fn read_codes(
    gpu: &Device,
    texture: &ID3D11Texture2D,
    subresource: u32,
    width: usize,
    height: usize,
) -> Result<Vec<u16>> {
    // SAFETY: The staging copy has the texture's own size and format; each row read lies
    // within the mapped planes (chroma after the allocated luma rows) until Unmap.
    unsafe {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        texture.GetDesc(&mut desc);
        let ten_bit = desc.Format == DXGI_FORMAT_P010;
        anyhow::ensure!(
            ten_bit || desc.Format == DXGI_FORMAT_NV12,
            "converter wrote format {:?}",
            desc.Format
        );
        desc.ArraySize = 1;
        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        desc.MiscFlags = 0;
        let mut staging = None;
        gpu.device
            .CreateTexture2D(&desc, None, Some(&mut staging))?;
        let staging = staging.unwrap();
        gpu.context
            .CopySubresourceRegion(&staging, 0, 0, 0, 0, texture, subresource, None);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        gpu.context
            .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        let pitch = mapped.RowPitch as usize;
        let base = mapped.pData as *const u8;
        let bytes = if ten_bit { 2 } else { 1 };
        let mut codes = Vec::with_capacity(width * height * 3 / 2);
        let row = |at: usize| std::slice::from_raw_parts(base.add(at), width * bytes);
        let mut push = |data: &[u8]| {
            if ten_bit {
                codes.extend(
                    data.as_chunks::<2>()
                        .0
                        .iter()
                        .map(|v| u16::from_le_bytes(*v) >> 6),
                );
            } else {
                codes.extend(data.iter().map(|&v| u16::from(v)));
            }
        };
        for y in 0..height {
            push(row(y * pitch));
        }
        let chroma = pitch * desc.Height as usize;
        for y in 0..height / 2 {
            push(row(chroma + y * pitch));
        }
        gpu.context.Unmap(&staging, 0);
        Ok(codes)
    }
}
/// Wait until the GPU finished writing `texture`, by reading two pixels.
pub(super) fn finish(gpu: &Device, texture: &ID3D11Texture2D, subresource: u32) -> Result<()> {
    // SAFETY: A 2x2 staging texture of the source's format receives a 2x2 copy (even, as 4:2:0
    // requires) and is mapped and unmapped at once.
    unsafe {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        texture.GetDesc(&mut desc);
        desc.Width = 2;
        desc.Height = 2;
        desc.ArraySize = 1;
        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        desc.MiscFlags = 0;
        let mut staging = None;
        gpu.device
            .CreateTexture2D(&desc, None, Some(&mut staging))?;
        let staging = staging.unwrap();
        gpu.context.CopySubresourceRegion(
            &staging,
            0,
            0,
            0,
            0,
            texture,
            subresource,
            Some(&D3D11_BOX {
                left: 0,
                top: 0,
                front: 0,
                right: 2,
                bottom: 2,
                back: 1,
            }),
        );
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        gpu.context
            .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        gpu.context.Unmap(&staging, 0);
        Ok(())
    }
}

/// `picture` (in its source encoding) as an uploadable capture.
pub(super) fn image(picture: &q::Picture, pixel: Pixel) -> Image {
    let (w, h) = (picture.width, picture.height);
    let bytes_per = if pixel == Pixel::RgbaF16 { 8 } else { 4 };
    let mut bytes = Vec::with_capacity(w * h * bytes_per);
    for p in &picture.rgb {
        match pixel {
            Pixel::Bgra8 => {
                let c = p.map(|v| (v.clamp(0., 1.) * 255.).round() as u8);
                bytes.extend_from_slice(&[c[2], c[1], c[0], 255]);
            }
            Pixel::RgbaF16 => {
                for v in p.iter().copied().chain([1.]) {
                    bytes.extend_from_slice(&half::f16::from_f64(v).to_le_bytes());
                }
            }
            Pixel::Rgba10Pq => {
                let c = p.map(|v| (v.clamp(0., 1.) * 1023.).round() as u32);
                let packed = c[0] | c[1] << 10 | c[2] << 20 | 3 << 30;
                bytes.extend_from_slice(&packed.to_le_bytes());
            }
        }
    }
    Image {
        width: w as u32,
        height: h as u32,
        stride: w * bytes_per,
        bytes,
        captured: std::time::Instant::now(),
        pixel,
    }
}
/// The picture the uploaded capture holds, after its own quantization, so
/// the reference starts from the same pixels the converters read.
pub(super) fn quantized(image: &Image) -> q::Picture {
    let (w, h) = (image.width as usize, image.height as usize);
    let rgb = match image.pixel {
        Pixel::Bgra8 => image
            .bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| [b[2], b[1], b[0]].map(|v| f64::from(v) / 255.))
            .collect(),
        Pixel::RgbaF16 => image
            .bytes
            .as_chunks::<8>()
            .0
            .iter()
            .map(|b| [0, 2, 4].map(|i| half::f16::from_le_bytes([b[i], b[i + 1]]).to_f64()))
            .collect(),
        Pixel::Rgba10Pq => image
            .bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| {
                let v = u32::from_le_bytes(*b);
                [v & 1023, v >> 10 & 1023, v >> 20 & 1023].map(|c| f64::from(c) / 1023.)
            })
            .collect(),
    };
    q::Picture {
        width: w,
        height: h,
        rgb,
    }
}

pub(super) fn negotiated(transfer: Transfer, width: u32, height: u32) -> Negotiated {
    Negotiated {
        width,
        height,
        codec: 1,
        hdr: transfer != Transfer::Sdr,
        // rubylight_protocol::hdr::HLG
        hdr_format: if transfer == Transfer::Hlg { 2 } else { 0 },
        // Rec.709, studio range.
        csc_mode: 2,
        ..Default::default()
    }
}
pub(super) fn spec(config: &Negotiated, pixel: Pixel) -> q::Spec {
    let stream = butterpollo_media_policy::amf_vpe::Stream::of(config);
    q::Spec {
        width: config.width as usize,
        height: config.height as usize,
        source: source_of(pixel),
        transfer: stream.transfer,
        matrix: stream.matrix,
        full_range: stream.full_range,
        ten_bit: stream.ten_bit,
    }
}

/// A test card in the stream's own terms: colour bars, a grey ramp, fine
/// lines one and two pixels wide and a zone plate, which aliasing shows on.
pub(super) fn card(spec: &q::Spec, width: usize, height: usize) -> q::Picture {
    let bars: [[f64; 3]; 8] = [
        [0.75, 0.75, 0.75],
        [0.75, 0.75, 0.],
        [0., 0.75, 0.75],
        [0., 0.75, 0.],
        [0.75, 0., 0.75],
        [0.75, 0., 0.],
        [0., 0., 0.75],
        [1., 1., 1.],
    ];
    let mut rgb = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            let (u, v) = (x as f64 / width as f64, y as f64 / height as f64);
            let value = if v < 0.25 {
                bars[(u * 8.) as usize % 8]
            } else if v < 0.4 {
                [u; 3]
            } else if v < 0.55 {
                let line = if u < 0.5 { x % 2 } else { (x / 2) % 2 };
                [line as f64 * 0.8 + 0.1; 3]
            } else {
                let (cx, cy) = (u - 0.5, (v - 0.775) * height as f64 / width as f64);
                let phase = (cx * cx + cy * cy) * width as f64 * 3.;
                let s = 0.5 + 0.4 * phase.cos();
                [s, 0.5 + 0.3 * (phase * 0.5).sin(), 1. - s]
            };
            rgb.push(q::source_for(spec, value));
        }
    }
    q::Picture { width, height, rgb }
}

/// One converter's scores and times on one case.
pub(super) fn score(spec: &q::Spec, codes: &[u16], reference: &q::Planes) -> Value {
    let Some(planes) = q::Planes::from_semi_planar(codes, spec.width, spec.height) else {
        return json!({"error": "short readback"});
    };
    let peak = spec.peak();
    let [mean, p99, worst] = q::delta_e(&planes, reference, spec);
    json!({
        "psnr_y": q::psnr(&planes.y, &reference.y, peak),
        "psnr_cb": q::psnr(&planes.cb, &reference.cb, peak),
        "psnr_cr": q::psnr(&planes.cr, &reference.cr, peak),
        "ssim_y": q::ssim(&planes.y, &reference.y, spec.width, spec.height, peak),
        "delta_e_itp_mean": mean,
        "delta_e_itp_p99": p99,
        "delta_e_itp_max": worst,
    })
}
pub(super) fn times(mut micros: Vec<f64>) -> Value {
    if micros.is_empty() {
        return Value::Null;
    }
    micros.sort_by(|a, b| a.total_cmp(b));
    let at = |p: f64| micros[((micros.len() - 1) as f64 * p).round() as usize];
    json!({
        "frames": micros.len(),
        "median_us": at(0.5),
        "p95_us": at(0.95),
        "worst_us": micros[micros.len() - 1],
        "mean_us": micros.iter().sum::<f64>() / micros.len() as f64,
    })
}
/// Chroma siting from a horizontal and a vertical ramp's planes.
pub(super) fn siting(spec: &q::Spec, across: &[u16], down: &[u16]) -> Value {
    let planes = |codes: &[u16]| q::Planes::from_semi_planar(codes, spec.width, spec.height);
    let (Some(across), Some(down)) = (planes(across), planes(down)) else {
        return Value::Null;
    };
    let (cw, ch) = (spec.width / 2, spec.height / 2);
    let row: Vec<f64> = across.cr[(ch / 2) * cw..(ch / 2 + 1) * cw].to_vec();
    let column: Vec<f64> = (0..ch).map(|j| down.cr[j * cw + cw / 2]).collect();
    json!({
        // 0: co-sited with the left luma sample (chroma_sample_loc_type 0); 0.5: centred.
        "horizontal_luma_samples": q::siting(&row, |x| q::ramp_chroma(spec, true, x).1),
        // 0.5: vertically centred, as type 0 is.
        "vertical_luma_samples": q::siting(&column, |y| q::ramp_chroma(spec, false, y).1),
    })
}
