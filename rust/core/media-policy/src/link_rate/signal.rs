//! Periodic evaluation tick, RTT floor and standing-queue detection, and loss/delivery estimation.

use std::time::{Duration, Instant};

use super::{Evidence, LinkRate, State, types::*};

impl LinkRate {
    /// Call every feedback tick (8 ms is fine; it evaluates every 100 ms). Returns
    /// the encoder bitrate to use while limited, `None` while `Full` or off.
    /// `fec` is the FEC percentage now in use, `current_kbps` the bitrate the
    /// encoder runs at, `busy` whether video traffic is busy (at least 60 % of
    /// the frames and bytes expected), and `rtt` the control RTT (zero when
    /// unknown).
    pub fn tick(
        &mut self,
        now: Instant,
        rtt: Duration,
        fec: usize,
        period: Duration,
        current_kbps: u32,
        busy: bool,
    ) -> Option<u32> {
        if !self.is_active() {
            return None;
        }
        self.rtt = rtt;
        self.static_picture = !busy;
        if self.next_eval.is_none_or(|at| now >= at) {
            self.next_eval = Some(now + EVALUATE_EVERY);
            self.observe_rtt(now, rtt);
            self.events.roll(now);
            self.events.prune(now);
            if let Some(why) = self.triggered(now) {
                self.queue_since = None;
                self.on_evidence(now, why, fec, period, current_kbps);
            } else if self.standing_queue(now, busy) {
                self.queue_cut(now, fec, period, current_kbps);
            } else if self.state != State::Full {
                self.advance(now, busy, fec, period);
            }
        }
        (self.state != State::Full).then(|| self.encoder_kbps(fec, period))
    }

    fn observe_rtt(&mut self, now: Instant, rtt: Duration) {
        if !rtt.is_zero() && self.ping_fresh(now) {
            self.minimum_rtt = Some(self.minimum_rtt.map_or(rtt, |m| m.min(rtt)));
            match self.rtt_floor.back_mut() {
                Some((at, low)) if now.saturating_duration_since(*at) < Duration::from_secs(1) => {
                    *low = (*low).min(rtt);
                }
                _ => {
                    if self.rtt_floor.len() >= FLOOR_BUCKETS {
                        self.rtt_floor.pop_front();
                    }
                    self.rtt_floor.push_back((now, rtt));
                }
            }
            while self
                .rtt_floor
                .front()
                .is_some_and(|(at, _)| now.saturating_duration_since(*at) > QUEUE_FLOOR_WINDOW)
            {
                self.rtt_floor.pop_front();
            }
        }
    }

    pub(super) fn rtt_floor(&self) -> Option<Duration> {
        self.rtt_floor.iter().map(|(_, low)| *low).min()
    }

