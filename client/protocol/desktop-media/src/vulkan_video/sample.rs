//! VA-API pictures sampled where VA-API wrote them, instead of copied into the presenter's planes
//! (`Planes::copy`): the presenter wraps the kept import's two images as SDL textures, hands the
//! picture to the renderer with one barrier before its draws and back to VA-API with one after
//! them, and holds it between (`Sampled`). It follows how PyroWave's planes are sampled in place.
//! A surface the driver does not sample (`ImportRef::sampled`), or anything else this does not
//! take, is copied as before, picture by picture.
//!
//! The wrapped textures belong to the presenter (`Planes`), which holds the import they wrap:
//! the import's own drop runs on FFmpeg's thread, whenever the last picture of its surface goes.

use std::collections::HashMap;

use super::imports::{import_of, ImportRef};
use super::*;

/// Surfaces whose images are wrapped at most; a stream's pool has a few dozen at the most.
const WRAPPED_LIMIT: usize = 64;

type FrameLock = unsafe extern "C" fn(*mut AVHWFramesContext, *mut VkFrame);

/// An import's two images as SDL textures of the renderer's device.
struct Wrapped {
    /// Held so the images outlive the textures (the import is dropped by its last holder).
    _import: ImportRef,
    textures: [*mut SDL_GPUTexture; 2],
}

/// One of the queue's turns: the barriers that acquire a picture, those that release one (with the
/// fence that tells when its release is done) and the picture the slot's turn is about.
struct Slot {
    acquire: vk::CommandBuffer,
    release: vk::CommandBuffer,
    fence: vk::Fence,
    frame: *mut VkFrame,
}

/// What `Planes` keeps to sample pictures in place.
pub(super) struct Sampling {
    switched_on: bool,
    slots: Vec<Slot>,
    state: Sampled<HeldFrame>,
    wrapped: HashMap<usize, Wrapped>,
    /// Pool generation ID of the entries currently in `wrapped`; a new pool evicts old entries.
    pool_id: Option<u64>,
    /// True after `evict_wrapped` released old textures so `PlanesPass` clears `sampled_states`.
    wrapped_evicted: bool,
    renderer: *mut SDL_Renderer,
    /// The frames context and frame locks of the last picture shown, for releasing it at the end.
    locks: Option<(*mut AVHWFramesContext, FrameLock, FrameLock)>,
    /// Sampling failed on the device: pictures are copied from now on.
    off: bool,
    said_sample: bool,
    said_copy: bool,
}

impl Sampling {
    pub(super) fn new() -> Self {
        Self {
            switched_on: sample_switched_on(std::env::var_os(SAMPLE_ENVIRONMENT).as_deref()),
            slots: Vec::new(),
            state: Sampled::new(SLOTS),
            wrapped: HashMap::new(),
            pool_id: None,
            wrapped_evicted: false,
            renderer: ptr::null_mut(),
            locks: None,
            off: false,
            said_sample: false,
            said_copy: false,
        }
    }
}

/// The barrier that hands `image` of a kept import to the renderer for sampling.
fn sampled_acquire(source: &FrameNow, index: usize, family: u32) -> vk::ImageMemoryBarrier2<'static> {
    let (old_layout, from, to) = acquire(source.queue_family[index], source.layout[index], family);
    let src_stage = if source.access[index].is_empty() {
        vk::PipelineStageFlags2::FRAGMENT_SHADER
    } else {
        vk::PipelineStageFlags2::ALL_COMMANDS
    };
    vk::ImageMemoryBarrier2::default()
        .src_stage_mask(src_stage)
        .src_access_mask(source.access[index])
        .dst_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
        .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
        .old_layout(old_layout)
        .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
        .src_queue_family_index(from)
        .dst_queue_family_index(to)
        .image(source.img[index])
        .subresource_range(color_range())
}

/// The barrier that gives a sampled `image` back to VA-API, behind the draws that read it.
fn sampled_release(image: vk::Image, family: u32) -> vk::ImageMemoryBarrier2<'static> {
    vk::ImageMemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
        .src_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
        .dst_stage_mask(vk::PipelineStageFlags2::NONE)
        .dst_access_mask(vk::AccessFlags2::NONE)
        .old_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
        .new_layout(vk::ImageLayout::GENERAL)
        .src_queue_family_index(family)
        .dst_queue_family_index(vk::QUEUE_FAMILY_EXTERNAL)
        .image(image)
        .subresource_range(color_range())
}

