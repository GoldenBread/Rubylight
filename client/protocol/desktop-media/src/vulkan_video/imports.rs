//! Per-surface DMA-BUF Vulkan imports for `VaapiMap`, kept while the decoder's VA-API pool lasts.
//! Each surface's layers (two planes for 4:2:0 NV12/P010, or one interleaved plane for 4:4:4
//! VUYA/VUYX/XV30) are imported once on first use and reused across subsequent frames of that
//! surface without re-exporting DMA-BUFs or recreating Vulkan images.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use super::*;
use crate::dmabuf::{self, Descriptor, FramesRef, SurfaceCache, CACHE_LIMIT};

/// Monotonic pool generation assigned to each VA-API surface pool in `SurfaceImports`.
static NEXT_POOL_ID: AtomicU64 = AtomicU64::new(1);

unsafe extern "C" {
    /// libavutil/hwcontext_vulkan.h: an AVVkFrame with its internal part (the lock FFmpeg's
    /// default `lock_frame` takes), freed with `av_free`.
    fn av_vk_frame_alloc() -> *mut VkFrame;
    fn av_free(pointer: *mut c_void);
    fn av_buffer_ref(reference: *const AVBufferRef) -> *mut AVBufferRef;
    /// libavutil/buffer.h (the size a `size_t` since FFmpeg 5); not in the bindings.
    fn av_buffer_create(
        data: *mut u8,
        size: usize,
        free: Option<unsafe extern "C" fn(*mut c_void, *mut u8)>,
        opaque: *mut c_void,
        flags: c_int,
    ) -> *mut AVBufferRef;
}

/// The renderer's device as the imports use it: its own functions, as `Planes` loads them.
struct ImportDevice {
    instance: ash::Instance,
    physical_device: vk::PhysicalDevice,
    device: ash::Device,
    memory_fd: ash::khr::external_memory_fd::Device,
    /// The property flags of each of the device's memory types.
    memory_types: Vec<vk::MemoryPropertyFlags>,
}

impl ImportDevice {
    /// # Safety
    /// `gpu` is the renderer's live SDL GPU device, made with `MAP_EXTENSIONS`
    /// (`map_device_context` checked them).
    unsafe fn of(gpu: *mut SDL_GPUDevice) -> Result<Self, String> {
        // SAFETY: Caller guarantees `gpu` is a live SDL GPU device with Vulkan sharing properties.
        unsafe {
            let sdl = SdlVulkan::of_device(gpu)?;
            let instance = ash::Instance::load(
                &ash::StaticFn {
                    get_instance_proc_addr: zero_copy::get_instance_proc_addr()?,
                },
                sdl.instance,
            );
            let device = ash::Device::load(instance.fp_v1_0(), sdl.device);
            let memory_fd = ash::khr::external_memory_fd::Device::new(&instance, &device);
            let memory = instance.get_physical_device_memory_properties(sdl.physical_device);
            let memory_types = memory.memory_types[..memory.memory_type_count as usize]
                .iter()
                .map(|kind| kind.property_flags)
                .collect();
            Ok(Self { instance, physical_device: sdl.physical_device, device, memory_fd, memory_types })
        }
    }

    /// Whether the driver imports a DMA-BUF of `modifier` as a `format` image with `usage` (the
    /// copy reads it as TRANSFER_SRC; sampling needs SAMPLED), as hwcontext_vulkan asks before
    /// its own import; creating one it does not would be undefined.
    unsafe fn check_importable(&self, format: vk::Format, modifier: u64, usage: vk::ImageUsageFlags) -> Result<(), String> {
        let mut drm = vk::PhysicalDeviceImageDrmFormatModifierInfoEXT::default()
            .drm_format_modifier(modifier)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let mut external = vk::PhysicalDeviceExternalImageFormatInfo::default()
            .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        let info = vk::PhysicalDeviceImageFormatInfo2::default()
            .format(format)
            .ty(vk::ImageType::TYPE_2D)
            .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
            .usage(usage)
            .push_next(&mut external)
            .push_next(&mut drm);
        let mut external_properties = vk::ExternalImageFormatProperties::default();
        let mut properties = vk::ImageFormatProperties2::default().push_next(&mut external_properties);
        // SAFETY: `self.instance` and `self.physical_device` are valid Vulkan handles.
        unsafe {
            self.instance
                .get_physical_device_image_format_properties2(self.physical_device, &info, &mut properties)
        }
        .map_err(|e| format!("the driver does not import Vulkan format {} DMA-BUFs of modifier {modifier:#x} ({e})", format.as_raw()))?;
        if !external_properties
            .external_memory_properties
            .external_memory_features
            .contains(vk::ExternalMemoryFeatureFlags::IMPORTABLE)
        {
            return Err(format!("the driver cannot import Vulkan format {} DMA-BUFs of modifier {modifier:#x}", format.as_raw()));
        }
        Ok(())
    }

