#[test]
fn folder_ids_are_short_plain_names() {
    for id in ["games", "saves-2", "a_b", &"x".repeat(64)] {
        assert!(super::valid_folder_id(id), "{id}");
    }
    for id in [
        "",
        "../games",
        "a/b",
        "a b",
        "ü",
        &"x".repeat(65),
        "__proto__",
        "constructor",
        "prototype",
    ] {
        assert!(!super::valid_folder_id(id), "{id}");
    }
}

#[test]
fn a_shared_folder_is_a_folder_on_a_drive() {
    for root in [
        r"D:\Games",
        r"d:/Games/Saves",
        r"E:\",
        r"C:\Users\Player\My Games",
    ] {
        assert!(super::valid_folder_root(root), "{root}");
    }
    for root in [
        "",
        "Games",
        r"\Games",
        "D:",
        "D:Games",
        r"\\server\share",
        r"\\?\D:\Games",
        r"\\.\D:\Games",
        r"\\?\UNC\server\share",
        r"D:\Games\..\Windows",
        r"D:\Games\.",
        r"D:\Games:secret",
        r"D:\..",
    ] {
        assert!(!super::valid_folder_root(root), "{root}");
    }
    let config = |path: &str| {
        let list = serde_json::json!([{"id": "games", "name": "Games", "path": path}]);
        super::Config::parse(&format!("shared_folders = {list}"))
            .unwrap()
            .shared_folders()
    };
    assert_eq!(config(r"D:\Games").len(), 1);
    assert!(config(r"\\server\games").is_empty());
    assert!(config("games").is_empty());
}

#[test]
fn webrtc_video_is_paced_unless_the_measurement_switch_turns_it_off() {
    let mut config = super::Config::default();
    assert!(config.webrtc_pacing());
    let saved = serde_json::json!({"webrtc_pacing": "false"});
    config.update(saved.as_object().unwrap()).unwrap();
    assert!(!config.webrtc_pacing());
}

#[test]
fn file_transfer_is_off_with_4_gib_uploads_paced_at_50_mbps_while_streaming() {
    let mut config = super::Config::default();
    assert!(!config.file_transfer());
    assert_eq!(config.file_transfer_dir(), None);
    assert_eq!(config.file_transfer_max_bytes(), 4096 << 20);
    assert_eq!(config.file_transfer_stream_rate(), Some(6_250_000));
    let saved = serde_json::json!({
        "file_transfer": true,
        "file_transfer_dir": "\"D:\\Inbox\"",
        "file_transfer_max_mb": "2",
        "file_transfer_stream_mbps": 0,
    });
    config.update(saved.as_object().unwrap()).unwrap();
    assert!(config.file_transfer());
    assert_eq!(
        config.file_transfer_dir(),
        Some(std::path::PathBuf::from("D:\\Inbox"))
    );
    assert_eq!(config.file_transfer_max_bytes(), 2 << 20);
    assert_eq!(config.file_transfer_stream_rate(), None);
    for bad in ["0", "-1", "9223372036854775807"] {
        let saved = serde_json::json!({ "file_transfer_max_mb": bad, "file_transfer_dir": " " });
        config.update(saved.as_object().unwrap()).unwrap();
        assert_eq!(config.file_transfer_max_bytes(), 4096 << 20, "{bad}");
        assert_eq!(config.file_transfer_dir(), None);
    }
}

#[test]
fn usb_forwarding_is_off_with_4_devices_at_once() {
    let mut config = super::Config::default();
    assert!(!config.usb_forwarding());
    assert_eq!(config.usb_max_devices(), 4);
    let saved = serde_json::json!({"usb_forwarding": "enabled", "usb_max_devices": "16"});
    config.update(saved.as_object().unwrap()).unwrap();
    assert!(config.usb_forwarding());
    assert_eq!(config.usb_max_devices(), 16);
    for bad in ["0", "17", "-1", "lots"] {
        let saved = serde_json::json!({ "usb_max_devices": bad });
        config.update(saved.as_object().unwrap()).unwrap();
        assert_eq!(config.usb_max_devices(), 4, "{bad}");
    }
    let saved = serde_json::json!({"usb_max_devices": 1});
    config.update(saved.as_object().unwrap()).unwrap();
    assert_eq!(config.usb_max_devices(), 1);
}

#[test]
fn webrtc_rtx_is_on_and_datagram_loss_is_off_until_set() {
    let mut config = super::Config::default();
    assert!(config.webrtc_rtx());
    assert_eq!(config.webrtc_loss_permille(), 0);
    let saved = serde_json::json!({"webrtc_rtx": "false", "webrtc_loss_permille": "1500"});
    config.update(saved.as_object().unwrap()).unwrap();
    assert!(!config.webrtc_rtx());
    assert_eq!(config.webrtc_loss_permille(), 1000);
}

#[test]
fn encoder_output_switches_keep_the_measured_paths_until_set() {
    let mut config = super::Config::default();
    assert!(!config.amd_output_prediction());
    assert!(config.amd_output_thread());
    assert!(config.nvenc_output_event());
    let saved = serde_json::json!({
        "amd_output_prediction": "true",
        "amd_output_thread": "false",
        "nvenc_output_event": "false",
    });
    config.update(saved.as_object().unwrap()).unwrap();
    assert!(config.amd_output_prediction());
    assert!(!config.amd_output_thread());
    assert!(!config.nvenc_output_event());
}

#[test]
fn pyrowave_encode_overlap_is_off_until_turned_on() {
    let mut config = super::Config::default();
    assert!(!config.pyrowave_encode_overlap());
    let saved = serde_json::json!({ "pyrowave_encode_overlap": "true" });
    config.update(saved.as_object().unwrap()).unwrap();
    assert!(config.pyrowave_encode_overlap());
    let saved = serde_json::json!({ "pyrowave_encode_overlap": "false" });
    config.update(saved.as_object().unwrap()).unwrap();
    assert!(!config.pyrowave_encode_overlap());
}

#[test]
fn the_stream_resolution_cap_is_native_until_set_and_refuses_other_text() {
    let mut config = super::Config::default();
    // Today's behaviour: every device gets the picture it asks for.
    assert_eq!(config.max_stream_resolution(), None);
    let saved = serde_json::json!({ "max_stream_resolution": "2560x1440" });
    config.update(saved.as_object().unwrap()).unwrap();
    assert_eq!(config.max_stream_resolution(), Some((2560, 1440)));
    // Clearing it goes back to native, as a reset does.
    let reset = serde_json::json!({ "max_stream_resolution": "" });
    config.update(reset.as_object().unwrap()).unwrap();
    assert_eq!(config.max_stream_resolution(), None);
    // The console refuses text that is neither native nor a size it could use.
    for refused in ["1440p", "2560x", "100x100", "8000x2160"] {
        let bad = serde_json::json!({ "max_stream_resolution": refused });
        assert!(
            config.update(bad.as_object().unwrap()).is_err(),
            "{refused}"
        );
    }
    let native = serde_json::json!({ "max_stream_resolution": "native" });
    config.update(native.as_object().unwrap()).unwrap();
    assert_eq!(config.max_stream_resolution(), None);
}

#[test]
fn audio_passthrough_is_on_unless_turned_off() {
    assert!(super::Config::default().audio_passthrough());
    for (text, on) in [
        ("audio_passthrough = true\n", true),
        ("audio_passthrough = disabled\n", false),
        ("audio_passthrough = 0\n", false),
    ] {
        assert_eq!(
            super::Config::parse(text).unwrap().audio_passthrough(),
            on,
            "{text}"
        );
    }
}

#[test]
fn client_roaming_is_on_unless_turned_off() {
    assert!(super::Config::default().client_roaming());
    for (text, on) in [
        ("client_roaming = true\n", true),
        ("client_roaming = off\n", false),
        ("client_roaming = 0\n", false),
    ] {
        assert_eq!(
            super::Config::parse(text).unwrap().client_roaming(),
            on,
            "{text}"
        );
    }
}