fn color_range() -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange { aspect_mask: vk::ImageAspectFlags::COLOR, base_mip_level: 0, level_count: 1, base_array_layer: 0, layer_count: 1 }
}

/// The renderer's queue as `Sampled` uses it.
struct Queue<'a> {
    device: &'a ash::Device,
    queue: vk::Queue,
    family: u32,
    lock: *mut SDL_Mutex,
    renderer: *mut SDL_Renderer,
    slots: &'a mut Vec<Slot>,
    /// The picture about to be shown, for `acquire`.
    incoming: *mut VkFrame,
    frames: *mut AVHWFramesContext,
    lock_frame: FrameLock,
    unlock_frame: FrameLock,
}

impl Queue<'_> {
    unsafe fn submit(&self, commands: vk::CommandBuffer, barriers: &[vk::ImageMemoryBarrier2], waits: &[vk::SemaphoreSubmitInfo], signals: &[vk::SemaphoreSubmitInfo], fence: vk::Fence) -> Result<(), String> {
        let device = self.device;
        // SAFETY: Caller guarantees `commands`, `barriers`, `waits`, `signals`, and `fence` belong to `self.device`.
        unsafe {
            device.reset_command_buffer(commands, vk::CommandBufferResetFlags::empty()).map_err(|e| format!("a barrier's command buffer cannot be reset ({e})"))?;
            device
                .begin_command_buffer(commands, &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT))
                .map_err(|e| format!("a barrier cannot be recorded ({e})"))?;
            device.cmd_pipeline_barrier2(commands, &vk::DependencyInfo::default().image_memory_barriers(barriers));
            device.end_command_buffer(commands).map_err(|e| format!("a barrier cannot be recorded ({e})"))?;
            let buffers = [vk::CommandBufferSubmitInfo::default().command_buffer(commands)];
            let submit = vk::SubmitInfo2::default().wait_semaphore_infos(waits).command_buffer_infos(&buffers).signal_semaphore_infos(signals);
            if fence != vk::Fence::null() {
                device.reset_fences(&[fence]).map_err(|e| format!("a fence cannot be reset ({e})"))?;
            }
            SDL_LockMutex(self.lock);
            let submitted = device.queue_submit2(self.queue, &[submit], fence);
            SDL_UnlockMutex(self.lock);
            if let Err(e) = submitted {
                if fence != vk::Fence::null() {
                    // Never submitted: the fence would stay unsignalled and its slot's wait time out.
                    SDL_LockMutex(self.lock);
                    let _ = device.queue_submit2(self.queue, &[], fence);
                    SDL_UnlockMutex(self.lock);
                }
                return Err(format!("a barrier cannot be submitted ({e})"));
            }
            Ok(())
        }
    }
}

