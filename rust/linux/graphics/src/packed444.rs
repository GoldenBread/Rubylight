//! Packed 4:4:4 on a Linux host: a captured BGRX picture's full-resolution YCbCr carried in one
//! ordinary NV12 picture of twice the padded height, which any 4:2:0 encoder takes and Nightjar
//! takes apart again (`rubylight_protocol::packed444`, version 1). The Windows host packs in its
//! colour shaders; here one Vulkan compute pass (`packed444/pack.comp`) does it on the GPU the
//! VA display runs on, writing the codes the Windows host's `lumaCode` and `chromaCode` write.
//!
//! Both pictures are linear VA surfaces ([`Pool::new_linear`](crate::vaapi::pool::Pool)),
//! exported as DMA-BUFs and imported into Vulkan as storage buffers, so the pass reads and
//! writes rows of bytes: no image formats or tiling. Each surface is imported once and kept for
//! as long as it lives. The pass is synchronous: [`Packer::pack`] returns once the target is
//! written, and the encoder reads it next.
//!
//! Not yet: version 2 (layered, for 10-bit streams), scaling (the picture must have the
//! stream's size) and HDR.
use crate::pyrowave::dmabuf::{self, DmaBuf};
use crate::vaapi::{Va, ffi::VASurfaceID, pool::Pooled};
use anyhow::{Context, Result, bail, ensure};
use ash::vk;
use rubylight_protocol::packed444::Geometry;
use std::collections::HashMap;
use std::ffi::CStr;
use std::os::fd::{AsRawFd, IntoRawFd};

/// `pack.comp` compiled by `glslc --target-env=vulkan1.2 -O pack.comp -o pack.spv`.
const SHADER: &[u8] = include_bytes!("packed444/pack.spv");
/// How long a pass may take before the GPU counts as hung: a 4K pass takes well under 1 ms.
const PASS_WAIT_NS: u64 = 1_000_000_000;
/// Source pixels one invocation codes, and the invocations of a workgroup in x and y.
const PIXELS: u32 = 8;
const GROUP: u32 = 8;

/// `Parameters` in `pack.comp`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct Parameters {
    width: u32,
    height: u32,
    source_pitch: u32,
    target_pitch: u32,
    padded_width: u32,
    padded_height: u32,
    chroma_offset: u32,
    color_matrix: u32,
    full_range: u32,
}

/// A surface's memory as a Vulkan buffer, with the layout of its planes.
struct Imported {
    /// The DMA-BUF's inode: a surface id the driver reuses for a new surface has another.
    inode: u64,
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    planes: Vec<dmabuf::Plane>,
}

/// The compute pass of one stream, on the GPU of its VA display.
pub struct Packer {
    _entry: ash::Entry,
    instance: ash::Instance,
    device: ash::Device,
    memory_fd: ash::khr::external_memory_fd::Device,
    family: u32,
    queue: vk::Queue,
    set_layout: vk::DescriptorSetLayout,
    pipeline_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    descriptors: vk::DescriptorPool,
    set: vk::DescriptorSet,
    commands: vk::CommandPool,
    command: vk::CommandBuffer,
    fence: vk::Fence,
    imports: HashMap<VASurfaceID, Imported>,
    geometry: Geometry,
    color_matrix: u32,
    full_range: bool,
}

// SAFETY: the handles are used from one thread at a time (`&mut self`); Vulkan objects may be
// used from any thread when access is externally synchronized.
unsafe impl Send for Packer {}

