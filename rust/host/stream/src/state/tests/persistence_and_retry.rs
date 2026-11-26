//! Unit tests for profile migration, retained display reuse, credential reloading, codec retry backoff, and app library loading.

use super::super::*;

#[test]
fn packed_modes_are_logged_by_name() {
    assert_eq!(super::super::packed_mode_names(0), "none");
    assert_eq!(
        super::super::packed_mode_names(1 | 4),
        "HEVC 8-bit, AV1 8-bit"
    );
    assert_eq!(
        super::super::packed_mode_names(rubylight_protocol::packed444::ALL_MODES),
        "HEVC 8-bit, HEVC 10-bit, AV1 8-bit, AV1 10-bit"
    );
}
#[test]
fn settings_of_removed_encoders_and_limiters_migrate_with_one_warning() {
    use super::super::test_support::Fixture;
    let directory = Fixture::profile();
    let path = directory.join("sunshine.conf");
    std::fs::write(
        &path,
        "encoder = quicksync\nframe_limiter_provider = nvidia-control-panel\nqsv_preset = slow\nnvenc_preset = 4\nsunshine_name = Host\n",
    )
    .unwrap();
    let first = Fixture::load(directory.clone());
    {
        let config = first.host.config.read().unwrap();
        assert_eq!(config.get("encoder", ""), "auto");
        assert_eq!(config.get("frame_limiter_provider", ""), "auto");
        assert!(!config.values.contains_key("qsv_preset"));
        // NVENC is an encoder again: its settings stay.
        assert_eq!(config.get("nvenc_preset", ""), "4");
        assert_eq!(config.get("sunshine_name", ""), "Host");
    }
    let warnings = first.host.warnings.snapshot();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0].code, "settings_retired");
    assert!(
        warnings[0]
            .message
            .contains("encoder quicksync is now auto")
    );
    assert!(
        warnings[0]
            .message
            .contains("frame_limiter_provider nvidia-control-panel is now auto")
    );
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(saved.contains("encoder = auto"), "{saved}");
    assert!(!saved.contains("qsv"), "{saved}");
    // The next start reads the migrated file; the notice stays until dismissed.
    let second = Fixture::load(directory.clone());
    let warnings = second.host.warnings.snapshot();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0].code, "settings_retired");
    crate::host_warnings::dismiss(&directory, &second.host.warnings, "settings_retired").unwrap();
    assert!(
        Fixture::load(directory.clone())
            .host
            .warnings
            .snapshot()
            .is_empty()
    );
    assert_eq!(
        second.host.config.read().unwrap().get("encoder", ""),
        "auto"
    );
}

#[test]
fn nvidia_settings_a_2_6_host_left_pending_are_reported_at_start() {
    use super::super::test_support::Fixture;
    let directory = Fixture::profile();
    std::fs::write(
        directory.join("frame-limiter-recovery.json"),
        r#"{"version":1,"rtss":null,"nvidia":[{"id":277041154,"before":{"effective":0,"override_value":null},"applied":60}]}"#,
    )
    .unwrap();
    let first = Fixture::load(directory.clone());
    let warnings = first.host.warnings.snapshot();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0].code, "nvidia_limiter_retired");
    assert!(warnings[0].message.contains("NVIDIA Control Panel"));
    // Shown at each start until dismissed.
    let second = Fixture::load(directory.clone());
    let warnings = second.host.warnings.snapshot();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0].code, "nvidia_limiter_retired");
    crate::host_warnings::dismiss(&directory, &second.host.warnings, "nvidia_limiter_retired")
        .unwrap();
    assert!(Fixture::load(directory).host.warnings.snapshot().is_empty());
}

