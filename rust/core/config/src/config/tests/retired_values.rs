//! Tests for retired values, removed keys, renamed keys, library sync folding, and app migration.
use super::*;

#[test]
fn the_fec_cap_is_fixed_and_its_retired_setting_is_dropped() {
    assert_eq!(super::FEC_MAX_PERCENTAGE, 50);
    for key in ["fec_max_percentage", "adaptive_fec"] {
        assert!(super::removed(key), "{key}");
        assert!(!super::override_allowed(key), "{key}");
        let mut layer = serde_json::json!({key: 40, "fec_percentage": 30})
            .as_object()
            .unwrap()
            .clone();
        assert!(super::migrate_overrides(&mut layer));
        assert!(!layer.contains_key(key), "{key}");
        assert_eq!(layer["fec_percentage"], 30);
    }
}

/// PyroWave follows Error correction like every codec; only a PyroWave value
/// that differs from it is kept, and then keeps being read.
#[test]
fn pyrowave_error_correction_follows_the_one_percentage() {
    for (text, expected, kept) in [
        ("", 20, false),
        ("fec_percentage=30", 30, false),
        ("pyrowave_critical_fec_percentage=20", 20, false),
        (
            "fec_percentage=30
pyrowave_critical_fec_percentage=30",
            30,
            false,
        ),
        (
            "fec_percentage=30
pyrowave_critical_fec_percentage=20",
            20,
            true,
        ),
        ("pyrowave_critical_fec_percentage=0", 0, true),
        ("pyrowave_critical_fec_percentage=40", 40, true),
        ("pyrowave_critical_fec_percentage=0x14", 20, false),
        ("pyrowave_critical_fec_percentage=lots", 20, false),
    ] {
        let config = super::Config::parse(text).unwrap();
        assert_eq!(config.pyrowave_fec_percentage(), expected, "{text}");
        assert_eq!(
            config.text().contains("pyrowave_critical_fec_percentage"),
            kept,
            "{text}"
        );
    }
    // A save that makes the two equal drops the PyroWave value.
    let mut config = super::Config::parse("pyrowave_critical_fec_percentage=40").unwrap();
    let sent = serde_json::json!({"fec_percentage": 40});
    config.update(sent.as_object().unwrap()).unwrap();
    assert!(!config.text().contains("pyrowave_critical_fec_percentage"));
    assert_eq!(config.pyrowave_fec_percentage(), 40);
    // An app's or device's overrides.
    for (layer, kept) in [
        (
            serde_json::json!({"fec_percentage": 30, "pyrowave_critical_fec_percentage": 30}),
            false,
        ),
        (
            serde_json::json!({"fec_percentage": 30, "pyrowave_critical_fec_percentage": 10}),
            true,
        ),
        (
            serde_json::json!({"pyrowave_critical_fec_percentage": 10}),
            true,
        ),
    ] {
        let mut layer = layer.as_object().unwrap().clone();
        super::migrate_overrides(&mut layer);
        assert_eq!(
            layer.contains_key("pyrowave_critical_fec_percentage"),
            kept,
            "{layer:?}"
        );
    }
    assert!(super::override_allowed("pyrowave_critical_fec_percentage"));
}

#[test]
fn rtx_hdr_settings_are_kept_and_its_peak_brightness_is_renamed() {
    // Profiles, apps and devices saved by Vibepollo and earlier Rubylight
    // releases: the NVIDIA image enhancements read them again.
    let mut config = super::Config::parse(
        "rtx_hdr = true
rtx_hdr_contrast = 14
rtx_hdr_sdr_brightness = 20
rtx_hdr_peak_brightness = 1500
port = 47989
",
    )
    .unwrap();
    for key in ["rtx_hdr", "rtx_hdr_contrast", "rtx_hdr_sdr_brightness"] {
        assert!(!super::removed(key), "{key}");
        assert!(config.values.contains_key(key), "{key}");
    }
    assert_eq!(config.integer("hdr_peak_brightness", 0), 1500);
    assert_eq!(config.integer("port", 0), 47989);
    // A console or API save sending them.
    let saved = serde_json::json!({"rtx_hdr_saturation": 5, "rtx_hdr_peak_brightness": 900});
    config.update(saved.as_object().unwrap()).unwrap();
    assert_eq!(config.integer("rtx_hdr_saturation", 0), 5);
    assert_eq!(config.integer("hdr_peak_brightness", 0), 900);
    // Devices and apps may choose them, and the peak under its new name.
    assert!(super::override_allowed("hdr_peak_brightness"));
    assert!(super::override_allowed("rtx_hdr"));
    assert!(super::override_allowed("rtx_video"));
    assert_eq!(
        super::current_name("rtx_hdr_peak_brightness"),
        "hdr_peak_brightness"
    );
}