    /// Waits up to `COPY_WAIT` for each of `frame`'s semaphores to reach its value, the last
    /// copy's signal: then the GPU no longer reads its images.
    unsafe fn wait(&self, frame: &FrameNow) -> bool {
        let mut semaphores = [vk::Semaphore::null(); 4];
        let mut values = [0u64; 4];
        let mut count = 0usize;
        for index in 0..frame.sem.len().min(4) {
            if frame.sem[index] != vk::Semaphore::null() {
                semaphores[count] = frame.sem[index];
                values[count] = frame.sem_value[index];
                count += 1;
            }
        }
        // SAFETY: Caller guarantees `frame` holds valid timeline semaphores for `self.device`.
        count == 0
            || unsafe {
                self.device.wait_semaphores(
                    &vk::SemaphoreWaitInfo::default()
                        .semaphores(&semaphores[..count])
                        .values(&values[..count]),
                    COPY_WAIT.as_nanos() as u64,
                )
            }
            .is_ok()
    }
}

/// One surface's import: an image per layer, each with its imported memory (a `dup` of the
/// layer's DMA-BUF) and a timeline semaphore, in an AVVkFrame FFmpeg's frame lock works on.
/// Mapped frames of the surface share it (`Lent`); it goes when the cache and the last of them
/// let go, which is when no copy reads it (`Drop`).
struct CachedImport {
    vulkan: Arc<ImportDevice>,
    frame: *mut VkFrame,
    /// The images were made to be sampled too (`SAMPLED`), as the driver takes the layers'
    /// formats and modifiers with that usage; else they are only copied from.
    sampled: bool,
    /// The picture's size, for the layers' extents.
    size: (u32, u32),
    /// Generation of the VA-API surface pool this import belongs to, so `Planes` evicts stale
    /// wrapped textures across pool reopens at the same resolution.
    pool_id: u64,
    /// Active `Lent` mapped frames currently holding this `AVVkFrame` across threads.
    /// `SurfaceImports::map` reuses a cached import only when `lent == 0` (avoiding a data race
    /// if FFmpeg emits a duplicate frame of the same `VASurfaceID`, e.g. AV1 `show_existing_frame`).
    lent: AtomicUsize,
}

// SAFETY: The frame is used by one picture at a time: a surface comes back from the decoder,
// and its import is handed out again, only once every frame mapped from it was freed (each one
// holds the surface, `Lent`); FFmpeg's reference counts order those uses across threads.
unsafe impl Send for CachedImport {}
unsafe impl Sync for CachedImport {}

