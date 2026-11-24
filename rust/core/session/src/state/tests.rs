//! Tests for client permission grants, credentials, and app/override deserialization.
mod paired;

use super::*;
fn client_with(perm: u32, extra: Value) -> Client {
    let mut document = json!({"name":"Phone","cert":"c","uuid":"u","perm":perm});
    document
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    serde_json::from_value(document).unwrap()
}
#[test]
fn host_control_is_off_by_default_and_for_a_full_permission_value() {
    assert!(!client_with(0x071f1f00, json!({})).allows_host_control());
    // Imports and fixtures use an all-ones permission value.
    assert!(!client_with(u32::MAX, json!({})).allows_host_control());
    assert!(!client_with(u32::MAX, json!({"allow_client_commands":true})).allows_host_control());
}
#[test]
fn host_control_reads_only_true_and_the_string_true() {
    assert!(client_with(0, json!({"host_control":true})).allows_host_control());
    assert!(client_with(0, json!({"host_control":"true"})).allows_host_control());
    for value in [
        json!("yes"),
        json!("1"),
        json!(1),
        json!("TRUE"),
        json!(false),
        json!(null),
        json!([true]),
    ] {
        assert!(
            !client_with(u32::MAX, json!({"host_control":value})).allows_host_control(),
            "{value}"
        );
    }
    let mut disabled = client_with(0, json!({"host_control":true}));
    disabled.enabled = false;
    assert!(!disabled.allows_host_control());
}
#[test]
fn imported_vibepollo_state_starts_without_host_control() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.json");
    let identity = crypto::Identity::generate().unwrap();
    write_json(
        &p,
        &json!({"root":{"named_devices":[
            {"name":"Old","uuid":"old","cert":identity.certificate,"perm":u32::MAX,
             "allow_client_commands":true}
        ]}}),
    )
    .unwrap();
    let s = PairedState::load(&p).unwrap();
    assert!(!s.clients[0].allows_host_control());
    // Saving and loading again doesn't add it either.
    s.save(&p).unwrap();
    let reloaded = PairedState::load(&p).unwrap();
    assert!(!reloaded.clients[0].allows_host_control());
    assert!(!reloaded.clients[0].extra.contains_key("host_control"));
}
#[test]
fn sleep_is_off_by_default_and_reads_only_a_json_true() {
    assert!(!client_with(0x071f1f00, json!({})).allows_sleep());
    assert!(!client_with(u32::MAX, json!({"host_control":true})).allows_sleep());
    assert!(client_with(0, json!({"allow_sleep":true})).allows_sleep());
    for value in [
        json!("true"),
        json!(1),
        json!("1"),
        json!(false),
        json!(null),
        json!([true]),
    ] {
        assert!(
            !client_with(u32::MAX, json!({"allow_sleep":value})).allows_sleep(),
            "{value}"
        );
    }
    let mut disabled = client_with(0, json!({"allow_sleep":true}));
    disabled.enabled = false;
    assert!(!disabled.allows_sleep());
}
#[test]
fn restart_and_shutdown_are_off_by_default_and_no_other_grant_or_role_implies_them() {
    // The pairing roles, Everything included, set permission bits only.
    for access in ["play", "view", "all"] {
        let perm = pairing_access(access).unwrap().unwrap();
        assert!(
            !client_with(perm, json!({})).allows_restart_shutdown(),
            "{access}"
        );
    }
    assert!(!client_with(FIRST_DEVICE_PERMISSION, json!({})).allows_restart_shutdown());
    assert!(
        !client_with(
            u32::MAX,
            json!({"host_control":true,"allow_sleep":true,"allow_usb":true})
        )
        .allows_restart_shutdown()
    );
    let granted = client_with(0, json!({"allow_restart_shutdown":true}));
    assert!(granted.allows_restart_shutdown());
    // Nor does it widen Sleep PC.
    assert!(!granted.allows_sleep());
    for value in [json!("true"), json!(1), json!(false), json!(null)] {
        assert!(
            !client_with(u32::MAX, json!({"allow_restart_shutdown":value}))
                .allows_restart_shutdown(),
            "{value}"
        );
    }
    let mut disabled = granted;
    disabled.enabled = false;
    assert!(!disabled.allows_restart_shutdown());
}
#[test]
fn usb_is_off_by_default_and_reads_only_a_json_true() {
    assert!(!client_with(0x071f1f00, json!({})).allows_usb());
    assert!(!client_with(u32::MAX, json!({})).allows_usb());
    assert!(!client_with(u32::MAX, json!({"host_control":true,"allow_sleep":true})).allows_usb());
    assert!(client_with(0, json!({"allow_usb":true})).allows_usb());
    for value in [
        json!("true"),
        json!(1),
        json!("1"),
        json!(false),
        json!(null),
        json!([true]),
    ] {
        assert!(
            !client_with(u32::MAX, json!({"allow_usb":value})).allows_usb(),
            "{value}"
        );
    }
    let mut disabled = client_with(0, json!({"allow_usb":true}));
    disabled.enabled = false;
    assert!(!disabled.allows_usb());
}
#[test]
fn coop_join_never_set_follows_launch_apps_and_false_refuses() {
    // Never set (or cleared with null): a device that launches apps may join, one
    // that only watches may not, and Host control has nothing to do with it.
    assert!(client_with(PLAY_DEVICE_PERMISSION, json!({})).allows_coop_join());
    assert!(client_with(LAUNCH_APPS, json!({"coop_join":null})).allows_coop_join());
    assert!(!client_with(LATER_DEVICE_PERMISSION, json!({})).allows_coop_join());
    assert!(!client_with(!LAUNCH_APPS, json!({"host_control":true})).allows_coop_join());
    // Joining launches the co-op entry, so without Launch apps even a stored true
    // keeps the device out: the console's switch and invite list say the same.
    assert!(!client_with(0, json!({"coop_join":true})).allows_coop_join());
    assert!(!client_with(LATER_DEVICE_PERMISSION, json!({"coop_join":"true"})).allows_coop_join());
    assert!(client_with(LAUNCH_APPS, json!({"coop_join":"true"})).allows_coop_join());
    // Set to anything but true, it keeps out a device that launches apps.
    for value in [json!(false), json!(1), json!("yes")] {
        assert!(
            !client_with(u32::MAX, json!({"coop_join":value})).allows_coop_join(),
            "{value}"
        );
    }
    let mut disabled = client_with(PLAY_DEVICE_PERMISSION, json!({"coop_join":true}));
    disabled.enabled = false;
    assert!(!disabled.allows_coop_join());
}
#[test]
fn folder_grants_read_only_read_and_write() {
    use rubylight_native_api::folders::FolderAccess;
    let client = client_with(
        u32::MAX,
        json!({"shared_folders":{"games":"read","mods":"write","bad":"Write","num":1}}),
    );
    assert_eq!(client.folder_access("games"), Some(FolderAccess::Read));
    assert_eq!(client.folder_access("mods"), Some(FolderAccess::Write));
    assert_eq!(client.folder_access("bad"), None);
    assert_eq!(client.folder_access("num"), None);
    assert_eq!(client.folder_access("other"), None);
    assert!(client.has_folders());
    assert!(!client_with(u32::MAX, json!({})).has_folders());
    assert!(!client_with(u32::MAX, json!({"shared_folders":{}})).has_folders());
    assert!(!client_with(u32::MAX, json!({"shared_folders":{"a":"none"}})).has_folders());
    assert!(!client_with(u32::MAX, json!({"shared_folders":["games"]})).has_folders());
    assert!(!client_with(u32::MAX, json!({"shared_folders":null})).has_folders());
    let mut disabled = client_with(0, json!({"shared_folders":{"games":"write"}}));
    disabled.enabled = false;
    assert_eq!(disabled.folder_access("games"), None);
    assert!(!disabled.has_folders());
}
#[test]
fn a_hand_edited_folder_id_the_console_refuses_grants_nothing() {
    for id in ["../x", "", "a/b", "a b", "games.", &"x".repeat(65)] {
        let client = client_with(u32::MAX, json!({"shared_folders":{id:"write"}}));
        assert_eq!(client.folder_access(id), None, "{id}");
        assert!(!client.has_folders(), "{id}");
    }
    // Next to a good one, only the good one counts.
    let client = client_with(
        u32::MAX,
        json!({"shared_folders":{"../x":"write","games":"read"}}),
    );
    assert_eq!(client.folder_access("../x"), None);
    assert!(client.has_folders());
}
#[test]
fn imported_vibepollo_sleep_opt_in_is_honoured() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.json");
    let identity = crypto::Identity::generate().unwrap();
    let other = crypto::Identity::generate().unwrap();
    write_json(
        &p,
        &json!({"root":{"named_devices":[
            {"name":"Opted in","uuid":"in","cert":identity.certificate,"perm":u32::MAX,
             "allow_sleep":true},
            {"name":"Not","uuid":"out","cert":other.certificate,"perm":u32::MAX,
             "allow_sleep":"true"}
        ]}}),
    )
    .unwrap();
    let s = PairedState::load(&p).unwrap();
    assert!(s.clients[0].allows_sleep());
    assert!(!s.clients[1].allows_sleep());
    s.save(&p).unwrap();
    let reloaded = PairedState::load(&p).unwrap();
    assert!(reloaded.clients[0].allows_sleep());
    assert!(!reloaded.clients[1].allows_sleep());
}

