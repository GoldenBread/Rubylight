//! Unit tests for `nightjar_core::hosts`: `HostStore`, `HostMetaStore`, `IdentityStore`,
//! atomic `0o600` writes, `.bad` backup, unreadable refusal, DER<->PEM, and migration idempotency.

use super::*;
use crate::http::ClientIdentity;
use nightjar_policy::http::ServerInfo;
use std::collections::BTreeMap;

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("nightjar-hosts-{tag}-{nanos}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn host_store_upsert_detects_changes_preserves_extra_and_does_not_resurrect_deleted() {
    let dir = temp_dir("store");
    let store = HostStore::open(&dir);

    let mut entry = HostEntry::default();
    entry.shared.unique_id = "u1".into();
    entry.shared.name = "Desk".into();
    entry.local_endpoint = EndpointSlot::new("192.168.1.10", 47989);
    entry.remote_endpoint = EndpointSlot::new("203.0.113.9", 48989);
    entry.extra.insert("custom".into(), serde_json::json!(42));

    assert!(store.upsert(entry.clone()).unwrap());
    assert!(!store.upsert(entry.clone()).unwrap(), "identical upsert is a no-op");

    let loaded = store.get_by_uuid("u1").unwrap();
    assert_eq!(loaded, entry);
    assert_eq!(loaded.remote_endpoint.as_ref().unwrap().port, 48989);
    assert_eq!(loaded.extra.get("custom"), Some(&serde_json::json!(42)));

    // Legacy SQLite migration runs at most once; deleting u2 afterwards never resurrects it.
    let mut legacy = HostEntry::default();
    legacy.shared.unique_id = "u2".into();
    legacy.shared.name = "Legacy".into();
    assert!(store.import_legacy_if_needed(vec![legacy.clone()]).unwrap());
    assert!(store.get_by_uuid("u2").is_some());
    assert!(store.delete_by_uuid("u2").unwrap());
    assert!(!store.import_legacy_if_needed(vec![legacy]).unwrap());
    assert!(store.get_by_uuid("u2").is_none());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn host_store_unreadable_directory_refuses_overwrite() {
    let dir = temp_dir("unreadable");
    let store = HostStore::open(&dir);
    std::fs::create_dir(dir.join("hosts.json")).unwrap();
    assert!(store.all().is_empty());
    let err = store.save_records::<HostEntry>(&[]).unwrap_err();
    assert!(err.to_string().contains("left as it is"), "{err}");
    assert!(store.import_legacy_if_needed(Vec::new()).is_err());
    assert!(!store.has_migrated_legacy());
    assert!(dir.join("hosts.json").is_dir());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn host_store_upsert_preserves_all_desktop_shared_fields_and_pinned_endpoint() {
    let dir = temp_dir("desktop-preserve");
    let store = HostStore::open(&dir);

    let mut desktop = HostEntry::default();
    desktop.shared.unique_id = "pc-desk".into();
    desktop.shared.name = "Rig".into();
    desktop.shared.alias = Some("Main Rig".into());
    desktop.shared.address = "192.168.1.5".into();
    desktop.shared.http_port = 47989;
    desktop.shared.https_port = 47984;
    desktop.shared.other_addresses = vec!["10.0.0.5".into()];
    desktop.shared.other_http_ports = vec![48000];
    desktop.shared.pinned_address = Some("10.0.0.5".into());
    desktop.shared.remote_address = Some("203.0.113.5".into());
    desktop.shared.wake_macs = vec!["aa:bb:cc:dd:ee:02".into()];
    desktop.shared.wake_broadcasts = vec!["10.0.0.255".into()];
    desktop.shared.own_wake_macs = vec!["aa:bb:cc:dd:ee:03".into()];
    desktop.shared.quit_on_leave.insert("uuid:game-1".into());
    desktop.shared.notices.insert("PYROWAVE_FALLBACK".into());
    desktop.shared.full_chroma.modes = Some(3);
    desktop.shared.full_chroma.refused = true;
    desktop.shared.internet_play = Some("{\"version\":1}".into());
    desktop.pinned_endpoint = EndpointSlot::new("10.0.0.5", 48000);
    desktop.extra.insert("pace".into(), serde_json::json!({"pace_mbps": 150}));
    desktop.extra.insert("future_field".into(), serde_json::json!("kept"));
    assert!(store.upsert(desktop).unwrap());

    let mut android_update = HostEntry::default();
    android_update.shared.unique_id = "pc-desk".into();
    android_update.shared.name = "Rig-Renamed".into();
    android_update.shared.address = "192.168.1.9".into();
    android_update.shared.http_port = 47989;
    android_update.local_endpoint = EndpointSlot::new("192.168.1.9", 47989);
    android_update.ipv6_endpoint = EndpointSlot::new("2001:db8::9", 47989);
    assert!(store.upsert(android_update.clone()).unwrap());
    assert!(!store.upsert(android_update).unwrap());

    let loaded = store.get_by_uuid("pc-desk").unwrap();
    assert_eq!(loaded.shared.name, "Rig-Renamed");
    assert_eq!(loaded.local_endpoint, EndpointSlot::new("192.168.1.9", 47989));
    assert_eq!(loaded.ipv6_endpoint, EndpointSlot::new("2001:db8::9", 47989));
    assert_eq!(loaded.shared.alias.as_deref(), Some("Main Rig"));
    assert_eq!(loaded.shared.https_port, 47984);
    assert_eq!(loaded.shared.other_addresses, vec!["10.0.0.5"]);
    assert_eq!(loaded.shared.other_http_ports, vec![48000]);
    assert_eq!(loaded.shared.pinned_address.as_deref(), Some("10.0.0.5"));
    assert_eq!(loaded.shared.wake_macs, vec!["aa:bb:cc:dd:ee:02"]);
    assert_eq!(loaded.shared.wake_broadcasts, vec!["10.0.0.255"]);
    assert_eq!(loaded.shared.own_wake_macs, vec!["aa:bb:cc:dd:ee:03"]);
    assert!(loaded.shared.quit_on_leave.contains("uuid:game-1"));
    assert!(loaded.shared.notices.contains("PYROWAVE_FALLBACK"));
    assert_eq!(loaded.shared.full_chroma.modes, Some(3));
    assert!(loaded.shared.full_chroma.refused);
    assert_eq!(loaded.shared.internet_play.as_deref(), Some("{\"version\":1}"));
    assert_eq!(loaded.pinned_endpoint, EndpointSlot::new("10.0.0.5", 48000));
    assert_eq!(loaded.extra.get("future_field"), Some(&serde_json::json!("kept")));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn host_meta_store_dedups_capabilities_and_imports_once() {
    let dir = temp_dir("meta");
    let meta = HostMetaStore::open(&dir);
    let info = ServerInfo::parse(crate::http::serverinfo::tests::RUBYLIGHT, 47989).unwrap();
    let json = info.capabilities.to_versioned_json().unwrap();
    assert!(meta.save_capabilities("pc", &json).unwrap());
    assert!(!meta.save_capabilities("pc", &json).unwrap());

    let mut legacy_notices = BTreeMap::new();
    legacy_notices.insert("pc".into(), serde_json::json!("PYROWAVE_FALLBACK,PACKED_UNAVAILABLE"));
    assert!(meta.import_legacy_source("HostNotices", &legacy_notices).unwrap());
    assert!(!meta.claim_notice("pc", "PYROWAVE_FALLBACK").unwrap());
    assert!(meta.forget("pc").unwrap());
    // Re-importing the same source does not resurrect forgotten notices.
    assert!(!meta.import_legacy_source("HostNotices", &legacy_notices).unwrap());
    assert!(meta.claim_notice("pc", "PYROWAVE_FALLBACK").unwrap());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn identity_store_protects_corrupt_files_and_imports_legacy_credentials() {
    let dir = temp_dir("identity");
    let store = IdentityStore::open(&dir);
    assert_eq!(store.identity().unwrap(), None);

    let damaged = br#"{"unique_id":"kept","certificate_pem":"#;
    std::fs::write(dir.join("identity.json"), damaged).unwrap();
    assert!(store.identity().is_err());
    assert!(dir.join("identity.json.bad").is_file());
    assert!(
        store
            .save_identity(&IdentityFile {
                unique_id: "new".into(),
                certificate_pem: "c".into(),
                private_key_pem: "k".into(),
            })
            .is_err()
    );
    assert_eq!(std::fs::read(dir.join("identity.json")).unwrap(), damaged);
    let _ = std::fs::remove_dir_all(&dir);

    // Fresh store imports valid legacy credentials cleanly and preserves pairing identity.
    let dir2 = temp_dir("identity-import");
    let store2 = IdentityStore::open(&dir2);
    let legacy = ClientIdentity::generate().unwrap();
    let loaded = store2
        .load_or_create(
            Some("0123456789abcdef"),
            Some(legacy.certificate_pem()),
            Some(legacy.private_key_pkcs8()),
        )
        .unwrap();
    assert_eq!(loaded.unique_id, "0123456789abcdef");
    assert_eq!(loaded.certificate_der, legacy.certificate_der());
    assert!(loaded.persisted);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir2.join("identity.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let _ = std::fs::remove_dir_all(&dir2);
}
