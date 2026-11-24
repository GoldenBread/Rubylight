//! Settings and values earlier releases and other hosts saved that the
//! host no longer reads, and what replaces each.
mod fold;

#[cfg(test)]
pub use super::retired_gen::LIBRARY_PREFIXES;
pub use super::retired_gen::{
    FOLDED_KEYS, LIBRARY_SYNC_OPTIONS, MIN_QP, REMOVED_APP_KEYS, REMOVED_KEYS, REMOVED_PREFIXES,
    RENAMED_APP_SWITCHES, RENAMED_KEYS, RETIRED_VALUES,
};
use super::unquote;
pub use fold::{
    AUTO_UPDATE, BITRATE_ADAPTATION, CONGESTION, DD_DISCONNECT_BEHAVIOR, DD_REVERT_ON_DISCONNECT,
    HDR_OPTION, HDR_REQUEST, HOST_PACING, LINK_RATE, MIN_QP_SWITCH, PACING_RATE, PACKET_PACING,
    PYROWAVE_FEC, REMOTE_CLIENT_DISCONNECT, REMOTE_STREAM_END, REMOVE_REMOTE_MONITOR,
    UPDATE_CHECK_INTERVAL, UPDATES, WIRELESS_PACING, fold_bitrate_adaptation,
    fold_bitrate_adaptation_decided, fold_disconnect_behavior, fold_library_sync, fold_min_qp,
    fold_min_qp_values, fold_packet_pacing, fold_packet_pacing_decided, fold_pyrowave_fec,
    fold_remove_remote_monitor, fold_remove_remote_monitor_decided, fold_updates,
    folded_bitrate_adaptation, folded_bitrate_adaptation_below, folded_disconnect_behavior,
    folded_hdr, folded_in_layer, folded_packet_pacing, folded_packet_pacing_below,
    folded_remove_remote_monitor, folded_remove_remote_monitor_below, folded_updates,
    holds_retired_settings, min_qp_switch_on, pyrowave_fec_is_default,
};

/// Settings whose values select features Rubylight removed. A value of
/// `key` that `retired` matches is replaced by `current` wherever the host
/// reads settings, and a value read from sunshine.conf is reported once
/// (`Config::load_retiring`).
pub struct RetiredSetting {
    pub key: &'static str,
    pub retired: fn(&str) -> bool,
    pub current: &'static str,
    /// Why the value no longer applies, for the warning.
    pub reason: &'static str,
}

/// The app key that holds device override blocks, by device uuid.
pub const DEVICE_OVERRIDES_SETTING: &str = "device-overrides";
/// The app's general overrides, in the app and in a device override block.
pub const DEVICE_OVERRIDES_CONFIG: &str = "config-overrides";
/// The setting earlier builds chose the background mode with.
pub const BACKGROUND_MODE_KEY: &str = "background_mode";

/// An `encoder` value naming an encoder this host does not have: Quick
/// Sync, software, Media Foundation or anything else. The configuration
/// replaces it with automatic selection.
pub fn retired_encoder(name: &str) -> bool {
    !matches!(
        name,
        "" | "auto"
            | "amf"
            | "nvenc"
            | "pyrowave"
            | "amdvce"
            | "amdvce_experimental"
            | "amdvce_ffmpeg"
            | "amdvce_legacy"
            | "nvenc_experimental"
            | "nvenc_legacy"
    )
}

fn normalize_limiter_value(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// A `frame_limiter_provider` naming the removed NVIDIA driver limiter.
pub fn retired_frame_limiter_provider(value: &str) -> bool {
    matches!(
        normalize_limiter_value(value).as_str(),
        "nvidia" | "nvidiacontrolpanel" | "nvcp"
    )
}

/// An app's frame generation naming the NVIDIA driver's Smooth Motion.
pub fn retired_frame_generation(value: &str) -> bool {
    matches!(
        normalize_limiter_value(value).as_str(),
        "nvidia" | "smoothmotion" | "nvidiasmoothmotion"
    )
}

/// Whether `value` is a background mode an earlier build wrote (trimmed,
/// any case).
pub fn retired_background_mode(value: &str) -> bool {
    ["auto", "child_session", "virtual_display"]
        .contains(&value.trim().to_ascii_lowercase().as_str())
}

pub const RETIRED_SETTINGS: &[RetiredSetting] = &[
    RetiredSetting {
        key: "encoder",
        retired: retired_encoder,
        current: "auto",
        reason: "this host encodes H.264, HEVC and AV1 with AMF or NVENC only",
    },
    RetiredSetting {
        key: "frame_limiter_provider",
        retired: retired_frame_limiter_provider,
        current: "auto",
        reason: "RTSS is the only frame limiter",
    },
    RetiredSetting {
        key: "frame-generation-mode",
        retired: retired_frame_generation,
        current: "none",
        reason: "driver frame generation needs an NVIDIA GPU",
    },
    RetiredSetting {
        key: "frame-generation-provider",
        retired: retired_frame_generation,
        current: "none",
        reason: "driver frame generation needs an NVIDIA GPU",
    },
    RetiredSetting {
        key: BACKGROUND_MODE_KEY,
        retired: retired_background_mode,
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

/// Whether `key` belongs to a removed feature (`REMOVED_KEYS`,
/// `REMOVED_PREFIXES`).
pub fn removed(key: &str) -> bool {
    REMOVED_KEYS.contains(&key) || REMOVED_PREFIXES.iter().any(|p| key.starts_with(p))
}

/// Whether `key` is a name the host no longer reads: a removed setting, or
/// one folded into another. Neither name is used again.
pub fn retired_name(key: &str) -> bool {
    removed(key) || FOLDED_KEYS.contains(&key)
}

/// Bring one layer of overrides (an app's `config-overrides`, a device
/// block's, a paired device's `config_overrides`) to the names and values the
/// host reads now. Returns whether anything changed.
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
    changed |= fold_bitrate_adaptation_decided(overrides);
    changed |= fold_packet_pacing_decided(overrides);
    changed |= fold_pyrowave_fec(overrides);
    changed |= fold_remove_remote_monitor_decided(overrides);
    changed |= fold_disconnect_behavior(overrides);
    changed |= fold_min_qp(overrides, |_| false, false);
    let before = overrides.len();
    overrides.retain(|key, _| !removed(key));
    changed |= overrides.len() != before;
    replace_retired(overrides.iter_mut()) || changed
}

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
/// top-level settings, removed/renamed app settings, and its `config-overrides`
/// and `device-overrides` blocks. Returns whether anything changed.
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
        .get_mut(DEVICE_OVERRIDES_SETTING)
        .and_then(serde_json::Value::as_object_mut)
        .into_iter()
        .flat_map(|blocks| blocks.values_mut())
    {
        if let Some(overrides) = block
            .get_mut(DEVICE_OVERRIDES_CONFIG)
            .and_then(serde_json::Value::as_object_mut)
        {
            changed |= migrate_overrides(overrides);
        }
    }
    changed
}