impl Packer {
    /// A pass that packs `width`x`height` pictures with `color_matrix` (0 BT.601, 1 BT.709,
    /// 2 BT.2020) in full or limited range, on the GPU of `va`.
    pub fn new(
        va: &Va,
        (width, height): (u32, u32),
        color_matrix: u8,
        full_range: bool,
    ) -> Result<Self> {
        let geometry = Geometry::new(width, height)
            .with_context(|| format!("a {width}x{height} picture cannot be packed"))?;
        ensure!(color_matrix <= 2, "colour matrix {color_matrix}");
        let node = va.render_node().context("the VA display's render node")?;
        // SAFETY: loads the system's Vulkan loader, which stays loaded as long as `_entry`.
        let entry = unsafe { ash::Entry::load() }.context("loading Vulkan (libvulkan.so.1)")?;
        let application = vk::ApplicationInfo::default()
            .application_name(c"Rubylight packed 4:4:4")
            .api_version(vk::API_VERSION_1_2);
        // SAFETY: a valid create info whose pointers live until the call returns.
        let instance = unsafe {
            entry.create_instance(
                &vk::InstanceCreateInfo::default().application_info(&application),
                None,
            )
        }
        .context("creating a Vulkan instance")?;
        let device = match open_device(&instance, node) {
            Ok(found) => found,
            Err(error) => {
                // SAFETY: nothing was made on the instance.
                unsafe { instance.destroy_instance(None) };
                return Err(error);
            }
        };
        let (device, family) = device;
        let memory_fd = ash::khr::external_memory_fd::Device::new(&instance, &device);
        // SAFETY: the family was asked for with one queue.
        let queue = unsafe { device.get_device_queue(family, 0) };
        let mut packer = Self {
            _entry: entry,
            instance,
            device,
            memory_fd,
            family,
            queue,
            set_layout: vk::DescriptorSetLayout::null(),
            pipeline_layout: vk::PipelineLayout::null(),
            pipeline: vk::Pipeline::null(),
            descriptors: vk::DescriptorPool::null(),
            set: vk::DescriptorSet::null(),
            commands: vk::CommandPool::null(),
            command: vk::CommandBuffer::null(),
            fence: vk::Fence::null(),
            imports: HashMap::new(),
            geometry,
            color_matrix: u32::from(color_matrix),
            full_range,
        };
        // On an error the objects made so far are destroyed with `packer`.
        packer.build().context("preparing the packed 4:4:4 pass")?;
        Ok(packer)
    }

    /// The size of the pictures the pass writes: the stream's coded size.
    pub fn coded_size(&self) -> (u32, u32) {
        self.geometry.coded_size()
    }