#[test]
fn a_saved_child_session_mode_becomes_nucleus_with_one_report() {
    for old in [
        "auto",
        "child_session",
        "virtual_display",
        " Child_Session ",
    ] {
        let text = format!("background_mode = {old}\nport = 48123\n");
        let (config, retired) = Config::parse_retiring(&text).unwrap();
        assert_eq!(config.get("background_mode", ""), "nucleus", "{old}");
        assert_eq!(retired.len(), 1, "{old}");
        let warning = retired_warning(&retired).unwrap();
        assert!(warning.contains("is now nucleus"), "{warning}");
        assert!(
            warning.contains("Windows child sessions were removed"),
            "{warning}"
        );
        // The saved text holds nucleus, and reads without a report.
        assert!(config.text().contains("background_mode = nucleus"));
        let (_, again) = Config::parse_retiring(&config.text()).unwrap();
        assert!(again.is_empty(), "{old}");
    }
    for kept in ["nucleus", ""] {
        let (config, retired) =
            Config::parse_retiring(&format!("background_mode = {kept}\n")).unwrap();
        assert!(retired.is_empty(), "{kept}");
        assert_eq!(config.get("background_mode", ""), kept);
    }
    // Apps and devices.
    let mut settings = serde_json::json!({"background_mode": "child_session", "port": "1"});
    assert!(super::replace_retired(
        settings.as_object_mut().unwrap().iter_mut()
    ));
    assert_eq!(settings["background_mode"], "nucleus");
    let mut nucleus = serde_json::json!({"background_mode": "nucleus"});
    assert!(!super::replace_retired(
        nucleus.as_object_mut().unwrap().iter_mut()
    ));
}

#[test]
fn the_child_session_settings_go_without_a_report() {
    let (config, retired) = Config::parse_retiring(
            "child_session_resolution = 2560x1440\nchild_session_scale = 150\nchild_session_port = 48289\nchild_session_host = true\nport = 48123\n",
        )
        .unwrap();
    assert!(retired.is_empty());
    for key in [
        "child_session_resolution",
        "child_session_scale",
        "child_session_port",
        "child_session_host",
    ] {
        assert!(!config.values.contains_key(key), "{key}");
        assert!(super::removed(key), "{key}");
    }
    assert_eq!(config.port().unwrap(), 48123);
}

#[test]
fn the_permanent_virtual_display_settings_go_without_a_report() {
    // Both names the removed permanent virtual displays were saved under.
    let names = [
        "dd_vdd_static_monitor_count",
        "dd_virtual_display_permanent_count",
    ];
    let (mut config, retired) = Config::parse_retiring(
        "dd_vdd_static_monitor_count = \"2\"\ndd_virtual_display_permanent_count = 1\nport = 48123\n",
    )
    .unwrap();
    assert!(retired.is_empty());
    for key in names {
        assert!(super::removed(key), "{key}");
        assert!(!super::override_allowed(key), "{key}");
        assert_eq!(super::current_name(key), key);
        assert!(!config.values.contains_key(key), "{key}");
    }
    // A console or API save still sending them.
    let saved = serde_json::json!({
        "dd_virtual_display_permanent_count": 3,
        "dd_vdd_static_monitor_count": 2
    });
    config.update(saved.as_object().unwrap()).unwrap();
    let text = config.text();
    for key in names {
        assert!(!text.contains(key), "{text}");
    }
    assert_eq!(config.port().unwrap(), 48123);
}

