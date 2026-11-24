//! D3D12 compute conversion into PyroWave's three separate Y, Cb and Cr
//! planes, compiling only `pyro_cs` upfront instead of the unused `yuv420_cs`
//! pipeline.

use super::*;

/// Which conversion pipeline [`Converter`] compiles upfront.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum InitialPipeline {
    /// NV12/P010 (or packed 4:4:4 after [`Converter::pack_chroma`]): `yuv420_cs` on `u0..u2`.
    Yuv,
    /// 8-bit native 4:4:4 AYUV: `yuv444_ayuv_cs` on `u0` without compiling `yuv420_cs`.
    Ayuv,
    /// PyroWave's three separate planes: `pyro_cs` on `u2..u5` without compiling `yuv420_cs`.
    Planar,
}

impl InitialPipeline {
    pub(super) const fn shader_entry(self) -> &'static [u8] {
        match self {
            Self::Yuv => b"yuv420_cs\0",
            Self::Ayuv => b"yuv444_ayuv_cs\0",
            Self::Planar => b"pyro_cs\0",
        }
    }

    pub(super) const fn uav_range(self) -> (u32, u32) {
        match self {
            Self::Yuv => (0, 2),
            Self::Ayuv => (0, 1),
            Self::Planar => (2, 3),
        }
    }
}

/// PyroWave's planes on the compute device and the fence the encoder's
/// Vulkan queue shares, for [`Converter::convert_planes`].
pub struct Planes {
    pub planes: [ID3D12Resource; 3],
    /// `DXGI_FORMAT_R16_UNORM` for ten-bit streams, else `R8_UNORM`.
    pub format: DXGI_FORMAT,
    pub fence: ID3D12Fence,
    pub after: u64,
    pub done: u64,
}

impl Converter {
    /// A compute converter for PyroWave's three separate planes: compiles only
    /// `pyro_cs` upfront instead of compiling the unused `yuv420_cs` pipeline
    /// and deferring `pyro_cs` to the first frame.
    pub fn new_planar(
        compute: Arc<Compute>,
        width: u32,
        height: u32,
        ten_bit: bool,
    ) -> Result<Self> {
        Self::new_with_pipeline(
            compute,
            width,
            height,
            OutputLayout::for_420(ten_bit),
            InitialPipeline::Planar,
        )
    }

    /// A compute converter for 8-bit native 4:4:4 (`DXGI_FORMAT_AYUV`, written
    /// through an `R8G8B8A8_UINT` UAV): compiles only `yuv444_ayuv_cs` upfront.
    pub fn new_ayuv(compute: Arc<Compute>, width: u32, height: u32) -> Result<Self> {
        Self::new_with_pipeline(
            compute,
            width,
            height,
            OutputLayout::Ayuv,
            InitialPipeline::Ayuv,
        )
    }

