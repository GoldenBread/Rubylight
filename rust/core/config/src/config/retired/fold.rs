//! Folding earlier multi-key settings into their current single-setting representations.
use super::super::retired_gen::{FOLDED_KEYS, LIBRARY_SYNC_OPTIONS, MIN_QP, RENAMED_KEYS};
use super::super::{parse_boolean, parse_integer, unquote};
use std::collections::BTreeMap;

/// Display HDR, which holds all four HDR choices: Match the stream (`auto`),
/// Always on (`force_on`), Always off (`force_off`) and Don't change HDR
/// (`disabled`).
pub const HDR_OPTION: &str = "dd_hdr_option";
/// The HDR request (follow the device, always on, always off) folded into Display HDR.
pub const HDR_REQUEST: &str = "dd_hdr_request_override";
/// Adaptive bitrate (`off`, `on`, `delay`).
pub const BITRATE_ADAPTATION: &str = "bitrate_adaptation";
/// The two switches folded into [`BITRATE_ADAPTATION`].
pub const LINK_RATE: &str = "link_rate_adaptation";
pub const CONGESTION: &str = "congestion_control";
/// Packet pacing (`auto`, `always`, `off`, or a fixed rate in kbps).
pub const PACKET_PACING: &str = "packet_pacing";
/// The pacing rate and adaptive pacing switches folded into [`PACKET_PACING`].
pub const PACING_RATE: &str = "pacing_max_bitrate_kbps";
pub const HOST_PACING: &str = "host_adaptive_pacing";
pub const WIRELESS_PACING: &str = "wireless_adaptive_pacing";
/// When a remote monitor is removed from Windows (`never`, `stream_end`, `disconnect`).
pub const REMOVE_REMOTE_MONITOR: &str = "remove_remote_monitor";
/// The two switches folded into [`REMOVE_REMOTE_MONITOR`].
pub const REMOTE_STREAM_END: &str = "remote_monitor_disconnect_on_stream_end";
pub const REMOTE_CLIENT_DISCONNECT: &str = "remote_monitor_disconnect_on_client_disconnect";
/// The PyroWave error-correction share.
pub const PYROWAVE_FEC: &str = "pyrowave_critical_fec_percentage";
/// What happens to a device's display when it disconnects (`restore`, `keep_while_paused`).
pub const DD_DISCONNECT_BEHAVIOR: &str = "dd_disconnect_behavior";
/// The setting folded into [`DD_DISCONNECT_BEHAVIOR`].
pub const DD_REVERT_ON_DISCONNECT: &str = "dd_config_revert_on_disconnect";
/// The retired Minimum quantizer switch of NVIDIA NVENC.
pub const MIN_QP_SWITCH: &str = "nvenc_enable_min_qp";
/// Updates (`off`, `tell_me`, `install`).
pub const UPDATES: &str = "updates";
pub const AUTO_UPDATE: &str = "auto_update";
/// How often to check, in seconds.
pub const UPDATE_CHECK_INTERVAL: &str = "update_check_interval";

