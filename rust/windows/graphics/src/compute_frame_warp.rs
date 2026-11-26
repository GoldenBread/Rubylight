//! Frame Warp's passes over captures (docs/frame-warp.md) on the capture
//! GPU's compute queue, for every capture format and either encoder: a luma
//! thumbnail of each new capture for the host's motion measurement and,
//! while the camera turns, which pixels stay the same within a small
//! tolerance, scored per HUD mask cell (`frame_warp_*_cs` in
//! `shaders/color.hlsl`). A capture's
//! list goes in after its frame went to the encoder, and nothing waits for
//! one on the CPU: [`FrameWarpGpu::poll`] hands back the passes that have
//! finished, usually a frame later.
use super::*;
use crate::capture::GpuImage;
#[cfg(test)]
use crate::capture::Pixel;

#[path = "compute_frame_warp_spec.rs"]
mod spec;
use spec::{
    CLEAR, CONSTANTS, Layout, RESET, SCORE, THUMBNAIL_BYTES, constants, layout, root_signature,
};
pub use spec::{
    HUD_SCORE, Hud, MAX_GRID_EDGE, MaskReadout, Pass, THUMBNAIL_EDGE, ThumbnailReadout, hud_grid,
};

struct Slot {
    allocator: ID3D12CommandAllocator,
    list: ID3D12GraphicsCommandList,
    /// Persistently mapped READBACK copy of the thumbnail.
    readback: ID3D12Resource,
    mapped: *const u8,
    /// The fence value of the slot's last list, 0 before its first.
    fence: u64,
    /// The thumbnail in `readback` once the list has run, not yet polled.
    thumbnail: Option<(u64, (u32, u32))>,
    /// The capture and its D3D12 view of it, kept until the list has run.
    /// Only a pool copy's reuse waits for this queue on the GPU (`read_by`);
    /// a keyed or fence-ring lease hands its texture back behind its own
    /// compute queue, which `can_read` does not prove is this one.
    capture: Option<(GpuImage, ID3D12Resource)>,
}
struct HudBuffer {
    buffer: ID3D12Resource,
    source: (u32, u32),
    grid: (u32, u32),
    layout: Layout,
    initialized: bool,
}
/// A mask copy in flight to [`FrameWarpGpu::mask_readback`].
struct MaskRead {
    fence: u64,
    grid: (u32, u32),
    stride: u32,
}
/// A persistently mapped READBACK buffer for the mask bits of one grid size.
struct MaskReadback {
    buffer: ID3D12Resource,
    mapped: *const u32,
    words: usize,
}

/// The passes' GPU objects on one compute queue.
pub struct FrameWarpGpu {
    compute: Arc<Compute>,
    /// The capture device these passes were made for ([`FrameWarpGpu::for_image`]).
    device: Option<windows::Win32::Graphics::Direct3D11::ID3D11Device>,
    root: ID3D12RootSignature,
    thumb: ID3D12PipelineState,
    hud: ID3D12PipelineState,
    score: ID3D12PipelineState,
    bits: ID3D12PipelineState,
    /// One capture view per slot.
    heap: ID3D12DescriptorHeap,
    increment: u32,
    /// The thumbnail's words, in COMMON between lists.
    thumbnail: ID3D12Resource,
    slots: Vec<Slot>,
    /// The HUD's words for the current capture size and grid, in COMMON
    /// between lists.
    hud_buffer: Option<HudBuffer>,
    /// The codes are a capture's that the next HUD pass compares with: the
    /// last list ran a HUD pass, or held its codes ([`Hud::every_us`]).
    primed: bool,
    /// The `at` of the capture whose codes those are.
    taken_at: u64,
    mask_read: Option<MaskRead>,
    /// Kept between mask reads, replaced only for a new grid size while no
    /// copy into it is in flight.
    mask_readback: Option<MaskReadback>,
    /// Buffers replaced while a list may still use them, with that fence value.
    retired: Vec<(u64, ID3D12Resource)>,
    /// The fence value of the last list submitted.
    last: u64,
}
// SAFETY: The D3D12 objects are free-threaded; the mapped pointers are only
// read through `&mut self`, by whichever thread owns the passes.
unsafe impl Send for FrameWarpGpu {}

