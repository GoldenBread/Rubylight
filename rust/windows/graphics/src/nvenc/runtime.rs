//! NVENC driver DLL loading (`nvEncodeAPI64.dll`), status errors, session limits,
//! and D3D12 fence synchronization helpers.

use super::SUCCESS;
use crate::capture::Device;
use anyhow::{Context, Result, bail};
use butterpollo_media_policy::nvenc::ApiVersion;
use butterpollo_win_abi::nvenc::*;
use std::{
    ffi::{CStr, c_void},
    ptr,
    rc::Rc,
    sync::Arc,
};
use windows::{
    Win32::{
        Foundation::HANDLE,
        Graphics::Direct3D12::{D3D12_FENCE_FLAG_NONE, ID3D12Fence},
        System::{
            LibraryLoader::LOAD_LIBRARY_SEARCH_SYSTEM32, SystemInformation::GetSystemDirectoryW,
        },
    },
    core::Interface,
};

type CreateInstance = unsafe extern "C" fn(*mut NV_ENCODE_API_FUNCTION_LIST) -> NVENCSTATUS;
type GetMaximum = unsafe extern "C" fn(*mut u32) -> NVENCSTATUS;

pub(crate) fn system_library(name: &str) -> Result<libloading::Library> {
    let mut directory = [0u16; 32768];
    // SAFETY: `directory` is a live, writable UTF-16 buffer and the call writes at most its length.
    let length = unsafe { GetSystemDirectoryW(Some(&mut directory)) } as usize;
    if length == 0 || length >= directory.len() {
        bail!("cannot locate the Windows system directory");
    }
    let path = std::path::PathBuf::from(String::from_utf16(&directory[..length])?).join(name);
    // SAFETY: `path` is an absolute System32 path, so no search-order DLL is picked up; loading
    // runs only the installed driver's initialisers.
    Ok(unsafe {
        libloading::os::windows::Library::load_with_flags(path, LOAD_LIBRARY_SEARCH_SYSTEM32.0)?
    }
    .into())
}
pub(super) struct Runtime {
    pub(super) _library: Option<libloading::Library>,
    pub(super) create: CreateInstance,
    pub(super) maximum: ApiVersion,
}
impl Runtime {
    pub(super) fn load() -> Result<Arc<Self>> {
        // SAFETY: `library` moves into the Runtime, keeping the copied `create` pointer loaded;
        // both symbols use NVIDIA's documented C signatures.
        unsafe {
            let library =
                system_library("nvEncodeAPI64.dll").context("NVIDIA encode driver unavailable")?;
            let create = *library.get::<CreateInstance>(b"NvEncodeAPICreateInstance\0")?;
            let maximum = if let Ok(get) =
                library.get::<GetMaximum>(b"NvEncodeAPIGetMaxSupportedVersion\0")
            {
                let mut value = 0;
                check_raw(
                    get(&mut value),
                    "query maximum API",
                    &NV_ENCODE_API_FUNCTION_LIST::default(),
                    ptr::null_mut(),
                )?;
                ApiVersion::from_driver(value)
            } else {
                ApiVersion::COMPILED
            };
            Ok(Arc::new(Self {
                _library: Some(library),
                create,
                maximum,
            }))
        }
    }
}
#[derive(Debug)]
pub(super) struct DriverError {
    status: NVENCSTATUS,
    operation: &'static str,
    detail: String,
}
impl std::fmt::Display for DriverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "NVENC {} failed ({}): {}",
            self.operation, self.status, self.detail
        )
    }
}
impl std::error::Error for DriverError {}
/// The driver refused another encode session: GeForce drivers cap the
/// sessions open at once and answer `NV_ENC_ERR_OUT_OF_MEMORY` or
/// `NV_ENC_ERR_INCOMPATIBLE_CLIENT_KEY` past the cap. Another driver API,
/// another device or a driver update does not help.
/// `NV_ENC_ERR_OUT_OF_MEMORY` also means the GPU's memory is exhausted,
/// so `memory` names both.
#[derive(Debug)]
pub struct SessionLimit {
    pub memory: bool,
}
impl std::fmt::Display for SessionLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.memory {
            f.write_str("NVENC session limit reached or GPU memory exhausted (another app such as OBS, ShadowPlay or Discord may hold the encoder sessions, or a game the GPU's memory)")
        } else {
            f.write_str("NVENC session limit reached (another app such as OBS, ShadowPlay or Discord holds the encoder sessions)")
        }
    }
}
/// `error` comes from the driver's session limit ([`SessionLimit`]).
pub fn session_limit(error: &anyhow::Error) -> bool {
    error.downcast_ref::<SessionLimit>().is_some()
}

