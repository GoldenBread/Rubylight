//! RTX Video enhancement through the NVIDIA component's NGX adapter: NVIDIA's
//! video super resolution (VSR) at the capture's own size, which sharpens and
//! deblocks the picture before encoding. SDR pictures go to the model as
//! they are; an FP16 HDR picture stays HDR, its SDR proxy is enhanced and
//! only the bounded change is added back (`shaders/rtx_video.hlsl`, after
//! Foundation Sunshine's DLSS NR HDR filter). Creation, enhancement and
//! teardown stay on one thread.
use crate::capture::{Device, GpuImage, GpuPool, Pixel};
use crate::nvidia_enhance::describe;
use anyhow::{Context, Result, bail};
use std::{ffi::c_void, ptr};
use windows::{
    Win32::Graphics::{
        Direct3D::{D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, Fxc::*},
        Direct3D11::*,
        Dxgi::Common::*,
    },
    core::{Interface, PCSTR},
};

use butterpollo_win_abi::shaders::RTX_VIDEO_HLSL as SHADER;
type Create = unsafe extern "C" fn(*mut c_void, *mut u32) -> *mut c_void;
type Convert = unsafe extern "C" fn(*mut c_void, *mut c_void, u32, *mut u32) -> *mut c_void;
type Destroy = unsafe extern "C" fn(*mut c_void);

/// Try to create a shared D3D11->D3D12 fence signal for `device` so enhanced
/// textures can stay `SHARED_NTHANDLE | SHARED` on the D3D12 compute queue.
fn d3d11_signal(device: &Device) -> Option<crate::compute::D3d11Signal> {
    crate::compute::Compute::for_device(&device.device)
        .and_then(|compute| crate::compute::D3d11Signal::new(compute, device))
        .ok()
}

