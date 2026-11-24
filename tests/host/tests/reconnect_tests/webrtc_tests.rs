//! WebRTC sessions through the real signalling server, session actor, media transport and stream
//! worker, beside the Moonlight workers of the reconnect matrix. Only the encoder's output is
//! synthetic; the client is a str0m peer on a loopback UDP socket.
use super::*;
use butterpollo_session::config_ext::ConfigExt;
use butterpollo_session::webrtc::auth::proof_hex;
use butterpollo_webrtc::{
    sdp::{AudioMode, OfferSpec, VideoCodec, testing as peer_sdp},
    server,
    test_support::Ws,
};
use serde_json::{Value, json};
use str0m::{
    Event, Input, Output,
    channel::{ChannelConfig, ChannelId, Reliability},
    media::{KeyframeRequestKind, Mid},
    net::{Protocol, Receive},
};

const VIEW: u32 = 1 << 25;
const LIST: u32 = 1 << 24;

pub(super) use butterpollo_stream::stream::reconnect_harness::{fixture::access_unit, reserved};
use butterpollo_webrtc::WebRtcRuntime;

#[path = "webrtc_case.rs"]
mod webrtc_case;
use webrtc_case::{Case, received};

/// A browser-like client: it answers the offer, connects over loopback and records the video it
/// receives. It asks for keyframes on request.
struct Peer {
    stop: Arc<AtomicBool>,
    units: Arc<Mutex<Vec<Vec<u8>>>>,
    audio: Arc<Mutex<Vec<Vec<u8>>>>,
    pli: Arc<AtomicUsize>,
    /// The client's transport is up (ICE and DTLS done).
    connected: Arc<AtomicBool>,
    control: Arc<Mutex<Vec<String>>>,
    /// Text the host sent on the control channel.
    heard: Arc<Mutex<Vec<String>>>,
    /// What the client sends on the gamepad and input channels, in order.
    outbox: Arc<Mutex<Vec<Outgoing>>>,
    thread: Option<thread::JoinHandle<()>>,
}

/// One thing a browser-like client does with its data channels.
enum Outgoing {
    Gamepad(String),
    OpenInput,
    Input(String),
    CloseInput,
}

