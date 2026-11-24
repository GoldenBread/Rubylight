//! libva on a DRM render node: the display, surfaces, buffers and the calls capture and the
//! encoders share. Ported from the Linux host spike's probes (`rust/linux/probes/src/va.rs`).
use anyhow::{Context, Result, anyhow, bail};
use std::{
    ffi::{CStr, c_void},
    fs::{File, OpenOptions},
    os::fd::AsRawFd,
    ptr,
};

use super::ffi::*;

/// `fourcc('N','V','1','2')` and friends, as libva spells pixel formats.
pub const fn fourcc(code: &[u8; 4]) -> u32 {
    u32::from_le_bytes(*code)
}
pub const NV12: u32 = fourcc(b"NV12");
pub const P010: u32 = fourcc(b"P010");
pub const BGRX: u32 = fourcc(b"BGRX");
pub const BGRA: u32 = fourcc(b"BGRA");
pub const RGBX: u32 = fourcc(b"RGBX");
pub const RGBA: u32 = fourcc(b"RGBA");

/// One plane of a picture in a DMA-BUF.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plane {
    pub fd: i32,
    pub offset: u32,
    pub pitch: u32,
}

/// Turns a libva status into an error that names the call.
pub fn check(dpy: VADisplay, status: VAStatus, what: &str) -> Result<()> {
    if status == VA_STATUS_SUCCESS as VAStatus {
        return Ok(());
    }
    // SAFETY: vaErrorStr returns a static string for any status.
    let text = unsafe { CStr::from_ptr(vaErrorStr(status)) }.to_string_lossy();
    let _ = dpy;
    Err(anyhow!("{what}: {text} (0x{status:x})"))
}

/// A libva display on a render node. Closing it ends every object made on it.
pub struct Va {
    pub dpy: VADisplay,
    _node: File,
}
// SAFETY: libva displays may be used from any thread as long as calls on one context are not
// concurrent: capture drives its conversion context from its own thread and each encoder drives
// its contexts from the stream thread. Mesa's driver serialises the calls that share a display.
unsafe impl Send for Va {}
// SAFETY: see above; no method here keeps unsynchronised state of its own.
unsafe impl Sync for Va {}

