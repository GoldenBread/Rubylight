//! Per-frame PyroWave conversion slots (shared plane images, single timeline
//! sync object, optional D3D12 compute converter and lazy D3D11 fallback) plus
//! creation-time warmup helpers.

use super::*;

/// One shared D3D11 plane texture imported into Vulkan for PyroWave to read.
pub(super) struct PlaneImage {
    api: Arc<Api>,
    pub(super) image: p::pyrowave_image,
    pub(super) view: p::pyrowave_image_view,
    _texture: Arc<ID3D11Texture2D>,
}

impl PlaneImage {
    fn new(api: Arc<Api>, pyro: p::pyrowave_device, texture: Arc<ID3D11Texture2D>) -> Result<Self> {
        // SAFETY: `texture` is a live shared D3D11 texture on the same adapter as `pyro`, the
        // create-info outlives the call, and `s` owns `image` so Drop frees it on error.
        unsafe {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);
            let mut s = Self {
                api,
                image: ptr::null_mut(),
                view: std::mem::zeroed(),
                _texture: texture,
            };
            let handle = s._texture.cast::<IDXGIResource1>()?.CreateSharedHandle(
                None,
                DXGI_SHARED_RESOURCE_READ.0 | DXGI_SHARED_RESOURCE_WRITE.0,
                PCWSTR::null(),
            )?;
            let image = p::VkImageCreateInfo {
                sType: p::VkStructureType_VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO,
                imageType: p::VkImageType_VK_IMAGE_TYPE_2D,
                format: if desc.Format == DXGI_FORMAT_R16_UNORM {
                    p::VkFormat_VK_FORMAT_R16_UNORM
                } else {
                    p::VkFormat_VK_FORMAT_R8_UNORM
                },
                extent: p::VkExtent3D {
                    width: desc.Width,
                    height: desc.Height,
                    depth: 1,
                },
                mipLevels: 1,
                arrayLayers: 1,
                samples: p::VkSampleCountFlagBits_VK_SAMPLE_COUNT_1_BIT,
                tiling: p::VkImageTiling_VK_IMAGE_TILING_OPTIMAL,
                usage: p::VkImageUsageFlagBits_VK_IMAGE_USAGE_SAMPLED_BIT
                    | p::VkImageUsageFlagBits_VK_IMAGE_USAGE_TRANSFER_SRC_BIT
                    | p::VkImageUsageFlagBits_VK_IMAGE_USAGE_TRANSFER_DST_BIT,
                sharingMode: p::VkSharingMode_VK_SHARING_MODE_EXCLUSIVE,
                ..std::mem::zeroed()
            };
            let info = p::pyrowave_image_create_info {
                device: pyro,
                external_handle: handle.0 as usize,
                handle_type: p::VkExternalMemoryHandleTypeFlagBits_VK_EXTERNAL_MEMORY_HANDLE_TYPE_D3D11_TEXTURE_BIT,
                image_create_info: &image,
            };
            let result = (s.api.image_create)(&info, &mut s.image);
            if result != 0 {
                let _ = CloseHandle(handle);
                check(result)?;
            }
            check((s.api.image_get_image_view)(
                s.image,
                p::VkImageAspectFlagBits_VK_IMAGE_ASPECT_COLOR_BIT,
                p::VkImageUsageFlagBits_VK_IMAGE_USAGE_SAMPLED_BIT,
                &mut s.view,
            ))?;
            Ok(s)
        }
    }
}

impl Drop for PlaneImage {
    fn drop(&mut self) {
        // SAFETY: `image` was created on the Encoder's device, which is destroyed after `slots`.
        unsafe {
            if !self.image.is_null() {
                (self.api.image_destroy)(self.image);
            }
        }
    }
}

/// One slot's shared timeline fence between colour conversion (D3D12 or D3D11)
/// and PyroWave's Vulkan encode queue.
pub(super) struct SlotSync {
    pub(super) api: Arc<Api>,
    pub(super) sync: p::pyrowave_sync_object,
    pub(super) fence: ID3D11Fence,
    pub(super) context: ID3D11DeviceContext4,
    pub(super) counter: u64,
}