impl Peer {
    /// The peer and the answer SDP for `offer`.
    fn answer(offer: &str, spec: &OfferSpec) -> (Self, String) {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        // The host's video fills the default 64 KB while a busy test runner deschedules this
        // peer, and the drops take the host's SCTP acknowledgements with them: a lost one costs
        // a retransmission timeout of a second or more.
        socket2::SockRef::from(&socket)
            .set_recv_buffer_size(8 * 1024 * 1024)
            .unwrap();
        let local = socket.local_addr().unwrap();
        let mut rtc = peer_sdp::client(spec, local);
        let answer = peer_sdp::answer_for(&mut rtc, offer, spec);
        let mut peer = Self {
            stop: Arc::new(AtomicBool::new(false)),
            units: Arc::default(),
            audio: Arc::default(),
            pli: Arc::default(),
            connected: Arc::default(),
            control: Arc::default(),
            heard: Arc::default(),
            outbox: Arc::default(),
            thread: None,
        };
        let (stop, units, audio, pli, connected, control, heard, outbox) = (
            peer.stop.clone(),
            peer.units.clone(),
            peer.audio.clone(),
            peer.pli.clone(),
            peer.connected.clone(),
            peer.control.clone(),
            peer.heard.clone(),
            peer.outbox.clone(),
        );
        peer.thread = Some(thread::spawn(move || {
            let mut video: Option<Mid> = None;
            let mut sound: Option<Mid> = None;
            let mut control_channel: Option<ChannelId> = None;
            let mut gamepad_channel: Option<ChannelId> = None;
            let mut input_channel: Option<ChannelId> = None;
            let mut input_open = false;
            let mut pending: std::collections::VecDeque<Outgoing> = Default::default();
            let mut buffer = vec![0u8; 2048];
            while !stop.load(Ordering::Acquire) {
                let timeout = loop {
                    match rtc.poll_output() {
                        Ok(Output::Timeout(at)) => break at,
                        Ok(Output::Transmit(t)) => {
                            let data: Vec<u8> = t.contents.into();
                            let _ = socket.send_to(&data, t.destination);
                        }
                        Ok(Output::Event(Event::Connected)) => {
                            connected.store(true, Ordering::Release)
                        }
                        Ok(Output::Event(Event::MediaAdded(added))) if added.kind.is_video() => {
                            video = Some(added.mid)
                        }
                        Ok(Output::Event(Event::MediaAdded(added))) if added.kind.is_audio() => {
                            sound = Some(added.mid)
                        }
                        Ok(Output::Event(Event::MediaData(data))) if Some(data.mid) == video => {
                            units.lock().unwrap().push(data.data.to_vec())
                        }
                        Ok(Output::Event(Event::MediaData(data))) if Some(data.mid) == sound => {
                            audio.lock().unwrap().push(data.data.to_vec())
                        }
                        Ok(Output::Event(Event::ChannelOpen(id, label))) if label == "control" => {
                            control_channel = Some(id)
                        }
                        Ok(Output::Event(Event::ChannelOpen(id, label))) if label == "gamepad" => {
                            gamepad_channel = Some(id)
                        }
                        Ok(Output::Event(Event::ChannelOpen(id, label))) if label == "input" => {
                            input_open = input_channel == Some(id)
                        }
                        Ok(Output::Event(Event::ChannelData(data)))
                            if Some(data.id) == control_channel && !data.binary =>
                        {
                            heard
                                .lock()
                                .unwrap()
                                .push(String::from_utf8_lossy(&data.data).into_owned())
                        }
                        Ok(Output::Event(_)) => {}
                        Err(_) => return,
                    }
                };
                if let Some(mid) = video
                    && pli.swap(0, Ordering::AcqRel) > 0
                {
                    rtc.direct_api()
                        .stream_rx_by_mid(mid, None)
                        .expect("receiving video")
                        .request_keyframe(KeyframeRequestKind::Pli);
                }
                // A write the channel does not accept (`Ok(false)`, its buffer is full) is
                // retried, in order.
                if let Some(id) = control_channel {
                    let mut control = control.lock().unwrap();
                    while control.first().is_some_and(|text| {
                        rtc.channel(id).is_some_and(|mut c| {
                            matches!(c.write(false, text.as_bytes()), Ok(true))
                        })
                    }) {
                        control.remove(0);
                    }
                }
                pending.extend(outbox.lock().unwrap().drain(..));
                // In order; a message for a channel that is not open yet waits for it.
                while let Some(item) = pending.pop_front() {
                    let sent = match &item {
                        Outgoing::Gamepad(text) => gamepad_channel.is_some_and(|id| {
                            rtc.channel(id).is_some_and(|mut c| {
                                matches!(c.write(false, text.as_bytes()), Ok(true))
                            })
                        }),
                        Outgoing::OpenInput => {
                            let mut api = rtc.sdp_api();
                            input_channel = Some(api.add_channel_with_config(ChannelConfig {
                                label: "input".into(),
                                ordered: true,
                                reliability: Reliability::Reliable,
                                ..Default::default()
                            }));
                            assert!(api.apply().is_none());
                            true
                        }
                        Outgoing::Input(text) => {
                            input_open
                                && input_channel.is_some_and(|id| {
                                    rtc.channel(id).is_some_and(|mut c| {
                                        matches!(c.write(false, text.as_bytes()), Ok(true))
                                    })
                                })
                        }
                        Outgoing::CloseInput => {
                            if let Some(id) = input_channel {
                                rtc.direct_api().close_data_channel(id);
                                input_open = false;
                            }
                            true
                        }
                    };
                    if !sent {
                        pending.push_front(item);
                        break;
                    }
                }
                let wait = timeout
                    .saturating_duration_since(Instant::now())
                    .clamp(Duration::from_millis(1), Duration::from_millis(5));
                socket.set_read_timeout(Some(wait)).unwrap();
                let now = Instant::now();
                match socket.recv_from(&mut buffer) {
                    Ok((length, source)) => {
                        if let Ok(receive) =
                            Receive::new(Protocol::Udp, source, local, &buffer[..length])
                        {
                            let _ = rtc.handle_input(Input::Receive(now, receive));
                        }
                    }
                    Err(_) => {
                        let _ = rtc.handle_input(Input::Timeout(now));
                    }
                }
            }
        }));
        (peer, answer)
    }

