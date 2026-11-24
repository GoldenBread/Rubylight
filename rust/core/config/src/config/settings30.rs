//! The settings of the 3.0 extensions, defined ahead of their features: the
//! console lists them in `web/src/lib/schema/extensions.gen.ts`, hidden until
//! each workstream ships its feature. A value the host can't use is reported
//! through `invalid` and the default applies, so a launch never fails because
//! of one of these settings.
pub use super::catalog_gen::SETTINGS_30;

/// `touch_mode`: how touches reach the PC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchMode {
    /// As the device sends them; `native_pen_touch` still applies.
    Client,
    Mouse,
    Trackpad,
}

/// `dd_rotation_option`, beside `dd_resolution_option`. Windows scaling has no
/// option of its own: `dd_virtual_display_scale` sets it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayAdjust {
    /// Leave the display as it is.
    Disabled,
    /// Match the device.
    Auto,
    /// Use the `dd_manual_*` value.
    Manual,
}

#[cfg(test)]
mod tests {
    use super::super::Config;
    use super::*;
    use std::path::PathBuf;

    /// The keys a device or an app may override.
    const PER_DEVICE: &[&str] = &[
        "audio_surround_714",
        "touch_mode",
        "touch_sensitivity",
        "touch_gestures",
        "client_cursor",
        "dd_rotation_option",
        "dd_manual_rotation",
    ];

    fn config(text: &str) -> Config {
        Config::parse(text).unwrap()
    }

    /// Every accessor's answer, so two configurations can be compared whole.
    fn answers(c: &Config) -> String {
        format!(
            "{:?}",
            (
                (
                    c.hdr_dynamic_metadata(),
                    c.hdr_dolby_vision(),
                    c.hdr_hlg(),
                    c.audio_surround_714(),
                    c.pen_barrel_roll(),
                ),
                (
                    c.touch_mode(),
                    c.touch_sensitivity(),
                    c.touch_gestures(),
                    c.client_cursor(),
                    c.clipboard_image(),
                    c.clipboard_image_max_bytes(),
                    c.shared_folders(),
                ),
                (
                    c.qr_pairing(),
                    c.dd_rotation_option(),
                    c.dd_manual_rotation(),
                ),
                (c.nucleus_path(), c.nucleus_coop_players()),
            )
        )
    }

    #[test]
    fn there_are_12_unique_settings() {
        let keys: std::collections::BTreeSet<_> = SETTINGS_30.iter().map(|(k, _)| k).collect();
        assert_eq!((SETTINGS_30.len(), keys.len()), (12, 12));
    }

    #[test]
    fn the_defaults_are_the_documented_values() {
        let c = Config::default();
        assert!(c.hdr_dynamic_metadata() && c.hdr_dolby_vision() && c.hdr_hlg());
        assert!(c.audio_surround_714());
        assert!(c.pen_barrel_roll());
        assert_eq!(c.touch_mode(), TouchMode::Client);
        assert_eq!(c.touch_sensitivity(), 100);
        assert!(c.touch_gestures() && c.client_cursor() && c.clipboard_image());
        assert_eq!(c.clipboard_image_max_bytes(), 16 * 1024 * 1024);
        assert!(c.shared_folders().is_empty());
        assert!(c.qr_pairing());
        assert_eq!(c.dd_rotation_option(), DisplayAdjust::Disabled);
        assert_eq!(c.dd_manual_rotation(), 0);
        assert_eq!(c.nucleus_path(), None);
        assert_eq!(c.nucleus_coop_players(), 4);
    }

    #[test]
    fn each_default_written_out_reads_as_the_unset_default() {
        let unset = answers(&Config::default());
        let written: String = SETTINGS_30
            .iter()
            .map(|(key, default)| format!("{key} = {default}\n"))
            .collect();
        assert_eq!(answers(&config(&written)), unset);
        for (key, default) in SETTINGS_30 {
            assert_eq!(
                answers(&config(&format!("{key} = {default}\n"))),
                unset,
                "{key}"
            );
        }
    }

    #[test]
    fn values_the_host_cannot_use_fall_back_to_the_default() {
        let unset = answers(&Config::default());
        for text in [
            "hdr_hlg = maybe",
            "touch_mode = pen",
            "touch_sensitivity = 24",
            "touch_sensitivity = 401",
            "clipboard_image_max_mb = 0",
            "clipboard_image_max_mb = 65",
            "shared_folders = {\"id\": \"games\"}",
            "shared_folders = [{\"id\": \"../games\", \"name\": \"Games\", \"path\": \"D:\\\\Games\"}]",
            "shared_folders = [{\"id\": \"games\", \"name\": \"Games\"}]",
            "shared_folders = [{\"id\": \"games\", \"name\": \"\", \"path\": \"D:\\\\Games\"}]",
            "shared_folders = [{\"id\": \"games\", \"name\": \"Games\", \"path\": \"D:\\\\Games\", \"mode\": \"rw\"}]",
            "dd_rotation_option = sideways",
            "dd_manual_rotation = 45",
            "dd_manual_rotation = 360",
            "nucleus_coop_players = 0",
            "nucleus_coop_players = 5",
        ] {
            assert_eq!(answers(&config(text)), unset, "{text}");
        }
    }