impl SlotSync {
    fn new(api: Arc<Api>, device: &Device, pyro: p::pyrowave_device) -> Result<Self> {
        // SAFETY: `device` is a live D3D11 device on the same adapter as `pyro`, the create-info
        // outlives the call, and `s` owns `sync` so Drop frees it on error.
        unsafe {
            let mut fence = None;
            device.device.cast::<ID3D11Device5>()?.CreateFence(
                0,
                D3D11_FENCE_FLAG_SHARED,
                &mut fence,
            )?;
            let mut s = Self {
                api,
                sync: ptr::null_mut(),
                fence: fence.context("no shared D3D11 fence")?,
                context: device.context.cast()?,
                counter: 0,
            };
            let handle = s
                .fence
                .CreateSharedHandle(None, GENERIC_ALL.0, PCWSTR::null())?;
            let info = p::pyrowave_sync_object_create_info {
                device: pyro,
                external_handle: handle.0 as usize,
                handle_type: p::VkExternalSemaphoreHandleTypeFlagBits_VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_D3D12_FENCE_BIT,
                semaphore_type: p::VkSemaphoreType_VK_SEMAPHORE_TYPE_TIMELINE,
                import_flags: 0,
            };
            let result = (s.api.sync_object_create)(&info, &mut s.sync);
            if result != 0 {
                let _ = CloseHandle(handle);
                check(result)?;
            }
            Ok(s)
        }
    }
}

impl Drop for SlotSync {
    fn drop(&mut self) {
        // SAFETY: `sync` was created on the Encoder's device, which is destroyed after `slots`.
        unsafe {
            if !self.sync.is_null() {
                (self.api.sync_object_destroy)(self.sync);
            }
        }
    }
}

/// The colour conversion on the GPU's compute queue. Beside a game using the
/// whole GPU, the D3D11 conversion of a 1080p HDR 4:4:4 frame waited 5.1 ms
/// on the graphics queue, against 0.15 ms idle, nearly all of the encode's
/// 5.7 ms; PyroWave's own GPU work took 0.2 ms either way.
pub(super) struct ComputePlanes {
    pub(super) converter: crate::compute::Converter,
    target: crate::compute::Planes,
    textures: [Arc<ID3D11Texture2D>; 3],
}

impl ComputePlanes {
    fn new(
        d3d: &Device,
        config: &Negotiated,
        textures: &[Arc<ID3D11Texture2D>; 3],
        fence: &ID3D11Fence,
    ) -> Result<Self> {
        let compute = crate::compute::Compute::for_device(&d3d.device)?;
        let converter = crate::compute::Converter::new_planar(
            compute.clone(),
            config.width,
            config.height,
            config.ten_bit(),
        )?;
        let planes = [
            compute.open(&textures[0])?,
            compute.open(&textures[1])?,
            compute.open(&textures[2])?,
        ];
        // SAFETY: `fence` is a live shared D3D11 fence and `compute.device` is a D3D12 device on
        // the same adapter; `opened` is a live local and the handle is closed once opened.
        let fence = unsafe {
            let handle = fence.CreateSharedHandle(None, GENERIC_ALL.0, PCWSTR::null())?;
            let mut opened: Option<windows::Win32::Graphics::Direct3D12::ID3D12Fence> = None;
            let result = compute.device.OpenSharedHandle(handle, &mut opened);
            let _ = CloseHandle(handle);
            result.context("opening the PyroWave fence on the compute queue")?;
            opened.context("no shared fence")?
        };
        Ok(Self {
            converter,
            textures: textures.clone(),
            target: crate::compute::Planes {
                planes,
                format: if config.ten_bit() {
                    DXGI_FORMAT_R16_UNORM
                } else {
                    DXGI_FORMAT_R8_UNORM
                },
                fence,
                after: 0,
                done: 0,
            },
        })
    }

    /// Convert once the encoder has signalled `after`; signals `done`.
    pub(super) fn convert(
        &mut self,
        image: &GpuImage,
        config: &Negotiated,
        luminance: [f32; 2],
        after: u64,
        done: u64,
    ) -> Result<()> {
        let texture = self.converter.compute().open(&image.texture)?;
        let pointer = image
            .cursor
            .as_ref()
            .map(|cursor| self.converter.pointer(cursor))
            .transpose()?;
        self.converter.values = crate::gpu_color::constants(
            config,
            (image.width, image.height, image.pixel),
            luminance,
            image.cursor.as_ref(),
        );
        self.converter.values[11] = u32::from(config.yuv444);
        self.target.after = after;
        self.target.done = done;
        self.converter.convert_planes(
            &texture,
            crate::compute::format(image.pixel),
            pointer.as_ref(),
            image.ready.as_ref(),
            &self.target,
            Some(image),
        )
    }
}

impl Drop for ComputePlanes {
    fn drop(&mut self) {
        for texture in &self.textures {
            self.converter.compute().forget(texture);
        }
    }
}

/// Minimum bitstream budget for the creation-time warmup encode so Vulkan
/// compiles every encode pipeline and allocates the frame-context readback
/// buffers ahead of frame 1.
pub(super) const WARMUP_MIN_BUDGET: usize = 4096;