#[test]
fn hdr_dynamic_metadata_is_on_unless_turned_off() {
    assert!(super::Config::default().hdr_dynamic_metadata());
    for (text, on) in [
        ("hdr_dynamic_metadata = true\n", true),
        ("hdr_dynamic_metadata = disabled\n", false),
        ("hdr_dynamic_metadata = 0\n", false),
    ] {
        assert_eq!(
            super::Config::parse(text).unwrap().hdr_dynamic_metadata(),
            on,
            "{text}"
        );
    }
}

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
fn the_keyboard_follows_text_fields_without_their_text_by_default() {
    let c = super::Config::default();
    assert!(c.keyboard_follows_focus() && !c.text_input_surrounding_text());
    let c = super::Config::parse(
        "keyboard_follows_focus = disabled\ntext_input_surrounding_text = enabled\n",
    )
    .unwrap();
    assert!(!c.keyboard_follows_focus() && c.text_input_surrounding_text());
    for key in [
        "keyboard_follows_focus",
        "text_input_surrounding_text",
        "keyboard_layout",
    ] {
        assert!(super::override_allowed(key), "{key}");
    }
}

#[test]
fn the_update_check_time_is_a_time_of_day_and_unset_otherwise() {
    assert_eq!(super::Config::default().update_check_time(), None);
    for (text, seconds) in [
        ("03:00", Some(3 * 3600)),
        ("3:00", Some(3 * 3600)),
        ("00:00", Some(0)),
        ("23:59", Some(23 * 3600 + 59 * 60)),
        (" 05:30 ", Some(5 * 3600 + 30 * 60)),
        // As Vibepollo writes values, in quotes.
        ("\"05:30\"", Some(5 * 3600 + 30 * 60)),
        // Unset, and text that is not a time: the interval applies.
        ("", None),
        ("24:00", None),
        ("12:60", None),
        ("3am", None),
        ("3", None),
        ("03:0", None),
        ("03:000", None),
        ("003:00", None),
        ("03:00:00", None),
        ("-3:00", None),
        ("+3:00", None),
        ("3.5", None),
        (":30", None),
        ("10800", None),
    ] {
        let c = super::Config::parse(&format!("update_check_time = {text}\n")).unwrap();
        assert_eq!(c.update_check_time(), seconds, "{text:?}");
        assert_eq!(super::parse_time_of_day(text), seconds, "{text:?}");
    }
    // A malformed time leaves the interval as it is set.
    let c =
        super::Config::parse("update_check_time = soon\nupdate_check_interval = 7200\n").unwrap();
    assert_eq!(c.update_check_time(), None);
    assert_eq!(c.integer("update_check_interval", 86400), 7200);
}

#[test]
fn frame_warp_is_on_unless_turned_off() {
    assert!(super::Config::default().frame_warp());
    for (text, on) in [
        ("frame_warp = true\n", true),
        ("frame_warp = disabled\n", false),
        ("frame_warp = 0\n", false),
    ] {
        assert_eq!(
            super::Config::parse(text).unwrap().frame_warp(),
            on,
            "{text}"
        );
    }
    assert!(super::override_allowed("frame_warp"));
}

#[test]
fn the_bandwidth_probe_is_on_unless_turned_off() {
    assert!(super::Config::default().bandwidth_probe());
    for (text, on) in [
        ("bandwidth_probe = true\n", true),
        ("bandwidth_probe = disabled\n", false),
        ("bandwidth_probe = 0\n", false),
    ] {
        assert_eq!(
            super::Config::parse(text).unwrap().bandwidth_probe(),
            on,
            "{text}"
        );
    }
}

#[test]
fn keyboard_layout_takes_client_us_or_a_klid() {
    use super::KeyboardLayoutSetting::{Client, Klid, Us};
    let layout = |text: &str| super::Config::parse(text).unwrap().keyboard_layout();
    assert_eq!(super::Config::default().keyboard_layout(), Client);
    for (text, expected) in [
        ("keyboard_layout = client", Client),
        ("keyboard_layout = us", Us),
        ("keyboard_layout = 00000407", Klid(0x407)),
        ("keyboard_layout = \"0001041f\"", Klid(0x1041F)),
        // What the host can't use is reported and the device's layout applies.
        ("keyboard_layout = de", Client),
        ("keyboard_layout = 0000040", Client),
        ("keyboard_layout = 0000040G", Client),
        ("keyboard_layout = 000004070", Client),
    ] {
        assert_eq!(layout(text), expected, "{text}");
    }
}