impl SampleGpu for Queue<'_> {
    fn flush(&mut self) -> Result<(), String> {
        // SAFETY: The presenter's live renderer, on its thread.
        if unsafe { SDL_FlushRenderer(self.renderer) } { Ok(()) } else { Err(format!("the renderer cannot flush: {}", crate::sdl::error())) }
    }

    fn wait(&mut self, slot: usize) -> Result<(), String> {
        // SAFETY: A fence of this device.
        match unsafe { self.device.wait_for_fences(&[self.slots[slot].fence], true, COPY_WAIT.as_nanos() as u64) } {
            Ok(()) => Ok(()),
            Err(vk::Result::TIMEOUT) => Err(format!("an earlier picture's release did not finish within {COPY_WAIT:?}: the Vulkan device may be hung or lost")),
            Err(e) => Err(format!("waiting for an earlier picture's release failed ({e})")),
        }
    }

    fn finished(&self, slot: usize) -> bool {
        // SAFETY: A fence of this device.
        unsafe { self.device.get_fence_status(self.slots[slot].fence) == Ok(true) }
    }

    fn acquire(&mut self, slot: usize) -> Result<(), String> {
        // SAFETY: `incoming` is the live frame of a picture the presenter holds; its lock is FFmpeg's.
        unsafe {
            (self.lock_frame)(self.frames, self.incoming);
            let source = &frame_now(self.incoming);
            let barriers: [_; 2] = std::array::from_fn(|index| sampled_acquire(source, index, self.family));
            let waits: [_; 2] = std::array::from_fn(|index| {
                vk::SemaphoreSubmitInfo::default().semaphore(source.sem[index]).value(source.sem_value[index]).stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
            });
            let result = self.submit(self.slots[slot].acquire, &barriers, &waits, &[], vk::Fence::null());
            (self.unlock_frame)(self.frames, self.incoming);
            if result.is_ok() {
                self.slots[slot].frame = self.incoming;
            }
            result
        }
    }

    fn release(&mut self, slot: usize) -> Result<(), String> {
        let frame = self.slots[slot].frame;
        if frame.is_null() {
            return Err("the picture to release was never acquired".into());
        }
        // SAFETY: The frame of a picture the presenter still holds; its lock is FFmpeg's.
        unsafe {
            (self.lock_frame)(self.frames, frame);
            let mut source = frame_fields(frame);
            let barriers: [_; 2] = std::array::from_fn(|index| sampled_release(source.img[index], self.family));
            let signals: [_; 2] = std::array::from_fn(|index| {
                // `sampled_release` uses `dst_stage_mask = NONE` for the QFOT/layout release;
                // signal at `ALL_COMMANDS` so the release transition finishes before VA-API unblocks.
                vk::SemaphoreSubmitInfo::default().semaphore(source.sem[index]).value(source.sem_value[index] + 1).stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
            });
            let result = self.submit(self.slots[slot].release, &barriers, &[], &signals, self.slots[slot].fence);
            if result.is_ok() {
                for index in 0..2 {
                    source.sem_value[index] += 1;
                    source.layout[index] = vk::ImageLayout::GENERAL;
                    source.queue_family[index] = vk::QUEUE_FAMILY_EXTERNAL;
                    source.access.set(index, vk::AccessFlags::empty());
                }
            }
            (self.unlock_frame)(self.frames, frame);
            self.slots[slot].frame = ptr::null_mut();
            result
        }
    }

    fn swap(&mut self, acquire_slot: usize, release_slot: usize) -> Result<(), String> {
        let old_frame = self.slots[release_slot].frame;
        if old_frame.is_null() {
            return Err("the picture to release was never acquired".into());
        }
        if old_frame == self.incoming {
            return swap_same_frame(self, acquire_slot, release_slot);
        }
        // SAFETY: Both `incoming` and `old_frame` are live frames held by the presenter; lock in
        // pointer order to avoid any AB-BA inversion.
        unsafe {
            let (first, second) = if (old_frame as usize) < (self.incoming as usize) {
                (old_frame, self.incoming)
            } else {
                (self.incoming, old_frame)
            };
            (self.lock_frame)(self.frames, first);
            (self.lock_frame)(self.frames, second);
            let incoming = frame_now(self.incoming);
            let mut old = frame_fields(old_frame);
            let barriers = [
                sampled_acquire(&incoming, 0, self.family),
                sampled_acquire(&incoming, 1, self.family),
                sampled_release(old.img[0], self.family),
                sampled_release(old.img[1], self.family),
            ];
            let waits: [_; 2] = std::array::from_fn(|index| {
                vk::SemaphoreSubmitInfo::default().semaphore(incoming.sem[index]).value(incoming.sem_value[index]).stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
            });
            let signals: [_; 2] = std::array::from_fn(|index| {
                // `sampled_release` uses `dst_stage_mask = NONE` for the QFOT/layout release;
                // signal at `ALL_COMMANDS` so the release transition finishes before VA-API unblocks.
                vk::SemaphoreSubmitInfo::default().semaphore(old.sem[index]).value(old.sem_value[index] + 1).stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
            });
            let result = self.submit(self.slots[release_slot].release, &barriers, &waits, &signals, self.slots[release_slot].fence);
            if result.is_ok() {
                for index in 0..2 {
                    old.sem_value[index] += 1;
                    old.layout[index] = vk::ImageLayout::GENERAL;
                    old.queue_family[index] = vk::QUEUE_FAMILY_EXTERNAL;
                    old.access.set(index, vk::AccessFlags::empty());
                }
                self.slots[acquire_slot].frame = self.incoming;
            }
            (self.unlock_frame)(self.frames, second);
            (self.unlock_frame)(self.frames, first);
            self.slots[release_slot].frame = ptr::null_mut();
            result
        }
    }
}