    #[test]
    fn values_in_range_are_read() {
        let c = config(concat!(
            "touch_mode = trackpad\n",
            "touch_sensitivity = 25\n",
            "clipboard_image_max_mb = 64\n",
            "dd_rotation_option = manual\n",
            "dd_manual_rotation = 270\n",
            "nucleus_path = \"D:\\Tools\\Nucleus\"\n",
            "nucleus_coop_players = 2\n",
        ));
        assert_eq!(c.touch_mode(), TouchMode::Trackpad);
        assert_eq!(c.touch_sensitivity(), 25);
        assert_eq!(c.clipboard_image_max_bytes(), 64 * 1024 * 1024);
        assert_eq!(c.dd_rotation_option(), DisplayAdjust::Manual);
        assert_eq!(c.dd_manual_rotation(), 270);
        assert_eq!(c.nucleus_path(), Some(PathBuf::from("D:\\Tools\\Nucleus")));
        assert_eq!(c.nucleus_coop_players(), 2);
        let folders = config(
            "shared_folders = [{\"id\": \"games\", \"name\": \"Games\", \"path\": \"D:\\\\Games\"}, {\"id\": \"saves-2\", \"name\": \"Saves\", \"path\": \"E:\\\\Saves\"}]",
        )
        .shared_folders();
        assert_eq!(
            folders
                .iter()
                .map(|f| (f.id.as_str(), f.path.to_str().unwrap()))
                .collect::<Vec<_>>(),
            [("games", "D:\\Games"), ("saves-2", "E:\\Saves")]
        );
        let twice = "[{\"id\": \"a\", \"name\": \"A\", \"path\": \"C:\\\\A\"}, {\"id\": \"a\", \"name\": \"B\", \"path\": \"C:\\\\B\"}]";
        assert!(
            config(&format!("shared_folders = {twice}"))
                .shared_folders()
                .is_empty()
        );
    }

    #[test]
    fn per_device_settings_are_exactly_the_documented_ones() {
        for (key, _) in SETTINGS_30 {
            assert_eq!(
                crate::config::override_allowed(key),
                PER_DEVICE.contains(key),
                "{key}"
            );
        }
        assert_eq!(PER_DEVICE.len(), 7);
    }

    /// The settings whose features have shipped, so the console shows them.
    const LIVE: &[&str] = &[
        "audio_surround_714",
        "dd_rotation_option",
        "dd_manual_rotation",
        // Input: pen barrel roll, the device-drawn cursor and touch profiles.
        "pen_barrel_roll",
        "client_cursor",
        "touch_mode",
        "touch_sensitivity",
        "touch_gestures",
        // Nucleus Co-op.
        "nucleus_path",
        "nucleus_coop_players",
    ];

    /// Each setting's key and default in extensions.gen.ts, with LF or CRLF line
    /// ends (CI checks the repository out with CRLF). A key is the one that
    /// starts its line: a select's `fixed: { ..., key: '...' }` names another
    /// setting, which has its own entry.
    fn schema_settings(schema: &str) -> Vec<(&str, &str)> {
        let mut listed = Vec::new();
        for entry in schema.split("\n    key: '").skip(1) {
            let key = &entry[..entry.find('\'').unwrap()];
            let default = entry
                .split("\n    default: ")
                .nth(1)
                .unwrap_or_else(|| panic!("{key} has no default"));
            let default = default
                .lines()
                .next()
                .and_then(|line| line.strip_suffix(','))
                .unwrap_or_else(|| panic!("{key}'s default does not end its line with a comma"))
                .trim_matches('\'');
            assert_eq!(
                entry.contains("\n    pending: true,"),
                !LIVE.contains(&key),
                "{key} is pending only until its feature ships"
            );
            listed.push((key, default));
        }
        listed.sort();
        listed
    }

    #[test]
    fn the_console_schema_lists_exactly_these_settings_with_these_defaults() {
        let schema = include_str!("../../../../web/src/lib/schema/extensions.gen.ts");
        let mut documented = SETTINGS_30.to_vec();
        documented.sort();
        assert_eq!(schema_settings(schema), documented);
        let crlf = schema.replace("\r\n", "\n").replace('\n', "\r\n");
        assert_eq!(schema_settings(&crlf), documented);
    }
}