#[test]
fn the_webrtc_server_is_off_on_8000_and_8001_and_refuses_bad_ports() {
    let config = super::Config::default();
    assert!(!config.webrtc_server());
    assert_eq!(
        (config.webrtc_port(), config.webrtc_media_port()),
        (8000, 8001)
    );
    let mut config = super::Config::default();
    let saved = serde_json::json!({"webrtc_server": true, "webrtc_port": 9000, "webrtc_media_port": "9001"});
    config.update(saved.as_object().unwrap()).unwrap();
    assert!(config.webrtc_server());
    assert_eq!(
        (config.webrtc_port(), config.webrtc_media_port()),
        (9000, 9001)
    );
    for bad in [
        serde_json::json!(80),
        serde_json::json!(70000),
        serde_json::json!("x"),
    ] {
        for key in ["webrtc_port", "webrtc_media_port", "dsu_server_port"] {
            let saved = serde_json::json!({ key: bad });
            assert!(
                config.update(saved.as_object().unwrap()).is_err(),
                "{key} {bad}"
            );
        }
    }
    // A hand-edited file with a bad port falls back loudly to the default.
    let config = super::Config::parse(
        "webrtc_port = 80
webrtc_media_port = 99999
",
    )
    .unwrap();
    assert_eq!(
        (config.webrtc_port(), config.webrtc_media_port()),
        (8000, 8001)
    );
}
#[test]
fn direct_driver_push_defaults_to_the_fence_ring_unless_turned_off() {
    let fence = |text: &str| super::Config::parse(text).unwrap().idd_push_fence_ring();
    assert!(fence(""));
    assert!(fence("port = 47989"));
    assert!(fence("idd_push_fence_ring = true"));
    assert!(!fence("idd_push_fence_ring = false"));
    assert!(!fence("idd_push_fence_ring = off"));
    assert!(super::Config::default().idd_push_fence_ring());
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
fn packet_pacing_reads_automatic_always_off_and_a_fixed_rate() {
    use super::PacketPacing as P;
    for (text, expected) in [
        ("", P::Auto),
        ("packet_pacing=auto", P::Auto),
        ("packet_pacing=AUTO", P::Auto),
        ("packet_pacing=always", P::Always),
        ("packet_pacing=off", P::Off),
        ("packet_pacing=1", P::Fixed(1)),
        ("packet_pacing=80000", P::Fixed(80000)),
        ("packet_pacing=0x1388", P::Fixed(5000)),
        // The earlier pacing limit read 0 and negative rates as no rate.
        ("packet_pacing=0", P::Auto),
        ("packet_pacing=-1", P::Auto),
        ("packet_pacing=invalid", P::Auto),
    ] {
        let config = super::Config::parse(text).unwrap();
        assert_eq!(config.packet_pacing(), expected, "{text}");
    }
    assert_eq!(super::Config::default().manual_pacing_kbps(), None);
    assert_eq!(
        super::Config::parse("packet_pacing=80000")
            .unwrap()
            .manual_pacing_kbps(),
        Some(80000)
    );
    assert!(super::override_allowed("packet_pacing"));
    assert!(!super::override_allowed("host_adaptive_pacing"));
    assert!(!super::override_allowed("wireless_adaptive_pacing"));
    assert!(!super::override_allowed("pacing_max_bitrate_kbps"));
}

#[test]
fn packet_pacing_picks_the_adaptive_controller_like_the_retired_switches_did() {
    let cases = [
        ("", [false, false, false, false]),
        ("packet_pacing=auto", [false, false, false, false]),
        ("packet_pacing=always", [true, true, true, true]),
        ("packet_pacing=off", [false, false, false, false]),
        ("packet_pacing=80000", [false, false, false, false]),
    ];
    for (text, expected) in cases {
        let config = super::Config::parse(text).unwrap();
        for (codec, enabled) in expected.into_iter().enumerate() {
            assert_eq!(
                config.adaptive_packet_pacing(codec as u8, false),
                enabled,
                "codec {codec}: {text}"
            );
        }
        assert!(!config.adaptive_packet_pacing(u8::MAX, false));
        assert!(!config.adaptive_packet_pacing(u8::MAX, true));
    }
}

#[test]
fn a_wireless_client_gets_adaptive_pacing_on_h26x_and_av1_unless_something_owns_pacing() {
    // (config, [H.264, HEVC, AV1, PyroWave] adaptive, keyframe-loss pacing, same for wired)
    let on = [true, true, true, false];
    let off = [false; 4];
    let all = [true; 4];
    let cases = [
        ("", on, [false, false, false, true], off, [true; 4]),
        (
            "packet_pacing=auto",
            on,
            [false, false, false, true],
            off,
            [true; 4],
        ),
        (
            "packet_pacing=invalid",
            on,
            [false, false, false, true],
            off,
            [true; 4],
        ),
        ("packet_pacing=off", off, [true; 4], off, [true; 4]),
        // Always still turns it on for everything, wired or wireless.
        ("packet_pacing=always", all, [false; 4], all, [false; 4]),
        // A fixed rate owns pacing.
        ("packet_pacing=80000", off, [false; 4], off, [false; 4]),
        (
            "packet_pacing=0",
            on,
            [false, false, false, true],
            off,
            [true; 4],
        ),
        // Keyframe-loss pacing can still be switched off.
        (
            "keyframe_loss_pacing=false",
            on,
            [false; 4],
            off,
            [false; 4],
        ),
    ];
    for (text, wireless, wireless_keyframe, wired, wired_keyframe) in cases {
        let config = super::Config::parse(text).unwrap();
        for codec in 0..4u8 {
            let i = usize::from(codec);
            assert_eq!(
                config.adaptive_packet_pacing(codec, true),
                wireless[i],
                "wireless codec {codec}: {text}"
            );
            assert_eq!(
                config.keyframe_loss_pacing(codec, true),
                wireless_keyframe[i],
                "wireless keyframe codec {codec}: {text}"
            );
            assert_eq!(
                config.adaptive_packet_pacing(codec, false),
                wired[i],
                "wired codec {codec}: {text}"
            );
            assert_eq!(
                config.keyframe_loss_pacing(codec, false),
                wired_keyframe[i],
                "wired keyframe codec {codec}: {text}"
            );
        }
    }
    // PyroWave plus a wireless client stays on its calibrated pace.
    let default = super::Config::default();
    assert!(!default.adaptive_packet_pacing(3, true));
    assert!(default.keyframe_loss_pacing(3, true));
}

#[test]
fn disconnected_displays_restore_by_default_and_exclusive_layouts_cannot_be_retained() {
    use std::time::Duration;
    for (text, ordinary, exclusive) in [
        ("", Some(3000), Some(3000)),
        ("dd_disconnect_behavior=restore", Some(3000), Some(3000)),
        ("dd_disconnect_behavior=invalid", Some(3000), Some(3000)),
        (
            "dd_disconnect_behavior=keep_while_paused",
            Some(7_200_000),
            Some(3000),
        ),
        (
            "dd_disconnect_behavior=keep_while_paused\ndd_paused_virtual_display_timeout_secs=0",
            None,
            Some(3000),
        ),
        (
            "dd_disconnect_behavior=keep_while_paused\ndd_paused_virtual_display_timeout_secs=20\ndd_config_revert_delay=500",
            Some(20_000),
            Some(500),
        ),
        ("dd_config_revert_delay=0", Some(0), Some(0)),
        ("dd_config_revert_delay=-1", Some(0), Some(0)),
        // Issue #92: keeping the monitors off while paused is the user's choice.
        (
            "dd_hold_layout_while_paused=true\ndd_disconnect_behavior=keep_while_paused\ndd_paused_virtual_display_timeout_secs=0",
            None,
            None,
        ),
        (
            "dd_hold_layout_while_paused=true\ndd_disconnect_behavior=keep_while_paused\ndd_paused_virtual_display_timeout_secs=60",
            Some(60_000),
            Some(60_000),
        ),
        // Restoring on disconnect still ends it after the restore delay.
        ("dd_hold_layout_while_paused=true", Some(3000), Some(3000)),
        // A value saved under the new name wins over the earlier one.
        (
            "dd_disconnect_behavior=restore\ndd_config_revert_on_disconnect=false",
            Some(3000),
            Some(3000),
        ),
    ] {
        let config = super::Config::parse(text).unwrap();
        assert_eq!(
            config.display_disconnect_delay(false),
            ordinary.map(Duration::from_millis),
            "{text}"
        );
        assert_eq!(
            config.display_disconnect_delay(true),
            exclusive.map(Duration::from_millis),
            "{text}"
        );
    }
}

#[test]
fn keyframe_loss_pacing_is_on_unless_a_fixed_rate_or_adaptation_owns_pacing() {
    for (text, enabled) in [
        ("", true),
        ("keyframe_loss_pacing=true", true),
        ("keyframe_loss_pacing=invalid", true),
        ("keyframe_loss_pacing=false", false),
        ("packet_pacing=0", true),
        ("packet_pacing=auto", true),
        ("packet_pacing=off", true),
        ("packet_pacing=80000", false),
        ("packet_pacing=always", false),
        // The retired host bitrate controller no longer turns it off.
        ("host_adaptive_bitrate=true", true),
    ] {
        let config = super::Config::parse(text).unwrap();
        assert_eq!(config.keyframe_loss_pacing(0, false), enabled, "{text}");
    }
    assert!(super::override_allowed("keyframe_loss_pacing"));
}

#[test]
fn link_rate_adaptation_is_on_unless_disabled() {
    for (text, enabled) in [
        ("", true),
        ("bitrate_adaptation=on", true),
        ("bitrate_adaptation=delay", true),
        ("bitrate_adaptation=off", false),
        ("bitrate_adaptation=\"off\"", false),
        ("bitrate_adaptation=invalid", true),
        ("link_rate_adaptation=true", true),
        ("link_rate_adaptation=invalid", true),
        ("link_rate_adaptation=false", false),
        // The retired host bitrate controller no longer turns it off.
        ("host_adaptive_bitrate=true", true),
        (
            "host_adaptive_bitrate=true\nlink_rate_adaptation=false",
            false,
        ),
        ("host_adaptive_bitrate=false", true),
        ("host_adaptive_pacing=true", true),
    ] {
        let config = super::Config::parse(text).unwrap();
        assert_eq!(config.link_rate_adaptation(), enabled, "{text}");
    }
    assert!(super::override_allowed("bitrate_adaptation"));
    assert!(!super::override_allowed("link_rate_adaptation"));
}

#[test]
fn congestion_control_is_off_unless_set() {
    for (text, enabled) in [
        ("", false),
        ("congestion_control=true", true),
        ("congestion_control=invalid", false),
        ("congestion_control=false", false),
        ("congestion_control=true\nhost_adaptive_bitrate=true", true),
        ("congestion_control=true\nlink_rate_adaptation=false", true),
        ("bitrate_adaptation=delay", true),
        ("bitrate_adaptation=on", false),
        ("bitrate_adaptation=off", false),
        // A value saved under the current name wins over the switches.
        ("bitrate_adaptation=on\ncongestion_control=true", false),
    ] {
        let config = super::Config::parse(text).unwrap();
        assert_eq!(config.congestion_control(), enabled, "{text}");
    }
    assert!(!super::override_allowed("congestion_control"));
}

/// The pacing rate and the two adaptive pacing switches fold into Packet
/// pacing: a positive rate is the rate, adaptive pacing for everything is
/// `always`, wireless pacing off alone is `off`, the rest the default.
#[test]
fn the_pacing_settings_fold_into_packet_pacing() {
    use super::{Config, PacketPacing as P, folded_packet_pacing as fold, migrate_overrides};
    assert_eq!(fold(None, None, None), None);
    for (rate, host, wireless, folded) in [
        (Some("80000"), None, None, Some("80000")),
        (Some("80000"), Some("true"), Some("false"), Some("80000")),
        (Some("0"), None, None, None),
        (Some("-1"), Some("true"), None, Some("always")),
        (None, Some("true"), None, Some("always")),
        (None, Some("true"), Some("false"), Some("always")),
        (None, Some("false"), Some("false"), Some("off")),
        (None, None, Some("false"), Some("off")),
        (None, None, Some("true"), None),
        (None, Some("false"), None, None),
        (None, Some("invalid"), Some("invalid"), None),
    ] {
        assert_eq!(
            fold(rate, host, wireless).as_deref(),
            folded,
            "{rate:?} {host:?} {wireless:?}"
        );
    }
    for (text, expected) in [
        ("", P::Auto),
        ("pacing_max_bitrate_kbps=120000", P::Fixed(120000)),
        ("host_adaptive_pacing=true", P::Always),
        ("wireless_adaptive_pacing=false", P::Off),
        (
            "host_adaptive_pacing=true
pacing_max_bitrate_kbps=80000",
            P::Fixed(80000),
        ),
        (
            "packet_pacing=off
host_adaptive_pacing=true",
            P::Off,
        ),
    ] {
        let config = Config::parse(text).unwrap();
        assert_eq!(config.packet_pacing(), expected, "{text}");
        for key in [
            "pacing_max_bitrate_kbps",
            "host_adaptive_pacing",
            "wireless_adaptive_pacing",
        ] {
            assert!(!config.text().contains(key), "{text}");
        }
    }
    // An app's or device's overrides.
    for (layer, expected) in [
        (
            serde_json::json!({"pacing_max_bitrate_kbps": 80000}),
            Some("80000"),
        ),
        (
            serde_json::json!({"host_adaptive_pacing": true}),
            Some("always"),
        ),
        (
            serde_json::json!({"wireless_adaptive_pacing": "false"}),
            Some("off"),
        ),
        (
            serde_json::json!({"host_adaptive_pacing": true, "packet_pacing": "off"}),
            Some("off"),
        ),
        (serde_json::json!({"wireless_adaptive_pacing": true}), None),
        (serde_json::json!({"fec_percentage": 20}), None),
    ] {
        let mut layer = layer.as_object().unwrap().clone();
        migrate_overrides(&mut layer);
        assert_eq!(
            layer.get("packet_pacing").and_then(|v| v.as_str()),
            expected,
            "{layer:?}"
        );
        assert!(!layer.contains_key("host_adaptive_pacing"));
        assert!(!layer.contains_key("wireless_adaptive_pacing"));
        assert!(!layer.contains_key("pacing_max_bitrate_kbps"));
    }
}

/// The two bitrate switches fold into Adaptive bitrate once, from what the
/// configuration file, a console save or an app's or device's overrides
/// held: congestion control on is `delay`, link-rate adaptation off alone is
/// `off`, the rest `on`; the switches are then gone.
#[test]
fn the_bitrate_switches_fold_into_adaptive_bitrate() {
    use super::{BitrateAdaptation as A, Config, folded_bitrate_adaptation as fold};
    assert_eq!(fold(None, None), None);
    for (link, congestion, folded) in [
        (Some("true"), None, "on"),
        (Some("false"), None, "off"),
        (Some("invalid"), None, "on"),
        (None, Some("false"), "on"),
        (None, Some("true"), "delay"),
        (Some("true"), Some("false"), "on"),
        (Some("true"), Some("true"), "delay"),
        (Some("false"), Some("false"), "off"),
        (Some("false"), Some("true"), "delay"),
        (Some("\"false\""), Some("\"no\""), "off"),
    ] {
        assert_eq!(
            fold(link, congestion),
            Some(folded),
            "{link:?} {congestion:?}"
        );
    }
    // The configuration file.
    for (text, expected) in [
        ("", A::On),
        ("link_rate_adaptation=false", A::Off),
        (
            "link_rate_adaptation=false\ncongestion_control=false",
            A::Off,
        ),
        ("congestion_control=true", A::Delay),
        (
            "link_rate_adaptation=false\ncongestion_control=true",
            A::Delay,
        ),
        ("link_rate_adaptation=true\ncongestion_control=false", A::On),
        ("bitrate_adaptation=off\nlink_rate_adaptation=true", A::Off),
    ] {
        let config = Config::parse(text).unwrap();
        assert_eq!(config.bitrate_adaptation(), expected, "{text}");
        assert!(!config.text().contains("link_rate_adaptation"), "{text}");
        assert!(!config.text().contains("congestion_control"), "{text}");
    }
    // A save sent with the earlier names.
    let mut config = Config::default();
    let sent = serde_json::json!({"link_rate_adaptation": false});
    config.update(sent.as_object().unwrap()).unwrap();
    assert_eq!(config.bitrate_adaptation(), A::Off);
    assert!(!config.text().contains("link_rate_adaptation"));
    let sent = serde_json::json!({"congestion_control": "true", "link_rate_adaptation": true});
    config.update(sent.as_object().unwrap()).unwrap();
    assert_eq!(config.bitrate_adaptation(), A::Delay);
    let sent = serde_json::json!({"congestion_control": true, "bitrate_adaptation": "off"});
    config.update(sent.as_object().unwrap()).unwrap();
    assert_eq!(config.bitrate_adaptation(), A::Off);
    // An app's or device's overrides.
    for (layer, expected) in [
        (
            serde_json::json!({"link_rate_adaptation": false}),
            Some("off"),
        ),
        (
            serde_json::json!({"link_rate_adaptation": "false", "congestion_control": "false"}),
            Some("off"),
        ),
        (
            serde_json::json!({"congestion_control": true}),
            Some("delay"),
        ),
        (
            serde_json::json!({"link_rate_adaptation": true}),
            Some("on"),
        ),
        (
            serde_json::json!({"congestion_control": true, "bitrate_adaptation": "on"}),
            Some("on"),
        ),
        (serde_json::json!({"fec_percentage": 20}), None),
    ] {
        let mut layer = layer.as_object().unwrap().clone();
        super::migrate_overrides(&mut layer);
        assert_eq!(
            layer
                .get("bitrate_adaptation")
                .and_then(serde_json::Value::as_str),
            expected,
            "{layer:?}"
        );
        assert!(!layer.contains_key("link_rate_adaptation"));
        assert!(!layer.contains_key("congestion_control"));
    }
    assert!(super::retired_name("link_rate_adaptation"));
    assert!(super::retired_name("congestion_control"));
}

#[test]
fn resetting_packet_pacing_restores_the_default() {
    let mut config = super::Config::default();
    assert!(!config.adaptive_packet_pacing(0, false));
    for reset in [serde_json::Value::Null, serde_json::json!("")] {
        config
            .update(
                serde_json::json!({"packet_pacing": "always"})
                    .as_object()
                    .unwrap(),
            )
            .unwrap();
        assert!(config.adaptive_packet_pacing(0, false));
        config
            .update(
                serde_json::json!({"packet_pacing": reset})
                    .as_object()
                    .unwrap(),
            )
            .unwrap();
        assert!(!config.adaptive_packet_pacing(0, false));
        assert!(!config.values.contains_key("packet_pacing"));
    }
    // The earlier names fold into Packet pacing when an older console or a
    // script sends them.
    config
        .update(
            serde_json::json!({"host_adaptive_pacing": true})
                .as_object()
                .unwrap(),
        )
        .unwrap();
    assert!(config.adaptive_packet_pacing(0, false));
    assert_eq!(config.get("packet_pacing", ""), "always");
    assert!(!config.values.contains_key("host_adaptive_pacing"));
}

#[test]
fn hardware_metrics_defaults_and_file_interval_bounds() {
    let defaults = super::Config::default();
    assert!(defaults.hardware_metrics());
    assert!(defaults.packed_444());
    assert_eq!(defaults.hardware_metrics_interval(), 1000);
    assert!(defaults.list("hardware_metrics_selection").is_empty());
    for (value, expected) in [
        ("invalid", 1000),
        ("0", 500),
        ("750", 750),
        ("20000", 10000),
    ] {
        let config = super::Config::parse(&format!("hardware_metrics_interval={value}\n")).unwrap();
        assert_eq!(config.hardware_metrics_interval(), expected);
    }
}
#[test]
fn stream_preset_saves_only_a_preset_name_and_clears_with_null() {
    let mut config = super::Config::default();
    for value in ["lowest_latency", "balanced", "reference", "Reference"] {
        change(&mut config, "stream_preset", value).unwrap();
        assert_eq!(config.get("stream_preset", ""), value);
    }
    for value in ["custom", "fastest", "lowest", "5"] {
        assert!(
            change(&mut config, "stream_preset", value).is_err(),
            "{value}"
        );
        assert_eq!(config.get("stream_preset", ""), "Reference");
    }
    let mut object = serde_json::Map::new();
    object.insert("stream_preset".into(), serde_json::Value::Null);
    config.update(&object).unwrap();
    assert!(!config.values.contains_key("stream_preset"));
    // A file the host did not write may hold anything; it still loads.
    let loaded = super::Config::parse(
        "stream_preset = fastest
",
    )
    .unwrap();
    assert_eq!(loaded.get("stream_preset", ""), "fastest");
}
#[test]
fn background_mode_saves_only_nucleus_and_earlier_modes_become_it() {
    let mut config = super::Config::default();
    // A client or an old console sending an earlier mode saves nucleus.
    for value in ["auto", "child_session", "virtual_display", "nucleus", ""] {
        change(&mut config, "background_mode", value).unwrap();
        let saved = if value.is_empty() { "" } else { "nucleus" };
        assert_eq!(config.get("background_mode", ""), saved, "{value}");
    }
    let error = format!(
        "{:#}",
        change(&mut config, "background_mode", "rdp").unwrap_err()
    );
    assert!(
            error.contains(
                "background_mode = rdp is not a mode: Nucleus Co-op is the only background mode (nucleus)"
            ),
            "{error}"
        );
    assert_eq!(config.get("background_mode", ""), "");
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
fn audio_haptics_settings_are_checked_on_save_and_override_per_device() {
    use crate::audio_haptics::{CURVE, ENABLED, GATE, SETTINGS, STRENGTH};
    let mut config = super::Config::default();
    for (key, value) in [
        (ENABLED, "true"),
        (STRENGTH, "0"),
        (STRENGTH, "200"),
        (CURVE, "soft"),
        (GATE, "-80"),
        (GATE, "-10"),
    ] {
        change(&mut config, key, value).unwrap();
    }
    for (key, value) in [
        (ENABLED, "sometimes"),
        (STRENGTH, "250"),
        (CURVE, "wobbly"),
        (GATE, "0"),
    ] {
        let error = change(&mut config, key, value).unwrap_err();
        assert!(error.to_string().contains(key), "{key}={value}: {error}");
    }
    let mut object = serde_json::Map::new();
    object.insert(STRENGTH.into(), 300.into());
    assert!(config.update(&object).is_err());
    object.insert(STRENGTH.into(), serde_json::Value::Null);
    config.update(&object).unwrap();
    assert!(!config.values.contains_key(STRENGTH));
    for key in SETTINGS {
        assert!(super::override_allowed(key), "{key}");
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
fn couch_menu_controls_are_checked_on_save_with_one_message_each() {
    let mut config = super::Config::default();
    for (key, value) in [
        ("couch_menu_controller_chord", "l1+r1+y"),
        ("couch_menu_controller_chord", "guide"),
        ("couch_menu_hold_ms", "300"),
        ("couch_menu_hold_ms", "3000"),
        ("couch_menu_corner_hold_ms", "500"),
        ("couch_menu_corner_hold_ms", "3000"),
        ("couch_menu_keyboard_chord", "ctrl+alt+insert"),
        ("couch_menu_open_with", "controller,touch"),
    ] {
        change(&mut config, key, value).unwrap();
        assert_eq!(config.get(key, ""), value);
    }
    for (key, value, message) in [
        (
            "couch_menu_controller_chord",
            "start+select+l1+r1",
            "quits the stream",
        ),
        ("couch_menu_controller_chord", "start", "mouse mode"),
        ("couch_menu_controller_chord", "start+select", "into Guide"),
        (
            "couch_menu_controller_chord",
            "select+l1+r1+x",
            "statistics",
        ),
        ("couch_menu_controller_chord", "a+start", "mouse mode"),
        (
            "couch_menu_controller_chord",
            "l1+r1+a",
            "A presses the focused row",
        ),
        ("couch_menu_controller_chord", "b", "B goes back"),
        ("couch_menu_controller_chord", "x+up", "Up moves the focus"),
        ("couch_menu_controller_chord", "l3+r2", "unknown button"),
        ("couch_menu_hold_ms", "299", "between 300 and 3000"),
        ("couch_menu_hold_ms", "3001", "between 300 and 3000"),
        ("couch_menu_corner_hold_ms", "499", "between 500 and 3000"),
        ("couch_menu_corner_hold_ms", "3001", "between 500 and 3000"),
        (
            "couch_menu_keyboard_chord",
            "ctrl+alt+shift+home",
            "Ctrl+Alt+Shift",
        ),
        ("couch_menu_keyboard_chord", "ctrl+alt+del", "unknown chord"),
        ("couch_menu_open_with", "pad", "unknown entry"),
    ] {
        let error = change(&mut config, key, value).unwrap_err().to_string();
        assert!(error.starts_with(key), "{key}={value}: {error}");
        assert!(error.contains(message), "{key}={value}: {error}");
    }
    // Nothing refused was kept.
    assert_eq!(config.get("couch_menu_controller_chord", ""), "guide");
    assert_eq!(config.integer("couch_menu_hold_ms", 0), 3000);
    assert_eq!(config.integer("couch_menu_corner_hold_ms", 0), 3000);
    assert_eq!(
        config.get("couch_menu_keyboard_chord", ""),
        "ctrl+alt+insert"
    );
    // A JSON number from the console is checked the same way.
    let mut object = serde_json::Map::new();
    object.insert("couch_menu_hold_ms".into(), 299.into());
    assert!(config.update(&object).is_err());
    object.insert("couch_menu_hold_ms".into(), 750.into());
    config.update(&object).unwrap();
    let open_with = crate::couch_menu::chord::OpenWith::from_config(&config);
    assert_eq!(open_with.pad_hold, std::time::Duration::from_millis(750));
    assert!(!open_with.keyboard);
}
#[test]
fn a_device_couch_menu_override_beats_the_host_and_a_bad_one_is_refused() {
    use crate::couch_menu::chord::{KeyChord, OpenWith, PadButtons};
    for key in [
        "couch_menu_open_with",
        "couch_menu_controller_chord",
        "couch_menu_hold_ms",
        "couch_menu_corner_hold_ms",
        "couch_menu_keyboard_chord",
    ] {
        assert!(super::override_allowed(key), "{key}");
    }
    let mut config =
        super::Config::parse("couch_menu_controller_chord = l1+r1+y\ncouch_menu_hold_ms = 2000\n")
            .unwrap();
    let layer = |config: &mut super::Config, key: &str, value: serde_json::Value| {
        config.update(&serde_json::Map::from_iter([(key.to_owned(), value)]))
    };
    layer(&mut config, "couch_menu_controller_chord", "select".into()).unwrap();
    layer(&mut config, "couch_menu_hold_ms", 400.into()).unwrap();
    layer(
        &mut config,
        "couch_menu_keyboard_chord",
        "ctrl+alt+end".into(),
    )
    .unwrap();
    layer(&mut config, "couch_menu_open_with", "controller".into()).unwrap();
    let open_with = OpenWith::from_config(&config);
    assert!(open_with.controller && !open_with.keyboard && !open_with.touch);
    assert_eq!(open_with.pad_chord, PadButtons::parse("select").unwrap());
    assert_eq!(open_with.pad_hold, std::time::Duration::from_millis(400));
    assert_eq!(
        open_with.key_chord,
        KeyChord::parse("ctrl+alt+end").unwrap()
    );
    assert!(layer(&mut config, "couch_menu_controller_chord", "start".into()).is_err());
    assert!(layer(&mut config, "couch_menu_corner_hold_ms", 100.into()).is_err());
    assert_eq!(config.get("couch_menu_controller_chord", ""), "select");
    assert!(!config.values.contains_key("couch_menu_corner_hold_ms"));
}
use super::*;
use std::collections::BTreeMap;
/// A change saved to sunshine.conf, as the settings API does it.
fn change(config: &mut Config, key: &str, value: &str) -> Result<()> {
    let mut object = serde_json::Map::new();
    object.insert(key.into(), value.into());
    let mut next = config.clone();
    next.update(&object)?;
    next.check_saved(object.keys())?;
    *config = next;
    Ok(())
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
#[test]
fn display_policies_keep_resolution_and_refresh_independent() {
    let config =
        Config::parse("dd_resolution_option = disabled\ndd_refresh_rate_option = auto\n").unwrap();
    let requested = config.display_request(1920, 1080, 120).unwrap();
    assert_eq!(requested.resolution, None);
    assert_eq!(requested.refresh, Some(120));
    let config = Config::parse("dd_resolution_option = manual\ndd_manual_resolution = 2560x1440\ndd_refresh_rate_option = disabled\n").unwrap();
    let requested = config.display_request(1920, 1080, 120).unwrap();
    assert_eq!(requested.resolution, Some((2560, 1440)));
    assert_eq!(requested.refresh, None);
    for value in ["NaN", "inf", "0", "1001"] {
        let config = Config::parse(&format!(
            "dd_refresh_rate_option = manual\ndd_manual_refresh_rate = {value}\n"
        ))
        .unwrap();
        assert!(config.display_request(1920, 1080, 60).is_err());
    }
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
/// Every setting the NVENC encoder reads (nvenc.rs, and its GPU priority in
/// gpu_priority.rs) stays in the configuration, and an app or a device may
/// override it as the AMF ones.
#[test]
fn nvenc_settings_are_kept_and_overridable() {
    let source = [
        include_str!("../nvenc.rs"),
        include_str!("../../../windows/src/gpu_priority.rs"),
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
    assert_eq!(one.boolean("library_auto_sync", true), true);
    assert_eq!(Config::parse(&one.text()).unwrap().values, one.values);
    // The recent-games options fold over the launchers that keep a play
    // history, and a value already saved under the shared name wins.
    let mixed = Config::parse(
        "battlenet_sync_all_installed = false\nbattlenet_recent_games = 3\nlibrary_recent_max_age_days = 0\nsteam_recent_max_age_days = 30\nplaynite_recent_max_age_days = 30\n",
    )
    .unwrap();
    assert_eq!(mixed.boolean("library_sync_all_installed", true), true);
    assert_eq!(mixed.get("battlenet_sync_all_installed", ""), "false");
    assert_eq!(mixed.integer("library_recent_games", 0), 10);
    assert_eq!(mixed.integer("library_recent_max_age_days", 0), 0);
    assert_eq!(mixed.integer("steam_recent_max_age_days", 0), 30);
    assert_eq!(mixed.integer("playnite_recent_max_age_days", 0), 30);
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
fn rtx_hdr_keeps_the_display_sdr_for_an_hdr_stream_only_where_it_runs() {
    let mut c = Config::parse(
        "rtx_hdr=true
",
    )
    .unwrap();
    c.values
        .insert(crate::nvidia_enhance::marker("rtx_hdr"), "true".into());
    let hdr = |c: &Config, stream_hdr| {
        c.display_request_rate(1920, 1080, crate::framegen::Rate(60000), stream_hdr, false)
            .unwrap()
            .hdr
    };
    assert_eq!(hdr(&c, true), Some(true));
    c.values.insert(
        crate::nvidia_enhance::OFFERED_KEY.into(),
        crate::nvidia_enhance::ALL.to_string(),
    );
    assert_eq!(hdr(&c, true), Some(false));
    assert_eq!(hdr(&c, false), Some(false));
}
#[test]
fn exact_display_remapping_hdr_override_and_disabled_policy() {
    let c = Config::parse("dd_mode_remapping={\"mixed\":[{\"requested_resolution\":\"1920x1080\",\"requested_fps\":\"59.94\",\"final_resolution\":\"2560x1440\",\"final_refresh_rate\":\"119.88\"}]}\ndd_hdr_request_override=force_off\n").unwrap();
    let r = c
        .display_request_rate(1920, 1080, crate::framegen::Rate(59940), true, false)
        .unwrap();
    assert_eq!(r.resolution, Some((2560, 1440)));
    assert_eq!(r.refresh, Some(119880));
    assert_eq!(r.hdr, Some(false));
    let c = Config::parse("dd_configuration_option=disabled\ndd_refresh_rate_option=manual\ndd_manual_refresh_rate=119.88\n").unwrap();
    let r = c
        .display_request_rate(1920, 1080, crate::framegen::Rate(59940), true, false)
        .unwrap();
    assert_eq!((r.resolution, r.refresh, r.hdr), (None, None, None));
    assert_eq!(
        c.display_request_rate(1920, 1080, crate::framegen::Rate(59940), true, true)
            .unwrap()
            .refresh,
        Some(119880)
    );
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

#[test]
fn the_dsu_server_is_off_by_default_and_takes_foundations_port() {
    assert_eq!(super::Config::parse("").unwrap().dsu_server_port(), None);
    let on = super::Config::parse("enable_dsu_server = true\n").unwrap();
    assert_eq!(on.dsu_server_port(), Some(26760));
    let moved =
        super::Config::parse("enable_dsu_server = true\ndsu_server_port = 26761\n").unwrap();
    assert_eq!(moved.dsu_server_port(), Some(26761));
    let invalid = super::Config::parse("enable_dsu_server = true\ndsu_server_port = 80\n").unwrap();
    assert_eq!(invalid.dsu_server_port(), Some(26760));
    let off = super::Config::parse("enable_dsu_server = false\ndsu_server_port = 26761\n").unwrap();
    assert_eq!(off.dsu_server_port(), None);
    // The server is this PC's unless LAN access is asked for.
    assert!(!on.dsu_server_lan());
    let lan = super::Config::parse("enable_dsu_server = true\ndsu_server_lan = true\n").unwrap();
    assert!(lan.dsu_server_lan());
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
    assert_eq!(
        crate::framegen::virtual_refresh(&c),
        crate::framegen::VirtualRefresh::Enabled
    );
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
    assert_eq!(
        crate::framegen::virtual_refresh(&config),
        crate::framegen::VirtualRefresh::Disabled
    );
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
/// The HDR request folds into Display HDR, one choice of four: Match the
/// stream, Always on, Always off, Don't change HDR.
#[test]
fn the_hdr_request_folds_into_display_hdr() {
    let hdr = |c: &Config, stream: bool| {
        c.display_request_rate(1920, 1080, crate::framegen::Rate(60000), stream, false)
            .unwrap()
            .hdr
    };
    // (file, Display HDR read from it, display HDR for an HDR and an SDR stream)
    for (text, option, for_hdr, for_sdr) in [
        ("", None, Some(true), Some(false)),
        (
            "dd_hdr_option = auto\n",
            Some("auto"),
            Some(true),
            Some(false),
        ),
        (
            "dd_hdr_request_override = force_on\n",
            Some("force_on"),
            Some(true),
            Some(true),
        ),
        (
            "dd_hdr_option = auto\ndd_hdr_request_override = force_off\n",
            Some("force_off"),
            Some(false),
            Some(false),
        ),
        // Don't change HDR still wins over the request, as it did.
        (
            "dd_hdr_option = disabled\ndd_hdr_request_override = force_on\n",
            Some("disabled"),
            None,
            None,
        ),
        (
            "dd_hdr_request_override = auto\n",
            None,
            Some(true),
            Some(false),
        ),
        (
            "dd_hdr_option = force_on\n",
            Some("force_on"),
            Some(true),
            Some(true),
        ),
        // Always on or off is a later choice than the request beside it.
        (
            "dd_hdr_option = force_on\ndd_hdr_request_override = force_off\n",
            Some("force_on"),
            Some(true),
            Some(true),
        ),
    ] {
        let c = Config::parse(text).unwrap();
        assert_eq!(
            c.values.get(HDR_OPTION).map(String::as_str),
            option,
            "{text}"
        );
        assert!(!c.values.contains_key(HDR_REQUEST), "{text}");
        assert_eq!(
            (hdr(&c, true), hdr(&c, false)),
            (for_hdr, for_sdr),
            "{text}"
        );
    }
    // Sent alone by an older console, it folds into Display HDR as held.
    let mut c = Config::default();
    change(&mut c, HDR_REQUEST, "force_off").unwrap();
    assert_eq!(c.get(HDR_OPTION, ""), "force_off");
    change(&mut c, HDR_REQUEST, "auto").unwrap();
    assert_eq!(c.get(HDR_OPTION, ""), "auto");
    let mut c = Config::parse("dd_hdr_option = disabled\n").unwrap();
    change(&mut c, HDR_REQUEST, "force_on").unwrap();
    assert_eq!(c.get(HDR_OPTION, ""), "disabled");
    assert!(!c.values.contains_key(HDR_REQUEST));
    // An empty or null request is Follow the device, as its reset was.
    for reset in [serde_json::json!(""), serde_json::Value::Null] {
        let mut c = Config::parse("dd_hdr_option = force_off\n").unwrap();
        c.update(&serde_json::Map::from_iter([(
            HDR_REQUEST.to_owned(),
            reset.clone(),
        )]))
        .unwrap();
        assert_eq!(c.get(HDR_OPTION, ""), "auto", "{reset}");
    }
    // Sent with Display HDR, both are one layer.
    let mut c = Config::parse("dd_hdr_option = disabled\n").unwrap();
    c.update(
        serde_json::json!({"dd_hdr_option": "auto", "dd_hdr_request_override": "force_on"})
            .as_object()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(c.get(HDR_OPTION, ""), "force_on");
    // Display HDR is the override; the retired name is read only to fold it.
    assert!(!override_allowed(HDR_REQUEST) && override_allowed(HDR_OPTION));
    // One layer of overrides at a time: an app's, a device block's or a
    // paired device's. A request beside the layer's own Display HDR folds
    // into it; one alone depends on the layers below and stays as saved.
    for (before, after) in [
        (
            serde_json::json!({"dd_hdr_option": "auto", "dd_hdr_request_override": "force_on"}),
            serde_json::json!({"dd_hdr_option": "force_on"}),
        ),
        (
            serde_json::json!({"dd_hdr_option": "disabled", "dd_hdr_request_override": "force_off"}),
            serde_json::json!({"dd_hdr_option": "disabled"}),
        ),
        (
            serde_json::json!({"dd_hdr_option": "auto", "dd_hdr_request_override": "auto"}),
            serde_json::json!({"dd_hdr_option": "auto"}),
        ),
        (
            serde_json::json!({"dd_hdr_option": "force_off", "dd_hdr_request_override": "force_on"}),
            serde_json::json!({"dd_hdr_option": "force_off"}),
        ),
    ] {
        let mut layer = before.as_object().unwrap().clone();
        assert!(migrate_overrides(&mut layer));
        assert_eq!(serde_json::Value::Object(layer.clone()), after, "{before}");
        assert!(!migrate_overrides(&mut layer));
    }
    for alone in ["force_on", "force_off", "auto"] {
        let mut layer = serde_json::json!({"dd_hdr_request_override": alone})
            .as_object()
            .unwrap()
            .clone();
        assert!(!migrate_overrides(&mut layer), "{alone}");
        assert_eq!(layer[HDR_REQUEST], alone);
    }
    // An app saved with a device's request alone is accepted as it stands.
    let extra = std::collections::BTreeMap::from([(
        crate::device_overrides::SETTING.to_owned(),
        serde_json::json!({"tv": {"config-overrides": {"dd_hdr_request_override": "force_on"}}}),
    )]);
    crate::device_overrides::check(&extra).unwrap();
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

#[test]
fn the_remote_monitor_switches_fold_into_remove_remote_monitor() {
    use super::{Config, RemoveRemoteMonitor as R, folded_remove_remote_monitor as fold};
    assert_eq!(fold(None, None), None);
    for (end, disconnect, folded) in [
        (Some("true"), None, "stream_end"),
        (Some("false"), None, "never"),
        (None, Some("true"), "disconnect"),
        (Some("true"), Some("true"), "disconnect"),
        (Some("true"), Some("false"), "stream_end"),
        (Some("invalid"), None, "never"),
        (Some("\"true\""), Some("\"no\""), "stream_end"),
    ] {
        assert_eq!(
            fold(end, disconnect),
            Some(folded),
            "{end:?} {disconnect:?}"
        );
    }
    for (text, expected) in [
        ("", R::Never),
        ("remote_monitor_disconnect_on_stream_end=true", R::StreamEnd),
        (
            "remote_monitor_disconnect_on_client_disconnect=true",
            R::Disconnect,
        ),
        (
            "remote_monitor_disconnect_on_stream_end=true
remote_monitor_disconnect_on_client_disconnect=true",
            R::Disconnect,
        ),
        (
            "remove_remote_monitor=stream_end
remote_monitor_disconnect_on_client_disconnect=true",
            R::StreamEnd,
        ),
    ] {
        let config = Config::parse(text).unwrap();
        assert_eq!(config.remove_remote_monitor(), expected, "{text}");
        assert!(
            !config.text().contains("remote_monitor_disconnect_on"),
            "{text}"
        );
    }
    let mut config = Config::default();
    let sent = serde_json::json!({"remote_monitor_disconnect_on_stream_end": true});
    config.update(sent.as_object().unwrap()).unwrap();
    assert_eq!(config.remove_remote_monitor(), R::StreamEnd);
    assert!(super::retired_name(
        "remote_monitor_disconnect_on_stream_end"
    ));
    assert!(super::retired_name(
        "remote_monitor_disconnect_on_client_disconnect"
    ));
}

// Every earlier combination of Restore displays on disconnect and Keep monitors
// off while paused, saved in the file, behaves exactly as before: how long an
// ordinary and an exclusive layout keep a disconnected display.
#[test]
fn every_earlier_disconnect_combination_keeps_its_behavior() {
    use std::time::Duration;
    let delay = Some(Duration::from_millis(3000));
    let timeout = Some(Duration::from_secs(7200));
    for (revert, hold, ordinary, exclusive) in [
        ("true", "false", delay, delay),
        ("true", "true", delay, delay),
        // Restoring off: an ordinary layout is kept, an exclusive one comes back.
        ("false", "false", timeout, delay),
        // Restoring off and the monitors held off: both are kept.
        ("false", "true", timeout, timeout),
    ] {
        let text =
            format!("dd_config_revert_on_disconnect={revert}\ndd_hold_layout_while_paused={hold}");
        let config = super::Config::parse(&text).unwrap();
        assert_eq!(config.display_disconnect_delay(false), ordinary, "{text}");
        assert_eq!(config.display_disconnect_delay(true), exclusive, "{text}");
        assert_eq!(
            config.boolean("dd_hold_layout_while_paused", false),
            hold == "true",
            "{text}"
        );
    }
}

#[test]
fn the_restore_setting_folds_into_the_disconnect_behavior_in_every_layer() {
    use super::folded_disconnect_behavior as fold;
    assert_eq!(fold(None), None);
    assert_eq!(fold(Some("false")), Some("keep_while_paused"));
    assert_eq!(fold(Some("\"no\"")), Some("keep_while_paused"));
    assert_eq!(fold(Some("true")), Some("restore"));
    assert_eq!(fold(Some("invalid")), Some("restore"));
    let mut config = super::Config::default();
    let sent = serde_json::json!({"dd_config_revert_on_disconnect": false});
    config.update(sent.as_object().unwrap()).unwrap();
    assert!(config.keeps_display_while_paused());
    assert!(!config.text().contains("dd_config_revert_on_disconnect"));
    for (layer, expected) in [
        (
            serde_json::json!({"dd_config_revert_on_disconnect": "false"}),
            Some("keep_while_paused"),
        ),
        (
            serde_json::json!({"dd_config_revert_on_disconnect": true}),
            Some("restore"),
        ),
        (
            serde_json::json!({"dd_config_revert_on_disconnect": false, "dd_disconnect_behavior": "restore"}),
            Some("restore"),
        ),
        (
            serde_json::json!({"dd_hold_layout_while_paused": true}),
            None,
        ),
        (serde_json::json!({"fec_percentage": 20}), None),
    ] {
        let mut layer = layer.as_object().unwrap().clone();
        super::migrate_overrides(&mut layer);
        assert_eq!(
            layer
                .get("dd_disconnect_behavior")
                .and_then(serde_json::Value::as_str),
            expected,
            "{layer:?}"
        );
        assert!(!layer.contains_key("dd_config_revert_on_disconnect"));
    }
    // Keep monitors off while paused is still its own setting, also per app.
    assert!(super::retired_name("dd_config_revert_on_disconnect"));
    assert!(!super::retired_name("dd_hold_layout_while_paused"));
    assert!(super::override_allowed("dd_disconnect_behavior"));
    assert!(super::override_allowed("dd_hold_layout_while_paused"));
}

/// Updates holds what Install updates automatically and an interval of 0
/// held: 0 is `off`, `auto_update` on is `install`, nothing saved is
/// `tell_me`; a value saved under Updates wins; the interval stays for any
/// other value.
#[test]
fn the_update_switches_fold_into_updates() {
    use super::{Config, UpdatesMode as U, folded_updates as fold};
    assert_eq!(fold(None, None), None);
    for (interval, auto, folded) in [
        (Some("0"), None, Some("off")),
        (Some("-1"), None, Some("off")),
        (Some("0"), Some("true"), Some("off")),
        (Some("86400"), Some("true"), Some("install")),
        (None, Some("true"), Some("install")),
        (None, Some("false"), None),
        (Some("3600"), Some("false"), None),
        (Some("soon"), None, None),
    ] {
        assert_eq!(fold(interval, auto), folded, "{interval:?} {auto:?}");
    }
    for (text, expected) in [
        ("", U::TellMe),
        ("auto_update=false", U::TellMe),
        ("auto_update=true", U::Install),
        ("update_check_interval=0", U::Off),
        ("update_check_interval=0\nauto_update=true", U::Off),
        ("update_check_interval=7200\nauto_update=true", U::Install),
        ("updates=off\nauto_update=true", U::Off),
        ("updates=tell_me\nupdate_check_interval=0", U::TellMe),
    ] {
        let config = Config::parse(text).unwrap();
        assert_eq!(config.updates(), expected, "{text}");
        assert!(!config.text().contains("auto_update"), "{text}");
    }
    // An interval other than 0 stays; Off makes it 0 for the schedule.
    let config = Config::parse("update_check_interval=7200").unwrap();
    assert_eq!(config.update_check_interval(), 7200);
    assert_eq!(config.updates(), U::TellMe);
    let config = Config::parse("update_check_interval=0\nauto_update=true").unwrap();
    assert_eq!(config.update_check_interval(), 0);
    assert!(!config.text().contains("update_check_interval"));
    assert_eq!(
        Config::parse("updates=off\nupdate_check_interval=7200")
            .unwrap()
            .update_check_interval(),
        0
    );
    // A save sent with the earlier names.
    let mut config = Config::default();
    let sent = serde_json::json!({"auto_update": true});
    config.update(sent.as_object().unwrap()).unwrap();
    assert_eq!(config.updates(), U::Install);
    let sent = serde_json::json!({"auto_update": false});
    config.update(sent.as_object().unwrap()).unwrap();
    assert_eq!(config.updates(), U::TellMe);
    let sent = serde_json::json!({"update_check_interval": 0});
    config.update(sent.as_object().unwrap()).unwrap();
    assert_eq!(config.updates(), U::Off);
    assert!(!config.text().contains("update_check_interval"));
    let sent = serde_json::json!({"update_check_interval": 86400});
    config.update(sent.as_object().unwrap()).unwrap();
    assert_eq!(config.updates(), U::TellMe);
    let sent = serde_json::json!({"auto_update": true, "updates": "off"});
    config.update(sent.as_object().unwrap()).unwrap();
    assert_eq!(config.updates(), U::Off);
    assert!(super::retired_name("auto_update"));
    assert!(!super::retired_name("update_check_interval"));
}

/// The Minimum quantizer switch folds into the minimum QPs: a switch that
/// was on keeps each saved value (the old default where none was saved), one
/// that was off leaves them out, so nothing turns on that was not on.
#[test]
fn the_minimum_quantizer_switch_folds_into_the_minimum_qps() {
    use super::Config;
    let qp = |config: &Config, key: &str| config.integer(key, 0);
    for (text, h264, hevc, av1) in [
        ("", 0, 0, 0),
        ("nvenc_enable_min_qp=false", 0, 0, 0),
        (
            "nvenc_enable_min_qp=false\nnvenc_min_qp_h264=30\nnvenc_min_qp_av1=40",
            0,
            0,
            0,
        ),
        ("nvenc_enable_min_qp=true", 19, 23, 23),
        ("nvenc_enable_min_qp=true\nnvenc_min_qp_hevc=30", 19, 30, 23),
        (
            "nvenc_enable_min_qp=true\nnvenc_min_qp_h264=0\nnvenc_min_qp_av1=51",
            0,
            23,
            51,
        ),
        ("nvenc_min_qp_h264=25", 25, 0, 0),
    ] {
        let config = Config::parse(text).unwrap();
        assert_eq!(
            [
                qp(&config, "nvenc_min_qp_h264"),
                qp(&config, "nvenc_min_qp_hevc"),
                qp(&config, "nvenc_min_qp_av1")
            ],
            [h264, hevc, av1],
            "{text}"
        );
        assert!(!config.text().contains("nvenc_enable_min_qp"), "{text}");
    }
    // A save sent with the switch.
    let mut config = Config::parse("nvenc_min_qp_hevc=30").unwrap();
    let sent = serde_json::json!({"nvenc_enable_min_qp": false});
    config.update(sent.as_object().unwrap()).unwrap();
    assert_eq!(qp(&config, "nvenc_min_qp_hevc"), 0);
    let sent = serde_json::json!({"nvenc_enable_min_qp": true});
    config.update(sent.as_object().unwrap()).unwrap();
    assert_eq!(qp(&config, "nvenc_min_qp_hevc"), 23);
    // An app's or device's overrides.
    for (layer, expected) in [
        (
            serde_json::json!({"nvenc_enable_min_qp": false, "nvenc_min_qp_h264": 30}),
            serde_json::json!({}),
        ),
        (
            serde_json::json!({"nvenc_enable_min_qp": "true", "nvenc_min_qp_h264": 30}),
            serde_json::json!({"nvenc_min_qp_h264": 30, "nvenc_min_qp_hevc": 23, "nvenc_min_qp_av1": 23}),
        ),
        (
            serde_json::json!({"nvenc_min_qp_hevc": 30}),
            serde_json::json!({"nvenc_min_qp_hevc": 30}),
        ),
    ] {
        let mut layer = layer.as_object().unwrap().clone();
        super::migrate_overrides(&mut layer);
        assert_eq!(serde_json::Value::Object(layer), expected);
    }
    assert!(super::retired_name("nvenc_enable_min_qp"));
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
    let config = super::Config::parse(
        "amd_input_queue_size=1\namd_lowlatency_mode=enabled\ncompute_queue_realtime=true\n",
    )
    .unwrap();
    for codec in 0..=2 {
        let stream = crate::rtsp::Negotiated {
            codec,
            ..Default::default()
        };
        let properties = crate::encoder_policy::amf(&config, &stream).unwrap();
        assert!(
            properties.iter().all(|property| {
                !property.name.ends_with("InputQueueSize") && property.name != "LowLatencyInternal"
            }),
            "codec {codec}"
        );
    }
}
