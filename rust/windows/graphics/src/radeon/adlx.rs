//! The FFI side of the probe. It walks `IADLX3DSettingsServices` (and its 1, 2 and 3 versions) and
//! copies what each getter says into `model::GroupRaw`; `model` decides what it means.
//!
//! Setter slots are bound as callable functions only for the settings `control` may write; every
//! other slot is a plain `usize`, so it cannot be called from here. Nothing in `probe` writes:
//! only `AdlxBackend::write` reaches a setter.
//!
//! The vtables are bound by hand from AMD's public C ABI, SDK 2.0.0.125:
//! https://github.com/GPUOpen-LibrariesAndSDKs/ADLX/tree/32b5a740d42295c5dfe9026b9f52683da0f3af91/SDK/Include
//! (`I3DSettings.h`, `I3DSettings1.h`, `I3DSettings2.h`, `I3DSettings3.h`, `ISystem.h`,
//! `ISystem2.h`). Loading, initialisation and reference counting reuse `telemetry`.
use super::{
    control::{BackendError, RadeonBackend},
    model::*,
};
use crate::telemetry::{
    ADLX_VERSION, BaseVtable, GetBool, GetGpuInterface, GetInt, GetInterface, Handle, Interface,
    ListVtable,
};
use anyhow::{Context, Result};
use serde::Serialize;
use std::{
    ffi::{CStr, c_char},
    ptr,
};

macro_rules! table {
    ($open:expr, $ty:ty) => {{
        let ready = $open.ready();
        // SAFETY: the interface was returned by the Get call whose ADLX type has this layout.
        let table: Option<&$ty> = ready.map(|i| unsafe { i.vtable::<$ty>() });
        (ready, table)
    }};
}

mod read;
mod vtable;
mod write;

use read::groups;
use vtable::*;
use write::write_setting;

#[derive(Clone, Debug, Serialize)]
pub struct GpuReport {
    pub id: i32,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub driver_path: Option<String>,
    pub driver: DriverInfo,
    pub settings: Vec<Setting>,
}
#[derive(Clone, Debug, Default, Serialize)]
pub struct DriverInfo {
    /// `IADLXGPU2::AMDSoftwareVersion`, for example the Adrenalin release.
    pub amd_software_version: Option<String>,
    pub amd_software_edition: Option<String>,
    /// `IADLXGPU2::DriverVersion`.
    pub driver_version: Option<String>,
    /// `IADLXGPU2::AMDWindowsDriverVersion`.
    pub windows_driver_version: Option<String>,
}
#[derive(Clone, Debug, Serialize)]
pub struct AdlxInfo {
    /// Installed runtime (`amdadlx64.dll`), from `ADLXQueryFullVersion`.
    pub runtime_version: Option<String>,
    /// What the probe asked `ADLXInitialize` for.
    pub requested_version: String,
    /// The headers the vtables were bound from.
    pub header_sdk_version: &'static str,
    pub initialize: String,
    /// The highest `IADLX3DSettingsServices` version the runtime answered for.
    pub services_interface: Option<&'static str>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Probe {
    pub adlx: AdlxInfo,
    pub gpus: Vec<GpuReport>,
}

fn version_text(full: u64) -> String {
    format!(
        "{}.{}.{}.{}",
        full >> 48,
        (full >> 32) & 0xffff,
        (full >> 16) & 0xffff,
        full & 0xffff
    )
}

