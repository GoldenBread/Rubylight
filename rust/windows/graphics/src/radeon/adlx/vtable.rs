//! ADLX 3D-settings C vtable layouts, COM/interface wrappers and layout offset tests.
//!
//! Bound from AMD's public C ABI, ADLX SDK 2.0.0.125
//! (`I3DSettings.h`, `I3DSettings1.h`, `I3DSettings2.h`, `I3DSettings3.h`,
//! `ISystem.h`, `ISystem2.h`).

use super::*;

/// A setter slot that is not bound. It is an integer so it cannot be called.
pub(super) type Unbound = usize;
pub(super) type SetBool = unsafe extern "system" fn(Handle, u8) -> i32;
pub(super) type SetInt = unsafe extern "system" fn(Handle, i32) -> i32;
pub(super) type GetRange = unsafe extern "system" fn(Handle, *mut RawRange) -> i32;
pub(super) type GetText = unsafe extern "system" fn(Handle, *mut *const c_char) -> i32;
#[repr(C)]
#[derive(Default)]
pub(super) struct RawRange {
    pub(super) min: i32,
    pub(super) max: i32,
    pub(super) step: i32,
}

#[repr(C)]
pub(super) struct SystemVt {
    pub(super) hybrid: Unbound,
    pub(super) gpus: GetInterface,
    /// GetDisplaysServices, GetDesktopsServices, GetGPUsChangedHandling, EnableLog.
    pub(super) unused: [usize; 5],
    pub(super) services_3d: GetInterface,
}
#[repr(C)]
pub(super) struct ServicesVt {
    pub(super) base: BaseVtable,
    pub(super) anti_lag: GetGpuInterface,
    pub(super) chill: GetGpuInterface,
    pub(super) boost: GetGpuInterface,
    pub(super) image_sharpening: GetGpuInterface,
    pub(super) enhanced_sync: GetGpuInterface,
    pub(super) vsync: GetGpuInterface,
    pub(super) frame_rate_target: GetGpuInterface,
    pub(super) anti_aliasing: GetGpuInterface,
    pub(super) morphological_aa: GetGpuInterface,
    pub(super) anisotropic_filtering: GetGpuInterface,
    pub(super) tessellation: GetGpuInterface,
    /// No GPU argument.
    pub(super) rsr: GetInterface,
    pub(super) reset_shader_cache: GetGpuInterface,
    pub(super) changed_handling: Unbound,
}
#[repr(C)]
pub(super) struct Services1Vt {
    pub(super) base: ServicesVt,
    /// No GPU argument.
    pub(super) afmf: GetInterface,
}
#[repr(C)]
pub(super) struct Services2Vt {
    pub(super) base: Services1Vt,
    pub(super) image_sharpen_desktop: GetGpuInterface,
}
#[repr(C)]
pub(super) struct Services3Vt {
    pub(super) base: Services2Vt,
    pub(super) fidelityfx_super_resolution: GetGpuInterface,
    pub(super) fidelityfx_frame_gen: GetGpuInterface,
}

#[repr(C)]
pub(super) struct GpuVt {
    pub(super) base: BaseVtable,
    /// VendorId, ASICFamilyType, Type, IsExternal.
    pub(super) unused1: [usize; 4],
    pub(super) name: GetText,
    pub(super) driver_path: GetText,
    /// PNPString .. SubSystemVendorId.
    pub(super) unused2: [usize; 9],
    pub(super) unique_id: GetInt,
}
#[repr(C)]
pub(super) struct Gpu2Vt {
    pub(super) base: GpuVt,
    /// PCIBusType .. AbortPowerOff, IsSupportedApplicationList, GetApplications.
    pub(super) unused: [usize; 10],
    pub(super) release_date: Unbound,
    pub(super) edition: GetText,
    pub(super) software_version: GetText,
    pub(super) driver_version: GetText,
    pub(super) windows_driver_version: GetText,
}