impl CachedImport {
    /// Imports `descriptor`, an exported two-plane 4:2:0 (`import_layers`) or single-layer 4:4:4
    /// (`import_interleaved_layer`) surface of a `width` x `height` picture, as hwcontext_vulkan's
    /// `vulkan_map_from_drm_frame_desc` does: one image per layer, in the object's DRM format
    /// modifier with the layer's offset and pitch, its memory imported from a `dup` of the
    /// object's descriptor (once per image, also where both layers share one object), dedicated
    /// where the driver prefers it, bound at 0 (the offset is in the layout).
    ///
    /// # Safety
    /// `descriptor`'s descriptors are open.
    #[allow(clippy::too_many_arguments)]
    unsafe fn new(
        vulkan: &Arc<ImportDevice>,
        descriptor: &Descriptor,
        width: u32,
        height: u32,
        ten_bit: bool,
        interleaved: bool,
        sample: bool,
        pool_id: u64,
    ) -> Result<Self, String> {
        // SAFETY: Caller guarantees `descriptor`'s file descriptors are open and `vulkan` is live.
        unsafe {
            let (formats, layer_count) = if interleaved {
                ([import_interleaved_layer(descriptor, ten_bit)?, vk::Format::UNDEFINED], 1usize)
            } else {
                (import_layers(descriptor, ten_bit)?, 2usize)
            };
            // Sampled in place only for 4:2:0 when the driver takes every layer with that usage:
            // a surface it does not (and all interleaved 4:4:4 surfaces) is copied from.
            let sampled = !interleaved
                && sample
                && formats[..layer_count].iter().enumerate().all(|(index, &format)| {
                    let (_, modifier, _) = layer_layout(descriptor, index);
                    modifier != DRM_FORMAT_MOD_INVALID
                        && vulkan
                            .check_importable(format, modifier, vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::SAMPLED)
                            .is_ok()
                });
            let usage = if sampled {
                vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::SAMPLED
            } else {
                vk::ImageUsageFlags::TRANSFER_SRC
            };
            let frame = av_vk_frame_alloc();
            if frame.is_null() {
                return Err("FFmpeg cannot allocate a Vulkan frame".into());
            }
            // From here on, `Drop` destroys what was made if a later step fails.
            let import = Self {
                vulkan: vulkan.clone(),
                frame,
                sampled,
                size: (width, height),
                pool_id,
                lent: AtomicUsize::new(0),
            };
            let device = &vulkan.device;
            let mut vkf = frame_fields(frame);
            *vkf.tiling = vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT;
            for (index, &format) in formats[..layer_count].iter().enumerate() {
                let (object, modifier, layout) = layer_layout(descriptor, index);
                if modifier == DRM_FORMAT_MOD_INVALID {
                    return Err(format!("the driver named no DRM format modifier for layer {index}"));
                }
                vulkan.check_importable(format, modifier, vk::ImageUsageFlags::TRANSFER_SRC)?;
                let (w, h) = layer_extent(index, width, height);
                let layouts = [layout];
                let mut explicit = vk::ImageDrmFormatModifierExplicitCreateInfoEXT::default()
                    .drm_format_modifier(modifier)
                    .plane_layouts(&layouts);
                let mut external = vk::ExternalMemoryImageCreateInfo::default()
                    .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
                let info = vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(format)
                    .extent(vk::Extent3D { width: w, height: h, depth: 1 })
                    .mip_levels(1)
                    .array_layers(1)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
                    .usage(usage)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE)
                    .initial_layout(vk::ImageLayout::UNDEFINED)
                    .push_next(&mut external)
                    .push_next(&mut explicit);
                let image = device
                    .create_image(&info, None)
                    .map_err(|e| format!("layer {index} (Vulkan format {}, modifier {modifier:#x}) cannot be imported as an image ({e})", format.as_raw()))?;
                vkf.img[index] = image;

                let fd = libc::dup(descriptor.objects[object].fd);
                if fd < 0 {
                    return Err(format!("layer {index}'s DMA-BUF cannot be duplicated ({})", std::io::Error::last_os_error()));
                }
                let mut fd_properties = vk::MemoryFdPropertiesKHR::default();
                if let Err(e) = vulkan.memory_fd.get_memory_fd_properties(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT, fd, &mut fd_properties) {
                    libc::close(fd);
                    return Err(format!("the driver does not import layer {index}'s DMA-BUF ({e})"));
                }
                let mut dedicated = vk::MemoryDedicatedRequirements::default();
                let mut needs = vk::MemoryRequirements2::default().push_next(&mut dedicated);
                device.get_image_memory_requirements2(&vk::ImageMemoryRequirementsInfo2::default().image(image), &mut needs);
                let requirements = needs.memory_requirements;
                let Some(kind) = import_memory_type(fd_properties.memory_type_bits, requirements.memory_type_bits, &vulkan.memory_types) else {
                    libc::close(fd);
                    return Err(format!(
                        "no memory type takes layer {index}'s DMA-BUF (types {:#x}) and its image (types {:#x})",
                        fd_properties.memory_type_bits, requirements.memory_type_bits
                    ));
                };
                let mut fd_import = vk::ImportMemoryFdInfoKHR::default()
                    .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT)
                    .fd(fd);
                let mut dedicated_to = vk::MemoryDedicatedAllocateInfo::default().image(image);
                let allocate = vk::MemoryAllocateInfo::default()
                    .allocation_size(requirements.size)
                    .memory_type_index(kind)
                    .push_next(&mut fd_import);
                let allocate = if dedicated.prefers_dedicated_allocation != vk::FALSE || dedicated.requires_dedicated_allocation != vk::FALSE {
                    allocate.push_next(&mut dedicated_to)
                } else {
                    allocate
                };
                // A successful import owns the descriptor; a failed one leaves it ours.
                let memory = device.allocate_memory(&allocate, None).map_err(|e| {
                    libc::close(fd);
                    format!("layer {index}'s DMA-BUF cannot be imported ({} bytes, memory type {kind}: {e})", requirements.size)
                })?;
                vkf.mem[index] = memory;
                vkf.size[index] = requirements.size as usize;
                *vkf.flags |= vulkan.memory_types[kind as usize];
                device
                    .bind_image_memory(image, memory, 0)
                    .map_err(|e| format!("layer {index}'s imported memory cannot be bound ({e})"))?;

                let mut timeline = vk::SemaphoreTypeCreateInfo::default()
                    .semaphore_type(vk::SemaphoreType::TIMELINE)
                    .initial_value(0);
                vkf.sem[index] = device
                    .create_semaphore(&vk::SemaphoreCreateInfo::default().push_next(&mut timeline), None)
                    .map_err(|e| format!("a timeline semaphore cannot be created ({e})"))?;
                vkf.sem_value[index] = 0;
                // Owned by VA-API, its contents those of the surface: the copy acquires it.
                vkf.layout[index] = vk::ImageLayout::UNDEFINED;
                vkf.access.set(index, vk::AccessFlags::empty());
                vkf.queue_family[index] = vk::QUEUE_FAMILY_EXTERNAL;
            }
            Ok(import)
        }
    }
}

