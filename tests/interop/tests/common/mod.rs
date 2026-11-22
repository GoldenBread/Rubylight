//! Shared loopback host/client helpers for the Rubylight-Nightjar interop test binaries.
#![allow(dead_code)]

use anyhow::Result;
use butterpollo::{
    state::{Launch, Session},
    stream::reconnect_harness::{Harness, until},
};
use butterpollo_crypto::crypto;
use butterpollo_media_policy::extensions::FecChoice;
use butterpollo_session::state::Client as PairedClient;
use nightjar_core::{
    control::ControlEvent,
    http::{ClientIdentity, HostClient, LaunchRequest},
    sdp::{StreamConfig, VIDEO_FORMAT_H264},
    session::{
        AudioRenderer, AudioSetup, DecodeStatus, HostInfo, Interrupt, Listener, Location,
        Session as Stream, SessionConfig, VideoDecoder, VideoSetup,
    },
    video::Frame,
};
use std::{
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

pub fn client_identity() -> Arc<ClientIdentity> {
    static ID: OnceLock<Arc<ClientIdentity>> = OnceLock::new();
    ID.get_or_init(|| Arc::new(ClientIdentity::generate().unwrap()))
        .clone()
}

pub fn host_identity() -> &'static crypto::Identity {
    static ID: OnceLock<crypto::Identity> = OnceLock::new();
    ID.get_or_init(|| crypto::Identity::generate().unwrap())
}

/// How long a step may take on a loaded machine.
pub const LOADED: Duration = Duration::from_secs(30);
/// The remote input key and its ID Nightjar launches with.
pub const KEY: [u8; 16] = *b"nightjar-interop";
pub const KEY_ID: i32 = 7;
/// Where the client roams to.
pub const MOVED: [u8; 4] = [127, 0, 0, 2];

/// Nightjar as a device this host paired, with `perm` and the console's `extra` settings.
pub fn device(h: &Harness, perm: u32, extra: &[(&str, bool)]) -> Result<HostClient> {
    let identity = Arc::new(ClientIdentity::generate()?);
    h.h.paired.write().unwrap().clients.push(PairedClient {
        name: "Nightjar".into(),
        uuid: "nightjar".into(),
        cert: identity.certificate_pem().into(),
        perm,
        enabled: true,
        extra: extra
            .iter()
            .map(|(key, value)| ((*key).into(), (*value).into()))
            .collect(),
    });
    Ok(HostClient::new("127.0.0.1", h.ports.http, identity)?
        .with_https_port(h.ports.https)
        .with_server_certificate_pem(&h.h.identity.certificate)?)
}

/// The paired Nightjar device's entry in the host's client list.
pub fn paired<R>(h: &Harness, change: impl FnOnce(&mut PairedClient) -> R) -> R {
    let mut paired = h.h.paired.write().unwrap();
    change(
        paired
            .clients
            .iter_mut()
            .find(|c| c.uuid == "nightjar")
            .expect("the Nightjar device is paired"),
    )
}

/// The only launch waiting for its RTSP handshake.
pub fn pending(h: &Harness) -> Launch {
    let sessions = h.h.sessions.lock().unwrap();
    assert_eq!(sessions.pending.len(), 1, "one launch is pending");
    sessions.pending.values().next().unwrap().clone()
}

/// What Nightjar's decoder, renderer and listener saw.
#[derive(Default)]
pub struct Seen {
    pub setup: Option<VideoSetup>,
    pub frames: usize,
    pub control_messages: Option<Vec<u16>>,
    pub events: Vec<ControlEvent>,
    pub terminated: Option<i32>,
    /// Where the host's USB messages go, as a platform hands them to its forwarder.
    pub usb: Option<Arc<nightjar_core::usb::Forwarder>>,
}
#[derive(Clone, Default)]
pub struct Recorder(pub Arc<Mutex<Seen>>);
impl Recorder {
    pub fn seen<R>(&self, read: impl FnOnce(&Seen) -> R) -> R {
        read(&self.0.lock().unwrap())
    }
    pub fn frames(&self) -> usize {
        self.seen(|s| s.frames)
    }
    pub fn saw(&self, event: impl Fn(&ControlEvent) -> bool) -> bool {
        self.seen(|s| s.events.iter().any(event))
    }
}
impl VideoDecoder for Recorder {
    fn setup(&mut self, setup: &VideoSetup) -> Result<(), i32> {
        self.0.lock().unwrap().setup = Some(*setup);
        Ok(())
    }
    fn submit(&mut self, _: &Frame) -> DecodeStatus {
        self.0.lock().unwrap().frames += 1;
        DecodeStatus::Ok
    }
}
impl AudioRenderer for Recorder {
    fn init(&mut self, _: &AudioSetup) -> Result<(), i32> {
        Ok(())
    }
    fn play(&mut self, _: Option<&[u8]>) {}
}
impl Listener for Recorder {
    fn host_control_messages(&self, messages: &[u16]) {
        self.0.lock().unwrap().control_messages = Some(messages.to_vec());
    }
    fn connection_terminated(&self, error: i32) {
        self.0.lock().unwrap().terminated = Some(error);
    }
    fn control_event(&self, event: ControlEvent) {
        let mut seen = self.0.lock().unwrap();
        let usb = match (&event, &seen.usb) {
            (ControlEvent::Usb(message), Some(usb)) => Some((usb.clone(), message.clone())),
            _ => None,
        };
        seen.events.push(event);
        drop(seen);
        if let Some((usb, message)) = usb {
            usb.on_message(message);
        }
    }
}