impl FrameWarpGpu {
    /// The passes on `compute`, the queue of the GPU the captures are on
    /// ([`Compute::for_device`]).
    pub fn new(compute: Arc<Compute>) -> Result<Self> {
        compute.ensure_active()?;
        let device = &compute.device;
        let root = root_signature(device)?;
        let thumb = pipeline(device, &root, b"frame_warp_thumb_cs\0")?;
        let hud = pipeline(device, &root, b"frame_warp_hud_cs\0")?;
        let score = pipeline(device, &root, b"frame_warp_score_cs\0")?;
        let bits = pipeline(device, &root, b"frame_warp_bits_cs\0")?;
        let thumbnail = focus_buffer(
            device,
            D3D12_HEAP_TYPE_DEFAULT,
            THUMBNAIL_BYTES,
            D3D12_RESOURCE_STATE_COMMON,
        )?;
        // SAFETY: Calls on the live device; each list is closed until a pass records it, and a
        // READBACK buffer stays mapped for its life (Drop unmaps it).
        unsafe {
            let heap: ID3D12DescriptorHeap =
                device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                    Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                    NumDescriptors: SLOTS as u32,
                    Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                    NodeMask: 0,
                })?;
            let increment =
                device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV);
            let mut slots = Vec::with_capacity(SLOTS);
            for _ in 0..SLOTS {
                let allocator: ID3D12CommandAllocator =
                    device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_COMPUTE)?;
                let list: ID3D12GraphicsCommandList = device.CreateCommandList(
                    0,
                    D3D12_COMMAND_LIST_TYPE_COMPUTE,
                    &allocator,
                    None,
                )?;
                list.Close()?;
                let readback = focus_buffer(
                    device,
                    D3D12_HEAP_TYPE_READBACK,
                    THUMBNAIL_BYTES,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                )?;
                let mut mapped = std::ptr::null_mut();
                readback.Map(0, None, Some(&mut mapped))?;
                slots.push(Slot {
                    allocator,
                    list,
                    readback,
                    mapped: mapped.cast_const().cast::<u8>(),
                    fence: 0,
                    thumbnail: None,
                    capture: None,
                });
            }
            Ok(Self {
                compute,
                device: None,
                root,
                thumb,
                hud,
                score,
                bits,
                heap,
                increment,
                thumbnail,
                slots,
                hud_buffer: None,
                primed: false,
                taken_at: 0,
                mask_read: None,
                mask_readback: None,
                retired: Vec::new(),
                last: 0,
            })
        }
    }

    /// The passes on the compute queue of the GPU `image` was captured on.
    pub fn for_image(image: &GpuImage) -> Result<Self> {
        let mut passes = Self::new(Compute::for_device(&image.gpu.device)?)?;
        passes.device = Some(image.gpu.device.clone());
        Ok(passes)
    }

    /// Whether `image` is on the capture device these passes were made for.
    pub fn accepts(&self, image: &GpuImage) -> bool {
        self.device
            .as_ref()
            .is_some_and(|device| *device == image.gpu.device)
    }

    /// The queue the passes run on.
    pub fn compute(&self) -> &Arc<Compute> {
        &self.compute
    }

    /// Whether the passes can read `image` without a CPU wait: its texture
    /// can be opened here, its copy finishes on this device, and its pool
    /// takes this queue's fence as the read.
    pub fn can_read(&self, image: &GpuImage) -> bool {
        let device = self.compute.device.as_raw() as usize;
        shareable(&image.texture)
            && image.ready.as_ref().is_none_or(|ready| {
                ready.device == device
                    && (ready.readers.is_none()
                        || ready.reader_fence.as_ref() == Some(&self.compute.fence))
            })
    }

    /// Run `pass` over `image` once its capture copy is done; its thumbnail
    /// comes back from [`FrameWarpGpu::poll`] with `at`. `Ok(None)` when the
    /// capture is skipped: it cannot be read without a CPU wait
    /// ([`FrameWarpGpu::can_read`]) or every slot is still busy. Otherwise
    /// whether a mask copy started.
    pub fn submit(&mut self, image: &GpuImage, pass: &Pass, at: u64) -> Result<Option<bool>> {
        pass.check()?;
        if !self.can_read(image) {
            return Ok(None);
        }
        self.compute.ensure_active()?;
        self.release_retired();
        let compute = self.compute.clone();
        let Some(index) = (0..SLOTS).find(|&slot| {
            let slot = &self.slots[slot];
            slot.thumbnail.is_none() && (slot.fence == 0 || compute.completed(slot.fence))
        }) else {
            return Ok(None);
        };
        let source = (image.width, image.height);
        anyhow::ensure!(
            source.0 > 0 && source.1 > 0,
            "Frame Warp over an empty capture"
        );
        let opened = compute.open(&image.texture)?;
        // Too soon after the capture the codes are from: no HUD pass, and the
        // next one still compares with that capture.
        let held = self.primed
            && pass
                .hud
                .is_some_and(|hud| at < self.taken_at.saturating_add(hud.every_us));
        let pass = &Pass {
            hud: pass.hud.filter(|_| !held),
            ..*pass
        };
        let mode = match pass.hud {
            Some(hud) => Some(self.prepare_hud(source, hud.grid)?),
            None => None,
        };
        let read = pass.hud.is_some_and(|hud| hud.read) && self.mask_read.is_none();
        if read {
            self.prepare_mask_readback()?;
        }
        let hud = mode.zip(self.hud_buffer.as_ref());
        let mask_readback = self
            .mask_readback
            .as_ref()
            .filter(|_| read && hud.is_some())
            .map(|readback| &readback.buffer);
        let reading = mask_readback.is_some();
        let values = constants(
            source,
            image.pixel,
            pass,
            mode.unwrap_or(CLEAR),
            hud.map(|(_, hud)| &hud.layout),
        );
        let (thumb_width, thumb_height) = pass.thumbnail;
        let thumb_bytes = u64::from(thumb_width) * u64::from(thumb_height);
        let slot = &self.slots[index];
        // SAFETY: The slot's last list has run (checked above), so its view, allocator and list
        // may be reused; every view, barrier and copy names a live resource on this device, the
        // copies stay inside both buffers, and the words go back to COMMON at the end.
        unsafe {
            let cpu = self.heap.GetCPUDescriptorHandleForHeapStart();
            let gpu = self.heap.GetGPUDescriptorHandleForHeapStart();
            compute.device.CreateShaderResourceView(
                &opened,
                Some(&texture_srv(format(image.pixel))),
                D3D12_CPU_DESCRIPTOR_HANDLE {
                    ptr: cpu.ptr + index * self.increment as usize,
                },
            );
            slot.allocator.Reset()?;
            slot.list.Reset(&slot.allocator, None)?;
            let list = &slot.list;
            list.SetComputeRootSignature(&self.root);
            list.SetDescriptorHeaps(&[Some(self.heap.clone())]);
            list.SetComputeRoot32BitConstants(0, CONSTANTS, values.as_ptr().cast(), 0);
            list.SetComputeRootDescriptorTable(
                1,
                D3D12_GPU_DESCRIPTOR_HANDLE {
                    ptr: gpu.ptr + (index * self.increment as usize) as u64,
                },
            );
            // One call per batch; then the references the transitions hold go.
            let barriers = |batch: &mut [D3D12_RESOURCE_BARRIER]| {
                list.ResourceBarrier(batch);
                for barrier in batch.iter_mut() {
                    if barrier.Type == D3D12_RESOURCE_BARRIER_TYPE_TRANSITION {
                        drop_barriers(std::slice::from_mut(barrier));
                    }
                }
            };
            let thumbnail = |before, after| transition(&self.thumbnail, before, after);
            // The thumbnail and HUD passes write different buffers: no barrier between them.
            match hud {
                Some((_, hud)) => barriers(&mut [
                    thumbnail(
                        D3D12_RESOURCE_STATE_COMMON,
                        D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                    ),
                    transition(
                        &hud.buffer,
                        D3D12_RESOURCE_STATE_COMMON,
                        D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                    ),
                ]),
                None => barriers(&mut [thumbnail(
                    D3D12_RESOURCE_STATE_COMMON,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                )]),
            }
            list.SetComputeRootUnorderedAccessView(2, self.thumbnail.GetGPUVirtualAddress());
            list.SetPipelineState(&self.thumb);
            list.Dispatch((thumb_width / 4).div_ceil(8), thumb_height.div_ceil(8), 1);
            let thumbnail_copy = thumbnail(
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
            );
            if let Some((_, hud)) = hud {
                list.SetComputeRootUnorderedAccessView(2, hud.buffer.GetGPUVirtualAddress());
                list.SetPipelineState(&self.hud);
                list.Dispatch(source.0.div_ceil(16), source.1.div_ceil(16), 1);
                // The score pass reads the HUD pass's counts.
                barriers(&mut [thumbnail_copy, uav_barrier()]);
            } else {
                barriers(&mut [thumbnail_copy]);
            }
            list.CopyBufferRegion(&slot.readback, 0, &self.thumbnail, 0, thumb_bytes);
            let thumbnail_done = thumbnail(
                D3D12_RESOURCE_STATE_COPY_SOURCE,
                D3D12_RESOURCE_STATE_COMMON,
            );
            match hud {
                Some((_, hud)) => {
                    list.SetPipelineState(&self.score);
                    list.Dispatch(hud.grid.0.div_ceil(8), hud.grid.1.div_ceil(8), 1);
                    if let Some(readback) = mask_readback {
                        barriers(&mut [uav_barrier()]);
                        list.SetPipelineState(&self.bits);
                        list.Dispatch(hud.layout.stride.div_ceil(8), hud.grid.1.div_ceil(8), 1);
                        barriers(&mut [transition(
                            &hud.buffer,
                            D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                            D3D12_RESOURCE_STATE_COPY_SOURCE,
                        )]);
                        list.CopyBufferRegion(
                            readback,
                            0,
                            &hud.buffer,
                            u64::from(hud.layout.bits) * 4,
                            u64::from(hud.layout.stride) * u64::from(hud.grid.1) * 4,
                        );
                        barriers(&mut [
                            thumbnail_done,
                            transition(
                                &hud.buffer,
                                D3D12_RESOURCE_STATE_COPY_SOURCE,
                                D3D12_RESOURCE_STATE_COMMON,
                            ),
                        ]);
                    } else {
                        barriers(&mut [
                            thumbnail_done,
                            transition(
                                &hud.buffer,
                                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                                D3D12_RESOURCE_STATE_COMMON,
                            ),
                        ]);
                    }
                }
                None => barriers(&mut [thumbnail_done]),
            }
            list.Close()?;
        }
        let grid = hud.map(|(_, hud)| (hud.grid, hud.layout.stride));
        let ready = image.ready.as_ref();
        if let Some(ready) = ready {
            compute.wait_ready(ready)?;
        }
        // Keep the capture image and imported resource retained in the slot before executing so
        // if `ExecuteCommandLists` ran and `Signal` fails (retiring `compute`), `Drop` still
        // holds the capture alive rather than releasing it under submitted GPU work.
        self.slots[index].capture = Some((image.clone(), opened));
        let fence = compute.execute(&self.slots[index].list)?;
        self.last = fence;
        let slot = &mut self.slots[index];
        slot.fence = fence;
        slot.thumbnail = Some((at, pass.thumbnail));
        if mode.is_some()
            && let Some(hud) = &mut self.hud_buffer
        {
            hud.initialized = true;
        }
        self.primed = pass.hud.is_some() || held;
        if pass.hud.is_some() {
            self.taken_at = at;
        }
        let started = match (reading, grid) {
            (true, Some((grid, stride))) => {
                self.mask_read = Some(MaskRead {
                    fence,
                    grid,
                    stride,
                });
                true
            }
            _ => false,
        };
        // The capture's pool must not overwrite the texture before the passes have read it.
        if let Some(ready) = ready {
            ready.read_by(&compute, fence)?;
        }
        Ok(Some(started))
    }

    /// The HUD buffer for `source` and `grid`, and the score pass's mode: a
    /// new size starts over, and the first pass after the camera stood still
    /// only takes the capture's codes.
    fn prepare_hud(&mut self, source: (u32, u32), grid: (u32, u32)) -> Result<u32> {
        if let Some(hud) = self.hud_buffer.as_ref()
            && hud.source == source
            && hud.grid == grid
        {
            return Ok(if !hud.initialized {
                RESET
            } else if self.primed {
                SCORE
            } else {
                CLEAR
            });
        }
        let layout = layout(source, grid)?;
        let buffer = focus_buffer(
            &self.compute.device,
            D3D12_HEAP_TYPE_DEFAULT,
            layout.words * 4,
            D3D12_RESOURCE_STATE_COMMON,
        )?;
        if let Some(old) = self.hud_buffer.replace(HudBuffer {
            buffer,
            source,
            grid,
            layout,
            initialized: false,
        }) {
            self.retired.push((self.last, old.buffer));
        }
        Ok(RESET)
    }

    /// The mapped READBACK buffer for the current HUD grid's mask bits, made
    /// once per grid size. Called only while no mask copy is in flight, so
    /// the buffer it replaces has had its last copy read.
    fn prepare_mask_readback(&mut self) -> Result<()> {
        let Some(hud) = &self.hud_buffer else {
            return Ok(());
        };
        let words = hud.layout.stride as usize * hud.grid.1 as usize;
        if self
            .mask_readback
            .as_ref()
            .is_some_and(|readback| readback.words == words)
        {
            return Ok(());
        }
        let buffer = focus_buffer(
            &self.compute.device,
            D3D12_HEAP_TYPE_READBACK,
            words as u64 * 4,
            D3D12_RESOURCE_STATE_COPY_DEST,
        )?;
        let mut mapped = std::ptr::null_mut();
        // SAFETY: A READBACK buffer may stay mapped for its life; Drop, or its replacement
        // here, unmaps it.
        unsafe { buffer.Map(0, None, Some(&mut mapped))? };
        let readback = MaskReadback {
            buffer,
            mapped: mapped.cast_const().cast::<u32>(),
            words,
        };
        if let Some(old) = self.mask_readback.replace(readback) {
            // SAFETY: Nothing reads the old mapping again; Unmap ends CPU access only.
            unsafe { old.buffer.Unmap(0, None) };
            self.retired.push((self.last, old.buffer));
        }
        Ok(())
    }

    fn release_retired(&mut self) {
        let compute = &self.compute;
        self.retired.retain(|(fence, _)| !compute.completed(*fence));
    }

    /// The thumbnails whose passes finished, oldest first, and the mask
    /// copy once it finished. Only asks the fence: never waits.
    pub fn poll(&mut self) -> Result<(Vec<ThumbnailReadout>, Option<MaskReadout>)> {
        let mut finished = Vec::new();
        for slot in &mut self.slots {
            if slot.fence == 0 || !self.compute.completed(slot.fence) {
                continue;
            }
            slot.capture = None;
            if let Some((at, (width, height))) = slot.thumbnail.take() {
                let (width, height) = (width as usize, height as usize);
                // SAFETY: The slot's copy has completed and the next one needs `&mut self`
                // (`submit`), so the mapped bytes stay unchanged while they are read.
                let luma =
                    unsafe { std::slice::from_raw_parts(slot.mapped, width * height) }.to_vec();
                finished.push((
                    slot.fence,
                    ThumbnailReadout {
                        at,
                        width,
                        height,
                        luma,
                    },
                ));
            }
        }
        finished.sort_by_key(|(fence, _)| *fence);
        let thumbnails = finished
            .into_iter()
            .map(|(_, thumbnail)| thumbnail)
            .collect();
        let mut mask = None;
        if let Some(read) = self
            .mask_read
            .take_if(|read| self.compute.completed(read.fence))
        {
            let count = read.stride as usize * read.grid.1 as usize;
            let readback = self
                .mask_readback
                .as_ref()
                .filter(|readback| count <= readback.words)
                .context("no Frame Warp mask readback for the copy")?;
            // SAFETY: The copy into the buffer has completed, the next one needs `&mut self`
            // (`submit`), and the slice stays inside the mapped words.
            let words = unsafe { std::slice::from_raw_parts(readback.mapped, count) }.to_vec();
            mask = Some(MaskReadout {
                width: read.grid.0,
                height: read.grid.1,
                words,
            });
        }
        self.release_retired();
        Ok((thumbnails, mask))
    }
}