/// A profile from a host that streamed with Intel or software encoding, or
/// limited frames in the NVIDIA driver.
const REMOVED_FEATURES: &str = "encoder = quicksync\nframe_limiter_provider = nvidia-control-panel\nrtss_frame_limit_type = nvidia-reflex\nqsv_preset = slow\nsw_preset = fast\nmin_threads = 4\nframe_limiter_disable_vsync = true\nrtss_disable_vsync_ullm = true\ndd_wa_dummy_plug_hdr10 = true\nport = 48123\n";

#[test]
fn removed_encoders_and_nvidia_limiters_migrate_with_one_report() {
    let (config, retired) = Config::parse_retiring(REMOVED_FEATURES).unwrap();
    assert_eq!(config.get("encoder", ""), "auto");
    assert_eq!(config.get("frame_limiter_provider", ""), "auto");
    // RTSS's Reflex mode came back with NVENC, so it stays.
    assert_eq!(config.get("rtss_frame_limit_type", ""), "nvidia-reflex");
    assert_eq!(config.port().unwrap(), 48123);
    // The removed encoders' and the NVIDIA driver's own settings go quietly.
    assert_eq!(config.values.len(), 4, "{:?}", config.values);
    assert_eq!(
        retired
            .iter()
            .map(|r| (r.key.as_str(), r.value.as_str(), r.current))
            .collect::<Vec<_>>(),
        [
            ("encoder", "quicksync", "auto"),
            ("frame_limiter_provider", "nvidia-control-panel", "auto"),
        ]
    );
    let warning = retired_warning(&retired).unwrap();
    assert!(
        warning.contains("encoder quicksync is now auto"),
        "{warning}"
    );
    assert!(warning.contains("frame_limiter_provider nvidia-control-panel is now auto"));
    // What the host writes back reads without a report.
    let (again, retired) = Config::parse_retiring(&config.text()).unwrap();
    assert_eq!(again.values, config.values);
    assert!(retired.is_empty());
    assert!(retired_warning(&retired).is_none());
}

#[test]
fn every_encoder_the_host_no_longer_has_selects_automatically() {
    for name in [
        "quicksync",
        "qsv",
        "software",
        "mediafoundation",
        "\"qsv\"",
        "something_else",
    ] {
        let (config, retired) = Config::parse_retiring(&format!("encoder = {name}\n")).unwrap();
        assert_eq!(config.get("encoder", ""), "auto", "{name}");
        assert_eq!(retired.len(), 1, "{name}");
    }
    for kept in [
        "auto",
        "amf",
        "nvenc",
        "pyrowave",
        "amdvce",
        "amdvce_legacy",
        "nvenc_legacy",
    ] {
        let (config, retired) = Config::parse_retiring(&format!("encoder = {kept}\n")).unwrap();
        assert_eq!(config.get("encoder", ""), kept);
        assert!(retired.is_empty(), "{kept}");
    }
    for (key, old, current) in [
        ("frame_limiter_provider", "nvcp", "auto"),
        ("frame_limiter_provider", "NVIDIA", "auto"),
    ] {
        let config = Config::parse(&format!("{key} = {old}\n")).unwrap();
        assert_eq!(config.get(key, ""), current, "{key} {old}");
    }
    for (key, kept) in [
        ("frame_limiter_provider", "rtss"),
        ("frame_limiter_provider", "none"),
        ("rtss_frame_limit_type", "back-edge-sync"),
        ("rtss_frame_limit_type", "nvidia-reflex"),
        ("rtss_frame_limit_type", "reflex"),
    ] {
        let config = Config::parse(&format!("{key} = {kept}\n")).unwrap();
        assert_eq!(config.get(key, ""), kept);
    }
    // A save sending a removed encoder stores automatic selection.
    let mut config = Config::default();
    change(&mut config, "encoder", "qsv").unwrap();
    assert_eq!(config.get("encoder", ""), "auto");
    change(&mut config, "encoder", "nvenc").unwrap();
    assert_eq!(config.get("encoder", ""), "nvenc");
    // An app's or a device's own overrides.
    let mut app = serde_json::json!({"name": "Game", "frame-generation-mode": "nvidia-smooth-motion",
            "config-overrides": {"encoder": "software", "frame_limiter_provider": "nvidia-control-panel"}});
    assert!(migrate_app(app.as_object_mut().unwrap()));
    assert_eq!(
        app,
        serde_json::json!({"name": "Game", "frame-generation-mode": "none",
                "config-overrides": {"encoder": "auto", "frame_limiter_provider": "auto"}})
    );
    assert!(!migrate_app(app.as_object_mut().unwrap()));
}

