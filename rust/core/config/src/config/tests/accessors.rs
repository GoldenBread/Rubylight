//! Tests for typed Config accessors, defaults, shared folders, and port/time parsing.

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
        assert!(super::folders::valid_drive_root(root), "{root}");
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
        assert!(!super::folders::valid_drive_root(root), "{root}");
    }
    for root in ["/srv/games", "/home/player/My Games", "/"] {
        assert!(super::folders::valid_unix_root(root), "{root}");
    }
    for root in [
        "",
        "games",
        "./games",
        "/srv/../etc",
        "/srv/games/.",
        r"D:\Games",
    ] {
        assert!(!super::folders::valid_unix_root(root), "{root}");
    }
    let config = |path: &str| {
        let list = serde_json::json!([{"id": "games", "name": "Games", "path": path}]);
        super::Config::parse(&format!("shared_folders = {list}"))
            .unwrap()
            .shared_folders()
    };
    let local = if cfg!(windows) {
        r"D:\Games"
    } else {
        "/srv/games"
    };
    assert_eq!(config(local).len(), 1);
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
