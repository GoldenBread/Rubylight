//! Lock-free session video and audio gauges safe to poll from decoders, renderers, or UI threads.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::{audio, video};

/// A session's video figures that are safe to read from inside a decoder's submit: atomics only,
/// never taking the receiver's or decode queue's lock ([`super::Session::video_gauges`]).
#[derive(Clone)]
pub struct VideoGauges(pub(super) Arc<video::VideoShared>);

impl VideoGauges {
    /// Frames dropped because the decode queue overflowed (`VideoStats::decoder_drops`), as of
    /// the last datagram batch received.
    pub fn decoder_drops(&self) -> u64 {
        self.0.counters.decoder_drops.load(Ordering::Relaxed)
    }

    /// Frames waiting in the decode queue now.
    pub fn decode_queue_len(&self) -> usize {
        self.0.queue.len()
    }
}

/// A session's audio figures that are safe to read from any thread without locking the audio
/// queue ([`super::Session::audio_gauges`]).
#[derive(Clone)]
pub struct AudioGauges(pub(super) Arc<audio::AudioShared>);

impl AudioGauges {
    /// `LiGetPendingAudioDuration`: the milliseconds of audio queued for the renderer.
    pub fn pending_audio_ms(&self) -> u32 {
        self.0.pending_ms()
    }
}