#[test]
fn retired_vigem_controllers_become_their_vhf_pads() {
    let c = Config::parse("gamepad = x360\n").unwrap();
    assert_eq!(c.get("gamepad", ""), "vhf_xbox_one");
    assert_eq!(c.text().trim(), "gamepad = vhf_xbox_one");
    let c = Config::parse("gamepad=ds4\n").unwrap();
    assert_eq!(c.get("gamepad", ""), "vhf_ds4");
    for kept in ["auto", "vhf_ds5", "ds5", "vhf_xbox"] {
        let c = Config::parse(&format!("gamepad={kept}\n")).unwrap();
        assert_eq!(c.get("gamepad", ""), kept);
    }
    // Only the gamepad setting has these retired values.
    assert_eq!(
        Config::parse("unknown_key=x360\n")
            .unwrap()
            .get("unknown_key", ""),
        "x360"
    );
    assert_eq!(replacement("gamepad", "x360"), Some("vhf_xbox_one"));
    assert_eq!(replacement("gamepad", "vhf_ds4"), None);
    // An app's or a device's own settings.
    let mut app = serde_json::json!({"name": "x360", "gamepad": "ds4", "output": "x360"});
    assert!(replace_retired(app.as_object_mut().unwrap()));
    assert_eq!(
        app,
        serde_json::json!({"name": "x360", "gamepad": "vhf_ds4", "output": "x360"})
    );
    assert!(!replace_retired(app.as_object_mut().unwrap()));
    let mut app = serde_json::json!({"name": "Game", "gamepad": "x360",
            "config-overrides": {"gamepad": "ds4", "fec_percentage": "30"}});
    assert!(migrate_app(app.as_object_mut().unwrap()));
    assert_eq!(
        app,
        serde_json::json!({"name": "Game", "gamepad": "vhf_xbox_one",
                "config-overrides": {"gamepad": "vhf_ds4", "fec_percentage": "30"}})
    );
    assert!(!migrate_app(app.as_object_mut().unwrap()));
    // A device's block in the app too.
    let mut app = serde_json::json!({"name": "Game",
            "device-overrides": {"tv": {"config-overrides": {"gamepad": "x360"}}}});
    assert!(migrate_app(app.as_object_mut().unwrap()));
    assert_eq!(
        app["device-overrides"]["tv"]["config-overrides"]["gamepad"],
        "vhf_xbox_one"
    );
    assert!(!migrate_app(app.as_object_mut().unwrap()));
}

/// A Vibepollo or Rubylight 2.0 file with every setting under its
/// earlier name, a setting this host does not know, and a comment.
const EARLIER_NAMES: &str = "# Vibepollo\nrtss_path = D:\\Tools\\RTSS\nnvenc_force_split_encode = driver_decides\nrtx_hdr_peak_brightness = \"1400\"\nport = 48123\nframe_limiter_auto_virtual_framegen = vrr\nexperimental_packed_444 = false\nfuture_setting = kept\n";