fn webrtc_client(n: u8, name: &str) -> Client {
    let id = format!("{n:02x}").repeat(16);
    let secret = format!("{n:02x}").repeat(32);
    serde_json::from_value(json!({
        "name": name, "cert": "", "uuid": format!("web-{n}"), "perm": 0x071f1f00u32,
        "webrtc": {"clientId": id, "secret": secret},
    }))
    .unwrap()
}
fn phone(name: &str) -> Client {
    let identity = crypto::Identity::generate().unwrap();
    serde_json::from_value(
            json!({"name":name,"uuid":format!("phone-{name}"),"cert":identity.certificate,"perm":FIRST_DEVICE_PERMISSION}),
        )
        .unwrap()
}
fn perm_of(s: &PairedState, name: &str) -> u32 {
    s.clients.iter().find(|c| c.name == name).unwrap().perm
}
#[test]
fn pairing_access_applies_to_later_devices_and_never_to_the_first() {
    assert_eq!(PLAY_DEVICE_PERMISSION, 0x07001f00);
    assert_eq!(PLAY_DEVICE_PERMISSION & !FIRST_DEVICE_PERMISSION, 0);
    assert_eq!(pairing_access("").unwrap(), None);
    assert_eq!(
        pairing_access("play").unwrap(),
        Some(PLAY_DEVICE_PERMISSION)
    );
    assert_eq!(
        pairing_access("view").unwrap(),
        Some(LATER_DEVICE_PERMISSION)
    );
    assert_eq!(
        pairing_access("all").unwrap(),
        Some(FIRST_DEVICE_PERMISSION)
    );
    assert!(pairing_access("Play").is_err());
    // Play games and Everything may join co-op too; view only may not.
    let dir = tempfile::tempdir().unwrap();
    let mut later = PairedState::load(&dir.path().join("state.json")).unwrap();
    later
        .clients
        .push(client_with(PLAY_DEVICE_PERMISSION, json!({})));
    for (perm, joins) in [
        (PLAY_DEVICE_PERMISSION, true),
        (FIRST_DEVICE_PERMISSION, true),
        (LATER_DEVICE_PERMISSION, false),
    ] {
        let mut client = client_with(perm, json!({}));
        client.extra = later.paired_device_extra(perm, None);
        assert_eq!(client.allows_coop_join(), joins, "{perm:#x}");
    }
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.json");
    let mut s = PairedState::load(&p).unwrap();
    assert_eq!(
        s.paired_device_permission(Some(LATER_DEVICE_PERMISSION)),
        FIRST_DEVICE_PERMISSION
    );
    s.add(&p, phone("Phone")).unwrap();
    assert_eq!(
        s.paired_device_permission(Some(PLAY_DEVICE_PERMISSION)),
        PLAY_DEVICE_PERMISSION
    );
    assert_eq!(s.paired_device_permission(None), LATER_DEVICE_PERMISSION);
}
const FILE_BITS: u32 = crate::native::FILE_UPLOAD | crate::native::FILE_DOWNLOAD;
#[test]
fn file_transfer_is_granted_on_its_own_never_with_everything() {
    assert_eq!(FIRST_DEVICE_PERMISSION & FILE_BITS, 0);
    assert_eq!(LATER_DEVICE_PERMISSION & FILE_BITS, 0);
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.json");
    let mut s = PairedState::load(&p).unwrap();
    s.add(&p, phone("Phone")).unwrap();
    assert_eq!(perm_of(&s, "Phone") & FILE_BITS, 0);
}
fn console_client(n: u8, username: &str) -> Client {
    let id = format!("{n:02x}").repeat(16);
    let secret = format!("{n:02x}").repeat(32);
    Client::console_over_webrtc(format!("Console ({username})"), username, &id, &secret)
}
#[test]
fn preparation_commands_keep_unknown_fields() {
    let original = json!({"do":"before","undo":"after","elevated":false,"future":{"value":42}});
    let command: PrepCommand = serde_json::from_value(original.clone()).unwrap();
    assert_eq!(serde_json::to_value(command).unwrap(), original);
}
#[test]
fn existing_credentials_format() {
    let c = Credentials {
        username: "user".into(),
        salt: "abc".into(),
        password: hex::encode(crypto::hash(b"passwordabc")).to_uppercase(),
    };
    assert!(c.verifies("user", "password"));
    assert!(c.verifies("User", "password"));
    assert!(!c.verifies("other", "password"));
    assert!(!c.verifies("user", "wrong"));
    // Independent SHA-256/password+salt vector in C++ util::hex byte order.
    let previous = Credentials {
        password: "F88D961F52F30D505CC0CBB98D01B38D0D789C075812B3C38748CEEAFFB73367".into(),
        ..c
    };
    assert!(previous.verifies("USER", "password"));
    assert!(!previous.verifies("user", "wrong"));
    assert!(!previous.verifies("other", "password"));
    let current = Credentials::new("user".into(), "new-password").unwrap();
    assert_eq!(
        current.password,
        crypto::legacy_hash(format!("new-password{}", current.salt).as_bytes())
    );
    assert!(current.verifies("user", "new-password"));
}