#[repr(C)]
pub(super) struct Head {
    pub(super) base: BaseVtable,
    pub(super) supported: GetBool,
}
#[repr(C)]
pub(super) struct ToggleVt {
    pub(super) head: Head,
    pub(super) enabled: GetBool,
    pub(super) set_enabled: SetBool,
}
#[repr(C)]
pub(super) struct AntiLag1Vt {
    pub(super) base: ToggleVt,
    pub(super) level: GetInt,
    pub(super) set_level: Unbound,
}
#[repr(C)]
pub(super) struct ChillVt {
    pub(super) head: Head,
    pub(super) enabled: GetBool,
    pub(super) range: GetRange,
    pub(super) min_fps: GetInt,
    pub(super) max_fps: GetInt,
    pub(super) set_enabled: SetBool,
    pub(super) set_min_fps: SetInt,
    pub(super) set_max_fps: SetInt,
}
/// Boost, image sharpening and frame rate target control share this layout.
#[repr(C)]
pub(super) struct RangedVt {
    pub(super) head: Head,
    pub(super) enabled: GetBool,
    pub(super) range: GetRange,
    pub(super) value: GetInt,
    pub(super) set_enabled: SetBool,
    pub(super) set_value: SetInt,
}
#[repr(C)]
pub(super) struct VsyncVt {
    pub(super) head: Head,
    pub(super) enabled: GetBool,
    pub(super) mode: GetInt,
    pub(super) set_mode: SetInt,
}
#[repr(C)]
pub(super) struct AntiAliasingVt {
    pub(super) head: Head,
    pub(super) mode: GetInt,
    pub(super) level: GetInt,
    pub(super) method: GetInt,
    pub(super) set_mode: SetInt,
    pub(super) set_level: SetInt,
    pub(super) set_method: SetInt,
}
#[repr(C)]
pub(super) struct AnisotropicVt {
    pub(super) head: Head,
    pub(super) enabled: GetBool,
    pub(super) level: GetInt,
    pub(super) set_enabled: SetBool,
    pub(super) set_level: SetInt,
}
#[repr(C)]
pub(super) struct TessellationVt {
    pub(super) head: Head,
    pub(super) mode: GetInt,
    pub(super) level: GetInt,
    pub(super) set_mode: SetInt,
    pub(super) set_level: SetInt,
}
#[repr(C)]
pub(super) struct RsrVt {
    pub(super) head: Head,
    pub(super) enabled: GetBool,
    pub(super) set_enabled: Unbound,
    pub(super) range: GetRange,
    pub(super) sharpness: GetInt,
    pub(super) set_sharpness: SetInt,
}
#[repr(C)]
pub(super) struct Afmf1Vt {
    pub(super) base: ToggleVt,
    pub(super) algorithm_supported: GetBool,
    pub(super) algorithm: GetInt,
    pub(super) set_algorithm: Unbound,
    pub(super) search_mode: GetInt,
    pub(super) set_search_mode: SetInt,
    pub(super) performance_mode: GetInt,
    pub(super) set_performance_mode: SetInt,
    pub(super) fast_motion: GetInt,
    pub(super) set_fast_motion: SetInt,
}
#[repr(C)]
pub(super) struct FrameGenVt {
    pub(super) base: BaseVtable,
    pub(super) ratios: GetInterface,
    pub(super) ratio: GetInt,
    pub(super) supported: GetBool,
    pub(super) enabled: GetBool,
    pub(super) set_enabled: Unbound,
    pub(super) set_ratio: Unbound,
}
#[repr(C)]
pub(super) struct RatioOptionVt {
    pub(super) base: BaseVtable,
    pub(super) ratio: GetInt,
}

pub(super) fn code(result: i32) -> Query<()> {
    if result == 0 {
        Ok(())
    } else {
        Err(Fault::Code(result))
    }
}
pub(super) fn get_bool(i: &Interface, f: GetBool) -> Query<bool> {
    let mut value = 0u8;
    // SAFETY: `f` is a getter of the live interface `i`; the output is a live scalar.
    code(unsafe { f(i.0, &mut value) }).map(|()| value != 0)
}
pub(super) fn get_int(i: &Interface, f: GetInt) -> Query<i32> {
    let mut value = 0;
    // SAFETY: `f` is a getter of the live interface `i`; the output is a live scalar.
    code(unsafe { f(i.0, &mut value) }).map(|()| value)
}
pub(super) fn get_range(i: &Interface, f: GetRange) -> Query<Range> {
    let mut raw = RawRange::default();
    // SAFETY: `f` is a range getter of the live interface `i`; the output is a live ADLX_IntRange.
    code(unsafe { f(i.0, &mut raw) }).map(|()| Range {
        min: raw.min.into(),
        max: raw.max.into(),
        step: raw.step.into(),
    })
}
pub(super) fn get_text(i: &Interface, f: GetText) -> Option<String> {
    let mut text = ptr::null();
    // SAFETY: `f` is a string getter of the live interface `i`; the string it returns is owned by
    // ADLX and stays valid while `i` lives, and is copied here.
    unsafe {
        (f(i.0, &mut text) == 0 && !text.is_null())
            .then(|| CStr::from_ptr(text).to_string_lossy().into_owned())
    }
}
pub(super) fn obtain(call: impl FnOnce(&mut Handle) -> i32) -> Query<Interface> {
    let mut out = ptr::null_mut();
    let result = call(&mut out);
    Interface::returned(result, out).ok_or(Fault::Code(if result == 0 { 3 } else { result }))
}
pub(super) fn query(i: &Interface, name: &str) -> Query<Interface> {
    let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    obtain(|out| {
        // SAFETY: Every ADLX interface starts with BaseVtable; the name is terminated and the
        // output is live.
        unsafe { (i.vtable::<BaseVtable>().query)(i.0, name.as_ptr(), out) }
    })
}

