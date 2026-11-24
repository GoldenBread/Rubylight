//! MMDevice endpoint enumeration and undocumented `IPolicyConfig` COM bindings.
use anyhow::{Result, bail};
use serde::Serialize;
use std::ffi::c_void;
use windows::{
    Win32::{
        Foundation::PROPERTYKEY,
        Media::Audio::*,
        System::Com::{StructuredStorage::*, *},
    },
    core::{GUID, HRESULT, IUnknown, IUnknown_Vtbl, Interface, PCWSTR},
};

pub(super) const ROLES: [ERole; 3] = [eConsole, eMultimedia, eCommunications];
const FRIENDLY: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
    pid: 14,
};
/// PKEY_Device_DeviceDesc: the endpoint's own name, e.g. "Speakers".
const DESCRIPTION: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
    pid: 2,
};
/// PKEY_DeviceInterface_FriendlyName: the adapter, e.g. "Steam Streaming Speakers".
const ADAPTER: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0x026e516e_b814_414b_83cd_856d6fef4822),
    pid: 2,
};

#[derive(Clone, Serialize)]
pub struct Endpoint {
    pub id: String,
    pub name: String,
    pub description: String,
    pub adapter: String,
    pub default: bool,
    pub virtual_sink: bool,
}

pub(crate) unsafe fn device_id(device: &IMMDevice) -> Result<String> {
    // SAFETY: `device` is a live IMMDevice borrowed for the call.
    let id = unsafe { device.GetId()? };
    // SAFETY: GetId returned a NUL-terminated string that is not freed until below.
    let value = unsafe { id.to_string() };
    // SAFETY: `id` came from CoTaskMemAlloc in GetId and is freed once, after its last use.
    unsafe {
        CoTaskMemFree(Some(id.0.cast()));
    }
    Ok(value?)
}

pub(super) fn enumerator() -> Result<IMMDeviceEnumerator> {
    // SAFETY: CoCreateInstance only reads the static CLSID, and fails cleanly when COM is not
    // initialised.
    Ok(unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? })
}

pub fn endpoints() -> Result<Vec<Endpoint>> {
    endpoints_for(eRender)
}

/// Active endpoints of one direction; `default` is the console default.
pub(super) fn endpoints_for(flow: EDataFlow) -> Result<Vec<Endpoint>> {
    let _com = crate::os_helpers::ComGuard::new()?;
    let enumerator = enumerator()?;
    // SAFETY: COM is initialised on this thread by `_com`, every interface used is live, and each
    // string from PropVariantToStringAlloc is freed once after it is copied.
    unsafe {
        let default = enumerator
            .GetDefaultAudioEndpoint(flow, eConsole)
            .ok()
            .and_then(|d| device_id(&d).ok());
        let list = enumerator.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE)?;
        let mut endpoints = Vec::new();
        for index in 0..list.GetCount()? {
            let device = list.Item(index)?;
            let id = device_id(&device)?;
            let store = device.OpenPropertyStore(STGM_READ).ok();
            let text = |key: &PROPERTYKEY| {
                store.as_ref().and_then(|store| {
                    let mut property = store.GetValue(key).ok()?;
                    let text = PropVariantToStringAlloc(&property).ok().and_then(|text| {
                        let result = text.to_string().ok();
                        CoTaskMemFree(Some(text.0.cast()));
                        result
                    });
                    let _ = PropVariantClear(&mut property);
                    text
                })
            };
            let name = text(&FRIENDLY).unwrap_or_else(|| id.clone());
            let description = text(&DESCRIPTION).unwrap_or_default();
            let adapter = text(&ADAPTER).unwrap_or_default();
            let virtual_sink = [&name, &adapter]
                .iter()
                .any(|n| n.to_ascii_lowercase().contains("steam streaming speakers"));
            endpoints.push(Endpoint {
                default: default.as_ref() == Some(&id),
                id,
                name,
                description,
                adapter,
                virtual_sink,
            });
        }
        Ok(endpoints)
    }
}

