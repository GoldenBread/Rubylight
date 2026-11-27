//! Shared test harness for WebRTC media engine, pacing, transport and socket tests.

use super::super::sdp::testing::{answer, answer_for, client, host_offer, spec, surround_spec};
use super::super::sdp::{VideoCodec, accept_answer};
use super::*;
use butterpollo_win_graphics::timing::Signal;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize};
use std::sync::{Mutex, mpsc};
use str0m::IceConnectionState;
use str0m::channel::ChannelId;
use str0m::media::{MediaTime, Mid};
use str0m::net::{Protocol, Receive};
use str0m::rtp::RawPacket;
use str0m::{Event, Input, Output};

mod engine;
mod pacing;
mod socket;
mod transport;

const HOST: &str = "192.168.1.10:8001";
const CLIENT: &str = "192.168.1.20:50000";
const PERIOD: Duration = Duration::from_micros(16_667);

#[derive(Clone, Debug)]
struct Rtp {
    pt: u8,
    marker: bool,
    timestamp: u32,
    seq: u16,
    playout: Option<(MediaTime, MediaTime)>,
}

#[derive(Debug)]
struct Received {
    pt: u8,
    data: Vec<u8>,
    time: MediaTime,
}

/// A host Engine against a plain str0m client, joined by an in-memory link and a virtual clock.
struct Link {
    host: Engine,
    client: Rtc,
    client_video: Option<Mid>,
    now: Instant,
    start: Instant,
    authenticated: Arc<AtomicBool>,
    rtp: Vec<Rtp>,
    media: Vec<Received>,
    host_events: Vec<TransportEvent>,
    idr: Vec<IdrReason>,
    channel_data: Vec<(String, Vec<u8>)>,
    labels: HashMap<ChannelId, String>,
    closed_by_host: Vec<String>,
    /// Drop the n-th host media datagram (counting from zero).
    drop_rtp: Option<usize>,
    rtp_seen: usize,
    /// Header extension ids on the wire, from the unencrypted part of every media datagram.
    wire_ext_ids: std::collections::BTreeSet<u8>,
    /// Drop every datagram in both directions (the path is down).
    partition: bool,
    /// The host's ICE username fragment, for forging STUN.
    host_ufrag: String,
}

/// The ids of the one-byte header extensions in an RTP packet (RFC 8285).
fn extension_ids(packet: &[u8]) -> Vec<u8> {
    let mut ids = Vec::new();
    if packet[0] & 0x10 == 0 {
        return ids;
    }
    let at = 12 + 4 * usize::from(packet[0] & 0x0f);
    if packet[at..at + 2] != [0xbe, 0xde] {
        return ids;
    }
    let words = usize::from(u16::from_be_bytes([packet[at + 2], packet[at + 3]]));
    let block = &packet[at + 4..at + 4 + 4 * words];
    let mut i = 0;
    while i < block.len() {
        if block[i] == 0 {
            i += 1;
            continue;
        }
        ids.push(block[i] >> 4);
        i += 1 + usize::from(block[i] & 0x0f) + 1;
    }
    ids
}

/// The client's answer for a surround request: str0m's own client only knows stereo Opus, so
/// it answers that and its Opus line is munged to multiopus the way Chromium would send it.
fn multiopus_answer(spec: &OfferSpec, offer: &str, client: &mut Rtc) -> String {
    answer_for(client, offer, spec)
}

impl Link {
    fn new(spec: OfferSpec) -> Self {
        let host_addr: SocketAddr = HOST.parse().unwrap();
        let (mut rtc, offer) = host_offer(&spec, host_addr);
        let host_ufrag = offer
            .sdp
            .lines()
            .find_map(|l| l.strip_prefix("a=ice-ufrag:"))
            .expect("the offer has an ice-ufrag")
            .trim()
            .to_string();
        let mut client = client(&spec, CLIENT.parse().unwrap());
        let answer_text = multiopus_answer(&spec, &offer.sdp, &mut client);
        let (ids, _) = accept_answer(&mut rtc, offer, &answer_text, &spec).unwrap();
        let now = Instant::now();
        let authenticated = Arc::new(AtomicBool::new(false));
        let host = Engine::new(rtc, ids, &spec, host_addr, now, authenticated.clone());
        Self {
            host,
            client,
            client_video: None,
            now,
            start: now,
            authenticated,
            rtp: Vec::new(),
            media: Vec::new(),
            host_events: Vec::new(),
            idr: Vec::new(),
            channel_data: Vec::new(),
            labels: HashMap::new(),
            closed_by_host: Vec::new(),
            drop_rtp: None,
            rtp_seen: 0,
            wire_ext_ids: Default::default(),
            partition: false,
            host_ufrag,
        }
    }

    fn connected(spec: OfferSpec) -> Self {
        let mut link = Self::new(spec);
        for _ in 0..3000 {
            link.step(Duration::from_millis(1));
            let open = |l: &str| {
                link.host_events
                    .contains(&TransportEvent::ChannelOpen(l.into()))
            };
            if link.host.is_connected() && open("control") && open("gamepad") {
                return link;
            }
        }
        panic!("never connected: {:?}", link.host_events);
    }

