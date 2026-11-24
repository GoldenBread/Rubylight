//! Read-only ADLX 3D-settings traversal converting driver getters into `model::GroupRaw`.

use super::*;

/// Stands in for a value that was not read because the feature is unavailable. `classify` stops
/// before it looks at fields of an unavailable interface.
const UNREAD: Fault = Fault::Missing;
fn field(
    id: &'static str,
    label: &'static str,
    kind: Kind,
    value: Option<Query<Value>>,
) -> FieldRaw {
    FieldRaw {
        id,
        label,
        kind,
        unit: None,
        value,
        range: None,
        options: vec![],
        options_queried: false,
        note: None,
    }
}
fn toggle(
    id: &'static str,
    label: &'static str,
    i: Option<&Interface>,
    f: impl FnOnce() -> GetBool,
) -> FieldRaw {
    field(
        id,
        label,
        Kind::Toggle,
        Some(i.map_or(Err(UNREAD), |i| get_bool(i, f()).map(Value::Bool))),
    )
}
fn integer(
    id: &'static str,
    label: &'static str,
    unit: &'static str,
    i: Option<&Interface>,
    value: impl FnOnce() -> GetInt,
    range: impl FnOnce() -> GetRange,
) -> FieldRaw {
    let mut f = field(
        id,
        label,
        Kind::Integer,
        Some(i.map_or(Err(UNREAD), |i| {
            get_int(i, value()).map(|v| Value::Int(v.into()))
        })),
    );
    f.unit = Some(unit);
    f.range = Some(i.map_or(Err(UNREAD), |i| get_range(i, range())));
    f
}
fn choice(
    id: &'static str,
    label: &'static str,
    items: EnumItems,
    i: Option<&Interface>,
    f: impl FnOnce() -> GetInt,
) -> FieldRaw {
    let mut out = field(
        id,
        label,
        Kind::Choice,
        Some(i.map_or(Err(UNREAD), |i| {
            get_int(i, f()).map(|raw| Value::Choice(enum_name(items, raw)))
        })),
    );
    out.options = enum_options(items);
    out
}
fn action(id: &'static str, label: &'static str) -> FieldRaw {
    field(id, label, Kind::Action, None)
}

