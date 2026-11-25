//! Unit tests for `nightjar_core::hosts`: `HostStore`, `HostMetaStore`, `IdentityStore`,
//! atomic `0o600` writes, `.bad` backup, unreadable refusal, DER<->PEM, and migration idempotency.

use super::*;
use crate::http::identity::tests::fixture;
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

pub(super) fn read_http_head(sock: &mut std::net::TcpStream) -> Vec<u8> {
    use std::io::Read;
    let _ = sock.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while head.len() < 8192 && !head.ends_with(b"\r\n\r\n") {
        match sock.read(&mut byte) {
            Ok(0) | Err(_) => break,
            Ok(_) => head.push(byte[0]),
        }
    }
    head
}

pub(super) fn reply_http_xml(sock: &mut std::net::TcpStream, body: &str) {
    use std::io::{Read, Write};
    let head = read_http_head(sock);
    assert!(head.ends_with(b"\r\n\r\n"), "incomplete HTTP request head");
    let reply = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    sock.write_all(reply.as_bytes()).unwrap();
    sock.flush().unwrap();
    let _ = sock.shutdown(std::net::Shutdown::Write);
    let mut drain = [0u8; 64];
    while matches!(sock.read(&mut drain), Ok(n) if n > 0) {}
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

#[test]
fn poll_host_tries_internet_play_mapped_ipv4_while_state_is_unknown_without_punching() {
    use super::manager::HostManager;
    use nightjar_policy::hosts::HostReachState;
    use std::net::TcpListener;
    use std::sync::Arc;

    let dir = temp_dir("mapped-unknown");
    let mgr = HostManager::open(&dir, Some(Arc::new(fixture()))).unwrap();
    mgr.set_stun_resolver(Arc::new(|_| None));

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let srv = std::thread::spawn(move || {
        let (mut sock, _) = listener.accept().unwrap();
        reply_http_xml(
            &mut sock,
            r#"<root status_code="200"><hostname>WanPC</hostname><uniqueid>pc-mapped</uniqueid><state>MJ_SERVER_FREE</state><PairStatus>0</PairStatus><HttpsPort>47984</HttpsPort><ExternalPort>47989</ExternalPort></root>"#,
        );
    });

    let mut entry = HostEntry::default();
    entry.shared.unique_id = "pc-mapped".into();
    entry.shared.name = "WanPC".into();
    entry.local_endpoint = EndpointSlot::new("127.0.0.2", 1);
    entry.shared.internet_play = Some(format!(
        r#"{{"version":1,"enabled":false,"externalIpv4":"127.0.0.1","mappedPorts":true,"ports":{{"http":{port},"https":47984,"rtsp":48010,"video":47998,"audio":48000,"control":47999,"mic":48002}}}}"#
    ));
    entry.sync_shared_from_slots();
    mgr.upsert_tuple(entry);

    // State starts at Unknown, where punches_now(...) is false; mapped IPv4 from internet_play
    // must still be probed and bring the host Online without punching.
    assert_eq!(mgr.get_snapshot("pc-mapped").unwrap().state, HostReachState::Unknown);
    assert!(mgr.poll_host("pc-mapped", true));
    let snap = mgr.get_snapshot("pc-mapped").unwrap();
    assert_eq!(snap.state, HostReachState::Online);
    assert_eq!(snap.active_endpoint, EndpointSlot::new("127.0.0.1", port));
    assert_eq!(snap.internet_path.as_deref(), Some("mapped"));

    srv.join().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn poll_host_sets_another_certificate_on_cert_mismatch_and_plain_http_does_not_clear_cert() {
    use super::manager::HostManager;
    use super::poll::try_poll_ip_with_client;
    use crate::http::{Error, HostClient};
    use nightjar_policy::hosts::{CandidateSlot, ProbeCandidate};
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    use std::net::TcpListener;
    use std::sync::Arc;

    let server_id = fixture();
    let pinned_id = ClientIdentity::generate().unwrap();
    let server_cfg = Arc::new(
        rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(server_id.certificate_der().to_vec())],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
                server_id.private_key_pkcs8().to_vec(),
            )),
        )
        .unwrap(),
    );

    let tls_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let https_port = tls_listener.local_addr().unwrap().port();
    let tls_srv = std::thread::spawn(move || {
        let (sock, _) = tls_listener.accept().unwrap();
        let _ = sock.set_read_timeout(Some(std::time::Duration::from_secs(5)));
        let mut tls = rustls::StreamOwned::new(
            rustls::ServerConnection::new(server_cfg).unwrap(),
            sock,
        );
        while tls.conn.is_handshaking() {
            if tls.conn.complete_io(&mut tls.sock).is_err() {
                break;
            }
        }
    });

    let http_disc_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let http_disc_port = http_disc_listener.local_addr().unwrap().port();
    let http_disc_srv = std::thread::spawn(move || {
        let (mut sock, _) = http_disc_listener.accept().unwrap();
        reply_http_xml(
            &mut sock,
            &format!(
                r#"<root status_code="200"><hostname>OtherPC</hostname><uniqueid>pc-cert</uniqueid><state>MJ_SERVER_FREE</state><PairStatus>0</PairStatus><HttpsPort>{https_port}</HttpsPort></root>"#
            ),
        );
    });

    let dir = temp_dir("cert-mismatch");
    let mgr = HostManager::open(&dir, Some(Arc::new(fixture()))).unwrap();
    mgr.set_stun_resolver(Arc::new(|_| None));
    let mut entry = HostEntry::default();
    entry.shared.unique_id = "pc-cert".into();
    entry.shared.https_port = https_port;
    entry.shared.server_certificate = Some(pinned_id.certificate_pem().to_owned());
    entry.local_endpoint = EndpointSlot::new("127.0.0.1", http_disc_port);
    entry.sync_shared_from_slots();
    mgr.upsert_tuple(entry);

    assert!(mgr.poll_host("pc-cert", true));
    let snap = mgr.get_snapshot("pc-cert").unwrap();
    assert!(snap.another_certificate);
    assert!(snap.entry.shared.server_certificate.is_some());
    http_disc_srv.join().unwrap();
    tls_srv.join().unwrap();

    // Also verify plain HTTP (InfoPath::Plain) with has_pinned_cert=true never triggers cert clear.
    let http_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let http_port = http_listener.local_addr().unwrap().port();
    let http_srv = std::thread::spawn(move || {
        let (mut sock, _) = http_listener.accept().unwrap();
        reply_http_xml(
            &mut sock,
            r#"<root status_code="200"><hostname>PlainPC</hostname><uniqueid>pc-plain</uniqueid><state>MJ_SERVER_FREE</state><PairStatus>0</PairStatus></root>"#,
        );
    });
    let unpinned = HostClient::new("127.0.0.1", http_port, Arc::new(fixture())).unwrap();
    let cand = ProbeCandidate {
        slot: CandidateSlot::Local,
        address: "127.0.0.1".into(),
        port: http_port,
        https_port: 0,
    };
    let win = try_poll_ip_with_client(&unpinned, &cand, Some("pc-plain"), true, false).unwrap();
    assert!(!win.should_clear_cert);
    assert!(!matches!(win.info.app_version, Some(ref v) if v == "bad"));
    let _ = Error::Cancelled;
    http_srv.join().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn apply_server_info_sets_local_ip_and_external_ip_preserving_127_guard() {
    use super::manager::{HostSnapshot, apply_server_info};
    use nightjar_policy::hosts::{CandidateSlot, HostReachState, ProbeCandidate};

    let mut snap = HostSnapshot {
        entry: HostEntry::default(),
        state: HostReachState::Online,
        active_endpoint: None,
        paired: false,
        another_certificate: false,
        running_game_id: 0,
        running_game_uuid: None,
        external_port: 0,
        unreachable: None,
        capabilities_json: None,
        alias: None,
        pinned_endpoint: None,
        internet_path: None,
        needs_vpn_stun: false,
        server_info: None,
    };
    let cand = ProbeCandidate {
        slot: CandidateSlot::Remote,
        address: "203.0.113.10".into(),
        port: 47989,
        https_port: 47984,
    };

    let info = ServerInfo {
        local_ip: Some("192.168.1.50".into()),
        external_ip: Some("198.51.100.42".into()),
        external_port: 48123,
        https_port: 47984,
        ..ServerInfo::parse(
            r#"<root status_code="200"><hostname>PC</hostname><uniqueid>u</uniqueid><state>MJ_SERVER_FREE</state><PairStatus>1</PairStatus></root>"#,
            47989,
        )
        .unwrap()
    };
    apply_server_info(&mut snap, &info, 47989, &cand);
    assert_eq!(snap.entry.local_endpoint, EndpointSlot::new("192.168.1.50", 47989));
    assert_eq!(snap.entry.remote_endpoint, EndpointSlot::new("198.51.100.42", 48123));

    // Loopback LocalIP (127.*) must never overwrite the real LAN endpoint.
    let mut loopback = info.clone();
    loopback.local_ip = Some("127.0.0.1".into());
    apply_server_info(&mut snap, &loopback, 47989, &cand);
    assert_eq!(snap.entry.local_endpoint, EndpointSlot::new("192.168.1.50", 47989));
}