impl Va {
    pub fn open(node: &str) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(node)
            .with_context(|| format!("open {node}"))?;
        // SAFETY: the descriptor stays open for the display's life (`_node`).
        let dpy = unsafe { vaGetDisplayDRM(file.as_raw_fd()) };
        if dpy.is_null() {
            bail!("vaGetDisplayDRM returned no display for {node}");
        }
        // SAFETY: a valid display; no callback silences libva's info lines.
        unsafe { vaSetInfoCallback(dpy, None, ptr::null_mut()) };
        let (mut major, mut minor) = (0, 0);
        // SAFETY: a valid display and two out pointers.
        let status = unsafe { vaInitialize(dpy, &mut major, &mut minor) };
        check(dpy, status, "vaInitialize")?;
        Ok(Self { dpy, _node: file })
    }

    pub fn vendor(&self) -> String {
        // SAFETY: an initialized display; the string lives as long as it does.
        let text = unsafe { vaQueryVendorString(self.dpy) };
        if text.is_null() {
            return String::new();
        }
        // SAFETY: a NUL-terminated string owned by libva.
        unsafe { CStr::from_ptr(text) }
            .to_string_lossy()
            .into_owned()
    }

    /// The entry points the driver offers for a profile.
    pub fn entrypoints(&self, profile: VAProfile) -> Vec<VAEntrypoint> {
        // SAFETY: the list holds the display's stated maximum of entries.
        unsafe {
            let max = vaMaxNumEntrypoints(self.dpy).max(1) as usize;
            let mut list = vec![0 as VAEntrypoint; max];
            let mut count = 0;
            if vaQueryConfigEntrypoints(self.dpy, profile, list.as_mut_ptr(), &mut count)
                != VA_STATUS_SUCCESS as VAStatus
            {
                return Vec::new();
            }
            list.truncate(count.max(0) as usize);
            list
        }
    }

    /// One configuration attribute's value, or None when the driver does not
    /// support the attribute.
    pub fn attribute(
        &self,
        profile: VAProfile,
        entrypoint: VAEntrypoint,
        kind: VAConfigAttribType,
    ) -> Option<u32> {
        let mut attrib = VAConfigAttrib {
            type_: kind,
            value: 0,
        };
        // SAFETY: one attribute, in and out.
        let status =
            unsafe { vaGetConfigAttributes(self.dpy, profile, entrypoint, &mut attrib, 1) };
        (status == VA_STATUS_SUCCESS as VAStatus && attrib.value != VA_ATTRIB_NOT_SUPPORTED)
            .then_some(attrib.value)
    }

    pub fn create_config(
        &self,
        profile: VAProfile,
        entrypoint: VAEntrypoint,
        attribs: &[(VAConfigAttribType, u32)],
    ) -> Result<VAConfigID> {
        let mut list: Vec<VAConfigAttrib> = attribs
            .iter()
            .map(|&(type_, value)| VAConfigAttrib { type_, value })
            .collect();
        let mut config = 0;
        // SAFETY: `list` outlives the call and its length is passed with it.
        let status = unsafe {
            vaCreateConfig(
                self.dpy,
                profile,
                entrypoint,
                list.as_mut_ptr(),
                list.len() as i32,
                &mut config,
            )
        };
        check(self.dpy, status, "vaCreateConfig")?;
        Ok(config)
    }

    /// Surfaces the driver allocates itself, in one pixel format.
    pub fn create_surfaces(
        &self,
        rt_format: u32,
        pixel: u32,
        width: u32,
        height: u32,
        count: usize,
    ) -> Result<Vec<VASurfaceID>> {
        let mut attrib = int_attrib(VASurfaceAttribPixelFormat, pixel as i32);
        let mut surfaces = vec![VA_INVALID_SURFACE; count];
        // SAFETY: `surfaces` has room for `count` ids; one attribute is passed.
        let status = unsafe {
            vaCreateSurfaces(
                self.dpy,
                rt_format,
                width,
                height,
                surfaces.as_mut_ptr(),
                count as u32,
                &mut attrib,
                1,
            )
        };
        check(self.dpy, status, "vaCreateSurfaces")?;
        Ok(surfaces)
    }

    /// A surface over a DMA-BUF someone else owns (a captured frame), without
    /// copying it. The caller keeps the descriptors open until the surface is
    /// destroyed.
    pub fn import_dmabuf(
        &self,
        pixel: u32,
        modifier: u64,
        width: u32,
        height: u32,
        planes: &[Plane],
    ) -> Result<VASurfaceID> {
        if planes.is_empty() || planes.len() > 4 {
            bail!("a DMA-BUF picture of {} planes", planes.len());
        }
        // SAFETY: the descriptor is plain data; zero is a valid start.
        let mut desc: VADRMPRIMESurfaceDescriptor = unsafe { std::mem::zeroed() };
        desc.fourcc = pixel;
        desc.width = width;
        desc.height = height;
        desc.num_layers = 1;
        desc.layers[0].drm_format = drm_format(pixel)?;
        desc.layers[0].num_planes = planes.len() as u32;
        // Planes that share a descriptor share an object.
        let mut objects: Vec<i32> = Vec::new();
        for (index, plane) in planes.iter().enumerate() {
            let object = match objects.iter().position(|&fd| fd == plane.fd) {
                Some(found) => found,
                None => {
                    objects.push(plane.fd);
                    let slot = &mut desc.objects[objects.len() - 1];
                    slot.fd = plane.fd;
                    // SAFETY: a seek on a descriptor the caller owns; a
                    // DMA-BUF reports its size this way.
                    let size = unsafe { libc::lseek(plane.fd, 0, libc::SEEK_END) };
                    slot.size = u32::try_from(size).unwrap_or(0);
                    slot.drm_format_modifier = modifier;
                    objects.len() - 1
                }
            };
            desc.layers[0].object_index[index] = object as u32;
            desc.layers[0].offset[index] = plane.offset;
            desc.layers[0].pitch[index] = plane.pitch;
        }
        desc.num_objects = objects.len() as u32;
        let rt_format = match pixel {
            NV12 => VA_RT_FORMAT_YUV420,
            P010 => VA_RT_FORMAT_YUV420_10,
            _ => VA_RT_FORMAT_RGB32,
        };
        let mut attribs = [
            int_attrib(VASurfaceAttribPixelFormat, pixel as i32),
            int_attrib(
                VASurfaceAttribMemoryType,
                VA_SURFACE_ATTRIB_MEM_TYPE_DRM_PRIME_2 as i32,
            ),
            VASurfaceAttrib {
                type_: VASurfaceAttribExternalBufferDescriptor,
                flags: VA_SURFACE_ATTRIB_SETTABLE,
                value: VAGenericValue {
                    type_: VAGenericValueTypePointer,
                    value: _VAGenericValue__bindgen_ty_1 {
                        p: (&raw mut desc).cast::<c_void>(),
                    },
                },
            },
        ];
        let mut surface = VA_INVALID_SURFACE;
        // SAFETY: `desc` and `attribs` outlive the call.
        let status = unsafe {
            vaCreateSurfaces(
                self.dpy,
                rt_format,
                width,
                height,
                &mut surface,
                1,
                attribs.as_mut_ptr(),
                attribs.len() as u32,
            )
        };
        check(self.dpy, status, "vaCreateSurfaces(DRM_PRIME_2)")?;
        Ok(surface)
    }

    pub fn destroy_surfaces(&self, surfaces: &mut [VASurfaceID]) {
        if surfaces.is_empty() {
            return;
        }
        // SAFETY: ids made on this display.
        unsafe { vaDestroySurfaces(self.dpy, surfaces.as_mut_ptr(), surfaces.len() as i32) };
    }

    pub fn create_context(
        &self,
        config: VAConfigID,
        width: u32,
        height: u32,
        targets: &mut [VASurfaceID],
    ) -> Result<VAContextID> {
        let mut context = 0;
        // SAFETY: `targets` outlives the call and its length is passed with it.
        let status = unsafe {
            vaCreateContext(
                self.dpy,
                config,
                width as i32,
                height as i32,
                VA_PROGRESSIVE as i32,
                targets.as_mut_ptr(),
                targets.len() as i32,
                &mut context,
            )
        };
        check(self.dpy, status, "vaCreateContext")?;
        Ok(context)
    }

    /// A parameter buffer holding one `T`.
    pub fn param_buffer<T>(
        &self,
        context: VAContextID,
        kind: VABufferType,
        value: &mut T,
    ) -> Result<VABufferID> {
        let mut id = 0;
        // SAFETY: libva copies `size_of::<T>()` bytes from `value` during the call.
        let status = unsafe {
            vaCreateBuffer(
                self.dpy,
                context,
                kind,
                size_of::<T>() as u32,
                1,
                ptr::from_mut(value).cast::<c_void>(),
                &mut id,
            )
        };
        check(self.dpy, status, "vaCreateBuffer")?;
        Ok(id)
    }

    /// A buffer of raw bytes, such as a header the application wrote.
    pub fn data_buffer(
        &self,
        context: VAContextID,
        kind: VABufferType,
        bytes: &[u8],
    ) -> Result<VABufferID> {
        let mut id = 0;
        // SAFETY: libva copies `bytes.len()` bytes during the call and does
        // not write through the pointer.
        let status = unsafe {
            vaCreateBuffer(
                self.dpy,
                context,
                kind,
                bytes.len() as u32,
                1,
                bytes.as_ptr().cast_mut().cast::<c_void>(),
                &mut id,
            )
        };
        check(self.dpy, status, "vaCreateBuffer(data)")?;
        Ok(id)
    }

    /// A miscellaneous encode parameter: libva's header followed by `T`.
    pub fn misc_buffer<T: Copy>(
        &self,
        context: VAContextID,
        kind: VAEncMiscParameterType,
        value: T,
    ) -> Result<VABufferID> {
        let size = size_of::<VAEncMiscParameterBuffer>() + size_of::<T>();
        let mut id = 0;
        // SAFETY: the buffer is created empty, then mapped and written within
        // its `size` bytes: the header first, `T` right after it.
        unsafe {
            let status = vaCreateBuffer(
                self.dpy,
                context,
                VAEncMiscParameterBufferType,
                size as u32,
                1,
                ptr::null_mut(),
                &mut id,
            );
            check(self.dpy, status, "vaCreateBuffer(misc)")?;
            let mut mapped = ptr::null_mut();
            check(
                self.dpy,
                vaMapBuffer(self.dpy, id, &mut mapped),
                "vaMapBuffer(misc)",
            )?;
            let header = mapped.cast::<VAEncMiscParameterBuffer>();
            (*header).type_ = kind;
            mapped
                .cast::<u8>()
                .add(size_of::<VAEncMiscParameterBuffer>())
                .cast::<T>()
                .write_unaligned(value);
            check(self.dpy, vaUnmapBuffer(self.dpy, id), "vaUnmapBuffer(misc)")?;
        }
        Ok(id)
    }

    pub fn coded_buffer(&self, context: VAContextID, size: usize) -> Result<VABufferID> {
        let mut id = 0;
        // SAFETY: an output buffer the driver fills; no data is passed in.
        let status = unsafe {
            vaCreateBuffer(
                self.dpy,
                context,
                VAEncCodedBufferType,
                size as u32,
                1,
                ptr::null_mut(),
                &mut id,
            )
        };
        check(self.dpy, status, "vaCreateBuffer(coded)")?;
        Ok(id)
    }

    pub fn destroy_buffer(&self, id: VABufferID) {
        // SAFETY: an id made on this display.
        unsafe { vaDestroyBuffer(self.dpy, id) };
    }

    /// Submits one picture: begin, the parameter buffers, end. The buffers are
    /// destroyed afterwards, as libva asks of buffers passed to render.
    pub fn submit(
        &self,
        context: VAContextID,
        target: VASurfaceID,
        buffers: &mut [VABufferID],
    ) -> Result<()> {
        // SAFETY: ids made on this display; the list's length is passed with it.
        let result = unsafe {
            check(
                self.dpy,
                vaBeginPicture(self.dpy, context, target),
                "vaBeginPicture",
            )
            .and_then(|()| {
                check(
                    self.dpy,
                    vaRenderPicture(
                        self.dpy,
                        context,
                        buffers.as_mut_ptr(),
                        buffers.len() as i32,
                    ),
                    "vaRenderPicture",
                )
            })
            .and_then(|()| check(self.dpy, vaEndPicture(self.dpy, context), "vaEndPicture"))
        };
        for &id in buffers.iter() {
            self.destroy_buffer(id);
        }
        result
    }

    pub fn sync(&self, surface: VASurfaceID) -> Result<()> {
        // SAFETY: a surface made on this display.
        check(
            self.dpy,
            unsafe { vaSyncSurface(self.dpy, surface) },
            "vaSyncSurface",
        )
    }

    /// Copies an encoded picture out of its coded buffer into `out`.
    pub fn read_coded(&self, coded: VABufferID, out: &mut Vec<u8>) -> Result<()> {
        out.clear();
        // SAFETY: a mapped coded buffer is a list of segments, each with `size`
        // valid bytes at `buf`, until it is unmapped below.
        unsafe {
            let mut mapped = ptr::null_mut();
            check(
                self.dpy,
                vaMapBuffer(self.dpy, coded, &mut mapped),
                "vaMapBuffer(coded)",
            )?;
            let mut segment = mapped.cast::<VACodedBufferSegment>();
            while !segment.is_null() {
                let s = &*segment;
                out.extend_from_slice(std::slice::from_raw_parts(
                    s.buf.cast::<u8>(),
                    s.size as usize,
                ));
                segment = s.next.cast::<VACodedBufferSegment>();
            }
            check(
                self.dpy,
                vaUnmapBuffer(self.dpy, coded),
                "vaUnmapBuffer(coded)",
            )?;
        }
        Ok(())
    }
}

impl Drop for Va {
    fn drop(&mut self) {
        // SAFETY: the display was initialized in `open`.
        unsafe { vaTerminate(self.dpy) };
    }
}

fn int_attrib(type_: VASurfaceAttribType, value: i32) -> VASurfaceAttrib {
    VASurfaceAttrib {
        type_,
        flags: VA_SURFACE_ATTRIB_SETTABLE,
        value: VAGenericValue {
            type_: VAGenericValueTypeInteger,
            value: _VAGenericValue__bindgen_ty_1 { i: value },
        },
    }
}

/// The DRM format code of a libva pixel format a capture can deliver.
pub fn drm_format(pixel: u32) -> Result<u32> {
    Ok(match pixel {
        BGRX => fourcc(b"XR24"),
        BGRA => fourcc(b"AR24"),
        RGBX => fourcc(b"XB24"),
        RGBA => fourcc(b"AB24"),
        NV12 | P010 => pixel,
        other => bail!("no DRM format for pixel format 0x{other:08x}"),
    })
}