#[test]
fn settings_under_an_earlier_name_are_read_under_the_current_one() {
    let c = Config::parse(EARLIER_NAMES).unwrap();
    // What the host used before through its fallbacks, now under the
    // names the console shows.
    assert_eq!(c.get("rtss_install_path", ""), "D:\\Tools\\RTSS");
    assert_eq!(c.get("nvenc_split_encode", "auto"), "driver_decides");
    assert_eq!(c.integer("hdr_peak_brightness", 0), 1400);
    assert_eq!(c.get("virtual_display_refresh", ""), "vrr");
    assert!(!c.packed_444());
    for (old, _) in RENAMED_KEYS {
        assert!(!c.values.contains_key(*old), "{old}");
    }
    assert_eq!(c.get("future_setting", ""), "kept");
    assert_eq!(c.port().unwrap(), 48123);
    // With both names set the host read the current one; it stays.
    let both = Config::parse(
            "rtss_install_path = C:\\RTSS\nrtss_path = D:\\RTSS\nnvenc_split_encode = disabled\nnvenc_force_split_encode = forced\nhdr_peak_brightness = 1000\nrtx_hdr_peak_brightness = 1400\n",
        )
        .unwrap();
    assert_eq!(both.get("rtss_install_path", ""), "C:\\RTSS");
    assert_eq!(both.get("nvenc_split_encode", ""), "disabled");
    assert_eq!(both.integer("hdr_peak_brightness", 0), 1000);
    assert_eq!(both.values.len(), 3);
    assert_eq!(current_name("rtss_path"), "rtss_install_path");
    assert_eq!(current_name("rtss_install_path"), "rtss_install_path");
    assert_eq!(current_name("port"), "port");
}

#[test]
fn the_launchers_sync_options_fold_into_the_shared_library_sync_ones() {
    // Every launcher agreeing takes its value as the shared one; the
    // per-launcher keys go.
    let agreed = Config::parse(
        "steam_auto_sync = false\nepic_auto_sync = false\nbattlenet_auto_sync = false\nmicrosoft_auto_sync = false\nea_auto_sync = false\nubisoft_auto_sync = false\ngog_auto_sync = false\nlibrary_local_auto_sync = false\nplaynite_auto_sync = false\n",
    )
    .unwrap();
    assert_eq!(agreed.get("library_auto_sync", ""), "false");
    for prefix in super::retired::LIBRARY_PREFIXES {
        assert!(
            !agreed.values.contains_key(&format!("{prefix}_auto_sync")),
            "{prefix}"
        );
    }
    // Agreeing on the default writes nothing at all.
    let defaults = Config::parse("steam_auto_sync = true\nepic_auto_sync = true\n").unwrap();
    assert_eq!(defaults.values, BTreeMap::new());
    // One launcher differs: it keeps its key as an override, the shared key
    // stays unset (the default), and the fold survives a save.
    let one = Config::parse("gog_auto_sync = false\n").unwrap();
    assert!(!one.values.contains_key("library_auto_sync"));
    assert_eq!(one.get("gog_auto_sync", ""), "false");
    assert!(one.boolean("library_auto_sync", true));
    assert_eq!(Config::parse(&one.text()).unwrap().values, one.values);
    // The recent-games options fold over the launchers that keep a play
    // history, and a value already saved under the shared name wins.
    let mixed = Config::parse(
        "battlenet_sync_all_installed = false\nbattlenet_recent_games = 3\nlibrary_recent_max_age_days = 0\nsteam_recent_max_age_days = 30\nplaynite_recent_max_age_days = 30\n",
    )
    .unwrap();
    assert!(mixed.boolean("library_sync_all_installed", true));
    assert_eq!(mixed.get("battlenet_sync_all_installed", ""), "false");
    // Launchers disagreeing leave the shared value at its default, which is
    // not written; the one that differs keeps its own.
    assert!(!mixed.values.contains_key("library_recent_games"));
    assert_eq!(mixed.integer("library_recent_games", 10), 10);
    assert_eq!(mixed.integer("battlenet_recent_games", 0), 3);
    assert_eq!(mixed.integer("library_recent_max_age_days", 0), 0);
    assert_eq!(mixed.integer("steam_recent_max_age_days", 0), 30);
    assert_eq!(mixed.integer("playnite_recent_max_age_days", 0), 30);
    // A profile from before the options were split: its shared switches never
    // covered Steam or Playnite, which kept syncing, so they keep their own.
    let before_split = Config::parse(
        "library_auto_sync = false\nlibrary_autosync_remove_uninstalled = false\nauto_update = false\n",
    )
    .unwrap();
    for option in ["auto_sync", "autosync_remove_uninstalled"] {
        assert_eq!(before_split.get(&format!("library_{option}"), ""), "false");
        for prefix in ["steam", "playnite"] {
            assert_eq!(
                before_split.get(&format!("{prefix}_{option}"), ""),
                "true",
                "{prefix}_{option}"
            );
        }
        assert!(!before_split.values.contains_key(&format!("epic_{option}")));
    }
    assert_eq!(
        Config::parse(&before_split.text()).unwrap().values,
        before_split.values
    );
    // This release's own file, with the shared switch off, turns them all off.
    let current = Config::parse("library_auto_sync = false\n").unwrap();
    assert!(!current.values.contains_key("steam_auto_sync"));
    assert!(!current.boolean("library_auto_sync", true));
}

