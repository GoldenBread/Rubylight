//! Unit tests for `nightjar_core::wol` planning, UDP send semantics, and `WakeTargetsStore`.

use super::*;
use std::collections::BTreeMap;

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("nightjar-wol-{tag}-{nanos}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn plan_and_send_handle_dns_fallback_and_best_effort_v6() {
    let links = Links {
        broadcasts: [std::net::Ipv4Addr::new(192, 168, 1, 255)].into(),
        ipv6: [5].into(),
    };
    let host = WakeHost {
        mac: Some("02-7C-16-B4-13-4F"),
        endpoints: &[("127.0.0.1".into(), 47989)],
        wake_macs: &[],
        wake_broadcasts: &["10.0.0.255".into()],
        own_wake_macs: &["0a:00:27:00:00:05".into()],
        fallback_http_port: 47989,
    };
    let p = plan(&host, &links).unwrap();
    assert_eq!(p.payloads.len(), 2);
    assert!(p.destinations.contains(&"127.0.0.1:47998".parse().unwrap()));
    assert!(p.destinations.contains(&"10.0.0.255:9".parse().unwrap()));

    let res = send(&p, |_, dest| {
        if dest.is_ipv6() {
            Err(io::Error::other("no v6"))
        } else {
            Ok(())
        }
    });
    assert!(res.is_ok());
}

#[test]
fn plan_preserves_scoped_ipv6_endpoints() {
    let host = WakeHost {
        mac: Some("02:7c:16:b4:13:4f"),
        endpoints: &[
            ("fe80::1%wlan0".into(), 47989),
            ("[fe80::2%3]".into(), 47989),
        ],
        wake_macs: &[],
        wake_broadcasts: &[],
        own_wake_macs: &[],
        fallback_http_port: 47989,
    };
    let p = plan(&host, &Links::default()).unwrap();
    assert!(p.destinations.iter().any(|d| matches!(
        d,
        SocketAddr::V6(v6) if *v6.ip() == "fe80::1".parse::<std::net::Ipv6Addr>().unwrap() && v6.port() == 47998
    )));
    assert!(p.destinations.iter().any(|d| matches!(
        d,
        SocketAddr::V6(v6) if *v6.ip() == "fe80::2".parse::<std::net::Ipv6Addr>().unwrap() && v6.scope_id() == 3 && v6.port() == 47998
    )));
}

#[test]
fn wake_targets_store_persists_isolates_manual_and_imports_idempotently() {
    let dir = temp_dir("store");
    let store = WakeTargetsStore::open(&dir);

    store
        .store_manual("pc", &["0a:00:27:00:00:05".into()])
        .unwrap();
    let b = vec!["192.168.004.255".into()];
    store
        .store_raw("pc", Some("02:7c:16:b4:13:4f"), [("02:7c:16:b4:13:4f", b.as_slice())])
        .unwrap();

    let loaded = store.load("pc");
    assert_eq!(
        loaded.macs,
        vec!["02:7c:16:b4:13:4f", "0a:00:27:00:00:05"]
    );
    assert_eq!(loaded.broadcasts, vec!["192.168.4.255"]);
    assert!(store.has_mac("pc", None));

    // Legacy import does not overwrite existing host entry.
    let mut legacy_hosts = BTreeMap::new();
    legacy_hosts.insert(
        "pc".into(),
        WakeTargetEntry {
            macs: vec!["aa:bb:cc:dd:ee:ff".into()],
            broadcasts: vec![],
        },
    );
    legacy_hosts.insert(
        "legacy".into(),
        WakeTargetEntry {
            macs: vec!["11:22:33:44:55:66".into()],
            broadcasts: vec!["10.1.0.255".into()],
        },
    );
    assert!(store.import_legacy(legacy_hosts.clone(), BTreeMap::new()).unwrap());
    assert!(!store.import_legacy(legacy_hosts, BTreeMap::new()).unwrap());
    assert_eq!(store.load("pc").macs[0], "02:7c:16:b4:13:4f");
    assert_eq!(store.load("legacy").macs, vec!["11:22:33:44:55:66"]);

    store.forget("pc").unwrap();
    assert_eq!(store.load("pc").macs, vec!["0a:00:27:00:00:05"]);
    store.forget_host("pc").unwrap();
    assert!(store.load("pc").macs.is_empty());

    let _ = std::fs::remove_dir_all(&dir);
}