    fn connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    fn count(&self) -> usize {
        self.units.lock().unwrap().len()
    }

    fn send(&self, item: Outgoing) {
        self.outbox.lock().unwrap().push(item);
    }

    /// A gamepad channel message.
    fn gamepad(&self, text: &str) {
        self.send(Outgoing::Gamepad(text.into()));
    }

    /// Open the `input` channel.
    fn open_input(&self) {
        self.send(Outgoing::OpenInput);
    }

    /// An `input` channel message, sent once the channel is open.
    fn input(&self, text: &str) {
        self.send(Outgoing::Input(text.into()));
    }

    fn close_input(&self) {
        self.send(Outgoing::CloseInput);
    }

    /// The control messages of `kind` the host sent.
    fn heard(&self, kind: &str) -> Vec<Value> {
        let all = self.heard.lock().unwrap();
        all.iter()
            .filter_map(|t| serde_json::from_str::<Value>(t).ok())
            .filter(|m| m["type"] == kind)
            .collect()
    }

    fn keyframes(&self, codec: u8) -> usize {
        let key = received(codec, true);
        self.units
            .lock()
            .unwrap()
            .iter()
            .filter(|u| **u == key)
            .count()
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Media ports go in pairs: the port and the claim on it.
const MEDIA_PORTS: u16 = (50000 - 44000) / 2;

// Each test process starts at its own pair, as `free_ports` does: two runs of this
// binary at once both starting at 44000 took each other's freed media port. The host
// binds the port only when a session starts, so the harness holds the pair's other
// port meanwhile, as `claim_slot` does, and a run that finds it held skips the pair.
fn media_port() -> (u16, UdpSocket) {
    static NEXT: AtomicU16 = AtomicU16::new(0);
    let start = (std::process::id() % u32::from(MEDIA_PORTS)) as u16;
    loop {
        let index = NEXT.fetch_add(1, Ordering::Relaxed);
        assert!(
            index < MEDIA_PORTS,
            "fixture WebRTC media port range exhausted"
        );
        let Some(port) = media_pair(start, index) else {
            continue;
        };
        if let Ok(claim) = UdpSocket::bind(address(port + 1))
            && UdpSocket::bind(address(port)).is_ok()
        {
            return (port, claim);
        }
    }
}
/// The `index`th media port pair after `start`, unless it touches a
/// [`reserved`] port.
fn media_pair(start: u16, index: u16) -> Option<u16> {
    let port = 44000 + ((start + index) % MEDIA_PORTS) * 2;
    (!reserved(port) && !reserved(port + 1)).then_some(port)
}

/// A process starting at the host's ports skips them (14 pairs) and the
/// harness's (14 more) instead of failing every test that asks for a port.
#[test]
fn media_pairs_skip_the_reserved_ports() {
    let pairs: Vec<_> = (0..MEDIA_PORTS)
        .filter_map(|index| media_pair((47984 - 44000) / 2, index))
        .collect();
    assert_eq!(pairs[0], 48012);
    assert_eq!(pairs.len(), usize::from(MEDIA_PORTS) - 28);
    assert!(
        pairs
            .iter()
            .all(|&port| (44000..50000).contains(&port) && !reserved(port) && !reserved(port + 1))
    );
}

/// A host with the Moonlight workers, the WebRTC server and its media transport.
struct Web {
    harness: Harness,
    server: Option<server::Running>,
    signalling: SocketAddr,
    media_port: u16,
    /// Keeps other runs of this binary off `media_port`.
    _media_claim: UdpSocket,
}

impl Web {
    async fn new() -> Result<Self> {
        let harness = Harness::new(10_000, 1).await?;
        harness.h.codecs.fetch_or(0x100, Ordering::AcqRel);
        let (media_port, media_claim) = media_port();
        harness
            .h
            .config
            .write()
            .unwrap()
            .update(
                json!({"webrtc_media_port": media_port})
                    .as_object()
                    .unwrap(),
            )
            .unwrap();
        let server = server::start_on(
            harness.h.clone(),
            [127, 0, 0, 1].into(),
            0,
            0,
            harness.media.clone(),
        )
        .context("the WebRTC server did not start")?;
        let signalling = WebRtcRuntime::for_host(&harness.h)
            .clients
            .status()
            .signalling
            .context("the WebRTC server is not listening")?;
        Ok(Self {
            harness,
            server: Some(server),
            signalling,
            media_port,
            _media_claim: media_claim,
        })
    }

    /// A paired WebRTC device with these permissions, signed in on a new connection.
    async fn device(&self, name: &str, permission: u32) -> Ws {
        let credentials = self.pair(name, permission);
        self.sign_in(&credentials).await
    }

    /// Pair a WebRTC device: its client id and secret.
    fn pair(&self, name: &str, permission: u32) -> (String, String) {
        let id = hex::encode(crypto::random::<16>());
        let secret = hex::encode(crypto::random::<32>());
        let mut client = PairedClient::paired_over_webrtc(name.into(), &id, &secret);
        client.perm = permission;
        self.harness.h.paired.write().unwrap().clients.push(client);
        (id, secret)
    }

    /// The host's id for the device paired as `name`.
    fn uuid(&self, name: &str) -> String {
        let paired = self.harness.h.paired.read().unwrap();
        let client = paired.clients.iter().find(|c| c.name == name);
        client.expect("the device is paired").uuid.clone()
    }

    /// A new signalling connection for a paired device.
    async fn sign_in(&self, (id, secret): &(String, String)) -> Ws {
        let mut ws = Ws::connect(self.signalling).await;
        let nonce = ws.expect("auth-required").await["nonce"]
            .as_str()
            .unwrap()
            .to_owned();
        ws.send(json!({"version":2,"type":"authenticate","clientId":id,"proof":proof_hex(secret, &nonce).unwrap()})).await;
        ws.expect("authenticated").await;
        ws.expect("gateway-status").await;
        ws.expect("capabilities").await;
        ws
    }

    fn app(&self) -> String {
        self.harness.h.apps.read().unwrap()[0].id().to_string()
    }

    fn request(&self, kind: &str, codec: &str, hdr: bool, channels: u32) -> Value {
        json!({"version":2,"type":kind,"appId":self.app(),"video":{"width":1920,"height":1080,"fps":60,"codec":codec,"bitrateKbps":20000,"hdr":hdr},"audio":{"channels":channels}})
    }

    /// The one active stream session.
    fn session(&self) -> Arc<Session> {
        let sessions = self.harness.h.sessions.lock().unwrap();
        assert_eq!(sessions.active.len(), 1);
        sessions.active.values().next().unwrap().clone()
    }

    /// A Remote Monitor session of another device, which no WebRTC request stops.
    fn add_monitor(&self) -> Arc<Session> {
        let fixture = butterpollo_stream::state::test_support::Fixture {
            host: self.harness.h.clone(),
        };
        let monitor = fixture.session(fixture.client(u32::MAX), Role::RemoteMonitor);
        // Its drop would end the harness host's app and delete its directory.
        std::mem::forget(fixture);
        monitor
    }

    /// No app runs, so the next launch starts one and records its owner.
    fn clear_app(&self) {
        *self.harness.h.current_app.lock().unwrap() = None;
    }

    /// The owner of the running app.
    fn owner(&self) -> String {
        let app = self.harness.h.current_app.lock().unwrap();
        app.as_ref().expect("an app is running").owner.clone()
    }

    async fn idle(&self) {
        until("sessions and WebRTC links released", WAIT, || {
            self.harness.h.sessions.lock().unwrap().idle() && self.harness.h.webrtc_links.is_empty()
        })
        .await;
    }
}

/// The next message that is not a gateway or capability refresh.
async fn next(ws: &mut Ws) -> Value {
    loop {
        let message = ws.next().await.expect("the socket stays open");
        if !matches!(
            message["type"].as_str(),
            Some("gateway-status" | "capabilities")
        ) {
            return message;
        }
    }
}

/// No message but gateway and capability refreshes arrives for `span`.
async fn nothing_for(ws: &mut Ws, span: Duration) {
    if let Ok(message) = tokio::time::timeout(span, next(ws)).await {
        panic!("announced before the source check: {message}");
    }
}

async fn status(ws: &mut Ws, kind: &str, state: &str) -> Value {
    let message = next(ws).await;
    assert_eq!(message["type"], kind, "{message}");
    assert_eq!(message["state"], state, "{message}");
    message
}

async fn error(ws: &mut Ws, request: &str, code: &str) {
    let message = next(ws).await;
    assert_eq!(message["type"], "error", "{message}");
    assert_eq!(message["requestType"], request, "{message}");
    assert_eq!(message["code"], code, "{message}");
}

/// Offer, answer and the first video, from the start of `status(.., "starting-webrtc")` on.
async fn answer(ws: &mut Ws, codec: VideoCodec) -> (u64, Peer) {
    answer_case(ws, &Case::new(codec, "", 0)).await
}

async fn answer_case(ws: &mut Ws, case: &Case) -> (u64, Peer) {
    let starting = status(ws, "session-status", "starting-webrtc").await;
    let offer = next(ws).await;
    assert_eq!(offer["type"], "offer", "{offer}");
    let id = offer["sessionId"].as_u64().unwrap();
    assert_eq!(starting["sessionId"], id);
    let (peer, sdp) = Peer::answer(offer["sdp"].as_str().unwrap(), &case.spec());
    ws.send(json!({"version":2,"type":"answer","sessionId":id,"sdp":sdp}))
        .await;
    (id, peer)
}

async fn streaming(ws: &mut Ws, id: u64) {
    let message = status(ws, "session-status", "streaming").await;
    assert_eq!(message["sessionId"], id);
    assert!(message["video"].is_object() && message["audio"].is_object());
}

/// A host that has every encoder the cases ask for: AV1, AV1 10-bit and HEVC Main10 as well.
async fn web_with_every_codec() -> Result<Web> {
    let web = Web::new().await?;
    web.harness
        .h
        .codecs
        .fetch_or(0x10000 | 0x20000 | 0x200, Ordering::AcqRel);
    Ok(web)
}

const H264: Case = Case::new(VideoCodec::H264, "h264", 0);
const HEVC: Case = Case::new(VideoCodec::Hevc, "hevc", 1);
const AV1: Case = Case::new(VideoCodec::Av1, "av1", 2);

/// Start a stream for a signed-in device and wait for its first video.
async fn stream(web: &Web, ws: &mut Ws) -> Peer {
    ws.send(web.request("start-session", "h264", false, 2))
        .await;
    status(ws, "session-status", "starting").await;
    let (id, peer) = answer(ws, VideoCodec::H264).await;
    streaming(ws, id).await;
    until("video received", WAIT, || peer.count() >= 2).await;
    peer
}

#[path = "webrtc_tests/input_tests.rs"]
mod input_tests;

#[path = "webrtc_tests/codecs_hdr_tests.rs"]
mod codecs_hdr_tests;
#[path = "webrtc_tests/session_lifecycle_tests.rs"]
mod session_lifecycle_tests;
