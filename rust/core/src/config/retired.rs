//! Settings and values earlier releases and other hosts saved that the
//! host no longer reads, and what replaces each.
use super::unquote;

/// Setting values Rubylight no longer offers, and what replaces each:
/// the ViGEmBus controllers, which Sunshine, Apollo, Vibepollo and earlier
/// Rubylight profiles can carry, became the VHF pad of the same family.
pub const RETIRED_VALUES: &[(&str, &str, &str)] = &[
    ("gamepad", "x360", "vhf_xbox_one"),
    ("gamepad", "ds4", "vhf_ds4"),
];
/// Settings whose values select features Rubylight removed. A value of
/// `key` that `retired` matches is replaced by `current` wherever the host
/// reads settings, and a value read from sunshine.conf is reported once
/// (see [`Config::load_retiring`]). Another setting's removed values take
/// one more row.
pub struct RetiredSetting {
    pub key: &'static str,
    pub retired: fn(&str) -> bool,
    pub current: &'static str,
    /// Why the value no longer applies, for the warning.
    pub reason: &'static str,
}
pub const RETIRED_SETTINGS: &[RetiredSetting] = &[
    RetiredSetting {
        key: "encoder",
        retired: crate::encoder_policy::retired,
        current: "auto",
        reason: "this host encodes H.264, HEVC and AV1 with AMF or NVENC only",
    },
    RetiredSetting {
        key: "frame_limiter_provider",
        retired: crate::framegen::retired_provider,
        current: "auto",
        reason: "RTSS is the only frame limiter",
    },
    RetiredSetting {
        key: "frame-generation-mode",
        retired: crate::framegen::retired_generation,
        current: "none",
        reason: "driver frame generation needs an NVIDIA GPU",
    },
    RetiredSetting {
        key: "frame-generation-provider",
        retired: crate::framegen::retired_generation,
        current: "none",
        reason: "driver frame generation needs an NVIDIA GPU",
    },
    RetiredSetting {
        key: crate::nucleus::mode::KEY,
        retired: crate::nucleus::mode::retired_background,
        current: "nucleus",
        reason: "Windows child sessions were removed: Windows signs a Microsoft account that uses only Windows Hello into one only with a password; Nucleus Co-op is the background mode",
    },
];
/// A retired value the host replaced while reading the configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Retired {
    pub key: String,
    pub value: String,
    pub current: &'static str,
    pub reason: &'static str,
}
/// The one-time warning for values replaced in sunshine.conf.
pub fn retired_warning(retired: &[Retired]) -> Option<String> {
    let changes: Vec<_> = retired
        .iter()
        .map(|r| format!("{} {} is now {} ({})", r.key, r.value, r.current, r.reason))
        .collect();
    (!changes.is_empty()).then(|| {
        format!(
            "Saved settings for removed features were changed: {}. The configuration file now holds the new values; check them in Settings.",
            changes.join("; ")
        )
    })
}
/// Settings that Vibepollo, Apollo and earlier Rubylight releases saved under
/// another name, and the name the host reads now. The values mean the same
/// under both names.
pub const RENAMED_KEYS: &[(&str, &str)] = &[
    ("rtss_path", "rtss_install_path"),
    ("nvenc_force_split_encode", "nvenc_split_encode"),
    // Also what a virtual display reports, the one part of NVIDIA RTX HDR
    // that stays.
    ("rtx_hdr_peak_brightness", "hdr_peak_brightness"),
    // The refresh of a virtual display, not frame generation.
    (
        "frame_limiter_auto_virtual_framegen",
        "virtual_display_refresh",
    ),
    // Full chroma shipped on by default; it is no experiment.
    ("experimental_packed_444", "packed_444"),
];
/// Settings that earlier releases kept once for several things and that became
/// one setting for each, which takes the earlier value unless it is set itself:
/// the launcher sync's two switches, now each launcher's own as Steam's are,
/// and the game folders'.
pub const SPLIT_KEYS: &[(&str, &[&str])] = &[
    (
        "library_auto_sync",
        &[
            "epic_auto_sync",
            "battlenet_auto_sync",
            "ea_auto_sync",
            "ubisoft_auto_sync",
            "gog_auto_sync",
            "microsoft_auto_sync",
            "library_local_auto_sync",
        ],
    ),
    (
        "library_autosync_remove_uninstalled",
        &[
            "epic_autosync_remove_uninstalled",
            "battlenet_autosync_remove_uninstalled",
            "ea_autosync_remove_uninstalled",
            "ubisoft_autosync_remove_uninstalled",
            "gog_autosync_remove_uninstalled",
            "microsoft_autosync_remove_uninstalled",
            "library_local_autosync_remove_uninstalled",
        ],
    ),
];
/// Settings of features Rubylight removed: the
/// NVIDIA driver's VSYNC override (all the dummy plug workaround still
/// did), and the software encoders' settings. Saved profiles, apps and
/// devices can still carry them; the host drops them without a warning,
/// and the next save leaves them out.
pub const REMOVED_KEYS: &[&str] = &[
    "frame_limiter_disable_vsync",
    "rtss_disable_vsync_ullm",
    "dd_wa_dummy_plug_hdr10",
    "min_threads",
    "crf",
    "qp",
    // The removed background child session.
    "child_session_resolution",
    "child_session_scale",
    "child_session_port",
    "child_session_host",
    // The removed permanent virtual displays, under both of their names.
    "dd_vdd_static_monitor_count",
    "dd_virtual_display_permanent_count",
    // Defined for the Windows touch keyboard and never read.
    "touch_keyboard_digitizer",
];
/// The settings of the removed Quick Sync and software encoders.
pub const REMOVED_PREFIXES: &[&str] = &["qsv_", "sw_"];
/// Whether `key` belongs to a removed feature (`REMOVED_KEYS`,
/// `REMOVED_PREFIXES`).
pub fn removed(key: &str) -> bool {
    REMOVED_KEYS.contains(&key) || REMOVED_PREFIXES.iter().any(|p| key.starts_with(p))
}
/// Display HDR, which holds all four HDR choices: Match the stream (`auto`),
/// Always on (`force_on`), Always off (`force_off`) and Don't change HDR
/// (`disabled`).
pub const HDR_OPTION: &str = "dd_hdr_option";
/// The HDR request (follow the device, always on, always off) that earlier
/// releases kept beside Display HDR. Its name is retired: its value moves
/// into Display HDR where the host reads settings ([`folded_hdr`],
/// [`folded_in_layer`]). An app's or device's request without a Display HDR
/// of its own depends on the layers below, so it stays as saved and folds
/// when the host applies that layer over them (`Config::update`).
pub const HDR_REQUEST: &str = "dd_hdr_request_override";
/// Settings folded into another one (see [`HDR_REQUEST`]).
pub const FOLDED_KEYS: &[&str] = &[HDR_REQUEST];
/// Whether `key` is a name the host no longer reads: a removed setting, or
/// one folded into another. Neither name is used again.
pub fn retired_name(key: &str) -> bool {
    removed(key) || FOLDED_KEYS.contains(&key)
}
/// Display HDR for a retired HDR `request` over the Display HDR `option`
/// that the layers below give (or the configuration holds): Don't change HDR
/// still wins, as it did over the request; Always on or off becomes that
/// choice; Follow the device leaves Display HDR as it is (`None`).
pub fn folded_hdr(option: Option<&str>, request: &str) -> Option<&'static str> {
    if option.is_some_and(|option| unquote(option.trim()) == "disabled") {
        return None;
    }
    match unquote(request.trim()) {
        "force_on" => Some("force_on"),
        "force_off" => Some("force_off"),
        _ => None,
    }
}
/// Display HDR for a retired HDR `request` saved in the same layer (the
/// configuration file, an app's or a device's overrides) as Display HDR
/// `option`. Earlier releases offered only Match the stream and Don't change
/// HDR there, so Always on or off is a later choice and wins over the
/// request, as Don't change HDR does; otherwise as [`folded_hdr`].
pub fn folded_in_layer(option: &str, request: &str) -> Option<&'static str> {
    match unquote(option.trim()) {
        "force_on" | "force_off" => None,
        option => folded_hdr(Some(option), request),
    }
}
/// Bring one layer of overrides (an app's `config-overrides`, a device
/// block's, a paired device's `config_overrides`) to the names and values the
/// host reads now: a renamed setting takes its current name (a value already
/// under that name wins), a removed one goes, the HDR request folds into the
/// layer's own Display HDR ([`HDR_REQUEST`] says when it stays) and retired
/// values take their replacements. Returns whether anything changed.
pub fn migrate_overrides(overrides: &mut serde_json::Map<String, serde_json::Value>) -> bool {
    let mut changed = false;
    for &(old, new) in RENAMED_KEYS {
        if let Some(value) = overrides.remove(old) {
            changed = true;
            if !overrides.contains_key(new) {
                overrides.insert(new.to_owned(), value);
            }
        }
    }
    if let Some(option) = overrides
        .get(HDR_OPTION)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        && let Some(request) = overrides.remove(HDR_REQUEST)
    {
        changed = true;
        if let Some(folded) = request
            .as_str()
            .and_then(|request| folded_in_layer(&option, request))
        {
            overrides.insert(HDR_OPTION.to_owned(), folded.into());
        }
    }
    let before = overrides.len();
    overrides.retain(|key, _| !removed(key));
    changed |= overrides.len() != before;
    replace_retired(overrides.iter_mut()) || changed
}
/// Settings of an app record that Rubylight no longer reads. The app editor
/// keeps keys it does not show, so the host drops these when it loads the
/// library: `frame-gen-limiter-fix` was a switch the host never read.
pub const REMOVED_APP_KEYS: &[&str] = &["frame-gen-limiter-fix"];
/// App switches that Apollo saved under another name and that mean what the
/// current one does: `virtual-screen` asked for a virtual display as
/// `virtual-display` does. Either one on turns the current one on.
pub const RENAMED_APP_SWITCHES: &[(&str, &str)] = &[("virtual-screen", "virtual-display")];
/// An app switch as the host reads it (`process::app_bool`).
fn app_switch(value: Option<&serde_json::Value>) -> bool {
    match value {
        Some(serde_json::Value::Bool(on)) => *on,
        Some(serde_json::Value::String(text)) => matches!(text.as_str(), "true" | "1" | "yes"),
        _ => false,
    }
}
/// The name the host reads `key` under now.
pub fn current_name(key: &str) -> &str {
    RENAMED_KEYS
        .iter()
        .find(|(old, _)| *old == key)
        .map_or(key, |(_, new)| new)
}
/// The current value for a retired `value` of `key`, if it is one.
pub fn replacement(key: &str, value: &str) -> Option<&'static str> {
    retirement(key, value).map(|(current, _)| current)
}
/// The current value for a retired `value` of `key`, and why it changed.
pub(super) fn retirement(key: &str, value: &str) -> Option<(&'static str, &'static str)> {
    let value = unquote(value.trim());
    RETIRED_VALUES
        .iter()
        .find(|(k, old, _)| *k == key && *old == value)
        .map(|(_, _, new)| (*new, "the controller type was replaced"))
        .or_else(|| {
            RETIRED_SETTINGS
                .iter()
                .find(|setting| setting.key == key && (setting.retired)(value))
                .map(|setting| (setting.current, setting.reason))
        })
}