    fn build(&mut self) -> Result<()> {
        let device = &self.device;
        let code = ash::util::read_spv(&mut std::io::Cursor::new(SHADER))?;
        // SAFETY: a create info whose pointers live until the call returns, on a live device;
        // the module is destroyed at the end of this function.
        let module = unsafe {
            device.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&code), None)
        }?;
        let bindings = [0, 1].map(|binding| {
            vk::DescriptorSetLayoutBinding::default()
                .binding(binding)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE)
        });
        let made = (|| -> Result<()> {
            // SAFETY: a create info whose pointers live until the call returns, on a live device;
            // the handle is destroyed in `Drop`.
            self.set_layout = unsafe {
                device.create_descriptor_set_layout(
                    &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                    None,
                )
            }?;
            let ranges = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::COMPUTE)
                .size(std::mem::size_of::<Parameters>() as u32)];
            let layouts = [self.set_layout];
            // SAFETY: a create info whose pointers live until the call returns, on a live device;
            // the handle is destroyed in `Drop`.
            self.pipeline_layout = unsafe {
                device.create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default()
                        .set_layouts(&layouts)
                        .push_constant_ranges(&ranges),
                    None,
                )
            }?;
            let stage = vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::COMPUTE)
                .module(module)
                .name(c"main");
            let info = vk::ComputePipelineCreateInfo::default()
                .stage(stage)
                .layout(self.pipeline_layout);
            // SAFETY: a create info whose pointers live until the call returns, on a live device;
            // the handle is destroyed in `Drop`.
            self.pipeline = unsafe {
                device.create_compute_pipelines(vk::PipelineCache::null(), &[info], None)
            }
            .map_err(|(_, error)| error)?[0];
            let sizes = [vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(2)];
            // SAFETY: a create info whose pointers live until the call returns, on a live device;
            // the handle is destroyed in `Drop`.
            self.descriptors = unsafe {
                device.create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .max_sets(1)
                        .pool_sizes(&sizes),
                    None,
                )
            }?;
            // SAFETY: a create info whose pointers live until the call returns, on a live device;
            // the handle is destroyed in `Drop`.
            self.set = unsafe {
                device.allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(self.descriptors)
                        .set_layouts(&layouts),
                )
            }?[0];
            // SAFETY: a create info whose pointers live until the call returns, on a live device;
            // the handle is destroyed in `Drop`.
            self.commands = unsafe {
                device.create_command_pool(
                    &vk::CommandPoolCreateInfo::default()
                        .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER)
                        .queue_family_index(self.family),
                    None,
                )
            }?;
            // SAFETY: a create info whose pointers live until the call returns, on a live device;
            // the handle is destroyed in `Drop`.
            self.command = unsafe {
                device.allocate_command_buffers(
                    &vk::CommandBufferAllocateInfo::default()
                        .command_pool(self.commands)
                        .level(vk::CommandBufferLevel::PRIMARY)
                        .command_buffer_count(1),
                )
            }?[0];
            // SAFETY: a create info whose pointers live until the call returns, on a live device;
            // the handle is destroyed in `Drop`.
            self.fence = unsafe { device.create_fence(&vk::FenceCreateInfo::default(), None) }?;
            Ok(())
        })();
        // SAFETY: the pipeline (if any) holds what it needs of the module.
        unsafe { device.destroy_shader_module(module, None) };
        made
    }

    /// Writes the packed picture of `source` (a linear BGRX surface holding the picture at its
    /// top left) into `target` (a linear NV12 surface of at least [`Self::coded_size`]). Both
    /// must be finished by the work that wrote or read them before.
    pub fn pack(&mut self, source: &Pooled, target: &Pooled) -> Result<()> {
        let geometry = self.geometry;
        let (from, from_pitch) = {
            let imported = self.import(source, false)?;
            (imported.buffer, imported.planes[0].pitch)
        };
        let (to, to_pitch, chroma) = {
            let imported = self.import(target, true)?;
            ensure!(
                imported.planes.len() == 2,
                "the target has {} planes; NV12 has two",
                imported.planes.len()
            );
            (
                imported.buffer,
                imported.planes[0].pitch,
                imported.planes[1].offset,
            )
        };
        let (coded_width, _) = geometry.coded_size();
        ensure!(
            from_pitch % 4 == 0 && from_pitch >= u64::from(geometry.width) * 4,
            "source rows of {from_pitch} bytes for a {}-pixel picture",
            geometry.width
        );
        ensure!(
            to_pitch % 4 == 0 && chroma % 4 == 0 && to_pitch >= u64::from(coded_width),
            "target rows of {to_pitch} bytes, chroma at {chroma}, for a {coded_width}-pixel picture"
        );
        let parameters = Parameters {
            width: geometry.width,
            height: geometry.height,
            source_pitch: (from_pitch / 4) as u32,
            target_pitch: (to_pitch / 4) as u32,
            padded_width: geometry.padded_width,
            padded_height: geometry.padded_height,
            chroma_offset: (chroma / 4) as u32,
            color_matrix: self.color_matrix,
            full_range: u32::from(self.full_range),
        };
        self.run(from, to, &parameters)
    }

    fn run(&mut self, from: vk::Buffer, to: vk::Buffer, parameters: &Parameters) -> Result<()> {
        let device = &self.device;
        let infos = [from, to].map(|buffer| {
            [vk::DescriptorBufferInfo::default()
                .buffer(buffer)
                .range(vk::WHOLE_SIZE)]
        });
        let writes = [0, 1].map(|binding| {
            vk::WriteDescriptorSet::default()
                .dst_set(self.set)
                .dst_binding(binding)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(&infos[binding as usize])
        });
        // The buffers' memory belongs to VA-API between passes: taken over before the pass
        // and handed back after it.
        let barrier = |buffer, access, acquire: bool| {
            let (from, to) = if acquire {
                (vk::QUEUE_FAMILY_EXTERNAL, self.family)
            } else {
                (self.family, vk::QUEUE_FAMILY_EXTERNAL)
            };
            let (src, dst) = if acquire {
                (vk::AccessFlags::empty(), access)
            } else {
                (access, vk::AccessFlags::empty())
            };
            vk::BufferMemoryBarrier::default()
                .src_access_mask(src)
                .dst_access_mask(dst)
                .src_queue_family_index(from)
                .dst_queue_family_index(to)
                .buffer(buffer)
                .size(vk::WHOLE_SIZE)
        };
        let acquire = [
            barrier(from, vk::AccessFlags::SHADER_READ, true),
            barrier(to, vk::AccessFlags::SHADER_WRITE, true),
        ];
        let release = [
            barrier(from, vk::AccessFlags::SHADER_READ, false),
            barrier(to, vk::AccessFlags::SHADER_WRITE, false),
        ];
        // SAFETY: plain `repr(C)` u32 fields, read as the bytes the shader's block declares.
        let constants = unsafe {
            std::slice::from_raw_parts(
                std::ptr::from_ref(parameters).cast::<u8>(),
                std::mem::size_of::<Parameters>(),
            )
        };
        let groups = (
            parameters.padded_width.div_ceil(PIXELS * GROUP),
            parameters.padded_height.div_ceil(GROUP),
        );
        // SAFETY: the set, command buffer and fence are this pass's own and idle (the previous
        // pass was waited for); the buffers outlive the submission, which is waited for below.
        unsafe {
            device.update_descriptor_sets(&writes, &[]);
            device.begin_command_buffer(
                self.command,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
            device.cmd_pipeline_barrier(
                self.command,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &acquire,
                &[],
            );
            device.cmd_bind_pipeline(self.command, vk::PipelineBindPoint::COMPUTE, self.pipeline);
            device.cmd_bind_descriptor_sets(
                self.command,
                vk::PipelineBindPoint::COMPUTE,
                self.pipeline_layout,
                0,
                &[self.set],
                &[],
            );
            device.cmd_push_constants(
                self.command,
                self.pipeline_layout,
                vk::ShaderStageFlags::COMPUTE,
                0,
                constants,
            );
            device.cmd_dispatch(self.command, groups.0, groups.1, 1);
            device.cmd_pipeline_barrier(
                self.command,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                vk::DependencyFlags::empty(),
                &[],
                &release,
                &[],
            );
            device.end_command_buffer(self.command)?;
            let commands = [self.command];
            device.queue_submit(
                self.queue,
                &[vk::SubmitInfo::default().command_buffers(&commands)],
                self.fence,
            )?;
            let waited = device.wait_for_fences(&[self.fence], true, PASS_WAIT_NS);
            if waited == Err(vk::Result::TIMEOUT) {
                bail!("the packed 4:4:4 pass did not finish within a second: the GPU may be hung");
            }
            waited?;
            device.reset_fences(&[self.fence])?;
        }
        Ok(())
    }

    /// `surface`'s memory in this device, imported the first time the surface is seen.
    fn import(&mut self, surface: &Pooled, writable: bool) -> Result<&Imported> {
        let exported = if writable {
            dmabuf::export_surface_writable(surface)
        } else {
            dmabuf::export_surface(surface)
        }?;
        ensure!(
            exported.modifier == 0,
            "packed 4:4:4 reads and writes linear surfaces; this one is laid out with modifier 0x{:x}",
            exported.modifier
        );
        let inode = inode(&exported)?;
        let id = surface.id();
        if self
            .imports
            .get(&id)
            .is_some_and(|found| found.inode == inode)
        {
            return Ok(&self.imports[&id]);
        }
        if let Some(stale) = self.imports.remove(&id) {
            self.forget(stale);
        }
        let imported = self.import_dmabuf(exported, inode)?;
        Ok(self.imports.entry(id).or_insert(imported))
    }

    fn import_dmabuf(&self, exported: DmaBuf, inode: u64) -> Result<Imported> {
        let device = &self.device;
        // SAFETY: a seek on a descriptor owned here; a DMA-BUF reports its size this way.
        let size = unsafe { libc::lseek(exported.fd.as_raw_fd(), 0, libc::SEEK_END) };
        ensure!(size > 0, "the DMA-BUF reports no size");
        let size = size as u64;
        let mut external = vk::ExternalMemoryBufferCreateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        // SAFETY: a valid create info; the buffer is destroyed in `forget` or below.
        let buffer = unsafe {
            device.create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(size)
                    .usage(vk::BufferUsageFlags::STORAGE_BUFFER)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE)
                    .push_next(&mut external),
                None,
            )
        }?;
        let bound = (|| -> Result<vk::DeviceMemory> {
            // SAFETY: a buffer of this device; the descriptor stays open for the query.
            let requirements = unsafe { device.get_buffer_memory_requirements(buffer) };
            ensure!(
                requirements.size <= size,
                "the buffer needs {} bytes of a {size}-byte DMA-BUF",
                requirements.size
            );
            let mut properties = vk::MemoryFdPropertiesKHR::default();
            // SAFETY: a DMA-BUF descriptor owned here.
            unsafe {
                self.memory_fd.get_memory_fd_properties(
                    vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT,
                    exported.fd.as_raw_fd(),
                    &mut properties,
                )
            }?;
            let types = requirements.memory_type_bits & properties.memory_type_bits;
            ensure!(types != 0, "no Vulkan memory type takes the DMA-BUF");
            // A successful import owns the descriptor; a failed one leaves it to be closed.
            let fd = exported.fd.into_raw_fd();
            let mut import = vk::ImportMemoryFdInfoKHR::default()
                .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT)
                .fd(fd);
            // SAFETY: a valid allocate info importing a descriptor owned here.
            let memory = unsafe {
                device.allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(size)
                        .memory_type_index(types.trailing_zeros())
                        .push_next(&mut import),
                    None,
                )
            };
            let memory = match memory {
                Ok(memory) => memory,
                Err(error) => {
                    // SAFETY: the import failed, so the descriptor is still ours.
                    unsafe { libc::close(fd) };
                    bail!("importing the DMA-BUF into Vulkan: {error}");
                }
            };
            // SAFETY: memory of a type the buffer takes, at least as large as it needs.
            if let Err(error) = unsafe { device.bind_buffer_memory(buffer, memory, 0) } {
                // SAFETY: memory just allocated and bound to nothing.
                unsafe { device.free_memory(memory, None) };
                bail!("binding the DMA-BUF's memory: {error}");
            }
            Ok(memory)
        })();
        match bound {
            Ok(memory) => Ok(Imported {
                inode,
                buffer,
                memory,
                planes: exported.planes,
            }),
            Err(error) => {
                // SAFETY: a buffer of this device that nothing uses.
                unsafe { device.destroy_buffer(buffer, None) };
                Err(error)
            }
        }
    }

    fn forget(&self, imported: Imported) {
        // SAFETY: every pass that used them was waited for.
        unsafe {
            self.device.destroy_buffer(imported.buffer, None);
            self.device.free_memory(imported.memory, None);
        }
    }
}

