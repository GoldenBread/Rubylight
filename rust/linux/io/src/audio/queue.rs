//! The queue between PipeWire's capture thread and the host's audio thread: interleaved
//! 48 kHz `f32` frames, bounded so a stalled reader loses old audio instead of adding
//! latency, with the discarded frames counted for the host's loss accounting.

use std::collections::VecDeque;

/// The capture's rate: PipeWire's stream converter resamples the device to it.
pub(super) const RATE: u32 = 48_000;
/// The most audio kept, in frames (100 ms); older frames are dropped when more arrive.
const HARD_CAP: usize = RATE as usize / 10;

/// SPA's channel ids (`spa_audio_channel`).
const MONO: u32 = 2;
const FL: u32 = 3;
const FR: u32 = 4;
const FC: u32 = 5;
const LFE: u32 = 6;
const SL: u32 = 7;
const SR: u32 = 8;
const RL: u32 = 12;
const RR: u32 = 13;

/// The SPA positions of `channels` channels in Moonlight's order (FL FR FC LFE BL BR SL SR), or
/// None for a count Moonlight has no layout for.
pub(super) fn positions(channels: usize) -> Option<&'static [u32]> {
    match channels {
        1 => Some(&[MONO]),
        2 => Some(&[FL, FR]),
        6 => Some(&[FL, FR, FC, LFE, RL, RR]),
        8 => Some(&[FL, FR, FC, LFE, RL, RR, SL, SR]),
        _ => None,
    }
}

/// Captured audio waiting for the host.
pub(super) struct Queue {
    channels: usize,
    samples: VecDeque<f32>,
    /// Frames this side discarded since `take_trimmed`.
    trimmed: u64,
    /// Whether the host has been woken for what is queued.
    pub(super) notify: bool,
    /// Whether audio arrived since `take_arrived`.
    arrived: bool,
    /// Why the capture ended, once it has.
    pub(super) error: Option<String>,
}

impl Queue {
    pub(super) fn new(channels: usize) -> Self {
        Self {
            channels,
            samples: VecDeque::new(),
            trimmed: 0,
            notify: false,
            arrived: false,
            error: None,
        }
    }
    /// Whole frames queued.
    pub(super) fn frames(&self) -> usize {
        self.samples.len() / self.channels
    }
    /// Queues interleaved samples (whole frames only), dropping the oldest beyond 100 ms.
    pub(super) fn push(&mut self, samples: impl Iterator<Item = f32>) {
        let before = self.samples.len();
        self.samples
            .extend(samples.map(|v| if v.is_finite() { v.clamp(-1., 1.) } else { 0. }));
        let whole = self.samples.len() - self.samples.len() % self.channels;
        self.samples.truncate(whole);
        if self.samples.len() > before {
            self.arrived = true;
            self.notify = true;
        }
        self.bound(HARD_CAP, HARD_CAP);
    }
    /// When more than `limit` frames are queued, drops the oldest down to `keep`.
    fn bound(&mut self, limit: usize, keep: usize) -> bool {
        let frames = self.frames();
        if frames <= limit {
            return false;
        }
        let drop = frames - keep.min(limit);
        self.samples.drain(..drop * self.channels);
        self.trimmed += drop as u64;
        true
    }
    /// One packet of `frames` into `out` (emptied first), when that much is queued. A backlog of
    /// more than three packets (30 ms at least) is cut to two first, as on Windows.
    pub(super) fn read_into(&mut self, frames: usize, out: &mut Vec<f32>) -> bool {
        out.clear();
        let packet = frames.max(1);
        let limit = (packet * 3).max(RATE as usize * 30 / 1000);
        if self.bound(limit, packet * 2) {
            tracing::debug!("audio capture backlog dropped");
        }
        let want = frames * self.channels;
        if frames == 0 || self.samples.len() < want {
            return false;
        }
        out.extend(self.samples.drain(..want));
        true
    }
    /// Whether audio arrived since the last call.
    pub(super) fn take_arrived(&mut self) -> bool {
        std::mem::take(&mut self.arrived)
    }
    /// Frames discarded since the last call.
    pub(super) fn take_trimmed(&mut self) -> u64 {
        std::mem::take(&mut self.trimmed)
    }
    /// Frames still queued.
    pub(super) fn queued(&self) -> u64 {
        self.frames() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(frames: usize, channels: usize) -> impl Iterator<Item = f32> {
        (0..frames * channels).map(move |i| (i / channels) as f32 / 10_000.)
    }

    #[test]
    fn positions_follow_moonlights_order() {
        assert_eq!(positions(2), Some(&[FL, FR][..]));
        assert_eq!(positions(6).unwrap().len(), 6);
        assert_eq!(positions(8).unwrap()[6..], [SL, SR]);
        assert_eq!(positions(8).unwrap()[4..6], [RL, RR]);
        assert!(positions(3).is_none());
    }

    #[test]
    fn a_packet_needs_whole_frames_and_keeps_the_rest() {
        let mut queue = Queue::new(2);
        let mut out = Vec::new();
        queue.push(ramp(200, 2));
        assert!(!queue.read_into(240, &mut out));
        assert!(out.is_empty());
        queue.push(ramp(100, 2));
        assert!(queue.read_into(240, &mut out));
        assert_eq!(out.len(), 480);
        assert_eq!(queue.frames(), 60);
        assert_eq!(queue.take_trimmed(), 0);
    }

    #[test]
    fn a_partial_frame_waits_for_its_channels() {
        let mut queue = Queue::new(6);
        queue.push(ramp(1, 6).take(5));
        assert_eq!(queue.frames(), 0);
        assert!(!queue.arrived);
    }

    #[test]
    fn a_backlog_is_cut_to_two_packets_and_counted() {
        let mut queue = Queue::new(2);
        let mut out = Vec::new();
        // 60 ms queued against 5 ms packets: more than 30 ms, so cut to 10 ms.
        queue.push(ramp(2880, 2));
        assert!(queue.read_into(240, &mut out));
        assert_eq!(queue.frames(), 240);
        assert_eq!(queue.take_trimmed(), 2880 - 480);
        assert_eq!(queue.take_trimmed(), 0);
        // The oldest audio went: the packet starts at frame 2400.
        assert!((out[0] - 0.24).abs() < 1e-6);
    }

    #[test]
    fn the_queue_never_holds_more_than_100_ms() {
        let mut queue = Queue::new(2);
        queue.push(ramp(9600, 2));
        assert_eq!(queue.frames(), 4800);
        assert_eq!(queue.take_trimmed(), 4800);
        assert_eq!(queue.queued(), 4800);
    }

    #[test]
    fn arrival_is_reported_once_and_bad_samples_are_silenced() {
        let mut queue = Queue::new(1);
        queue.push([f32::NAN, 3., -3., 0.5].into_iter());
        assert!(queue.take_arrived());
        assert!(!queue.take_arrived());
        let mut out = Vec::new();
        assert!(queue.read_into(4, &mut out));
        assert_eq!(out, [0., 1., -1., 0.5]);
    }
}
