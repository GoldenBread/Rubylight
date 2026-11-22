//! Cross-domain validation hooks for `Config::update` and `Config::check_saved`.

use anyhow::{Result, bail};
use butterpollo_config::config::Config;
use butterpollo_input_policy::{audio_haptics, couch_menu, dualsense, input_policy};
use butterpollo_media_policy::{framegen, stream_preset};

/// Full cross-domain `Config` validation and persistence checks.
pub trait ConfigExt {
    fn update(&mut self, object: &serde_json::Map<String, serde_json::Value>) -> Result<()>;
    fn check_saved<'a>(&self, keys: impl IntoIterator<Item = &'a String>) -> Result<()>;
}

impl ConfigExt for Config {
    fn update(&mut self, object: &serde_json::Map<String, serde_json::Value>) -> Result<()> {
        self.update_with(object, |next, changed| {
            if changed("frame_limiter_fps_limit") {
                framegen::Rate::parse(next.get("frame_limiter_fps_limit", "0"))?;
            }
            if changed(stream_preset::SETTING)
                && !stream_preset::valid(next.get(stream_preset::SETTING, ""))
            {
                bail!("stream_preset must be lowest_latency, balanced or reference");
            }
            if changed(crate::nucleus::mode::KEY) {
                crate::nucleus::mode::parse(&next.values[crate::nucleus::mode::KEY])
                    .map_err(anyhow::Error::msg)?;
            }
            if changed(dualsense::HAPTICS_GAIN) {
                dualsense::check_haptics_gain(&next.values[dualsense::HAPTICS_GAIN])?;
            }
            for key in audio_haptics::SETTINGS {
                if changed(key) {
                    audio_haptics::check(key, &next.values[key])?;
                }
            }
            for key in couch_menu::chord::SETTINGS {
                if changed(key)
                    && let Some(warning) = couch_menu::chord::check(key, &next.values[key])
                        .map_err(anyhow::Error::msg)?
                {
                    tracing::warn!("{warning}");
                }
            }
            // Warn, not refuse: either setting alone is fine. The couch menu
            // values were checked above, so the read refuses none.
            if couch_menu::chord::SETTINGS
                .into_iter()
                .chain(["back_button_timeout"])
                .any(changed)
                && let Some(warning) = couch_menu::chord::OpenWith::read(next, &mut |_, _, _| {})
                    .select_conflict(input_policy::back_button_timeout(next))
            {
                tracing::warn!("{warning}");
            }
            Ok(())
        })
    }

    fn check_saved<'a>(&self, keys: impl IntoIterator<Item = &'a String>) -> Result<()> {
        self.check_saved_with(keys, |key, value| {
            if key == crate::steamgriddb::KEY_SETTING {
                crate::steamgriddb::check_key(value).map_err(|message| anyhow::anyhow!(message))?;
            }
            Ok(())
        })
    }
}