#[test]
fn an_older_device_layout_becomes_learned_once_with_a_notice() {
    use super::super::test_support::Fixture;
    let directory = Fixture::profile();
    let first = Fixture::load(directory.clone());
    first.client(u32::MAX);
    let path = first.host.paired_path.clone();
    let mut paired = PairedState::load(&path).unwrap();
    let device = paired
        .clients
        .iter_mut()
        .find(|c| c.name == "Parity device")
        .unwrap();
    device
        .extra
        .insert("virtual_display_layout".into(), json!("extended"));
    paired.document["root"]
        .as_object_mut()
        .unwrap()
        .remove("legacy_extended_layout_migrated");
    paired.save(&path).unwrap();
    // The fixture host asks for the default exclusive layout: the device's "extended" stays,
    // learned over it, instead of yielding to a layout that turns the other monitors off.
    let second = Fixture::load(directory.clone());
    {
        let paired = second.host.paired.read().unwrap();
        let device = paired
            .clients
            .iter()
            .find(|c| c.name == "Parity device")
            .unwrap();
        assert_eq!(
            device.extra["virtual_display_layout"],
            json!("extended"),
            "{:?}",
            device.extra
        );
        assert_eq!(
            device.extra[butterpollo_session::state::LEARNED_OVER],
            json!("exclusive")
        );
        assert_eq!(
            paired.document["root"]["legacy_extended_layout_migrated"],
            json!(true)
        );
    }
    let warnings = second.host.warnings.snapshot();
    let notice = warnings
        .iter()
        .find(|warning| warning.code == "display_layout_learned")
        .unwrap_or_else(|| panic!("{warnings:?}"));
    assert!(
        notice.message.contains("Parity device"),
        "{}",
        notice.message
    );
    assert!(notice.message.contains("keep Extend"), "{}", notice.message);
    assert!(
        notice.message.contains("under Devices"),
        "{}",
        notice.message
    );
    // The one-time flag keeps a later start from migrating or warning again.
    crate::host_warnings::dismiss(&directory, &second.host.warnings, "display_layout_learned")
        .unwrap();
    let third = Fixture::load(directory);
    assert!(third.host.warnings.snapshot().is_empty());
    assert_eq!(
        third.host.paired.read().unwrap().clients[0].extra
            [butterpollo_session::state::LEARNED_OVER],
        json!("exclusive")
    );
}