impl Drop for CachedImport {
    fn drop(&mut self) {
        registry().lock().unwrap_or_else(|e| e.into_inner()).remove(&(self.frame as usize));
        // SAFETY: Objects this import made on the renderer's device, which outlives the decoder
        // and the presenter's pictures; the frame is no picture's any more.
        unsafe {
            let frame = frame_now(self.frame);
            if !self.vulkan.wait(&frame) {
                note("a kept VAAPI import's last copy did not finish; its Vulkan images stay allocated");
                return;
            }
            let device = &self.vulkan.device;
            for index in 0..frame.img.len() {
                if frame.img[index] != vk::Image::null() {
                    device.destroy_image(frame.img[index], None);
                }
                if frame.mem[index] != vk::DeviceMemory::null() {
                    device.free_memory(frame.mem[index], None);
                }
                if frame.sem[index] != vk::Semaphore::null() {
                    device.destroy_semaphore(frame.sem[index], None);
                }
            }
            free_frame(self.frame);
        }
    }
}

/// Frees an AVVkFrame of `av_vk_frame_alloc` as hwcontext_vulkan's `vulkan_free_internal`
/// does: its internal part begins with the `pthread_mutex_t` the default frame lock takes
/// (libavutil 59 to 61; the semaphore of 61's DRM export and the CUDA fields after it are never
/// set on these frames), which is destroyed, then both are freed.
///
/// # Safety
/// `frame` is an unused frame of `av_vk_frame_alloc`.
unsafe fn free_frame(frame: *mut VkFrame) {
    // SAFETY: Caller guarantees `frame` is an unused frame allocated by `av_vk_frame_alloc`.
    unsafe {
        let internal = frame_now(frame).internal;
        if !internal.is_null() {
            libc::pthread_mutex_destroy(internal.cast());
            av_free(internal);
        }
        av_free(frame.cast());
    }
}

/// The kept imports by their AVVkFrame's address (`AVFrame::data[0]` of a frame mapped from one),
/// weakly: a mapped frame holds its import (`Lent`), so a frame that is in use finds it here.
fn registry() -> &'static std::sync::Mutex<std::collections::HashMap<usize, std::sync::Weak<CachedImport>>> {
    static REGISTRY: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<usize, std::sync::Weak<CachedImport>>>> = std::sync::OnceLock::new();
    REGISTRY.get_or_init(Default::default)
}

/// A kept import the presenter holds on to while it has textures wrapping its images: they go
/// (on the decoder's thread, when the last holder lets go) only after the presenter let go.
#[derive(Clone)]
pub struct ImportRef(Arc<CachedImport>);

// SAFETY: As `CachedImport`.
unsafe impl Send for ImportRef {}

impl ImportRef {
    /// The images are SAMPLED too.
    pub fn sampled(&self) -> bool {
        self.0.sampled
    }

    /// The luma and chroma images.
    pub fn images(&self) -> [vk::Image; 2] {
        // SAFETY: The frame lives as long as the import.
        unsafe {
            let frame = frame_now(self.0.frame);
            [frame.img[0], frame.img[1]]
        }
    }

    /// The picture's size.
    pub fn size(&self) -> (u32, u32) {
        self.0.size
    }

