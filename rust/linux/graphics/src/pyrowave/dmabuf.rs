//! A GPU picture as a DMA-BUF, and its import into PyroWave's Vulkan device as external memory
//! (`VK_EXT_external_memory_dma_buf` with `VK_EXT_image_drm_format_modifier`, through
//! `pyrowave_image_create`). [`export_surface`] makes one from a VA-API surface with
//! `vaExportSurfaceHandle`, so a capture's picture reaches the encoder without a copy.

use super::ffi::{self, Api};
use crate::vaapi::{ffi as va, pool::Pooled};
use anyhow::{Context, Result, bail, ensure};
use ash::vk;
use std::{
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    sync::Arc,
};

const fn fourcc(code: &[u8; 4]) -> u32 {
    u32::from_le_bytes(*code)
}
pub const DRM_FORMAT_XRGB8888: u32 = fourcc(b"XR24");
pub const DRM_FORMAT_ARGB8888: u32 = fourcc(b"AR24");
pub const DRM_FORMAT_XBGR8888: u32 = fourcc(b"XB24");
pub const DRM_FORMAT_ABGR8888: u32 = fourcc(b"AB24");
pub const DRM_FORMAT_NV12: u32 = fourcc(b"NV12");
pub const DRM_FORMAT_MOD_INVALID: u64 = 0x00ff_ffff_ffff_ffff;

/// One plane of a [`DmaBuf`] in its memory object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plane {
    pub offset: u64,
    pub pitch: u64,
}

/// A picture in one DMA-BUF object: the descriptor of `vaExportSurfaceHandle` or a
/// compositor's buffer, with its file descriptor owned.
#[derive(Debug)]
pub struct DmaBuf {
    pub fd: OwnedFd,
    pub width: u32,
    pub height: u32,
    /// The DRM fourcc of the picture (`DRM_FORMAT_*`).
    pub fourcc: u32,
    pub modifier: u64,
    pub planes: Vec<Plane>,
}

impl DmaBuf {
    /// The Vulkan format of the picture and whether it is multi-planar YCbCr.
    pub fn format(&self) -> Result<(vk::Format, bool)> {
        Ok(match self.fourcc {
            DRM_FORMAT_XRGB8888 | DRM_FORMAT_ARGB8888 => (vk::Format::B8G8R8A8_UNORM, false),
            DRM_FORMAT_XBGR8888 | DRM_FORMAT_ABGR8888 => (vk::Format::R8G8B8A8_UNORM, false),
            DRM_FORMAT_NV12 => (vk::Format::G8_B8R8_2PLANE_420_UNORM, true),
            other => bail!(
                "DRM format {:?} cannot be encoded by PyroWave yet",
                other.to_le_bytes().map(char::from)
            ),
        })
    }
}

/// Exports `surface` as one DMA-BUF with its planes composed in a single layer. The surface
/// must be finished by the GPU work that wrote it (`vaSyncSurface`) before the picture is read.
pub fn export_surface(surface: &Pooled) -> Result<DmaBuf> {
    export(surface, va::VA_EXPORT_SURFACE_READ_ONLY)
}

/// [`export_surface`] for a picture the importer writes (the packed 4:4:4 pass's target).
pub fn export_surface_writable(surface: &Pooled) -> Result<DmaBuf> {
    export(surface, va::VA_EXPORT_SURFACE_READ_WRITE)
}

