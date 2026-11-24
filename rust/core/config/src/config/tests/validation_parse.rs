//! Tests for sunshine.conf parsing, encoding compatibility, save validation, and overrides.
use super::*;

#[test]
fn hardware_metrics_saves_validate_interval_and_selection() {
    let mut config = super::Config::default();
    for value in ["500", "1000", "10000"] {
        change(&mut config, "hardware_metrics_interval", value).unwrap();
    }
    for value in ["499", "10001", "invalid", "1.5"] {
        assert!(change(&mut config, "hardware_metrics_interval", value).is_err());
    }
    for value in [
        serde_json::Value::Null,
        serde_json::Value::String(String::new()),
    ] {
        change(&mut config, "hardware_metrics_interval", "750").unwrap();
        let mut object = serde_json::Map::new();
        object.insert("hardware_metrics_interval".into(), value);
        config.update(&object).unwrap();
        assert!(!config.values.contains_key("hardware_metrics_interval"));
        assert_eq!(config.hardware_metrics_interval(), 1000);
    }
    change(
        &mut config,
        "hardware_metrics_selection",
        "[cpu_usage_percent, gpu_temperature_c]",
    )
    .unwrap();
    assert_eq!(
        config.list("hardware_metrics_selection"),
        ["cpu_usage_percent", "gpu_temperature_c"]
    );
    assert!(
        change(
            &mut config,
            "hardware_metrics_selection",
            "[cpu_usage_percent, cpu_temperature_c]"
        )
        .is_err()
    );
    assert_eq!(
        config.list("hardware_metrics_selection"),
        ["cpu_usage_percent", "gpu_temperature_c"]
    );
    change(&mut config, "hardware_metrics_selection", "[]").unwrap();
    assert!(config.list("hardware_metrics_selection").is_empty());
    let mut config =
        super::Config::parse("hardware_metrics_interval=0\nhardware_metrics_selection=[unknown]\n")
            .unwrap();
    change(&mut config, "sunshine_name", "Gaming PC").unwrap();
    assert_eq!(config.list("hardware_metrics_selection"), ["unknown"]);
}

#[test]
fn a_saved_ltr_zero_from_before_2_0_0_gives_way_to_the_new_default_once() {
    let mut config = super::Config::parse("amd_ltr_frames = 0\nport = 47989\n").unwrap();
    assert!(config.upgrade_ltr_default(false));
    assert!(!config.values.contains_key("amd_ltr_frames"));
    assert_eq!(config.integer("port", 0), 47989);
    // Chosen after the upgrade, 0 stays off.
    let mut config = super::Config::parse("amd_ltr_frames = 0\n").unwrap();
    assert!(!config.upgrade_ltr_default(true));
    assert_eq!(config.integer("amd_ltr_frames", 4), 0);
    // Other values are the user's own.
    for text in ["", "amd_ltr_frames = 1\n", "amd_ltr_frames = 4\n"] {
        let mut config = super::Config::parse(text).unwrap();
        let before = config.values.clone();
        assert!(!config.upgrade_ltr_default(false), "{text}");
        assert_eq!(config.values, before);
    }
}

#[test]
fn overrides_allow_stream_input_display_and_encoder_keys_but_not_host_settings() {
    for key in [
        "max_bitrate",
        "stream_audio",
        "keyboard",
        "gamepad",
        "dd_hdr_option",
        "dd_mode_remapping",
        "encoder",
        "amd_quality",
        "encoder_focus",
        "hdr_dynamic_metadata",
        "hdr_dolby_vision",
        "hdr_hlg",
        "frame_limiter_fps_limit",
        "packet_pacing",
    ] {
        assert!(super::override_allowed(key), "{key}");
    }
    for key in [
        "port",
        "bind_address",
        "file_apps",
        "cert",
        "pkey",
        "enable_pairing",
        "server_cmd",
        "global_prep_cmd",
        "log_path",
        "unknown",
    ] {
        assert!(!super::override_allowed(key), "{key}");
    }
}

