//! Capture copies and colour conversion on D3D12 compute queues.
//!
//! D3D11 work waits behind whatever a game has queued on the GPU's graphics
//! engine: beside a game using the whole GPU, the D3D11 colour conversion
//! took 7 ms instead of 0.9 ms and a plain copy 8 ms instead of 0.7 ms. A
//! compute queue runs next to the graphics engine and finished the same
//! work in 0.2-0.3 ms under that load (`examples/d3d12_probe.rs`,
//! `examples/gpu_load.rs`). Captured frames are copied into shared textures
//! on one compute queue ([`Handoff`]) and converted to the encoder's YUV
//! format on another ([`Converter`]); the encoder waits for the conversion's
//! fence on the GPU.

use anyhow::{Context, Result, bail};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
use windows::{
    Win32::{
        Foundation::{CloseHandle, GENERIC_ALL, HANDLE, LUID},
        Graphics::{
            Direct3D::D3D_FEATURE_LEVEL_11_0,
            Direct3D11::{ID3D11Device5, ID3D11DeviceContext4, ID3D11Fence, ID3D11Texture2D},
            Direct3D12::*,
            Dxgi::{Common::*, *},
        },
    },
    core::{Interface, PCWSTR},
};

use butterpollo_win_abi::shaders::shader_bytecode;
mod handoff;
mod retained;
use handoff::wait_fence;
pub use handoff::*;
#[cfg(test)]
use retained::RETAINED;
pub(crate) use retained::RetainedWork;
use retained::allow_compute_generation;
mod convert;
mod device;
mod focus;
mod pipeline;

use device::*;
pub use device::{Compute, enabled, shareable};
pub(crate) use focus::focus_buffer;
use focus::*;
pub use focus::{FocusSettings, MapView};
pub use pipeline::Converted;
use pipeline::*;
pub(crate) use pipeline::{OutputLayout, format};

/// RGB to NV12, P010 or 8-bit AYUV on the compute queue, with the same shader
/// as the D3D11 converter (scaling, letterboxing, HDR and the colour matrix).
pub struct Converter {
    compute: Arc<Compute>,
    /// Luma and chroma in one pass (`yuv420_cs`, `chroma_pack_cs`, or
    /// `yuv444_ayuv_cs`). `None` on a planar-only converter until `convert` or
    /// `pack_chroma` needs it.
    yuv: Option<(ID3D12RootSignature, ID3D12PipelineState)>,
    /// PyroWave's three planes (`pyro_cs`), made at creation by
    /// [`Self::new_planar`] or when [`Self::convert_planes`] is first used.
    planar: Option<(ID3D12RootSignature, ID3D12PipelineState)>,
    heap: ID3D12DescriptorHeap,
    increment: u32,
    constants: ID3D12Resource,
    mapped: *mut u8,
    slots: Vec<Slot>,
    slot: usize,
    pub values: [u32; 20],
    width: u32,
    height: u32,
    layout: OutputLayout,
    targets: Vec<Target>,
    /// Bounded cache of immutable pointer textures for recurring animation shapes.
    pointers: crate::cursor::ShapeCache<ID3D12Resource>,
    /// Bumped whenever a new pointer shape is inserted (which may evict an older one).
    pointer_epoch: u64,
    uploads: Vec<PointerUpload>,
    /// Encoder focus, once [`Converter::enable_focus`] made it.
    focus: Option<Box<Focus>>,
    /// `pack_chroma` replaced `convert`, which focus cannot follow.
    packs_chroma: bool,
    /// The couch menu's pass, while an overlay is set.
    menu: menu::Menu,
    /// HDR statistics, once [`Converter::enable_hdr_stats`] made them.
    hdr_stats: Option<Box<hdr_stats::HdrStats>>,
}
// SAFETY: `mapped` and the focus and HDR statistics readback pointers point into buffers this
// converter owns, the D3D12 objects are free-threaded, and the command allocators and lists are
// used only through `&mut self`.
unsafe impl Send for Converter {}
const SLOTS: usize = 4;
/// Descriptors per slot: the source and pointer, up to three planar outputs,
/// then the couch menu's panel image ([`menu::VIEW`]).
const VIEWS: usize = 6;
/// Maximum pooled YUV targets; each target owns 2 permanent UAV descriptors
/// after the slot region so coprime target counts (e.g. 3 targets vs 4 slots)
/// never recreate target UAVs.
const MAX_TARGETS: usize = 8;
const TARGET_UAV_BASE: usize = SLOTS * VIEWS;