/// Fold the launchers' `<launcher>_<option>` settings into the shared
/// `library_<option>` one (`LIBRARY_SYNC_OPTIONS`).
pub fn fold_library_sync(values: &mut BTreeMap<String, String>, old_file: bool) -> bool {
    let mut changed = false;
    if old_file {
        for &(option, default, _) in LIBRARY_SYNC_OPTIONS {
            let off = values
                .get(&format!("library_{option}"))
                .and_then(|value| parse_boolean(value))
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

/// A switch's value in a JSON layer as text: a string as it is, anything but
/// null as its JSON, null (or nothing) as not saved.
fn layer_text(layer: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<String> {
    match layer.get(key) {
        Some(serde_json::Value::String(text)) => Some(text.clone()),
        Some(serde_json::Value::Null) | None => None,
        Some(other) => Some(other.to_string()),
    }
}

/// Removing a remote monitor for the two switches saved in one layer.
pub fn folded_remove_remote_monitor_below(
    stream_end: Option<&str>,
    client_disconnect: Option<&str>,
    below: Option<&str>,
) -> Option<&'static str> {
    if stream_end.is_none() && client_disconnect.is_none() {
        return None;
    }
    let below = below.map(|below| unquote(below.trim()));
    let read =
        |value: Option<&str>, default: bool| value.and_then(parse_boolean).unwrap_or(default);
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
    let decided = layer.contains_key(REMOVE_REMOTE_MONITOR)
        || client_disconnect.as_deref().and_then(parse_boolean) == Some(true)
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

/// [`folded_remove_remote_monitor_below`] over a JSON layer with `below`.
pub fn fold_remove_remote_monitor(
    layer: &mut serde_json::Map<String, serde_json::Value>,
    below: &str,
) -> bool {
    fold_remote_monitor_layer(layer, Some(below))
}

/// [`fold_remove_remote_monitor`] at migration when layers below are not known.
pub fn fold_remove_remote_monitor_decided(
    layer: &mut serde_json::Map<String, serde_json::Value>,
) -> bool {
    fold_remote_monitor_layer(layer, None)
}

/// Adaptive bitrate for the two switches saved in one layer.
pub fn folded_bitrate_adaptation_below(
    link: Option<&str>,
    congestion: Option<&str>,
    below: Option<&str>,
) -> Option<&'static str> {
    if link.is_none() && congestion.is_none() {
        return None;
    }
    let below = below.map(|below| unquote(below.trim()));
    let read =
        |value: Option<&str>, default: bool| value.and_then(parse_boolean).unwrap_or(default);
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
    let decided = layer.contains_key(BITRATE_ADAPTATION)
        || congestion.as_deref().and_then(parse_boolean) == Some(true)
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

/// [`folded_bitrate_adaptation_below`] over a JSON layer with `below`.
pub fn fold_bitrate_adaptation(
    layer: &mut serde_json::Map<String, serde_json::Value>,
    below: &str,
) -> bool {
    fold_bitrate_layer(layer, Some(below))
}

/// [`fold_bitrate_adaptation`] at migration when layers below are not known.
pub fn fold_bitrate_adaptation_decided(
    layer: &mut serde_json::Map<String, serde_json::Value>,
) -> bool {
    fold_bitrate_layer(layer, None)
}

/// Packet pacing for the retired pacing settings saved in one layer.
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
    let read =
        |value: Option<&str>, default: bool| value.and_then(parse_boolean).unwrap_or(default);
    let positive = |value: &str| parse_integer(value).is_some_and(|rate| rate > 0);
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
    let decided = layer.contains_key(PACKET_PACING)
        || rate
            .as_deref()
            .and_then(parse_integer)
            .is_some_and(|rate| rate > 0)
        || host.as_deref().and_then(parse_boolean) == Some(true)
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
            None if below.is_some_and(|below| unquote(below.trim()) != "auto") => {
                layer.insert(PACKET_PACING.to_owned(), "auto".into());
            }
            None => {}
        }
    }
    true
}

/// [`folded_packet_pacing_below`] over a JSON layer with `below`.
pub fn fold_packet_pacing(
    layer: &mut serde_json::Map<String, serde_json::Value>,
    below: &str,
) -> bool {
    fold_pacing_layer(layer, Some(below))
}

/// [`fold_packet_pacing`] at migration when layers below are not known.
pub fn fold_packet_pacing_decided(layer: &mut serde_json::Map<String, serde_json::Value>) -> bool {
    fold_pacing_layer(layer, None)
}

/// Whether a saved PyroWave share is the Error correction share anyway, so it can go.
pub fn pyrowave_fec_is_default(pyrowave: Option<&str>, fec: Option<&str>) -> bool {
    let read = |value: Option<&str>| value.and_then(parse_integer).unwrap_or(20);
    pyrowave.is_some() && read(pyrowave) == read(fec)
}

/// [`pyrowave_fec_is_default`] over a JSON layer that holds both values.
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

/// The disconnect behavior for Restore displays on disconnect as saved in one layer.
pub fn folded_disconnect_behavior(revert_on_disconnect: Option<&str>) -> Option<&'static str> {
    let revert = revert_on_disconnect?;
    Some(if parse_boolean(revert).unwrap_or(true) {
        "restore"
    } else {
        "keep_while_paused"
    })
}

/// [`folded_disconnect_behavior`] over a JSON layer.
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

/// Whether a saved Minimum quantizer switch was on.
pub fn min_qp_switch_on(value: &str) -> bool {
    parse_boolean(value).unwrap_or(false)
}

/// The minimum QPs of a layer holding the retired Minimum quantizer switch.
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

/// Whether a configuration file was written by an earlier release.
pub fn holds_retired_settings(values: &BTreeMap<String, String>) -> bool {
    values.keys().any(|key| {
        FOLDED_KEYS.contains(&key.as_str()) || RENAMED_KEYS.iter().any(|(old, _)| old == key)
    })
}

/// The minimum QPs of a configuration file ([`fold_min_qp`] over its settings).
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

/// Updates for what one layer saved.
pub fn folded_updates(interval: Option<&str>, auto_update: Option<&str>) -> Option<&'static str> {
    if interval
        .and_then(parse_integer)
        .is_some_and(|seconds| seconds <= 0)
    {
        Some("off")
    } else if auto_update.and_then(parse_boolean) == Some(true) {
        Some("install")
    } else {
        None
    }
}

/// [`folded_updates`] over a JSON patch of the configuration.
pub fn fold_updates(layer: &mut serde_json::Map<String, serde_json::Value>, held: &str) -> bool {
    let text = |value: &serde_json::Value| match value {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Null => None,
        other => Some(other.to_string()),
    };
    let auto_update = layer.remove(AUTO_UPDATE);
    let auto = auto_update.as_ref().and_then(text);
    let interval = layer.get(UPDATE_CHECK_INTERVAL).and_then(text);
    let seconds = interval.as_deref().and_then(parse_integer);
    let changed = auto_update.is_some() || seconds.is_some_and(|seconds| seconds <= 0);
    let mut folded = folded_updates(interval.as_deref(), auto.as_deref());
    if folded.is_none() {
        let held = unquote(held.trim());
        let auto_off = auto.as_deref().and_then(parse_boolean) == Some(false);
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

/// Display HDR for a retired HDR `request` over the Display HDR `option`.
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

/// Display HDR for a retired HDR `request` saved in the same layer as Display HDR `option`.
pub fn folded_in_layer(option: &str, request: &str) -> Option<&'static str> {
    match unquote(option.trim()) {
        "force_on" | "force_off" => None,
        option => folded_hdr(Some(option), request),
    }
}