pub(super) fn groups(c: &Ctx) -> Vec<GroupRaw> {
    let mut out = vec![];

    let anti_lag = Open::standard(c.with_gpu(|s| s.anti_lag));
    let (i, t) = table!(anti_lag, ToggleVt);
    let fields = vec![toggle("anti_lag.enabled", "Anti-Lag", i, || {
        t.unwrap().enabled
    })];
    let level = Open::extension(&anti_lag, "IADLX3DAntiLag1");
    let (li, lt) = table!(level, AntiLag1Vt);
    let mut level_field = choice(
        "anti_lag.level",
        "Anti-Lag: driver Anti-Lag or Anti-Lag Next",
        ANTILAG_STATE,
        li,
        || lt.unwrap().level,
    );
    level_field.note = Some(
        "Anti-Lag 2 is an in-game SDK integration and is not exposed by ADLX; this selects the driver's Anti-Lag or Anti-Lag Next".into(),
    );
    out.push(anti_lag.group("anti_lag", "IADLX3DAntiLag", fields));
    out.push(level.group("anti_lag", "IADLX3DAntiLag1", vec![level_field]));

    let chill = Open::standard(c.with_gpu(|s| s.chill));
    let (i, t) = table!(chill, ChillVt);
    let fields = vec![
        toggle("chill.enabled", "Radeon Chill", i, || t.unwrap().enabled),
        integer(
            "chill.min_fps",
            "Radeon Chill: minimum FPS",
            "fps",
            i,
            || t.unwrap().min_fps,
            || t.unwrap().range,
        ),
        integer(
            "chill.max_fps",
            "Radeon Chill: maximum FPS",
            "fps",
            i,
            || t.unwrap().max_fps,
            || t.unwrap().range,
        ),
    ];
    out.push(chill.group("chill", "IADLX3DChill", fields));

    let boost = Open::standard(c.with_gpu(|s| s.boost));
    let (i, t) = table!(boost, RangedVt);
    let fields = vec![
        toggle("boost.enabled", "Radeon Boost", i, || t.unwrap().enabled),
        integer(
            "boost.min_resolution",
            "Radeon Boost: minimum resolution",
            "%",
            i,
            || t.unwrap().value,
            || t.unwrap().range,
        ),
    ];
    out.push(boost.group("boost", "IADLX3DBoost", fields));

    let sharpening = Open::standard(c.with_gpu(|s| s.image_sharpening));
    let (i, t) = table!(sharpening, RangedVt);
    let fields = vec![
        toggle(
            "image_sharpening.enabled",
            "Radeon Image Sharpening",
            i,
            || t.unwrap().enabled,
        ),
        integer(
            "image_sharpening.sharpness",
            "Radeon Image Sharpening: sharpness",
            "%",
            i,
            || t.unwrap().value,
            || t.unwrap().range,
        ),
    ];
    out.push(sharpening.group("image_sharpening", "IADLX3DImageSharpening", fields));

    let sync = Open::standard(c.with_gpu(|s| s.enhanced_sync));
    let (i, t) = table!(sync, ToggleVt);
    let fields = vec![toggle("enhanced_sync.enabled", "Enhanced Sync", i, || {
        t.unwrap().enabled
    })];
    out.push(sync.group("enhanced_sync", "IADLX3DEnhancedSync", fields));

    let vsync = Open::standard(c.with_gpu(|s| s.vsync));
    let (i, t) = table!(vsync, VsyncVt);
    let fields = vec![
        toggle("vsync.enabled", "Wait for Vertical Refresh", i, || {
            t.unwrap().enabled
        }),
        choice(
            "vsync.mode",
            "Wait for Vertical Refresh: mode",
            WAIT_FOR_VERTICAL_REFRESH,
            i,
            || t.unwrap().mode,
        ),
    ];
    out.push(vsync.group("vsync", "IADLX3DWaitForVerticalRefresh", fields));

    let target = Open::standard(c.with_gpu(|s| s.frame_rate_target));
    let (i, t) = table!(target, RangedVt);
    let fields = vec![
        toggle(
            "frame_rate_target.enabled",
            "Frame Rate Target Control",
            i,
            || t.unwrap().enabled,
        ),
        integer(
            "frame_rate_target.fps",
            "Frame Rate Target Control: target",
            "fps",
            i,
            || t.unwrap().value,
            || t.unwrap().range,
        ),
    ];
    out.push(target.group("frame_rate_target", "IADLX3DFrameRateTargetControl", fields));

    let afmf = Open::standard(c.afmf());
    let (i, t) = table!(afmf, ToggleVt);
    let fields = vec![toggle("afmf.enabled", "AMD Fluid Motion Frames", i, || {
        t.unwrap().enabled
    })];
    let detail = Open::extension(&afmf, "IADLX3DAMDFluidMotionFrames1");
    let (di, dt) = table!(detail, Afmf1Vt);
    let mut algorithm = choice(
        "afmf.algorithm",
        "AFMF: algorithm",
        AFMF_ALGORITHM,
        di,
        || dt.unwrap().algorithm,
    );
    algorithm.note = Some(
        match di.zip(dt).map(|(i, t)| get_bool(i, t.algorithm_supported)) {
            Some(Ok(true)) => "IsSupportedAlgorithm is true".into(),
            Some(Ok(false)) => {
                "IsSupportedAlgorithm is false: the algorithm choice is not offered on this GPU"
                    .into()
            }
            Some(Err(fault)) => format!("IsSupportedAlgorithm failed: {}", fault.describe()),
            None => "IsSupportedAlgorithm not read".into(),
        },
    );
    let fields_detail = vec![
        algorithm,
        choice(
            "afmf.search_mode",
            "AFMF: search mode",
            AFMF_SEARCH_MODE,
            di,
            || dt.unwrap().search_mode,
        ),
        choice(
            "afmf.performance_mode",
            "AFMF: performance mode",
            AFMF_PERFORMANCE_MODE,
            di,
            || dt.unwrap().performance_mode,
        ),
        choice(
            "afmf.fast_motion_response",
            "AFMF: fast motion response",
            AFMF_FAST_MOTION,
            di,
            || dt.unwrap().fast_motion,
        ),
    ];
    out.push(afmf.group("afmf", "IADLX3DAMDFluidMotionFrames", fields));
    out.push(detail.group("afmf", "IADLX3DAMDFluidMotionFrames1", fields_detail));

    let rsr = Open::standard(c.rsr());
    let (i, t) = table!(rsr, RsrVt);
    let fields = vec![
        toggle("rsr.enabled", "Radeon Super Resolution", i, || {
            t.unwrap().enabled
        }),
        integer(
            "rsr.sharpness",
            "Radeon Super Resolution: sharpness",
            "%",
            i,
            || t.unwrap().sharpness,
            || t.unwrap().range,
        ),
    ];
    out.push(rsr.group("rsr", "IADLX3DRadeonSuperResolution", fields));

    let aa = Open::standard(c.with_gpu(|s| s.anti_aliasing));
    let (i, t) = table!(aa, AntiAliasingVt);
    let fields = vec![
        choice(
            "anti_aliasing.mode",
            "Anti-aliasing: mode",
            AA_MODE,
            i,
            || t.unwrap().mode,
        ),
        choice(
            "anti_aliasing.level",
            "Anti-aliasing: level",
            AA_LEVEL,
            i,
            || t.unwrap().level,
        ),
        choice(
            "anti_aliasing.method",
            "Anti-aliasing: method",
            AA_METHOD,
            i,
            || t.unwrap().method,
        ),
    ];
    out.push(aa.group("anti_aliasing", "IADLX3DAntiAliasing", fields));

    let mlaa = Open::standard(c.with_gpu(|s| s.morphological_aa));
    let (i, t) = table!(mlaa, ToggleVt);
    let fields = vec![toggle(
        "morphological_aa.enabled",
        "Morphological anti-aliasing",
        i,
        || t.unwrap().enabled,
    )];
    out.push(mlaa.group(
        "morphological_aa",
        "IADLX3DMorphologicalAntiAliasing",
        fields,
    ));

    let af = Open::standard(c.with_gpu(|s| s.anisotropic_filtering));
    let (i, t) = table!(af, AnisotropicVt);
    let fields = vec![
        toggle(
            "anisotropic_filtering.enabled",
            "Anisotropic filtering",
            i,
            || t.unwrap().enabled,
        ),
        choice(
            "anisotropic_filtering.level",
            "Anisotropic filtering: level",
            AF_LEVEL,
            i,
            || t.unwrap().level,
        ),
    ];
    out.push(af.group(
        "anisotropic_filtering",
        "IADLX3DAnisotropicFiltering",
        fields,
    ));

    let tess = Open::standard(c.with_gpu(|s| s.tessellation));
    let (i, t) = table!(tess, TessellationVt);
    let fields = vec![
        choice(
            "tessellation.mode",
            "Tessellation: mode",
            TESSELLATION_MODE,
            i,
            || t.unwrap().mode,
        ),
        choice(
            "tessellation.level",
            "Tessellation: level",
            TESSELLATION_LEVEL,
            i,
            || t.unwrap().level,
        ),
    ];
    out.push(tess.group("tessellation", "IADLX3DTessellation", fields));

    let reset = Open::standard(c.with_gpu(|s| s.reset_shader_cache));
    let mut run = action("reset_shader_cache.run", "Reset shader cache");
    run.note = Some("Action: nothing to read; the probe does not invoke it".into());
    out.push(reset.group("reset_shader_cache", "IADLX3DResetShaderCache", vec![run]));

    let desktop = Open::standard(c.sharpen_desktop());
    let (i, t) = table!(desktop, ToggleVt);
    let fields = vec![toggle(
        "image_sharpen_desktop.enabled",
        "Image Sharpening on the desktop",
        i,
        || t.unwrap().enabled,
    )];
    out.push(desktop.group(
        "image_sharpen_desktop",
        "IADLX3DImageSharpenDesktop",
        fields,
    ));

    let fsr = Open::standard(c.fidelityfx(false));
    let (i, t) = table!(fsr, ToggleVt);
    let fields = vec![toggle(
        "fidelityfx_super_resolution.enabled",
        "FidelityFX Super Resolution upgrade",
        i,
        || t.unwrap().enabled,
    )];
    out.push(fsr.group(
        "fidelityfx_super_resolution",
        "IADLX3DFidelityFXSuperResolution",
        fields,
    ));

    let frame_gen = Open::new(c.fidelityfx(true), |i| {
        // SAFETY: `i` is IADLX3DFidelityFXFrameGenUpgrade, whose IsSupported follows GetAvailableRatios and GetRatio.
        get_bool(i, unsafe { i.vtable::<FrameGenVt>() }.supported)
    });
    let (i, t) = table!(frame_gen, FrameGenVt);
    let mut ratio = field(
        "fidelityfx_frame_gen.ratio",
        "FidelityFX frame generation upgrade: ratio",
        Kind::Choice,
        Some(i.zip(t).map_or(Err(UNREAD), |(i, t)| {
            get_int(i, t.ratio).map(|raw| Value::Choice(enum_name(FFX_FRAME_GEN_RATIO, raw)))
        })),
    );
    if let Some((i, t)) = i.zip(t) {
        ratio.options_queried = true;
        ratio.options = frame_gen_ratios(i, t);
    }
    let fields = vec![
        toggle(
            "fidelityfx_frame_gen.enabled",
            "FidelityFX frame generation upgrade",
            i,
            || t.unwrap().enabled,
        ),
        ratio,
    ];
    out.push(frame_gen.group(
        "fidelityfx_frame_gen",
        "IADLX3DFidelityFXFrameGenUpgrade",
        fields,
    ));
    out
}