#[test]
fn saving_the_shared_library_sync_setting_redecides_every_launcher() {
    // A save that leaves the shared value as held keeps a launcher's
    // override, so saving another setting does not change that launcher.
    let mut config = Config::parse("gog_auto_sync = false\n").unwrap();
    let mut object = serde_json::Map::new();
    object.insert("library_auto_sync".into(), true.into());
    object.insert("sunshine_name".into(), "Pegasus".into());
    config.update(&object).unwrap();
    assert_eq!(config.get("gog_auto_sync", ""), "false");
    assert_eq!(config.get("library_auto_sync", ""), "true");
    // Changing the shared value re-decides it for every launcher: the
    // override goes, as resetting the shared key does.
    let mut object = serde_json::Map::new();
    object.insert("library_auto_sync".into(), false.into());
    config.update(&object).unwrap();
    assert_eq!(config.get("library_auto_sync", ""), "false");
    assert!(!config.values.contains_key("gog_auto_sync"));
}

#[test]
fn a_saved_configuration_holds_settings_under_their_current_names() {
    // The console saves every setting as held after parsing, so the
    // first save after an upgrade retires the earlier names.
    let before = Config::parse(EARLIER_NAMES).unwrap();
    let text = before.text();
    for (old, new) in RENAMED_KEYS {
        assert!(!text.contains(&format!("{old} =")), "{text}");
        assert!(text.contains(&format!("{new} =")), "{text}");
    }
    assert!(text.contains("future_setting = kept"), "{text}");
    assert_eq!(Config::parse(&text).unwrap().values, before.values);
}

#[test]
fn a_setting_sent_under_an_earlier_name_is_saved_under_the_current_one() {
    let mut config = Config::parse("nvenc_split_encode = disabled\n").unwrap();
    change(&mut config, "nvenc_force_split_encode", "forced").unwrap();
    assert_eq!(config.get("nvenc_split_encode", ""), "forced");
    assert!(!config.values.contains_key("nvenc_force_split_encode"));
    // Sent together, the current name wins, as in the file.
    let mut object = serde_json::Map::new();
    object.insert("rtss_path".into(), "D:\\Old".into());
    object.insert("rtss_install_path".into(), "C:\\New".into());
    config.update(&object).unwrap();
    assert_eq!(config.get("rtss_install_path", ""), "C:\\New");
    assert!(!config.values.contains_key("rtss_path"));
    // Resetting the earlier name resets the setting.
    let mut object = serde_json::Map::new();
    object.insert("rtss_path".into(), serde_json::Value::Null);
    config.update(&object).unwrap();
    assert!(!config.values.contains_key("rtss_install_path"));
    // The value is checked under the name it is saved under.
    assert!(change(&mut config, "rtss_path", "C:\\RTSS #2").is_err());
    assert!(!config.values.contains_key("rtss_install_path"));
}