    /// What names the import: its frame's address.
    pub fn key(&self) -> usize {
        self.0.frame as usize
    }

    /// The VA-API surface pool generation this import belongs to.
    pub fn pool_id(&self) -> u64 {
        self.0.pool_id
    }
}

/// The kept import `frame`, a Vulkan frame mapped from VAAPI, shows (none for any other frame).
///
/// # Safety
/// `frame` is a live AVFrame.
pub unsafe fn import_of(frame: *const AVFrame) -> Option<ImportRef> {
    // SAFETY: Caller guarantees `frame` is a live AVFrame.
    let key = unsafe { (*frame).data[0] as usize };
    registry().lock().unwrap_or_else(|e| e.into_inner()).get(&key)?.upgrade().map(ImportRef)
}

/// What a mapped frame of a kept import holds (its `buf[0]`'s opaque): its VAAPI picture, so the
/// surface is the presenter's until the frame is freed, and the import.
struct Lent {
    source: *mut AVFrame,
    import: Arc<CachedImport>,
}

impl Drop for Lent {
    fn drop(&mut self) {
        // SAFETY: The import's frame and our own reference to the picture.
        unsafe {
            // `Planes` keeps a picture until its copy is done, so this does not wait; a picture
            // freed earlier (its reference could not be kept) waits for the copy, as FFmpeg's
            // own unmap does, before VA-API may decode into the surface again.
            if !self.import.vulkan.wait(&frame_now(self.import.frame)) {
                note("a mapped VAAPI picture's copy did not finish; its surface goes back to the decoder anyway");
            }
            // Release after `frame_now` and semaphore wait finish so `SurfaceImports::map` never
            // mutates `import.frame` concurrently with this drop.
            self.import.lent.fetch_sub(1, Ordering::Release);
            av_frame_free(&mut self.source);
        }
    }
}

unsafe extern "C" fn give_back(opaque: *mut c_void, _data: *mut u8) {
    // SAFETY: `opaque` was created with `Box::into_raw(Box::new(Lent { .. }))` in `lend`.
    unsafe { drop(Box::from_raw(opaque.cast::<Lent>())) };
}

/// The imports of the surfaces of the pool pictures come from now (`SurfaceCache`), and the
/// renderer's device they are made on.
pub struct SurfaceImports {
    vulkan: Arc<ImportDevice>,
    cache: SurfaceCache<Arc<CachedImport>>,
    /// Monotonic generation ID of the current surface pool.
    pool_id: u64,
    /// The frames context whose surfaces the cache holds imports of, kept alive with it so
    /// no other pool takes its address (the cache's key).
    _pool: FramesRef,
    /// `dmabuf::export`'s, which only the OpenGL import sets.
    decline: Arc<AtomicBool>,
    /// New imports are made to be sampled where the driver takes them so (`SAMPLE_ENVIRONMENT`).
    sample: bool,
}

impl SurfaceImports {
    /// The imports for VAAPI pictures on the renderer `gpu`, unless `NIGHTJAR_DMABUF_CACHE=0`
    /// or its device cannot be loaded; which path the pictures take goes to the log.
    ///
    /// # Safety
    /// As `ImportDevice::of`.
    pub(super) unsafe fn start(gpu: *mut SDL_GPUDevice) -> Option<Self> {
        if !dmabuf::caching() {
            note(&format!("VAAPI pictures are imported into Vulkan anew for every picture: {}=0", dmabuf::CACHE_ENVIRONMENT));
            return None;
        }
        // SAFETY: Caller guarantees `gpu` is a live SDL GPU device with Vulkan sharing properties.
        match unsafe { ImportDevice::of(gpu) } {
            Ok(vulkan) => {
                note("VAAPI pictures are imported into Vulkan once per decoder surface, kept while the pool lasts");
                Some(Self {
                    vulkan: Arc::new(vulkan),
                    cache: SurfaceCache::new(CACHE_LIMIT),
                    pool_id: NEXT_POOL_ID.fetch_add(1, Ordering::Relaxed),
                    _pool: FramesRef::none(),
                    decline: Arc::new(AtomicBool::new(false)),
                    sample: sample_switched_on(std::env::var_os(SAMPLE_ENVIRONMENT).as_deref()),
                })
            }
            Err(why) => {
                note(&format!("VAAPI pictures are imported into Vulkan anew for every picture: {why}"));
                None
            }
        }
    }