#[test]
fn reset_network_state_respects_at_ms_guard_and_emits_snapshot() {
    use super::manager::HostManager;
    use nightjar_policy::hosts::HostReachState;
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    let dir = temp_dir("reset-guard");
    let mgr = HostManager::open(&dir, Some(Arc::new(fixture()))).unwrap();
    mgr.set_stun_resolver(Arc::new(|_| None));
    let emitted = Arc::new(Mutex::new(Vec::<HostReachState>::new()));
    let emitted_cb = Arc::clone(&emitted);
    mgr.set_listener(Some(Arc::new(move |snap| {
        emitted_cb.lock().unwrap().push(snap.state);
    })));

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let srv = std::thread::spawn(move || {
        let (mut sock, _) = listener.accept().unwrap();
        reply_http_xml(
            &mut sock,
            r#"<root status_code="200"><hostname>NetPC</hostname><uniqueid>pc-net</uniqueid><state>MJ_SERVER_FREE</state><PairStatus>0</PairStatus></root>"#,
        );
    });

    let mut entry = HostEntry::default();
    entry.shared.unique_id = "pc-net".into();
    entry.local_endpoint = EndpointSlot::new("127.0.0.1", port);
    mgr.upsert_tuple(entry);
    std::thread::sleep(std::time::Duration::from_millis(15));
    let before_poll_ms = mgr.now_ms();
    assert!(mgr.poll_host("pc-net", true));
    let after_poll_ms = mgr.now_ms() + 100;
    srv.join().unwrap();
    emitted.lock().unwrap().clear();

    // Older transition before the successful poll must NOT revert Online -> Unknown.
    mgr.reset_network_state(true, before_poll_ms);
    assert_eq!(mgr.get_snapshot("pc-net").unwrap().state, HostReachState::Online);
    assert!(emitted.lock().unwrap().is_empty());

    // Newer transition after the successful poll MUST revert Online -> Offline (available=false) and emit.
    mgr.reset_network_state(false, after_poll_ms);
    assert_eq!(mgr.get_snapshot("pc-net").unwrap().state, HostReachState::Offline);
    assert_eq!(*emitted.lock().unwrap(), vec![HostReachState::Offline]);
    let _ = std::fs::remove_dir_all(&dir);
}