impl Planes {
    /// Hands the VA-API picture `frame` to the renderer for sampling in place, and returns the
    /// textures of its two images for the plane shader, or `None` when it is to be copied
    /// (`copy`), say why once: sampling is off, the picture is no kept VA-API import, the driver
    /// does not sample its format and modifier, or its size is not the planes'. The picture is
    /// held until the next one is shown; the renderer's draws so far are flushed first, then the
    /// shown picture is released to VA-API behind them (`Sampled::show`).
    ///
    /// # Safety
    /// `frame` must be a live Vulkan AVFrame made with `device_context` on this device, and
    /// `renderer` the live renderer of the planes' device, on its thread.
    pub unsafe fn sample(&mut self, renderer: *mut SDL_Renderer, frame: *const AVFrame) -> Result<Option<[*mut SDL_GPUTexture; 2]>, String> {
        // SAFETY: Caller guarantees `frame` is a live Vulkan AVFrame and `renderer` is the live renderer on its thread.
        unsafe {
            if self.interleaved || self.chroma444 {
                return Ok(None);
            }
            let sampling = self.sampling.get_or_insert_with(Sampling::new);
            if sampling.off {
                return Ok(None);
            }
            let frames_ref = (*frame).hw_frames_ctx;
            let source = (*frame).data[0].cast::<VkFrame>();
            if frames_ref.is_null() || source.is_null() {
                return Ok(None);
            }
            let frames = (*frames_ref).data.cast::<AVHWFramesContext>();
            let import = if mapped_frames(&*frames) { import_of(frame) } else { None };
            let eligibility = Eligibility {
                switched_on: sampling.switched_on,
                sampleable: import.as_ref().is_some_and(ImportRef::sampled),
                spare_surface: true,
                stable_copy: false,
            };
            let why = match route(eligibility) {
                Route::Copy(_) if import.is_none() && sampling.switched_on => Route::Copy("the picture is not a kept VA-API import"),
                route => route,
            };
            let (Route::Sample, Some(import)) = (why, import) else {
                if !sampling.said_copy {
                    sampling.said_copy = true;
                    note(&describe_route(why));
                }
                return Ok(None);
            };
            if import.size() != (self.width, self.height) || sw_format(frame) != picture_sw_format(self.ten_bit, false) {
                return Ok(None);
            }
            let vulkan_frames = &*(*frames).hwctx.cast::<FramesContext>();
            let (Some(lock_frame), Some(unlock_frame)) = (vulkan_frames.lock_frame, vulkan_frames.unlock_frame) else {
                return Ok(None);
            };
            if self.sampling.as_ref().is_some_and(|s| should_evict_pool(s.pool_id, import.pool_id()))
                && !self.evict_wrapped(import.pool_id())
            {
                return Ok(None);
            }
            if let Some(sampling) = self.sampling.as_mut()
                && sampling.pool_id.is_none()
            {
                sampling.pool_id = Some(import.pool_id());
            }
            let Some(textures) = self.wrap(&import) else {
                return Ok(None);
            };
            let held = av_frame_clone(frame);
            if held.is_null() {
                return Ok(None);
            }
            let held = HeldFrame(held);
            let Planes { device, queue, family, lock, sampling, pool, .. } = self;
            let sampling = sampling.as_mut().expect("made above");
            if sampling.slots.is_empty()
                && let Err(error) = Self::make_slots(device, *pool, &mut sampling.slots)
            {
                note(&format!("VA-API pictures are copied: {error}"));
                sampling.off = true;
                return Ok(None);
            }
            sampling.renderer = renderer;
            sampling.locks = Some((frames, lock_frame, unlock_frame));
            let mut gpu = Queue {
                device,
                queue: *queue,
                family: *family,
                lock: *lock,
                renderer,
                slots: &mut sampling.slots,
                incoming: source,
                frames,
                lock_frame,
                unlock_frame,
            };
            match sampling.state.show(&mut gpu, held) {
                Ok(()) => {
                    if !sampling.said_sample {
                        sampling.said_sample = true;
                        note(&describe_route(Route::Sample));
                    }
                    Ok(Some(textures))
                }
                Err((_, error)) => {
                    // The picture is not shown: it is copied instead. A device that fails here is not
                    // sampled on again.
                    note(&format!("VA-API pictures are copied from now on: sampling failed ({error})"));
                    sampling.off = true;
                    Ok(None)
                }
            }
        }
    }