#[test]
fn approve_new_devices_is_only_a_json_true_and_grants_nothing_else() {
    let client = client_with(u32::MAX, json!({"pairing_approve": true}));
    assert!(client.allows_pairing_approve() && client.may_approve_pairing());
    assert!(!client.allows_host_control() && !client.allows_sleep());
    assert!(!client.allows_restart_shutdown() && !client.allows_usb());
    // Host control approves too, without holding this flag.
    let client = client_with(0, json!({"host_control": true}));
    assert!(client.may_approve_pairing() && !client.allows_pairing_approve());
    // A permission value or anything but true never grants it.
    assert!(!client_with(u32::MAX, json!({})).may_approve_pairing());
    for value in [json!("true"), json!(1), json!(false), json!(null)] {
        let client = client_with(u32::MAX, json!({"pairing_approve": value}));
        assert!(!client.may_approve_pairing(), "{value}");
    }
    let mut disabled = client_with(0, json!({"pairing_approve": true}));
    disabled.enabled = false;
    assert!(!disabled.may_approve_pairing());
}

#[test]
fn only_the_first_device_gets_approve_new_devices() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = PairedState::load(&dir.path().join("state.json")).unwrap();
    let extra = state.paired_device_extra(FIRST_DEVICE_PERMISSION, None);
    assert_eq!(extra.get(PAIRING_APPROVE_KEY), Some(&json!(true)));
    // The next one does not, not even with everything chosen in the console.
    state
        .clients
        .push(client_with(FIRST_DEVICE_PERMISSION, json!({})));
    for perm in [
        FIRST_DEVICE_PERMISSION,
        PLAY_DEVICE_PERMISSION,
        LATER_DEVICE_PERMISSION,
    ] {
        assert!(
            !state
                .paired_device_extra(perm, None)
                .contains_key(PAIRING_APPROVE_KEY)
        );
    }
    // Once every device is removed the next one is first again.
    state.clients.clear();
    assert!(
        state
            .paired_device_extra(FIRST_DEVICE_PERMISSION, None)
            .contains_key(PAIRING_APPROVE_KEY)
    );
}

