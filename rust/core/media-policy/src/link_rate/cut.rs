//! Loss evidence handling, bitrate cuts, standing-queue cuts, learned ceilings, and knee backoff.

use std::time::{Duration, Instant};

use super::{Change, ChangeKind, Evidence, LinkRate, State, types::*};

impl LinkRate {
    /// A cut or a failed step at `failed` landed on `safe`. A first failure
    /// at a new edge changes nothing else: the knee and the hold deal with
    /// it as before, so a dip that passes costs what it did. A failure at
    /// the same edge as the last one (within [`EDGE_PERCENT`]), or one a
    /// retry caused (`retry_failed`), makes `safe` the learned ceiling and
    /// doubles the wait before the next retry, from [`RETRY_FIRST`] up to
    /// [`RETRY_MAX`]. Once there is a ceiling, every cut lowers it to where
    /// it landed; only a retry that confirms raises it.
    pub(super) fn learn(&mut self, safe: u64, failed: u64, now: Instant, retry_failed: bool) {
        let same_edge = self.failed_at.is_some_and(|at| {
            failed * 100 >= at * (100 - EDGE_PERCENT) && failed * 100 <= at * (100 + EDGE_PERCENT)
        });
        let repeat = retry_failed || same_edge;
        self.retry_wait = if repeat {
            (self.retry_wait * 2).clamp(RETRY_FIRST, RETRY_MAX)
        } else {
            HOLD
        };
        self.failed_at = Some(failed);
        if repeat || self.learned.is_some() {
            self.learned = Some(self.learned.map_or(safe, |learned| learned.min(safe)));
        }
        self.next_retry = Some(now + self.retry_wait);
        self.queue_peak = self
            .queue_peak
            .max(self.queue_excess_now())
            .min(QUEUE_PEAK_MAX);
    }

    pub(super) fn on_evidence(
        &mut self,
        now: Instant,
        why: Trigger,
        fec: usize,
        period: Duration,
        current_kbps: u32,
    ) {
        let from_kbps = if self.state == State::Full {
            current_kbps.min(self.ceiling_kbps)
        } else {
            self.encoder_kbps(fec, period)
        };
        let budget = self.budget();
        if self.state == State::Full {
            self.current = budget
                .total_bps(from_kbps.max(1), fec, period)
                .min(self.ceiling_total(fec, period));
            self.good = self.current;
        }
        let evidence = why.evidence;
        // A host that cannot send is not the path: it limits probes and the
        // rate, but must not teach a network ceiling.
        let host_only = self.host_only(&evidence);
        // A probe is failing: go back to the last good rate, not below it.
        if let Some((failed, back_to, retry)) = self.failing(now) {
            self.pending = [None; PENDING_CAP];
            self.confirmed = None;
            let knee = self.bad.is_some_and(|bad| {
                failed * 100 >= bad * (100 - KNEE_PERCENT)
                    && failed * 100 <= bad * (100 + KNEE_PERCENT)
            });
            self.bad = Some(match self.bad {
                Some(bad) if knee => bad.min(failed),
                _ => failed,
            });
            // A second failure at the knee goes back to under it, not to a rate
            // a step above the first.
            let back_to = match self.bad {
                Some(bad) if knee => back_to
                    .min((bad * KNEE_CAP_PERCENT).div_ceil(100))
                    .max(self.encoder_floor.unwrap_or(0)),
                _ => back_to,
            };
            self.good = back_to;
            self.current = back_to;
            // A failed step found the path's edge: hold under it. A step
            // rolled back by the host's own refused sends says nothing about
            // where that edge is.
            if !host_only {
                self.learn(back_to, failed, now, retry);
            }
            self.hold = (self.hold * 2).min(MAX_HOLD);
            self.next_probe = Some(now + self.hold);
            self.knee_failed(now, knee);
            self.long_knee(now);
            self.failed_probes += 1;
            self.at_ceiling = None;
            self.near_bad = None;
            self.set_fence(now);
            let to_kbps = self.encoder_kbps(fec, period);
            self.record(
                now,
                Change {
                    kind: ChangeKind::ProbeFailed,
                    reason: why.reason,
                    from_kbps,
                    to_kbps,
                    delivered_kbps: (self.delivered_bps / 1000) as u32,
                    evidence,
                },
            );
            return;
        }
        let cur = self.current;
        let estimate = self.delivered(now);
        // A cut was made and the frames sent since still run well above what
        // it asked for, twice running: the encoder has a minimum, and cutting
        // below it only lowers a target nothing follows. Two rounds, because a
        // keyframe among the few frames of one round looks the same.
        let overshoot = match estimate {
            Some((sent, _)) if self.state != State::Full && !host_only => {
                let offered = sent + budget.audio_bps;
                (offered * 100 > cur * ENCODER_OVERSHOOT_PERCENT).then_some((sent, offered))
            }
            _ => None,
        };
        let Some((sent, offered)) = overshoot else {
            self.overshoots = 0;
            return self.cut(now, why, fec, period, from_kbps, estimate);
        };
        self.overshoots += 1;
        if self.overshoots < 2 {
            return self.cut(now, why, fec, period, from_kbps, estimate);
        }
        self.overshoots = 0;
        let observed = offered.min(self.ceiling_total(fec, period));
        self.encoder_floor = Some(observed);
        self.current = observed.max(cur);
        self.good = self.current;
        self.confirmed = None;
        self.pending = [None; PENDING_CAP];
        self.delivered_bps = sent;
        self.at_ceiling = None;
        self.near_bad = None;
        self.reason = "encoder_floor";
        self.next_probe = Some(now + self.hold.max(self.fence_wait()));
        self.set_fence(now);
        let to_kbps = self.encoder_kbps(fec, period);
        self.record(
            now,
            Change {
                kind: ChangeKind::EncoderFloor,
                reason: "encoder_floor",
                from_kbps,
                to_kbps,
                delivered_kbps: (sent / 1000) as u32,
                evidence,
            },
        );
    }