/// An interface that was asked for, and what IsSupported said.
pub(super) struct Open {
    pub(super) obtained: Query<()>,
    pub(super) supported: Option<Query<bool>>,
    pub(super) interface: Option<Interface>,
}
impl Open {
    pub(super) fn new(
        obtained: Query<Interface>,
        supported: impl FnOnce(&Interface) -> Query<bool>,
    ) -> Self {
        match obtained {
            Ok(interface) => Self {
                obtained: Ok(()),
                supported: Some(supported(&interface)),
                interface: Some(interface),
            },
            Err(fault) => Self {
                obtained: Err(fault),
                supported: None,
                interface: None,
            },
        }
    }
    /// Every interface but FrameGen starts with `Head`.
    pub(super) fn standard(obtained: Query<Interface>) -> Self {
        Self::new(obtained, |i| {
            // SAFETY: `i` is an IADLX3D* interface whose first method after IADLXInterface is IsSupported.
            get_bool(i, unsafe { i.vtable::<Head>() }.supported)
        })
    }
    /// An interface reached through QueryInterface on another one.
    pub(super) fn extension(parent: &Open, name: &str) -> Self {
        match (&parent.interface, &parent.obtained) {
            (Some(parent), _) => Self::standard(query(parent, name)),
            (None, Err(fault)) => Self::standard(Err(*fault)),
            (None, Ok(())) => Self::standard(Err(Fault::Missing)),
        }
    }
    /// The interface, only when ADLX said the feature is supported. Reading an unsupported
    /// feature proves nothing, so the getters are not called otherwise.
    pub(super) fn ready(&self) -> Option<&Interface> {
        match self.supported {
            Some(Ok(true)) => self.interface.as_ref(),
            _ => None,
        }
    }
    pub(super) fn group(
        self,
        group: &'static str,
        interface: &'static str,
        fields: Vec<FieldRaw>,
    ) -> GroupRaw {
        GroupRaw {
            group,
            interface,
            obtained: self.obtained,
            supported: self.supported,
            fields,
        }
    }
}