impl Drop for FrameWarpGpu {
    fn drop(&mut self) {
        let mut work = RetainedWork::new(&self.compute, &self.compute.queue);
        work.keep(&self.root);
        work.keep(&self.thumb);
        work.keep(&self.hud);
        work.keep(&self.score);
        work.keep(&self.bits);
        work.keep(&self.heap);
        work.keep(&self.thumbnail);
        if let Some(hud) = &self.hud_buffer {
            work.keep(&hud.buffer);
        }
        if let Some(readback) = &self.mask_readback {
            work.keep(&readback.buffer);
        }
        for (_, buffer) in &self.retired {
            work.keep(buffer);
        }
        for slot in &mut self.slots {
            work.keep(&slot.allocator);
            work.keep(&slot.list);
            work.keep(&slot.readback);
            if let Some((capture, opened)) = slot.capture.take() {
                work.keep(&opened);
                work.captures.push(capture);
            }
        }
        work.finish(&self.compute);
        for slot in &self.slots {
            // SAFETY: Each buffer was mapped once in `new`; nothing reads it after this. Unmap
            // ends CPU access only; retained GPU work still owns the resource.
            unsafe { slot.readback.Unmap(0, None) };
        }
        if let Some(readback) = &self.mask_readback {
            // SAFETY: Mapped once in `prepare_mask_readback`; as above.
            unsafe { readback.buffer.Unmap(0, None) };
        }
    }
}

#[cfg(test)]
#[path = "compute_frame_warp_tests.rs"]
mod tests;
