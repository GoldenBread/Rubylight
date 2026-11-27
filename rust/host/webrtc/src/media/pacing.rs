//! Video RTP pacing queue, batch segmentation and simulated packet loss.

use super::{CATCH_UP_CREDIT, MAX_PACED_BACKLOG, Outgoing, Pacing};
use butterpollo_media_policy::network_pacing;
use butterpollo_win_io::net::Segments;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Paces the video packets and passes everything else straight through.
pub struct SendQueue {
    pacing: Pacing,
    stream_kbps: u32,
    frame_period: Duration,
    ipv6: bool,
    /// When the next paced group may go.
    due: Instant,
    /// Whether `paced` was empty before the next group starts, so an idle gap
    /// does not leave `due` stale in the past.
    idle: bool,
    /// Wire bytes of the biggest frame noted since the paced queue was last empty.
    frame_bytes: usize,
    immediate: VecDeque<Outgoing>,
    paced: VecDeque<Outgoing>,
    /// Wire bytes waiting in `paced`.
    pub(super) paced_wire_bytes: usize,
    loss: Loss,
}

/// Which video datagrams a lossy link would eat: a fixed-seed xorshift, so two runs lose the
/// same share, not the same packets.
struct Loss {
    permille: u32,
    state: u64,
}

impl Loss {
    fn drops(&mut self) -> bool {
        if self.permille == 0 {
            return false;
        }
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        (self.state >> 11) % 1000 < u64::from(self.permille)
    }
}

#[derive(Default)]
pub struct Batch {
    pub packets: Vec<Outgoing>,
    /// Bytes of the paced packets, for [`SendQueue::sent`].
    pub paced_bytes: usize,
    pub paced_packets: usize,
    /// The rate the group was cut for.
    rate_bps: u64,
    /// The socket writes, one per run of equal-sized datagrams to a peer.
    pub segments: Segments,
}

impl SendQueue {
    pub fn new(
        pacing: Pacing,
        stream_kbps: u32,
        frame_period: Duration,
        ipv6: bool,
        now: Instant,
    ) -> Self {
        Self {
            pacing,
            stream_kbps,
            frame_period,
            ipv6,
            due: now,
            idle: true,
            frame_bytes: 0,
            immediate: VecDeque::new(),
            paced: VecDeque::new(),
            paced_wire_bytes: 0,
            loss: Loss {
                permille: 0,
                state: 0x9e37_79b9_7f4a_7c15,
            },
        }
    }

    /// Drops this many video datagrams per thousand at the socket.
    pub fn with_loss(mut self, permille: u32) -> Self {
        self.loss.permille = permille;
        self
    }

    /// Update the live stream bitrate (kbps) used to compute pacing rate and backlog bounds.
    pub fn set_stream_kbps(&mut self, stream_kbps: u32) {
        self.stream_kbps = stream_kbps;
    }

    pub fn stream_kbps(&self) -> u32 {
        self.stream_kbps
    }

    /// Whether the simulated link loses this datagram. Only video, as `webrtc_loss_permille`
    /// promises; `send_due` counts it as a simulated loss, not as sent.
    pub fn lost_in_the_air(&mut self, out: &Outgoing) -> bool {
        out.paced && self.loss.drops()
    }

    /// A frame of this many wire bytes is about to be queued: it sets the rate it needs and the
    /// backlog that is still normal.
    pub fn note_frame(&mut self, wire_bytes: usize) {
        self.frame_bytes = self.frame_bytes.max(wire_bytes);
    }