pub(super) fn check_raw(
    status: NVENCSTATUS,
    operation: &'static str,
    functions: &NV_ENCODE_API_FUNCTION_LIST,
    raw: *mut c_void,
) -> Result<()> {
    if status == SUCCESS {
        return Ok(());
    }
    let detail = if !raw.is_null() {
        functions
            .nvEncGetLastErrorString
            .and_then(|get| {
                // SAFETY: `raw` is non-null and every caller passes the live session that
                // `functions` was created for.
                let message = unsafe { get(raw) };
                (!message.is_null()).then(|| {
                    // SAFETY: The driver returns a NUL-terminated string that stays valid until the
                    // next call on `raw`.
                    unsafe { CStr::from_ptr(message) }
                        .to_string_lossy()
                        .into_owned()
                })
            })
            .unwrap_or_default()
    } else {
        String::new()
    };
    Err(DriverError {
        status,
        operation,
        detail,
    }
    .into())
}
pub(super) fn retry_version(error: &anyhow::Error) -> bool {
    error.downcast_ref::<DriverError>().is_some_and(|e| {
        matches!(
            e.status,
            _NVENCSTATUS_NV_ENC_ERR_INVALID_VERSION | _NVENCSTATUS_NV_ENC_ERR_UNSUPPORTED_PARAM
        )
    })
}

/// The fence a D3D12 session's driver signals once a frame's bitstream is
/// written ([`Session::open`]).
pub(super) trait Fence {
    fn raw(&self) -> *mut c_void;
    /// The completed value; fails once the device is removed.
    fn completed(&self) -> Result<u64>;
    /// Set `event` once the fence reaches `value`.
    fn notify(&self, value: u64, event: HANDLE) -> Result<()>;
}
impl Fence for ID3D12Fence {
    fn raw(&self) -> *mut c_void {
        self.as_raw()
    }
    fn notify(&self, value: u64, event: HANDLE) -> Result<()> {
        // SAFETY: The fence is live and `event` an open event its owner keeps
        // until the session (which holds this fence) is gone or rearms it.
        unsafe { self.SetEventOnCompletion(value, event) }.context("NVENC D3D12 output fence event")
    }
    fn completed(&self) -> Result<u64> {
        // SAFETY: The fence is a live D3D12 fence; GetCompletedValue only reads it.
        let value = unsafe { self.GetCompletedValue() };
        if value == u64::MAX {
            bail!("the D3D12 device was removed while NVENC encoded");
        }
        Ok(value)
    }
}

/// A D3D12 fence point: the driver waits for `value` before it reads the
/// input, or signals it once the output is written.
pub(super) fn fence_point(
    api: ApiVersion,
    fence: *mut c_void,
    value: u64,
    signal: bool,
) -> NV_ENC_FENCE_POINT_D3D12 {
    let mut point = NV_ENC_FENCE_POINT_D3D12 {
        version: api.structure(1, false),
        pFence: fence,
        ..Default::default()
    };
    if signal {
        point.signalValue = value;
        point.set_bSignal(1);
    } else {
        point.waitValue = value;
        point.set_bWait(1);
    }
    point
}

/// The DirectX device a session opens on: the D3D12 conversion's, with an
/// output fence of its own, or the capture device.
pub(super) fn directx_device(
    converter: Option<&crate::compute::Converter>,
    gpu: &Device,
) -> Result<(*mut c_void, Option<Rc<dyn Fence>>)> {
    let Some(converter) = converter else {
        return Ok((gpu.device.as_raw(), None));
    };
    let device = &converter.compute().device;
    // SAFETY: `device` is the compute queue's live D3D12 device; the fence is owned.
    let fence: ID3D12Fence = unsafe { device.CreateFence(0, D3D12_FENCE_FLAG_NONE) }
        .context("NVENC D3D12 output fence")?;
    Ok((device.as_raw(), Some(Rc::new(fence) as Rc<dyn Fence>)))
}
