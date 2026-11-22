//! Rubylight native API v1 interop between `butterpollo_core::native` and `nightjar_core::http::HostClient`.
mod common;

use butterpollo_core::native::{self, SessionSnapshot};
use butterpollo_core::state::{App, Client};
use butterpollo_core::{crypto, native_api as host_api};
use common::{client_identity, host_identity};
use nightjar_core::http::HostClient;
use nightjar_core::http::native_api::library::Library;
use nightjar_core::http::native_api::radeon::SettingsWrite;
use nightjar_core::http::native_api::session::{ActionId, Owner, State};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

const FIXTURES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../client/protocol/native-api/tests/fixtures"
);

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!("{FIXTURES}/{name}.json")).unwrap()
}

fn client(extra: Value) -> Client {
    let mut value = json!({"name": "Phone", "cert": "", "uuid": "me", "perm": (1u32 << 24) | (1 << 25) | (1 << 26)});
    value
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    serde_json::from_value(value).unwrap()
}

fn dota() -> App {
    serde_json::from_value(json!({
        "name": "Dota 2", "uuid": "c6f1a9a4-4b51-5f43-9d0b-7a2b1c2d3e4f",
        "launcher-source": "steam", "launcher-id": "570", "image-path": "dota.png"
    }))
    .unwrap()
}

fn suspended_snapshot() -> SessionSnapshot {
    SessionSnapshot {
        revision: 42,
        state: host_api::session::State::Suspended,
        game_running: true,
        owner: Some("me".into()),
        app: Some(host_api::session::SessionApp {
            install_id: "c6f1a9a4-4b51-5f43-9d0b-7a2b1c2d3e4f".into(),
            app_id: 1093052211,
            title: "Dota 2".into(),
        }),
        since_unix_ms: Some(1792303200000),
        suspended_until_unix_ms: Some(1792310400000),
        ..Default::default()
    }
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
struct Seen {
    method: String,
    path: String,
    body: Vec<u8>,
}

struct MockHost {
    port: u16,
    seen: Arc<Mutex<Vec<Seen>>>,
}

#[derive(Debug)]
struct AnyClientCertificate(Arc<rustls::crypto::CryptoProvider>);

impl rustls::server::danger::ClientCertVerifier for AnyClientCertificate {
    fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
        &[]
    }
    fn verify_client_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::server::danger::ClientCertVerified, rustls::Error> {
        Ok(rustls::server::danger::ClientCertVerified::assertion())
    }
    fn client_auth_mandatory(&self) -> bool {
        false
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

fn read_request(stream: &mut impl Read) -> Option<(String, String, Vec<u8>)> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte).ok()? == 0 {
            return None;
        }
        head.push(byte[0]);
    }
    let head = String::from_utf8(head).unwrap();
    let header = |name: &str| {
        head.lines()
            .find_map(|line| line.strip_prefix(name))
            .map(|v| v.trim().to_owned())
    };
    let mut words = head.split(' ');
    let (method, target) = (words.next()?.to_owned(), words.next()?.to_owned());
    let mut body = vec![0; header("Content-Length: ").map_or(0, |n| n.parse().unwrap())];
    stream.read_exact(&mut body).unwrap();
    Some((method, target, body))
}

fn serve(handler: impl Fn(&str, &str, &[u8]) -> (u16, String) + Send + 'static) -> MockHost {
    let identity: &crypto::Identity = host_identity();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = Arc::new(
        rustls::ServerConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_client_cert_verifier(Arc::new(AnyClientCertificate(provider)))
            .with_single_cert(
                vec![CertificateDer::from_pem_slice(identity.certificate.as_bytes()).unwrap()],
                PrivateKeyDer::from_pem_slice(identity.private_pem.as_bytes()).unwrap(),
            )
            .unwrap(),
    );
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let connection = rustls::ServerConnection::new(config.clone()).unwrap();
            let mut tls = rustls::StreamOwned::new(connection, stream.unwrap());
            let Some((method, target, body)) = read_request(&mut tls) else {
                continue;
            };
            let path = target
                .split_once('?')
                .map_or(target.as_str(), |(p, _)| p)
                .to_owned();
            log.lock().unwrap().push(Seen {
                method: method.clone(),
                path: path.clone(),
                body: body.clone(),
            });
            let (status, reply) = handler(&method, &path, &body);
            write!(
                tls,
                "HTTP/1.1 {status} Status\r\nContent-Type: application/json; charset=utf-8\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                reply.len()
            )
            .unwrap();
            tls.conn.send_close_notify();
            let _ = tls.flush();
        }
    });
    MockHost { port, seen }
}

#[test]
fn the_hosts_session_view_is_the_session_fixture() {
    let view = native::session_view(&suspended_snapshot(), &client(json!({})));
    assert_eq!(
        serde_json::to_value(&view).unwrap(),
        serde_json::from_str::<Value>(&fixture("session")).unwrap()
    );
}

#[test]
fn the_hosts_library_reads_back() {
    let apps = [dota()];
    let host = native::library(
        &apps,
        Some(&native::Scan::new(["steam:570".into()])),
        Some(apps[0].id()),
    );
    let text = serde_json::to_string(&host).unwrap();
    let library: Library = serde_json::from_str(&text).unwrap();
    assert_eq!(library.revision, host.revision);
    let game = &library.games[0];
    assert_eq!(
        (
            game.title.as_str(),
            game.game_id.as_deref(),
            game.installed,
            game.running
        ),
        ("Dota 2", Some("steam:570"), Some(true), true)
    );
    assert_eq!(game.store.as_ref().map(|s| s.name.as_str()), Some("Steam"));
    assert_eq!(
        game.artwork.cover.as_deref(),
        Some(format!("/appasset?appid={}&AssetType=2&AssetIdx=0", apps[0].id()).as_str())
    );
}

#[test]
fn host_native_views_round_trip_through_nightjars_host_client() {
    let apps = [dota()];
    let library = serde_json::to_string(&native::library(&apps, None, None)).unwrap();
    let view = |snapshot: &SessionSnapshot| {
        serde_json::to_string(&native::session_view(
            snapshot,
            &client(json!({"host_control": true})),
        ))
        .unwrap()
    };
    let (suspended, ended) = (
        view(&suspended_snapshot()),
        view(&SessionSnapshot::default()),
    );
    let host = serve(move |method, path, _| match (method, path) {
        ("GET", "/api/native/v1/library") => (200, library.clone()),
        ("GET", "/api/native/v1/session") => (200, suspended.clone()),
        ("POST", "/api/native/v1/session/end") => (200, ended.clone()),
        ("GET", "/api/native/v1/radeon") | ("POST", "/api/native/v1/radeon/settings") => {
            (200, fixture("radeon"))
        }
        _ => (404, "{}".into()),
    });
    let client = HostClient::new("127.0.0.1", 1, client_identity())
        .unwrap()
        .with_https_port(host.port)
        .with_server_certificate_pem(&host_identity().certificate)
        .unwrap();

    assert_eq!(client.library().unwrap().library.games[0].title, "Dota 2");
    let session = client.session().unwrap();
    assert_eq!(
        (session.state, session.owner),
        (State::Suspended, Owner::You)
    );
    assert_eq!(
        client.session_action(ActionId::End).unwrap().state,
        State::Idle
    );
    let write: SettingsWrite = serde_json::from_str(&fixture("settings_write")).unwrap();
    assert!(client.write_radeon(&write).unwrap().host_control);
    let seen = host.seen.lock().unwrap();
    assert!(serde_json::from_slice::<host_api::radeon::SettingsWrite>(&seen[3].body).is_ok());
}
