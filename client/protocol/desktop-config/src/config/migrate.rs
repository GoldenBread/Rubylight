//! Desktop settings migrations, range deserializers, and cross-layer value normalization.

use serde::Deserialize;

use super::{DEADZONE, DISPLAY_SCALES, MIC_GAIN, MIC_GAIN_STEP, RENDER_SCALE, SHARP_COLOR_KEYS, Settings};

pub(super) fn render_scale<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    Ok(u32::deserialize(deserializer)?.clamp(*RENDER_SCALE.start(), *RENDER_SCALE.end()))
}

pub(super) fn display_scale<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    let percent = u32::deserialize(deserializer)?;
    Ok(if DISPLAY_SCALES.contains(&percent) { percent } else { 0 })
}

impl Settings {
    /// What every load does to settings saved by an earlier version. Idempotent: it needs no flags
    /// but `auto_reconnect_default_on`'s and `real_vrr_default_on`'s.
    pub(crate) fn migrate(&mut self) {
        // Turns Recover interrupted streams on once in settings saved before it became the default.
        if !self.auto_reconnect_default_on {
            (self.auto_reconnect, self.auto_reconnect_default_on) = (true, true);
        }
        // The same for Real VRR: its off was the old default, not a choice.
        if !self.real_vrr_default_on {
            (self.present_at_host_times, self.real_vrr_default_on) = (true, true);
        }
        // The old default 100 % becomes Auto once; after that, a chosen 100 % stays.
        if !self.ui_zoom_auto_migrated {
            if self.ui_zoom == 100 {
                self.ui_zoom = 0;
            }
            self.ui_zoom_auto_migrated = true;
        }
        // Lower bitrate's three switches become its one value: a player who had it off keeps it off.
        let switches = (
            self.retired_busy_bitrate.take(),
            self.retired_lost_bitrate.take(),
            self.retired_keep_bitrate.take(),
        );
        if let Some(choice) = folded_lower_bitrate(switches) {
            self.lower_bitrate = choice;
        }
        // One switch writes both of these now; where an earlier version saved them apart and only one was
        // off, the switch shows off, so both are off.
        let on = self.tell_host_wireless && self.tell_host_link_speed;
        (self.tell_host_wireless, self.tell_host_link_speed) = (on, on);
        normalize_values(&mut SettingsValues(self));
    }
}

/// A layer that saved only one of the two network hints off turns both off: the one switch for both
/// writes them together, so it shows off.
pub(super) fn fold_tell_host(values: &mut serde_json::Map<String, serde_json::Value>) {
    let off = |key: &str| values.get(key).and_then(|value| value.as_bool()) == Some(false);
    if off("tell_host_wireless") || off("tell_host_link_speed") {
        values.insert("tell_host_wireless".to_owned(), false.into());
        values.insert("tell_host_link_speed".to_owned(), false.into());
    }
}

/// The Lower bitrate the three switches of earlier versions chose, a missing one read as off; `None`
/// when none was saved.
fn folded_lower_bitrate(
    switches: (Option<bool>, Option<bool>, Option<bool>),
) -> Option<nightjar_policy::choices::LowerBitrate> {
    let (busy, lost, keep) = switches;
    (busy.is_some() || lost.is_some() || keep.is_some()).then(|| {
        nightjar_policy::choices::LowerBitrate::from_saved(
            busy.unwrap_or(false),
            lost.unwrap_or(false),
            keep.unwrap_or(false),
        )
    })
}

/// A PC's, game's or profile's layer (an import too) saved with Lower bitrate's three switches holds
/// that choice as its own `lower_bitrate`; a value saved under the new key wins.
pub(super) fn fold_lower_bitrate(values: &mut serde_json::Map<String, serde_json::Value>) {
    let mut take = |key: &str| values.remove(key).and_then(|value| value.as_bool());
    let switches = (take("adaptive_bitrate"), take("client_hint_bitrate"), take("keep_bitrate"));
    if let Some(choice) = folded_lower_bitrate(switches) {
        values.entry("lower_bitrate").or_insert_with(|| serde_json::Value::from(choice.as_str()));
    }
}

/// A settings layer's values, main or not, for [`normalize_values`].
pub(super) trait Values {
    fn bool(&self, key: &str) -> Option<bool>;
    fn u32(&self, key: &str) -> Option<u32>;
    fn set_bool(&mut self, key: &str, value: bool);
    fn set_u32(&mut self, key: &str, value: u32);
}