/// A Vibepollo or earlier Rubylight file that holds the renamed settings: the
/// host reads them under their current names (`virtual_display_refresh`,
/// `packed_444`) with the same meaning, and saves them under those.
#[test]
fn the_virtual_display_refresh_and_full_chroma_keep_their_values_under_new_names() {
    let c = Config::parse(
        "# Vibepollo\nframe_limiter_auto_virtual_framegen = enabled\nexperimental_packed_444 = false\n",
    )
    .unwrap();
    assert!(!c.packed_444());
    assert_eq!(c.get("virtual_display_refresh", ""), "enabled");
    assert_eq!(c.get("packed_444", ""), "false");
    let text = c.text();
    assert!(
        !text.contains("frame_limiter_auto_virtual_framegen"),
        "{text}"
    );
    assert!(!text.contains("experimental_packed_444"), "{text}");
    // Sent under the earlier name by an older console or a script.
    let mut config = Config::default();
    change(
        &mut config,
        "frame_limiter_auto_virtual_framegen",
        "disabled",
    )
    .unwrap();
    change(&mut config, "experimental_packed_444", "false").unwrap();
    assert_eq!(config.get("virtual_display_refresh", ""), "disabled");
    assert!(!config.packed_444());
    assert!(!config.values.contains_key("experimental_packed_444"));
    // An app or a device may still set both.
    assert!(override_allowed("virtual_display_refresh") && override_allowed("packed_444"));
    assert_eq!(current_name("experimental_packed_444"), "packed_444");
    assert_eq!(
        current_name("frame_limiter_auto_virtual_framegen"),
        "virtual_display_refresh"
    );
}

/// A setting that never had an effect is dropped wherever it is stored.
#[test]
fn the_touch_keyboard_setting_is_removed_with_its_stored_value() {
    let c = Config::parse("touch_keyboard_digitizer = true\nsunshine_name = Den\n").unwrap();
    assert!(!c.values.contains_key("touch_keyboard_digitizer"));
    assert!(!c.text().contains("touch_keyboard_digitizer"));
    let mut config = Config::default();
    change(&mut config, "touch_keyboard_digitizer", "true").unwrap();
    assert!(!config.values.contains_key("touch_keyboard_digitizer"));
    assert!(removed("touch_keyboard_digitizer"));
    assert!(
        !SETTINGS_30
            .iter()
            .any(|(key, _)| *key == "touch_keyboard_digitizer")
    );
}

/// The retired host bitrate controller's switch, true or false, is dropped
/// wherever it is stored, and no other value changes with it.
#[test]
fn the_host_bitrate_controller_setting_is_dropped_without_changing_others() {
    for value in ["true", "false"] {
        let text = format!(
            "host_adaptive_bitrate = {value}
link_rate_adaptation = false
host_adaptive_pacing = true
"
        );
        let (c, retired) = Config::parse_retiring(&text).unwrap();
        assert!(retired.is_empty(), "{value}");
        assert!(!c.values.contains_key("host_adaptive_bitrate"), "{value}");
        assert!(!c.text().contains("host_adaptive_bitrate"), "{value}");
        assert!(!c.link_rate_adaptation(), "{value}");
        assert!(c.adaptive_packet_pacing(0, false), "{value}");
        // A console, script or client that still sends it is accepted.
        let mut config = Config::default();
        change(&mut config, "host_adaptive_bitrate", value).unwrap();
        assert!(
            !config.values.contains_key("host_adaptive_bitrate"),
            "{value}"
        );
        // An app's or a device's overrides lose it and keep the rest.
        let mut layer = serde_json::json!({"host_adaptive_bitrate": value, "max_bitrate": 50000});
        assert!(migrate_overrides(layer.as_object_mut().unwrap()));
        assert_eq!(layer, serde_json::json!({"max_bitrate": 50000}));
    }
    assert!(removed("host_adaptive_bitrate"));
    assert!(!override_allowed("host_adaptive_bitrate"));
    assert!(
        !SETTINGS_30
            .iter()
            .any(|(key, _)| *key == "host_adaptive_bitrate")
    );
}

