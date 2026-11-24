//! The PyroWave C API (`pyrowave/pyrowave.h`, API 1.0, MIT, Hans-Kristian Arntzen) as the
//! Linux host loads it: `libpyrowave-shared.so` is opened at run time, so a host without it
//! still starts and answers "PyroWave unavailable". Vulkan types come from `ash::vk`; the
//! structures below mirror the header field for field.
//!
//! The library is the pinned SDK Nightjar's Linux client builds (`client/scripts/build-pyrowave-sdk.sh`
//! with the host's `rust/codec-patches/pyrowave`); see NOTICE and `rust/THIRD_PARTY.md`.
#![allow(non_camel_case_types, clippy::too_many_arguments)]

use anyhow::{Context, Result, bail};
use ash::vk;
use libloading::Library;
use std::{
    ffi::{c_int, c_void},
    path::PathBuf,
};

pub type Device = *mut c_void;
pub type Encoder = *mut c_void;
pub type Image = *mut c_void;

pub const SUCCESS: i32 = 0;
pub const CPU_FORMAT_YUV420P: u32 = 1;
pub const CPU_FORMAT_YUV444P: u32 = 2;

#[repr(C)]
pub struct Uuid(pub [u8; 16]);
#[repr(C)]
pub struct Luid(pub [u8; 8]);

#[repr(C)]
pub struct ImageCreateInfo {
    pub device: Device,
    pub external_handle: usize,
    pub handle_type: vk::ExternalMemoryHandleTypeFlags,
    pub image_create_info: *const vk::ImageCreateInfo<'static>,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ImageView {
    pub image: vk::Image,
    pub width: u32,
    pub height: u32,
    pub image_format: vk::Format,
    pub view_format: vk::Format,
    pub mip_level: u32,
    pub layer: u32,
    pub aspect: vk::ImageAspectFlags,
    pub swizzle: vk::ComponentSwizzle,
    pub layout: vk::ImageLayout,
}