/// The main settings, which hold every key.
struct SettingsValues<'a>(&'a mut Settings);

impl Values for SettingsValues<'_> {
    fn bool(&self, key: &str) -> Option<bool> {
        match key {
            "prefer_yuv444" => Some(self.0.prefer_yuv444),
            "experimental_packed_444" => Some(self.0.experimental_packed_444),
            _ => None,
        }
    }
    fn u32(&self, key: &str) -> Option<u32> {
        match key {
            "network_grace_ms" => Some(self.0.network_grace_ms),
            "quality_min_short_edge" => Some(self.0.quality_min_short_edge),
            "quality_min_fps" => Some(self.0.quality_min_fps),
            "mic_gain" => Some(self.0.mic_gain),
            "deadzone" => Some(self.0.deadzone),
            "trigger_deadzone" => Some(self.0.trigger_deadzone),
            _ => None,
        }
    }
    fn set_bool(&mut self, key: &str, value: bool) {
        match key {
            "prefer_yuv444" => self.0.prefer_yuv444 = value,
            "experimental_packed_444" => self.0.experimental_packed_444 = value,
            _ => {}
        }
    }
    fn set_u32(&mut self, key: &str, value: u32) {
        let s = &mut *self.0;
        match key {
            "network_grace_ms" => s.network_grace_ms = value,
            "quality_min_short_edge" => s.quality_min_short_edge = value,
            "quality_min_fps" => s.quality_min_fps = value,
            "mic_gain" => s.mic_gain = value,
            "deadzone" => s.deadzone = value,
            "trigger_deadzone" => s.trigger_deadzone = value,
            _ => {}
        }
    }
}

/// A PC's, game's or profile's own values, which hold only the keys they set.
impl Values for serde_json::Map<String, serde_json::Value> {
    fn bool(&self, key: &str) -> Option<bool> {
        self.get(key).and_then(serde_json::Value::as_bool)
    }
    fn u32(&self, key: &str) -> Option<u32> {
        self.get(key)
            .and_then(serde_json::Value::as_u64)
            .and_then(|v| u32::try_from(v).ok())
    }
    fn set_bool(&mut self, key: &str, value: bool) {
        self.insert(key.to_owned(), value.into());
    }
    fn set_u32(&mut self, key: &str, value: u32) {
        self.insert(key.to_owned(), value.into());
    }
}

/// Brings a layer's saved values onto what this version offers. Every load of every layer runs it
/// (the main settings, profiles, PCs and games), and it changes nothing a second time:
/// - "Sharper colored text" is one choice in two keys: a layer that holds either gets both, set to
///   whether either was on. A layer that holds neither keeps inheriting them.
/// - "Wait through network dropouts", "Minimum resolution" and "Minimum frame rate" read as the
///   nearest of the choices both clients offer (`nightjar_policy::recovery`, `::quality`).
/// - Microphone gain and the dead zones stay in their ranges.
pub(super) fn normalize_values(values: &mut impl Values) {
    use nightjar_policy::quality::{MIN_FPS_CHOICES, MIN_SHORT_EDGE_CHOICES, nearest_floor};
    let held: Vec<bool> = SHARP_COLOR_KEYS.iter().filter_map(|key| values.bool(key)).collect();
    if !held.is_empty() {
        let on = held.contains(&true);
        for key in SHARP_COLOR_KEYS {
            values.set_bool(key, on);
        }
    }
    let fixes: [(&str, &dyn Fn(u32) -> u32); 6] = [
        ("network_grace_ms", &nightjar_policy::recovery::network_grace_ms),
        ("quality_min_short_edge", &|v: u32| nearest_floor(&MIN_SHORT_EDGE_CHOICES, v)),
        ("quality_min_fps", &|v: u32| nearest_floor(&MIN_FPS_CHOICES, v)),
        ("mic_gain", &|v: u32| v.clamp(*MIC_GAIN.start(), *MIC_GAIN.end()) / MIC_GAIN_STEP * MIC_GAIN_STEP),
        ("deadzone", &|v: u32| v.min(*DEADZONE.end())),
        ("trigger_deadzone", &|v: u32| v.min(u32::from(crate::gamepad_types::MAX_TRIGGER_DEADZONE))),
    ];
    for (key, fix) in fixes {
        if let Some(value) = values.u32(key) {
            let fixed = fix(value);
            if fixed != value {
                values.set_u32(key, fixed);
            }
        }
    }
}