    /// Move the clock `dt` and carry datagrams until the link is quiet.
    fn step(&mut self, dt: Duration) {
        self.now += dt;
        let (host_addr, client_addr): (SocketAddr, SocketAddr) =
            (HOST.parse().unwrap(), CLIENT.parse().unwrap());
        for _ in 0..200 {
            let mut moved = false;
            self.host.advance(self.now).unwrap();
            self.client.handle_input(Input::Timeout(self.now)).unwrap();
            self.collect_host();
            for out in self.host.take_outgoing() {
                moved = true;
                if out.paced {
                    self.wire_ext_ids.extend(extension_ids(&out.data));
                    let n = self.rtp_seen;
                    self.rtp_seen += 1;
                    if self.drop_rtp == Some(n) {
                        continue;
                    }
                }
                if self.partition {
                    continue;
                }
                let recv = Receive::new(Protocol::Udp, host_addr, client_addr, &out.data).unwrap();
                self.client
                    .handle_input(Input::Receive(self.now, recv))
                    .unwrap();
            }
            loop {
                match self.client.poll_output().unwrap() {
                    Output::Timeout(_) => break,
                    Output::Transmit(t) => {
                        moved = true;
                        let data: Vec<u8> = t.contents.into();
                        if !self.partition {
                            let fate = self.host.handle_datagram(self.now, client_addr, &data);
                            assert_eq!(fate, Datagram::Handled, "the real client's datagram");
                        }
                    }
                    Output::Event(e) => self.on_client_event(e),
                }
            }
            self.collect_host();
            if !moved {
                break;
            }
        }
    }

    fn run(&mut self, total: Duration) {
        for _ in 0..total.as_millis() {
            self.step(Duration::from_millis(1));
        }
    }

    fn collect_host(&mut self) {
        self.host_events.extend(self.host.take_events());
        self.idr.extend(self.host.take_idr_requests());
    }

    fn on_client_event(&mut self, e: Event) {
        match e {
            Event::MediaAdded(added) if added.kind.is_video() => {
                self.client_video = Some(added.mid)
            }
            Event::MediaData(d) => self.media.push(Received {
                pt: *d.pt,
                data: d.data.to_vec(),
                time: d.time,
            }),
            Event::RawPacket(raw) => {
                if let RawPacket::RtpRx(h, _) = *raw {
                    self.rtp.push(Rtp {
                        pt: *h.payload_type,
                        marker: h.marker,
                        timestamp: h.timestamp,
                        seq: h.sequence_number,
                        playout: h.ext_vals.play_delay_min.zip(h.ext_vals.play_delay_max),
                    });
                }
            }
            Event::ChannelOpen(id, label) => {
                self.labels.insert(id, label);
            }
            Event::ChannelData(d) => {
                let label = self.labels.get(&d.id).cloned().unwrap_or_default();
                self.channel_data.push((label, d.data));
            }
            Event::ChannelClose(id) => {
                if let Some(label) = self.labels.get(&id) {
                    self.closed_by_host.push(label.clone());
                }
            }
            _ => {}
        }
    }

    fn at(&self, ms: u64) -> Instant {
        self.start + Duration::from_millis(ms)
    }

    fn video(&mut self, data: Vec<u8>, at_ms: u64, keyframe: bool) {
        let frame = VideoFrame {
            data: data.into(),
            capture: self.at(at_ms),
            keyframe,
        };
        assert!(
            self.host.write_video(self.now, frame).unwrap(),
            "not connected"
        );
    }

    fn audio(&mut self, data: Vec<u8>, at_ms: u64, position: u64) {
        let packet = AudioPacket {
            data,
            capture: self.at(at_ms),
            position,
        };
        assert!(
            self.host.write_audio(self.now, packet).unwrap(),
            "not connected"
        );
    }

    fn client_channel(&self, label: &str) -> ChannelId {
        *self
            .labels
            .iter()
            .find(|(_, l)| l.as_str() == label)
            .unwrap_or_else(|| panic!("no {label} channel"))
            .0
    }

    fn client_writes(&mut self, label: &str, data: &[u8]) {
        let id = self.client_channel(label);
        self.client_writes_to(id, data);
    }

    fn client_writes_to(&mut self, id: ChannelId, data: &[u8]) {
        assert!(self.client.channel(id).unwrap().write(true, data).unwrap());
    }

    fn client_opens(&mut self, label: &str) -> ChannelId {
        let mut api = self.client.sdp_api();
        let id = api.add_channel(label.into());
        assert!(
            api.apply().is_none(),
            "opening a channel should not renegotiate"
        );
        id
    }
}

/// Bytes that can't contain a start code or a valid-looking OBU boundary by accident.
fn filler(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8 % 200).wrapping_add(seed).max(1) | 0x01)
        .collect()
}