/// Bitstream budget for the creation-time warmup encode: sized for one stream
/// frame at the configured bitrate (at least [`WARMUP_MIN_BUDGET`]) so the
/// SDK's frame-context readback buffers do not reallocate on frame 1.
pub(super) fn warmup_budget(
    config: &Negotiated,
    critical_fec: bool,
    critical_fec_percentage: u32,
) -> usize {
    if config.packet_size <= 16 {
        return WARMUP_MIN_BUDGET;
    }
    let period = butterpollo_media_policy::framegen::Rate(config.fps_millihz()).period();
    let budget = pyrowave::budget_with_minimum(
        config.bitrate_kbps,
        period,
        config.packet_size,
        config.pyrowave_records,
        critical_fec_percentage,
        config.min_fec,
    );
    let limit = pyrowave::max_frame_bytes(config.packet_size, critical_fec).max(WARMUP_MIN_BUDGET);
    budget.clamp(WARMUP_MIN_BUDGET, limit) & !3
}

/// Whether a frame's source `(width, height, pixel)` requires discarding and
/// rebuilding the encoder's slots. An encoder created without a sample image
/// (`previous == None`) reuses its pre-created slots on the first frame.
pub(super) fn source_rebuilds_slots(
    previous: Option<(u32, u32, Pixel)>,
    next: (u32, u32, Pixel),
) -> bool {
    previous.is_some_and(|previous| previous != next)
}

/// Records the outcome of the creation-time warmup wait: an unconfirmed wait
/// (timeout or Vulkan device loss) marks the encoder unsettled so `Drop`
/// quarantines and retains its resources; a D3D device loss after confirmation
/// does not quarantine.
pub(super) fn finish_warmup(
    waited: Result<bool>,
    device_removed: Option<crate::device_loss::DeviceLost>,
    unconfirmed: &mut bool,
    lost: &mut Option<crate::device_loss::DeviceLost>,
) -> Result<()> {
    match waited {
        Ok(true) => {}
        Ok(false) => {
            *unconfirmed = true;
            bail!("PyroWave warmup completion was not confirmed");
        }
        Err(error) => {
            *unconfirmed = true;
            if let Some(loss) = crate::device_loss::DeviceLost::from_error(&error) {
                *lost = Some(loss);
            }
            return Err(error);
        }
    }
    if let Some(loss) = device_removed {
        *lost = Some(loss);
        return Err(loss.into());
    }
    Ok(())
}

/// One frame's conversion target: its colour planes, their Vulkan images and
/// the fence between the conversion and the encode. Each frame in flight has
/// its own; fields drop in this order.
pub(super) struct Slot {
    pub(super) compute: Option<ComputePlanes>,
    /// The three planes as Vulkan images.
    pub(super) planes: [PlaneImage; 3],
    /// The shared fence and Vulkan timeline semaphore ordering this slot.
    pub(super) sync: SlotSync,
    /// D3D11 fallback converter; only created when `compute` is unavailable or
    /// a non-shareable capture texture arrives.
    pub(super) converter: Option<PlanarConverter>,
    /// Shared D3D11 plane textures, kept so a fallback [`PlanarConverter`] can
    /// attach to the exact planes already imported into Vulkan.
    pub(super) textures: [Arc<ID3D11Texture2D>; 3],
}

impl Slot {
    pub(super) fn new(
        api: &Arc<Api>,
        d3d: &Device,
        device: p::pyrowave_device,
        config: &Negotiated,
        source: Option<(u32, u32, Pixel)>,
        use_compute: bool,
        warnings: &Arc<dyn WarningSink>,
    ) -> Result<Self> {
        let textures = crate::gpu_color::planar_textures(d3d, config)?;
        let mut planes = Vec::with_capacity(3);
        for texture in &textures {
            planes.push(PlaneImage::new(api.clone(), device, texture.clone())?);
        }
        let planes: [PlaneImage; 3] = planes.try_into().ok().unwrap();
        let sync = SlotSync::new(api.clone(), d3d, device)?;
        let mut compute = None;
        if use_compute {
            match ComputePlanes::new(d3d, config, &textures, &sync.fence) {
                Ok(planes) => compute = Some(planes),
                Err(error) => {
                    warnings.set("pyrowave_conversion", &format!("PyroWave compute conversion unavailable ({error:#}); converting on the graphics queue. Game rendering may delay frames; update the AMD driver or lower GPU load."))
                }
            }
        }
        let converter = match (&compute, source) {
            (None, Some(source)) => Some(PlanarConverter::with_textures(
                d3d,
                config,
                source,
                textures.clone(),
            )?),
            _ => None,
        };
        Ok(Self {
            compute,
            planes,
            sync,
            converter,
            textures,
        })
    }

    pub(super) fn semaphore(&self) -> p::VkSemaphore {
        // SAFETY: `sync` is the live sync object `SlotSync::new` imported on this device.
        unsafe { (self.sync.api.sync_object_get_semaphore)(self.sync.sync) }
    }
}
