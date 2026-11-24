//! Probe confirmation, upward rate probing, learned-ceiling retries, and recovery to Full.

use std::time::{Duration, Instant};

use super::{Change, ChangeKind, Evidence, LinkRate, State, types::*};

impl LinkRate {
    /// How long a step from `from` to `rate` must stay clean. On a path that
    /// has shown no queue, [`CONFIRM_AFTER`]. Behind a deep buffer an
    /// overshoot fills the queue slowly and loses nothing for seconds (the
    /// 2026-11-17 path held 270 ms): the step waits as long as its overshoot
    /// would take to fill the deepest queue this path has shown, up to
    /// [`CONFIRM_MAX`].
    fn confirm_window(&self, from: u64, rate: u64) -> Duration {
        if self.queue_peak.is_zero() || rate <= from {
            return CONFIRM_AFTER;
        }
        let fill = self.queue_peak.mul_f64(rate as f64 / (rate - from) as f64);
        fill.clamp(CONFIRM_AFTER, CONFIRM_MAX)
    }

    /// No evidence this round: confirm earlier probe steps, then probe or
    /// return to `Full`.
    pub(super) fn advance(&mut self, now: Instant, busy: bool, fec: usize, period: Duration) {
        let host_limited = self.host_limited(now);
        // On a path that has shown a queue, a step whose RTT has risen is
        // filling it, however slowly: it is not clean yet.
        let queued = !self.queue_peak.is_zero() && self.ping_fresh(now);
        for slot in &mut self.pending {
            let Some(step) = slot else { continue };
            // A static picture proves nothing: a step sent just before it
            // needs its seconds of motion. Nor does a step past the learned
            // ceiling while the host's own socket refuses frames.
            if !busy || (step.retry && host_limited) {
                step.at = now;
                continue;
            }
            if now.saturating_duration_since(step.at) < step.confirm
                || (queued && self.rtt > step.rtt + QUEUE_MARGIN)
            {
                continue;
            }
            if step.rate > self.good {
                // A failure now goes back to the rate before this step,
                // not to where the climb began.
                self.confirmed = Some(Confirmed {
                    before: self.good,
                    rate: step.rate,
                    at: now,
                    window: step.confirm.max(EVIDENCE_WINDOW),
                    retry: step.retry,
                });
            }
            self.good = self.good.max(step.rate);
            if step.retry {
                // The path carries more than the learned ceiling. Near the
                // rate that failed, it rises by this step and the next
                // retry may follow at once; well under it the edge has moved
                // (a dip that passed), and probes climb at the usual pace.
                if self
                    .failed_at
                    .is_some_and(|failed| step.rate * 100 >= failed * NEAR_BAD_PERCENT)
                {
                    self.learned = Some(step.rate);
                    self.next_retry = Some(now);
                } else {
                    self.learned = None;
                    self.next_retry = None;
                }
            }
            self.hold = HOLD;
            if self.bad.is_none_or(|bad| step.rate > bad) {
                self.bad = None;
                self.knee = HOLD;
                self.knee_until = None;
            }
            *slot = None;
        }
        self.pending.sort_by_key(|s| s.is_none());
        // Quiet video, stale pings or a queue in the path prove nothing.
        if !(busy && self.ping_fresh(now) && !self.rtt.is_zero() && !self.rtt_elevated()) {
            return;
        }
        // A PC that cannot send is the limit: no probe goes up until its
        // socket has been quiet for HOST_LOSS_HOLD.
        if host_limited {
            return;
        }
        let ceiling = self.ceiling_total(fec, period);
        let unconfirmed = self.pending.iter().any(Option::is_some);
        if self.current >= ceiling {
            let since = *self.at_ceiling.get_or_insert(now);
            if !unconfirmed && now.saturating_duration_since(since) >= FULL_AFTER {
                self.finish(now, fec, period);
            }
            return;
        }
        match (self.bad, self.near_bad) {
            (Some(bad), None) if self.current * 100 >= bad * 95 => self.near_bad = Some(now),
            (Some(bad), Some(since)) => {
                if self.current * 100 < bad * 95 {
                    self.near_bad = None;
                } else if now.saturating_duration_since(since) >= FORGET_BAD_AFTER && !unconfirmed {
                    self.bad = None;
                    self.near_bad = None;
                    self.forget_knee();
                    // A minute clean near it: the next failure there is new.
                    if self.learned.is_none() {
                        self.failed_at = None;
                        self.retry_wait = HOLD;
                    }
                }
            }
            _ => {}
        }
        if self.next_probe.is_some_and(|at| now < at) || self.pending[PENDING_CAP - 1].is_some() {
            return;
        }
        let mut divisor = PROBE_STEP_DIVISOR;
        let wait = PROBE_EVERY.max(self.fence_wait());
        if let Some(bad) = self.bad
            && (self.current + self.current / PROBE_STEP_DIVISOR) * 100 >= bad * NEAR_BAD_PERCENT
        {
            if unconfirmed {
                return;
            }
            divisor = NEAR_STEP_DIVISOR;
        }
        // The packet count per frame quantizes the encoder bitrate; a step that
        // changes nothing is no probe.
        let level = self.kbps_for(self.current, fec, period);
        // On a known knee, probes stop just under it until the wait is over.
        let limit = match (self.bad, self.knee_until) {
            (Some(bad), Some(until)) if now < until => {
                ceiling.min((bad * KNEE_CAP_PERCENT).div_ceil(100))
            }
            _ => ceiling,
        };
        // Up to the learned ceiling probes climb as before.
        let under = limit.min(self.learned.unwrap_or(u64::MAX));
        let step = (self.current / divisor).max(1);
        let below = self.climb(self.current, step, under, ceiling, level, fec, period);
        let (next, retry) = match (below, self.learned) {
            (Some(next), _) => (next, false),
            (None, None) => return,
            // At the learned ceiling: hold until a retry is due, then one
            // small step past it, alone, to the next rate the encoder shows.
            (None, Some(learned)) => {
                if unconfirmed || self.next_retry.is_none_or(|at| now < at) {
                    return;
                }
                let from = learned.max(self.current);
                let step = (learned / NEAR_STEP_DIVISOR).max(1);
                match self.climb(from, step, limit, ceiling, level, fec, period) {
                    Some(next) => (next, true),
                    None => return,
                }
            }
        };
        let from_kbps = self.encoder_kbps(fec, period);
        let confirm = self.confirm_window(self.current, next);
        self.current = next;
        self.next_probe = Some(now + wait);
        if let Some(slot) = self.pending.iter_mut().find(|s| s.is_none()) {
            *slot = Some(Step {
                rate: next,
                at: now,
                confirm,
                rtt: self.rtt,
                retry,
            });
        }
        self.probes += 1;
        self.raised_at = Some(now);
        if self.state == State::Floor {
            self.state = State::Limited;
        }
        let to_kbps = self.encoder_kbps(fec, period);
        self.record(
            now,
            Change {
                kind: ChangeKind::Probe,
                reason: "probe",
                from_kbps,
                to_kbps,
                delivered_kbps: (self.delivered_bps / 1000) as u32,
                evidence: Evidence::default(),
            },
        );
    }