#[test]
fn audio_processing_overrides_per_device_and_app_but_idle_sleep_stays_host_wide() {
    assert!(super::override_allowed("audio_night_mode"));
    assert!(super::override_allowed("audio_headphone_surround"));
    assert!(!super::override_allowed("sleep_after_idle_minutes"));
    assert!(!super::override_allowed("sleep_while_suspended"));
}

#[test]
fn idle_sleep_minutes_are_off_or_between_5_and_1440() {
    let mut config = super::Config::default();
    for value in ["0", "5", "30", "1440", "\"60\""] {
        change(&mut config, "sleep_after_idle_minutes", value).unwrap();
    }
    assert_eq!(config.integer("sleep_after_idle_minutes", -1), 60);
    for value in ["3", "4", "1441", "-5", "off", "1.5", "true"] {
        let error = change(&mut config, "sleep_after_idle_minutes", value).unwrap_err();
        assert!(
            error.to_string().contains("sleep_after_idle_minutes"),
            "{value}: {error}"
        );
    }
    assert_eq!(config.integer("sleep_after_idle_minutes", -1), 60);
    // A JSON number from the console is checked the same way.
    let mut object = serde_json::Map::new();
    object.insert("sleep_after_idle_minutes".into(), 3.into());
    assert!(config.update(&object).is_err());
    object.insert("sleep_after_idle_minutes".into(), 15.into());
    config.update(&object).unwrap();
    assert_eq!(config.integer("sleep_after_idle_minutes", -1), 15);
    // Null resets it to off.
    object.insert("sleep_after_idle_minutes".into(), serde_json::Value::Null);
    config.update(&object).unwrap();
    assert!(!config.values.contains_key("sleep_after_idle_minutes"));
}

#[test]
fn audio_processing_and_suspended_sleep_settings_are_booleans() {
    for key in [
        "audio_night_mode",
        "audio_headphone_surround",
        "sleep_while_suspended",
    ] {
        let mut config = super::Config::default();
        for value in ["true", "on", "enabled", "1"] {
            change(&mut config, key, value).unwrap();
            assert!(config.boolean(key, false), "{key}={value}");
        }
        for value in ["false", "off", "0"] {
            change(&mut config, key, value).unwrap();
            assert!(!config.boolean(key, true), "{key}={value}");
        }
        for value in ["maybe", "2", "night"] {
            let error = change(&mut config, key, value).unwrap_err();
            assert!(error.to_string().contains(key), "{key}={value}: {error}");
        }
        let mut object = serde_json::Map::new();
        object.insert(key.into(), true.into());
        config.update(&object).unwrap();
        assert!(config.boolean(key, false), "{key} as a JSON boolean");
    }
}

#[test]
fn a_device_or_app_audio_override_beats_the_host_and_a_bad_one_is_refused() {
    // The host applies device, then app overrides onto its own file
    // through `update`, one key at a time.
    let mut config = super::Config::parse("audio_night_mode = off\n").unwrap();
    let layer = |config: &mut super::Config, key: &str, value: serde_json::Value| {
        config.update(&serde_json::Map::from_iter([(key.to_owned(), value)]))
    };
    layer(&mut config, "audio_night_mode", true.into()).unwrap();
    assert!(config.boolean("audio_night_mode", false));
    layer(&mut config, "audio_night_mode", "off".into()).unwrap();
    assert!(!config.boolean("audio_night_mode", true));
    assert!(layer(&mut config, "audio_headphone_surround", "loud".into()).is_err());
    assert!(!config.values.contains_key("audio_headphone_surround"));
}

#[test]
fn overrides_that_are_never_saved_may_hold_a_comment_sign() {
    let mut config = Config::default();
    let mut object = serde_json::Map::new();
    object.insert("sunshine_name".into(), "Gaming PC #2".into());
    config.update(&object).unwrap();
    assert_eq!(config.get("sunshine_name", ""), "Gaming PC #2");
}

