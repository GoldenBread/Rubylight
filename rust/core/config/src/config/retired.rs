//! Settings and values earlier releases and other hosts saved that the
//! host no longer reads, and what replaces each.
use super::unquote;
use std::collections::BTreeMap;

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
/// The launchers whose sync options earlier releases kept each under its own
/// keys: Steam's `steam_*`, the launcher sync's `<launcher>_*`, the game
/// folders' `library_local_*` and Playnite's `playnite_*`.
pub const LIBRARY_PREFIXES: &[&str] = &[
    "steam",
    "epic",
    "battlenet",
    "microsoft",
    "ea",
    "ubisoft",
    "gog",
    "library_local",
    "playnite",
];
/// The launchers that record when a game was played, the ones whose sync has
/// the `sync_all_installed`, `recent_games` and `recent_max_age_days` options.
pub const LIBRARY_HISTORY_PREFIXES: &[&str] = &["steam", "battlenet", "playnite"];
/// Each Library sync option that became one shared setting for every launcher:
/// its name (the shared key is `library_<name>`), its default and the
/// launchers that had their own.
pub const LIBRARY_SYNC_OPTIONS: &[(&str, &str, &[&str])] = &[
    ("auto_sync", "true", LIBRARY_PREFIXES),
    ("autosync_remove_uninstalled", "true", LIBRARY_PREFIXES),
    ("sync_all_installed", "true", LIBRARY_HISTORY_PREFIXES),
    ("recent_games", "10", LIBRARY_HISTORY_PREFIXES),
    ("recent_max_age_days", "30", LIBRARY_HISTORY_PREFIXES),
];
/// Fold the launchers' `<launcher>_<option>` settings into the shared
/// `library_<option>` one ([`LIBRARY_SYNC_OPTIONS`]). The shared value is the
/// one every launcher agrees on; while they differ it is the default, and a
/// launcher whose value differs keeps its own key, which the sync still reads
/// as an override. A value already saved under the shared name wins, and a
/// launcher's key equal to the shared value goes. A profile an earlier
/// release wrote before the options were split (`old_file`,
/// [`holds_retired_settings`]) kept one `library_auto_sync` and
/// `library_autosync_remove_uninstalled` for the launcher sync and the game
/// folders but never for Steam or Playnite, which kept syncing: with one off,
/// they keep their own key at the default they read. Returns whether
/// anything changed.
pub fn fold_library_sync(values: &mut BTreeMap<String, String>, old_file: bool) -> bool {
    let mut changed = false;
    if old_file {
        for &(option, default, _) in LIBRARY_SYNC_OPTIONS {
            let off = values
                .get(&format!("library_{option}"))
                .and_then(|value| super::parse_boolean(value))
                == Some(false);
            if off && matches!(option, "auto_sync" | "autosync_remove_uninstalled") {
                for prefix in ["steam", "playnite"] {
                    values
                        .entry(format!("{prefix}_{option}"))
                        .or_insert_with(|| default.to_owned());
                    changed = true;
                }
            }
        }
    }
    for &(option, default, prefixes) in LIBRARY_SYNC_OPTIONS {
        let shared = format!("library_{option}");
        let keys: Vec<(String, String)> = prefixes
            .iter()
            .filter_map(|prefix| {
                let key = format!("{prefix}_{option}");
                values.get(&key).map(|value| (key, value.clone()))
            })
            .collect();
        if keys.is_empty() {
            continue;
        }
        // The value every launcher agrees on counts the ones not set as the
        // default, which is what they read.
        let effective: Vec<String> = prefixes
            .iter()
            .map(|prefix| {
                values
                    .get(&format!("{prefix}_{option}"))
                    .cloned()
                    .unwrap_or_else(|| default.to_owned())
            })
            .collect();
        let agreed = effective
            .iter()
            .all(|value| value == &effective[0])
            .then(|| effective[0].clone());
        let shared_value = values
            .get(&shared)
            .cloned()
            .unwrap_or_else(|| agreed.unwrap_or_else(|| default.to_owned()));
        if shared_value != default && !values.contains_key(&shared) {
            values.insert(shared, shared_value.clone());
            changed = true;
        }
        for (key, value) in &keys {
            if value == &shared_value {
                values.remove(key);
                changed = true;
            }
        }
    }
    changed
}
/// Settings of features Rubylight removed: the
/// NVIDIA driver's VSYNC override (all the dummy plug workaround still
/// did), and the software encoders' settings. Saved profiles, apps and
/// devices can still carry them; the host drops them without a warning,
/// and the next save leaves them out.
pub const REMOVED_KEYS: &[&str] = &[
    // Error correction is one share for every codec: the host's own share
    // no longer decays after loss (it bought no picture quality), and the
    // cap on a device's choice is the fixed 50 percent.
    "adaptive_fec",
    "fec_max_percentage",
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
    // The opt-in host bitrate controller; link-rate adaptation and
    // congestion control run in its place.
    "host_adaptive_bitrate",
    // Holding a capped game's frames for the device's refresh: the hold cost
    // 8 to 11 ms on a Shield TV, so the phase lock always stands aside for a
    // source at the stream rate or slower.
    "phase_lock_hold_at_stream_rate",
    // The compute queue always runs at high priority: real-time could hang
    // the graphics driver and measured no gain.
    "compute_queue_realtime",
    // AMF always takes the driver's own input queue and low-latency choice:
    // forcing either froze video on Radeon cards.
    "amd_input_queue_size",
    "amd_lowlatency_mode",
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
/// Adaptive bitrate (`off`, `on`, `delay`), which holds what two switches
/// held before: Lower the bitrate when the link cannot carry it and Adapt to
/// network congestion.
pub const BITRATE_ADAPTATION: &str = "bitrate_adaptation";
/// The two switches folded into [`BITRATE_ADAPTATION`]: link-rate
/// adaptation (on by default) and congestion control (off by default).
pub const LINK_RATE: &str = "link_rate_adaptation";
pub const CONGESTION: &str = "congestion_control";
/// Packet pacing (`auto`, `always`, `off`, or a fixed rate in kbps), which
/// holds what the pacing rate and the two adaptive pacing switches held
/// before.
pub const PACKET_PACING: &str = "packet_pacing";
/// The pacing rate and the two adaptive pacing switches folded into
/// [`PACKET_PACING`]: a manual rate in kbps (0 unset), adaptive pacing for
/// every client and codec (off by default) and adaptive pacing for clients
/// that report a wireless route on H.264, HEVC and AV1 (on by default).
pub const PACING_RATE: &str = "pacing_max_bitrate_kbps";
pub const HOST_PACING: &str = "host_adaptive_pacing";
pub const WIRELESS_PACING: &str = "wireless_adaptive_pacing";
/// Settings folded into another one (see [`HDR_REQUEST`],
/// [`BITRATE_ADAPTATION`], [`PACKET_PACING`]).
pub const FOLDED_KEYS: &[&str] = &[
    HDR_REQUEST,
    LINK_RATE,
    CONGESTION,
    PACING_RATE,
    HOST_PACING,
    WIRELESS_PACING,
    REMOTE_STREAM_END,
    REMOTE_CLIENT_DISCONNECT,
    DD_REVERT_ON_DISCONNECT,
    MIN_QP_SWITCH,
    AUTO_UPDATE,
];

/// When a remote monitor is removed from Windows (`never`, `stream_end`,
/// `disconnect`), which holds what the two switches held before: Remove
/// when its stream ends and Remove when the device disconnects (both off by
/// default).
pub const REMOVE_REMOTE_MONITOR: &str = "remove_remote_monitor";
/// The two switches folded into [`REMOVE_REMOTE_MONITOR`].
pub const REMOTE_STREAM_END: &str = "remote_monitor_disconnect_on_stream_end";
pub const REMOTE_CLIENT_DISCONNECT: &str = "remote_monitor_disconnect_on_client_disconnect";
/// A switch's value in a JSON layer as text: a string as it is, anything but
/// null as its JSON, null (or nothing) as not saved.
fn layer_text(layer: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<String> {
    match layer.get(key) {
        Some(serde_json::Value::String(text)) => Some(text.clone()),
        Some(serde_json::Value::Null) | None => None,
        Some(other) => Some(other.to_string()),
    }
}
/// Removing a remote monitor for the two switches saved in one layer, each
/// `None` when that layer does not hold it (its earlier default, or the value
/// of the layers `below`, then counts): disconnect on the client's disconnect
/// is `disconnect`; else removing at the stream's end is `stream_end`; else
/// `never`. `None` when the layer holds neither. An unreadable value counts as
/// its default, as the host read it. `below` is Remove remote monitor as the
/// layers below hold it; `None` for the configuration file, which has none.
pub fn folded_remove_remote_monitor_below(
    stream_end: Option<&str>,
    client_disconnect: Option<&str>,
    below: Option<&str>,
) -> Option<&'static str> {
    if stream_end.is_none() && client_disconnect.is_none() {
        return None;
    }
    let below = below.map(|below| unquote(below.trim()));
    let read = |value: Option<&str>, default: bool| {
        value.and_then(super::parse_boolean).unwrap_or(default)
    };
    Some(if read(client_disconnect, below == Some("disconnect")) {
        "disconnect"
    } else if read(
        stream_end,
        matches!(below, Some("stream_end" | "disconnect")),
    ) {
        "stream_end"
    } else {
        "never"
    })
}
/// [`folded_remove_remote_monitor_below`] without layers below.
pub fn folded_remove_remote_monitor(
    stream_end: Option<&str>,
    client_disconnect: Option<&str>,
) -> Option<&'static str> {
    folded_remove_remote_monitor_below(stream_end, client_disconnect, None)
}
fn fold_remote_monitor_layer(
    layer: &mut serde_json::Map<String, serde_json::Value>,
    below: Option<&str>,
) -> bool {
    if !layer.contains_key(REMOTE_STREAM_END) && !layer.contains_key(REMOTE_CLIENT_DISCONNECT) {
        return false;
    }
    let stream_end = layer_text(layer, REMOTE_STREAM_END);
    let client_disconnect = layer_text(layer, REMOTE_CLIENT_DISCONNECT);
    // The client's disconnect on decides alone, and so do both switches
    // saved; otherwise the missing one comes from the layers below.
    let decided = layer.contains_key(REMOVE_REMOTE_MONITOR)
        || client_disconnect.as_deref().and_then(super::parse_boolean) == Some(true)
        || (layer.contains_key(REMOTE_STREAM_END) && layer.contains_key(REMOTE_CLIENT_DISCONNECT));
    if below.is_none() && !decided {
        return false;
    }
    layer.remove(REMOTE_STREAM_END);
    layer.remove(REMOTE_CLIENT_DISCONNECT);
    if !layer.contains_key(REMOVE_REMOTE_MONITOR)
        && let Some(folded) = folded_remove_remote_monitor_below(
            stream_end.as_deref(),
            client_disconnect.as_deref(),
            below,
        )
    {
        layer.insert(REMOVE_REMOTE_MONITOR.to_owned(), folded.into());
    }
    true
}
/// [`folded_remove_remote_monitor_below`] over a JSON layer, with the Remove
/// remote monitor the layers `below` hold: removes both switches and, unless
/// the layer already holds Remove remote monitor, sets it. Returns whether
/// anything changed.
pub fn fold_remove_remote_monitor(
    layer: &mut serde_json::Map<String, serde_json::Value>,
    below: &str,
) -> bool {
    fold_remote_monitor_layer(layer, Some(below))
}
/// [`fold_remove_remote_monitor`] at migration, where the layers below are
/// not known: only a layer whose switches decide it alone folds; one holding
/// a single switch that does not stays as saved and folds when the host
/// applies it over the layers below (`Config::fold_layer`).
pub fn fold_remove_remote_monitor_decided(
    layer: &mut serde_json::Map<String, serde_json::Value>,
) -> bool {
    fold_remote_monitor_layer(layer, None)
}
/// Adaptive bitrate for the two switches saved in one layer, each `None`
/// when that layer does not hold it (its earlier default, or the value of the
/// layers `below`, then counts): congestion control on is `delay`, as it ran
/// together with link-rate adaptation; link-rate adaptation off on its own is
/// `off`; anything else `on`. `None` when the layer holds neither. An
/// unreadable value counts as its default, as the host read it. `below` is
/// Adaptive bitrate as the layers below hold it; `None` for the configuration
/// file, which has none.
pub fn folded_bitrate_adaptation_below(
    link: Option<&str>,
    congestion: Option<&str>,
    below: Option<&str>,
) -> Option<&'static str> {
    if link.is_none() && congestion.is_none() {
        return None;
    }
    let below = below.map(|below| unquote(below.trim()));
    let read = |value: Option<&str>, default: bool| {
        value.and_then(super::parse_boolean).unwrap_or(default)
    };
    Some(if read(congestion, below == Some("delay")) {
        "delay"
    } else if !read(link, below != Some("off")) {
        "off"
    } else {
        "on"
    })
}
/// [`folded_bitrate_adaptation_below`] without layers below.
pub fn folded_bitrate_adaptation(
    link: Option<&str>,
    congestion: Option<&str>,
) -> Option<&'static str> {
    folded_bitrate_adaptation_below(link, congestion, None)
}
fn fold_bitrate_layer(
    layer: &mut serde_json::Map<String, serde_json::Value>,
    below: Option<&str>,
) -> bool {
    if !layer.contains_key(LINK_RATE) && !layer.contains_key(CONGESTION) {
        return false;
    }
    let link = layer_text(layer, LINK_RATE);
    let congestion = layer_text(layer, CONGESTION);
    // Congestion control on is `delay` whatever the link-rate switch says,
    // and both switches saved decide too; otherwise the missing one comes
    // from the layers below.
    let decided = layer.contains_key(BITRATE_ADAPTATION)
        || congestion.as_deref().and_then(super::parse_boolean) == Some(true)
        || (layer.contains_key(LINK_RATE) && layer.contains_key(CONGESTION));
    if below.is_none() && !decided {
        return false;
    }
    layer.remove(LINK_RATE);
    layer.remove(CONGESTION);
    if !layer.contains_key(BITRATE_ADAPTATION)
        && let Some(folded) =
            folded_bitrate_adaptation_below(link.as_deref(), congestion.as_deref(), below)
    {
        layer.insert(BITRATE_ADAPTATION.to_owned(), folded.into());
    }
    true
}
/// [`folded_bitrate_adaptation_below`] over a JSON layer, with the Adaptive
/// bitrate the layers `below` hold: removes both switches and, unless the
/// layer already holds Adaptive bitrate, sets it. Returns whether anything
/// changed.
pub fn fold_bitrate_adaptation(
    layer: &mut serde_json::Map<String, serde_json::Value>,
    below: &str,
) -> bool {
    fold_bitrate_layer(layer, Some(below))
}
/// [`fold_bitrate_adaptation`] at migration, where the layers below are not
/// known: only a layer whose switches decide it alone folds; one holding a
/// single switch that does not stays as saved and folds when the host applies
/// it over the layers below (`Config::fold_layer`).
pub fn fold_bitrate_adaptation_decided(
    layer: &mut serde_json::Map<String, serde_json::Value>,
) -> bool {
    fold_bitrate_layer(layer, None)
}
/// Packet pacing for the retired pacing settings saved in one layer, each
/// `None` when that layer does not hold it (its earlier default, or what the
/// layers `below` hold, then counts): a positive rate is the fixed rate;
/// adaptive pacing for every client and codec (`host_adaptive_pacing`) is
/// `always`; adaptive pacing switched off for wireless clients
/// (`wireless_adaptive_pacing=false`) alone is `off`; anything else is
/// `auto`, the default (`None`). An unreadable value counts as its default,
/// as the host read it. `below` is Packet pacing as the layers below hold it;
/// `None` for the configuration file, which has none.
pub fn folded_packet_pacing_below(
    rate: Option<&str>,
    host: Option<&str>,
    wireless: Option<&str>,
    below: Option<&str>,
) -> Option<String> {
    if rate.is_none() && host.is_none() && wireless.is_none() {
        return None;
    }
    let below = below.map(|below| unquote(below.trim()));
    let read = |value: Option<&str>, default: bool| {
        value.and_then(super::parse_boolean).unwrap_or(default)
    };
    let positive = |value: &str| super::parse_integer(value).is_some_and(|rate| rate > 0);
    // A rate the layer does not hold is the one below it, which owns pacing.
    let rate = match rate {
        Some(rate) => positive(rate).then_some(rate),
        None => below.filter(|below| positive(below)),
    };
    if let Some(rate) = rate {
        return Some(rate.to_owned());
    }
    if read(host, below == Some("always")) {
        return Some("always".to_owned());
    }
    if !read(wireless, below != Some("off")) {
        return Some("off".to_owned());
    }
    None
}
/// [`folded_packet_pacing_below`] without layers below.
pub fn folded_packet_pacing(
    rate: Option<&str>,
    host: Option<&str>,
    wireless: Option<&str>,
) -> Option<String> {
    folded_packet_pacing_below(rate, host, wireless, None)
}
fn fold_pacing_layer(
    layer: &mut serde_json::Map<String, serde_json::Value>,
    below: Option<&str>,
) -> bool {
    if ![PACING_RATE, HOST_PACING, WIRELESS_PACING]
        .iter()
        .any(|key| layer.contains_key(*key))
    {
        return false;
    }
    let rate = layer_text(layer, PACING_RATE);
    let host = layer_text(layer, HOST_PACING);
    let wireless = layer_text(layer, WIRELESS_PACING);
    // A rate, or adaptive pacing for everything, decides whatever the layers
    // below hold; so do both adaptive switches saved together. Otherwise the
    // layers below fill in what is missing.
    let decided = layer.contains_key(PACKET_PACING)
        || rate
            .as_deref()
            .and_then(super::parse_integer)
            .is_some_and(|rate| rate > 0)
        || host.as_deref().and_then(super::parse_boolean) == Some(true)
        || (layer.contains_key(HOST_PACING) && layer.contains_key(WIRELESS_PACING));
    if below.is_none() && !decided {
        return false;
    }
    layer.remove(PACING_RATE);
    layer.remove(HOST_PACING);
    layer.remove(WIRELESS_PACING);
    if !layer.contains_key(PACKET_PACING) {
        match folded_packet_pacing_below(
            rate.as_deref(),
            host.as_deref(),
            wireless.as_deref(),
            below,
        ) {
            Some(folded) => {
                layer.insert(PACKET_PACING.to_owned(), folded.into());
            }
            // Auto, over a layer below that holds something else.
            None if below.is_some_and(|below| unquote(below.trim()) != "auto") => {
                layer.insert(PACKET_PACING.to_owned(), "auto".into());
            }
            None => {}
        }
    }
    true
}
/// [`folded_packet_pacing_below`] over a JSON layer, with the Packet pacing
/// the layers `below` hold: removes the pacing rate and the two switches and,
/// unless the layer already holds Packet pacing, sets it. Returns whether
/// anything changed.
pub fn fold_packet_pacing(
    layer: &mut serde_json::Map<String, serde_json::Value>,
    below: &str,
) -> bool {
    fold_pacing_layer(layer, Some(below))
}
/// [`fold_packet_pacing`] at migration, where the layers below are not known:
/// only a layer whose settings decide it alone folds; one that does not stays
/// as saved and folds when the host applies it over the layers below
/// (`Config::fold_layer`).
pub fn fold_packet_pacing_decided(layer: &mut serde_json::Map<String, serde_json::Value>) -> bool {
    fold_pacing_layer(layer, None)
}
/// The PyroWave error-correction share. PyroWave follows Error correction
/// (`fec_percentage`) like every codec; only a value that differs from it
/// stays saved, and then keeps being read.
pub const PYROWAVE_FEC: &str = "pyrowave_critical_fec_percentage";
/// Whether a saved PyroWave share is the Error correction share anyway, so
/// it can go: each is `None` when its layer does not hold it, and an
/// unreadable value counts as the earlier default of 20, as the host read it.
pub fn pyrowave_fec_is_default(pyrowave: Option<&str>, fec: Option<&str>) -> bool {
    let read = |value: Option<&str>| value.and_then(super::parse_integer).unwrap_or(20);
    pyrowave.is_some() && read(pyrowave) == read(fec)
}
/// [`pyrowave_fec_is_default`] over a JSON layer that holds both values: a
/// PyroWave share equal to the layer's Error correction goes. A layer
/// without its own Error correction keeps it, as the share it differs from
/// is in a layer below. Returns whether anything changed.
pub fn fold_pyrowave_fec(layer: &mut serde_json::Map<String, serde_json::Value>) -> bool {
    let text = |value: &serde_json::Value| match value {
        serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    let (Some(pyrowave), Some(fec)) = (layer.get(PYROWAVE_FEC), layer.get("fec_percentage")) else {
        return false;
    };
    if pyrowave_fec_is_default(Some(&text(pyrowave)), Some(&text(fec))) {
        layer.remove(PYROWAVE_FEC);
        return true;
    }
    false
}
/// What happens to a device's display when it disconnects (`restore`, after
/// the restore delay, or `keep_while_paused`, until the paused-display
/// timeout), which holds what Restore displays on disconnect held before.
/// Keep monitors off while paused (`dd_hold_layout_while_paused`) stays its
/// own setting: it decides whether an exclusive layout may keep its
/// monitors off, so no old combination changes.
pub const DD_DISCONNECT_BEHAVIOR: &str = "dd_disconnect_behavior";
/// The setting folded into [`DD_DISCONNECT_BEHAVIOR`].
pub const DD_REVERT_ON_DISCONNECT: &str = "dd_config_revert_on_disconnect";
/// The disconnect behavior for Restore displays on disconnect as saved in
/// one layer: off is `keep_while_paused`, anything else `restore`. `None`
/// when the layer does not hold it. An unreadable value counts as its
/// default (on).
pub fn folded_disconnect_behavior(revert_on_disconnect: Option<&str>) -> Option<&'static str> {
    let revert = revert_on_disconnect?;
    Some(if super::parse_boolean(revert).unwrap_or(true) {
        "restore"
    } else {
        "keep_while_paused"
    })
}
/// [`folded_disconnect_behavior`] over a JSON layer: removes the setting
/// and, unless the layer already holds the disconnect behavior, sets it.
/// Returns whether anything changed.
pub fn fold_disconnect_behavior(layer: &mut serde_json::Map<String, serde_json::Value>) -> bool {
    let text = |value: serde_json::Value| match value {
        serde_json::Value::String(text) => Some(text),
        serde_json::Value::Null => None,
        other => Some(other.to_string()),
    };
    let Some(revert) = layer.remove(DD_REVERT_ON_DISCONNECT) else {
        return false;
    };
    if let Some(folded) = folded_disconnect_behavior(text(revert).as_deref())
        && !layer.contains_key(DD_DISCONNECT_BEHAVIOR)
    {
        layer.insert(DD_DISCONNECT_BEHAVIOR.to_owned(), folded.into());
    }
    true
}
/// The Minimum quantizer switch of NVIDIA NVENC, which earlier releases kept
/// beside a minimum QP for each codec (H.264, HEVC, AV1) and which no longer
/// exists: each QP now counts on its own, 0 (the default) being off.
pub const MIN_QP_SWITCH: &str = "nvenc_enable_min_qp";
/// The minimum QP of each codec and the value it had when the switch was on
/// and none was saved.
pub const MIN_QP: &[(&str, i64)] = &[
    ("nvenc_min_qp_h264", 19),
    ("nvenc_min_qp_hevc", 23),
    ("nvenc_min_qp_av1", 23),
];
/// Whether a saved Minimum quantizer switch was on (an unreadable value
/// counts as off, its default, as the host read it).
pub fn min_qp_switch_on(value: &str) -> bool {
    super::parse_boolean(value).unwrap_or(false)
}
/// The minimum QPs of a layer holding the retired Minimum quantizer switch.
/// On keeps each saved QP and writes the old default where the layer (or
/// `held`, what the configuration already holds) has none. Off writes 0 for
/// each codec, which is off now, so the layer still overrides a host that has
/// the switch on; with `reset` it writes null instead, to clear a QP the
/// configuration stores. A layer without the switch stays as saved. Returns
/// whether anything changed.
pub fn fold_min_qp(
    layer: &mut serde_json::Map<String, serde_json::Value>,
    held: impl Fn(&str) -> bool,
    reset: bool,
) -> bool {
    let Some(switch) = layer.remove(MIN_QP_SWITCH) else {
        return false;
    };
    let on = match &switch {
        serde_json::Value::String(text) => min_qp_switch_on(text),
        serde_json::Value::Null => false,
        other => min_qp_switch_on(&other.to_string()),
    };
    for &(key, default) in MIN_QP {
        if on {
            if !layer.contains_key(key) && !held(key) {
                layer.insert(key.to_owned(), default.into());
            }
        } else if reset {
            layer.insert(key.to_owned(), serde_json::Value::Null);
        } else {
            layer.insert(key.to_owned(), 0.into());
        }
    }
    true
}
/// Whether a configuration file was written by an earlier release: it holds a
/// setting this release folded into another or renamed (which includes the
/// Minimum quantizer switch). Check it before the file's settings move.
pub fn holds_retired_settings(values: &BTreeMap<String, String>) -> bool {
    values.keys().any(|key| {
        FOLDED_KEYS.contains(&key.as_str()) || RENAMED_KEYS.iter().any(|(old, _)| old == key)
    })
}
/// The minimum QPs of a configuration file ([`fold_min_qp`] over its
/// settings). The switch decides when the file saved it: on keeps each saved
/// QP and writes the old default for a codec with none, off drops them, as 0
/// is off. A file without the switch counts it as off, its earlier default,
/// when `old_file` says an earlier release wrote it
/// ([`holds_retired_settings`]): the QPs it saved never applied, so they
/// drop. A file of this release ignores the switch it does not have, and
/// keeps each QP, which applies on its own. Returns whether anything changed.
pub fn fold_min_qp_values(values: &mut BTreeMap<String, String>, old_file: bool) -> bool {
    let on = match values.remove(MIN_QP_SWITCH) {
        Some(switch) => min_qp_switch_on(&switch),
        None if old_file => false,
        None => return false,
    };
    for &(key, default) in MIN_QP {
        if on {
            values
                .entry(key.to_owned())
                .or_insert_with(|| default.to_string());
        } else {
            values.remove(key);
        }
    }
    true
}
/// Updates (`off`, `tell_me`, `install`), which holds what Install updates
/// automatically (`auto_update`) and an update check interval of 0 held
/// before.
pub const UPDATES: &str = "updates";
pub const AUTO_UPDATE: &str = "auto_update";
/// How often to check, in seconds. It stays a setting; a value of 0 (checks
/// off) became Updates `off`.
pub const UPDATE_CHECK_INTERVAL: &str = "update_check_interval";
/// Updates for what one layer saved, each `None` when it holds neither: an
/// interval of 0 or less was checks off, so `off`; Install updates
/// automatically on was `install`; anything else saved changes nothing (the
/// default is `tell_me`).
pub fn folded_updates(interval: Option<&str>, auto_update: Option<&str>) -> Option<&'static str> {
    if interval
        .and_then(super::parse_integer)
        .is_some_and(|seconds| seconds <= 0)
    {
        Some("off")
    } else if auto_update.and_then(super::parse_boolean) == Some(true) {
        Some("install")
    } else {
        None
    }
}
/// [`folded_updates`] over a JSON patch of the configuration (what an earlier
/// console or an API caller sends): removes `auto_update`, clears an interval
/// of 0 and sets Updates unless the patch sets it too. `held` is Updates as
/// the configuration holds it now, so that switching Install updates
/// automatically off, or a check interval on again, ends what the other
/// state held. Returns whether anything changed.
pub fn fold_updates(layer: &mut serde_json::Map<String, serde_json::Value>, held: &str) -> bool {
    let text = |value: &serde_json::Value| match value {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Null => None,
        other => Some(other.to_string()),
    };
    let auto_update = layer.remove(AUTO_UPDATE);
    let auto = auto_update.as_ref().and_then(text);
    let interval = layer.get(UPDATE_CHECK_INTERVAL).and_then(text);
    let seconds = interval.as_deref().and_then(super::parse_integer);
    let changed = auto_update.is_some() || seconds.is_some_and(|seconds| seconds <= 0);
    let mut folded = folded_updates(interval.as_deref(), auto.as_deref());
    if folded.is_none() {
        let held = unquote(held.trim());
        let auto_off = auto.as_deref().and_then(super::parse_boolean) == Some(false);
        if (auto_off && held == "install")
            || (seconds.is_some_and(|seconds| seconds > 0) && held == "off")
        {
            folded = Some("tell_me");
        }
    }
    if seconds.is_some_and(|seconds| seconds <= 0) {
        layer.insert(UPDATE_CHECK_INTERVAL.to_owned(), serde_json::Value::Null);
    }
    if let Some(folded) = folded
        && !layer.contains_key(UPDATES)
    {
        layer.insert(UPDATES.to_owned(), folded.into());
    }
    changed
}
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
/// host reads now:
///
/// - a renamed setting takes its current name (a value already under that
///   name wins) and a removed one goes;
/// - the HDR request folds into the layer's own Display HDR ([`HDR_REQUEST`]
///   says when it stays as saved);
/// - the two bitrate switches fold into its Adaptive bitrate, the pacing rate
///   and the two adaptive pacing switches into its Packet pacing, and the
///   remote monitor's two switches into its Remove remote monitor, each only
///   where the layer's own switches decide the result; a layer holding some
///   of them stays as saved, because the others come from the layers below,
///   and folds when the host applies it over them (`Config::fold_layer`);
/// - a PyroWave share equal to the layer's Error correction goes;
/// - Restore displays on disconnect folds into its disconnect behavior;
/// - the Minimum quantizer switch folds into its minimum QPs, off as 0 for
///   each codec so the layer still overrides the host's;
/// - a retired value takes its replacement.
///
/// Returns whether anything changed.
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
