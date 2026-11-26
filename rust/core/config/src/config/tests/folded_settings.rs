//! Tests for folded settings (HDR request, remote monitor, disconnect behavior, updates, min QP).
use super::*;

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

/// The HDR request folds into Display HDR, one choice of four: Match the
/// stream, Always on, Always off, Don't change HDR.
#[test]
fn the_hdr_request_folds_into_display_hdr() {
    for (text, option) in [
        ("", None),
        ("dd_hdr_option = auto\n", Some("auto")),
        ("dd_hdr_request_override = force_on\n", Some("force_on")),
        (
            "dd_hdr_option = auto\ndd_hdr_request_override = force_off\n",
            Some("force_off"),
        ),
        // Don't change HDR still wins over the request, as it did.
        (
            "dd_hdr_option = disabled\ndd_hdr_request_override = force_on\n",
            Some("disabled"),
        ),
        ("dd_hdr_request_override = auto\n", None),
        ("dd_hdr_option = force_on\n", Some("force_on")),
        // Always on or off is a later choice than the request beside it.
        (
            "dd_hdr_option = force_on\ndd_hdr_request_override = force_off\n",
            Some("force_on"),
        ),
    ] {
        let c = Config::parse(text).unwrap();
        assert_eq!(
            c.values.get(HDR_OPTION).map(String::as_str),
            option,
            "{text}"
        );
        assert!(!c.values.contains_key(HDR_REQUEST), "{text}");
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
    // The stream's end removes the monitor for the end and for disconnect,
    // as both earlier switches on did.
    for (text, at_end) in [
        ("", false),
        ("remove_remote_monitor=stream_end", true),
        ("remove_remote_monitor=disconnect", true),
        (
            "remote_monitor_disconnect_on_stream_end=true
remote_monitor_disconnect_on_client_disconnect=true",
            true,
        ),
        ("remote_monitor_disconnect_on_client_disconnect=true", true),
    ] {
        assert_eq!(
            Config::parse(text)
                .unwrap()
                .removes_remote_monitor_at_stream_end(),
            at_end,
            "{text}"
        );
    }
    assert!(super::retired_name(
        "remote_monitor_disconnect_on_stream_end"
    ));
    assert!(super::retired_name(
        "remote_monitor_disconnect_on_client_disconnect"
    ));
    // An app's or device's overrides fold at migration where their own
    // switches decide the result: the client's disconnect on, or both saved.
    for (layer, expected) in [
        (
            serde_json::json!({"remote_monitor_disconnect_on_client_disconnect": true}),
            Some("disconnect"),
        ),
        (
            serde_json::json!({"remote_monitor_disconnect_on_stream_end": true, "remote_monitor_disconnect_on_client_disconnect": false}),
            Some("stream_end"),
        ),
        (
            serde_json::json!({"remote_monitor_disconnect_on_stream_end": false, "remote_monitor_disconnect_on_client_disconnect": false}),
            Some("never"),
        ),
        (
            serde_json::json!({"remote_monitor_disconnect_on_stream_end": true, "remove_remote_monitor": "never"}),
            Some("never"),
        ),
        (serde_json::json!({"fec_percentage": 20}), None),
    ] {
        let mut layer = layer.as_object().unwrap().clone();
        super::migrate_overrides(&mut layer);
        assert_eq!(
            layer
                .get("remove_remote_monitor")
                .and_then(serde_json::Value::as_str),
            expected,
            "{layer:?}"
        );
        assert!(!layer.contains_key("remote_monitor_disconnect_on_stream_end"));
        assert!(!layer.contains_key("remote_monitor_disconnect_on_client_disconnect"));
    }
    // One that does not decide alone stays as saved and folds over the layers
    // below when it is applied.
    for (host, layer, expected) in [
        (
            "",
            serde_json::json!({"remote_monitor_disconnect_on_stream_end": true}),
            "stream_end",
        ),
        (
            "remote_monitor_disconnect_on_client_disconnect=true",
            serde_json::json!({"remote_monitor_disconnect_on_stream_end": false}),
            "disconnect",
        ),
        (
            "remote_monitor_disconnect_on_stream_end=true",
            serde_json::json!({"remote_monitor_disconnect_on_client_disconnect": false}),
            "stream_end",
        ),
        (
            "remote_monitor_disconnect_on_stream_end=true",
            serde_json::json!({"remote_monitor_disconnect_on_stream_end": false}),
            "never",
        ),
        (
            "remote_monitor_disconnect_on_stream_end=true
remote_monitor_disconnect_on_client_disconnect=true",
            serde_json::json!({"remote_monitor_disconnect_on_client_disconnect": false}),
            "stream_end",
        ),
    ] {
        let layer = layer.as_object().unwrap().clone();
        let mut migrated = layer.clone();
        assert!(!super::migrate_overrides(&mut migrated), "{layer:?}");
        assert_eq!(migrated, layer);
        let mut applied = Config::parse(host).unwrap();
        applied.update(&migrated).unwrap();
        assert_eq!(
            applied.get("remove_remote_monitor", "never"),
            expected,
            "{host:?} {layer:?}"
        );
    }
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
/// that was off (or, in a file an earlier release wrote, was not saved: off
/// was its default) leaves them out, so nothing turns on that was not on.
/// An earlier file is told by holding any setting this release folded or
/// renamed; a file without one cannot be told from this release's own, whose
/// minimum QPs apply on their own.
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
        // This release's own file: a QP applies without a switch.
        ("nvenc_min_qp_h264=25", 25, 0, 0),
        // An earlier file with no switch saved had it off.
        ("link_rate_adaptation=true\nnvenc_min_qp_h264=25", 0, 0, 0),
        (
            "auto_update=false\nnvenc_min_qp_hevc=30\nnvenc_min_qp_av1=40",
            0,
            0,
            0,
        ),
        // With the switch saved, it decides whatever else the file holds.
        (
            "link_rate_adaptation=true\nnvenc_enable_min_qp=true\nnvenc_min_qp_h264=25",
            25,
            23,
            23,
        ),
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
        // Off is 0 for each codec, not nothing: it overrides a host that has
        // the QPs on.
        (
            serde_json::json!({"nvenc_enable_min_qp": false, "nvenc_min_qp_h264": 30}),
            serde_json::json!({"nvenc_min_qp_h264": 0, "nvenc_min_qp_hevc": 0, "nvenc_min_qp_av1": 0}),
        ),
        (
            serde_json::json!({"nvenc_enable_min_qp": "false"}),
            serde_json::json!({"nvenc_min_qp_h264": 0, "nvenc_min_qp_hevc": 0, "nvenc_min_qp_av1": 0}),
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
    // A device with the switch off under a host with it on applies 0.
    let host = Config::parse(
        "nvenc_enable_min_qp=true
nvenc_min_qp_h264=21",
    )
    .unwrap();
    assert_eq!(qp(&host, "nvenc_min_qp_h264"), 21);
    let mut layer = serde_json::json!({"nvenc_enable_min_qp": false})
        .as_object()
        .unwrap()
        .clone();
    super::migrate_overrides(&mut layer);
    let mut stream = host.clone();
    stream.update(&layer).unwrap();
    assert_eq!(
        [
            qp(&stream, "nvenc_min_qp_h264"),
            qp(&stream, "nvenc_min_qp_hevc"),
            qp(&stream, "nvenc_min_qp_av1")
        ],
        [0, 0, 0]
    );
    // The host's own are untouched.
    assert_eq!(qp(&host, "nvenc_min_qp_hevc"), 23);
}

#[test]
fn per_source_library_sync_override_passes_check_saved_when_shared_sync_is_off() {
    use super::Config;
    let mut config = Config::parse("library_auto_sync=false").unwrap();
    let sent = serde_json::json!({"steam_auto_sync": true});
    config.update(sent.as_object().unwrap()).unwrap();
    config
        .check_saved([&"steam_auto_sync".to_string()])
        .unwrap();
    assert!(config.boolean("steam_auto_sync", false));
    assert!(!config.boolean("library_auto_sync", true));

    // Actual comment/unclosed syntax is still rejected.
    let mut bad = Config::default();
    let sent = serde_json::json!({"sunshine_name": "Desk # 1"});
    bad.update(sent.as_object().unwrap()).unwrap();
    assert!(bad.check_saved([&"sunshine_name".to_string()]).is_err());
}
