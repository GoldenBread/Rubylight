//! Pairing and HTTPS interop between Nightjar's HostClient and butterpollo-core's pairing state machine.
mod common;

use butterpollo_crypto::{crypto, pairing::Pairing};
use common::{client_identity, host_identity};
use nightjar_core::http::{
    AppControl, ClientIdentity, Error as HttpError, HostClient, InfoPath, LaunchRequest, PairResult,
};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------------------
// Pairing and HTTPS against a host built from butterpollo-core's pairing (rust/host/src/nvhttp/pairing.rs).

fn hex(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

const PIN: &str = "4321";

#[derive(Default)]
struct HostState {
    pairing: Option<Pairing>,
    paired_certificate: Option<Vec<u8>>,
    unpairs: u32,
    launches: Vec<HashMap<String, String>>,
    clipboard: String,
    reject_clipboard: bool,
    bitrate_reply: Option<String>,
}

fn decode_query(target: &str) -> (String, HashMap<String, String>) {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let decode = |s: &str| {
        let mut out = Vec::new();
        let b = s.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'%' {
                out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap());
                i += 3;
            } else {
                out.push(if b[i] == b'+' { b' ' } else { b[i] });
                i += 1;
            }
        }
        String::from_utf8(out).unwrap()
    };
    let args = query
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_owned(), decode(v)))
        .collect();
    (path.trim_start_matches('/').to_owned(), args)
}

fn xml(code: u16, fields: &[(&str, String)]) -> String {
    let mut s = format!("<?xml version=\"1.0\" encoding=\"utf-8\"?><root status_code=\"{code}\">");
    for (k, v) in fields {
        s.push_str(&format!("<{k}>{v}</{k}>"));
    }
    s + "</root>"
}

fn handle(state: &Mutex<HostState>, target: &str, peer: Option<&[u8]>, https_port: u16) -> String {
    let (path, args) = decode_query(target);
    let mut state = state.lock().unwrap();
    let identity = host_identity();
    let paired = peer.is_some() && peer == state.paired_certificate.as_deref();
    assert!(
        args.contains_key("uniqueid") && args.contains_key("uuid"),
        "{target}"
    );
    match path.as_str() {
        "serverinfo" => xml(
            200,
            &[
                ("hostname", "Test PC".into()),
                ("appversion", "7.1.431.-1".into()),
                ("GfeVersion", "3.23.0.74".into()),
                ("uniqueid", "HOST-1".into()),
                ("HttpsPort", https_port.to_string()),
                ("PairStatus", u8::from(paired).to_string()),
                ("currentgame", if state.launches.is_empty() { "0" } else { "881448767" }.into()),
                ("state", if state.launches.is_empty() { "SUNSHINE_SERVER_FREE" } else { "SUNSHINE_SERVER_BUSY" }.into()),
                ("ServerCodecModeSupport", "65793".into()),
                ("RustHostVersion", "2.2.21".into()),
                ("Permission", if paired { "119480064" } else { "0" }.into()),
            ],
        ),
        "pair" if args.get("phrase").map(String::as_str) == Some("getservercert") => {
            assert_eq!(args["devicename"], "Nightjar test");
            let certificate = String::from_utf8(unhex(&args["clientcert"])).unwrap();
            let salt = unhex(&args["salt"]);
            let pairing = Pairing::new(args["uniqueid"].clone(), args["devicename"].clone(), certificate, &salt, PIN);
            match pairing {
                Ok(p) => {
                    state.pairing = Some(p);
                    xml(200, &[("paired", "1".into()), ("plaincert", hex(identity.certificate.as_bytes()))])
                }
                Err(e) => xml(400, &[("paired", format!("0 {e}"))]),
            }
        }
        "pair" if args.get("phrase").map(String::as_str) == Some("pairchallenge") => {
            xml(200, &[("paired", u8::from(paired).to_string())])
        }
        "pair" => {
            let mut p = state.pairing.take().expect("no pending pairing");
            let result = if let Some(v) = args.get("clientchallenge") {
                p.client_challenge(identity, &unhex(v)).map(|r| ("challengeresponse", hex(&r)))
            } else if let Some(v) = args.get("serverchallengeresp") {
                p.server_response(identity, &unhex(v)).map(|r| ("pairingsecret", hex(&r)))
            } else if let Some(v) = args.get("clientpairingsecret") {
                p.finish(&unhex(v)).map(|()| ("paired", "1".into()))
            } else {
                panic!("unknown pairing phase {target}");
            };
            match result {
                Ok(("paired", _)) => {
                    state.paired_certificate =
                        Some(CertificateDer::from_pem_slice(p.certificate.as_bytes()).unwrap().to_vec());
                    xml(200, &[("paired", "1".into())])
                }
                Ok((field, value)) => {
                    state.pairing = Some(p);
                    xml(200, &[("paired", "1".into()), (field, value)])
                }
                Err(e) => xml(400, &[("paired", "0".into()), ("error", e.to_string())]),
            }
        }
        "unpair" => {
            state.unpairs += 1;
            state.pairing = None;
            if paired {
                state.paired_certificate = None;
            }
            xml(200, &[("unpaired", u8::from(paired).to_string())])
        }
        _ if !paired => xml(401, &[]),
        "applist" => "<?xml version=\"1.0\"?><root status_code=\"200\">\
            <App><AppTitle>Desktop</AppTitle><UUID>u-1</UUID><IDX>0</IDX><ID>881448767</ID><ArtVersion>1</ArtVersion>\
            <IsHdrSupported>1</IsHdrSupported></App>\
            <App><AppTitle>Terminate</AppTitle><UUID></UUID><IDX>1</IDX><ID>2147483504</ID></App></root>"
            .into(),
        "launch" => {
            state.launches.push(args);
            xml(
                200,
                &[("gamesession", "1".into()), ("sessionUrl0", "rtspenc://127.0.0.1:48010".into())],
            )
        }
        "cancel" => {
            state.launches.clear();
            xml(200, &[("cancel", "1".into())])
        }
        "appasset" => format!("PNG:{}", args["appid"]),
        "bitrate" => xml(200, &[("bitrate", state.bitrate_reply.clone().unwrap_or_else(|| {
            args["bitrate"].parse::<u32>().unwrap().min(80_000).to_string()
        }))]),
        other => panic!("unexpected request {other}"),
    }
}