impl Drop for Packer {
    fn drop(&mut self) {
        // SAFETY: the device is idle after the wait; null handles of a build that failed part
        // way are ignored by Vulkan's destroy calls.
        unsafe {
            let _ = self.device.device_wait_idle();
            for (_, imported) in std::mem::take(&mut self.imports) {
                self.forget(imported);
            }
            self.device.destroy_fence(self.fence, None);
            self.device.destroy_command_pool(self.commands, None);
            self.device.destroy_descriptor_pool(self.descriptors, None);
            self.device.destroy_pipeline(self.pipeline, None);
            self.device
                .destroy_pipeline_layout(self.pipeline_layout, None);
            self.device
                .destroy_descriptor_set_layout(self.set_layout, None);
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
    }
}

/// The Vulkan device of the GPU whose render node is `node`, with a compute queue family and
/// DMA-BUF import.
fn open_device(instance: &ash::Instance, node: (u32, u32)) -> Result<(ash::Device, u32)> {
    let wanted: [&CStr; 2] = [
        ash::khr::external_memory_fd::NAME,
        ash::ext::external_memory_dma_buf::NAME,
    ];
    // SAFETY: a live instance.
    let physicals = unsafe { instance.enumerate_physical_devices() }?;
    let found = physicals.into_iter().find_map(|physical| {
        // SAFETY: a physical device of this instance.
        let extensions =
            unsafe { instance.enumerate_device_extension_properties(physical) }.ok()?;
        let has = |name: &CStr| {
            extensions
                .iter()
                .any(|e| e.extension_name_as_c_str() == Ok(name))
        };
        if !has(ash::ext::physical_device_drm::NAME) || !wanted.iter().all(|name| has(name)) {
            return None;
        }
        let mut drm = vk::PhysicalDeviceDrmPropertiesEXT::default();
        let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut drm);
        // SAFETY: the device supports the DRM properties extension.
        unsafe { instance.get_physical_device_properties2(physical, &mut properties) };
        let render = (drm.render_major as u32, drm.render_minor as u32);
        if drm.has_render != vk::TRUE || render != node {
            return None;
        }
        // SAFETY: a physical device of this instance.
        let families = unsafe { instance.get_physical_device_queue_family_properties(physical) };
        let family = families
            .iter()
            .position(|f| f.queue_flags.contains(vk::QueueFlags::COMPUTE))?;
        Some((physical, family as u32))
    });
    let (physical, family) = found.with_context(|| {
        format!(
            "no Vulkan device with DMA-BUF import is the GPU of render node {}:{}",
            node.0, node.1
        )
    })?;
    let priorities = [1.0f32];
    let queues = [vk::DeviceQueueCreateInfo::default()
        .queue_family_index(family)
        .queue_priorities(&priorities)];
    let names = wanted.map(CStr::as_ptr);
    // SAFETY: a valid create info whose pointers live until the call returns.
    let device = unsafe {
        instance.create_device(
            physical,
            &vk::DeviceCreateInfo::default()
                .queue_create_infos(&queues)
                .enabled_extension_names(&names),
            None,
        )
    }
    .context("creating the Vulkan device")?;
    Ok((device, family))
}

fn inode(exported: &DmaBuf) -> Result<u64> {
    // SAFETY: a plain-data struct the call fills in, for a descriptor owned here.
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: `stat` is a valid out-pointer; the descriptor stays open for the call.
    let status = unsafe { libc::fstat(exported.fd.as_raw_fd(), &raw mut stat) };
    ensure!(status == 0, "fstat on a DMA-BUF failed");
    Ok(stat.st_ino)
}

#[cfg(test)]
#[path = "packed444/tests.rs"]
mod tests;