/// Launches the fixture's desktop as Nightjar does, checks what reached the host and streams
/// it: the RTSP handshake, ENet control and video, until frames arrive.
pub async fn stream(
    h: &Harness,
    client: &HostClient,
) -> Result<(Launch, Arc<Session>, Stream, Recorder)> {
    stream_with(h, client, None).await
}

/// [`stream`] with Nightjar's video FEC choice.
pub async fn stream_with(
    h: &Harness,
    client: &HostClient,
    fec: Option<FecChoice>,
) -> Result<(Launch, Arc<Session>, Stream, Recorder)> {
    stream_asking(
        h,
        client,
        LaunchRequest {
            fec,
            ..Default::default()
        },
    )
    .await
}

/// [`stream`] with `asked`'s choices; the app, mode and key are the fixture's.
pub async fn stream_asking(
    h: &Harness,
    client: &HostClient,
    asked: LaunchRequest,
) -> Result<(Launch, Arc<Session>, Stream, Recorder)> {
    let info = client.server_info(true)?;
    assert!(info.paired);
    let apps = client.app_list(&info.capabilities)?;
    let desktop = h.h.apps.read().unwrap()[0].clone();
    let app = apps
        .iter()
        .find(|app| app.id as u32 == desktop.id())
        .expect("the desktop is in Nightjar's app list");
    let request = LaunchRequest {
        app_id: app.id,
        app_uuid: app.uuid.clone(),
        width: 640,
        height: 480,
        fps: 60,
        ri_key: KEY,
        ri_key_id: KEY_ID,
        ..asked
    };
    let launched = client.launch(&request, &info)?.expect("a session URL");
    assert_eq!(
        launched.rtsp_session_url,
        Some(format!("rtspenc://127.0.0.1:{}", h.ports.rtsp))
    );
    let launch = pending(h);
    assert_eq!(launch.client.uuid, "nightjar");
    assert_eq!(launch.app_id, desktop.id());
    assert_eq!((launch.key, launch.key_id), (KEY, KEY_ID as u32));
    assert!(launch.rtsp_encrypted, "corever=1 asks for encrypted RTSP");

    let recorder = Recorder::default();
    let mut iv = [0; 16];
    iv[..4].copy_from_slice(&KEY_ID.to_be_bytes());
    let stream = Stream::start(
        &HostInfo {
            address: "127.0.0.1".into(),
            app_version: info.app_version.clone().expect("appversion"),
            rtsp_session_url: launched.rtsp_session_url,
            server_codec_mode_support: info.server_codec_mode_support,
        },
        SessionConfig {
            stream: StreamConfig {
                width: 640,
                height: 480,
                fps: 60,
                // The fixture's packetizer sends 1024-byte packets.
                packet_size: 1024,
                remote_input_aes_key: KEY,
                remote_input_aes_iv: iv,
                supported_video_formats: VIDEO_FORMAT_H264,
                ..Default::default()
            },
            location: Location::Auto,
            ..Default::default()
        },
        Box::new(recorder.clone()),
        Box::new(recorder.clone()),
        Arc::new(recorder.clone()),
        &Interrupt::default(),
    )
    .map_err(|e| anyhow::anyhow!("Nightjar's stream did not start: {e:?}"))?;
    let session = h.h.sessions.lock().unwrap().active[&launch.id].clone();
    until("video at Nightjar's decoder", LOADED, || {
        recorder.frames() > 2
    })
    .await;
    assert!(!session.stopping());
    Ok((launch, session, stream, recorder))
}