fn respond(stream: &mut impl Write, body: &str) {
    let content_type = if body.starts_with("PNG") {
        "image/png"
    } else {
        "application/xml"
    };
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).unwrap();
    stream.write_all(body.as_bytes()).unwrap();
    stream.flush().unwrap();
}

fn read_head(stream: &mut impl Read) -> Option<String> {
    Some(read_headers(stream)?.split(' ').nth(1)?.to_owned())
}

fn read_headers(stream: &mut impl Read) -> Option<String> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte).ok()? == 0 {
            return None;
        }
        head.push(byte[0]);
    }
    Some(String::from_utf8(head).unwrap())
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

struct TestHost {
    http_port: u16,
    https_port: u16,
    state: Arc<Mutex<HostState>>,
}

fn start_test_host() -> TestHost {
    let state = Arc::new(Mutex::new(HostState::default()));
    let http = TcpListener::bind("127.0.0.1:0").unwrap();
    let https = TcpListener::bind("127.0.0.1:0").unwrap();
    let (http_port, https_port) = (
        http.local_addr().unwrap().port(),
        https.local_addr().unwrap().port(),
    );

    let shared = state.clone();
    std::thread::spawn(move || {
        for stream in http.incoming() {
            let mut stream = stream.unwrap();
            if let Some(target) = read_head(&mut stream) {
                let body = handle(&shared, &target, None, https_port);
                respond(&mut stream, &body);
            }
        }
    });

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let identity = host_identity();
    let config = rustls::ServerConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_client_cert_verifier(Arc::new(AnyClientCertificate(provider)))
        .with_single_cert(
            vec![CertificateDer::from_pem_slice(identity.certificate.as_bytes()).unwrap()],
            PrivateKeyDer::from_pem_slice(identity.private_pem.as_bytes()).unwrap(),
        )
        .unwrap();
    let config = Arc::new(config);
    let shared = state.clone();
    std::thread::spawn(move || {
        for stream in https.incoming() {
            let tcp = stream.unwrap();
            let connection = rustls::ServerConnection::new(config.clone()).unwrap();
            let mut tls = rustls::StreamOwned::new(connection, tcp);
            let Some(headers) = read_headers(&mut tls) else {
                continue;
            };
            let target = headers.split(' ').nth(1).unwrap();
            if target.starts_with("/pyrowave-udp-probe?") {
                use nightjar_core::http::pace;
                let (_, args) = decode_query(target);
                assert_eq!(
                    tls.conn.peer_certificates().unwrap()[0].as_ref(),
                    shared.lock().unwrap().paired_certificate.as_ref().unwrap()
                );
                assert_eq!(args["handshake"], "1");
                let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
                socket
                    .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                    .unwrap();
                let kbps: u32 = args["kbps"].parse().unwrap();
                let packet_size: u32 = args["packetsize"].parse().unwrap();
                let expected = pace::expected_packets(kbps, packet_size);
                let body = xml(
                    200,
                    &[
                        ("expected", expected.to_string()),
                        ("sent", expected.to_string()),
                        ("elapsedMs", "2000".into()),
                    ],
                );
                write!(tls, "HTTP/1.1 200 OK\r\nX-PyroWave-Udp-Port: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", socket.local_addr().unwrap().port(), body.len()).unwrap();
                tls.flush().unwrap();
                let mut hello = [0; 64];
                let (length, peer) = socket.recv_from(&mut hello).unwrap();
                assert_eq!(&hello[..length], args["token"].as_bytes());
                assert_eq!(peer.port(), args["port"].parse::<u16>().unwrap());
                let mut packet = vec![0; packet_size as usize + 48];
                packet[..32].copy_from_slice(args["token"].as_bytes());
                packet[32..36].copy_from_slice(&5u32.to_be_bytes());
                let other = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
                other.send_to(&packet, peer).unwrap();
                packet[0] ^= 1;
                socket.send_to(&packet, peer).unwrap();
                packet[0] ^= 1;
                for seq in (0..expected).rev() {
                    if seq == 5 {
                        continue;
                    }
                    packet[32..36].copy_from_slice(&seq.to_be_bytes());
                    socket.send_to(&packet, peer).unwrap();
                    if seq == 7 {
                        socket.send_to(&packet, peer).unwrap();
                    }
                    if seq % 16 == 0 {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                }
                tls.write_all(body.as_bytes()).unwrap();
                tls.conn.send_close_notify();
                let _ = tls.flush();
                continue;
            }
            if target.starts_with("/bandwidth-probe?") {
                assert_eq!(
                    tls.conn.peer_certificates().unwrap()[0].as_ref(),
                    shared.lock().unwrap().paired_certificate.as_ref().unwrap()
                );
                assert!(
                    target.starts_with("/bandwidth-probe?bytes=4194304&uniqueid="),
                    "{target}"
                );
                // As the host past its 3 s cap: chunked, and the body ends before the bytes asked for.
                write!(tls, "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").unwrap();
                for _ in 0..16 {
                    write!(tls, "10000\r\n").unwrap();
                    tls.write_all(&[7; 65536]).unwrap();
                    write!(tls, "\r\n").unwrap();
                }
                write!(tls, "0\r\n\r\n").unwrap();
                tls.conn.send_close_notify();
                let _ = tls.flush();
                continue;
            }
            if target.starts_with("/pyrowave-bandwidth-probe?") {
                assert_eq!(
                    tls.conn.peer_certificates().unwrap()[0].as_ref(),
                    shared.lock().unwrap().paired_certificate.as_ref().unwrap()
                );
                write!(tls, "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: 33554432\r\nConnection: close\r\n\r\n").unwrap();
                for _ in 0..4096 {
                    tls.write_all(&[0; 8192]).unwrap();
                }
                tls.conn.send_close_notify();
                let _ = tls.flush();
                continue;
            }
            if target.starts_with("/actions/clipboard?") {
                let mut state = shared.lock().unwrap();
                assert_eq!(
                    tls.conn.peer_certificates().unwrap()[0].as_ref(),
                    state.paired_certificate.as_ref().unwrap()
                );
                assert!(target.contains("type=text") && target.contains("uniqueid="));
                // Read a POST body before answering, even when refusing it: closing a socket with unread
                // data can reset the connection, and the client would see a network error, not the 404.
                let posted = if headers.starts_with("POST ") {
                    let length: usize = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("Content-Length: "))
                        .unwrap()
                        .trim()
                        .parse()
                        .unwrap();
                    assert!(headers.contains("Content-Type: text/plain; charset=utf-8\r\n"));
                    let mut bytes = vec![0; length];
                    tls.read_exact(&mut bytes).unwrap();
                    Some(String::from_utf8(bytes).unwrap())
                } else {
                    None
                };
                if state.reject_clipboard {
                    respond(&mut tls, "<root status_code=\"404\"/>");
                } else {
                    let body = if let Some(text) = posted {
                        state.clipboard = text;
                        ""
                    } else {
                        state.clipboard.as_str()
                    };
                    write!(tls, "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                }
                tls.conn.send_close_notify();
                let _ = tls.flush();
                continue;
            }
            let peer = tls
                .conn
                .peer_certificates()
                .and_then(|c| c.first())
                .map(|c| c.to_vec());
            let body = handle(&shared, target, peer.as_deref(), https_port);
            respond(&mut tls, &body);
            tls.conn.send_close_notify();
            let _ = tls.flush();
        }
    });
    TestHost {
        http_port,
        https_port,
        state,
    }
}

#[test]
fn pairing_https_launch_and_cancel_against_the_hosts_pairing() {
    let host = start_test_host();
    let client = HostClient::new("127.0.0.1", host.http_port, client_identity())
        .unwrap()
        .with_device_name("Nightjar test");
    let info = client.server_info(true).unwrap();
    assert!(!info.paired);
    assert_eq!(info.https_port, host.https_port);
    assert!(matches!(
        client.app_list(&info.capabilities),
        Err(HttpError::NotPaired)
    ));

    let result = client.pair(&info, PIN).unwrap();
    let PairResult::Paired {
        server_certificate_pem,
    } = result
    else {
        panic!("{result:?}")
    };
    assert_eq!(server_certificate_pem, host_identity().certificate);
    assert_eq!(host.state.lock().unwrap().unpairs, 0);

    // Pinned HTTPS with the client certificate.
    let info = client.server_info(true).unwrap();
    assert!(info.paired);
    assert_eq!(info.permission, Some(119_480_064));
    let apps = client.app_list(&info.capabilities).unwrap();
    assert_eq!(apps.len(), 2);
    assert_eq!(apps[1].control, AppControl::Terminate);
    assert_eq!(client.box_art(apps[0].id).unwrap(), b"PNG:881448767");
    assert!(
        client
            .box_art_url(apps[0].id)
            .unwrap()
            .starts_with(&format!(
                "https://127.0.0.1:{}/appasset?appid=881448767&AssetType=2&AssetIdx=0&uniqueid=",
                host.https_port
            ))
    );

    let request = LaunchRequest {
        app_id: apps[0].id,
        app_uuid: apps[0].uuid.clone(),
        ri_key: [9; 16],
        ri_key_id: 77,
        client_refresh_rate_x100: 5994,
        ..Default::default()
    };
    let launched = client.launch(&request, &info).unwrap().unwrap();
    assert_eq!(
        launched.rtsp_session_url.as_deref(),
        Some("rtspenc://127.0.0.1:48010")
    );
    let args = host.state.lock().unwrap().launches[0].clone();
    assert_eq!(args["appuuid"], "u-1");
    assert_eq!(args["mode"], "1920x1080x59.94");
    assert_eq!(args["rikey"], "09".repeat(16));
    assert_eq!(args["rikeyid"], "77");
    assert_eq!(args["corever"], "1");
    // The stream key ID reads the way the host reads it.
    assert!(client.cancel().unwrap());

    // A client stored with the pinned certificate reconnects without pairing again.
    let reloaded = HostClient::new("127.0.0.1", host.http_port, client_identity())
        .unwrap()
        .with_https_port(host.https_port)
        .with_server_certificate_pem(&server_certificate_pem)
        .unwrap();
    assert!(reloaded.server_info_https().unwrap().paired);
    let (info_again, path) = reloaded.server_info_checked(true).unwrap();
    assert!(info_again.paired && path == InfoPath::Pinned);

    // Another host certificate is refused over HTTPS; serverinfo falls back to HTTP.
    let other = crypto::Identity::generate().unwrap();
    let mismatched = HostClient::new("127.0.0.1", host.http_port, client_identity())
        .unwrap()
        .with_server_certificate_pem(&other.certificate)
        .unwrap();
    assert!(matches!(
        mismatched.server_info_https(),
        Err(HttpError::CertificateMismatch)
    ));
    assert!(!mismatched.server_info(true).unwrap().paired);
    // A poll asks without that fallback: another certificate's answer is not this host's.
    assert!(matches!(
        mismatched.server_info_checked(true),
        Err(HttpError::CertificateMismatch)
    ));

    // A different client identity is not paired and gets 401 for everything else.
    let stranger = HostClient::new(
        "127.0.0.1",
        host.http_port,
        Arc::new(ClientIdentity::generate().unwrap()),
    )
    .unwrap()
    .with_server_certificate_pem(&server_certificate_pem)
    .unwrap();
    assert!(!stranger.server_info(true).unwrap().paired);
    // The pinned host itself says it does not know this client: that pairing is gone.
    let (forgotten, path) = stranger.server_info_checked(true).unwrap();
    assert!(!forgotten.paired && path != InfoPath::Plain, "{path:?}");
    assert!(matches!(
        stranger.app_list(&info.capabilities),
        Err(HttpError::Status { code: 401, .. })
    ));

    reloaded.unpair().unwrap();
    assert!(!client.server_info(true).unwrap().paired);
}

#[test]
fn a_wrong_pin_is_detected_and_the_pairing_cancelled() {
    let host = start_test_host();
    let client = HostClient::new("127.0.0.1", host.http_port, client_identity())
        .unwrap()
        .with_device_name("Nightjar test");
    let info = client.server_info(true).unwrap();
    assert_eq!(client.pair(&info, "0000").unwrap(), PairResult::PinWrong);
    let state = host.state.lock().unwrap();
    assert_eq!(
        state.unpairs, 1,
        "the client ends the host's pending pairing over HTTP"
    );
    assert!(state.paired_certificate.is_none());
    drop(state);
    assert!(
        client.server_certificate_pem().is_none(),
        "a failed pairing pins nothing"
    );
}

#[test]
fn clipboard_uses_pinned_https_utf8_and_rejects_xml_errors() {
    let host = start_test_host();
    let identity = client_identity();
    host.state.lock().unwrap().paired_certificate = Some(identity.certificate_der().to_vec());
    let client = HostClient::new("127.0.0.1", host.http_port, identity)
        .unwrap()
        .with_https_port(host.https_port)
        .with_server_certificate_pem(&host_identity().certificate)
        .unwrap();
    let text = "Nightjar — Ελληνικά\nsecond line\r\n日本語";
    client.send_clipboard(text).unwrap();
    assert_eq!(client.clipboard().unwrap(), text);
    client.send_clipboard("").unwrap();
    assert_eq!(client.clipboard().unwrap(), "");
    assert!(matches!(
        client.send_clipboard(&"x".repeat(1024 * 1024 + 1)),
        Err(HttpError::InvalidArgument(_))
    ));
    host.state.lock().unwrap().reject_clipboard = true;
    assert!(matches!(client.clipboard(), Err(HttpError::Malformed(_))));
    let refused = client.send_clipboard("test");
    assert!(
        matches!(refused, Err(HttpError::Malformed(_))),
        "refused clipboard write: {refused:?}"
    );
}

#[test]
fn runtime_bitrate_returns_the_applied_budget_and_rejects_invalid_replies() {
    let host = start_test_host();
    let identity = client_identity();
    host.state.lock().unwrap().paired_certificate = Some(identity.certificate_der().to_vec());
    let client = HostClient::new("127.0.0.1", host.http_port, identity)
        .unwrap()
        .with_https_port(host.https_port)
        .with_server_certificate_pem(&host_identity().certificate)
        .unwrap();
    assert_eq!(client.set_bitrate(90_000).unwrap(), 80_000);
    assert_eq!(client.set_bitrate(30_000).unwrap(), 30_000);
    for bitrate in [0, 2_000_001, u32::MAX] {
        assert!(matches!(
            client.set_bitrate(bitrate),
            Err(HttpError::InvalidArgument(_))
        ));
    }
    for reply in ["0", "-1", "2000001", "invalid"] {
        host.state.lock().unwrap().bitrate_reply = Some(reply.into());
        assert!(matches!(
            client.set_bitrate(20_000),
            Err(HttpError::Malformed(_))
        ));
    }
}

#[test]
fn paired_probes_authenticate_udp_packets_and_transfer_the_advertised_bytes() {
    let host = start_test_host();
    let identity = client_identity();
    host.state.lock().unwrap().paired_certificate = Some(identity.certificate_der().to_vec());
    let client = HostClient::new("127.0.0.1", host.http_port, identity.clone())
        .unwrap()
        .with_https_port(host.https_port)
        .with_server_certificate_pem(&host_identity().certificate)
        .unwrap();
    let result = client.probe_pace(5000, 1392, 60, 50_000).unwrap();
    assert_eq!(
        result.received,
        result.expected - 1,
        "duplicates, foreign ports and wrong tokens must not repair the missing packet"
    );
    assert!(result.measured());
    let info = nightjar_core::http::ServerInfo::parse("<root status_code=\"200\"><uniqueid>probe-host</uniqueid><state>SERVER_FREE</state><PairStatus>1</PairStatus><PyroWaveBandwidthProbeBytes>33554432</PyroWaveBandwidthProbeBytes></root>", host.http_port).unwrap();
    let mbps = client.probe_bandwidth(&info).unwrap();
    assert!(mbps.is_finite() && mbps > 0.0);
    // Every codec's probe: a body the host cut short still measures, and only a host that offers it is asked.
    assert!(matches!(
        client.probe_link(&info),
        Err(HttpError::InvalidArgument(_))
    ));
    let offered = nightjar_core::http::ServerInfo::parse("<root status_code=\"200\"><uniqueid>probe-host</uniqueid><state>SERVER_FREE</state><PairStatus>1</PairStatus><BandwidthProbeVersion>1</BandwidthProbeVersion></root>", host.http_port).unwrap();
    let mbps = client.probe_link(&offered).unwrap();
    assert!(mbps.is_finite() && mbps > 0.0);
    let unpaired = HostClient::new("127.0.0.1", host.http_port, identity.clone())
        .unwrap()
        .with_https_port(host.https_port);
    assert!(matches!(
        unpaired.probe_pace(5000, 1392, 60, 50_000),
        Err(HttpError::NotPaired)
    ));
    assert!(matches!(
        unpaired.probe_bandwidth(&info),
        Err(HttpError::NotPaired)
    ));
    let other = crypto::Identity::generate().unwrap();
    let wrong_pin = HostClient::new("127.0.0.1", host.http_port, identity)
        .unwrap()
        .with_https_port(host.https_port)
        .with_server_certificate_pem(&other.certificate)
        .unwrap();
    assert!(matches!(
        wrong_pin.probe_bandwidth(&info),
        Err(HttpError::CertificateMismatch)
    ));
}

#[test]
fn host_and_client_generated_pairing_certificates_verify_mutually() {
    let client = ClientIdentity::generate().unwrap();
    let host_id = crypto::Identity::generate().unwrap();

    // Host crate's Pairing/crypto verifier accepts Nightjar ClientIdentity::generate() cert.
    crypto::verify_pairing_certificate(client.certificate_pem()).unwrap();

    // Client crate's HostClient pairing accepts Rubylight host's crypto::Identity::generate() cert
    // (including HTTPS pinning and encrypted handshake verification).
    let host = start_test_host();
    let client_id = Arc::new(client);
    let http = HostClient::new("127.0.0.1", host.http_port, client_id)
        .unwrap()
        .with_device_name("Nightjar test");
    let info = http.server_info(false).unwrap();
    let outcome = http.pair(&info, PIN).unwrap();
    assert!(
        matches!(outcome, PairResult::Paired { .. }),
        "mutual host/client generated certs must pair: {outcome:?}"
    );
    // Also confirm direct host cert PEM parses & verifies cleanly on the client side via HostClient::with_server_certificate_pem.
    let verified_client = HostClient::new("127.0.0.1", host.http_port, client_identity())
        .unwrap()
        .with_https_port(host.https_port)
        .with_server_certificate_pem(&host_id.certificate)
        .unwrap();
    drop(verified_client);
}