#[test]
fn a_bad_value_already_in_the_file_does_not_block_other_saves() {
    let mut config = Config::parse("port = 70000\nframe_limiter_fps_limit = -1\n").unwrap();
    change(&mut config, "encoder", "amf").unwrap();
    assert_eq!(config.get("encoder", ""), "amf");
    assert!(change(&mut config, "port", "70000").is_err());
    change(&mut config, "port", "48000").unwrap();
}

#[test]
fn values_the_file_cannot_give_back_are_refused() {
    let mut config = Config::default();
    // A comment would cut it short.
    assert!(change(&mut config, "sunshine_name", "Gaming PC #2").is_err());
    change(&mut config, "sunshine_name", "\"Gaming PC #2\"").unwrap();
    // An unclosed bracket would swallow the settings after it.
    assert!(change(&mut config, "adapter_name", "[Radeon").is_err());
    assert!(!config.values.contains_key("adapter_name"));
    // Spaces around a value and indented lists come back as the file reads them.
    change(&mut config, "sunshine_name", "  Living room  ").unwrap();
    change(&mut config, "global_prep_cmd", "[\n  {\"do\": \"a\"}\n]").unwrap();
    let reread = Config::parse(&config.text()).unwrap();
    assert_eq!(reread.get("sunshine_name", ""), "Living room");
    assert_eq!(reread.get("global_prep_cmd", ""), "[\n{\"do\": \"a\"}\n]");
}

#[test]
fn bind_address_and_log_path_that_would_stop_the_host_are_refused() {
    let mut config = Config::default();
    assert!(change(&mut config, "bind_address", "Ethernet").is_err());
    change(&mut config, "bind_address", "192.168.1.5").unwrap();
    change(&mut config, "bind_address", "").unwrap();
    assert!(!config.values.contains_key("bind_address"));
    assert!(change(&mut config, "log_path", "C:\\Logs\\").is_err());
    let folder = std::env::temp_dir();
    assert!(change(&mut config, "log_path", folder.to_str().unwrap()).is_err());
    change(&mut config, "log_path", "D:\\Logs\\host.log").unwrap();
}

/// Every setting the NVENC encoder reads (nvenc.rs, and its GPU priority in
/// gpu_priority.rs) stays in the configuration, and an app or a device may
/// override it as the AMF ones.
#[test]
fn nvenc_settings_are_kept_and_overridable() {
    let source = [
        include_str!("../../../../media-policy/src/nvenc.rs"),
        include_str!("../../../../../windows/graphics/src/gpu_priority.rs"),
    ]
    .concat();
    let keys: std::collections::BTreeSet<&str> = source
        .match_indices("\"nvenc_")
        .filter_map(|(at, _)| source[at + 1..].split('"').next())
        .filter(|key| {
            key.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        })
        .collect();
    assert!(keys.len() >= 17, "{keys:?}");
    assert!(keys.contains("nvenc_realtime_hags"), "{keys:?}");
    for key in keys {
        assert!(super::override_allowed(key), "{key}");
        assert!(!super::removed(key), "{key}");
        let (config, retired) = Config::parse_retiring(&format!("{key} = 1\n")).unwrap();
        assert_eq!(config.get(key, ""), "1", "{key}");
        assert!(retired.is_empty(), "{key}");
    }
}

#[test]
fn existing_config_round_trips() {
    let c = Config::parse("# Apollo\nport=48123\nunknown_key = custom\nprep_cmd = [\n {\"do\":\"echo #kept\",\"undo\":\"\"}\n]\n").unwrap();
    assert_eq!(c.ports().unwrap().rtsp, 48144);
    assert_eq!(Config::parse(&c.text()).unwrap().values, c.values);
    assert_eq!(c.get("unknown_key", ""), "custom");
    for (legacy, expected) in [
        ("debug", "debug"),
        ("1", "debug"),
        ("warning", "warn"),
        ("verbose", "trace"),
        ("fatal", "error"),
        ("none", "off"),
        ("invalid", "info"),
    ] {
        assert_eq!(
            Config::parse(&format!("min_log_level={legacy}\n"))
                .unwrap()
                .log_level(),
            expected
        );
    }
}