/// Everything the probe needs to reach the 3D settings of one GPU.
pub(super) struct Ctx<'a> {
    pub(super) gpu: &'a Interface,
    pub(super) v0: &'a Interface,
    pub(super) v1: Option<&'a Interface>,
    pub(super) v2: Option<&'a Interface>,
    pub(super) v3: Option<&'a Interface>,
}
impl Ctx<'_> {
    pub(super) fn with_gpu(
        &self,
        pick: impl FnOnce(&ServicesVt) -> GetGpuInterface,
    ) -> Query<Interface> {
        // SAFETY: `v0` is IADLX3DSettingsServices (or a later version, which shares its prefix).
        let call = pick(unsafe { self.v0.vtable::<ServicesVt>() });
        // SAFETY: `call` belongs to the live services object; the GPU and output are live.
        obtain(|out| unsafe { call(self.v0.0, self.gpu.0, out) })
    }
    pub(super) fn afmf(&self) -> Query<Interface> {
        let Some(v1) = self.v1 else {
            return Err(Fault::Missing);
        };
        // SAFETY: `v1` was returned by QueryInterface for IADLX3DSettingsServices1.
        let call = unsafe { v1.vtable::<Services1Vt>() }.afmf;
        // SAFETY: `call` belongs to the live services object; the output is live.
        obtain(|out| unsafe { call(v1.0, out) })
    }
    pub(super) fn sharpen_desktop(&self) -> Query<Interface> {
        let Some(v2) = self.v2 else {
            return Err(Fault::Missing);
        };
        // SAFETY: `v2` was returned by QueryInterface for IADLX3DSettingsServices2.
        let call = unsafe { v2.vtable::<Services2Vt>() }.image_sharpen_desktop;
        // SAFETY: `call` belongs to the live services object; the GPU and output are live.
        obtain(|out| unsafe { call(v2.0, self.gpu.0, out) })
    }
    pub(super) fn fidelityfx(&self, frame_gen: bool) -> Query<Interface> {
        let Some(v3) = self.v3 else {
            return Err(Fault::Missing);
        };
        // SAFETY: `v3` was returned by QueryInterface for IADLX3DSettingsServices3.
        let table = unsafe { v3.vtable::<Services3Vt>() };
        let call = if frame_gen {
            table.fidelityfx_frame_gen
        } else {
            table.fidelityfx_super_resolution
        };
        // SAFETY: `call` belongs to the live services object; the GPU and output are live.
        obtain(|out| unsafe { call(v3.0, self.gpu.0, out) })
    }
    pub(super) fn rsr(&self) -> Query<Interface> {
        // SAFETY: `v0` is IADLX3DSettingsServices or later, whose RSR getter takes no GPU.
        let call = unsafe { self.v0.vtable::<ServicesVt>() }.rsr;
        // SAFETY: `call` belongs to the live services object; the output is live.
        obtain(|out| unsafe { call(self.v0.0, out) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{offset_of, size_of};

    #[test]
    fn vtable_offsets_match_the_public_c_interfaces() {
        let p = size_of::<usize>();
        assert_eq!(offset_of!(SystemVt, gpus), p);
        assert_eq!(offset_of!(SystemVt, services_3d), 7 * p);
        assert_eq!(offset_of!(ServicesVt, anti_lag), 3 * p);
        assert_eq!(offset_of!(ServicesVt, tessellation), 13 * p);
        assert_eq!(offset_of!(ServicesVt, rsr), 14 * p);
        assert_eq!(offset_of!(ServicesVt, reset_shader_cache), 15 * p);
        assert_eq!(offset_of!(ServicesVt, changed_handling), 16 * p);
        assert_eq!(offset_of!(Services1Vt, afmf), 17 * p);
        assert_eq!(offset_of!(Services2Vt, image_sharpen_desktop), 18 * p);
        assert_eq!(offset_of!(Services3Vt, fidelityfx_super_resolution), 19 * p);
        assert_eq!(offset_of!(Services3Vt, fidelityfx_frame_gen), 20 * p);
        assert_eq!(size_of::<Services3Vt>(), 21 * p);
        assert_eq!(offset_of!(GpuVt, name), 7 * p);
        assert_eq!(offset_of!(GpuVt, driver_path), 8 * p);
        assert_eq!(offset_of!(GpuVt, unique_id), 18 * p);
        assert_eq!(offset_of!(Gpu2Vt, edition), 30 * p);
        assert_eq!(offset_of!(Gpu2Vt, software_version), 31 * p);
        assert_eq!(offset_of!(Gpu2Vt, driver_version), 32 * p);
        assert_eq!(offset_of!(Gpu2Vt, windows_driver_version), 33 * p);
    }

    #[test]
    fn interface_vtable_offsets_match_the_public_c_interfaces() {
        let p = size_of::<usize>();
        assert_eq!(size_of::<ToggleVt>(), 6 * p);
        assert_eq!(offset_of!(AntiLag1Vt, level), 6 * p);
        assert_eq!(offset_of!(ChillVt, range), 5 * p);
        assert_eq!(offset_of!(ChillVt, max_fps), 7 * p);
        assert_eq!(size_of::<ChillVt>(), 11 * p);
        assert_eq!(offset_of!(RangedVt, value), 6 * p);
        assert_eq!(size_of::<RangedVt>(), 9 * p);
        assert_eq!(offset_of!(VsyncVt, mode), 5 * p);
        assert_eq!(size_of::<VsyncVt>(), 7 * p);
        assert_eq!(offset_of!(AntiAliasingVt, method), 6 * p);
        assert_eq!(size_of::<AntiAliasingVt>(), 10 * p);
        assert_eq!(offset_of!(AnisotropicVt, level), 5 * p);
        assert_eq!(size_of::<AnisotropicVt>(), 8 * p);
        assert_eq!(offset_of!(TessellationVt, level), 5 * p);
        assert_eq!(size_of::<TessellationVt>(), 8 * p);
        assert_eq!(offset_of!(RsrVt, range), 6 * p);
        assert_eq!(offset_of!(RsrVt, sharpness), 7 * p);
        assert_eq!(size_of::<RsrVt>(), 9 * p);
        assert_eq!(offset_of!(Afmf1Vt, algorithm_supported), 6 * p);
        assert_eq!(offset_of!(Afmf1Vt, fast_motion), 13 * p);
        assert_eq!(size_of::<Afmf1Vt>(), 15 * p);
        assert_eq!(offset_of!(FrameGenVt, supported), 5 * p);
        assert_eq!(offset_of!(FrameGenVt, enabled), 6 * p);
        assert_eq!(size_of::<FrameGenVt>(), 9 * p);
        assert_eq!(offset_of!(RatioOptionVt, ratio), 3 * p);
        assert_eq!(size_of::<RawRange>(), 12);
    }
}