/// An initialised ADLX with the 3D settings services and the GPU list. It holds the process-wide
/// ADLX lock (`crate::adlx_system`) until it is dropped, so a read or write never overlaps
/// telemetry's or another session's.
struct Session {
    // Fields drop in this order: interfaces, then the lock.
    gpus: Vec<(i32, Interface)>,
    v3: Option<Interface>,
    v2: Option<Interface>,
    v1: Option<Interface>,
    services: Interface,
    info: AdlxInfo,
    _runtime: crate::adlx_system::Guard,
}
impl Session {
    fn open() -> Result<Self> {
        let runtime = crate::adlx_system::acquire()?;
        // SAFETY: This export and its C signature are specified by AMD's ADLX.h.
        let full_version = unsafe {
            runtime
                .library()
                .get::<unsafe extern "C" fn(*mut u64) -> i32>(b"ADLXQueryFullVersion\0")
                .ok()
                .map(|f| *f)
        };
        let runtime_version = full_version.and_then(|query| {
            let mut full = 0u64;
            // SAFETY: The export writes one u64.
            (unsafe { query(&mut full) } == 0).then(|| version_text(full))
        });
        let system = runtime.system();
        let result = runtime.initialize_result();
        // SAFETY: The shared runtime returned this IADLXSystem and never terminates it.
        let table = unsafe { &**system.cast::<*const SystemVt>() };
        let list = obtain(|out| {
            // SAFETY: The system and output are live; GetGPUs returns IADLXGPUList.
            unsafe { (table.gpus)(system, out) }
        })
        .map_err(|fault| anyhow::anyhow!("GetGPUs: {}", fault.describe()))?;
        let services = obtain(|out| {
            // SAFETY: The system and output are live; Get3DSettingsServices returns IADLX3DSettingsServices.
            unsafe { (table.services_3d)(system, out) }
        })
        .map_err(|fault| anyhow::anyhow!("Get3DSettingsServices: {}", fault.describe()))
        .context("ADLX 3D settings services unavailable")?;
        let v1 = query(&services, "IADLX3DSettingsServices1").ok();
        let v2 = query(&services, "IADLX3DSettingsServices2").ok();
        let v3 = query(&services, "IADLX3DSettingsServices3").ok();
        let services_interface = [
            (v3.is_some(), "IADLX3DSettingsServices3"),
            (v2.is_some(), "IADLX3DSettingsServices2"),
            (v1.is_some(), "IADLX3DSettingsServices1"),
            (true, "IADLX3DSettingsServices"),
        ]
        .into_iter()
        .find_map(|(present, name)| present.then_some(name));

        // SAFETY: The list was returned by GetGPUs as IADLXGPUList.
        let list_table = unsafe { list.vtable::<ListVtable>() };
        // SAFETY: The list is live.
        let count = unsafe { (list_table.size)(list.0) };
        let mut gpus = vec![];
        for index in 0..count {
            let Ok(gpu) = obtain(|out| {
                // SAFETY: index is within the list; At_GPUList acquires a reference into `out`.
                unsafe { (list_table.at)(list.0, index, out) }
            }) else {
                continue;
            };
            // SAFETY: `gpu` is an IADLXGPU.
            let gpu_table = unsafe { gpu.vtable::<GpuVt>() };
            let mut id = 0;
            // SAFETY: The getter belongs to the live GPU; `id` is a live scalar.
            if unsafe { (gpu_table.unique_id)(gpu.0, &mut id) } != 0 {
                continue;
            }
            gpus.push((id, gpu));
        }
        Ok(Self {
            gpus,
            v3,
            v2,
            v1,
            services,
            info: AdlxInfo {
                runtime_version,
                requested_version: version_text(ADLX_VERSION),
                header_sdk_version: "2.0.0.125",
                initialize: result_name(result),
                services_interface,
            },
            _runtime: runtime,
        })
    }

    fn ctx<'a>(&'a self, gpu: &'a Interface) -> Ctx<'a> {
        Ctx {
            gpu,
            v0: &self.services,
            v1: self.v1.as_ref(),
            v2: self.v2.as_ref(),
            v3: self.v3.as_ref(),
        }
    }

    fn settings(&self, gpu: &Interface) -> Vec<Setting> {
        let mut settings: Vec<Setting> = groups(&self.ctx(gpu))
            .iter()
            .flat_map(classify_group)
            .collect();
        apply_dependencies(&mut settings);
        settings
    }

    fn gpu(&self, id: i32) -> Result<&Interface> {
        self.gpus
            .iter()
            .find(|(gpu, _)| *gpu == id)
            .map(|(_, gpu)| gpu)
            .with_context(|| format!("ADLX reports no GPU with id {id}"))
    }
}