#[test]
fn a_rejoin_in_the_same_mode_reuses_the_clients_own_display_untouched() {
    use super::super::take_retained;
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::time::Instant;
    let paused = Some(Instant::now());
    let mine = Arc::new((2560, 1440, 120_000, true));
    let theirs = Arc::new((2560, 1440, 120_000, true));
    let mut displays = BTreeMap::from([
        ("me".to_string(), (mine.clone(), paused)),
        ("other".to_string(), (theirs.clone(), None)),
    ]);
    let same = |lease: &Arc<(u32, u32, u32, bool)>| **lease == (2560, 1440, 120_000, true);

    // The same client in the same mode gets the very display it had, and
    // the retained entry stays as it was: nothing is created or released.
    let reused = take_retained(&mut displays, "me", same).unwrap();
    assert!(Arc::ptr_eq(&reused, &mine));
    assert_eq!(displays.len(), 2);
    assert!(Arc::ptr_eq(&displays["me"].0, &mine));
    assert_eq!(displays["me"].1, paused);
    drop(reused);
    // Rejoining again still finds it.
    assert!(Arc::ptr_eq(
        &take_retained(&mut displays, "me", same).unwrap(),
        &mine
    ));

    // A client with nothing retained never takes another client's display.
    assert_eq!(take_retained(&mut displays, "new", same), Err(None));
    assert!(Arc::ptr_eq(&displays["other"].0, &theirs));
    assert!(Arc::ptr_eq(&displays["me"].0, &mine));
}
#[test]
fn a_rejoin_in_another_mode_releases_only_the_clients_own_display() {
    use super::super::take_retained;
    use std::collections::BTreeMap;
    use std::sync::Arc;
    let mine = Arc::new((2560, 1440, 120_000, true));
    let theirs = Arc::new((2560, 1440, 120_000, true));
    let mut displays = BTreeMap::from([
        ("me".to_string(), (mine.clone(), None)),
        ("other".to_string(), (theirs.clone(), None)),
    ]);
    let sdr = |lease: &Arc<(u32, u32, u32, bool)>| **lease == (2560, 1440, 120_000, false);
    let released = take_retained(&mut displays, "me", sdr)
        .unwrap_err()
        .unwrap();
    assert!(Arc::ptr_eq(&released, &mine));
    assert!(!displays.contains_key("me"));
    assert!(Arc::ptr_eq(&displays["other"].0, &theirs));
    assert_eq!(take_retained(&mut displays, "me", sdr), Err(None));
}
#[test]
fn a_sign_in_set_while_the_host_runs_is_taken_up_and_not_written_back() {
    use super::super::test_support::Fixture;
    use butterpollo_session::state::{Credentials, load_json, write_json};
    let f = Fixture::new();
    let h = &f.host;
    // As in Apollo and Sunshine profiles, the sign-in shares the
    // paired-state file.
    assert_eq!(h.credentials_path, h.paired_path);
    let old = Credentials::new("apollo".into(), "old-password").unwrap();
    h.save_credentials(&old).unwrap();
    *h.credentials.write().unwrap() = Some(old);
    h.new_web_session(
        "apollo".into(),
        false,
        String::new(),
        "127.0.0.1".into(),
        None,
    )
    .unwrap();
    let tokens = |h: &Host| h.aliases.lock().unwrap()["root"]["api_tokens"].clone();
    {
        let mut aliases = h.aliases.lock().unwrap();
        aliases["root"]["api_tokens"] = serde_json::json!([{"hash": "made with old-password"}]);
        write_json(&h.aliases_path, &*aliases).unwrap();
    }
    // The console's own write is no change from outside.
    h.reload_credentials();
    assert_eq!(h.web_sessions.lock().unwrap().len(), 1);
    assert_eq!(tokens(h).as_array().map(Vec::len), Some(1));
    // --creds writes the file while the host runs.
    let set = |credentials: Option<&Credentials>| {
        let mut document = load_json(&h.credentials_path, serde_json::json!({})).unwrap();
        let document = document.as_object_mut().unwrap();
        for key in ["username", "password", "salt"] {
            document.remove(key);
        }
        if let Some(c) = credentials {
            document.extend(
                serde_json::to_value(c)
                    .unwrap()
                    .as_object()
                    .unwrap()
                    .clone(),
            );
        }
        write_json(&h.credentials_path, &document).unwrap();
    };
    let new = Credentials::new("Ramazan K".into(), "new-password").unwrap();
    set(Some(&new));
    h.paired.write().unwrap().save(&h.paired_path).unwrap();
    h.reload_credentials();
    let current = h.credentials.read().unwrap().clone().unwrap();
    assert!(current.verifies("ramazan k", "new-password"));
    assert!(!current.verifies("apollo", "old-password"));
    assert!(h.web_sessions.lock().unwrap().is_empty());
    // So are the API tokens made while the old password was in force.
    assert_eq!(tokens(h), serde_json::json!([]));
    assert_eq!(
        load_json(&h.aliases_path, serde_json::json!({})).unwrap()["root"]["api_tokens"],
        serde_json::json!([])
    );
    // A later save of the paired state keeps the new sign-in.
    let paired = h.paired.read().unwrap();
    paired.save(&h.paired_path).unwrap();
    drop(paired);
    let saved = Credentials::load(&h.credentials_path).unwrap().unwrap();
    assert!(saved.verifies("Ramazan K", "new-password"));
    assert!(h.paired.read().unwrap().document["root"]["named_devices"].is_array());
    // A sign-in removed from the file offers the first-run setup.
    set(None);
    h.reload_credentials();
    assert!(h.credentials.read().unwrap().is_none());
    assert!(h.paired.read().unwrap().document.get("username").is_none());
}
#[test]
fn host_load_reuses_the_shared_virtual_display_guid_without_replacing_an_existing_one() {
    use super::super::test_support::Fixture;
    let f = Fixture::new();
    let imported = "f773d31b-43da-470c-80d5-02e777a6d993";
    let existing = "15ff84bd-9db8-45ef-bfde-e60337744358";
    butterpollo_session::state::write_json(
        &f.host.aliases_path,
        &serde_json::json!({"root":{"shared_virtual_display_guid":imported}}),
    )
    .unwrap();
    let load =
        || super::super::Host::load(f.host.directory.clone(), f.host.assets.clone(), None).unwrap();
    let host = load();
    assert_eq!(
        host.paired.read().unwrap().document["root"]["shared_virtual_display_guid"],
        imported
    );
    let mut state = host.paired.write().unwrap();
    state.document["root"]["shared_virtual_display_guid"] = existing.into();
    state.save(&host.paired_path).unwrap();
    drop(state);
    assert_eq!(
        load().paired.read().unwrap().document["root"]["shared_virtual_display_guid"],
        existing
    );
}
#[test]
fn host_load_migrates_inline_webhook_secrets_out_of_sunshine_conf() {
    use super::super::test_support::Fixture;
    let f = Fixture::new();
    let conf_path = f.host.directory.join("sunshine.conf");
    std::fs::write(
        &conf_path,
        "webhooks = [{\"url\":\"https://example.test/hook\",\"events\":[\"app.started\"],\"secret\":\"s3cret\"}]\n",
    )
    .unwrap();
    let host =
        super::super::Host::load(f.host.directory.clone(), f.host.assets.clone(), None).unwrap();
    let on_disk = std::fs::read_to_string(&conf_path).unwrap();
    assert!(
        !on_disk.contains("s3cret"),
        "inline secret must be stripped from sunshine.conf: {on_disk}"
    );
    let loaded = butterpollo_session::webhook::load_from_profile(
        &host.config.read().unwrap(),
        &host.directory,
    );
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].secret, "s3cret");
    assert!(butterpollo_session::webhook::secrets_path(&host.directory).is_file());
}
#[test]
#[ignore = "requires SYSTEM in a console session with nobody signed in"]
fn standard_codec_retries_do_not_wait_for_windows_sign_in() {
    assert!(butterpollo_win_io::process::is_system());
    assert!(!butterpollo_win_io::process::user_signed_in());
    let f = test_support::Fixture::new();
    let now = Instant::now() - CODEC_RETRY_MIN;
    let mut retry = CodecRetry::new(now);
    retry.completed(0, Some("GPU not ready at startup"), &f.host.warnings, now);
    let host = f.host.clone();
    let (send, receive) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        send.send(host.wait_for_codec_retry(&retry)).unwrap();
    });
    let result = receive.recv_timeout(Duration::from_secs(2));
    f.host
        .stop
        .store(true, std::sync::atomic::Ordering::Release);
    worker.join().unwrap();
    assert!(result.unwrap());
}
#[test]
fn codecs_keep_retrying_with_capped_backoff_until_the_probe_recovers() {
    let mut now = Instant::now();
    let mut retry = CodecRetry::new(now);
    let warnings = Warnings::default();
    let mut attempts = 0;
    let mut probe = || {
        attempts += 1;
        if attempts <= 100 {
            (0, Some("AMF error 1"))
        } else {
            (0x101, None)
        }
    };
    for delay in [5, 10, 20, 40]
        .into_iter()
        .chain(std::iter::repeat_n(60, 96))
    {
        let (flags, error) = probe();
        assert!(retry.completed(flags, error, &warnings, now));
        assert_eq!(
            warnings.snapshot()[0].message,
            "No video encoder available: AMF error 1. Retrying."
        );
        now += Duration::from_secs(delay);
        assert!(!retry.due(now - Duration::from_millis(1), false));
        assert!(retry.due(now, false));
    }
    let (flags, error) = probe();
    assert!(!retry.completed(flags, error, &warnings, now));
    assert!(warnings.snapshot().is_empty());
    assert!(!retry.due(now + Duration::from_secs(600), true));
    assert_eq!(attempts, 101);
}
#[test]
fn client_requests_bypass_backoff_but_never_the_minimum_interval() {
    let mut now = Instant::now();
    let mut retry = CodecRetry::new(now);
    let warnings = Warnings::default();
    let mut calls = 0;
    let mut probe = || {
        calls += 1;
        0
    };
    for _ in 0..10 {
        assert!(retry.completed(probe(), Some("driver unavailable"), &warnings, now));
        for milliseconds in [0, 1, 250, 4999] {
            assert!(!retry.due(now + Duration::from_millis(milliseconds), true));
        }
        now += CODEC_RETRY_MIN;
        assert!(retry.due(now, true));
    }
    assert_eq!(calls, 10);
    assert!(!retry.due(now, false));
    assert!(retry.due(now + Duration::from_secs(55), false));
}
#[test]
fn codec_failure_reminders_are_limited_and_recovery_resets_the_delay() {
    let now = Instant::now();
    let mut retry = CodecRetry::new(now);
    assert!(retry.warn(now));
    for seconds in 1..60 {
        assert!(!retry.warn(now + Duration::from_secs(seconds)));
    }
    assert!(retry.warn(now + Duration::from_secs(60)));
    let warnings = Warnings::default();
    for _ in 0..4 {
        assert!(retry.completed(0, None, &warnings, now));
    }
    assert!(!retry.due(now + Duration::from_secs(39), false));
    assert!(!retry.completed(1, None, &warnings, now));
    assert!(retry.completed(0, Some("COM initialization failed"), &warnings, now));
    assert_eq!(
        warnings.snapshot()[0].message,
        "No video encoder available: COM initialization failed. Retrying."
    );
    assert!(!retry.due(now + Duration::from_secs(4), false));
    assert!(retry.due(now + Duration::from_secs(5), false));
}
#[test]
fn codec_modifiers_do_not_count_as_video_and_optional_codecs_keep_standard_retries() {
    let now = Instant::now();
    let mut retry = CodecRetry::new(now);
    let warnings = Warnings::default();
    assert!(retry.completed(0x40000000, None, &warnings, now));
    assert_eq!(
        warnings.snapshot()[0].message,
        "No video encoder available: the encoder produced no packets. Retrying."
    );
    assert!(retry.completed(0x800000, None, &warnings, now));
    assert!(warnings.snapshot().is_empty());
    assert!(retry.completed(0, Some("AMF error 1"), &warnings, now));
    assert_eq!(warnings.snapshot().len(), 1);
    assert!(!retry.completed(0x10000, None, &warnings, now));
    assert!(warnings.snapshot().is_empty());
}
#[test]
fn an_invalid_library_starts_empty_and_is_kept() {
    let dir = std::env::temp_dir().join(format!("butterpollo-apps-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("apps.json");
    let default = json!({"apps":[{"name":"Desktop"}]});
    assert_eq!(load_library(&path, default.clone()).unwrap(), default);
    std::fs::write(&path, b"{\"apps\":[{\"name\":\"Game\",}]}").unwrap();
    assert_eq!(
        load_library(&path, default.clone()).unwrap()["apps"],
        json!([])
    );
    assert_eq!(
        std::fs::read(dir.join("apps.json.invalid")).unwrap(),
        b"{\"apps\":[{\"name\":\"Game\",}]}"
    );
    std::fs::write(&path, b"{\"apps\":[{\"name\":\"Game\"}]}").unwrap();
    assert_eq!(
        load_library(&path, default.clone()).unwrap()["apps"][0]["name"],
        "Game"
    );
    // Notepad's byte order mark, and a blank file Vibepollo reads as missing.
    std::fs::write(&path, b"\xef\xbb\xbf{\"apps\":[{\"name\":\"Noted\"}]}").unwrap();
    assert_eq!(
        load_library(&path, default.clone()).unwrap()["apps"][0]["name"],
        "Noted"
    );
    std::fs::write(&path, b"\r\n").unwrap();
    assert_eq!(load_library(&path, default.clone()).unwrap(), default);
    std::fs::remove_dir_all(&dir).ok();
}
/// A profile copied from Vibepollo or restored from an earlier backup: the
/// host reads sunshine.conf and apps.json under the current setting names,
/// and writes the library back migrated.
#[test]
fn a_profile_with_earlier_setting_names_loads_under_the_current_ones() {
    let directory = super::super::test_support::Fixture::profile();
    std::fs::write(
        directory.join("sunshine.conf"),
        "frame_limiter_auto_virtual_framegen = enabled\nexperimental_packed_444 = false\ndd_hdr_request_override = force_on\ntouch_keyboard_digitizer = true\n",
    )
    .unwrap();
    std::fs::write(
        directory.join("apps.json"),
        json!({"env": {}, "apps": [{
            "name": "Game",
            "frame-gen-limiter-fix": true,
            "virtual-screen": true,
            "config-overrides": {"dd_hdr_option": "auto", "dd_hdr_request_override": "force_off",
                                 "experimental_packed_444": true},
            "device-overrides": {"tv": {"config-overrides": {"frame_limiter_auto_virtual_framegen": "disabled"}}}
        }]})
        .to_string(),
    )
    .unwrap();
    let f = super::super::test_support::Fixture::load(directory.clone());
    {
        let config = f.host.config.read().unwrap();
        assert_eq!(config.get("virtual_display_refresh", ""), "enabled");
        assert!(!config.packed_444());
        assert_eq!(config.get("dd_hdr_option", ""), "force_on");
        for old in [
            "frame_limiter_auto_virtual_framegen",
            "experimental_packed_444",
            "dd_hdr_request_override",
            "touch_keyboard_digitizer",
        ] {
            assert!(!config.values.contains_key(old), "{old}");
        }
    }
    let app = f.host.apps.read().unwrap()[0].clone();
    assert!(!app.extra.contains_key("frame-gen-limiter-fix"));
    assert!(!app.extra.contains_key("virtual-screen"));
    assert_eq!(app.extra["virtual-display"], json!(true));
    assert_eq!(
        app.extra["config-overrides"],
        json!({"dd_hdr_option": "force_off", "packed_444": true})
    );
    assert_eq!(
        app.extra["device-overrides"]["tv"]["config-overrides"],
        json!({"virtual_display_refresh": "disabled"})
    );
    // The file holds the migrated record, so the next start is quiet.
    let saved: Value = serde_json::from_slice(&std::fs::read(&f.host.apps_path).unwrap()).unwrap();
    assert_eq!(
        saved["apps"][0]["config-overrides"],
        app.extra["config-overrides"]
    );
    assert!(saved["apps"][0].get("frame-gen-limiter-fix").is_none());
    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn concurrent_web_session_refresh_uses_short_rotation_grace_until_revoked() {
    let f = super::super::test_support::Fixture::new();
    let h = &f.host;
    let (_, initial_refresh, _, _) = h
        .new_web_session(
            "apollo".into(),
            false,
            String::new(),
            "127.0.0.1".into(),
            None,
        )
        .unwrap();
    let previous = {
        let sessions = h.web_sessions.lock().unwrap();
        sessions
            .values()
            .find(|s| crate::web_sessions::hash(&initial_refresh) == s.refresh)
            .cloned()
            .unwrap()
    };

    let first = h
        .new_web_session(
            "apollo".into(),
            false,
            String::new(),
            "127.0.0.1".into(),
            Some(previous.clone()),
        )
        .unwrap();
    // Concurrent second tab arriving with the pre-rotation snapshot gets the same replacement.
    let second = h
        .new_web_session(
            "apollo".into(),
            false,
            String::new(),
            "127.0.0.1".into(),
            Some(previous.clone()),
        )
        .unwrap();
    assert_eq!(first.0, second.0);
    assert_eq!(first.1, second.1);
    assert_eq!(first.2, second.2);

    // Revoking or signing out invalidates the grace entry.
    h.web_sessions.lock().unwrap().clear();
    assert!(
        h.new_web_session(
            "apollo".into(),
            false,
            String::new(),
            "127.0.0.1".into(),
            Some(previous),
        )
        .is_err()
    );
}

#[test]
fn reload_apps_retries_after_transient_read_error() {
    let f = super::super::test_support::Fixture::new();
    let h = &f.host;
    // Truncated/invalid JSON does not burn the stamp, so a later valid read succeeds
    // even if mtime/size check sees the updated write or retry.
    let before = h.apps.read().unwrap().len();
    std::fs::write(&h.apps_path, b"{").unwrap();
    h.reload_apps();
    assert_eq!(h.apps.read().unwrap().len(), before);
    // Keep same size & clear cached stamp? Instead ensure file_stamp only updated on success:
    let failed_stamp = file_stamp(&h.apps_path);
    assert_ne!(*h.apps_stamp.lock().unwrap(), failed_stamp);
}