impl Converter {
    pub(crate) fn pack_chroma(&mut self) -> Result<()> {
        anyhow::ensure!(
            self.layout != OutputLayout::Ayuv,
            "packed 4:4:4 does not use AYUV targets"
        );
        anyhow::ensure!(
            self.focus.is_none(),
            "encoder focus does not support packed 4:4:4"
        );
        let root = match &self.yuv {
            Some((root, _)) => root.clone(),
            None => root_signature(&self.compute.device, 0, 2)?,
        };
        let convert = pipeline(&self.compute.device, &root, b"chroma_pack_cs\0")?;
        self.yuv = Some((root, convert));
        self.packs_chroma = true;
        Ok(())
    }
    /// Probe view formats and the actual target allocation before AMF or NVENC
    /// initializes its D3D12 context, so unsupported devices fall back once.
    /// PyroWave uses separate R8/R16 planes and does not need this YUV probe.
    pub(crate) fn prepare_yuv(&mut self) -> Result<()> {
        self.compute.ensure_active()?;
        require_format_support(
            &self.compute.device,
            self.layout.texture_format(),
            D3D12_FORMAT_SUPPORT1_TEXTURE2D,
            D3D12_FORMAT_SUPPORT2_NONE,
        )?;
        for &plane in self.layout.uav_formats() {
            require_format_support(
                &self.compute.device,
                plane,
                D3D12_FORMAT_SUPPORT1_TYPED_UNORDERED_ACCESS_VIEW,
                D3D12_FORMAT_SUPPORT2_UAV_TYPED_STORE,
            )?;
        }
        let _ = self.target()?;
        if self.layout == OutputLayout::Ayuv {
            // `target()` created the R8G8B8A8_UINT UAV on DXGI_FORMAT_AYUV;
            // confirm the driver accepted it before NVENC opens its D3D12
            // session (Rubylight #150).
            if let Some(loss) = crate::device_loss::DeviceLost::d3d12(&self.compute.device) {
                self.compute.retire();
                return Err(anyhow::Error::new(loss)
                    .context("D3D12 device rejected R8G8B8A8_UINT UAV on DXGI_FORMAT_AYUV"));
            }
        }
        Ok(())
    }
    pub fn new(compute: Arc<Compute>, width: u32, height: u32, ten_bit: bool) -> Result<Self> {
        Self::new_with_pipeline(
            compute,
            width,
            height,
            OutputLayout::for_420(ten_bit),
            InitialPipeline::Yuv,
        )
    }
    fn new_with_pipeline(
        compute: Arc<Compute>,
        width: u32,
        height: u32,
        layout: OutputLayout,
        initial: InitialPipeline,
    ) -> Result<Self> {
        compute.ensure_active()?;
        if width == 0 || height == 0 || !width.is_multiple_of(2) || !height.is_multiple_of(2) {
            bail!("compute conversion needs even, nonzero dimensions");
        }
        // SAFETY: Every call is on the live `compute.device` with descriptions that outlive it, and
        // `constants` (SLOTS * 512 bytes) stays mapped at `mapped` until Unmap in Drop.
        unsafe {
            let device = &compute.device;
            let (first_uav, uav_count) = initial.uav_range();
            let root = root_signature(device, first_uav, uav_count)?;
            let pso = pipeline(device, &root, initial.shader_entry())?;
            let (yuv, planar) = match initial {
                InitialPipeline::Yuv | InitialPipeline::Ayuv => (Some((root, pso)), None),
                InitialPipeline::Planar => (None, Some((root, pso))),
            };
            let heap: ID3D12DescriptorHeap =
                device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                    Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                    NumDescriptors: (TARGET_UAV_BASE + MAX_TARGETS * 2) as u32,
                    Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                    NodeMask: 0,
                })?;
            let increment =
                device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV);
            let mut constants: Option<ID3D12Resource> = None;
            device.CreateCommittedResource(
                &D3D12_HEAP_PROPERTIES {
                    Type: D3D12_HEAP_TYPE_UPLOAD,
                    ..Default::default()
                },
                D3D12_HEAP_FLAG_NONE,
                // `Config` per slot, then `cbuffer Menu` per slot.
                &buffer_desc((SLOTS * 512) as u64),
                D3D12_RESOURCE_STATE_GENERIC_READ,
                None,
                &mut constants,
            )?;
            let constants = constants.context("no constant buffer")?;
            let mut mapped = std::ptr::null_mut();
            constants.Map(0, None, Some(&mut mapped))?;
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
                slots.push(Slot {
                    allocator,
                    list,
                    fence: 0,
                    inputs: [None, None, None],
                    capture: None,
                    outputs: Vec::new(),
                    waits: Vec::new(),
                    stats: 0,
                    source_srv: None,
                    pointer_srv: None,
                    planar_view: None,
                });
            }
            Ok(Self {
                compute,
                yuv,
                planar,
                heap,
                increment,
                constants,
                mapped: mapped.cast(),
                slots,
                slot: 0,
                values: [0; 20],
                width,
                height,
                layout,
                targets: vec![],
                pointers: Default::default(),
                pointer_epoch: 0,
                uploads: Vec::new(),
                focus: None,
                packs_chroma: false,
                menu: Default::default(),
                hdr_stats: None,
            })
        }
    }
    pub fn compute(&self) -> &Arc<Compute> {
        &self.compute
    }
    /// Wait for submitted compute conversions, stats and focus passes on this
    /// converter's queue to finish on the GPU. Used during encoder teardown.
    pub(crate) fn wait_idle(&self) {
        self.compute.wait_idle();
    }
    #[cfg(test)]
    pub(crate) fn test_amf_target_states(&self) -> Vec<Option<u32>> {
        self.targets
            .iter()
            .map(|target| {
                crate::amf::gpu::resource_state(&target.texture)
                    .ok()
                    .map(|state| state.0 as u32)
            })
            .collect()
    }
    pub(crate) fn release_completed(&mut self) {
        self.uploads
            .retain(|upload| !self.compute.completed(upload.fence));
        for slot in &mut self.slots {
            // A queued statistics pass still reads the capture once submitted.
            if slot.stats != hdr_stats::QUEUED && self.compute.completed(slot.last()) {
                slot.capture = None;
                slot.inputs = [None, None, None];
                // Cached planar descriptors use resource addresses as their key.
                // Keep their resources alive until those descriptors are replaced,
                // so a new allocation cannot reuse an address still in the cache.
                if slot.planar_view.is_none() {
                    slot.outputs.clear();
                }
                slot.waits.clear();
            }
        }
    }
    pub(crate) fn log_stall(&self) {
        // SAFETY: These live devices and fences are only queried, never waited on.
        let (completed, removed) = unsafe {
            (
                self.compute.fence.GetCompletedValue(),
                self.compute.device.GetDeviceRemovedReason(),
            )
        };
        tracing::warn!(
            priority = self.compute.priority,
            conversion_submitted = self.slots.iter().map(|slot| slot.fence).max().unwrap_or(0),
            conversion_completed = completed,
            d3d12_removed = ?removed,
            "compute conversion progress at stall"
        );
        for (index, target) in self.targets.iter().enumerate() {
            // SAFETY: `target` owns this live fence; the query does not wait.
            let completed = unsafe { target.fence.GetCompletedValue() };
            tracing::warn!(
                index,
                conversion_value = target.value,
                amf_value = ?crate::amf::gpu::fence_value(&target.fence),
                completed,
                owners = Arc::strong_count(&target.texture),
                "conversion surface fence at stall"
            );
        }
    }
}
impl Drop for Converter {
    fn drop(&mut self) {
        let mut work = RetainedWork::new(&self.compute, &self.compute.queue);
        if let Some((root, pipeline)) = &self.yuv {
            work.keep(root);
            work.keep(pipeline);
        }
        if let Some((root, pipeline)) = &self.planar {
            work.keep(root);
            work.keep(pipeline);
        }
        work.keep(&self.heap);
        work.keep(&self.constants);
        for pointer in self.pointers.values() {
            work.keep(pointer);
        }
        for slot in &mut self.slots {
            work.keep(&slot.allocator);
            work.keep(&slot.list);
            for resource in slot.inputs.iter().flatten().chain(&slot.outputs) {
                work.keep(resource);
            }
            for fence in &slot.waits {
                work.keep(fence);
            }
            if let Some(capture) = slot.capture.take() {
                work.captures.push(capture);
            }
        }
        for upload in &self.uploads {
            work.keep(&upload._allocator);
            work.keep(&upload._list);
            work.keep(&upload._upload);
            work.keep(&upload._texture);
        }
        for target in &self.targets {
            work.keep(target.texture.as_ref());
            work.waits.push((target.fence.clone(), target.last()));
        }
        if let Some(focus) = &self.focus {
            focus.retain(&mut work);
        }
        self.menu.retain(&mut work);
        if let Some(stats) = &self.hdr_stats {
            stats.retain(&mut work);
        }
        work.finish(&self.compute);
        // SAFETY: `constants` was mapped once in `new`, and nothing writes through `mapped` after
        // this. Unmap ends CPU access; retained GPU work still owns the resource.
        unsafe { self.constants.Unmap(0, None) };
        if let Some(focus) = &self.focus {
            focus.unmap();
        }
        if let Some(stats) = &self.hdr_stats {
            stats.unmap();
        }
    }
}
#[cfg(test)]
#[path = "compute_focus_tests.rs"]
mod focus_tests;
#[path = "compute_frame_warp.rs"]
pub mod frame_warp;
#[path = "compute_hdr_stats.rs"]
pub(crate) mod hdr_stats;
#[path = "compute_menu.rs"]
mod menu;
mod planar;
pub use hdr_stats::{HDR_STATS_BINS, HdrStatsReadout};
use planar::InitialPipeline;
pub use planar::Planes;
#[cfg(test)]
mod ayuv_tests;
#[cfg(test)]
mod tests;