fn export(surface: &Pooled, access: u32) -> Result<DmaBuf> {
    let display = surface.va().dpy;
    // SAFETY: a plain-data descriptor the call fills in.
    let mut desc: va::VADRMPRIMESurfaceDescriptor = unsafe { std::mem::zeroed() };
    // SAFETY: `display` and the surface id are live for as long as `surface` is; `desc` is a
    // valid out-pointer for the PRIME_2 memory type.
    let status = unsafe {
        va::vaSyncSurface(display, surface.id());
        va::vaExportSurfaceHandle(
            display,
            surface.id(),
            va::VA_SURFACE_ATTRIB_MEM_TYPE_DRM_PRIME_2,
            access | va::VA_EXPORT_SURFACE_COMPOSED_LAYERS,
            std::ptr::from_mut(&mut desc).cast(),
        )
    };
    ensure!(
        status == va::VA_STATUS_SUCCESS as i32,
        "vaExportSurfaceHandle failed ({status})"
    );
    // SAFETY: the export handed over `num_objects` file descriptors, now owned here.
    let fds: Vec<OwnedFd> = (0..desc.num_objects as usize)
        .map(|i| unsafe { OwnedFd::from_raw_fd(desc.objects[i].fd) })
        .collect();
    ensure!(
        fds.len() == 1 && desc.num_layers == 1,
        "the surface exports as {} memory objects in {} layers; one of each is supported",
        fds.len(),
        desc.num_layers
    );
    let layer = &desc.layers[0];
    let planes = (0..layer.num_planes as usize)
        .map(|i| Plane {
            offset: u64::from(layer.offset[i]),
            pitch: u64::from(layer.pitch[i]),
        })
        .collect();
    let fd = fds.into_iter().next().context("no exported object")?;
    Ok(DmaBuf {
        fd,
        width: desc.width,
        height: desc.height,
        fourcc: layer.drm_format,
        modifier: desc.objects[0].drm_format_modifier,
        planes,
    })
}

/// A [`DmaBuf`] imported into the PyroWave device, destroyed on drop.
pub struct Imported {
    api: Arc<Api>,
    pub image: ffi::Image,
    pub view: ffi::ImageView,
}

impl Imported {
    /// Imports `buf` (its descriptor is duplicated; the caller keeps the original).
    pub fn new(api: &Arc<Api>, device: ffi::Device, buf: &DmaBuf) -> Result<Self> {
        let (format, planar) = buf.format()?;
        ensure!(
            buf.modifier != DRM_FORMAT_MOD_INVALID,
            "the picture's DRM modifier is unknown, so its layout cannot be imported"
        );
        let layouts: Vec<vk::SubresourceLayout> = buf
            .planes
            .iter()
            .map(|p| vk::SubresourceLayout {
                offset: p.offset,
                row_pitch: p.pitch,
                ..Default::default()
            })
            .collect();
        let mut modifier = vk::ImageDrmFormatModifierExplicitCreateInfoEXT::default()
            .drm_format_modifier(buf.modifier)
            .plane_layouts(&layouts);
        let mut external = vk::ExternalMemoryImageCreateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        let mut create = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D {
                width: buf.width,
                height: buf.height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
            .usage(vk::ImageUsageFlags::SAMPLED)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .push_next(&mut external)
            .push_next(&mut modifier);
        if planar {
            create = create.flags(vk::ImageCreateFlags::MUTABLE_FORMAT);
        }
        let fd = buf.fd.try_clone().context("duplicating the DMA-BUF")?;
        let info = ffi::ImageCreateInfo {
            device,
            // The library takes the descriptor on a successful import.
            external_handle: fd.as_raw_fd() as usize,
            handle_type: vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT,
            image_create_info: std::ptr::from_ref(&create).cast(),
        };
        let mut image = std::ptr::null_mut();
        // SAFETY: `info` and the chain it points to live across the call; `device` is a live
        // PyroWave device from `api`.
        let result = unsafe { (api.image_create)(&info, &mut image) };
        if result == ffi::SUCCESS {
            std::mem::forget(fd);
        }
        ffi::check(result).context("importing the DMA-BUF into Vulkan")?;
        let mut imported = Self {
            api: api.clone(),
            image,
            view: ffi::ImageView {
                image: vk::Image::null(),
                width: 0,
                height: 0,
                image_format: vk::Format::UNDEFINED,
                view_format: vk::Format::UNDEFINED,
                mip_level: 0,
                layer: 0,
                aspect: vk::ImageAspectFlags::empty(),
                swizzle: vk::ComponentSwizzle::IDENTITY,
                layout: vk::ImageLayout::GENERAL,
            },
        };
        // SAFETY: `image` was just created; `view` is a valid out-pointer.
        ffi::check(unsafe {
            (api.image_get_image_view)(
                image,
                vk::ImageAspectFlags::COLOR,
                vk::ImageUsageFlags::SAMPLED,
                &mut imported.view,
            )
        })?;
        Ok(imported)
    }
}

impl Drop for Imported {
    fn drop(&mut self) {
        // SAFETY: the image is live and no encode of it is still running (encodes are
        // synchronous).
        unsafe { (self.api.image_destroy)(self.image) };
    }
}
