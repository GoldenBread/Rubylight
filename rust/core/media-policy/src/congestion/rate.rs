//! AIMD delay-based rate control, loss-based rate limits, recovery tracking, and pacing.

use std::time::Duration;

use super::{
    ADDITIVE_INCREASE, APP_LIMITED, Change, ChangeKind, Congestion, DECREASE_FACTOR, Decision,
    FEC_ABSORBS, FORGET_CAPACITY, LOSS_CUT_GAP_US, LOSS_HIGH, LOSS_LOW, MAX_CUT, MIN_ADDITIVE_BPS,
    MULTIPLICATIVE_INCREASE, NEAR_CAPACITY, PACE_FRACTION, PACE_HEADROOM, PACE_SAMPLES,
    PACE_WINDOW_US, RECOVERED_FRACTION, RECOVERIES, RECOVERY_CAP_US, RateState, STALE_US,
};

impl Congestion {
    /// Run the rate control: `fec_percent` is the parity share in use (parity per 100 data
    /// packets).
    pub fn tick(&mut self, now_us: u64, rtt: Duration, fec_percent: u32) -> Decision {
        self.fec_percent = fec_percent;
        let dt = self
            .last_tick
            .replace(now_us)
            .map_or(0., |last| now_us.saturating_sub(last) as f64 / 1e6)
            .min(1.);
        let stale = self
            .last_feedback
            .is_none_or(|at| now_us.saturating_sub(at) >= STALE_US);
        if stale {
            if self.limited {
                self.note(ChangeKind::Stale, self.target_bps(), self.max_bps, 0, 0.);
                self.limited = false;
            }
            self.delay_bps = self.max_bps as f64;
            self.loss_bps = self.max_bps as f64;
            self.state = RateState::Hold;
            self.recovering = None;
            return Decision::default();
        }
        let before = self.target_bps();
        let acked = self.acked_bps();
        let rtt_us = rtt.as_micros().min(u128::from(u64::MAX)) as u64;
        self.delay_rate(now_us, dt, acked, rtt_us);
        self.loss_rate(now_us, dt, before, rtt_us);
        let target = self.target_bps();
        self.track_recovery(now_us, before, target);
        if target >= self.max_bps && self.limited {
            self.limited = false;
            self.note(ChangeKind::Restored, before, target, acked.unwrap_or(0), 0.);
        } else if target < self.max_bps {
            self.limited = true;
        }
        Decision {
            video_bps: (target < self.max_bps).then_some(target),
            pace_bps: self.pace_bps(now_us),
        }
    }

    fn delay_rate(&mut self, now_us: u64, dt: f64, acked: Option<u64>, rtt_us: u64) {
        match self.state {
            RateState::Hold => {}
            RateState::Increase => {
                let limit = acked.map(|a| APP_LIMITED * a as f64 + 10_000.);
                if limit.is_some_and(|limit| self.delay_bps >= limit) {
                    return;
                }
                let near = self
                    .capacity_bps
                    .is_some_and(|cap| self.delay_bps >= NEAR_CAPACITY * cap);
                let grown = if near {
                    self.delay_bps + (ADDITIVE_INCREASE * self.delay_bps).max(MIN_ADDITIVE_BPS) * dt
                } else {
                    self.delay_bps * (1. + MULTIPLICATIVE_INCREASE).powf(dt)
                };
                let grown = limit.map_or(grown, |limit| grown.min(limit.max(self.delay_bps)));
                self.delay_bps = grown.min(self.max_bps as f64);
                // The link carries more than it did: forget the old capacity.
                if let (Some(cap), Some(a)) = (self.capacity_bps, acked)
                    && a as f64 > FORGET_CAPACITY * cap
                {
                    self.capacity_bps = None;
                }
            }
            RateState::Decrease => {
                let due = self
                    .last_decrease
                    .is_none_or(|at| now_us.saturating_sub(at) >= rtt_us);
                if let (true, Some(acked)) = (due, acked) {
                    if self.application_limited() {
                        // Neither a cut nor a sample of the link's capacity.
                        self.counters.overuse_held += 1;
                    } else {
                        let from = self.target_bps();
                        let aimed = DECREASE_FACTOR * acked as f64;
                        if aimed < self.delay_bps {
                            self.delay_bps = aimed.max(self.delay_bps * (1. - MAX_CUT));
                            self.delay_bps = self.delay_bps.max(self.min_bps as f64);
                            self.counters.overuse_cuts += 1;
                            self.last_decrease = Some(now_us);
                            self.note(ChangeKind::Overuse, from, self.target_bps(), acked, 0.);
                        }
                        self.capacity_bps = Some(match self.capacity_bps {
                            Some(cap) => 0.95 * cap + 0.05 * acked as f64,
                            None => acked as f64,
                        });
                    }
                }
                self.state = RateState::Hold;
            }
        }
    }

    /// The share of packets lost that FEC no longer carries.
    fn loss_threshold(&self) -> f64 {
        let parity = f64::from(self.fec_percent) / (100. + f64::from(self.fec_percent));
        LOSS_HIGH.max(FEC_ABSORBS * parity)
    }

    fn loss_rate(&mut self, now_us: u64, dt: f64, before: u64, rtt_us: u64) {
        let Some(loss) = self.loss() else {
            return;
        };
        if loss > self.loss_threshold() {
            let due = self
                .last_loss_cut
                .is_none_or(|at| now_us.saturating_sub(at) >= LOSS_CUT_GAP_US + rtt_us);
            if due {
                let cut = (before as f64 * (1. - 0.5 * loss)).max(self.min_bps as f64);
                self.loss_bps = cut;
                // The delay-based rate starts its climb from here too.
                self.delay_bps = self.delay_bps.min(cut);
                self.last_loss_cut = Some(now_us);
                self.counters.loss_cuts += 1;
                self.note(ChangeKind::Loss, before, self.target_bps(), 0, loss);
            }
        } else if loss < LOSS_LOW {
            self.loss_bps =
                (self.loss_bps * (1. + MULTIPLICATIVE_INCREASE).powf(dt)).min(self.max_bps as f64);
        }
    }

    fn track_recovery(&mut self, now_us: u64, before: u64, target: u64) {
        if target < before && self.recovering.is_none() {
            self.recovering = Some((now_us, before as f64));
        }
        if let Some((since, from)) = self.recovering {
            let elapsed = now_us.saturating_sub(since);
            if target as f64 >= RECOVERED_FRACTION * from || elapsed >= RECOVERY_CAP_US {
                self.recovering = None;
                if self.recoveries.len() == RECOVERIES {
                    self.recoveries.pop_front();
                }
                self.recoveries.push_back(elapsed.min(RECOVERY_CAP_US));
            }
        }
    }

    pub(super) fn pace_bps(&self, now_us: u64) -> Option<u64> {
        let recent = self
            .bottleneck
            .iter()
            .filter(|(at, _)| at + PACE_WINDOW_US >= now_us)
            .count();
        if recent < PACE_SAMPLES {
            return None;
        }
        let pace =
            (self.bottleneck_bps? * PACE_FRACTION).max(self.target_bps() as f64 * PACE_HEADROOM);
        Some(pace as u64)
    }

    fn note(&mut self, kind: ChangeKind, from_bps: u64, to_bps: u64, acked_bps: u64, loss: f64) {
        self.change = Some(Change {
            kind,
            from_bps,
            to_bps,
            acked_bps,
            loss,
        });
    }
}