fn annex_b(nals: &[Vec<u8>]) -> Vec<u8> {
    nals.iter()
        .flat_map(|n| [&[0, 0, 0, 1][..], n.as_slice()].concat())
        .collect()
}

fn h264_frame(keyframe: bool, size: usize) -> Vec<u8> {
    if keyframe {
        annex_b(&[
            [&[0x67u8][..], &filler(20, 3)].concat(),
            [&[0x68u8][..], &filler(6, 5)].concat(),
            [&[0x65u8][..], &filler(size, 7)].concat(),
        ])
    } else {
        annex_b(&[[&[0x41u8][..], &filler(size, 9)].concat()])
    }
}

fn hevc_frame(keyframe: bool, size: usize) -> Vec<u8> {
    if keyframe {
        annex_b(&[
            [&[0x40u8, 0x01][..], &filler(20, 3)].concat(),
            [&[0x42u8, 0x01][..], &filler(30, 4)].concat(),
            [&[0x44u8, 0x01][..], &filler(6, 5)].concat(),
            [&[0x26u8, 0x01][..], &filler(size, 7)].concat(),
        ])
    } else {
        annex_b(&[[&[0x02u8, 0x01][..], &filler(size, 9)].concat()])
    }
}

fn obu(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![(kind << 3) | 0x02];
    let mut size = payload.len();
    loop {
        let byte = (size & 0x7f) as u8;
        size >>= 7;
        if size == 0 {
            out.push(byte);
            break;
        }
        out.push(byte | 0x80);
    }
    out.extend_from_slice(payload);
    out
}

const TD: [u8; 2] = [0x12, 0x00];

/// A temporal delimiter, then (for a keyframe) a sequence header, then the frame OBU.
fn av1_frame(keyframe: bool, size: usize) -> Vec<u8> {
    let mut out = TD.to_vec();
    if keyframe {
        out.extend(obu(1, &filler(12, 3)));
    }
    out.extend(obu(6, &filler(size, 9)));
    out
}

fn without_td(frame: &[u8]) -> Vec<u8> {
    frame[TD.len()..].to_vec()
}

fn diff(a: &[u8], b: &[u8]) -> String {
    match a.iter().zip(b).position(|(x, y)| x != y) {
        Some(i) => format!(
            "first difference at byte {i} (lengths {} and {})",
            a.len(),
            b.len()
        ),
        None if a.len() != b.len() => {
            format!("same prefix, lengths {} and {}", a.len(), b.len())
        }
        None => "identical".into(),
    }
}

fn rtp_packet(len: usize) -> Outgoing {
    let mut data = vec![0x60u8; len];
    data[0] = 0x80;
    Outgoing {
        destination: CLIENT.parse().unwrap(),
        data,
        paced: true,
    }
}

fn control_packet() -> Outgoing {
    Outgoing {
        destination: CLIENT.parse().unwrap(),
        data: vec![0x16; 100],
        paced: false,
    }
}

/// Paced at a configured cap, in kbps, on a route of unknown speed.
fn capped(kbps: i64) -> Pacing {
    Pacing::Paced {
        configured_kbps: kbps,
        link_bps: 0,
        wireless: false,
    }
}

fn detached(
    options: TransportOptions,
) -> (
    MediaHandle,
    mpsc::Receiver<Command>,
    Arc<Mutex<Vec<IdrReason>>>,
) {
    let idr = Arc::new(Mutex::new(Vec::new()));
    let sink = idr.clone();
    let hooks = Hooks::new(move |r| sink.lock().unwrap().push(r), |_| {});
    let (tx, rx) = mpsc::channel();
    let shared = Arc::new(Shared {
        stop: AtomicBool::new(false),
        wake: Signal::new().unwrap(),
        authenticated: Arc::new(AtomicBool::new(false)),
        pending_video: AtomicUsize::new(0),
        pending_audio: AtomicUsize::new(0),
        pending_datagrams: AtomicUsize::new(0),
        stream_kbps: AtomicU32::new(30_000),
        need_keyframe: AtomicBool::new(true),
        last_idr_ms: AtomicU64::new(0),
        epoch: Instant::now(),
        counters: Counters::default(),
        hooks,
        options,
        peer: std::sync::Mutex::new(None),
    });
    (
        MediaHandle {
            tx,
            shared,
            threads: Vec::new(),
            local: HOST.parse().unwrap(),
            audio_layout: None,
            codec: VideoCodec::H264,
            followers: Arc::default(),
        },
        rx,
        idr,
    )
}

fn frame(keyframe: bool) -> VideoFrame {
    VideoFrame {
        data: vec![0; 10].into(),
        capture: Instant::now(),
        keyframe,
    }
}

fn frame_of(data: Vec<u8>, keyframe: bool) -> VideoFrame {
    VideoFrame {
        data: data.into(),
        capture: Instant::now(),
        keyframe,
    }
}

fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let started = Instant::now();
    while !done() {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn ice(link: &mut Link, state: IceConnectionState) {
    link.host
        .on_event(link.now, Event::IceConnectionStateChange(state));
}
