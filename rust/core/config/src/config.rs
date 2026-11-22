//! The host configuration: `sunshine.conf`-compatible `key = value` settings,
//! typed accessors with Vibepollo's defaults, per-app and per-client overrides,
//! and the port layout derived from the base port.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

mod settings30;
pub use settings30::{DisplayAdjust, SETTINGS_30, TouchMode};

mod folders;
mod overrides;
mod retired;
mod text;
mod values;
use folders::parse_shared_folders;
pub use folders::{SharedFolderConfig, valid_folder_id, valid_folder_root};
pub use overrides::override_allowed;
use retired::retirement;
pub use retired::{
    AUTO_UPDATE, BACKGROUND_MODE_KEY, BITRATE_ADAPTATION, DD_DISCONNECT_BEHAVIOR,
    DD_REVERT_ON_DISCONNECT, DEVICE_OVERRIDES_CONFIG, DEVICE_OVERRIDES_SETTING, FOLDED_KEYS,
    HDR_OPTION, HDR_REQUEST, HOST_PACING, LIBRARY_SYNC_OPTIONS, LINK_RATE, MIN_QP, MIN_QP_SWITCH,
    PACING_RATE, PACKET_PACING, REMOTE_CLIENT_DISCONNECT, REMOTE_STREAM_END, REMOVE_REMOTE_MONITOR,
    REMOVED_APP_KEYS, REMOVED_KEYS, REMOVED_PREFIXES, RENAMED_APP_SWITCHES, RENAMED_KEYS,
    RETIRED_SETTINGS, RETIRED_VALUES, Retired, RetiredSetting, UPDATE_CHECK_INTERVAL, UPDATES,
    WIRELESS_PACING, current_name, fold_bitrate_adaptation, fold_bitrate_adaptation_decided,
    fold_disconnect_behavior, fold_library_sync, fold_min_qp, fold_min_qp_values,
    fold_packet_pacing, fold_packet_pacing_decided, fold_pyrowave_fec, fold_remove_remote_monitor,
    fold_remove_remote_monitor_decided, fold_updates, folded_bitrate_adaptation,
    folded_bitrate_adaptation_below, folded_disconnect_behavior, folded_hdr, folded_in_layer,
    folded_packet_pacing, folded_packet_pacing_below, folded_remove_remote_monitor,
    folded_remove_remote_monitor_below, folded_updates, holds_retired_settings, migrate_app,
    migrate_overrides, min_qp_switch_on, removed, replace_retired, replacement,
    retired_background_mode, retired_encoder, retired_frame_generation,
    retired_frame_limiter_provider, retired_name, retired_warning,
};
use text::{bracket_depth, decode, strip_comment};
pub use values::{
    fallback, first_report, invalid, number_text, parse_boolean, parse_integer, parse_time_of_day,
    refused, unquote,
};

/// Default UDP port for the DSU (cemuhook) motion server.
pub const DSU_DEFAULT_PORT: u16 = 26760;
/// Offset from the base port for the host's microphone receiver port.
pub const MIC_PORT_OFFSET: u16 = 13;
/// The setting that caps the picture every stream is encoded at.
pub const MAX_STREAM_RESOLUTION: &str = "max_stream_resolution";
/// The value that leaves every device the size it asks for: the default.
pub const MAX_STREAM_RESOLUTION_NATIVE: &str = "native";
/// The smallest and largest cap, the range the streamed display itself has
/// (`dd_manual_resolution`).
pub const MAX_STREAM_RESOLUTION_RANGE: (u32, u32, u32, u32) = (320, 200, 7680, 4320);

/// A resolution `WIDTHxHEIGHT`.
pub fn parse_resolution(text: &str) -> Option<(u32, u32)> {
    let (width, height) = text.trim().split_once(['x', 'X'])?;
    let width = width.trim().parse::<u32>().ok()?;
    let height = height.trim().parse::<u32>().ok()?;
    ((1..=16384).contains(&width) && (1..=16384).contains(&height)).then_some((width, height))
}

/// The cap `max_stream_resolution` sets: `None` for `native` (or an empty
/// value), which leaves each device the picture it asks for, and an error for
/// anything else but a `WIDTHxHEIGHT` within [`MAX_STREAM_RESOLUTION_RANGE`].
pub fn parse_max_stream_resolution(text: &str) -> Result<Option<(u32, u32)>> {
    let text = number_text(text);
    if text.is_empty() || text.eq_ignore_ascii_case(MAX_STREAM_RESOLUTION_NATIVE) {
        return Ok(None);
    }
    let (min_width, min_height, max_width, max_height) = MAX_STREAM_RESOLUTION_RANGE;
    match parse_resolution(text) {
        Some((width, height))
            if (min_width..=max_width).contains(&width)
                && (min_height..=max_height).contains(&height) =>
        {
            Ok(Some((width, height)))
        }
        _ => bail!(
            "{MAX_STREAM_RESOLUTION} must be {MAX_STREAM_RESOLUTION_NATIVE} or WIDTHxHEIGHT between {min_width}x{min_height} and {max_width}x{max_height}"
        ),
    }
}

/// Updates ([`Config::updates`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdatesMode {
    /// No automatic check; Check now still works.
    Off,
    /// Checks and tells the user about a new release.
    TellMe,
    /// As `TellMe`, and installs the release when nothing is streaming.
    Install,
}

/// Adaptive bitrate ([`Config::bitrate_adaptation`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BitrateAdaptation {
    /// The stream keeps the bitrate it was set to.
    Off,
    /// Lowered on loss error correction cannot repair, then raised again.
    On,
    /// As `On`, and also lowered as network delay builds (congestion control).
    Delay,
}

/// The most video FEC a device may ask for (`rlFecPercentage`) or reach
/// through its loss reports, in percent of the video packets: half of them.
pub const FEC_MAX_PERCENTAGE: u8 = 50;

/// Packet pacing ([`Config::packet_pacing`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PacketPacing {
    /// Adaptive pacing for clients that report a Wi-Fi or cellular route on
    /// H.264, HEVC and AV1; every other stream keeps the route policy.
    Auto,
    /// Adaptive pacing for every client and codec, PyroWave included.
    Always,
    /// No adaptive pacing; the loss latch may still pace after video loss.
    Off,
    /// A fixed send rate in kbps that takes over pacing.
    Fixed(i64),
}

/// When a remote monitor is removed from Windows ([`Config::remove_remote_monitor`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoveRemoteMonitor {
    /// Kept after its stream ends, for the device to resume where it was.
    Never,
    /// Removed when its stream ends.
    StreamEnd,
    /// Removed as soon as the device's connection drops, even while kept for resuming.
    Disconnect,
}