/// Replace retired values among an app's or a device's settings, which are
/// JSON. Returns whether anything changed.
pub fn replace_retired<'a>(
    settings: impl IntoIterator<Item = (&'a String, &'a mut serde_json::Value)>,
) -> bool {
    let mut changed = false;
    for (key, value) in settings {
        if let Some(current) = value.as_str().and_then(|old| replacement(key, old)) {
            *value = serde_json::Value::from(current);
            changed = true;
        }
    }
    changed
}

/// Bring a library app to what the host reads now: retired values among its
/// top-level settings, as Apollo and Vibepollo store them, take their
/// replacements; removed app settings go and renamed ones take their current
/// name; its `config-overrides` and those of its devices' blocks
/// (`device-overrides`) are migrated ([`migrate_overrides`]). Returns whether
/// anything changed.
pub fn migrate_app(app: &mut serde_json::Map<String, serde_json::Value>) -> bool {
    let mut changed = replace_retired(app.iter_mut());
    for key in REMOVED_APP_KEYS {
        changed |= app.remove(*key).is_some();
    }
    for &(old, new) in RENAMED_APP_SWITCHES {
        if let Some(value) = app.remove(old) {
            changed = true;
            if app_switch(Some(&value)) && !app_switch(app.get(new)) {
                app.insert(new.to_owned(), true.into());
            }
        }
    }
    if let Some(overrides) = app
        .get_mut("config-overrides")
        .and_then(serde_json::Value::as_object_mut)
    {
        changed |= migrate_overrides(overrides);
    }
    for block in app
        .get_mut(crate::device_overrides::SETTING)
        .and_then(serde_json::Value::as_object_mut)
        .into_iter()
        .flat_map(|blocks| blocks.values_mut())
    {
        if let Some(overrides) = block
            .get_mut(crate::device_overrides::CONFIG)
            .and_then(serde_json::Value::as_object_mut)
        {
            changed |= migrate_overrides(overrides);
        }
    }
    changed
}