    /// Cut the rate toward what the link delivered, or hold at the floor.
    fn cut(
        &mut self,
        now: Instant,
        why: Trigger,
        fec: usize,
        period: Duration,
        from_kbps: u32,
        estimate: Option<(u64, f64)>,
    ) {
        let cur = self.current;
        let budget = self.budget();
        let mut evidence = why.evidence;
        let host_only = self.host_only(&evidence);
        let floor = self.cut_floor(fec, period).min(cur);
        if cur <= floor {
            // Nothing left to cut. At the encoder's own minimum the stream is
            // already as low as it goes and has said so. Loss at the floor
            // restarts the wait for `Full`: where the floor is the ceiling
            // (PyroWave with no room) the state would otherwise flip between
            // `Floor` and `Full` every ten seconds while the loss goes on.
            self.at_ceiling = None;
            if let Some((sent, share)) = estimate.filter(|_| why.share_known) {
                evidence.loss_share = share;
                self.delivered_bps = (sent as f64 * (1. - share)) as u64;
            }
            if self.state != State::Floor && self.encoder_floor.is_none() {
                self.state = State::Floor;
                self.reason = "floor";
                self.record(
                    now,
                    Change {
                        kind: ChangeKind::Floor,
                        reason: why.reason,
                        from_kbps,
                        to_kbps: from_kbps,
                        delivered_kbps: (self.delivered_bps / 1000) as u32,
                        evidence,
                    },
                );
            }
            self.set_fence(now);
            return;
        }
        // What the stream sent, audio included, on average and in the largest
        // frame the loss can have come from: a cut that lands at or above
        // both is held, not made (see [`HELD_FOR`]).
        let offered = estimate.and_then(|(sent, _)| {
            let largest = self.largest_frame_bps(now, period)?;
            Some(sent.max(largest) + budget.audio_bps)
        });
        // PyroWave fills a share of its budget whatever the picture, so any
        // cut lowers what it sends.
        let holdable = budget.pyrowave_records.is_none()
            && !host_only
            && !why.latch_failed
            && !self.rtt_elevated();
        // The cuts held back lately count as made: this one starts from where
        // they would have left the rate, unless the stream has sent more
        // than that since.
        let held = self.held.filter(|(level, at)| {
            holdable
                && now.saturating_duration_since(*at) < HELD_FOR
                && offered.is_some_and(|offered| offered <= *level)
        });
        let from = held.map_or(cur, |(level, _)| level.min(cur));
        // The first cut of an episode goes down by at most half; one right
        // after it means the estimate was too high, so it may go to the
        // delivered rate. A held cut lowered nothing, so loss after it says
        // nothing about the estimate: the next goes down by half again.
        let repeat = self
            .last_cut
            .is_some_and(|at| now.saturating_duration_since(at) < EVIDENCE_WINDOW);
        let lowest = if repeat {
            REPEAT_CUT_FLOOR_BPS
        } else {
            from * MIN_CUT_PERCENT / 100
        };
        let estimate = estimate.filter(|_| why.share_known);
        // A queue standing at a rate the stream climbed to after an earlier
        // cut: the link edge is just under it, not far below.
        let climbing = self.state == State::Limited && self.bad.is_some() && self.rtt_elevated();
        let target = match estimate {
            Some((sent, share)) => {
                evidence.loss_share = share;
                let delivered = (sent as f64 * (1. - share)) as u64;
                self.delivered_bps = delivered;
                (delivered + budget.audio_bps) * DELIVERED_PERCENT / 100
            }
            None => {
                self.delivered_bps = 0;
                // The latch did not help, so the loss is the rate, by an
                // amount nothing here measured: a client waiting for a
                // keyframe reports almost nothing. A 30 % cut left r50-hevc-hi
                // at 41 Mbps on a 50 Mbit/s link with 75 % of packets lost for
                // another 1.6 s; go as deep as a cut may, and probe back.
                let percent = if why.latch_failed {
                    MIN_CUT_PERCENT
                } else if climbing {
                    KNEE_BACKOFF_PERCENT
                } else {
                    UNKNOWN_SHARE_PERCENT
                };
                from * percent / 100
            }
        }
        .clamp(
            lowest.min(from * MAX_CUT_PERCENT / 100),
            from * MAX_CUT_PERCENT / 100,
        );
        // A cut never lands above a learned ceiling: the path's edge is below
        // that, and holding over it only fills the queue again. A host-side
        // cut is not the path's edge, and it must be free to go below it.
        let target = match (self.learned, host_only) {
            (Some(learned), false) => target.min(learned),
            _ => target,
        };
        let target = target.max(floor);
        if holdable && offered.is_some_and(|offered| target >= offered) {
            // The picture needs less than the cut would leave it, frame by
            // frame: nothing is cut or learned.
            self.held = Some((target, now));
            self.held_cuts += 1;
            self.set_fence(now);
            return;
        }
        self.held = None;
        // The rate the stream now holds, after the floor had its say, is the
        // learned ceiling: loss and queue cuts lower it, nothing else does.
        if !host_only {
            self.learn(target, from, now, false);
        }
        let knee = self
            .bad
            .is_some_and(|bad| from * 100 >= bad * KNEE_CUT_PERCENT);
        self.bad = Some(self.bad.map_or(from, |bad| bad.min(from)));
        self.confirmed = None;
        self.current = target;
        self.good = target;
        // A cut from the knee is a failed probe that confirmed first.
        self.hold = if knee {
            (self.hold * 2).min(MAX_HOLD)
        } else {
            HOLD
        };
        self.next_probe = Some(now + self.hold.max(self.fence_wait()));
        self.knee_failed(now, knee);
        if self.rtt_elevated() {
            self.long_knee(now);
        }
        self.at_ceiling = None;
        self.near_bad = None;
        self.steps_down += 1;
        self.last_cut = Some(now);
        if !host_only {
            self.loss_cut_at = Some(now);
        }
        self.state = State::Limited;
        self.reason = why.reason;
        self.set_fence(now);
        let to_kbps = self.encoder_kbps(fec, period);
        self.record(
            now,
            Change {
                kind: ChangeKind::StepDown,
                reason: why.reason,
                from_kbps,
                to_kbps,
                delivered_kbps: (self.delivered_bps / 1000) as u32,
                evidence,
            },
        );
    }