fn driver_info(gpu: &Interface) -> DriverInfo {
    let Ok(gpu2) = query(gpu, "IADLXGPU2") else {
        return DriverInfo::default();
    };
    // SAFETY: `gpu2` is an IADLXGPU2, whose layout extends IADLXGPU.
    let t = unsafe { gpu2.vtable::<Gpu2Vt>() };
    DriverInfo {
        amd_software_version: get_text(&gpu2, t.software_version),
        amd_software_edition: get_text(&gpu2, t.edition),
        driver_version: get_text(&gpu2, t.driver_version),
        windows_driver_version: get_text(&gpu2, t.windows_driver_version),
    }
}

/// Query ADLX for the 3D settings of every AMD GPU. Reads only; no setting is written.
pub fn probe() -> Result<Probe> {
    let session = Session::open()?;
    let mut gpus = vec![];
    for (id, gpu) in &session.gpus {
        // SAFETY: `gpu` is an IADLXGPU.
        let gpu_table = unsafe { gpu.vtable::<GpuVt>() };
        gpus.push(GpuReport {
            id: *id,
            name: get_text(gpu, gpu_table.name).unwrap_or_default(),
            driver_path: get_text(gpu, gpu_table.driver_path),
            driver: driver_info(gpu),
            settings: session.settings(gpu),
        });
    }
    Ok(Probe {
        adlx: session.info.clone(),
        gpus,
    })
}

/// The AMD GPUs ADLX lists, by id, name and driver. No setting is read.
pub fn gpus() -> Result<Vec<super::control::GpuInfo>> {
    let session = Session::open()?;
    Ok(session
        .gpus
        .iter()
        .map(|(id, gpu)| {
            // SAFETY: `gpu` is an IADLXGPU.
            let gpu_table = unsafe { gpu.vtable::<GpuVt>() };
            let driver = driver_info(gpu);
            super::control::GpuInfo {
                id: *id,
                name: get_text(gpu, gpu_table.name).unwrap_or_default(),
                driver: driver
                    .amd_software_version
                    .or(driver.driver_version)
                    .unwrap_or_default(),
            }
        })
        .collect())
}

/// AMD Software's version (`26.10.1`) as ADLX reports it
/// (`IADLXGPU2::AMDSoftwareVersion`), from the first GPU that has one; never
/// the driver's own version. None when no GPU reports it.
pub fn amd_software_version() -> Result<Option<String>> {
    let session = Session::open()?;
    Ok(session.gpus.iter().find_map(|(_, gpu)| {
        driver_info(gpu)
            .amd_software_version
            .filter(|version| !version.trim().is_empty())
    }))
}

/// Reads and writes through ADLX. Every call opens its own session, so nothing is held between
/// calls and the host's telemetry keeps its own ADLX.
pub struct AdlxBackend;
impl RadeonBackend for AdlxBackend {
    fn read(&mut self, gpu: i32) -> Result<Vec<Setting>, BackendError> {
        let session = Session::open().map_err(|e| BackendError(format!("{e:#}")))?;
        let gpu = session
            .gpu(gpu)
            .map_err(|e| BackendError(format!("{e:#}")))?;
        Ok(session.settings(gpu))
    }
    fn write(&mut self, gpu: i32, id: &str, value: &Value) -> Result<(), BackendError> {
        let session = Session::open().map_err(|e| BackendError(format!("{e:#}")))?;
        let gpu = session
            .gpu(gpu)
            .map_err(|e| BackendError(format!("{e:#}")))?;
        write_setting(&session.ctx(gpu), id, value).map_err(BackendError)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_version_formats_as_four_parts() {
        assert_eq!(version_text(ADLX_VERSION), "1.0.5.30");
        assert_eq!(version_text((2 << 48) | (1 << 16) | 125), "2.0.1.125");
    }

    /// Needs an AMD GPU with the Radeon driver installed. Read-only.
    #[test]
    #[ignore = "needs an AMD GPU and the Radeon driver"]
    fn probe_reads_every_3d_setting_without_writing() {
        let probe = probe().expect("ADLX probe");
        assert!(!probe.gpus.is_empty(), "no AMD GPU reported");
        for gpu in &probe.gpus {
            assert!(gpu.settings.len() >= 20, "{} settings", gpu.settings.len());
            for setting in &gpu.settings {
                assert!(setting.adlx.contains_key("get_interface"), "{}", setting.id);
            }
        }
    }
}
