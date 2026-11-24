//! D3D12 compute pointer upload, target allocation and frame conversion dispatch.

use super::*;

impl Converter {
    /// The pointer shape as a texture on this device, uploaded when it
    /// changes. The conversion follows the upload on the same queue.
    pub(crate) fn pointer(&mut self, cursor: &crate::cursor::Cursor) -> Result<ID3D12Resource> {
        self.compute.ensure_active()?;
        self.release_completed();
        if let Some(texture) =
            self.pointers
                .get(cursor.width, cursor.height, cursor.logic, &cursor.pixels)
        {
            return Ok(texture);
        }
        // SAFETY: `cursor.pixels` holds `width * height` RGBA pixels and `upload` `pitch * height`
        // bytes, so each row copy stays in both; `uploads` retains the copy's objects to its fence.
        unsafe {
            let device = &self.compute.device;
            let desc = D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                Width: u64::from(cursor.width),
                Height: cursor.height,
                DepthOrArraySize: 1,
                MipLevels: 1,
                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
                Flags: D3D12_RESOURCE_FLAG_NONE,
                Alignment: 0,
            };
            let mut texture: Option<ID3D12Resource> = None;
            device.CreateCommittedResource(
                &D3D12_HEAP_PROPERTIES {
                    Type: D3D12_HEAP_TYPE_DEFAULT,
                    ..Default::default()
                },
                D3D12_HEAP_FLAG_NONE,
                &desc,
                D3D12_RESOURCE_STATE_COMMON,
                None,
                &mut texture,
            )?;
            let texture = texture.context("no pointer texture")?;
            let pitch = (cursor.width * 4).div_ceil(256) * 256;
            let mut upload: Option<ID3D12Resource> = None;
            device.CreateCommittedResource(
                &D3D12_HEAP_PROPERTIES {
                    Type: D3D12_HEAP_TYPE_UPLOAD,
                    ..Default::default()
                },
                D3D12_HEAP_FLAG_NONE,
                &buffer_desc(u64::from(pitch * cursor.height)),
                D3D12_RESOURCE_STATE_GENERIC_READ,
                None,
                &mut upload,
            )?;
            let upload = upload.context("no pointer upload buffer")?;
            let mut mapped = std::ptr::null_mut();
            upload.Map(0, None, Some(&mut mapped))?;
            let row = (cursor.width * 4) as usize;
            for y in 0..cursor.height as usize {
                std::ptr::copy_nonoverlapping(
                    cursor.pixels.as_ptr().add(y * row),
                    mapped.cast::<u8>().add(y * pitch as usize),
                    row.min(cursor.pixels.len().saturating_sub(y * row)),
                );
            }
            upload.Unmap(0, None);
            let allocator: ID3D12CommandAllocator =
                device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_COMPUTE)?;
            let list: ID3D12GraphicsCommandList =
                device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_COMPUTE, &allocator, None)?;
            let destination = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::ManuallyDrop::new(Some(texture.clone())),
                Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                    SubresourceIndex: 0,
                },
            };
            let source = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::ManuallyDrop::new(Some(upload.clone())),
                Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                    PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                        Offset: 0,
                        Footprint: D3D12_SUBRESOURCE_FOOTPRINT {
                            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                            Width: cursor.width,
                            Height: cursor.height,
                            Depth: 1,
                            RowPitch: pitch,
                        },
                    },
                },
            };
            list.CopyTextureRegion(&destination, 0, 0, 0, &source, None);
            drop(std::mem::ManuallyDrop::into_inner(destination.pResource));
            drop(std::mem::ManuallyDrop::into_inner(source.pResource));
            // This plain texture was implicitly promoted to COPY_DEST on a compute
            // queue, so it does not decay to COMMON after the upload. Every later
            // use samples the cached cursor; transition once before its first read.
            let mut barriers = [transition(
                &texture,
                D3D12_RESOURCE_STATE_COPY_DEST,
                D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
            )];
            list.ResourceBarrier(&barriers);
            drop_barriers(&mut barriers);
            list.Close()?;
            self.uploads.push(PointerUpload {
                // Own every submitted object before ExecuteCommandLists: its
                // following Signal may fail after work has already been queued.
                fence: u64::MAX,
                _allocator: allocator,
                _list: list.clone(),
                _upload: upload,
                _texture: texture.clone(),
            });
            let done = self.compute.execute(&list)?;
            self.uploads.last_mut().unwrap().fence = done;
            self.pointers.insert(
                cursor.width,
                cursor.height,
                cursor.logic,
                cursor.pixels.clone(),
                texture.clone(),
            );
            self.pointer_epoch = self.pointer_epoch.wrapping_add(1);
            Ok(texture)
        }
    }
    /// Copy `values` into `slot`'s 256-byte part of the constant buffer.
    ///
    /// # Safety
    /// The slot's last conversion has finished, so the GPU no longer reads it.
    pub(super) unsafe fn write_values(&self, slot: usize) {
        // SAFETY: `mapped` stays mapped for the converter's life and holds `SLOTS` 256-byte
        // slots; the 80 bytes of `values` fit one, and the caller keeps the GPU off it.
        unsafe {
            std::ptr::copy_nonoverlapping(
                self.values.as_ptr().cast::<u8>(),
                self.mapped.add(slot * 256),
                std::mem::size_of_val(&self.values),
            );
        }
    }
    /// Order this queue after the capture's copy behind `ready`: on the GPU
    /// when its fence is on this queue's device, else on the CPU.
    pub(super) fn wait_capture(&self, ready: Option<&Ready>) -> Result<()> {
        if let Some(ready) = ready {
            self.compute.wait_ready(ready)?;
        }
        Ok(())
    }
    /// A free output texture's index.
    pub(super) fn target(&mut self) -> Result<usize> {
        for (index, target) in self.targets.iter().enumerate() {
            if target.free()? {
                return Ok(index);
            }
        }
        if self.targets.len() >= MAX_TARGETS {
            // AMF can release its surface reference just before its GPU
            // completion signal. Wait for such a target instead of declaring
            // the bounded pool exhausted and triggering keyframe recovery.
            if let Some((index, target)) = self
                .targets
                .iter()
                .enumerate()
                .find(|(_, target)| Arc::strong_count(&target.texture) == 1)
            {
                wait_fence(&target.fence, target.last()).inspect_err(|_| self.compute.retire())?;
                return Ok(index);
            }
            bail!("compute conversion queue reached its bounded limit");
        }
        // SAFETY: `compute.device` is a live D3D12 device and the description and out-parameter
        // outlive the call.
        unsafe {
            let mut texture: Option<ID3D12Resource> = None;
            self.compute
                .device
                .CreateCommittedResource(
                    &D3D12_HEAP_PROPERTIES {
                        Type: D3D12_HEAP_TYPE_DEFAULT,
                        ..Default::default()
                    },
                    D3D12_HEAP_FLAG_NONE,
                    &D3D12_RESOURCE_DESC {
                        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                        Width: u64::from(self.width),
                        Height: self.height,
                        DepthOrArraySize: 1,
                        MipLevels: 1,
                        Format: self.layout.texture_format(),
                        SampleDesc: DXGI_SAMPLE_DESC {
                            Count: 1,
                            Quality: 0,
                        },
                        Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
                        Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
                        Alignment: 0,
                    },
                    D3D12_RESOURCE_STATE_COMMON,
                    None,
                    &mut texture,
                )
                .context("YUV texture on the compute device")?;
            let texture = texture.context("no YUV texture")?;
            crate::amf::gpu::set_resource_state(&texture, D3D12_RESOURCE_STATE_COMMON)?;
            let index = self.targets.len();
            self.targets.push(Target {
                texture: Arc::new(texture),
                fence: self.compute.device.CreateFence(0, D3D12_FENCE_FLAG_NONE)?,
                value: 0,
            });
            self.create_target_uavs(index);
            Ok(index)
        }
    }
    /// Write the permanent UAV descriptor(s) for `self.targets[index]` at
    /// `TARGET_UAV_BASE + index * 2`.
    ///
    /// # Safety
    /// `index < self.targets.len()` and `self.targets[index]` is freshly created
    /// (not referenced by any in-flight command list yet).
    unsafe fn create_target_uavs(&self, index: usize) {
        let device = &self.compute.device;
        let target = &self.targets[index].texture;
        // SAFETY: `self.heap` is live on `device` and holds `MAX_TARGETS * 2` descriptors past
        // `TARGET_UAV_BASE`.
        unsafe {
            let cpu = self.heap.GetCPUDescriptorHandleForHeapStart();
            for (plane, &uav_format) in self.layout.uav_formats().iter().enumerate() {
                let handle = D3D12_CPU_DESCRIPTOR_HANDLE {
                    ptr: cpu.ptr + (TARGET_UAV_BASE + index * 2 + plane) * self.increment as usize,
                };
                device.CreateUnorderedAccessView(
                    target.as_ref(),
                    None,
                    Some(&texture_uav(uav_format, plane as u32)),
                    handle,
                );
            }
        }
    }
    /// Update `slot`'s source (`at(0)`) and pointer (`at(1)`) SRVs only when
    /// the resource, format or cache epoch changed.
    ///
    /// # Safety
    /// `slot` is idle on the GPU and `source`/`pointer` live on `self.compute.device`.
    pub(super) unsafe fn update_input_srvs(
        &mut self,
        slot: usize,
        source: &ID3D12Resource,
        format: DXGI_FORMAT,
        pointer: Option<&ID3D12Resource>,
    ) {
        let cache_epoch = self.compute.cache_epoch.load(Ordering::Acquire);
        let source_key = (source.as_raw() as usize, format.0, cache_epoch);
        let pointer_key = (
            pointer.map_or(0, |p| p.as_raw() as usize),
            if pointer.is_some() {
                self.pointer_epoch
            } else {
                0
            },
        );
        // SAFETY: `self.heap` is live on `device` and the caller waited for `slot` to finish.
        unsafe {
            let device = &self.compute.device;
            let cpu = self.heap.GetCPUDescriptorHandleForHeapStart();
            let at = |index: usize| D3D12_CPU_DESCRIPTOR_HANDLE {
                ptr: cpu.ptr + (slot * VIEWS + index) * self.increment as usize,
            };
            if self.slots[slot].source_srv != Some(source_key) {
                device.CreateShaderResourceView(source, Some(&texture_srv(format)), at(0));
                self.slots[slot].source_srv = Some(source_key);
            }
            if self.slots[slot].pointer_srv != Some(pointer_key) {
                device.CreateShaderResourceView(
                    pointer,
                    Some(&texture_srv(DXGI_FORMAT_R8G8B8A8_UNORM)),
                    at(1),
                );
                self.slots[slot].pointer_srv = Some(pointer_key);
            }
        }
    }
    /// Convert `source` (an RGB texture opened on this device) into a new
    /// YUV texture. The values follow the D3D11 converter's constants.
    pub fn convert(
        &mut self,
        source: &ID3D12Resource,
        format: DXGI_FORMAT,
        pointer: Option<&ID3D12Resource>,
        image: &crate::capture::GpuImage,
    ) -> Result<Converted> {
        self.compute.ensure_active()?;
        validate_colour_input(&self.values)?;
        let ready = image.ready.as_ref();
        let index = self.target()?;
        let target = self.targets[index].texture.clone();
        // AMF updates this private data when it reads a submitted texture. Its
        // native surface release and completion fence make the selected target
        // free; neither returns its resource state to COMMON.
        let state = crate::amf::gpu::resource_state(&target)?;
        ensure_compute_texture_state(state)?;
        let slot = self.slot;
        self.slot = (self.slot + 1) % SLOTS;
        self.compute.wait(self.slots[slot].last())?;
        self.slots[slot].capture = None;
        self.reset_hdr_stats(slot);
        let (entry, block) = self.layout.menu_entry(self.packs_chroma, self.values[11]);
        let menu = self.menu_pass(slot, entry, 0, self.layout.uav_count(), block)?;
        let focus_active = self
            .focus
            .as_deref()
            .is_some_and(|focus| focus.settings.is_some());
        if !focus_active && self.yuv.is_none() {
            let root = root_signature(&self.compute.device, 0, self.layout.uav_count())?;
            let convert = pipeline(&self.compute.device, &root, self.layout.shader_entry())?;
            self.yuv = Some((root, convert));
        }
        // SAFETY: The slot's last conversion has finished (waited above), so its constants and
        // views may be rewritten, and every view and barrier names a live resource on this device.
        unsafe {
            self.write_values(slot);
            self.slots[slot].planar_view = None;
            self.update_input_srvs(slot, source, format, pointer);
            let gpu = self.heap.GetGPUDescriptorHandleForHeapStart();
            let Slot {
                allocator, list, ..
            } = &self.slots[slot];
            let focus = self
                .focus
                .as_deref()
                .filter(|focus| focus.settings.is_some());
            let (root, convert) = match focus {
                Some(focus) => (&focus.root, &focus.convert),
                None => {
                    let (root, convert) = self.yuv.as_ref().expect("YUV pipeline ensured above");
                    (root, convert)
                }
            };
            allocator.Reset()?;
            list.Reset(allocator, convert)?;
            if let Some(menu) = &menu {
                menu.copy(list);
            }
            let mut barriers = [transition(
                target.as_ref(),
                state,
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
            )];
            list.ResourceBarrier(&barriers);
            drop_barriers(&mut barriers);
            list.SetComputeRootSignature(root);
            list.SetDescriptorHeaps(&[Some(self.heap.clone())]);
            list.SetComputeRootConstantBufferView(
                0,
                self.constants.GetGPUVirtualAddress() + (slot * 256) as u64,
            );
            let target_uav = D3D12_GPU_DESCRIPTOR_HANDLE {
                ptr: gpu.ptr + ((TARGET_UAV_BASE + index * 2) * self.increment as usize) as u64,
            };
            let table = |which: usize| {
                if which == 2 {
                    target_uav
                } else {
                    D3D12_GPU_DESCRIPTOR_HANDLE {
                        ptr: gpu.ptr + ((slot * VIEWS + which) * self.increment as usize) as u64,
                    }
                }
            };
            list.SetComputeRootDescriptorTable(1, table(0));
            list.SetComputeRootDescriptorTable(2, target_uav);
            if let Some(focus) = focus {
                focus.bind(list);
            }
            let (groups_x, groups_y) = self.layout.dispatch_groups_for(
                self.width,
                self.height,
                self.packs_chroma,
                self.values[11],
            );
            list.Dispatch(groups_x, groups_y, 1);
            if let Some(focus) = focus {
                focus.record(list, slot);
            }
            if let Some(menu) = &menu {
                menu.record(list, self.constants.GetGPUVirtualAddress(), slot, table);
            }
            let mut barriers = [transition(
                target.as_ref(),
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                D3D12_RESOURCE_STATE_COMMON,
            )];
            list.ResourceBarrier(&barriers);
            drop_barriers(&mut barriers);
            list.Close()?;
        }
        // The slot's previous conversion has finished (waited above), so its
        // inputs can go; this one's stay until the slot comes round again.
        self.slots[slot].inputs = [
            Some(source.clone()),
            pointer.cloned(),
            menu.as_ref().map(|menu| menu.texture.clone()),
        ];
        self.slots[slot].capture = Some(image.clone());
        self.slots[slot].outputs.clear();
        self.slots[slot].waits = ready.map(|ready| ready.fence.clone()).into_iter().collect();
        // The capture's copy into `source` may still be waiting for DWM.
        let ordered = self
            .compute
            .ensure_active()
            .and_then(|()| self.wait_capture(ready));
        let Slot { fence, list, .. } = &mut self.slots[slot];
        let (compute, overlay) = (&self.compute, &mut self.menu);
        let fence = submit_slot(
            fence,
            ordered,
            || overlay.submitting(menu.as_ref()),
            || compute.execute(list).inspect_err(|_| compute.retire()),
        )?;
        if let Some(menu) = &menu {
            self.menu_submitted(menu, fence);
        }
        // Only now, so a failed wait or submission leaves the age reset pending.
        if let Some(focus) = self.focus.as_deref_mut() {
            focus.recorded(slot);
        }
        if let Some(ready) = ready {
            ready.read_by(&self.compute, fence)?;
        }
        // The encoder's signal for this texture, after the conversion.
        let target = &mut self.targets[index];
        let value = target.last() + 1;
        // SAFETY: `queue` and `target.fence` were created on the same D3D12 device and outlive the
        // call.
        unsafe {
            self.compute
                .queue
                .Signal(&target.fence, value)
                .inspect_err(|_| self.compute.retire())?;
        }
        target.value = value;
        let converted = Converted {
            texture: target.texture.clone(),
            fence: target.fence.clone(),
            value: target.value,
            conversion: fence,
        };
        self.queue_hdr_stats(slot, fence);
        Ok(converted)
    }
}