/// The ratios ADLX lists for this GPU.
fn frame_gen_ratios(i: &Interface, t: &FrameGenVt) -> Vec<ChoiceOption> {
    let Ok(list) = obtain(|out| {
        // SAFETY: `t` is the vtable of the live frame generation interface; the output is live.
        unsafe { (t.ratios)(i.0, out) }
    }) else {
        return vec![];
    };
    // SAFETY: GetAvailableRatios returns an IADLX3DFidelityFXFrameGenUpgradeRatioOptionList, which
    // has the common ADLX list layout.
    let table = unsafe { list.vtable::<ListVtable>() };
    // SAFETY: The list is live.
    let count = unsafe { (table.size)(list.0) };
    (0..count)
        .filter_map(|index| {
            let item = obtain(|out| {
                // SAFETY: index is within the list; At_OptionList acquires a reference into `out`.
                unsafe { (table.at)(list.0, index, out) }
            })
            .ok()?;
            // SAFETY: the item is an IADLX3DFidelityFXFrameGenUpgradeRatioOption.
            let raw = get_int(&item, unsafe { item.vtable::<RatioOptionVt>() }.ratio).ok()?;
            Some(ChoiceOption {
                value: enum_name(FFX_FRAME_GEN_RATIO, raw),
                raw,
            })
        })
        .collect()
}