    /// Maps the two-plane 4:2:0 or single-layer 4:4:4 VAAPI picture `source` into `target`, an
    /// empty frame, as a frame of `frames` (`VaapiMap`'s, marked `MAPPED`) whose images are the
    /// surface's kept import, made now if the surface has none. True when it was kept. On an
    /// error `target` is empty again.
    ///
    /// # Safety
    /// `source` is a live VAAPI frame, `target` an empty one, `frames` a live Vulkan frames
    /// context on the renderer's device.
    pub(super) unsafe fn map(
        &mut self,
        source: *const AVFrame,
        target: *mut AVFrame,
        frames: *mut AVBufferRef,
        ten_bit: bool,
        interleaved: bool,
    ) -> Result<bool, String> {
        // SAFETY: Caller guarantees `source`, `target`, and `frames` are live FFmpeg objects as documented.
        unsafe {
            // Syncs the surface: the decode is done before any copy reads it.
            let export = dmabuf::export(source, &self.decline)?;
            let generation = export.generation();
            // Another pool or size: its surfaces are other buffers. The old imports go once no
            // mapped frame uses them.
            let (changed, gone) = self.cache.retarget(generation);
            drop(gone);
            if changed {
                self._pool = FramesRef::of(export.frames());
                self.pool_id = NEXT_POOL_ID.fetch_add(1, Ordering::Relaxed);
            }
            let (import, cached) = match self.cache.get(export.surface()) {
                Some(import) if can_reuse_cached_import(import.lent.load(Ordering::Acquire)) => {
                    (import.clone(), true)
                }
                Some(_) => {
                    // Duplicate live reference to the same VA-API surface (e.g. AV1
                    // `show_existing_frame`): allocate an uncached copy-only import instead of
                    // mutating the still-lent `AVVkFrame` across threads.
                    let import = Arc::new(CachedImport::new(
                        &self.vulkan,
                        export.descriptor()?,
                        generation.width,
                        generation.height,
                        ten_bit,
                        interleaved,
                        false,
                        self.pool_id,
                    )?);
                    (import, false)
                }
                None => {
                    let import = Arc::new(CachedImport::new(
                        &self.vulkan,
                        export.descriptor()?,
                        generation.width,
                        generation.height,
                        ten_bit,
                        interleaved,
                        self.sample,
                        self.pool_id,
                    )?);
                    registry().lock().unwrap_or_else(|e| e.into_inner()).insert(import.frame as usize, Arc::downgrade(&import));
                    drop(self.cache.insert(export.surface(), import.clone()));
                    (import, false)
                }
            };
            // The exported descriptors close here; the imports hold their own.
            drop(export);
            // Exclusive: either newly created or verified `lent == 0` with Acquire ordering.
            ready_for_reuse(&mut frame_fields(import.frame));
            lend(target, source, frames, import)?;
            Ok(cached)
        }
    }
}

/// Makes `target` a Vulkan frame of `frames` showing `import`'s images, holding `source` and
/// the import until it is freed (`Lent`).
///
/// # Safety
/// As `SurfaceImports::map`.
unsafe fn lend(target: *mut AVFrame, source: *const AVFrame, frames: *mut AVBufferRef, import: Arc<CachedImport>) -> Result<(), String> {
    // SAFETY: Caller guarantees `target`, `source`, and `frames` are valid per `SurfaceImports::map`.
    unsafe {
        let held = av_frame_clone(source);
        if held.is_null() {
            return Err("the VAAPI picture cannot be referenced (out of memory)".into());
        }
        let vkf = import.frame;
        import.lent.fetch_add(1, Ordering::AcqRel);
        let opaque = Box::into_raw(Box::new(Lent { source: held, import }));
        let buffer = av_buffer_create(vkf.cast(), StructLayout::linked().frame_size(), Some(give_back), opaque.cast(), 0);
        if buffer.is_null() {
            drop(Box::from_raw(opaque));
            return Err("FFmpeg cannot allocate the mapped picture's buffer".into());
        }
        (*target).buf[0] = buffer;
        (*target).data[0] = vkf.cast();
        (*target).format = AV_PIX_FMT_VULKAN;
        (*target).width = (*source).width;
        (*target).height = (*source).height;
        (*target).hw_frames_ctx = av_buffer_ref(frames);
        if (*target).hw_frames_ctx.is_null() {
            av_frame_unref(target);
            return Err("FFmpeg cannot reference the Vulkan frames context".into());
        }
        Ok(())
    }
}