#[test]
fn vibepollo_files_load_like_vibepollo() {
    let c = Config::parse(concat!(
        "\u{feff}sunshine_name = 27\" PC # living room\n",
        "not a setting\n",
        "keybindings = [0x10, 0xA0, \"0x11\", 0xA2]\n",
        "csrf_allowed_origins = [https://a.example, https://b.example]\n",
        "encoder =\n",
        "port = \"48123\"\n",
        "port = 50000\n",
        "audio_sink = Speakers\n",
        "bad key = 1\n",
        "global_prep_cmd = [\n",
        "  {\"do\":\"echo #1\",\"undo\":\"\"}\n",
        "]\n",
        "dd_snapshot_exclude_devices = DISPLAY1, \"DISPLAY2\"\n",
        "fec_percentage = 0x1e\n",
        "enable_pairing = enable\n",
    ))
    .unwrap();
    assert_eq!(c.get("sunshine_name", ""), "27\" PC");
    assert_eq!(c.list("keybindings"), ["0x10", "0xA0", "0x11", "0xA2"]);
    assert_eq!(
        c.list("csrf_allowed_origins"),
        ["https://a.example", "https://b.example"]
    );
    assert_eq!(c.get("encoder", "auto"), "auto");
    assert!(!c.values.contains_key("encoder"));
    assert_eq!(c.port().unwrap(), 48123);
    assert_eq!(c.get("audio_sink", ""), "Speakers");
    assert!(!c.values.contains_key("bad key") && !c.values.contains_key("key"));
    assert_eq!(
        c.get("global_prep_cmd", ""),
        "[\n{\"do\":\"echo #1\",\"undo\":\"\"}\n]"
    );
    assert_eq!(
        c.list("dd_snapshot_exclude_devices"),
        ["DISPLAY1", "DISPLAY2"]
    );
    assert_eq!(c.integer("fec_percentage", 20), 30);
    assert!(c.boolean("enable_pairing", false));
    assert_eq!(
        Config::parse("port = 70000\n").unwrap().port().unwrap(),
        47989
    );
    // An unclosed list is dropped; the rest of the file still loads.
    let c = Config::parse("keybindings = [0x10,\nport = 48123\n").unwrap();
    assert_eq!(c.port().unwrap(), 48123);
    assert!(!c.values.contains_key("keybindings"));
    let c = Config::parse("port = 48123\nkeybindings = [0x10,\n").unwrap();
    assert_eq!(c.port().unwrap(), 48123);
    assert!(!c.values.contains_key("keybindings"));
}

#[test]
fn files_saved_as_utf16_or_in_the_ansi_code_page_load() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("sunshine.conf");
    let text = "sunshine_name = Café € PC\r\nport = 48123 # base\r\n";
    let mut utf16 = vec![0xff, 0xfe];
    utf16.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
    let mut big_endian = vec![0xfe, 0xff];
    big_endian.extend(text.encode_utf16().flat_map(u16::to_be_bytes));
    let mut bom = b"\xef\xbb\xbf".to_vec();
    bom.extend_from_slice(text.as_bytes());
    let ansi = b"sunshine_name = Caf\xe9 \x80 PC\r\nport = 48123 # base\r\n".to_vec();
    for bytes in [utf16, big_endian, bom, ansi, text.as_bytes().to_vec()] {
        std::fs::write(&path, bytes).unwrap();
        let c = Config::load(&path).unwrap();
        assert_eq!(c.get("sunshine_name", ""), "Café € PC");
        assert_eq!(c.port().unwrap(), 48123);
        assert_eq!(c.values.len(), 2);
    }
}

#[test]
fn update_is_transactional_and_rejects_injection() {
    let mut c = Config::default();
    assert!(
        c.update(
            serde_json::json!({"port":65535,"encoder":"amf"})
                .as_object()
                .unwrap()
        )
        .is_err()
    );
    assert!(c.values.is_empty());
    assert!(
        c.update(
            serde_json::json!({"encoder":"amf\nport=1"})
                .as_object()
                .unwrap()
        )
        .is_err()
    );
}