pub struct Filter {
    state: *mut c_void,
    convert: Convert,
    destroy: Destroy,
    device: Device,
    lock: ID3D11Multithread,
    quality: u32,
    pool: GpuPool,
    signal: Option<crate::compute::D3d11Signal>,
    hdr: Option<HdrPasses>,
    _dll: libloading::Library,
}
impl Filter {
    /// An enhancer on the capture's device at VSR quality level `quality`
    /// (`butterpollo_media_policy::nvidia_enhance::rtx_video_quality`).
    pub fn new_gpu(image: &GpuImage, quality: u32) -> Result<Self> {
        let device = image.gpu.clone();
        if device.vendor != 0x10de {
            bail!("RTX Video enhancement needs an NVIDIA GPU");
        }
        let lock: ID3D11Multithread = device.context.cast()?;
        let dll = crate::nvidia_enhance::load()?;
        // SAFETY: The adapter's ABI 3 (checked by `load`) exports these symbols with these types,
        // and Filter keeps `dll` and `device` while the pointers are used.
        unsafe {
            let create = *dll.get::<Create>(b"butterpollo_vsr_create\0")?;
            let convert = *dll.get::<Convert>(b"butterpollo_vsr_convert\0")?;
            let destroy = *dll.get::<Destroy>(b"butterpollo_vsr_destroy\0")?;
            let mut error = 0;
            let state = create(device.device.as_raw(), &mut error);
            if state.is_null() {
                bail!("RTX Video enhancement could not start: {}", describe(error));
            }
            let signal = d3d11_signal(&device);
            let pool = if signal.is_some() {
                GpuPool::shared_rtv()
            } else {
                GpuPool::default()
            };
            Ok(Self {
                state,
                convert,
                destroy,
                device,
                lock,
                quality: quality.clamp(1, 4),
                pool,
                signal,
                hdr: None,
                _dll: dll,
            })
        }
    }
    pub fn set_quality(&mut self, quality: u32) {
        self.quality = quality.clamp(1, 4);
    }
    /// The VSR quality level this filter uses.
    pub fn quality(&self) -> u32 {
        self.quality
    }
    /// The enhanced picture, in the capture's format. SDR output is copied into
    /// a bounded shared pool; HDR output resolves directly into a pooled shared
    /// render-target texture when D3D11->D3D12 fence signalling is available.
    pub fn apply_gpu(&mut self, image: &GpuImage) -> Result<GpuImage> {
        if image.gpu.device.as_raw() != self.device.device.as_raw() {
            bail!("RTX Video enhancement needs a picture on the capture's GPU");
        }
        if !matches!(image.pixel, Pixel::Bgra8 | Pixel::RgbaF16) {
            bail!("RTX Video enhancement reads 8-bit SDR and FP16 HDR pictures only");
        }
        // Order D3D11 work after any external producer fence on the GPU without
        // stalling the stream thread on the CPU.
        image.order_d3d11()?;
        // SAFETY: `state` is the live NGX state on this device and `image` a live picture on it.
        // The adapter takes its own lock and then the device's multithread lock (ABI 3), so VSR
        // runs with neither held here; this filter's own passes, pool work and fence signal hold
        // the cached device multithread lock. NGX's output is only borrowed while it is read.
        unsafe { self.enhance(image) }
    }
    /// Copy `texture` into `self.pool`, falling back once to a non-shared pool
    /// if shared texture allocation fails on a healthy device.
    ///
    /// # Safety
    /// Caller holds `self.lock`.
    unsafe fn copy_pooled(&mut self, texture: &ID3D11Texture2D) -> Result<GpuImage> {
        let mut copy = self.pool.copy(&self.device, texture, None);
        if copy.is_err()
            && self.pool.shared
            && crate::device_loss::DeviceLost::d3d11(&self.device.device).is_none()
        {
            self.downgrade_pool();
            copy = self.pool.copy(&self.device, texture, None);
        }
        let mut enhanced = copy?.context("the enhanced pictures are all still being encoded")?;
        if self.pool.shared {
            if enhanced.ready.is_none() {
                // Downgrade and re-copy into a non-shared pool so even this transition
                // frame is non-shareable instead of reaching D3D12 with `ready: None`.
                self.downgrade_pool();
                enhanced = self
                    .pool
                    .copy(&self.device, texture, None)?
                    .context("the enhanced pictures are all still being encoded")?;
                // SAFETY: Caller holds `self.lock`.
                unsafe { self.device.context.Flush() };
            }
        } else {
            // SAFETY: Caller holds `self.lock`.
            unsafe { self.device.context.Flush() };
        }
        Ok(enhanced)
    }
    fn downgrade_pool(&mut self) {
        self.pool = GpuPool::default();
        self.signal = None;
        if let Some(passes) = self.hdr.as_mut() {
            passes.pooled_targets.clear();
        }
    }
    /// # Safety
    /// The caller holds no multithread lock of the device.
    unsafe fn enhance(&mut self, image: &GpuImage) -> Result<GpuImage> {
        // SAFETY: VSR runs outside `self.lock` (matching adapter lock order);
        // our passes, pool operations, and fence/flush run under `self.lock`.
        unsafe {
            if image.pixel == Pixel::Bgra8 {
                let texture = self.vsr(image.texture.as_raw())?;
                self.lock.Enter();
                if let Some(passes) = self.hdr.as_mut() {
                    // Format switched to SDR: release any cached FP16 pooled RTVs immediately.
                    passes.pooled_targets.clear();
                }
                let result = self.copy_pooled(&texture);
                self.lock.Leave();
                let mut enhanced = result?;
                enhanced.inherit(image);
                return Ok(enhanced);
            }
            self.lock.Enter();
            let prepared = (|| -> Result<(ID3D11ShaderResourceView, *mut c_void)> {
                if self.hdr.is_none() {
                    self.hdr = Some(HdrPasses::new(&self.device)?);
                }
                let passes = self.hdr.as_mut().unwrap();
                passes.size(&self.device, image.width, image.height)?;
                let copy = passes.original(&self.device, image)?;
                passes.prepare(&self.device, &copy, image.width, image.height);
                Ok((copy, passes.proxy.as_raw()))
            })();
            self.lock.Leave();
            let (original, proxy) = prepared?;
            let vsr_out = self.vsr(proxy)?;
            self.lock.Enter();
            let result = self.resolve_hdr(image, &original, &vsr_out);
            self.lock.Leave();
            let mut enhanced = result?;
            enhanced.inherit(image);
            Ok(enhanced)
        }
    }
    /// Resolve the HDR enhancement pass directly into a pooled shared RTV
    /// texture (eliminating an extra 4K FP16 copy) when D3D11->D3D12 fencing
    /// is available, or fall back to an internal target + pool copy.
    ///
    /// # Safety
    /// Caller holds `self.lock`.
    unsafe fn resolve_hdr(
        &mut self,
        image: &GpuImage,
        original: &ID3D11ShaderResourceView,
        vsr_out: &ID3D11Texture2D,
    ) -> Result<GpuImage> {
        // SAFETY: Caller holds `self.lock`; all resources live on `self.device`.
        unsafe {
            if self.signal.is_some() {
                match self.pool.acquire_texture(
                    &self.device,
                    image.width,
                    image.height,
                    DXGI_FORMAT_R16G16B16A16_FLOAT,
                ) {
                    Ok(Some(texture)) => {
                        let passes = self.hdr.as_mut().unwrap();
                        let rtv = passes.pooled_target(
                            &self.device,
                            texture.as_ref(),
                            image.width,
                            image.height,
                        )?;
                        passes.resolve_into(
                            &self.device,
                            original,
                            vsr_out,
                            &rtv,
                            image.width,
                            image.height,
                        )?;
                        match self.signal.as_mut().unwrap().signal() {
                            Ok(ready) => {
                                return Ok(GpuImage::from_texture(
                                    self.device.clone(),
                                    texture,
                                    image.width,
                                    image.height,
                                    Pixel::RgbaF16,
                                    Some(ready),
                                ));
                            }
                            Err(error) => {
                                tracing::debug!(
                                    %error,
                                    "RTX Video shared D3D11 fence signal failed; copying into non-shared pool"
                                );
                                self.downgrade_pool();
                                // Copy the already-resolved pixels into the fresh non-shared pool
                                // so this transition frame is non-shareable too.
                                return self.copy_pooled(texture.as_ref());
                            }
                        }
                    }
                    Ok(None) => bail!("the enhanced pictures are all still being encoded"),
                    Err(error)
                        if crate::device_loss::DeviceLost::d3d11(&self.device.device).is_none() =>
                    {
                        tracing::debug!(
                            %error,
                            "RTX Video shared RTV pool unavailable; falling back to non-shared pool"
                        );
                        self.downgrade_pool();
                    }
                    Err(error) => return Err(error),
                }
            }
            let passes = self.hdr.as_mut().unwrap();
            let (output, rtv) = passes.fallback_target(&self.device, image.width, image.height)?;
            passes.resolve_into(
                &self.device,
                original,
                vsr_out,
                &rtv,
                image.width,
                image.height,
            )?;
            self.copy_pooled(&output)
        }
    }
    /// NGX's VSR over `input` at its own size.
    unsafe fn vsr(&mut self, input: *mut c_void) -> Result<ID3D11Texture2D> {
        // SAFETY: `state` is the live NGX state on this device and `input` a live texture on it;
        // the caller holds no multithread lock, which the adapter takes after its own. NGX's
        // output is only borrowed while it is read.
        unsafe {
            let mut error = 0;
            let output = (self.convert)(self.state, input, self.quality, &mut error);
            let texture = ID3D11Texture2D::from_raw_borrowed(&output)
                .with_context(|| format!("RTX Video enhancement failed: {}", describe(error)))?;
            Ok(texture.clone())
        }
    }
}
impl Drop for Filter {
    fn drop(&mut self) {
        // Release any cached SRV/RTV referencing NGX's output texture before
        // tearing down the NGX state.
        self.hdr = None;
        // SAFETY: `state` came from create and is destroyed once, here; `_dll` is dropped after
        // this.
        unsafe {
            (self.destroy)(self.state);
        }
        self.state = ptr::null_mut();
    }
}