fn spawn_stalled_http_server(
    count: usize,
) -> (
    u16,
    std::sync::mpsc::Receiver<()>,
    std::thread::JoinHandle<()>,
) {
    use std::io::Read;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
        for _ in 0..count {
            let (mut sock, _) = listener.accept().unwrap();
            let _ = read_http_head(&mut sock);
            let _ = entered_tx.send(());
            let mut buf = [0u8; 64];
            while matches!(sock.read(&mut buf), Ok(n) if n > 0) {}
        }
    });
    (port, entered_rx, handle)
}

#[test]
fn stop_polling_and_wait_cancel_stalled_single_and_parallel_polls_under_500ms() {
    use super::manager::HostManager;
    use nightjar_policy::hosts::HostReachState;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    let dir = temp_dir("stop-cancel");
    let mgr = HostManager::open(&dir, Some(Arc::new(fixture()))).unwrap();
    mgr.set_stun_resolver(Arc::new(|_| None));

    let (single_port, single_rx, single_srv) = spawn_stalled_http_server(1);
    let (local_port, local_rx, local_srv) = spawn_stalled_http_server(1);
    let (manual_port, manual_rx, manual_srv) = spawn_stalled_http_server(1);

    let mut single = HostEntry::default();
    single.shared.unique_id = "pc-single".into();
    single.local_endpoint = EndpointSlot::new("127.0.0.1", single_port);
    mgr.upsert_tuple(single);

    let mut multi = HostEntry::default();
    multi.shared.unique_id = "pc-multi".into();
    multi.local_endpoint = EndpointSlot::new("127.0.0.1", local_port);
    multi.manual_endpoint = EndpointSlot::new("127.0.0.1", manual_port);
    mgr.upsert_tuple(multi);

    mgr.start_polling();
    assert!(single_rx.recv_timeout(Duration::from_secs(5)).is_ok());
    assert!(local_rx.recv_timeout(Duration::from_secs(5)).is_ok());
    assert!(manual_rx.recv_timeout(Duration::from_secs(5)).is_ok());

    let t0 = Instant::now();
    mgr.stop_polling();
    assert!(
        t0.elapsed() < Duration::from_millis(500),
        "stop_polling blocked for {:?}",
        t0.elapsed()
    );

    let t1 = Instant::now();
    mgr.wait_for_polling_stopped();
    assert!(
        t1.elapsed() < Duration::from_millis(500),
        "wait_for_polling_stopped blocked for {:?}",
        t1.elapsed()
    );

    assert_ne!(
        mgr.get_snapshot("pc-single").unwrap().state,
        HostReachState::Offline
    );
    assert_ne!(
        mgr.get_snapshot("pc-multi").unwrap().state,
        HostReachState::Offline
    );

    single_srv.join().unwrap();
    local_srv.join().unwrap();
    manual_srv.join().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn stop_polling_followed_by_restart_and_remove_computer_do_not_stall_or_leak() {
    use super::manager::HostManager;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    let dir = temp_dir("restart-remove");
    let mgr = HostManager::open(&dir, Some(Arc::new(fixture()))).unwrap();
    mgr.set_stun_resolver(Arc::new(|_| None));

    let (restart_port, restart_rx, restart_srv) = spawn_stalled_http_server(2);
    let mut e1 = HostEntry::default();
    e1.shared.unique_id = "pc-restart".into();
    e1.local_endpoint = EndpointSlot::new("127.0.0.1", restart_port);
    mgr.upsert_tuple(e1);

    mgr.start_polling();
    assert!(restart_rx.recv_timeout(Duration::from_secs(5)).is_ok());
    let t0 = Instant::now();
    mgr.stop_polling();
    assert!(t0.elapsed() < Duration::from_millis(500));

    mgr.start_polling();
    assert!(restart_rx.recv_timeout(Duration::from_secs(5)).is_ok());
    let t1 = Instant::now();
    mgr.stop_polling();
    assert!(t1.elapsed() < Duration::from_millis(500));
    let t2 = Instant::now();
    mgr.wait_for_polling_stopped();
    assert!(t2.elapsed() < Duration::from_millis(500));
    restart_srv.join().unwrap();

    let (remove_port, remove_rx, remove_srv) = spawn_stalled_http_server(1);
    let mgr2 = HostManager::open(&dir, Some(Arc::new(fixture()))).unwrap();
    mgr2.set_stun_resolver(Arc::new(|_| None));
    let mut e2 = HostEntry::default();
    e2.shared.unique_id = "pc-remove".into();
    e2.local_endpoint = EndpointSlot::new("127.0.0.1", remove_port);
    mgr2.upsert_tuple(e2);

    mgr2.start_polling();
    assert!(remove_rx.recv_timeout(Duration::from_secs(5)).is_ok());
    let t3 = Instant::now();
    mgr2.remove_computer("pc-remove").unwrap();
    assert!(
        t3.elapsed() < Duration::from_millis(500),
        "remove_computer blocked for {:?}",
        t3.elapsed()
    );
    let t4 = Instant::now();
    mgr2.wait_for_polling_stopped();
    assert!(
        t4.elapsed() < Duration::from_millis(500),
        "wait_for_polling_stopped after remove_computer blocked for {:?}",
        t4.elapsed()
    );
    assert!(mgr2.get_snapshot("pc-remove").is_none());

    remove_srv.join().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn site_local_host_detects_private_literals_without_dns() {
    use super::poll::is_site_local_host;
    assert!(is_site_local_host("192.168.1.50"));
    assert!(is_site_local_host("10.0.0.5"));
    assert!(is_site_local_host("172.16.4.1"));
    assert!(is_site_local_host("[fd00::1]"));
    assert!(!is_site_local_host("203.0.113.10"));
    assert!(!is_site_local_host("definitely-invalid.invalid"));
}

#[test]
fn parallel_poll_direct_prefers_higher_precedence_winner_and_cancels_loser() {
    use super::client_pool::HostClientPool;
    use super::poll::parallel_poll_direct;
    use nightjar_policy::hosts::{CandidateSlot, ProbeCandidate};
    use std::io::Read;
    use std::net::TcpListener;
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    let local_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let local_port = local_listener.local_addr().unwrap().port();
    let manual_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let manual_port = manual_listener.local_addr().unwrap().port();

    let (accepted_tx, accepted_rx) = mpsc::channel();
    let (manual_ready_tx, manual_ready_rx) = mpsc::channel();
    let local_srv = std::thread::spawn(move || {
        let (mut sock, _) = local_listener.accept().unwrap();
        // Wait until the slower manual candidate is demonstrably mid-request so CPU scheduling
        // cannot cancel `nj-poll-1` before it connects.
        let _ = manual_ready_rx.recv_timeout(Duration::from_secs(2));
        reply_http_xml(
            &mut sock,
            r#"<root status_code="200"><hostname>LocalPC</hostname><uniqueid>pc-1</uniqueid><state>MJ_SERVER_FREE</state><PairStatus>0</PairStatus><HttpsPort>47984</HttpsPort><ExternalPort>47989</ExternalPort></root>"#,
        );
    });

    let manual_srv = std::thread::spawn(move || {
        let (mut sock, _) = manual_listener.accept().unwrap();
        let _ = accepted_tx.send(());
        let _ = read_http_head(&mut sock);
        let _ = manual_ready_tx.send(());
        let mut buf = [0u8; 64];
        while matches!(sock.read(&mut buf), Ok(n) if n > 0) {}
    });

    let pool = Arc::new(HostClientPool::new(Arc::new(fixture())));
    let candidates = vec![
        ProbeCandidate {
            slot: CandidateSlot::Local,
            address: "127.0.0.1".into(),
            port: local_port,
            https_port: 0,
        },
        ProbeCandidate {
            slot: CandidateSlot::Manual,
            address: "127.0.0.1".into(),
            port: manual_port,
            https_port: 0,
        },
    ];
    let outcome = parallel_poll_direct(&pool, &candidates, Some("pc-1"), None, true);
    let winner = outcome.winner.expect("expected local winner");
    assert_eq!(winner.candidate.slot, CandidateSlot::Local);
    assert_eq!(winner.info.hostname, "LocalPC");
    assert!(accepted_rx.recv_timeout(Duration::from_secs(2)).is_ok());

    local_srv.join().unwrap();
    manual_srv.join().unwrap();
}

#[test]
fn desktop_poll_addresses_and_away_address_preserve_order_and_internet_paths() {
    use super::poll::{away_address, desktop_poll_addresses};
    use crate::http::InternetPlay;
    let shared = SharedHostFields {
        address: "192.168.1.5".into(),
        remote_address: Some("203.0.113.10".into()),
        other_addresses: vec!["203.0.113.10".into(), "10.0.0.5".into()],
        ..Default::default()
    };
    let reply = InternetPlay::parse(
        r#"{"version":1,"enabled":true,"mappedPorts":true,"externalIpv4":"198.51.100.7","ipv6":["2001:db8::9"],"ports":{"http":47989,"https":47984,"rtsp":48010,"video":47998,"audio":48000,"control":47999,"mic":48002},"endpoints":[],"nat":"unknown"}"#,
    )
    .unwrap();
    assert_eq!(
        desktop_poll_addresses(&shared, Some(&reply)),
        vec!["192.168.1.5", "10.0.0.5", "2001:db8::9", "203.0.113.10", "198.51.100.7"]
    );
    assert!(away_address(&shared, Some(&reply), "203.0.113.10"));
    assert!(away_address(&shared, Some(&reply), "2001:db8::9"));
    assert!(away_address(&shared, Some(&reply), "127.78.0.5"));
    assert!(!away_address(&shared, Some(&reply), "192.168.1.5"));
}

#[test]
fn poll_saves_only_changes_and_remove_tombstones_cleanly() {
    use super::manager::HostManager;
    use std::net::TcpListener;
    use std::sync::Arc;

    let dir = temp_dir("saves");
    let mgr = HostManager::open(&dir, Some(Arc::new(fixture()))).unwrap();
    mgr.set_stun_resolver(Arc::new(|_| None));

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let srv = std::thread::spawn(move || {
        for _ in 0..2 {
            let (mut sock, _) = listener.accept().unwrap();
            reply_http_xml(
                &mut sock,
                r#"<root status_code="200"><hostname>StudioPC</hostname><uniqueid>pc-save</uniqueid><state>MJ_SERVER_FREE</state><PairStatus>0</PairStatus><HttpsPort>47984</HttpsPort><ExternalPort>47989</ExternalPort></root>"#,
            );
        }
    });

    let mut initial = HostEntry::default();
    initial.shared.unique_id = "pc-save".into();
    initial.shared.name = "Initial".into();
    initial.local_endpoint = EndpointSlot::new("127.0.0.1", port);
    initial.sync_shared_from_slots();
    mgr.store().upsert(initial.clone()).unwrap();
    mgr.reload_from_store();

    assert!(mgr.poll_host("pc-save", true), "first poll updates name + online state");
    assert!(!mgr.poll_host("pc-save", true), "identical second poll reports no change");

    mgr.remove_computer("pc-save").unwrap();
    mgr.wait_for_polling_stopped();
    assert!(mgr.get_snapshot("pc-save").is_none());
    assert!(mgr.store().get_by_uuid("pc-save").is_none());
    assert!(!mgr.poll_host("pc-save", true));

    srv.join().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn start_and_stop_polling_join_worker_threads_cleanly() {
    use super::manager::HostManager;
    use std::sync::Arc;

    let dir = temp_dir("lifecycle");
    let mgr = HostManager::open(&dir, Some(Arc::new(fixture()))).unwrap();
    mgr.set_stun_resolver(Arc::new(|_| None));
    let mut entry = HostEntry::default();
    entry.shared.unique_id = "pc-bg".into();
    entry.local_endpoint = EndpointSlot::new("127.0.0.1", 1);
    mgr.upsert_tuple(entry);

    mgr.start_polling();
    mgr.stop_polling();
    mgr.wait_for_polling_stopped();
    let _ = std::fs::remove_dir_all(&dir);
}