// PolicyConfig is the Windows interface used by the retained host. Keep its
// actual COM slot layout, including the unused methods before SetDefault.
#[repr(transparent)]
#[derive(Clone)]
pub(super) struct Policy(IUnknown);
#[repr(C)]
pub(super) struct PolicyVtbl {
    unknown: IUnknown_Vtbl,
    mix: usize,
    device_format:
        unsafe extern "system" fn(*mut c_void, PCWSTR, i32, *mut *mut WAVEFORMATEX) -> HRESULT,
    reset_format: usize,
    set_format: unsafe extern "system" fn(
        *mut c_void,
        PCWSTR,
        *const WAVEFORMATEX,
        *const WAVEFORMATEX,
    ) -> HRESULT,
    unused: [usize; 6],
    pub(super) default: unsafe extern "system" fn(*mut c_void, PCWSTR, ERole) -> HRESULT,
    visibility: usize,
}
// SAFETY: PolicyVtbl mirrors IPolicyConfig's slot layout (the offset test checks it) and Policy is
// a transparent IUnknown, so the IID names an interface with exactly this vtable.
unsafe impl Interface for Policy {
    type Vtable = PolicyVtbl;
    const IID: GUID = GUID::from_u128(0xf8679f50_850a_41cf_9c72_430f290290c8);
}
impl Policy {
    pub(super) fn new() -> Result<Self> {
        // SAFETY: CoCreateInstance only reads the static CLSID and returns an owned interface or an
        // error.
        Ok(unsafe {
            CoCreateInstance(
                &GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9),
                None,
                CLSCTX_ALL,
            )?
        })
    }
    pub(super) fn set_default(&self, id: &str, role: ERole) -> Result<()> {
        let id = crate::text::to_wide(id);
        // SAFETY: `self` is a live IPolicyConfig whose slot matches PolicyVtbl, and `id` is
        // NUL-terminated and outlives the call.
        unsafe {
            (self.vtable().default)(self.as_raw(), PCWSTR(id.as_ptr()), role).ok()?;
        }
        Ok(())
    }
    pub(super) fn format(&self, id: &str) -> Result<Vec<u8>> {
        let id = crate::text::to_wide(id);
        let mut format = std::ptr::null_mut();
        // SAFETY: `format` receives a CoTaskMemAlloc'd WAVEFORMATEX that is read only within its
        // header plus cbSize bytes and freed once.
        unsafe {
            (self.vtable().device_format)(self.as_raw(), PCWSTR(id.as_ptr()), 0, &mut format)
                .ok()?;
            if format.is_null() {
                bail!("audio endpoint returned no format");
            }
            let size = size_of::<WAVEFORMATEX>() + usize::from((*format).cbSize);
            let result = if size > 4096 {
                Err(anyhow::anyhow!("invalid audio endpoint format"))
            } else {
                Ok(std::slice::from_raw_parts(format.cast::<u8>(), size).to_vec())
            };
            CoTaskMemFree(Some(format.cast()));
            result
        }
    }
    pub(super) fn set_format(&self, id: &str, bytes: &[u8]) -> Result<()> {
        if bytes.len() < size_of::<WAVEFORMATEX>() || bytes.len() > 4096 {
            bail!("invalid saved audio format");
        }
        let id = crate::text::to_wide(id);
        // Stored format bytes need native alignment when passed back to COM.
        let mut storage = vec![0u64; bytes.len().div_ceil(8)];
        // SAFETY: `storage` is u64-aligned and at least `bytes.len()` long, and `id` and `empty`
        // outlive the call, whose slot matches PolicyVtbl.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), storage.as_mut_ptr().cast(), bytes.len());
            let empty = WAVEFORMATEXTENSIBLE::default();
            (self.vtable().set_format)(
                self.as_raw(),
                PCWSTR(id.as_ptr()),
                storage.as_ptr().cast(),
                (&empty as *const WAVEFORMATEXTENSIBLE).cast(),
            )
            .ok()?;
        }
        Ok(())
    }
}

/// The default playback endpoints for console, multimedia and
/// communications.
pub(crate) fn defaults() -> Result<[Option<String>; 3]> {
    defaults_for(eRender)
}
pub(super) fn defaults_for(flow: EDataFlow) -> Result<[Option<String>; 3]> {
    let enumerator = enumerator()?;
    // SAFETY: `enumerator` is a live interface and device_id only needs a live IMMDevice.
    Ok(ROLES.map(|role| unsafe {
        enumerator
            .GetDefaultAudioEndpoint(flow, role)
            .ok()
            .and_then(|d| device_id(&d).ok())
    }))
}
/// Playback and recording endpoints, to tell the host's own apart and to name
/// devices in the log. Empty where they cannot be listed: a restore still runs.
pub(super) fn all_endpoints() -> Vec<Endpoint> {
    let mut all = endpoints_for(eRender).unwrap_or_default();
    all.extend(endpoints_for(eCapture).unwrap_or_default());
    all
}
/// Steam Streaming Speakers and both sides of Steam Streaming Microphone: never
/// a device to restore, for any role or direction.
pub(super) fn host_owned(all: &[Endpoint], id: &str) -> bool {
    all.iter().any(|endpoint| {
        endpoint.id.eq_ignore_ascii_case(id)
            && butterpollo_media_policy::audio_defaults::host_owned(
                &endpoint.name,
                &endpoint.adapter,
            )
    })
}
pub(super) fn device_name(all: &[Endpoint], id: Option<&str>) -> String {
    let Some(id) = id else {
        return "none".into();
    };
    all.iter()
        .find(|endpoint| endpoint.id.eq_ignore_ascii_case(id))
        .map_or_else(|| id.to_string(), |endpoint| endpoint.name.clone())
}
pub(super) fn device_names(all: &[Endpoint], defaults: &[Option<String>; 3]) -> String {
    defaults
        .iter()
        .map(|id| device_name(all, id.as_deref()))
        .collect::<Vec<_>>()
        .join(" | ")
}
/// An endpoint by ID, friendly name, description or adapter name, in that
/// order, as Vibepollo matches audio_sink and virtual_sink.
pub(crate) fn find<'a>(endpoints: &'a [Endpoint], requested: &str) -> Option<&'a Endpoint> {
    let requested = requested.trim();
    let fields: [fn(&Endpoint) -> &str; 4] =
        [|e| &e.id, |e| &e.name, |e| &e.description, |e| &e.adapter];
    fields.iter().find_map(|field| {
        endpoints
            .iter()
            .find(|endpoint| field(endpoint).eq_ignore_ascii_case(requested))
    })
}