#[repr(C)]
pub struct EncoderCreateInfo {
    pub device: Device,
    pub width: c_int,
    pub height: c_int,
    pub chroma: c_int,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SyncPoint {
    pub semaphore: vk::Semaphore,
    pub value: u64,
}

#[repr(C)]
pub struct ExternalReference {
    pub image: Image,
    pub queue_family_index: u32,
}

#[repr(C)]
pub struct SyncOperation {
    pub images: *const ExternalReference,
    pub num_images: usize,
    pub sync: SyncPoint,
}

#[repr(C)]
pub struct ScaledEncodeInfo {
    pub view: ImageView,
    pub input_color_space: vk::ColorSpaceKHR,
    pub output_color_space: vk::ColorSpaceKHR,
    pub intermediate_plane_format: vk::Format,
    pub ycbcr_chroma_midpoint: f32,
    pub force_linear_filtering: bool,
    pub skip_dither: bool,
    pub crop_rect: *const vk::Rect2D,
    pub ycbcr_range: vk::SamplerYcbcrRange,
    pub ycbcr_range_bit_depth: u32,
}

#[repr(C)]
pub struct CpuBuffer {
    pub data: [*mut c_void; 3],
    pub row_stride_in_bytes: [usize; 3],
    pub plane_size_in_bytes: [usize; 3],
    pub width: c_int,
    pub height: c_int,
    pub format: u32,
}

#[repr(C)]
pub struct RateControl {
    pub maximum_bitstream_size: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Packet {
    pub offset: usize,
    pub size: usize,
}

macro_rules! api {
    ($($field:ident = $name:literal: $ty:ty;)*) => {
        /// The entry points of one loaded library.
        pub struct Api {
            _lib: Library,
            $(pub $field: $ty,)*
        }
        impl Api {
            fn bind(lib: Library) -> Result<Self> {
                // SAFETY: each symbol is declared with the signature `pyrowave.h` gives it, and
                // the library stays loaded for as long as the pointers do (`_lib`).
                unsafe {
                    Ok(Self {
                        $($field: *lib.get::<$ty>(concat!($name, "\0").as_bytes())
                            .with_context(|| concat!("the PyroWave library has no ", $name))?,)*
                        _lib: lib,
                    })
                }
            }
        }
    };
}

api! {
    get_api_version = "pyrowave_get_api_version": unsafe extern "C" fn(*mut u32, *mut u32, *mut u32);
    create_device_by_compat = "pyrowave_create_device_by_compat": unsafe extern "C" fn(
        u32, u32, *const Uuid, *const Uuid, *const Luid, vk::QueueGlobalPriorityKHR, *mut Device) -> i32;
    device_get_global_priority = "pyrowave_device_get_global_priority":
        unsafe extern "C" fn(Device) -> vk::QueueGlobalPriorityKHR;
    device_set_queue_type = "pyrowave_device_set_queue_type":
        unsafe extern "C" fn(Device, vk::QueueFlags) -> i32;
    device_destroy = "pyrowave_device_destroy": unsafe extern "C" fn(Device);
    image_create = "pyrowave_image_create": unsafe extern "C" fn(*const ImageCreateInfo, *mut Image) -> i32;
    image_get_image_view = "pyrowave_image_get_image_view":
        unsafe extern "C" fn(Image, vk::ImageAspectFlags, vk::ImageUsageFlags, *mut ImageView) -> i32;
    image_destroy = "pyrowave_image_destroy": unsafe extern "C" fn(Image);
    encoder_create = "pyrowave_encoder_create": unsafe extern "C" fn(*const EncoderCreateInfo, *mut Encoder) -> i32;
    encoder_encode_gpu_scaled = "pyrowave_encoder_encode_gpu_scaled": unsafe extern "C" fn(
        Encoder, *const SyncOperation, *const SyncOperation, *const ScaledEncodeInfo, *const RateControl) -> i32;
    encoder_encode_cpu = "pyrowave_encoder_encode_cpu":
        unsafe extern "C" fn(Encoder, *const CpuBuffer, *const RateControl) -> i32;
    encoder_compute_num_packets_with_padding = "pyrowave_encoder_compute_num_packets_with_padding":
        unsafe extern "C" fn(Encoder, usize, usize, *mut usize) -> i32;
    encoder_packetize_with_padding = "pyrowave_encoder_packetize_with_padding": unsafe extern "C" fn(
        Encoder, *mut Packet, usize, usize, *mut usize, *mut c_void, usize) -> i32;
    encoder_get_mapped_raw_bitstream = "pyrowave_encoder_get_mapped_raw_bitstream": unsafe extern "C" fn(
        Encoder, *mut *const c_void, *mut usize, *mut *const c_void, *mut usize) -> i32;
    encoder_destroy = "pyrowave_encoder_destroy": unsafe extern "C" fn(Encoder);
}

/// Where the library may be: `BUTTERPOLLO_PYROWAVE_LIB`, then next to the executable, then the
/// loader's own search.
fn candidates() -> Vec<PathBuf> {
    let mut found = Vec::new();
    if let Some(path) = std::env::var_os("BUTTERPOLLO_PYROWAVE_LIB") {
        found.push(PathBuf::from(path));
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(PathBuf::from))
    {
        found.push(dir.join("libpyrowave-shared.so"));
    }
    found.push(PathBuf::from("libpyrowave-shared.so"));
    found.push(PathBuf::from("libpyrowave-shared.so.1"));
    found
}

pub(super) fn open() -> Result<Library> {
    let mut errors = Vec::new();
    for path in candidates() {
        // SAFETY: loading the PyroWave library runs its (volk and static) initialisers only.
        match unsafe { Library::new(&path) } {
            Ok(lib) => return Ok(lib),
            Err(error) => errors.push(format!("{}: {error}", path.display())),
        }
    }
    bail!(
        "the PyroWave library (libpyrowave-shared.so) was not found ({})",
        errors.join("; ")
    )
}

impl Api {
    /// Loads the library and checks its API is 1.x.
    pub fn load() -> Result<Self> {
        let api = Self::bind(open()?)?;
        let (mut major, mut minor, mut patch) = (0, 0, 0);
        // SAFETY: three live out-pointers.
        unsafe { (api.get_api_version)(&mut major, &mut minor, &mut patch) };
        if major != 1 {
            bail!("PyroWave API {major}.{minor}.{patch} is not the 1.x this host was built for");
        }
        Ok(api)
    }
}

/// An error from a call.
pub fn check(code: i32) -> Result<()> {
    if code == SUCCESS {
        return Ok(());
    }
    let what = match code {
        -2 => "invalid argument",
        -3 => "out of host memory",
        -4 => "out of device memory",
        -5 => "no Vulkan",
        -6 => "not implemented",
        -7 => "unsupported external handle",
        -8 => "external handle failed",
        _ => "error",
    };
    bail!("PyroWave call failed: {what} ({code})")
}

/// Decoder entry points, for the tests that read back what the encoder wrote.
#[cfg(test)]
pub mod decode {
    use super::*;
    #[repr(C)]
    pub struct DecoderCreateInfo {
        pub device: Device,
        pub width: c_int,
        pub height: c_int,
        pub chroma: c_int,
        pub fragment_path: bool,
    }
    pub type Decoder = *mut c_void;
    pub struct Api {
        pub create: unsafe extern "C" fn(*const DecoderCreateInfo, *mut Decoder) -> i32,
        pub push_packet: unsafe extern "C" fn(Decoder, *const c_void, usize) -> i32,
        pub is_ready: unsafe extern "C" fn(Decoder, bool) -> bool,
        pub decode_cpu: unsafe extern "C" fn(Decoder, *const CpuBuffer) -> i32,
        pub destroy: unsafe extern "C" fn(Decoder),
    }
    impl Api {
        pub fn bind(lib: &Library) -> Result<Self> {
            // SAFETY: signatures as in `pyrowave.h`; `lib` outlives the test that uses these.
            unsafe {
                Ok(Self {
                    create: *lib.get(b"pyrowave_decoder_create\0")?,
                    push_packet: *lib.get(b"pyrowave_decoder_push_packet\0")?,
                    is_ready: *lib.get(b"pyrowave_decoder_decode_is_ready\0")?,
                    decode_cpu: *lib.get(b"pyrowave_decoder_decode_cpu_buffer_synchronous\0")?,
                    destroy: *lib.get(b"pyrowave_decoder_destroy\0")?,
                })
            }
        }
    }
}