#[test]
fn a_browser_paired_first_leaves_approving_to_the_first_device_that_can() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = PairedState::load(&dir.path().join("state.json")).unwrap();
    let mut browser = phone("Browser");
    browser.cert = String::new();
    state.clients.push(browser);
    // The browser took the first device's permissions, not the grant.
    let perm = state.paired_device_permission(None);
    assert_eq!(perm, LATER_DEVICE_PERMISSION);
    assert_eq!(
        state
            .paired_device_extra(perm, None)
            .get(PAIRING_APPROVE_KEY),
        Some(&json!(true))
    );
    let play = Some(PLAY_DEVICE_PERMISSION);
    assert!(
        state
            .paired_device_extra(PLAY_DEVICE_PERMISSION, play)
            .contains_key(PAIRING_APPROVE_KEY)
    );
    // Watch only, chosen by the owner, stays watch only.
    let view = Some(LATER_DEVICE_PERMISSION);
    assert!(
        !state
            .paired_device_extra(LATER_DEVICE_PERMISSION, view)
            .contains_key(PAIRING_APPROVE_KEY)
    );
    // Only the first with a certificate gets it, whatever it was given.
    state.clients.push(phone("Phone"));
    assert!(
        !state
            .paired_device_extra(FIRST_DEVICE_PERMISSION, None)
            .contains_key(PAIRING_APPROVE_KEY)
    );
}