    unsafe fn make_slots(device: &ash::Device, pool: vk::CommandPool, slots: &mut Vec<Slot>) -> Result<(), String> {
        // SAFETY: Caller guarantees `device` and `pool` are live Vulkan handles.
        unsafe {
            let buffers = device
                .allocate_command_buffers(
                    &vk::CommandBufferAllocateInfo::default().command_pool(pool).level(vk::CommandBufferLevel::PRIMARY).command_buffer_count(2 * SLOTS as u32),
                )
                .map_err(|e| format!("command buffers cannot be allocated ({e})"))?;
            for pair in buffers.chunks(2) {
                let fence = device
                    .create_fence(&vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED), None)
                    .map_err(|e| format!("a fence cannot be created ({e})"))?;
                slots.push(Slot { acquire: pair[0], release: pair[1], fence, frame: ptr::null_mut() });
            }
            Ok(())
        }
    }

    /// The textures wrapping `import`'s images, made on first use; none (with the reason logged
    /// once) when the textures cannot be made or too many surfaces are wrapped.
    unsafe fn wrap(&mut self, import: &ImportRef) -> Option<[*mut SDL_GPUTexture; 2]> {
        let key = import.key();
        let sampling = self.sampling.as_mut()?;
        if let Some(wrapped) = sampling.wrapped.get(&key) {
            return Some(wrapped.textures);
        }
        if sampling.wrapped.len() >= WRAPPED_LIMIT {
            if !sampling.said_copy {
                sampling.said_copy = true;
                note("VA-API pictures are copied into the presenter's planes: too many decoder surfaces are wrapped");
            }
            return None;
        }
        let mut textures = [ptr::null_mut(); 2];
        let images = import.images();
        // SAFETY: Caller guarantees `self.gpu` is live and `import`'s images belong to `self.gpu`'s Vulkan device.
        unsafe {
            for (plane, texture) in textures.iter_mut().enumerate() {
                let (width, height) = layer_extent(plane, import.size().0, import.size().1);
                let props = SDL_CreateProperties();
                SDL_SetNumberProperty(props, TEXTURE_IMAGE_PROPERTY.as_ptr(), images[plane].as_raw() as i64);
                *texture = SDL_CreateGPUTexture(
                    self.gpu,
                    &SDL_GPUTextureCreateInfo {
                        r#type: SDL_GPU_TEXTURETYPE_2D,
                        format: self.sdl_format(plane),
                        usage: SDL_GPU_TEXTUREUSAGE_SAMPLER,
                        width,
                        height,
                        layer_count_or_depth: 1,
                        num_levels: 1,
                        sample_count: SDL_GPU_SAMPLECOUNT_1,
                        props,
                    },
                );
                SDL_DestroyProperties(props);
            }
            if textures.iter().any(|texture| texture.is_null()) {
                let error = crate::sdl::error();
                for texture in textures.into_iter().filter(|texture| !texture.is_null()) {
                    SDL_ReleaseGPUTexture(self.gpu, texture);
                }
                let sampling = self.sampling.as_mut()?;
                if !sampling.said_copy {
                    sampling.said_copy = true;
                    note(&format!("VA-API pictures are copied into the presenter's planes: a decoder surface cannot be wrapped as textures: {error}"));
                }
                return None;
            }
        }
        self.sampling.as_mut()?.wrapped.insert(key, Wrapped { _import: import.clone(), textures });
        Some(textures)
    }

    /// Whether `sample()` evicted an earlier VA-API pool's wrapped textures, so `PlanesPass` must
    /// destroy its cached `sampled_states`. Cleared when read.
    pub fn take_wrapped_evicted(&mut self) -> bool {
        self.sampling.as_mut().is_some_and(|s| std::mem::take(&mut s.wrapped_evicted))
    }

    /// Evicts wrapped textures belonging to a retired VA-API surface pool after releasing any
    /// sampled picture and waiting for the GPU to finish reading them.
    unsafe fn evict_wrapped(&mut self, next_pool_id: u64) -> bool {
        // SAFETY: Caller guarantees `self.gpu` and `self.device` are live on the presenting thread.
        unsafe {
            if !self.end_sampling() {
                if let Some(sampling) = self.sampling.as_mut() {
                    sampling.off = true;
                }
                return false;
            }
            let old = match self.sampling.as_mut() {
                Some(sampling) => std::mem::take(&mut sampling.wrapped),
                None => return false,
            };
            for wrapped in old.values() {
                for texture in wrapped.textures {
                    SDL_ReleaseGPUTexture(self.gpu, texture);
                }
            }
            let idle = SDL_WaitForGPUIdle(self.gpu);
            if let Some(sampling) = self.sampling.as_mut() {
                sampling.wrapped_evicted = true;
                sampling.pool_id = Some(next_pool_id);
                if !idle {
                    sampling.off = true;
                }
            }
            if !idle {
                note("the GPU did not go idle while retiring VA-API wrapped textures; keeping their imports allocated");
                std::mem::forget(old);
                return false;
            }
            drop(old);
            true
        }
    }

    /// Gives the shown picture back to VA-API behind the renderer's draws so far and waits for
    /// every held picture to come back; before the planes are copied into (the copy overwrites what
    /// a sampled picture's draws may still need) and before they are dropped. False when a
    /// release failed (the device may be lost).
    pub(super) unsafe fn end_sampling(&mut self) -> bool {
        let Planes { device, queue, family, lock, sampling, .. } = self;
        let Some(sampling) = sampling.as_mut() else { return true };
        if sampling.state.held() == 0 {
            return true;
        }
        let Some((frames, lock_frame, unlock_frame)) = sampling.locks else { return false };
        if sampling.slots.is_empty() || sampling.renderer.is_null() {
            return false;
        }
        let mut gpu = Queue {
            device,
            queue: *queue,
            family: *family,
            lock: *lock,
            renderer: sampling.renderer,
            slots: &mut sampling.slots,
            incoming: ptr::null_mut(),
            frames,
            lock_frame,
            unlock_frame,
        };
        match sampling.state.finish(&mut gpu) {
            Ok(()) => true,
            Err(error) => {
                note(&format!("the sampled VA-API pictures could not be given back: {error}"));
                false
            }
        }
    }

    /// What `Drop` does for sampling before it waits for the device: gives the pictures back and
    /// releases the wrapped textures (SDL destroys their views once its last command is done).
    pub(super) unsafe fn release_wrapped(&mut self) {
        // SAFETY: Caller guarantees `self.gpu` and `self.device` are live and on the presenting thread.
        unsafe {
            self.end_sampling();
            if let Some(sampling) = &self.sampling {
                for wrapped in sampling.wrapped.values() {
                    for texture in wrapped.textures {
                        SDL_ReleaseGPUTexture(self.gpu, texture);
                    }
                }
            }
        }
    }

    /// What `Drop` does for sampling once the device is idle (`idle`): lets the imports go; with
    /// the device not idle their images stay allocated.
    pub(super) unsafe fn drop_wrapped(&mut self, idle: bool) {
        let Some(sampling) = self.sampling.take() else { return };
        let Sampling { slots, wrapped, state, .. } = sampling;
        drop(state);
        if !idle {
            // The views may still be in use: keep the imports (and their images) rather than free them.
            std::mem::forget(wrapped);
            return;
        }
        let waits: Vec<vk::Fence> = slots.iter().map(|slot| slot.fence).collect();
        for fence in waits {
            // SAFETY: Caller guarantees `self.device` is idle and owns these fences.
            unsafe { self.device.destroy_fence(fence, None) };
        }
        drop(wrapped);
    }
}