    /// Convert `source` into PyroWave's Y, Cb and Cr planes, full size or
    /// with chroma halved (`values[11]`), in textures the encoder shares
    /// with Vulkan. The queue first waits for `target.fence` to reach
    /// `target.after`, when the encoder is done with the planes, then
    /// signals `target.done`.
    pub fn convert_planes(
        &mut self,
        source: &ID3D12Resource,
        format: DXGI_FORMAT,
        pointer: Option<&ID3D12Resource>,
        ready: Option<&Ready>,
        target: &Planes,
        image: Option<&crate::capture::GpuImage>,
    ) -> Result<()> {
        self.compute.ensure_active()?;
        self.release_completed();
        validate_colour_input(&self.values)?;
        let (root, planar) = match &self.planar {
            Some(planar) => planar.clone(),
            None => {
                let root = root_signature(&self.compute.device, 2, 3)?;
                let planar = pipeline(&self.compute.device, &root, b"pyro_cs\0")?;
                self.planar = Some((root.clone(), planar.clone()));
                (root, planar)
            }
        };
        let slot = self.slot;
        self.slot = (self.slot + 1) % SLOTS;
        self.compute.wait(self.slots[slot].last())?;
        self.slots[slot].capture = None;
        self.reset_hdr_stats(slot);
        let menu = self.menu_pass(slot, b"pyro_menu_cs\0", 2, 3, [2, 2])?;
        self.slots[slot].inputs = [
            Some(source.clone()),
            pointer.cloned(),
            menu.as_ref().map(|menu| menu.texture.clone()),
        ];
        self.slots[slot].capture = image.cloned();
        self.slots[slot].outputs = target.planes.to_vec();
        self.slots[slot].waits = ready
            .map(|ready| ready.fence.clone())
            .into_iter()
            .chain(std::iter::once(target.fence.clone()))
            .collect();
        // The planes carry no map, so the slot's old one must not look like theirs.
        if let Some(focus) = self.focus.as_deref_mut() {
            focus.written[slot] = false;
        }
        // `ComputePlanes` passes the encoder's same three planes every frame, so
        // their UAVs are only remade when the planes or their format change. The
        // slot's `outputs` keep them alive while their views are cached.
        let planar_key = (
            target
                .planes
                .each_ref()
                .map(|plane| plane.as_raw() as usize),
            target.format.0,
        );
        // SAFETY: As in `convert`, the slot is idle; `target`'s planes are on `compute.device`.
        unsafe {
            self.write_values(slot);
            self.update_input_srvs(slot, source, format, pointer);
            let device = &self.compute.device;
            let cpu = self.heap.GetCPUDescriptorHandleForHeapStart();
            let gpu = self.heap.GetGPUDescriptorHandleForHeapStart();
            let at = |index: usize| D3D12_CPU_DESCRIPTOR_HANDLE {
                ptr: cpu.ptr + (slot * VIEWS + index) * self.increment as usize,
            };
            if self.slots[slot].planar_view != Some(planar_key) {
                for (index, plane) in target.planes.iter().enumerate() {
                    device.CreateUnorderedAccessView(
                        plane,
                        None,
                        Some(&texture_uav(target.format, 0)),
                        at(2 + index),
                    );
                }
                self.slots[slot].planar_view = Some(planar_key);
            }
            let Slot {
                allocator, list, ..
            } = &self.slots[slot];
            allocator.Reset()?;
            list.Reset(allocator, &planar)?;
            if let Some(menu) = &menu {
                menu.copy(list);
            }
            let mut barriers = target.planes.each_ref().map(|plane| {
                transition(
                    plane,
                    D3D12_RESOURCE_STATE_COMMON,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                )
            });
            list.ResourceBarrier(&barriers);
            drop_barriers(&mut barriers);
            list.SetComputeRootSignature(&root);
            list.SetDescriptorHeaps(&[Some(self.heap.clone())]);
            list.SetComputeRootConstantBufferView(
                0,
                self.constants.GetGPUVirtualAddress() + (slot * 256) as u64,
            );
            let table = |index: usize| D3D12_GPU_DESCRIPTOR_HANDLE {
                ptr: gpu.ptr + ((slot * VIEWS + index) * self.increment as usize) as u64,
            };
            list.SetComputeRootDescriptorTable(1, table(0));
            list.SetComputeRootDescriptorTable(2, table(2));
            list.Dispatch(
                (self.width / 2).div_ceil(8),
                (self.height / 2).div_ceil(8),
                1,
            );
            if let Some(menu) = &menu {
                menu.record(list, self.constants.GetGPUVirtualAddress(), slot, table);
            }
            let mut barriers = target.planes.each_ref().map(|plane| {
                transition(
                    plane,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                    D3D12_RESOURCE_STATE_COMMON,
                )
            });
            list.ResourceBarrier(&barriers);
            drop_barriers(&mut barriers);
            list.Close()?;
        }
        let ordered = self
            .compute
            .ensure_active()
            .and_then(|()| self.wait_capture(ready))
            .and_then(|()| {
                // SAFETY: `target.fence` is opened on this queue's device and outlives the call.
                unsafe { self.compute.queue.Wait(&target.fence, target.after) }
                    .inspect_err(|_| self.compute.retire())
                    .map_err(Into::into)
            });
        let Slot { fence, list, .. } = &mut self.slots[slot];
        let (compute, overlay) = (&self.compute, &mut self.menu);
        submit_slot(
            fence,
            ordered,
            || overlay.submitting(menu.as_ref()),
            || compute.execute(list).inspect_err(|_| compute.retire()),
        )?;
        if let Some(menu) = &menu {
            self.menu_submitted(menu, self.slots[slot].fence);
        }
        if let Some(ready) = ready {
            ready.read_by(&self.compute, self.slots[slot].fence)?;
        }
        // SAFETY: `target.fence` is opened on this queue's device and outlives the call.
        unsafe { self.compute.queue.Signal(&target.fence, target.done) }
            .inspect_err(|_| self.compute.retire())?;
        Ok(())
    }
}