    /// The RTT has been over the floor by [`QUEUE_MARGIN`] in every sample for
    /// [`QUEUE_HOLD`] while video is busy: a lossless queue stands in the path.
    /// A queue that a cut did not change at all is no queue (see
    /// [`QUEUE_FLAT_MS`]): the floor is re-learned and this says no.
    fn standing_queue(&mut self, now: Instant, busy: bool) -> bool {
        let excess = self
            .rtt_floor()
            .filter(|_| !self.rtt.is_zero())
            .map(|floor| self.rtt.saturating_sub(floor));
        if excess.is_some_and(|excess| excess <= QUEUE_MARGIN) {
            // The RTT came back: the queue is over, the next is a new one.
            self.queue_excess = None;
            self.queue_steps = 0;
        }
        if !(busy && self.ping_fresh(now) && excess.is_some_and(|e| e > QUEUE_MARGIN)) {
            self.queue_since = None;
            return false;
        }
        if self.queue_quiet_until.is_some_and(|until| now < until) {
            return false;
        }
        let since = match self.queue_since {
            Some(since) => since,
            None => {
                // Only a queue the stream's own rate can have made: one that
                // starts soon after a probe step or while one is unconfirmed
                // (behind a deep buffer a small step fills it slowly), or goes
                // on after a cut. A slower route or a competing flow shows up
                // as the same rise at any other time.
                let caused = self.queue_excess.is_some()
                    || self.pending.iter().any(Option::is_some)
                    || self
                        .loss_cut_at
                        .is_some_and(|at| now.saturating_duration_since(at) <= QUEUE_RAISED)
                    || self
                        .raised_at
                        .is_some_and(|at| now.saturating_duration_since(at) <= QUEUE_RAISED);
                if !caused {
                    return false;
                }
                self.queue_since = Some(now);
                now
            }
        };
        if now.saturating_duration_since(since) < QUEUE_HOLD {
            return false;
        }
        if let (Some(before), Some(excess)) = (self.queue_excess, excess) {
            let slack = (before / 4).max(Duration::from_millis(QUEUE_FLAT_MS));
            if excess + slack < before {
                // Draining, only slowly: the cut works, wait for it. The
                // ENet round trip is smoothed and lags a queue by seconds.
                self.queue_excess = Some(excess);
                self.queue_since = None;
                return false;
            }
            let flat = excess.abs_diff(before) <= slack;
            if flat || self.queue_steps >= QUEUE_STEPS_MAX {
                // A slower route, or a flow that keeps the queue full whatever
                // the stream sends. Only the queue's own floor is re-learned:
                // `minimum_rtt` stays, so `rtt_elevated` keeps holding probes
                // and the reorder baseline sees the same RTT as before. No
                // new queue is looked for until a probe raises the rate again.
                self.rtt_floor.clear();
                self.rtt_floor.push_back((now, self.rtt));
                self.raised_at = None;
                self.loss_cut_at = None;
                self.queue_since = None;
                self.queue_excess = None;
                self.queue_steps = 0;
                return false;
            }
        }
        true
    }

    pub(super) fn ping_fresh(&self, now: Instant) -> bool {
        self.last_ping
            .is_some_and(|at| now.saturating_duration_since(at) < PING_FRESH)
    }

    pub(super) fn rtt_elevated(&self) -> bool {
        !self.rtt.is_zero()
            && self
                .minimum_rtt
                .is_some_and(|min| self.rtt > min + (min / 2).max(Duration::from_millis(8)))
    }

    /// The RTT over its floor right now.
    pub(super) fn queue_excess_now(&self) -> Duration {
        self.rtt_floor()
            .filter(|_| !self.rtt.is_zero())
            .map_or(Duration::ZERO, |floor| self.rtt.saturating_sub(floor))
    }

    fn triggered(&self, now: Instant) -> Option<Trigger> {
        let e = &self.events;
        let evidence = Evidence {
            unrecoverable_frames: e.unrecoverable.len(),
            send_losses: e.send_losses.len(),
            keyframe_requests: e.requests.len(),
            loss_windows: e.hot,
            loss_share: 0.,
        };
        let latch_failed = self.watching(now) && e.latch_failed;
        let (reason, share_known) = if evidence.unrecoverable_frames >= UNRECOVERABLE_FRAMES
            || (latch_failed && evidence.unrecoverable_frames > 0)
        {
            ("unrecoverable_frames", true)
        } else if evidence.loss_windows >= LOSS_WINDOWS {
            ("fec_loss_share", true)
        } else if evidence.send_losses >= SEND_LOSS_FRAMES {
            // Refused sends with a queue standing in the path are the path's
            // (a switch pausing the adapter); the PC's own only when flat.
            if self.host_only(&evidence) {
                ("send_losses", true)
            } else {
                ("send_losses_queued", true)
            }
        } else if ((!self.fec_status_seen || self.probe_unconfirmed(now))
            && evidence.keyframe_requests >= KEYFRAME_REQUESTS)
            || (latch_failed && evidence.keyframe_requests > 0)
        {
            ("keyframe_requests", false)
        } else {
            return None;
        };
        Some(Trigger {
            reason,
            share_known,
            latch_failed,
            evidence,
        })
    }

