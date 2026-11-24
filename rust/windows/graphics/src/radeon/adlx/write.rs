//! Bounded ADLX 3D-settings setter dispatch used exclusively by `AdlxBackend::write`.

use super::*;

/// A setter's answer. ADLX_ALREADY_ENABLED (1) is what some setters say when the value is already
/// in effect; the caller's read-back still decides whether the write counts.
fn written(result: i32) -> Query<()> {
    code(if result == 1 { 0 } else { result })
}
fn set_bool(i: &Interface, set: SetBool, on: bool) -> Query<()> {
    // SAFETY: `set` is a SetEnabled slot of the live interface `i`.
    written(unsafe { set(i.0, u8::from(on)) })
}
fn set_int(i: &Interface, set: SetInt, value: i32) -> Query<()> {
    // SAFETY: `set` is a setter slot of the live interface `i` that takes one 32-bit value.
    written(unsafe { set(i.0, value) })
}
/// Why a setter could not be reached.
fn unreachable_reason(open: &Open) -> Fault {
    match (&open.obtained, &open.supported) {
        (Err(fault), _) | (_, Some(Err(fault))) => *fault,
        // IsSupported said false: ADLX_NOT_SUPPORTED.
        _ => Fault::Code(12),
    }
}
macro_rules! call {
    ($open:expr, $ty:ty, |$i:ident, $t:ident| $body:expr) => {{
        let open = $open;
        let (ready, table) = table!(open, $ty);
        match ready.zip(table) {
            Some(($i, $t)) => $body,
            None => Err(unreachable_reason(&open)),
        }
    }};
}