/// The phase lock always stands aside for a game capped at the stream rate,
/// so the setting that asked it to hold is dropped wherever it is stored.
#[test]
fn the_phase_lock_hold_setting_is_dropped() {
    let c = Config::parse("phase_lock_hold_at_stream_rate = true\n").unwrap();
    assert!(!c.values.contains_key("phase_lock_hold_at_stream_rate"));
    assert!(!c.text().contains("phase_lock_hold_at_stream_rate"));
    let mut config = Config::default();
    change(&mut config, "phase_lock_hold_at_stream_rate", "true").unwrap();
    assert!(!config.values.contains_key("phase_lock_hold_at_stream_rate"));
    assert!(removed("phase_lock_hold_at_stream_rate"));
    assert!(!override_allowed("phase_lock_hold_at_stream_rate"));
    // An app's or a device's overrides lose it and keep the rest.
    let mut layer =
        serde_json::json!({"phase_lock_hold_at_stream_rate": true, "max_bitrate": 50000});
    assert!(migrate_overrides(layer.as_object_mut().unwrap()));
    assert_eq!(layer, serde_json::json!({"max_bitrate": 50000}));
}

/// An app record from Apollo, Vibepollo or an earlier Rubylight, as
/// apps.json holds it: removed switches go, renamed ones and renamed or
/// folded overrides take their current names in the app and in its devices'
/// blocks, and a second pass changes nothing.
#[test]
fn an_app_record_is_brought_to_the_current_names() {
    let mut app = serde_json::json!({
        "name": "Game",
        "frame-gen-limiter-fix": true,
        "virtual-screen": "true",
        "gen1-framegen-fix": true,
        "config-overrides": {
            "experimental_packed_444": false,
            "frame_limiter_auto_virtual_framegen": "enabled",
            "dd_hdr_request_override": "force_on",
            "touch_keyboard_digitizer": true,
            "max_bitrate": 50000
        },
        "device-overrides": {"tv": {"config-overrides": {
            "dd_hdr_option": "disabled",
            "dd_hdr_request_override": "force_off",
            "experimental_packed_444": true
        }}}
    });
    assert!(migrate_app(app.as_object_mut().unwrap()));
    assert_eq!(
        app,
        serde_json::json!({
            "name": "Game",
            "virtual-display": true,
            "gen1-framegen-fix": true,
            "config-overrides": {
                "packed_444": false,
                "virtual_display_refresh": "enabled",
                // Alone, it folds over the host's Display HDR when applied.
                "dd_hdr_request_override": "force_on",
                "max_bitrate": 50000
            },
            "device-overrides": {"tv": {"config-overrides": {
                "dd_hdr_option": "disabled",
                "packed_444": true
            }}}
        })
    );
    assert!(!migrate_app(app.as_object_mut().unwrap()));
    // `virtual-screen` off asks for nothing, and never turns the current switch off.
    for (before, after) in [
        (
            serde_json::json!({"name": "A", "virtual-screen": false}),
            serde_json::json!({"name": "A"}),
        ),
        (
            serde_json::json!({"name": "A", "virtual-screen": false, "virtual-display": true}),
            serde_json::json!({"name": "A", "virtual-display": true}),
        ),
        (
            serde_json::json!({"name": "A", "virtual-screen": true, "virtual-display": false}),
            serde_json::json!({"name": "A", "virtual-display": true}),
        ),
    ] {
        let mut app = before.clone();
        assert!(migrate_app(app.as_object_mut().unwrap()));
        assert_eq!(app, after, "{before}");
    }
}

/// The retired compute-queue, AMF queue and AMF low-latency settings are
/// dropped from every layer, and AMF no longer forces either.
#[test]
fn the_retired_driver_switches_are_dropped_and_no_longer_reach_the_amf_encoder() {
    for key in [
        "compute_queue_realtime",
        "amd_input_queue_size",
        "amd_lowlatency_mode",
    ] {
        assert!(super::removed(key), "{key}");
        assert!(!super::override_allowed(key), "{key}");
        let mut layer = serde_json::json!({ key: "1", "fec_percentage": 20 })
            .as_object()
            .unwrap()
            .clone();
        assert!(super::migrate_overrides(&mut layer));
        assert!(!layer.contains_key(key));
    }
}
