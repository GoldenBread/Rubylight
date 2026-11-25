//! Regression tests for `HostManager`, `HostStore`, `PollCancel`, pairing checks,
//! tombstone races, poll cancellation on network reset, and 3+ endpoint probing (#164).

use super::client_pool::HostClientPool;
use super::manager::HostManager;
use super::pairing::{PairSecret, execute_pair_checked};
use super::poll::PollCancel;
use super::tests::{read_http_head, reply_http_xml};
use super::*;
use crate::http::identity::tests::fixture;
use crate::http::{ClientIdentity, HostClient};
use nightjar_policy::hosts::HostReachState;
use std::net::TcpListener;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("nightjar-hosts-reg-{tag}-{nanos}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn host_manager_clear_mutations_persist_across_reload() {
    let dir = temp_dir("clear-persist");
    let cert_pem = fixture().certificate_pem().to_owned();
    let mgr = HostManager::open(&dir, Some(Arc::new(fixture()))).unwrap();

    let mut entry = HostEntry::default();
    entry.shared.unique_id = "pc-clear".into();
    entry.shared.name = "Rig".into();
    entry.shared.alias = Some("Office".into());
    entry.shared.server_certificate = Some(cert_pem);
    entry.shared.internet_play = Some(r#"{"version":1}"#.into());
    entry.local_endpoint = EndpointSlot::new("192.168.1.20", 47989);
    entry.pinned_endpoint = EndpointSlot::new("100.64.0.20", 47989);
    entry.extra.insert("future".into(), serde_json::json!("preserved"));
    entry.sync_shared_from_slots();
    mgr.store().upsert(entry.clone()).unwrap();
    mgr.upsert_tuple(entry);

    mgr.pin_address("pc-clear", None).unwrap();
    mgr.rename_computer("pc-clear", None).unwrap();
    mgr.forget_certificate("pc-clear").unwrap();
    mgr.import_internet_play("pc-clear", None).unwrap();
    drop(mgr);

    let reopened = HostManager::open(&dir, Some(Arc::new(fixture()))).unwrap();
    let snap = reopened.get_snapshot("pc-clear").expect("host still saved");
    assert_eq!(snap.pinned_endpoint, None);
    assert_eq!(snap.entry.pinned_endpoint, None);
    assert_eq!(snap.entry.shared.pinned_address, None);
    assert_eq!(snap.alias, None);
    assert_eq!(snap.entry.shared.alias, None);
    assert_eq!(snap.entry.shared.server_certificate, None);
    assert_eq!(snap.entry.shared.internet_play, None);
    assert_eq!(
        snap.entry.extra.get("future"),
        Some(&serde_json::json!("preserved"))
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn stale_poll_cancel_after_pool_return_does_not_abort_subsequent_checkout() {
    let pool = HostClientPool::new(Arc::new(fixture()));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let srv = std::thread::spawn(move || {
        for _ in 0..2 {
            let (mut sock, _) = listener.accept().unwrap();
            reply_http_xml(
                &mut sock,
                r#"<root status_code="200"><hostname>Box</hostname><uniqueid>pc-pool</uniqueid><state>MJ_SERVER_FREE</state><PairStatus>0</PairStatus><HttpsPort>47984</HttpsPort></root>"#,
            );
        }
    });

    let old_cancel = PollCancel::new();
    let (first, generation) = pool.take("127.0.0.1", port, 47984, None).unwrap();
    {
        let _guard = old_cancel.register(first.canceller());
        assert!(first.server_info_http(true).is_ok());
    }
    pool.give_back("127.0.0.1", port, None, generation, first);

    // Borrow the same pooled client again, then cancel the OLD scope: must not cancel `second`.
    let (second, generation2) = pool.take("127.0.0.1", port, 47984, None).unwrap();
    old_cancel.cancel();
    assert!(
        second.server_info_http(true).is_ok(),
        "cancelling an already-returned poll scope must not poison the next borrower"
    );
    pool.give_back("127.0.0.1", port, None, generation2, second);
    srv.join().unwrap();
}

#[test]
fn execute_pair_checked_does_not_trust_unauthenticated_http_pair_status() {
    use std::sync::atomic::{AtomicBool, Ordering};

    // Start an HTTPS server presenting a DIFFERENT certificate than the one pinned on `client`,
    // forcing `server_info_with_cert_check` to fall back to HTTP with `InfoPath::CertificateMismatch`.
    let other_host = ClientIdentity::generate().unwrap();
    let tls_cfg = Arc::new(
        rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![rustls::pki_types::CertificateDer::from(
                other_host.certificate_der().to_vec(),
            )],
            rustls::pki_types::PrivateKeyDer::Pkcs8(rustls::pki_types::PrivatePkcs8KeyDer::from(
                other_host.private_key_pkcs8().to_vec(),
            )),
        )
        .unwrap(),
    );
    let https_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let https_port = https_listener.local_addr().unwrap().port();
    let https_srv = std::thread::spawn(move || {
        if let Ok((mut tcp, _)) = https_listener.accept()
            && let Ok(mut conn) = rustls::ServerConnection::new(tls_cfg)
        {
            while conn.is_handshaking() {
                if conn.complete_io(&mut tcp).is_err() {
                    break;
                }
            }
        }
    });

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let saw_pair = Arc::new(AtomicBool::new(false));
    let saw_pair_srv = Arc::clone(&saw_pair);
    let srv = std::thread::spawn(move || {
        let (mut sock1, _) = listener.accept().unwrap();
        reply_http_xml(
            &mut sock1,
            &format!(
                r#"<root status_code="200"><hostname>Spoof</hostname><appversion>7.1.431.0</appversion><uniqueid>pc-spoof</uniqueid><state>MJ_SERVER_FREE</state><PairStatus>1</PairStatus><HttpsPort>{https_port}</HttpsPort></root>"#
            ),
        );
        // After refusing the unauthenticated PairStatus=1 shortcut, pairing attempts /pair.
        let (mut sock2, _) = listener.accept().unwrap();
        let head = read_http_head(&mut sock2);
        if String::from_utf8_lossy(&head).contains("/pair?") {
            saw_pair_srv.store(true, Ordering::SeqCst);
        }
        use std::io::{Read, Write};
        let body = r#"<root status_code="200"><paired>0</paired></root>"#;
        let reply = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = sock2.write_all(reply.as_bytes());
        let _ = sock2.flush();
        let _ = sock2.shutdown(std::net::Shutdown::Write);
        let mut drain = [0u8; 64];
        while matches!(sock2.read(&mut drain), Ok(n) if n > 0) {}
    });

    let identity = Arc::new(fixture());
    let pinned_der = identity.certificate_der().to_vec();
    let client = HostClient::new("127.0.0.1", port, identity)
        .unwrap()
        .with_https_port(https_port);
    client.pin_server_certificate_der(Some(pinned_der));
    let outcome = execute_pair_checked(
        &client,
        &PairSecret::Pin("1234".into()),
        true,
        false,
        &[],
    );
    assert!(
        outcome.is_err(),
        "plain HTTP PairStatus=1 after cert mismatch must never short-circuit as already paired: {outcome:?}"
    );
    assert!(
        saw_pair.load(Ordering::SeqCst),
        "must fall through to /pair when serverinfo came over InfoPath::CertificateMismatch; outcome={outcome:?}"
    );
    srv.join().unwrap();
    https_srv.join().unwrap();
}

#[test]
fn removed_host_is_not_resurrected_by_mutations() {
    let dir = temp_dir("tombstone");
    let mgr = HostManager::open(&dir, Some(Arc::new(fixture()))).unwrap();

    let mut entry = HostEntry::default();
    entry.shared.unique_id = "pc-gone".into();
    entry.shared.name = "Gone".into();
    entry.local_endpoint = EndpointSlot::new("192.168.1.99", 47989);
    entry.sync_shared_from_slots();
    mgr.store().upsert(entry.clone()).unwrap();
    mgr.upsert_tuple(entry);

    mgr.remove_computer("pc-gone").unwrap();
    mgr.pin_address("pc-gone", EndpointSlot::new("192.168.1.99", 47989))
        .unwrap();
    mgr.rename_computer("pc-gone", Some("Zombie")).unwrap();
    mgr.set_remote_endpoint("pc-gone", EndpointSlot::new("203.0.113.5", 47989))
        .unwrap();
    mgr.forget_certificate("pc-gone").unwrap();
    mgr.import_internet_play("pc-gone", Some(r#"{"version":1}"#))
        .unwrap();

    assert!(mgr.get_snapshot("pc-gone").is_none());
    assert!(mgr.store().get_by_uuid("pc-gone").is_none());
    assert_eq!(mgr.meta().get_alias("pc-gone"), None);
    assert_eq!(mgr.meta().get_pinned("pc-gone"), None);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn reset_network_state_and_invalidate_cancel_blocked_poll_promptly() {
    for use_reset in [true, false] {
        let dir = temp_dir(if use_reset { "cancel-reset" } else { "cancel-inv" });
        let mgr = HostManager::open(&dir, Some(Arc::new(fixture()))).unwrap();
        mgr.set_stun_resolver(Arc::new(|_| None));

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (accepted_tx, accepted_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let srv = std::thread::spawn(move || {
            let (sock, _) = listener.accept().unwrap();
            let _ = accepted_tx.send(());
            // Hold the TCP socket open without answering until the test finishes.
            let _ = release_rx.recv_timeout(Duration::from_secs(5));
            drop(sock);
        });

        let mut entry = HostEntry::default();
        entry.shared.unique_id = "pc-stall".into();
        entry.shared.name = "Stall".into();
        entry.local_endpoint = EndpointSlot::new("127.0.0.1", port);
        entry.sync_shared_from_slots();
        mgr.upsert_tuple(entry);

        let poller_mgr = mgr.clone();
        let poller = std::thread::spawn(move || poller_mgr.poll_host("pc-stall", true));
        accepted_rx
            .recv_timeout(Duration::from_secs(3))
            .expect("poll connected to stalling listener");

        let start = Instant::now();
        if use_reset {
            mgr.reset_network_state(true, mgr.now_ms() + 1);
        } else {
            mgr.invalidate_state("pc-stall");
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_secs(2),
            "should cancel in-flight poll promptly instead of stalling ({elapsed:?})"
        );
        let _ = poller.join();
        let _ = release_tx.send(());
        let _ = srv.join();
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn three_plus_endpoints_preserved_in_other_addresses_and_probed() {
    let dir = temp_dir("three-endpoints");
    let mgr = HostManager::open(&dir, Some(Arc::new(fixture()))).unwrap();
    mgr.set_stun_resolver(Arc::new(|_| None));

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let srv = std::thread::spawn(move || {
        let (mut sock, _) = listener.accept().unwrap();
        reply_http_xml(
            &mut sock,
            r#"<root status_code="200"><hostname>MultiHome</hostname><uniqueid>pc-multi</uniqueid><state>MJ_SERVER_FREE</state><PairStatus>0</PairStatus><HttpsPort>47984</HttpsPort><ExternalPort>47989</ExternalPort></root>"#,
        );
    });

    // First record has a working endpoint (`127.0.0.1`) in `manual_endpoint` plus LAN & WAN.
    let mut first = HostEntry::default();
    first.shared.unique_id = "pc-multi".into();
    first.shared.name = "MultiHome".into();
    first.shared.http_port = port;
    first.local_endpoint = EndpointSlot::new("192.0.2.10", port);
    first.manual_endpoint = EndpointSlot::new("127.0.0.1", port);
    first.remote_endpoint = EndpointSlot::new("198.51.100.10", port);
    first.sync_shared_from_slots();
    mgr.upsert_tuple(first);

    // Updating `manual_endpoint` to a Tailscale IP must preserve `127.0.0.1` in `other_addresses`.
    let mut update = HostEntry::default();
    update.shared.unique_id = "pc-multi".into();
    update.shared.http_port = port;
    update.manual_endpoint = EndpointSlot::new("100.64.10.20", port);
    let merged = mgr.upsert_tuple(update);
    assert!(
        merged
            .entry
            .shared
            .other_addresses
            .contains(&"127.0.0.1".to_owned()),
        "displaced endpoint must be preserved in other_addresses: {:?}",
        merged.entry.shared.other_addresses
    );

    // Polling must probe `other_addresses` and reach `127.0.0.1:port` even though the 3 primary
    // slots (LAN, Tailscale manual, WAN) are unreachable.
    assert!(mgr.poll_host("pc-multi", true));
    let online = mgr.get_snapshot("pc-multi").unwrap();
    assert_eq!(online.state, HostReachState::Online);
    assert_eq!(
        online.active_endpoint,
        EndpointSlot::new("127.0.0.1", port)
    );

    srv.join().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