    /// Step down 12 % for a standing queue, or back to the last good rate when
    /// a probe is the one that raised it, and keep probes under it for the
    /// quiet hold.
    pub(super) fn queue_cut(
        &mut self,
        now: Instant,
        fec: usize,
        period: Duration,
        current_kbps: u32,
    ) {
        let from_kbps = if self.state == State::Full {
            current_kbps.min(self.ceiling_kbps)
        } else {
            self.encoder_kbps(fec, period)
        };
        let budget = self.budget();
        if self.state == State::Full {
            self.current = budget
                .total_bps(from_kbps.max(1), fec, period)
                .min(self.ceiling_total(fec, period));
            self.good = self.current;
        }
        let cur = self.current;
        let floor = self.cut_floor(fec, period).min(cur);
        let excess = self
            .rtt_floor()
            .map_or(Duration::ZERO, |low| self.rtt.saturating_sub(low));
        self.queue_since = None;
        self.queue_quiet_until = Some(now + QUEUE_SETTLE);
        if cur <= floor {
            // Nothing to cut; a delay alone never makes a floor.
            self.queue_excess = None;
            return;
        }
        self.queue_excess = Some(excess);
        self.queue_steps += 1;
        // A probe that raised the rate into this queue is the failed one.
        let failing = self.failing(now);
        let back_to = failing.map(|(_, back_to, _)| back_to);
        self.pending = [None; PENDING_CAP];
        self.confirmed = None;
        if back_to.is_some() {
            self.failed_probes += 1;
        }
        let mut target = cur * QUEUE_CUT_PERCENT / 100;
        if let Some(back_to) = back_to {
            target = target.min(back_to);
        }
        // A queue above a learned ceiling is the ceiling's fault; never cut
        // to a rate over what the path was last seen to carry.
        if let Some(learned) = self.learned {
            target = target.min(learned);
        }
        let target = target.max(floor);
        let (failed, retry) = failing.map_or((cur, false), |(rate, _, retry)| (rate, retry));
        self.learn(target, failed, now, retry);
        let knee = self
            .bad
            .is_some_and(|bad| cur * 100 >= bad * KNEE_CUT_PERCENT);
        self.bad = Some(self.bad.map_or(cur, |bad| bad.min(cur)));
        self.current = target;
        self.good = target;
        self.hold = if knee {
            (self.hold * 2).min(MAX_HOLD)
        } else {
            HOLD
        };
        self.next_probe = Some(now + self.hold.max(self.fence_wait()));
        self.knee_failed(now, knee);
        self.long_knee(now);
        self.at_ceiling = None;
        self.near_bad = None;
        if back_to.is_none() {
            self.steps_down += 1;
        }
        self.last_cut = Some(now);
        self.state = State::Limited;
        self.reason = "queue";
        self.set_fence(now);
        let to_kbps = self.encoder_kbps(fec, period);
        self.record(
            now,
            Change {
                kind: if back_to.is_some() {
                    ChangeKind::ProbeFailed
                } else {
                    ChangeKind::StepDown
                },
                reason: "queue",
                from_kbps,
                to_kbps,
                delivered_kbps: (self.delivered_bps / 1000) as u32,
                evidence: Evidence::default(),
            },
        );
    }

    /// A failure at the bad rate (`knee`) doubles the wait that keeps probes
    /// under it; any other failure leaves the bad rate somewhere new.
    fn knee_failed(&mut self, now: Instant, knee: bool) {
        if knee {
            self.knee = (self.knee * 2).min(KNEE_MAX_HOLD);
            self.knee_until = Some(now + self.knee);
        } else {
            self.knee = HOLD;
            self.knee_until = None;
        }
    }

    /// A probe that failed, or a cut with a queue standing in the path, found
    /// the link's edge: keep probes under it for a long quiet hold, then let
    /// a small step past it show whether the link improved.
    fn long_knee(&mut self, now: Instant) {
        self.knee = self.knee.max(KNEE_QUIET);
        self.knee_until = Some(now + self.knee);
    }

    /// The bad rate is gone (passed or forgotten): probe freely again.
    pub(super) fn forget_knee(&mut self) {
        self.knee = HOLD;
        self.knee_until = None;
    }
}