    /// The first rate `step` apart above `from`, up to `limit`, at which the
    /// encoder runs faster than at `level`; `None` when there is none above
    /// the current rate. A step to the request itself always counts.
    #[allow(clippy::too_many_arguments)]
    fn climb(
        &self,
        from: u64,
        step: u64,
        limit: u64,
        ceiling: u64,
        level: u32,
        fec: usize,
        period: Duration,
    ) -> Option<u64> {
        let mut next = from;
        while next < limit && self.kbps_for(next, fec, period) <= level {
            next += step;
        }
        let next = next.min(limit);
        (next > self.current && (next >= ceiling || self.kbps_for(next, fec, period) > level))
            .then_some(next)
    }

    fn finish(&mut self, now: Instant, fec: usize, period: Duration) {
        let kbps = self.encoder_kbps(fec, period);
        self.cache = None;
        self.state = State::Full;
        self.reason = "healthy";
        self.bad = None;
        // The path carried the request: no ceiling, no edge and no queue
        // remembered. A later dip is a new edge, handled as the first was.
        self.learned = None;
        self.next_retry = None;
        self.retry_wait = HOLD;
        self.failed_at = None;
        self.queue_peak = Duration::ZERO;
        self.encoder_floor = None;
        self.overshoots = 0;
        self.hold = HOLD;
        self.forget_knee();
        self.at_ceiling = None;
        self.near_bad = None;
        self.pending = [None; PENDING_CAP];
        self.confirmed = None;
        self.record(
            now,
            Change {
                kind: ChangeKind::Full,
                reason: "recovered",
                from_kbps: kbps,
                to_kbps: kbps,
                delivered_kbps: 0,
                evidence: Evidence::default(),
            },
        );
    }
}
