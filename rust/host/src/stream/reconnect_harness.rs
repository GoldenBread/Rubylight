//! GPU-free loopback test harness for host session and interop suites.
use super::Media;
use crate::state::{Launch, Session, Shared};
use anyhow::Result;
use butterpollo_core::{
    config::Ports,
    crypto::{self, Identity},
    state::{App, Client as PairedClient},
};
use std::{
    net::{SocketAddr, UdpSocket},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU16, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub mod client;
pub mod fixture;
pub use client::{Client, FrameWarpMessages, LossySocket, Notices};
pub use fixture::{Fixture, Slot, access_unit, audio_payload};

pub const HOST_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const CLOSED: u32 = 0x8003_0023;
pub const FAILED: u32 = 0x8000_4005;
pub const WAIT: Duration = Duration::from_secs(5);
/// Only bounds a hung HTTP request: a launch runs prep commands, which a loaded PC starts
/// slowly.
pub const REQUEST_WAIT: Duration = Duration::from_secs(30);

pub async fn until(message: &str, timeout: Duration, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !condition() {
        assert!(Instant::now() < deadline, "timed out: {message}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// Waits for video and the host's first IDR request; false if the session stopped first.
pub async fn established(session: &Session, client: &Client) -> bool {
    let deadline = Instant::now() + WAIT;
    while client.frames.load(Ordering::Acquire) <= 2
        || session.stats.idr_requests.load(Ordering::Acquire) == 0
    {
        if session.stopping() {
            return false;
        }
        assert!(
            Instant::now() < deadline,
            "{}",
            setup_state("timed out: control and video established", session, client)
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    !session.stopping()
}

pub fn setup_state(what: &str, session: &Session, client: &Client) -> String {
    format!(
        "{what} (ENet connected: {}, video packets: {}, IDR requests: {}, stopping: {}, reason: {:#x}, warnings: {:?})",
        client.connected.load(Ordering::Acquire),
        client.frames.load(Ordering::Acquire),
        session.stats.idr_requests.load(Ordering::Acquire),
        session.stopping(),
        session.termination_reason(),
        session.launch.warnings.snapshot()
    )
}

/// The host's default ports and the release harness's, which no fixture takes.
pub fn reserved(port: u16) -> bool {
    (47984..=48010).contains(&port) || (48518..=48544).contains(&port)
}
pub fn address(port: u16) -> SocketAddr {
    assert!(!reserved(port));
    ([127, 0, 0, 1], port).into()
}
const CLIENT_PORTS: u16 = 64000 - 52000;

/// A client socket. Each test process starts at its own port, as `free_ports` does.
pub fn udp() -> UdpSocket {
    static NEXT: AtomicU16 = AtomicU16::new(0);
    let start = (std::process::id() % u32::from(CLIENT_PORTS)) as u16;
    loop {
        let index = NEXT.fetch_add(1, Ordering::Relaxed);
        assert!(index < CLIENT_PORTS, "fixture UDP port range exhausted");
        if let Ok(socket) = UdpSocket::bind(address(52000 + (start + index) % CLIENT_PORTS)) {
            socket.set_nonblocking(true).unwrap();
            return socket;
        }
    }
}
pub const PORT_SLOTS: u16 = (40000 - 22000) / 32;

/// A slot's spare port, which no slot's `Ports` use (they span `base - 5` to `base + 21`).
pub const CLAIM: u16 = 22;

// Each test process starts at its own slot, so two `cargo test` runs at once
// rarely try the same ports.
pub fn port_base(start: u16, index: u16) -> u16 {
    assert!(index < PORT_SLOTS, "fixture host port range exhausted");
    22000 + (start.wrapping_add(index) % PORT_SLOTS) * 32
}
/// The slot at `base` if it is free, with a socket on its spare port that claims it while it
/// is held. The host binds its ports a while after `reserve` lets go of them (after loading
/// its identity, and the listeners in their own tasks); another run of this binary that
/// checked the same slot meanwhile took them, and one of the two failed with "address in
/// use" (os error 10048). A run skips a slot another holds the claim of.
pub fn claim_slot(base: u16) -> Option<(Ports, UdpSocket)> {
    let claim = UdpSocket::bind(address(base + CLAIM)).ok()?;
    let ports = Ports::from_base(base);
    reserve(ports).ok()?;
    Some((ports, claim))
}
pub fn free_ports() -> (Ports, UdpSocket) {
    static NEXT: AtomicU16 = AtomicU16::new(0);
    let start = (std::process::id() % u32::from(PORT_SLOTS)) as u16;
    loop {
        if let Some(slot) = claim_slot(port_base(start, NEXT.fetch_add(1, Ordering::Relaxed))) {
            return slot;
        }
    }
}

pub fn reserve(ports: Ports) -> Result<(Vec<std::net::TcpListener>, Vec<UdpSocket>)> {
    let tcp = [ports.http, ports.https, ports.web, ports.rtsp]
        .into_iter()
        .map(|p| std::net::TcpListener::bind(address(p)))
        .collect::<std::io::Result<_>>()?;
    let udp = [ports.video, ports.control, ports.audio, ports.mic]
        .into_iter()
        .map(|p| UdpSocket::bind(address(p)))
        .collect::<std::io::Result<_>>()?;
    Ok((tcp, udp))
}

pub struct Harness {
    pub h: Shared,
    pub fixture: Arc<Fixture>,
    pub media: Option<Arc<Media>>,
    pub listeners: Vec<tokio::task::JoinHandle<Result<()>>>,
    pub http: reqwest::Client,
    pub clients: Vec<reqwest::Client>,
    pub ports: Ports,
    /// Keeps other runs of this binary off `ports` (`claim_slot`).
    _claim: UdpSocket,
    /// A `ping_timeout` a loaded PC's stream setup can outlast (`play_short_timeout`).
    pub short_timeout: bool,
    pub directory: PathBuf,
    pub started: Instant,
}
impl Harness {
    pub async fn new(timeout_ms: u32, clients: usize) -> Result<Self> {
        Self::configured(timeout_ms, clients, "").await
    }
    /// A harness whose configuration also has the lines `extra`.
    pub async fn configured(timeout_ms: u32, clients: usize, extra: &str) -> Result<Self> {
        let started = Instant::now();
        // A failed test prints what its host logged (the test writer is captured per test).
        crate::test_log::install();
        let directory =
            std::env::temp_dir().join(format!("butterpollo-reconnect-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory)?;
        let (ports, claim) = free_ports();
        std::fs::write(
            directory.join("sunshine.conf"),
            format!(
                "port={}\nbind_address=127.0.0.1\nping_timeout={timeout_ms}\nstream_audio=false\nenable_discovery=false\nupnp=false\n{extra}",
                ports.http
            ),
        )?;
        let mut h = crate::state::Host::load(directory.clone(), directory.clone(), None)?;
        let fixture = Arc::new(Fixture::default());
        Arc::get_mut(&mut h).unwrap().reconnect_fixture = Some(fixture.clone());
        h.codecs.store(1 | 0x200, Ordering::Release);
        h.startup_codecs.send_replace(true);
        // No startup pass runs here: `/serverinfo` would otherwise wait its whole deadline
        // for a packed probe that never publishes.
        h.startup_packed.send_replace(true);
        let app = App::desktop();
        *h.apps.write().unwrap() = vec![app.clone()];
        *h.current_app.lock().unwrap() = Some(crate::process::RunningApp::with_environment(
            &app,
            Default::default(),
        )?);
        let mut http_clients = vec![];
        for index in 0..clients {
            let extra_identity;
            let identity = if index == 0 {
                &h.identity
            } else {
                extra_identity = Identity::generate()?;
                &extra_identity
            };
            h.paired.write().unwrap().clients.push(PairedClient {
                name: format!("reconnect-{index}"),
                uuid: index.to_string(),
                cert: identity.certificate.clone(),
                perm: u32::MAX,
                enabled: true,
                extra: Default::default(),
            });
            let pem = format!("{}{}", identity.certificate, identity.private_pem);
            http_clients.push(
                reqwest::Client::builder()
                    .no_proxy()
                    .danger_accept_invalid_certs(true)
                    .identity(reqwest::Identity::from_pem(pem.as_bytes())?)
                    .pool_max_idle_per_host(0)
                    .timeout(REQUEST_WAIT)
                    .build()?,
            );
        }
        let media = Media::new(h.clone(), address(ports.video).ip())?;
        let acceptor = crate::tls::acceptor(&h.identity, true)?;
        let listeners = vec![
            tokio::spawn(crate::tls::serve(
                address(ports.http),
                crate::nvhttp::router(h.clone(), false),
                None,
            )),
            tokio::spawn(crate::tls::serve(
                address(ports.https),
                crate::nvhttp::router(h.clone(), true),
                Some(acceptor),
            )),
            tokio::spawn(crate::rtsp_server::serve(
                address(ports.rtsp),
                h.clone(),
                media.clone(),
            )),
        ];
        let harness = Self {
            h,
            fixture,
            media: Some(media),
            listeners,
            clients: http_clients,
            http: reqwest::Client::builder()
                .no_proxy()
                .pool_max_idle_per_host(0)
                .timeout(REQUEST_WAIT)
                .build()?,
            ports,
            _claim: claim,
            short_timeout: timeout_ms <= 2000,
            directory,
            started,
        };
        for port in [ports.http, ports.https, ports.rtsp] {
            let deadline = Instant::now() + WAIT;
            loop {
                if tokio::net::TcpStream::connect(address(port)).await.is_ok() {
                    break;
                }
                assert!(Instant::now() < deadline, "listener {port} did not start");
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
        harness.counts(0, 0).await?;
        Ok(harness)
    }
    pub async fn launch_response(&self, client: usize, resume: bool) -> Result<String> {
        self.launch_response_hdr(client, resume, None).await
    }
    /// A launch or resume with the given `hdrMode` query (absent for `None`).
    pub async fn launch_response_hdr(
        &self,
        client: usize,
        resume: bool,
        hdr_mode: Option<&str>,
    ) -> Result<String> {
        self.launch_response_with(client, resume, hdr_mode, &[])
            .await
    }
    /// A launch or resume with the given `hdrMode` and the `extra` query parameters.
    pub async fn launch_response_with(
        &self,
        client: usize,
        resume: bool,
        hdr_mode: Option<&str>,
        extra: &[(&'static str, &str)],
    ) -> Result<String> {
        let endpoint = if resume { "resume" } else { "launch" };
        let key = hex::encode(crypto::random::<16>());
        let app = self.h.apps.read().unwrap()[0].id();
        let mut query = vec![
            ("appid", app.to_string()),
            ("rikey", key),
            ("rikeyid", "1".into()),
            ("corever", "1".into()),
            ("mode", "640x480x60".into()),
        ];
        query.extend(hdr_mode.map(|mode| ("hdrMode", mode.to_string())));
        query.extend(extra.iter().map(|(name, value)| (*name, value.to_string())));
        Ok(self.clients[client]
            .get(format!("https://127.0.0.1:{}/{endpoint}", self.ports.https))
            .query(&query)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?)
    }
    pub async fn launch(&self, client: usize, resume: bool) -> Result<Launch> {
        self.launch_hdr(client, resume, None).await
    }
    pub async fn launch_hdr(
        &self,
        client: usize,
        resume: bool,
        hdr_mode: Option<&str>,
    ) -> Result<Launch> {
        self.launch_with(client, resume, hdr_mode, &[]).await
    }
    pub async fn launch_with(
        &self,
        client: usize,
        resume: bool,
        hdr_mode: Option<&str>,
        extra: &[(&'static str, &str)],
    ) -> Result<Launch> {
        let response = self
            .launch_response_with(client, resume, hdr_mode, extra)
            .await?;
        assert!(
            response.contains("status_code=\"200\""),
            "resume={resume}: {response}"
        );
        assert_eq!(
            xml(&response, if resume { "resume" } else { "gamesession" }),
            "1"
        );
        let launch = self
            .h
            .sessions
            .lock()
            .unwrap()
            .pending
            .values()
            .find(|l| l.client.uuid == client.to_string())
            .cloned()
            .expect("launch must be pending");
        Ok(launch)
    }
    pub async fn rtsp(
        &self,
        launch: &Launch,
        sequence: u32,
        method: &str,
        body: &str,
    ) -> Result<String> {
        self.rtsp_at(launch, sequence, method, "rtsp://127.0.0.1/stream", body)
            .await
    }
    pub async fn rtsp_at(
        &self,
        launch: &Launch,
        sequence: u32,
        method: &str,
        target: &str,
        body: &str,
    ) -> Result<String> {
        let wire = sealed_rtsp(launch, sequence, method, target, body)?;
        let response = tokio::time::timeout(WAIT, async {
            let mut socket = tokio::net::TcpStream::connect(address(self.ports.rtsp)).await?;
            socket.write_all(&wire).await?;
            let mut response = vec![];
            // A host that closes without answering may reset the connection: Windows reports
            // that as aborted rather than as the end of the stream.
            match socket.read_to_end(&mut response).await {
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::ConnectionAborted | std::io::ErrorKind::ConnectionReset
                    ) => {}
                result => {
                    result?;
                }
            }
            Ok::<_, anyhow::Error>(response)
        })
        .await??;
        opened_rtsp(launch, method, &response)
    }
    pub async fn play(&self, launch: &Launch) -> Result<(Arc<Session>, Client)> {
        self.play_encrypted(launch, 0).await
    }
    pub async fn play_encrypted(
        &self,
        launch: &Launch,
        encryption: u32,
    ) -> Result<(Arc<Session>, Client)> {
        self.play_announcing(launch, encryption, "").await
    }
    /// Plays after an ANNOUNCE that carries the extra SDP lines `sdp`.
    pub async fn play_announcing(
        &self,
        launch: &Launch,
        encryption: u32,
        sdp: &str,
    ) -> Result<(Arc<Session>, Client)> {
        let (session, client) = self.connect_announcing(launch, encryption, sdp).await?;
        assert!(
            established(&session, &client).await,
            "{}",
            setup_state(
                "stopped before control and video established",
                &session,
                &client
            )
        );
        Ok((session, client))
    }
    /// `play` for a host whose `ping_timeout` (1 or 2 s) is shorter than a loaded PC can take
    /// from PLAY to the client's first ENet packet: a session that ended before ENet connected
    /// is resumed and played again, as Moonlight would reconnect. One that ended after it
    /// connected fails the test. The tests that use it check what happens once the stream runs.
    pub async fn play_short_timeout(&self, launch: &Launch) -> Result<(Arc<Session>, Client)> {
        assert!(self.short_timeout, "a long ping_timeout outlasts any setup");
        let mut launch = launch.clone();
        for attempt in 1.. {
            let (session, client) = self.connect_announcing(&launch, 0, "").await?;
            if established(&session, &client).await {
                return Ok((session, client));
            }
            let state = setup_state("stopped during setup", &session, &client);
            assert!(attempt < 3, "{state}");
            assert_eq!(session.termination_reason(), FAILED, "{state}");
            assert!(!client.connected.load(Ordering::Acquire), "{state}");
            eprintln!("attempt {attempt}: {state}; resuming");
            drop(client);
            self.empty().await?;
            launch = self.launch(0, true).await?;
        }
        unreachable!()
    }
    /// ANNOUNCE with the extra SDP lines `sdp`, PLAY and the control connection,
    /// without waiting for video or the first IDR request.
    pub async fn connect_announcing(
        &self,
        launch: &Launch,
        encryption: u32,
        sdp: &str,
    ) -> Result<(Arc<Session>, Client)> {
        self.rtsp(launch, 1, "ANNOUNCE", &format!("a=x-nv-video[0].clientViewportWd:640\r\na=x-nv-video[0].clientViewportHt:480\r\na=x-ss-general.encryptionEnabled:{encryption}\r\n{sdp}")).await?;
        self.rtsp(launch, 2, "PLAY", "").await?;
        let session = self.h.sessions.lock().unwrap().active[&launch.id].clone();
        let client = Client::new(self.ports, launch.clone())?;
        Ok((session, client))
    }
    pub async fn counts(&self, active: usize, pending: usize) -> Result<()> {
        let response = self
            .http
            .get(format!("http://127.0.0.1:{}/serverinfo", self.ports.http))
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        assert_eq!(
            xml(&response, "RustHostSessionCount"),
            active.to_string(),
            "{response}"
        );
        assert_eq!(
            xml(&response, "RustHostPendingSessionCount"),
            pending.to_string(),
            "{response}"
        );
        Ok(())
    }
    pub async fn empty(&self) -> Result<()> {
        until(
            "sessions, display leases and input slots released",
            WAIT,
            || {
                self.h.sessions.lock().unwrap().idle()
                    && self.fixture.displays.load(Ordering::Acquire) == 0
                    && self.fixture.audios.load(Ordering::Acquire) == 0
                    && self.fixture.inputs.load(Ordering::Acquire) == 0
            },
        )
        .await;
        self.counts(0, 0).await?;
        assert!(self.h.sessions.lock().unwrap().teardown.is_empty());
        assert!(self.h.app_display.lock().unwrap().is_empty());
        assert!(self.h.monitors.lock().unwrap().is_empty());
        assert!(
            self.media
                .as_ref()
                .unwrap()
                .peers
                .lock()
                .unwrap()
                .is_empty()
        );
        Ok(())
    }
    pub async fn end(&self, session: &Session, client: Client) -> Result<()> {
        self.rtsp(&session.launch, 3, "TEARDOWN", "").await?;
        // After an outage the host's ENet resends on a timeout that doubled while the client
        // was away, seconds after the network returns.
        until("normal termination delivered", WAIT * 3, || {
            client.reason.load(Ordering::Acquire) != 0
        })
        .await;
        assert_eq!(session.termination_reason(), CLOSED);
        assert_eq!(client.reason.load(Ordering::Acquire), CLOSED);
        drop(client);
        self.empty().await
    }
    pub async fn finish(mut self) -> Result<()> {
        self.empty().await?;
        // Reuse the same listeners/UDP ports for a fresh launch before checking
        // that stopping this isolated host releases the sockets themselves.
        let launch = self.launch(0, false).await?;
        let (session, client) = if self.short_timeout {
            self.play_short_timeout(&launch).await?
        } else {
            self.play(&launch).await?
        };
        self.counts(1, 0).await?;
        self.end(&session, client).await?;
        self.h.stop.store(true, Ordering::Release);
        for listener in self.listeners.drain(..) {
            listener.abort();
            assert!(listener.await.unwrap_err().is_cancelled());
        }
        let weak = Arc::downgrade(self.media.as_ref().unwrap());
        self.media.take();
        // The microphone's blocking receive can outlive Media until its timeout.
        until("media workers and sockets stopped", WAIT, || {
            weak.upgrade().is_none() && reserve(self.ports).is_ok()
        })
        .await;
        eprintln!(
            "reconnect matrix case completed in {:.3}s",
            self.started.elapsed().as_secs_f64()
        );
        Ok(())
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        self.fixture.hold_teardown.store(false, Ordering::Release);
        self.h.stop.store(true, Ordering::Release);
        let canceled = self.h.sessions.lock().unwrap().request_stop(None);
        drop(canceled);
        for listener in &self.listeners {
            listener.abort();
        }
        let weak = self.media.as_ref().map(Arc::downgrade);
        self.media.take();
        let deadline = Instant::now() + WAIT;
        while weak.as_ref().is_some_and(|w| w.upgrade().is_some()) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
/// An encrypted RTSP request from `launch`'s client, as Moonlight sends it.
pub fn sealed_rtsp(
    launch: &Launch,
    sequence: u32,
    method: &str,
    target: &str,
    body: &str,
) -> Result<Vec<u8>> {
    let request = format!(
        "{method} {target} RTSP/1.0\r\nCSeq: {sequence}\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let mut iv = [0; 12];
    iv[..4].copy_from_slice(&sequence.to_le_bytes());
    iv[10..].copy_from_slice(b"CR");
    let (tag, ciphertext) = crypto::gcm_seal(&launch.key, &iv, request.as_bytes())?;
    let mut wire = ((ciphertext.len() as u32) | 0x80000000)
        .to_be_bytes()
        .to_vec();
    wire.extend_from_slice(&sequence.to_be_bytes());
    wire.extend_from_slice(&tag);
    wire.extend_from_slice(&ciphertext);
    Ok(wire)
}
/// The host's decrypted answer to `method`; an error unless it is a 200, or
/// when the host closed the connection without one.
pub fn opened_rtsp(launch: &Launch, method: &str, response: &[u8]) -> Result<String> {
    anyhow::ensure!(
        response.len() >= 24,
        "RTSP {method} closed without a response"
    );
    let length = u32::from_be_bytes(response[..4].try_into()?) & 0x7fffffff;
    assert_eq!(length as usize, response.len() - 24);
    let mut iv = [0; 12];
    iv[..4].copy_from_slice(&u32::from_be_bytes(response[4..8].try_into()?).to_le_bytes());
    iv[10..].copy_from_slice(b"HR");
    let response = String::from_utf8(crypto::gcm_open(
        &launch.key,
        &iv,
        &response[8..24],
        &response[24..],
    )?)?;
    anyhow::ensure!(response.starts_with("RTSP/1.0 200"), "{response}");
    Ok(response)
}
pub fn xml<'a>(text: &'a str, tag: &str) -> &'a str {
    text.split_once(&format!("<{tag}>"))
        .unwrap()
        .1
        .split_once(&format!("</{tag}>"))
        .unwrap()
        .0
}
