//! WebRTC media socket preparation, answer acceptance and stream pipeline startup.

use super::{BIND_WAIT, Bridge};
use crate::{
    guests::{self, OwnerGuard},
    input,
    media::{self, MediaHandle, Pacing, TransportOptions},
    sdp::{self, AudioMode, OfferSpec, VideoCodec, VideoMode},
};
use anyhow::{Context, Result, anyhow, bail};
use butterpollo_media_policy::{
    audio::OpusLayout,
    rtsp::{Negotiated, webrtc_audio_kbps},
};
use butterpollo_session::webrtc::capabilities::{Codec, StreamSettings};
use butterpollo_stream::{
    network::{media_address, this_pc_address},
    state::{Launch, Session, Shared},
    stream::{Media, cap_stream_resolution},
};
use std::{
    net::{IpAddr, SocketAddr, UdpSocket},
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};

/// Why an answered session did not start.
pub(super) enum Rejected {
    /// The answer was not one for this offer.
    Answer(anyhow::Error),
    /// The host could not start the transport or the session, which may have been replaced.
    Host(anyhow::Error),
}

/// The transport pieces between building the offer and the client's answer.
pub(super) struct Prepared {
    socket: Option<UdpSocket>,
    local: SocketAddr,
    rtc: Option<str0m::Rtc>,
    offer: Option<sdp::Offer>,
    spec: OfferSpec,
}

impl Prepared {
    pub(super) fn offer_sdp(&self) -> String {
        self.offer
            .as_ref()
            .map(|offer| offer.sdp.clone())
            .unwrap_or_default()
    }
}

/// Bind the media socket and build the offer.
pub(super) async fn prepare(
    h: &Shared,
    signalled_local: IpAddr,
    media_bind: IpAddr,
    peer: SocketAddr,
    this_pc: bool,
    settings: &StreamSettings,
) -> Result<Prepared> {
    let port = h.config.read().unwrap().webrtc_media_port();
    // The previous session's worker frees the port as it ends, which can be moments after the
    // client was told the session is over.
    let deadline = tokio::time::Instant::now() + BIND_WAIT;
    let address = media_address(signalled_local, media_bind, this_pc_address);
    let socket = loop {
        match media::bind_socket((address, port).into()) {
            Ok(socket) => break socket,
            Err(error)
                if tokio::time::Instant::now() < deadline
                    && error
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|e| e.kind() == std::io::ErrorKind::AddrInUse) =>
            {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(error) => return Err(error),
        }
    };
    let local = socket.local_addr().context("reading the media address")?;
    tracing::info!(media = %local, signalled = %signalled_local, %peer, this_pc, "WebRTC offer's host candidate");
    let spec = OfferSpec {
        video: VideoMode {
            codec: match settings.codec {
                Codec::H264 => VideoCodec::H264,
                Codec::Hevc => VideoCodec::Hevc,
                Codec::Av1 => VideoCodec::Av1,
            },
            width: settings.width,
            height: settings.height,
            fps: settings.fps,
            hdr: settings.hdr,
            bitrate_kbps: settings.bitrate_kbps,
        },
        audio: AudioMode::request(settings.audio_channels as usize)?,
        // NACK and RTX, on by default; `webrtc_rtx = false` leaves recovery to keyframes.
        rtx: h.config.read().unwrap().webrtc_rtx(),
    };
    let mut rtc = sdp::rtc_config(&spec)?.build(Instant::now());
    rtc.add_local_candidate(str0m::Candidate::host(local, "udp").context("the host candidate")?);
    let offer = sdp::create_offer(&mut rtc, &spec)?;
    Ok(Prepared {
        socket: Some(socket),
        local,
        rtc: Some(rtc),
        offer: Some(offer),
        spec,
    })
}

/// Release a launch no stream took, and its transport if nobody took that either.
pub(super) fn cancel_launch(h: &Shared, launch: Launch) {
    h.webrtc_links.discard(&launch.id);
    let canceled = h.sessions.lock().unwrap().request_stop(Some(&launch.id));
    drop(canceled);
}

/// Accept the answer, start the transport and the stream worker. Blocking: it routes, starts the
/// transport's threads and takes the session lock. A launch that does not start is cancelled here,
/// where releasing what it holds may block too.
#[allow(clippy::too_many_arguments)]
pub(super) fn go_live(
    h: &Shared,
    media: &Arc<Media>,
    peer: SocketAddr,
    launch: Launch,
    settings: StreamSettings,
    mut prepared: Prepared,
    answer: &str,
    bridge: &Arc<Bridge>,
) -> Result<(Arc<Session>, Option<OwnerGuard>), Rejected> {
    let started = start_live(
        h,
        media,
        peer,
        &launch,
        &settings,
        &mut prepared,
        answer,
        bridge,
    );
    if started.is_err() {
        drop(prepared);
        cancel_launch(h, launch);
    }
    started
}