#[test]
fn an_upgrade_leaves_devices_paired_before_it_without_the_grant() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let mut state = PairedState::load(&path).unwrap();
    let mut existing = phone("Phone");
    existing.perm = FIRST_DEVICE_PERMISSION;
    existing.extra.insert("coop_join".into(), json!(true));
    state.add(&path, existing).unwrap();
    let before = state.clients[0].extra.clone();
    let loaded = PairedState::load(&path).unwrap();
    assert_eq!(loaded.clients[0].extra, before);
    assert!(!loaded.clients[0].may_approve_pairing());
    assert!(
        !loaded
            .paired_device_extra(FIRST_DEVICE_PERMISSION, None)
            .contains_key(PAIRING_APPROVE_KEY)
    );
}

#[test]
fn an_app_written_with_nulls_by_another_tool_loads_with_empty_fields() {
    let app: App = serde_json::from_value(json!({
        "name": "MoonDeckStream",
        "cmd": null,
        "working-dir": null,
        "prep-cmd": [{"do": null, "undo": null}],
        "aliases": null,
    }))
    .unwrap();
    assert_eq!(app.cmd, "");
    assert_eq!(app.working_dir, "");
    assert_eq!(app.prep.len(), 1);
    assert_eq!(app.prep[0].r#do, "");
    assert_eq!(app.prep[0].undo, "");
    assert!(app.aliases.is_empty());
    let app: App = serde_json::from_value(json!({"name": "X", "prep-cmd": null})).unwrap();
    assert!(app.prep.is_empty());
}
#[test]
fn a_device_s_overrides_take_the_current_setting_names() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.json");
    let identity = crypto::Identity::generate().unwrap();
    let mut s = PairedState::load(&p).unwrap();
    s.add(
        &p,
        Client {
            name: "Phone".into(),
            cert: identity.certificate.clone(),
            uuid: "a".into(),
            perm: 0x071f_1f00,
            enabled: true,
            extra: BTreeMap::from([(
                "config_overrides".into(),
                json!({"experimental_packed_444": "false",
                       "frame_limiter_auto_virtual_framegen": "vrr",
                       "dd_hdr_option": "auto",
                       "dd_hdr_request_override": "force_off",
                       "touch_keyboard_digitizer": "true",
                       "keyboard": "true"}),
            )]),
        },
    )
    .unwrap();
    let loaded = PairedState::load(&p).unwrap();
    assert_eq!(
        loaded.clients[0].extra["config_overrides"],
        json!({"packed_444": "false", "virtual_display_refresh": "vrr",
               "dd_hdr_option": "force_off", "keyboard": "true"})
    );
}