/// A shader resource view of `texture`.
unsafe fn view(device: &Device, texture: &ID3D11Texture2D) -> Result<ID3D11ShaderResourceView> {
    let mut view = None;
    // SAFETY: `texture` is a live texture on `device`.
    unsafe {
        device
            .device
            .CreateShaderResourceView(texture, None, Some(&mut view))
            .context("the picture cannot be read by a shader")?;
    }
    view.context("D3D11 returned no shader view")
}

/// The HDR picture's two passes around the model, and their textures.
struct HdrPasses {
    vertex: ID3D11VertexShader,
    prepare: ID3D11PixelShader,
    resolve: ID3D11PixelShader,
    proxy: ID3D11Texture2D,
    proxy_view: ID3D11ShaderResourceView,
    proxy_target: ID3D11RenderTargetView,
    /// Lazy fallback FP16 render target used only when shared RTV pooling is
    /// unavailable.
    fallback_output: Option<(ID3D11Texture2D, ID3D11RenderTargetView)>,
    shape: (u32, u32),
    /// Views of the pictures this pass prepared, kept while the capture pool hands the same
    /// textures again: making one every frame cost the render thread a call and a lock turn.
    originals: Vec<Original>,
    /// Cached shader resource view for NGX's VSR output texture.
    enhanced_view: Option<(usize, u32, u32, ID3D11ShaderResourceView)>,
    /// Cached render target views for pooled FP16 textures rendered into directly.
    pooled_targets: Vec<(usize, u32, u32, ID3D11RenderTargetView)>,
}
/// A picture's view, with the texture, size and format it was made for.
struct Original {
    texture: usize,
    width: u32,
    height: u32,
    pixel: Pixel,
    view: ID3D11ShaderResourceView,
}
impl HdrPasses {
    fn new(device: &Device) -> Result<Self> {
        // SAFETY: `device` is a live D3D11 device; each bytecode blob outlives its call.
        unsafe {
            let mut vertex = None;
            let mut prepare = None;
            let mut resolve = None;
            device.device.CreateVertexShader(
                &compile(b"vertex\0", b"vs_5_0\0")?,
                None,
                Some(&mut vertex),
            )?;
            device.device.CreatePixelShader(
                &compile(b"prepare\0", b"ps_5_0\0")?,
                None,
                Some(&mut prepare),
            )?;
            device.device.CreatePixelShader(
                &compile(b"resolve\0", b"ps_5_0\0")?,
                None,
                Some(&mut resolve),
            )?;
            let (proxy, proxy_view, proxy_target) =
                target(device, 1, 1, DXGI_FORMAT_B8G8R8A8_UNORM)?;
            Ok(Self {
                vertex: vertex.context("no vertex shader")?,
                prepare: prepare.context("no prepare shader")?,
                resolve: resolve.context("no resolve shader")?,
                proxy,
                proxy_view,
                proxy_target,
                fallback_output: None,
                shape: (1, 1),
                originals: Vec::new(),
                enhanced_view: None,
                pooled_targets: Vec::new(),
            })
        }
    }
    /// A shader view of `image`'s texture, kept while the capture pool hands the same picture.
    ///
    /// # Safety
    /// The caller holds the device's multithread lock and `image` is a live picture on it.
    unsafe fn original(
        &mut self,
        device: &Device,
        image: &GpuImage,
    ) -> Result<ID3D11ShaderResourceView> {
        let key = (
            image.texture.as_raw() as usize,
            image.width,
            image.height,
            image.pixel,
        );
        if let Some(found) = self.originals.iter().find(|original| {
            (
                original.texture,
                original.width,
                original.height,
                original.pixel,
            ) == key
        }) {
            return Ok(found.view.clone());
        }
        // SAFETY: `image`'s texture is live on `device`.
        let created = unsafe { view(device, image.texture.as_ref()) }?;
        // The capture pool rotates up to 8 textures; clear if shapes rotated.
        if self.originals.len() >= 8 {
            self.originals.clear();
        }
        self.originals.push(Original {
            texture: key.0,
            width: key.1,
            height: key.2,
            pixel: key.3,
            view: created.clone(),
        });
        Ok(created)
    }
    /// Cached shader resource view for NGX's VSR output texture.
    ///
    /// # Safety
    /// Caller holds the device's multithread lock and `enhanced` is live on `device`.
    unsafe fn enhanced_view(
        &mut self,
        device: &Device,
        enhanced: &ID3D11Texture2D,
        width: u32,
        height: u32,
    ) -> Result<ID3D11ShaderResourceView> {
        let raw = enhanced.as_raw() as usize;
        if let Some((cached_raw, w, h, cached_view)) = &self.enhanced_view
            && (*cached_raw, *w, *h) == (raw, width, height)
        {
            return Ok(cached_view.clone());
        }
        // SAFETY: `enhanced` is a live texture on `device`.
        let created = unsafe { view(device, enhanced) }?;
        self.enhanced_view = Some((raw, width, height, created.clone()));
        Ok(created)
    }
    /// Cached render target view for a pooled FP16 texture.
    ///
    /// # Safety
    /// Caller holds the device's multithread lock and `texture` is live on `device`.
    unsafe fn pooled_target(
        &mut self,
        device: &Device,
        texture: &ID3D11Texture2D,
        width: u32,
        height: u32,
    ) -> Result<ID3D11RenderTargetView> {
        let raw = texture.as_raw() as usize;
        if let Some((_, _, _, rtv)) = self
            .pooled_targets
            .iter()
            .find(|(k, w, h, _)| (*k, *w, *h) == (raw, width, height))
        {
            return Ok(rtv.clone());
        }
        let mut render = None;
        // SAFETY: `texture` is a live RTV-bound texture on `device`.
        unsafe {
            device
                .device
                .CreateRenderTargetView(texture, None, Some(&mut render))?;
        }
        let rtv = render.context("D3D11 returned no pooled render view")?;
        if self.pooled_targets.len() >= 8 {
            self.pooled_targets.clear();
        }
        self.pooled_targets.push((raw, width, height, rtv.clone()));
        Ok(rtv)
    }
    fn fallback_target(
        &mut self,
        device: &Device,
        width: u32,
        height: u32,
    ) -> Result<(ID3D11Texture2D, ID3D11RenderTargetView)> {
        if let Some((texture, rtv)) = &self.fallback_output {
            return Ok((texture.clone(), rtv.clone()));
        }
        let (texture, _, rtv) = target(device, width, height, DXGI_FORMAT_R16G16B16A16_FLOAT)?;
        self.fallback_output = Some((texture.clone(), rtv.clone()));
        Ok((texture, rtv))
    }
    /// Makes the proxy texture `width` by `height` and drops stale views.
    fn size(&mut self, device: &Device, width: u32, height: u32) -> Result<()> {
        if self.shape == (width, height) {
            return Ok(());
        }
        self.originals.clear();
        self.enhanced_view = None;
        self.pooled_targets.clear();
        self.fallback_output = None;
        (self.proxy, self.proxy_view, self.proxy_target) =
            target(device, width, height, DXGI_FORMAT_B8G8R8A8_UNORM)?;
        self.shape = (width, height);
        Ok(())
    }
    /// # Safety
    /// The caller holds the device's multithread lock.
    unsafe fn draw(
        &self,
        device: &Device,
        shader: &ID3D11PixelShader,
        views: &[Option<ID3D11ShaderResourceView>],
        target: &ID3D11RenderTargetView,
        width: u32,
        height: u32,
    ) {
        let context = &device.context;
        // SAFETY: every bound object is live on this device; the bindings are cleared after the
        // draw, and the other passes on this context set their own state.
        unsafe {
            context.IASetInputLayout(None);
            context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.VSSetShader(&self.vertex, None);
            context.RSSetState(None);
            context.OMSetBlendState(None, None, u32::MAX);
            context.RSSetViewports(Some(&[D3D11_VIEWPORT {
                Width: width as f32,
                Height: height as f32,
                MinDepth: 0.,
                MaxDepth: 1.,
                ..Default::default()
            }]));
            context.PSSetShaderResources(0, Some(views));
            context.OMSetRenderTargets(Some(&[Some(target.clone())]), None);
            context.PSSetShader(shader, None);
            context.Draw(3, 0);
            context.PSSetShaderResources(0, Some(&[None, None, None]));
            context.OMSetRenderTargets(None, None);
        }
    }
    unsafe fn prepare(
        &self,
        device: &Device,
        original: &ID3D11ShaderResourceView,
        width: u32,
        height: u32,
    ) {
        // SAFETY: the caller holds the device's multithread lock, as `draw` requires; the view
        // and target are live on `device`.
        unsafe {
            self.draw(
                device,
                &self.prepare,
                &[Some(original.clone())],
                &self.proxy_target,
                width,
                height,
            );
        }
    }
    unsafe fn resolve_into(
        &mut self,
        device: &Device,
        original: &ID3D11ShaderResourceView,
        enhanced: &ID3D11Texture2D,
        output_target: &ID3D11RenderTargetView,
        width: u32,
        height: u32,
    ) -> Result<()> {
        // SAFETY: the caller holds the device's multithread lock, as `draw` requires; both
        // textures and `output_target` are live on `device`.
        unsafe {
            let enhanced = self.enhanced_view(device, enhanced, width, height)?;
            self.draw(
                device,
                &self.resolve,
                &[
                    Some(original.clone()),
                    Some(self.proxy_view.clone()),
                    Some(enhanced),
                ],
                output_target,
                width,
                height,
            );
            Ok(())
        }
    }
}

