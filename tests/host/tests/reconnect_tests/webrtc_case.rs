//! Codec/audio negotiation case matrix helper for WebRTC reconnect tests.
use super::{AudioMode, OfferSpec, VideoCodec, access_unit, peer_sdp};

/// What the receiver gets for [`access_unit`]: the same, but AV1 travels without its temporal
/// delimiter.
pub(super) fn received(codec: u8, idr: bool) -> Vec<u8> {
    let unit = access_unit(codec, idr);
    if codec == 2 { unit[2..].to_vec() } else { unit }
}

/// What the client asks for and what its answer agrees to.
#[derive(Clone, Copy)]
pub(super) struct Case {
    pub(super) video: VideoCodec,
    pub(super) name: &'static str,
    /// The negotiated codec number: 0 H.264, 1 HEVC, 2 AV1.
    pub(super) codec: u8,
    pub(super) hdr: bool,
    pub(super) channels: u32,
}

impl Case {
    pub(super) const fn new(video: VideoCodec, name: &'static str, codec: u8) -> Self {
        Self {
            video,
            name,
            codec,
            hdr: false,
            channels: 2,
        }
    }

    pub(super) const fn hdr(self) -> Self {
        Self { hdr: true, ..self }
    }

    pub(super) const fn surround(self, channels: u32) -> Self {
        Self { channels, ..self }
    }

    /// The codecs and audio the peer answers with.
    pub(super) fn spec(&self) -> OfferSpec {
        OfferSpec {
            audio: AudioMode::request(self.channels as usize).unwrap(),
            ..peer_sdp::spec(self.video, self.hdr)
        }
    }

    /// The payload of the audio the host sends for this case: Chromium's canonical layouts at
    /// GameStream's surround bitrates, and the protocol's 128 kbps stereo.
    pub(super) fn audio(&self) -> Vec<u8> {
        let (streams, coupled, mapping, kbps): (u8, u8, &[u8], u16) = match self.channels {
            2 => (1, 1, &[0, 1], 128),
            6 => (4, 2, &[0, 4, 1, 2, 3, 5], 256),
            _ => (5, 3, &[0, 6, 1, 2, 3, 4, 5, 7], 450),
        };
        let mut payload = vec![self.channels as u8, streams, coupled];
        payload.extend_from_slice(mapping);
        payload.extend_from_slice(&kbps.to_be_bytes());
        payload
    }
}