    pub fn push(&mut self, out: Outgoing) {
        if out.paced && self.pacing != Pacing::Off {
            self.paced_wire_bytes += out.data.len() + network_pacing::overhead(self.ipv6);
            self.paced.push_back(out);
        } else {
            self.immediate.push_back(out);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.immediate.is_empty() && self.paced.is_empty()
    }

    /// When the next batch may go: now for control traffic, the pacer's time for media.
    pub fn due(&self, now: Instant) -> Option<Instant> {
        if !self.immediate.is_empty() {
            Some(now)
        } else if !self.paced.is_empty() {
            Some(self.due)
        } else {
            None
        }
    }

    /// Wire bits per second the pacer works at: `network_pacing::rate_bps` for the route, with
    /// the biggest frame's own demand as the needed rate. A cap (configured, or 4/5 of the
    /// routed link) holds however much a frame needs.
    pub fn rate(&self) -> u64 {
        match self.pacing {
            Pacing::Off => u64::MAX,
            Pacing::Paced {
                configured_kbps,
                link_bps,
                wireless,
            } => {
                let needed =
                    (self.frame_bytes as f64 * 8. / self.frame_period.as_secs_f64()) as u64;
                network_pacing::rate_bps(
                    configured_kbps,
                    self.stream_kbps,
                    link_bps,
                    wireless,
                    needed,
                )
                .max(1)
            }
        }
    }

    /// More paced media waits than the pacer can deliver on time: [`MAX_PACED_BACKLOG`] at the
    /// pacing rate, and never less than two of the biggest frame (a lone keyframe is not a
    /// backlog however long it takes).
    pub fn backlog_exceeded(&self) -> bool {
        if self.pacing == Pacing::Off {
            return false;
        }
        let by_time = self.rate() as f64 / 8. * MAX_PACED_BACKLOG.as_secs_f64();
        let cap = by_time.max(2. * self.frame_bytes as f64);
        self.paced_wire_bytes as f64 > cap
    }

    /// Drop the waiting media. Returns how many packets went.
    pub fn drop_paced(&mut self) -> usize {
        let dropped = self.paced.len();
        self.paced.clear();
        self.paced_wire_bytes = 0;
        self.frame_bytes = 0;
        self.idle = true;
        dropped
    }

    #[cfg(test)]
    pub fn next_batch(&mut self, now: Instant) -> Batch {
        let mut batch = Batch::default();
        self.fill_batch(now, &mut batch);
        batch
    }

    /// Everything unpaced, plus (when due) up to one millisecond of media at the pacing rate and
    /// always at least one datagram. `batch` is emptied first; its allocation is reused.
    pub fn fill_batch(&mut self, now: Instant, batch: &mut Batch) {
        batch.packets.clear();
        batch.packets.extend(self.immediate.drain(..));
        (batch.paced_bytes, batch.paced_packets, batch.rate_bps) = (0, 0, 0);
        if !self.paced.is_empty() {
            if std::mem::take(&mut self.idle) {
                self.due = self.due.max(now);
            }
            if now >= self.due {
                batch.rate_bps = self.rate();
                let datagram = self.paced.front().map_or(0, |p| p.data.len());
                let limit = network_pacing::batch_payload_limit(
                    batch.rate_bps,
                    datagram.max(1),
                    self.ipv6,
                    usize::MAX,
                );
                while let Some(front) = self.paced.front() {
                    if batch.paced_packets > 0 && batch.paced_bytes + front.data.len() > limit {
                        break;
                    }
                    let packet = self.paced.pop_front().expect("front exists");
                    self.paced_wire_bytes = self
                        .paced_wire_bytes
                        .saturating_sub(packet.data.len() + network_pacing::overhead(self.ipv6));
                    batch.paced_bytes += packet.data.len();
                    batch.paced_packets += 1;
                    batch.packets.push(packet);
                }
                if self.paced.is_empty() {
                    self.frame_bytes = 0;
                    self.paced_wire_bytes = 0;
                    self.idle = true;
                }
            }
        }
    }

    /// Called right after the batch's datagrams were written to the socket. The next group is
    /// due one interval after this one was due, so the average rate is exact; a late wake may
    /// send the next groups back to back, but only [`CATCH_UP_CREDIT`] behind schedule.
    pub fn sent(&mut self, completed: Instant, batch: &Batch) {
        if batch.paced_packets == 0 {
            return;
        }
        let wire = batch.paced_bytes + batch.paced_packets * network_pacing::overhead(self.ipv6);
        let interval = Duration::from_secs_f64(wire as f64 * 8. / batch.rate_bps.max(1) as f64);
        let floor = (completed + interval)
            .checked_sub(CATCH_UP_CREDIT)
            .unwrap_or(completed);
        self.due = (self.due + interval).max(floor);
    }
}