/// A render target texture with its shader and target views.
fn target(
    device: &Device,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
) -> Result<(
    ID3D11Texture2D,
    ID3D11ShaderResourceView,
    ID3D11RenderTargetView,
)> {
    // SAFETY: `device` is a live D3D11 device and the descriptor outlives the call.
    unsafe {
        let mut texture = None;
        device.device.CreateTexture2D(
            &D3D11_TEXTURE2D_DESC {
                Width: width,
                Height: height,
                MipLevels: 1,
                ArraySize: 1,
                Format: format,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
                ..Default::default()
            },
            None,
            Some(&mut texture),
        )?;
        let texture = texture.context("D3D11 returned no texture")?;
        let shader = view(device, &texture)?;
        let mut render = None;
        device
            .device
            .CreateRenderTargetView(&texture, None, Some(&mut render))?;
        Ok((
            texture,
            shader,
            render.context("D3D11 returned no render view")?,
        ))
    }
}

/// `entry` of `shaders/rtx_video.hlsl` for `target`, compiled when the
/// enhancement first meets an HDR picture.
///
/// # Safety
///
/// `entry` and `target` must be NUL-terminated.
unsafe fn compile(entry: &'static [u8], target: &'static [u8]) -> Result<Vec<u8>> {
    // SAFETY: `SHADER` is passed with its exact length, `entry` and `target` are NUL-terminated,
    // and each blob is read only within GetBufferSize bytes while it is alive.
    unsafe {
        let mut code = None;
        let mut errors = None;
        let result = D3DCompile(
            SHADER.as_ptr().cast(),
            SHADER.len(),
            PCSTR::null(),
            None,
            None,
            PCSTR(entry.as_ptr()),
            PCSTR(target.as_ptr()),
            D3DCOMPILE_OPTIMIZATION_LEVEL3,
            0,
            &mut code,
            Some(&mut errors),
        );
        if let Err(error) = result {
            let detail = errors
                .map(|blob| {
                    String::from_utf8_lossy(std::slice::from_raw_parts(
                        blob.GetBufferPointer().cast(),
                        blob.GetBufferSize(),
                    ))
                    .into_owned()
                })
                .unwrap_or_default();
            bail!("RTX Video shader compilation: {error}: {detail}");
        }
        let code = code.context("shader compiler returned no bytecode")?;
        Ok(
            std::slice::from_raw_parts(code.GetBufferPointer().cast::<u8>(), code.GetBufferSize())
                .to_vec(),
        )
    }
}

#[cfg(test)]
mod tests {
    /// The HLSL compiles for every entry the passes use, without a GPU.
    #[test]
    fn hdr_passes_compile() {
        let entries: [(&'static [u8], &'static [u8]); 3] = [
            (b"vertex\0", b"vs_5_0\0"),
            (b"prepare\0", b"ps_5_0\0"),
            (b"resolve\0", b"ps_5_0\0"),
        ];
        for (entry, target) in entries {
            // SAFETY: both names are NUL-terminated.
            let code = unsafe { super::compile(entry, target) };
            assert!(!code.unwrap().is_empty());
        }
    }
}