/// The Opus layout the audio thread encodes: the one the client answered with for 5.1 and 7.1,
/// stereo otherwise, at the bitrate [`Negotiated::webrtc`] took out of the video budget for it.
pub(super) fn audio_layout(channels: u8, answered: Option<OpusLayout>) -> Result<OpusLayout> {
    let mut layout = match (channels, answered) {
        (2, _) => OpusLayout::select(2, false, None)?,
        (6 | 8, Some(layout)) if layout.channels == usize::from(channels) => layout,
        (6 | 8, _) => bail!("the answer has no {channels}-channel multiopus layout"),
        _ => bail!("unsupported audio channel count {channels}"),
    };
    layout.bitrate = webrtc_audio_kbps(channels) as i32 * 1000;
    Ok(layout)
}

#[allow(clippy::too_many_arguments)]
fn start_live(
    h: &Shared,
    media: &Arc<Media>,
    peer: SocketAddr,
    launch: &Launch,
    settings: &StreamSettings,
    prepared: &mut Prepared,
    answer: &str,
    bridge: &Arc<Bridge>,
) -> Result<(Arc<Session>, Option<OwnerGuard>), Rejected> {
    let (Some(socket), Some(mut rtc), Some(offer)) = (
        prepared.socket.take(),
        prepared.rtc.take(),
        prepared.offer.take(),
    ) else {
        return Err(Rejected::Answer(anyhow!("the offer was already answered")));
    };
    let (ids, answered) =
        sdp::accept_answer(&mut rtc, offer, answer, &prepared.spec).map_err(Rejected::Answer)?;
    tracing::info!(
        codec = ?prepared.spec.video.codec,
        level = ?answered.video_level,
        tier = ?answered.video_tier,
        "WebRTC answer accepted"
    );
    let answered_layout = answered.audio_layout.clone();
    let audio_layout = audio_layout(settings.audio_channels as u8, answered.audio_layout)
        .map_err(Rejected::Answer)?;
    let link = butterpollo_win_io::net::routed_link(peer);
    let (configured_kbps, paced, loss_permille) = {
        let config = h.config.read().unwrap();
        (
            config.manual_pacing_kbps().unwrap_or(0),
            config.webrtc_pacing(),
            config.webrtc_loss_permille(),
        )
    };
    let options = TransportOptions {
        pacing: if paced {
            Pacing::Paced {
                configured_kbps,
                link_bps: link.bps,
                wireless: link.wireless,
            }
        } else {
            Pacing::Off
        },
        loss_permille,
        ..TransportOptions::default()
    };
    // Before the transport, so the client's first frame has somewhere to go.
    let input = input::spawn(h.clone()).map_err(Rejected::Host)?;
    bridge.attach_input(input.clone());
    let handle = MediaHandle::spawn(
        socket,
        prepared.local,
        media::Session {
            rtc,
            ids,
            spec: &prepared.spec,
            // The signalling connection authenticated the device.
            authenticated: Arc::new(AtomicBool::new(true)),
        },
        options,
        bridge.hooks(),
    )
    .map_err(Rejected::Host)?;
    input.control(handle.control_sender());
    let followers = handle.followers();
    h.webrtc_links.put(
        launch.id.clone(),
        Box::new(handle.with_audio_layout(audio_layout)),
    );
    let mut negotiated = Negotiated::webrtc(
        settings.width,
        settings.height,
        settings.fps,
        settings.codec.into(),
        settings.hdr,
        settings.bitrate_kbps,
        settings.audio_channels as u8,
    );
    // The browser's canvas is the display; a host cap shrinks only the picture
    // the encoder codes.
    cap_stream_resolution(&mut negotiated, &h.config.read().unwrap(), &launch.warnings);
    let started = h.sessions.lock().unwrap().start(launch.clone(), negotiated);
    let session = started.map_err(|error| Rejected::Host(error.context("starting the session")))?;
    bridge.bind(&session);
    media.start(h.clone(), session.clone());
    // Only the console's own stream page streams for an owner who can invite guests.
    let owner = launch.client.console().is_some().then(|| {
        OwnerGuard::new(
            h,
            guests::Owner {
                spec: prepared.spec.clone(),
                audio_layout: answered_layout,
                followers,
                input,
                session: Arc::downgrade(&session),
                bind: media.bind(),
            },
        )
    });
    Ok((session, owner))
}