pub const HARDWARE_METRICS: &[&str] = &[
    "cpu_usage_percent",
    "gpu_usage_percent",
    "gpu_clock_mhz",
    "gpu_memory_clock_mhz",
    "gpu_temperature_c",
    "gpu_hotspot_temperature_c",
    "gpu_memory_temperature_c",
    "gpu_voltage_mv",
    "gpu_fan_rpm",
    "gpu_fan_percent",
    "gpu_power_w",
    "gpu_board_power_w",
    "gpu_vram_used_mb",
    "gpu_vram_usage_percent",
];

/// Retain all keys, including extension keys a newer UI or client may write.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    pub values: BTreeMap<String, String>,
}
impl Config {
    /// Read `sunshine.conf` as Vibepollo does, so an imported file never stops
    /// the host: a line without `=` or with an unusable key is skipped with a
    /// warning, characters before a key (a UTF-8 byte order mark) are dropped,
    /// the first of duplicate keys wins and an empty value keeps the default.
    /// `#` starts a comment outside quotes, and a value opening `[` or `{`
    /// continues until its brackets close. A setting saved under an earlier
    /// name is read under its current one (`RENAMED_KEYS`).
    pub fn parse(text: &str) -> Result<Self> {
        Ok(Self::parse_retiring(text)?.0)
    }
    /// `parse`, and the retired values it replaced (`RETIRED_VALUES`,
    /// `RETIRED_SETTINGS`).
    pub fn parse_retiring(text: &str) -> Result<(Self, Vec<Retired>)> {
        let (mut config, retired) = Self::parse_names_as_written(text)?;
        config.rename_keys();
        Ok((config, retired))
    }
    fn parse_names_as_written(text: &str) -> Result<(Self, Vec<Retired>)> {
        let mut values = BTreeMap::new();
        let mut retired = Vec::new();
        let mut lines = text.lines().enumerate();
        while let Some((number, raw)) = lines.next() {
            let number = number + 1;
            if raw.contains('\0') {
                tracing::warn!(line = number, "configuration line contains NUL; skipped");
                continue;
            }
            let line = strip_comment(raw);
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                tracing::warn!(line = number, "configuration line has no '='; skipped");
                continue;
            };
            let key = key
                .trim()
                .trim_start_matches(|c: char| !(c.is_ascii_alphanumeric() || c == '_'));
            let mut value = value.trim().to_owned();
            if value.starts_with(['[', '{']) {
                let continuation = lines.clone();
                let mut depth = bracket_depth(&value);
                while depth > 0 {
                    let Some((_, next)) = lines.next() else {
                        break;
                    };
                    let next = strip_comment(next);
                    depth += bracket_depth(&next);
                    value.push('\n');
                    value.push_str(next.trim());
                }
                if depth != 0 {
                    lines = continuation;
                    tracing::warn!(
                        line = number,
                        key,
                        "configuration list is not closed; skipped"
                    );
                    continue;
                }
            }
            if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                tracing::warn!(line = number, key, "invalid configuration key; skipped");
                continue;
            }
            if value.is_empty() || removed(key) {
                continue;
            }
            if values.contains_key(key) {
                tracing::warn!(
                    line = number,
                    key,
                    "configuration key repeated; the first value is used"
                );
                continue;
            }
            // The console then shows, and the next save writes, the
            // replacement.
            if let Some((current, reason)) = retirement(key, &value) {
                tracing::info!(
                    key,
                    retired = value,
                    current,
                    "retired setting value replaced"
                );
                retired.push(Retired {
                    key: key.to_owned(),
                    value: std::mem::replace(&mut value, current.to_owned()),
                    current,
                    reason,
                });
            }
            values.insert(key.to_owned(), value);
        }
        Ok((Self { values }, retired))
    }
    /// Move settings found under an earlier name to their current one. When
    /// both names are set the current one is what the host has read, so it
    /// stays and the earlier one is dropped. The file keeps the earlier name
    /// until the console next saves, which writes every setting under the
    /// name it is held under here.
    fn rename_keys(&mut self) {
        // Whether an earlier release wrote this file, before its old names go.
        let old_file = holds_retired_settings(&self.values);
        for &(old, new) in RENAMED_KEYS {
            if let Some(value) = self.values.remove(old)
                && !self.values.contains_key(new)
            {
                self.values.insert(new.to_owned(), value);
            }
        }
        // Each launcher's sync options fold into the shared Library sync
        // settings; a launcher that differs keeps its key as an override.
        fold_library_sync(&mut self.values, old_file);
        if let Some(request) = self.values.remove(HDR_REQUEST)
            && let Some(folded) = folded_in_layer(
                self.values.get(HDR_OPTION).map_or("auto", String::as_str),
                &request,
            )
        {
            self.values.insert(HDR_OPTION.to_owned(), folded.to_owned());
        }
        // The two bitrate switches fold into Adaptive bitrate; a value
        // already saved under it wins.
        let link = self.values.remove(retired::LINK_RATE);
        let congestion = self.values.remove(retired::CONGESTION);
        if let Some(folded) = folded_bitrate_adaptation(link.as_deref(), congestion.as_deref())
            && !self.values.contains_key(BITRATE_ADAPTATION)
        {
            self.values
                .insert(BITRATE_ADAPTATION.to_owned(), folded.to_owned());
        }
        // A PyroWave share equal to the error correction share is that share.
        if let Some(value) = self.values.get(retired::PYROWAVE_FEC)
            && retired::pyrowave_fec_is_default(
                Some(value),
                self.values.get("fec_percentage").map(String::as_str),
            )
        {
            self.values.remove(retired::PYROWAVE_FEC);
        }
        // The pacing rate and the two adaptive pacing switches fold into
        // Packet pacing; a value already saved under it wins.
        let rate = self.values.remove(retired::PACING_RATE);
        let host = self.values.remove(retired::HOST_PACING);
        let wireless = self.values.remove(retired::WIRELESS_PACING);
        if let Some(folded) =
            folded_packet_pacing(rate.as_deref(), host.as_deref(), wireless.as_deref())
            && !self.values.contains_key(PACKET_PACING)
        {
            self.values.insert(PACKET_PACING.to_owned(), folded);
        }
        // The remote monitor's two switches fold into Remove remote monitor;
        // a value already saved under it wins.
        let stream_end = self.values.remove(REMOTE_STREAM_END);
        let client_disconnect = self.values.remove(REMOTE_CLIENT_DISCONNECT);
        if let Some(folded) =
            folded_remove_remote_monitor(stream_end.as_deref(), client_disconnect.as_deref())
            && !self.values.contains_key(REMOVE_REMOTE_MONITOR)
        {
            self.values
                .insert(REMOVE_REMOTE_MONITOR.to_owned(), folded.to_owned());
        }
        // Restore displays on disconnect folds into the disconnect behavior;
        // a value already saved wins.
        let revert = self.values.remove(DD_REVERT_ON_DISCONNECT);
        if let Some(folded) = folded_disconnect_behavior(revert.as_deref())
            && !self.values.contains_key(DD_DISCONNECT_BEHAVIOR)
        {
            self.values
                .insert(DD_DISCONNECT_BEHAVIOR.to_owned(), folded.to_owned());
        }
        // Install updates automatically and an interval of 0 fold into
        // Updates; a value already saved under it wins.
        let auto_update = self.values.remove(AUTO_UPDATE);
        let interval = self.values.get(UPDATE_CHECK_INTERVAL).cloned();
        if let Some(folded) = folded_updates(interval.as_deref(), auto_update.as_deref()) {
            if folded == "off" {
                self.values.remove(UPDATE_CHECK_INTERVAL);
            }
            self.values
                .entry(UPDATES.to_owned())
                .or_insert_with(|| folded.to_owned());
        }
        // The Minimum quantizer switch folds into the minimum QPs: on keeps
        // them (the old defaults where none is saved), off clears them. An
        // earlier file without the switch had it off.
        fold_min_qp_values(&mut self.values, old_file);
    }
    pub fn load(path: &Path) -> Result<Self> {
        Ok(Self::load_retiring(path)?.0)
    }
    /// `load`, and the retired values it replaced. The host writes the
    /// replacements back and warns once, so the next start is quiet.
    pub fn load_retiring(path: &Path) -> Result<(Self, Vec<Retired>)> {
        match std::fs::read(path) {
            Ok(bytes) => Self::parse_retiring(&decode(&bytes))
                .with_context(|| format!("reading {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Default::default()),
            Err(e) => Err(e.into()),
        }
    }
    /// The value of `key`, or `default` when it is unset or empty.
    pub fn get<'a>(&'a self, key: &str, default: &'a str) -> &'a str {
        self.values
            .get(key)
            .map(String::as_str)
            .filter(|v| !v.trim().is_empty())
            .unwrap_or(default)
    }
    pub fn boolean(&self, key: &str, default: bool) -> bool {
        self.values
            .get(key)
            .and_then(|s| parse_boolean(s))
            .unwrap_or(default)
    }
    /// Whether direct driver push (`idd-push`) uses the GPU fence ring. It is the default;
    /// `idd_push_fence_ring = false` selects the keyed-mutex ring. Capture reuse and capture
    /// itself read this one answer so they cannot disagree.
    pub fn idd_push_fence_ring(&self) -> bool {
        self.boolean("idd_push_fence_ring", true)
    }
    /// Whether Nightjar clients are offered packed full chroma
    /// (`packed_444`, on unless turned off; earlier releases called it
    /// `experimental_packed_444`). It only offers the modes: a stream uses one
    /// only when its client asks, so a client that does not ask pays nothing
    /// for it.
    pub fn packed_444(&self) -> bool {
        self.boolean("packed_444", true)
    }
    /// Whether paired clients may read hardware metrics during a stream
    /// (`hardware_metrics`, on unless turned off). Readings are taken only
    /// while a client asks for them.
    pub fn hardware_metrics(&self) -> bool {
        self.boolean("hardware_metrics", true)
    }
    /// Whether a disconnected device's display is kept while its app runs
    /// (`dd_disconnect_behavior=keep_while_paused`) instead of restored after
    /// the restore delay (`restore`, the default). An unknown value reads as
    /// `restore`.
    pub fn keeps_display_while_paused(&self) -> bool {
        let value = self.get(DD_DISCONNECT_BEHAVIOR, "restore");
        match unquote(value.trim()).to_ascii_lowercase().as_str() {
            "keep_while_paused" => true,
            "restore" => false,
            _ => {
                invalid(DD_DISCONNECT_BEHAVIOR, value);
                false
            }
        }
    }
    /// How long a disconnected stream may keep its display. Exclusive layouts
    /// must release even when an older profile disables restoration: otherwise
    /// the local user's physical displays can remain off for hours. Keeping the
    /// monitors off while paused (`dd_hold_layout_while_paused`, which no older
    /// profile carries) is the user's choice to keep them off: such a layout keeps
    /// its display as long as an ordinary one would (issue #92).
    pub fn display_disconnect_delay(&self, exclusive: bool) -> Option<std::time::Duration> {
        let forced = exclusive && !self.boolean("dd_hold_layout_while_paused", false);
        if forced || !self.keeps_display_while_paused() {
            Some(std::time::Duration::from_millis(
                self.integer("dd_config_revert_delay", 3000).max(0) as u64,
            ))
        } else {
            let seconds = self
                .integer("dd_paused_virtual_display_timeout_secs", 7200)
                .max(0);
            (seconds != 0).then(|| std::time::Duration::from_secs(seconds as u64))
        }
    }
    /// Packet pacing: `auto` (the default: adaptive pacing for a client that
    /// reports a Wi-Fi or cellular route on H.264, HEVC and AV1), `always`
    /// (every client and codec, PyroWave included), `off`, or a fixed rate
    /// in kbps. An unreadable value is reported and reads as `auto`; a rate
    /// of 0 or below is no rate, as the earlier pacing limit read it.
    pub fn packet_pacing(&self) -> PacketPacing {
        let value = self.get(PACKET_PACING, "auto");
        match unquote(value.trim()).to_ascii_lowercase().as_str() {
            "auto" => PacketPacing::Auto,
            "always" => PacketPacing::Always,
            "off" => PacketPacing::Off,
            _ => match parse_integer(value) {
                Some(rate) if rate > 0 => PacketPacing::Fixed(rate),
                // A rate of 0 or below is no rate, as the earlier pacing
                // limit read it.
                Some(_) => PacketPacing::Auto,
                None => {
                    invalid(PACKET_PACING, value);
                    PacketPacing::Auto
                }
            },
        }
    }
    /// When a remote monitor is removed from Windows: `never` (the default,
    /// kept for the device to resume where it was), `stream_end` or
    /// `disconnect`. An unknown value is reported and reads as `never`.
    pub fn remove_remote_monitor(&self) -> RemoveRemoteMonitor {
        let value = self.get(REMOVE_REMOTE_MONITOR, "never");
        match unquote(value.trim()).to_ascii_lowercase().as_str() {
            "never" => RemoveRemoteMonitor::Never,
            "stream_end" => RemoveRemoteMonitor::StreamEnd,
            "disconnect" => RemoveRemoteMonitor::Disconnect,
            _ => {
                invalid(REMOVE_REMOTE_MONITOR, value);
                RemoveRemoteMonitor::Never
            }
        }
    }
    /// Whether a remote monitor is removed when its stream ends: for
    /// `stream_end`, and for `disconnect`, which removes it then and when the
    /// device disconnects (both earlier switches on removed it at both).
    pub fn removes_remote_monitor_at_stream_end(&self) -> bool {
        self.remove_remote_monitor() != RemoveRemoteMonitor::Never
    }
    /// Whether the stream's packet pacing adapts to the link, by Packet
    /// pacing: `always` for every client on every codec (codec 0-3, PyroWave
    /// included), `auto` for a client that reports a Wi-Fi or cellular route
    /// (`client_wireless`) on H.264, HEVC and AV1 (codec 0-2) while PyroWave
    /// keeps its calibrated-pace policy, and never for `off` or a fixed rate.
    pub fn adaptive_packet_pacing(&self, codec: u8, client_wireless: bool) -> bool {
        match self.packet_pacing() {
            PacketPacing::Always => codec <= 3,
            PacketPacing::Auto => client_wireless && codec <= 2,
            PacketPacing::Off | PacketPacing::Fixed(_) => false,
        }
    }
    /// The fixed packet pacing rate in kbps, `None` when Packet pacing is
    /// not a positive rate. A fixed rate owns pacing: adaptive pacing and
    /// the loss latch both stay off.
    pub fn manual_pacing_kbps(&self) -> Option<i64> {
        match self.packet_pacing() {
            PacketPacing::Fixed(rate) => Some(rate),
            _ => None,
        }
    }
    /// Pace like Wi-Fi after a lost keyframe or repeated frame loss (on by default). A
    /// manual pacing rate keeps its own policy, and so does a stream the
    /// adaptive pacing controller owns.
    pub fn keyframe_loss_pacing(&self, codec: u8, client_wireless: bool) -> bool {
        self.boolean("keyframe_loss_pacing", true)
            && self.manual_pacing_kbps().is_none()
            && !self.adaptive_packet_pacing(codec, client_wireless)
    }
    /// Adaptive bitrate: `off`, `on` (the default: lower the bitrate on loss
    /// error correction cannot repair) or `delay` (also from rising network
    /// delay). An unknown value is reported and reads as `on`.
    pub fn bitrate_adaptation(&self) -> BitrateAdaptation {
        let value = self.get(BITRATE_ADAPTATION, "on");
        match unquote(value.trim()).to_ascii_lowercase().as_str() {
            "off" => BitrateAdaptation::Off,
            "on" => BitrateAdaptation::On,
            "delay" => BitrateAdaptation::Delay,
            _ => {
                invalid(BITRATE_ADAPTATION, value);
                BitrateAdaptation::On
            }
        }
    }
    /// Lower the encoder bitrate when the link cannot carry the stream:
    /// Adaptive bitrate on, or on with network delay.
    pub fn link_rate_adaptation(&self) -> bool {
        self.bitrate_adaptation() != BitrateAdaptation::Off
    }
    /// Packet-level congestion control from the client's arrival feedback: Adaptive bitrate's
    /// `delay` (not the default until the A/B decides).
    pub fn congestion_control(&self) -> bool {
        self.bitrate_adaptation() == BitrateAdaptation::Delay
    }
    /// An integer setting; Vibepollo also writes quoted (`"756"`) and
    /// hexadecimal (`0x2a`) values.
    pub fn integer(&self, key: &str, default: i64) -> i64 {
        let Some(value) = self.values.get(key).filter(|v| !v.trim().is_empty()) else {
            return default;
        };
        parse_integer(value).unwrap_or_else(|| {
            invalid(key, value);
            default
        })
    }
    /// A list setting. JSON arrays and Vibepollo's `[a, b]` or `a, b` lists
    /// are accepted; numbers are returned as written.
    pub fn list(&self, key: &str) -> Vec<String> {
        let Some(value) = self.values.get(key) else {
            return Vec::new();
        };
        if let Ok(serde_json::Value::Array(items)) = serde_json::from_str(value) {
            return items
                .into_iter()
                .map(|item| match item {
                    serde_json::Value::String(s) => s,
                    other => other.to_string(),
                })
                .filter(|s| !s.trim().is_empty())
                .collect();
        }
        let inner = value.trim();
        let inner = inner.strip_prefix('[').unwrap_or(inner);
        let inner = inner.strip_suffix(']').unwrap_or(inner);
        let mut items = Vec::new();
        let (mut depth, mut quote, mut start) = (0i32, false, 0);
        for (i, c) in inner.char_indices() {
            match c {
                '"' => quote = !quote,
                '[' | '{' if !quote => depth += 1,
                ']' | '}' if !quote => depth -= 1,
                ',' if !quote && depth == 0 => {
                    items.push(&inner[start..i]);
                    start = i + 1;
                }
                _ => {}
            }
        }
        items.push(&inner[start..]);
        items
            .into_iter()
            .map(|item| unquote(item.trim()).to_owned())
            .filter(|item| !item.is_empty())
            .collect()
    }
    pub fn hardware_metrics_interval(&self) -> u64 {
        self.integer("hardware_metrics_interval", 1000)
            .clamp(500, 10000) as u64
    }
    /// `virtual_display_mode`; unset, it is per client where the OS ships the
    /// IddCx the driver needs (`os_gate::virtual_display`, Windows 11 23H2 and
    /// later) and disabled before that, as Vibepollo does on Windows 10.
    pub fn virtual_display_mode(&self, windows_11: bool) -> &str {
        self.get(
            "virtual_display_mode",
            if windows_11 { "per_client" } else { "disabled" },
        )
    }
    /// `max_stream_resolution`: the host-wide cap on the picture every stream
    /// is encoded at, `None` for `native` (the default), which codes what each
    /// device asks for. A value the console would refuse is reported and
    /// behaves as native.
    pub fn max_stream_resolution(&self) -> Option<(u32, u32)> {
        let value = self.get(MAX_STREAM_RESOLUTION, MAX_STREAM_RESOLUTION_NATIVE);
        parse_max_stream_resolution(value)
            .inspect_err(|_| invalid(MAX_STREAM_RESOLUTION, value))
            .ok()
            .flatten()
    }
    pub fn log_level(&self) -> &'static str {
        match self
            .get("min_log_level", "info")
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "0" | "verbose" | "trace" => "trace",
            "1" | "debug" => "debug",
            "2" | "info" => "info",
            "3" | "warning" | "warn" => "warn",
            "4" | "5" | "error" | "fatal" => "error",
            "6" | "none" | "off" => "off",
            _ => "info",
        }
    }
    /// The base port. An out-of-range port is ignored, as in Vibepollo.
    pub fn port(&self) -> Result<u16> {
        let n = self.integer("port", 47989);
        if !PORTS.contains(&n) {
            invalid("port", self.get("port", ""));
            return Ok(47989);
        }
        Ok(n as u16)
    }
    pub fn ports(&self) -> Result<Ports> {
        Ok(Ports::from_base(self.port()?))
    }
    /// Client roaming (`client_roaming`, on by default): DESCRIBE offers it, and a client
    /// that proves a new address keeps its stream. Nothing happens until an authenticated
    /// probe arrives (`roaming`).
    pub fn client_roaming(&self) -> bool {
        self.boolean("client_roaming", true)
    }
    /// HDR10+ and HDR Vivid dynamic metadata in HDR streams (`hdr_dynamic_metadata`, on
    /// by default): per-frame brightness statistics for displays that tone-map with them.
    pub fn hdr_dynamic_metadata(&self) -> bool {
        self.boolean("hdr_dynamic_metadata", true)
    }
    /// PCM, Dolby Digital (AC3) and Dolby Digital Plus (E-AC3) audio for clients that pick one
    /// (`audio_passthrough`, on by default): serverinfo and DESCRIBE offer the codecs, and stock
    /// clients keep Opus. AC3 and E-AC3 also need FFmpeg's libraries beside the host.
    pub fn audio_passthrough(&self) -> bool {
        self.boolean("audio_passthrough", true)
    }
    /// The PyroWave error-correction share, in percent: `fec_percentage`, as
    /// for every codec, unless a PyroWave value that differed from it was
    /// kept (`pyrowave_critical_fec_percentage`, [`retired::PYROWAVE_FEC`]).
    pub fn pyrowave_fec_percentage(&self) -> i64 {
        if self.values.contains_key(retired::PYROWAVE_FEC) {
            self.integer(retired::PYROWAVE_FEC, 20)
        } else {
            self.integer("fec_percentage", 20)
        }
    }
    /// `GET /bandwidth-probe` for paired devices that time a download before
    /// they pick a bitrate (`bandwidth_probe`, on by default).
    pub fn bandwidth_probe(&self) -> bool {
        self.boolean("bandwidth_probe", true)
    }
    /// Let devices use Frame Warp (`frame_warp`, on by default): the host
    /// offers `FrameWarpVersion` and 0x5543-0x5546, and does the work only
    /// for a client that asks.
    pub fn frame_warp(&self) -> bool {
        self.boolean("frame_warp", true)
    }
    /// Show the device's keyboard when a text field on the host gets the focus
    /// (`keyboard_follows_focus`, on by default, Experimental): the host offers
    /// text input (0x553B) and watches the focus for devices that ask.
    pub fn keyboard_follows_focus(&self) -> bool {
        self.boolean("keyboard_follows_focus", true)
    }
    /// Updates: `off`, `tell_me` (the default: check and tell the user about a
    /// new release) or `install` (also install it when nothing is streaming).
    /// An unknown value is reported and reads as `tell_me`.
    pub fn updates(&self) -> UpdatesMode {
        let value = self.get(UPDATES, "tell_me");
        match unquote(value.trim()).to_ascii_lowercase().as_str() {
            "off" => UpdatesMode::Off,
            "tell_me" => UpdatesMode::TellMe,
            "install" => UpdatesMode::Install,
            _ => {
                invalid(UPDATES, value);
                UpdatesMode::TellMe
            }
        }
    }
    /// Seconds between automatic update checks (`update_check_interval`, a day
    /// unless set); 0 or less is no check, as is Updates `off`.
    pub fn update_check_interval(&self) -> i64 {
        if self.updates() == UpdatesMode::Off {
            0
        } else {
            self.integer(UPDATE_CHECK_INTERVAL, 86400)
        }
    }
    /// `update_check_time`: the time of day (`HH:MM` on this PC's clock) of
    /// the daily automatic update check, as seconds after midnight. `None`
    /// when it is unset, and for text that is not a time (reported once):
    /// the check then follows `update_check_interval`.
    pub fn update_check_time(&self) -> Option<u32> {
        let value = self
            .values
            .get("update_check_time")
            .filter(|v| !v.trim().is_empty())?;
        let time = parse_time_of_day(value);
        if time.is_none() {
            invalid("update_check_time", value);
        }
        time
    }
    /// Send the text around the cursor to the device's keyboard
    /// (`text_input_surrounding_text`, off by default); never for password or
    /// other sensitive fields.
    pub fn text_input_surrounding_text(&self) -> bool {
        self.boolean("text_input_surrounding_text", false)
    }
    /// `keyboard_layout`: `client` (the default, the layout the device reports
    /// in 0x553E), `us` or an 8-hex-digit Windows layout id such as `00000407`.
    /// The host turns a device's keys into scan codes with it.
    pub fn keyboard_layout(&self) -> KeyboardLayoutSetting {
        match unquote(self.get("keyboard_layout", "client").trim()).trim() {
            "" | "client" => KeyboardLayoutSetting::Client,
            "us" => KeyboardLayoutSetting::Us,
            other => match parse_klid(other) {
                Some(klid) => KeyboardLayoutSetting::Klid(klid),
                None => {
                    invalid("keyboard_layout", other);
                    KeyboardLayoutSetting::Client
                }
            },
        }
    }
    /// The Dolby Vision offer on HEVC and AV1 HDR streams, for devices that
    /// pick it (`hdr_dolby_vision`, on by default; `extensions::hdr_offer`):
    /// 8.1 on HEVC, and 8.4 while `hdr_hlg` is on too; 10.1 on AV1.
    pub fn hdr_dolby_vision(&self) -> bool {
        self.boolean("hdr_dolby_vision", true)
    }
    /// The HLG offer on HEVC and AV1 HDR streams, for devices that pick it
    /// (`hdr_hlg`, on by default; `extensions::hdr_offer`).
    pub fn hdr_hlg(&self) -> bool {
        self.boolean("hdr_hlg", true)
    }
    /// The WebRTC server for TV and browser clients (off by default).
    pub fn webrtc_server(&self) -> bool {
        self.boolean("webrtc_server", false)
    }
    /// File transfer from paired devices (off by default). A device also
    /// needs its Upload files or Download files permission.
    pub fn file_transfer(&self) -> bool {
        self.boolean("file_transfer", false)
    }
    /// USB forwarding from devices with `allow_usb` (off by default,
    /// Experimental).
    pub fn usb_forwarding(&self) -> bool {
        self.boolean("usb_forwarding", false)
    }
    /// How many forwarded USB devices the host takes at once
    /// (`usb_max_devices`, 1 to 16, 4 by default). Outside that is reported
    /// and the default is used.
    pub fn usb_max_devices(&self) -> u8 {
        let n = self.integer("usb_max_devices", 4);
        if (1..=16).contains(&n) {
            n as u8
        } else {
            invalid("usb_max_devices", self.get("usb_max_devices", ""));
            4
        }
    }
    /// The host-drawn couch menu (on by default, Experimental). Off means no
    /// input filter, no menu worker and no `session.couch_menu` feature.
    pub fn couch_menu(&self) -> bool {
        self.boolean("couch_menu", true)
    }
    /// The folder transfers use; `None` (empty) is the signed-in user's
    /// `Downloads\Rubylight`.
    pub fn file_transfer_dir(&self) -> Option<std::path::PathBuf> {
        let dir = unquote(self.get("file_transfer_dir", "").trim()).trim();
        (!dir.is_empty()).then(|| std::path::PathBuf::from(dir))
    }
    /// The largest upload in bytes (`file_transfer_max_mb`, MiB, 4096 by
    /// default). Below 1 MiB is reported and the default is used.
    pub fn file_transfer_max_bytes(&self) -> u64 {
        const MIB: i64 = 1024 * 1024;
        let mut mib = self.integer("file_transfer_max_mb", 4096);
        if !(1..=i64::MAX / MIB).contains(&mib) {
            invalid("file_transfer_max_mb", self.get("file_transfer_max_mb", ""));
            mib = 4096;
        }
        (mib * MIB) as u64
    }
    /// How fast uploads are written while a stream runs, in bytes per
    /// second (`file_transfer_stream_mbps`, 50 by default). 0 or less does
    /// not slow them.
    pub fn file_transfer_stream_rate(&self) -> Option<u64> {
        let mbps = self.integer("file_transfer_stream_mbps", 50);
        (mbps > 0).then(|| (mbps as u64).saturating_mul(125_000))
    }
    /// Images on the shared clipboard, besides text (on by default,
    /// Experimental). A device still needs the clipboard's permission bits.
    pub fn clipboard_image(&self) -> bool {
        self.boolean("clipboard_image", true)
    }
    /// The largest clipboard image in bytes (`clipboard_image_max_mb`, MiB,
    /// 1 to 64, 16 by default).
    pub fn clipboard_image_max_bytes(&self) -> u64 {
        let mut mib = self.integer("clipboard_image_max_mb", 16);
        if !(1..=64).contains(&mib) {
            invalid(
                "clipboard_image_max_mb",
                self.get("clipboard_image_max_mb", ""),
            );
            mib = 16;
        }
        mib as u64 * 1024 * 1024
    }
    /// `shared_folders` (Experimental): a JSON list of id, name and path. A
    /// list that isn't that, repeats or misspells an id, or names a path that
    /// isn't a folder on a drive of this PC is reported, and none is shared.
    pub fn shared_folders(&self) -> Vec<SharedFolderConfig> {
        let value = self.get("shared_folders", "[]");
        match parse_shared_folders(value) {
            Ok(folders) => folders,
            Err(error) => {
                refused("shared_folders", value, &error);
                Vec::new()
            }
        }
    }
    /// Whether the WebRTC media thread paces video (the default). `webrtc_pacing = false` sends
    /// every packet the moment str0m makes it; it exists to measure the two against each other.
    pub fn webrtc_pacing(&self) -> bool {
        self.boolean("webrtc_pacing", true)
    }
    /// Whether the WebRTC offer carries NACK and RTX, so a lost video packet is resent
    /// (`webrtc_rtx = false` turns it off). On by default: at 0.1 % datagram loss it kept 60 fps
    /// with no freeze where keyframe recovery alone dropped to 36 to 43 fps with 3 to 18 freezes,
    /// and on a clean link it changes nothing (rust/PERFORMANCE.md).
    pub fn webrtc_rtx(&self) -> bool {
        self.boolean("webrtc_rtx", true)
    }
    /// Whether the AMF encoder skips an output query while its oldest frame cannot be finished
    /// yet, judged by the fastest encode it has returned, and waits instead where a fresh capture
    /// can still be claimed (`amd_output_prediction`). Off by default: the November 8 A/B under
    /// GPU load measured no consistent gain over the blocking 1 ms query (rust/PERFORMANCE.md).
    /// It applies only with `amd_output_thread` off.
    pub fn amd_output_prediction(&self) -> bool {
        self.boolean("amd_output_prediction", false)
    }
    /// Whether a thread of its own waits in AMF's output query and hands finished frames to the
    /// stream thread, which then never blocks in it (`amd_output_thread`). On by default since
    /// the November 20 A/B (AV1 and HEVC under GPU load, rust/PERFORMANCE.md): without it 0.7
    /// to 7 % of captured pictures waited up to 2.2 ms behind a blocking query before encoding,
    /// and the encode tail was longer. It costs about 1 to 1.5 % of one core; end to end did not
    /// improve (its p99 ran 0.5 to 2.4 ms later, within run-to-run spread). It takes precedence
    /// over `amd_output_prediction`.
    pub fn amd_output_thread(&self) -> bool {
        self.boolean("amd_output_thread", true)
    }
    /// Whether the stream thread also wakes on NVENC's D3D12 output fence instead of only on its
    /// 100 µs output poll, which stays as the bound (`nvenc_output_event = false` turns it off).
    pub fn nvenc_output_event(&self) -> bool {
        self.boolean("nvenc_output_event", true)
    }
    /// Whether PyroWave converts the next picture while the GPU still encodes the last one: two
    /// frames in flight, each with its own colour planes and fence (`pyrowave_encode_overlap`).
    /// Off, the default, encodes one frame at a time and waits for it: at 4K120 on a Radeon
    /// 780M client the overlap measured no gain in end-to-end latency or host processing.
    pub fn pyrowave_encode_overlap(&self) -> bool {
        self.boolean("pyrowave_encode_overlap", false)
    }
    /// Video datagrams the WebRTC media thread drops per thousand (`webrtc_loss_permille`, 0 to
    /// 1000), as a lossy link does. A measurement switch, 0 by default.
    pub fn webrtc_loss_permille(&self) -> u32 {
        self.integer("webrtc_loss_permille", 0).clamp(0, 1000) as u32
    }
    /// The WebRTC signalling port (TCP; discovery always listens on UDP 8000).
    /// A value outside 1024..=65535 is reported and the default is used.
    pub fn webrtc_port(&self) -> u16 {
        self.webrtc_port_setting("webrtc_port", WEBRTC_PORT)
    }
    /// The WebRTC media port (UDP).
    pub fn webrtc_media_port(&self) -> u16 {
        self.webrtc_port_setting("webrtc_media_port", WEBRTC_MEDIA_PORT)
    }
    /// The DSU (cemuhook) motion server's UDP port when `enable_dsu_server`
    /// is on (off by default). `dsu_server_port` defaults to 26760, Foundation
    /// Sunshine's; a value outside 1024..=65535 is reported and that is used.
    pub fn dsu_server_port(&self) -> Option<u16> {
        self.boolean("enable_dsu_server", false)
            .then(|| self.webrtc_port_setting("dsu_server_port", DSU_DEFAULT_PORT))
    }
    /// Whether the DSU motion server listens where other devices on the
    /// network can reach it (`dsu_server_lan`, off by default): off keeps it
    /// on this PC only, where an emulator on the same PC finds it.
    pub fn dsu_server_lan(&self) -> bool {
        self.boolean("dsu_server_lan", false)
    }
    fn webrtc_port_setting(&self, key: &str, default: u16) -> u16 {
        let n = self.integer(key, i64::from(default));
        if !WEBRTC_PORTS.contains(&n) {
            invalid(key, self.get(key, ""));
            return default;
        }
        n as u16
    }
    /// Before 2.0.0 `amd_ltr_frames` defaulted to 0, and a console save
    /// could write that default into sunshine.conf, which kept AV1's
    /// recovery without a keyframe off after the upgrade. Until `upgraded`,
    /// a saved 0 is taken for that old default and dropped, so the current
    /// one applies; a 0 saved after the upgrade stays. Returns whether the
    /// saved value was dropped.
    pub fn upgrade_ltr_default(&mut self, upgraded: bool) -> bool {
        let old_default = !upgraded
            && self
                .values
                .get("amd_ltr_frames")
                .is_some_and(|value| parse_integer(value) == Some(0));
        if old_default {
            self.values.remove("amd_ltr_frames");
        }
        old_default
    }
    pub fn text(&self) -> String {
        self.values
            .iter()
            .map(|(k, v)| format!("{k} = {v}\n"))
            .collect()
    }
    pub fn json(&self) -> serde_json::Value {
        serde_json::Value::Object(
            self.values
                .iter()
                .map(|(k, v)| {
                    let parsed = serde_json::from_str(v)
                        .unwrap_or_else(|_| serde_json::Value::String(v.clone()));
                    (k.clone(), parsed)
                })
                .collect(),
        )
    }
    /// Fold the retired switches of a layer of settings (an app's or a
    /// device's overrides, or an earlier console's save) over what this
    /// configuration holds, which is the layers below it: a switch the layer
    /// does not hold keeps what the layers below gave, as the earlier
    /// switches combined. Adaptive bitrate from link-rate adaptation and
    /// congestion control, Packet pacing from the pacing rate and the two
    /// adaptive pacing switches, Remove remote monitor from the two remote
    /// monitor switches ([`migrate_overrides`] leaves a layer that cannot be
    /// folded alone as saved for this).
    pub fn fold_layer(&self, layer: &mut serde_json::Map<String, serde_json::Value>) {
        fold_bitrate_adaptation(layer, self.get(BITRATE_ADAPTATION, "on"));
        fold_packet_pacing(layer, self.get(PACKET_PACING, "auto"));
        fold_remove_remote_monitor(layer, self.get(REMOVE_REMOTE_MONITOR, "never"));
    }
    pub fn update_with(
        &mut self,
        object: &serde_json::Map<String, serde_json::Value>,
        extra_validate: impl FnOnce(&Config, &dyn Fn(&str) -> bool) -> Result<()>,
    ) -> Result<()> {
        // A setting sent under an earlier name is saved under its current
        // one, unless that is sent too.
        let mut object = object.clone();
        for &(old, new) in RENAMED_KEYS {
            if let Some(value) = object.remove(old)
                && !object.contains_key(new)
            {
                object.insert(new.to_owned(), value);
            }
        }
        // The retired HDR request folds into Display HDR as it is sent with
        // it, or else over the Display HDR held: the configuration's own, or
        // for an app's or device's overrides the layers below them. An empty
        // or null request is Follow the device, as its reset was.
        if let Some(request) = object.remove(HDR_REQUEST) {
            let request = match request.as_str().map(|text| unquote(text.trim())) {
                Some(text) if !text.is_empty() => text.to_owned(),
                _ => "auto".to_owned(),
            };
            let folded = match object.get(HDR_OPTION) {
                Some(option) => folded_in_layer(option.as_str().unwrap_or_default(), &request),
                None => {
                    let held = self.get(HDR_OPTION, "auto");
                    folded_hdr(Some(held), &request).or_else(|| {
                        // Follow the device ends an earlier Always on or off.
                        (request == "auto"
                            && matches!(unquote(held.trim()), "force_on" | "force_off"))
                        .then_some("auto")
                    })
                }
            };
            if let Some(folded) = folded {
                object.insert(HDR_OPTION.to_owned(), folded.into());
            }
        }
        // The bitrate, pacing and remote monitor switches, as an earlier
        // console or an API caller sends them, fold into Adaptive bitrate,
        // Packet pacing and Remove remote monitor unless those are sent too.
        self.fold_layer(&mut object);
        // Likewise the two disconnect settings fold into the disconnect behavior.
        fold_disconnect_behavior(&mut object);
        fold_min_qp(&mut object, |key| self.values.contains_key(key), true);
        fold_updates(&mut object, self.get(UPDATES, "tell_me"));
        object.retain(|key, _| !removed(key));
        let object = &object;
        let mut next = self.clone();
        for (key, value) in object {
            if !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') || key.is_empty() {
                bail!("invalid configuration key");
            }
            // Legacy PATCH and POST both reset a setting when its value is
            // null/empty. Keeping the literal "null" silently defeats defaults.
            if value.is_null() || value.as_str() == Some("") {
                next.values.remove(key);
                continue;
            }
            let value = match value {
                serde_json::Value::String(v) => v.clone(),
                _ => value.to_string(),
            };
            if value.contains('\0') {
                bail!("configuration cannot contain NUL");
            }
            if value.contains(['\r', '\n'])
                && !(value.trim().starts_with('[') || value.trim().starts_with('{'))
            {
                bail!("configuration values cannot inject new keys");
            }
            // A client or an old console sending a retired value saves its
            // replacement, as the file would be read.
            let value = replacement(key, &value).map_or(value, str::to_owned);
            next.values.insert(key.clone(), value);
        }
        // A PyroWave share equal to Error correction is that share.
        if let Some(pyrowave) = next.values.get(retired::PYROWAVE_FEC)
            && retired::pyrowave_fec_is_default(
                Some(pyrowave),
                next.values.get("fec_percentage").map(String::as_str),
            )
        {
            next.values.remove(retired::PYROWAVE_FEC);
        }
        // Changing a shared Library sync setting re-decides it for every
        // launcher: the per-launcher keys of earlier releases go. A save that
        // leaves the shared value as held (or unsent) keeps a launcher's
        // override, so saving another setting does not change a launcher that
        // differed.
        for &(option, default, prefixes) in LIBRARY_SYNC_OPTIONS {
            let shared = format!("library_{option}");
            let Some(sent) = object.get(&shared) else {
                continue;
            };
            let sent = match sent {
                serde_json::Value::String(text) => text.clone(),
                serde_json::Value::Null => String::new(),
                other => other.to_string(),
            };
            let held = self
                .values
                .get(&shared)
                .map(String::as_str)
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(default);
            if held == sent {
                continue;
            }
            for prefix in prefixes {
                next.values.remove(&format!("{prefix}_{option}"));
            }
        }
        // Only what changes is checked: a value the file already had (the
        // host reads it leniently) must not block every other save.
        let changed = |key: &str| object.contains_key(key) && next.values.contains_key(key);
        if changed("port") && !PORTS.contains(&next.integer("port", 0)) {
            bail!("port must be between 1029 and 65514");
        }
        for key in ["webrtc_port", "webrtc_media_port", "dsu_server_port"] {
            if changed(key) && !WEBRTC_PORTS.contains(&next.integer(key, 0)) {
                bail!("{key} must be between 1024 and 65535");
            }
        }
        if changed(MAX_STREAM_RESOLUTION) {
            parse_max_stream_resolution(&next.values[MAX_STREAM_RESOLUTION])?;
        }
        if changed("hardware_metrics_interval")
            && next.values.contains_key("hardware_metrics_interval")
            && !(500..=10000).contains(&next.integer("hardware_metrics_interval", 0))
        {
            bail!("hardware metrics interval must be between 500 and 10000 milliseconds");
        }
        for key in [
            "audio_night_mode",
            "audio_headphone_surround",
            "sleep_while_suspended",
        ] {
            if changed(key) && parse_boolean(&next.values[key]).is_none() {
                bail!("{key} must be true or false");
            }
        }
        if changed("sleep_after_idle_minutes")
            && !parse_integer(&next.values["sleep_after_idle_minutes"])
                .is_some_and(|minutes| minutes == 0 || (5..=1440).contains(&minutes))
        {
            bail!("sleep_after_idle_minutes must be 0 (off) or between 5 and 1440");
        }
        if changed("hardware_metrics_selection") {
            for metric in next.list("hardware_metrics_selection") {
                if !HARDWARE_METRICS.contains(&metric.as_str()) {
                    bail!("unknown hardware metric: {metric}");
                }
            }
        }
        extra_validate(&next, &changed)?;
        *self = next;
        Ok(())
    }
    /// Before `keys` are written to sunshine.conf: values the file would not
    /// give back, and settings that would keep the host from starting.
    pub fn check_saved_with<'a>(
        &self,
        keys: impl IntoIterator<Item = &'a String>,
        mut extra_check: impl FnMut(&str, &str) -> Result<()>,
    ) -> Result<()> {
        for key in keys {
            let key = current_name(key);
            let Some(value) = self.values.get(key) else {
                continue;
            };
            // '#' starts a comment and a value opening [ or { runs until it
            // closes, so such a value would be cut short, or swallow the
            // settings after it, on the next start.
            let expected = value.lines().map(str::trim).collect::<Vec<_>>().join("\n");
            let stored = Self::parse(&format!("{key} = {value}\n"))?;
            if stored.values.get(key).map(String::as_str) != Some(expected.trim()) {
                bail!(
                    "{key} cannot be saved: in the configuration file # starts a comment and a value opening [ or {{ has to close it"
                );
            }
            let value = unquote(value.trim());
            if key == "bind_address" && value.parse::<std::net::IpAddr>().is_err() {
                bail!("bind_address must be an IP address of this PC, or blank for all of them");
            }
            if key == "log_path"
                && (value.ends_with(['\\', '/'])
                    || (Path::new(value).is_absolute() && Path::new(value).is_dir()))
            {
                bail!("log_path must name a file, not a folder");
            }
            extra_check(key, value)?;
        }
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn update(
        &mut self,
        object: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<()> {
        self.update_with(object, |_, _| Ok(()))
    }
    #[cfg(test)]
    pub(crate) fn check_saved<'a>(&self, keys: impl IntoIterator<Item = &'a String>) -> Result<()> {
        self.check_saved_with(keys, |_, _| Ok(()))
    }
    pub fn path(&self, key: &str, directory: &Path, default: &str) -> PathBuf {
        let p = PathBuf::from(self.get(key, default));
        if p.is_absolute() {
            p
        } else {
            directory.join(p)
        }
    }
}
/// Base ports whose derived ports (base - 5 to base + 21) are all valid.
const PORTS: std::ops::RangeInclusive<i64> = 1029..=65514;
/// The WebRTC ports the TV expects by default, and the range a saved port may take.
pub const WEBRTC_PORT: u16 = 8000;
pub const WEBRTC_MEDIA_PORT: u16 = 8001;
const WEBRTC_PORTS: std::ops::RangeInclusive<i64> = 1024..=65535;
/// `keyboard_layout`: the layout the host types a device's keys with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyboardLayoutSetting {
    /// The layout the device reports (0x553E).
    Client,
    /// US English.
    Us,
    /// A Windows keyboard layout id (KLID), written as 8 hex digits.
    Klid(u32),
}

/// A KLID as Windows writes it: exactly 8 hex digits, such as `00000407`.
pub fn parse_klid(text: &str) -> Option<u32> {
    (text.len() == 8 && text.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| u32::from_str_radix(text, 16).ok())
        .flatten()
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Ports {
    pub http: u16,
    pub https: u16,
    pub web: u16,
    /// The guest page and its socket (docs/browser-guests.md), the one port guests on the
    /// internet need forwarded besides their media; nothing of the console's answers on it.
    pub guest: u16,
    pub video: u16,
    pub control: u16,
    pub audio: u16,
    pub mic: u16,
    pub rtsp: u16,
}
impl Ports {
    pub fn from_base(n: u16) -> Self {
        Self {
            http: n,
            https: n - 5,
            web: n + 1,
            guest: n + 2,
            video: n + 9,
            control: n + 10,
            audio: n + 11,
            mic: n + MIC_PORT_OFFSET,
            rtsp: n + 21,
        }
    }
}
#[cfg(test)]
mod tests;