/// The only place a setter is called. Settings the host does not write have no arm.
pub(super) fn write_setting(c: &Ctx, id: &str, value: &Value) -> Result<(), String> {
    let flag = || match value {
        Value::Bool(on) => Ok(*on),
        _ => Err(format!("{id} takes a toggle")),
    };
    let int = || match value {
        Value::Int(v) => i32::try_from(*v).map_err(|_| format!("{v} does not fit {id}")),
        _ => Err(format!("{id} takes an integer")),
    };
    let raw = || match value {
        Value::Choice(name) => enum_items(id)
            .and_then(|items| enum_raw(items, name))
            .ok_or_else(|| format!("{name} is not a value of {id}")),
        _ => Err(format!("{id} takes one of its options")),
    };
    let result: Query<()> = match id {
        "anti_lag.enabled" => {
            let on = flag()?;
            call!(
                Open::standard(c.with_gpu(|s| s.anti_lag)),
                ToggleVt,
                |i, t| { set_bool(i, t.set_enabled, on) }
            )
        }
        "chill.enabled" => {
            let on = flag()?;
            call!(Open::standard(c.with_gpu(|s| s.chill)), ChillVt, |i, t| {
                set_bool(i, t.set_enabled, on)
            })
        }
        "chill.min_fps" => {
            let v = int()?;
            call!(Open::standard(c.with_gpu(|s| s.chill)), ChillVt, |i, t| {
                set_int(i, t.set_min_fps, v)
            })
        }
        "chill.max_fps" => {
            let v = int()?;
            call!(Open::standard(c.with_gpu(|s| s.chill)), ChillVt, |i, t| {
                set_int(i, t.set_max_fps, v)
            })
        }
        "boost.enabled" => {
            let on = flag()?;
            call!(Open::standard(c.with_gpu(|s| s.boost)), RangedVt, |i, t| {
                set_bool(i, t.set_enabled, on)
            })
        }
        "boost.min_resolution" => {
            let v = int()?;
            call!(Open::standard(c.with_gpu(|s| s.boost)), RangedVt, |i, t| {
                set_int(i, t.set_value, v)
            })
        }
        "image_sharpening.enabled" => {
            let on = flag()?;
            call!(
                Open::standard(c.with_gpu(|s| s.image_sharpening)),
                RangedVt,
                |i, t| set_bool(i, t.set_enabled, on)
            )
        }
        "image_sharpening.sharpness" => {
            let v = int()?;
            call!(
                Open::standard(c.with_gpu(|s| s.image_sharpening)),
                RangedVt,
                |i, t| set_int(i, t.set_value, v)
            )
        }
        "enhanced_sync.enabled" => {
            let on = flag()?;
            call!(
                Open::standard(c.with_gpu(|s| s.enhanced_sync)),
                ToggleVt,
                |i, t| set_bool(i, t.set_enabled, on)
            )
        }
        "vsync.mode" => {
            let v = raw()?;
            call!(Open::standard(c.with_gpu(|s| s.vsync)), VsyncVt, |i, t| {
                set_int(i, t.set_mode, v)
            })
        }
        "frame_rate_target.enabled" => {
            let on = flag()?;
            call!(
                Open::standard(c.with_gpu(|s| s.frame_rate_target)),
                RangedVt,
                |i, t| set_bool(i, t.set_enabled, on)
            )
        }
        "frame_rate_target.fps" => {
            let v = int()?;
            call!(
                Open::standard(c.with_gpu(|s| s.frame_rate_target)),
                RangedVt,
                |i, t| set_int(i, t.set_value, v)
            )
        }
        "afmf.enabled" => {
            let on = flag()?;
            call!(Open::standard(c.afmf()), ToggleVt, |i, t| {
                set_bool(i, t.set_enabled, on)
            })
        }
        "afmf.search_mode" | "afmf.performance_mode" | "afmf.fast_motion_response" => {
            let v = raw()?;
            let parent = Open::standard(c.afmf());
            call!(
                Open::extension(&parent, "IADLX3DAMDFluidMotionFrames1"),
                Afmf1Vt,
                |i, t| match id {
                    "afmf.search_mode" => set_int(i, t.set_search_mode, v),
                    "afmf.performance_mode" => set_int(i, t.set_performance_mode, v),
                    _ => set_int(i, t.set_fast_motion, v),
                }
            )
        }
        "rsr.sharpness" => {
            let v = int()?;
            call!(Open::standard(c.rsr()), RsrVt, |i, t| {
                set_int(i, t.set_sharpness, v)
            })
        }
        "anti_aliasing.mode" | "anti_aliasing.level" | "anti_aliasing.method" => {
            let v = raw()?;
            call!(
                Open::standard(c.with_gpu(|s| s.anti_aliasing)),
                AntiAliasingVt,
                |i, t| match id {
                    "anti_aliasing.mode" => set_int(i, t.set_mode, v),
                    "anti_aliasing.level" => set_int(i, t.set_level, v),
                    _ => set_int(i, t.set_method, v),
                }
            )
        }
        "morphological_aa.enabled" => {
            let on = flag()?;
            call!(
                Open::standard(c.with_gpu(|s| s.morphological_aa)),
                ToggleVt,
                |i, t| set_bool(i, t.set_enabled, on)
            )
        }
        "anisotropic_filtering.enabled" => {
            let on = flag()?;
            call!(
                Open::standard(c.with_gpu(|s| s.anisotropic_filtering)),
                AnisotropicVt,
                |i, t| set_bool(i, t.set_enabled, on)
            )
        }
        "anisotropic_filtering.level" => {
            let v = raw()?;
            call!(
                Open::standard(c.with_gpu(|s| s.anisotropic_filtering)),
                AnisotropicVt,
                |i, t| set_int(i, t.set_level, v)
            )
        }
        "tessellation.mode" | "tessellation.level" => {
            let v = raw()?;
            call!(
                Open::standard(c.with_gpu(|s| s.tessellation)),
                TessellationVt,
                |i, t| match id {
                    "tessellation.mode" => set_int(i, t.set_mode, v),
                    _ => set_int(i, t.set_level, v),
                }
            )
        }
        "fidelityfx_super_resolution.enabled" => {
            let on = flag()?;
            call!(Open::standard(c.fidelityfx(false)), ToggleVt, |i, t| {
                set_bool(i, t.set_enabled, on)
            })
        }
        _ => return Err(format!("{id} has no ADLX setter the host uses")),
    };
    result.map_err(|fault| format!("{id}: {}", fault.describe()))
}