    /// Frames sent since the fence and since the first frame with loss
    /// evidence, within the evidence window: the video wire rate the host
    /// offered, and the share of its packets the client reports missing.
    pub(super) fn delivered(&self, now: Instant) -> Option<(u64, f64)> {
        let mut frames = 0usize;
        let (mut first, mut last): (Option<Instant>, Option<Instant>) = (None, None);
        let (mut bytes, mut first_bytes, mut packets, mut missing) = (0u64, 0u64, 0u64, 0u64);
        for s in self.ring.iter() {
            let Some(at) = s.at else { continue };
            if now.saturating_duration_since(at) >= EVIDENCE_WINDOW
                || !self.fence.is_none_or(|f| f.admits(Some(s.frame), at))
                || self
                    .events
                    .first_loss
                    .is_some_and(|first| s.frame.wrapping_sub(first) >= 1 << 31)
                || self
                    .events
                    .last_loss
                    .is_some_and(|last| last.wrapping_sub(s.frame) >= 1 << 31)
            {
                continue;
            }
            frames += 1;
            bytes += u64::from(s.bytes);
            packets += u64::from(s.packets);
            missing += u64::from(s.missing);
            if first.is_none_or(|f| at < f) {
                first = Some(at);
                first_bytes = u64::from(s.bytes);
            }
            if last.is_none_or(|l| at > l) {
                last = Some(at);
            }
        }
        let span = last?.saturating_duration_since(first?).as_secs_f64();
        if frames < 2 || span <= 0. || packets == 0 {
            return None;
        }
        let e = &self.events;
        let packet_share = (missing as f64 / packets as f64).min(1.);
        let frame_share =
            ((e.unrecoverable.len() + e.send_losses.len()) as f64 / frames as f64).min(1.);
        Some((
            ((bytes - first_bytes) as f64 * 8. / span) as u64,
            packet_share.max(frame_share).min(MAX_LOSS_SHARE),
        ))
    }

    /// The largest frame the loss can have come from, as the wire rate of a
    /// stream of such frames: a keyframe or a scene cut among small frames may
    /// be what the path lost to, and a lower bitrate makes those smaller. That
    /// is a frame between the first and the last with loss evidence, or one of
    /// the [`LARGE_FRAME_LEAD`] before the first. Frames after the last do not
    /// count: the keyframe that repairs a loss is sent because of it, and it
    /// is in the window by the time the cut is weighed. Rebuilt by error
    /// correction it is no loss evidence (`on_status`, while the RTT is flat);
    /// lost too, it is the last such frame and counts. Without a frame to
    /// place the loss at, every frame of the window counts.
    pub(super) fn largest_frame_bps(&self, now: Instant, period: Duration) -> Option<u64> {
        let e = &self.events;
        let bytes = self
            .ring
            .iter()
            .filter(|s| {
                s.at.is_some_and(|at| {
                    now.saturating_duration_since(at) < EVIDENCE_WINDOW
                        && self.fence.is_none_or(|f| f.admits(Some(s.frame), at))
                }) && e.first_loss.is_none_or(|first| {
                    s.frame.wrapping_sub(first.wrapping_sub(LARGE_FRAME_LEAD)) < 1 << 31
                }) && e
                    .last_loss
                    .is_none_or(|last| last.wrapping_sub(s.frame) < 1 << 31)
            })
            .map(|s| u64::from(s.bytes))
            .max()?;
        Some((bytes as f64 * 8. / period.as_secs_f64().max(1e-6)) as u64)
    }

    /// Frames the host's socket refused, with no client loss and no queue
    /// behind them: the PC's limit, which teaches no learned ceiling. Field
    /// log 2026-11-17: refusals came in the same seconds as hundreds of
    /// unrecoverable frames and a 200 ms round trip, where they are the path's.
    pub(super) fn host_only(&self, evidence: &Evidence) -> bool {
        evidence.send_losses >= SEND_LOSS_FRAMES
            && evidence.unrecoverable_frames < UNRECOVERABLE_FRAMES
            && evidence.loss_windows < LOSS_WINDOWS
            && !self.rtt_elevated()
    }
}
