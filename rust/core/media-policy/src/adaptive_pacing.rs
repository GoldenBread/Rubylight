//! Loss-responsive packet spacing, independent of encoder bitrate.
//!
//! Start with room for the stream's wire demand, then cautiously probe the
//! route's ordinary ceiling. Validated video loss can reduce bursts, but never
//! below the current wire demand plus headroom. This is not a measurement of
//! path capacity: control pings establish activity, not successful video
//! delivery. Silence alone never authorizes faster probing.
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::adaptive_fec::Block;

/// Rolling window for the RTT floor used to detect delay growth without
/// locking onto a one-off minimum from an earlier route state.
const RTT_FLOOR_WINDOW: Duration = Duration::from_secs(30);
const RTT_FLOOR_BUCKETS: usize = 32;
/// Excess RTT over the rolling floor required before an unknown-link wireless
/// stream treats delay as a standing queue rather than ordinary Wi-Fi jitter.
const QUEUE_DELAY_MARGIN: Duration = Duration::from_millis(15);
/// Continuous busy, non-draining queue delay required before stepping down.
const QUEUE_DELAY_HOLD: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Default)]
pub struct Traffic {
    pub frames: u64,
    pub bytes: u64,
    pub packets: u64,
    pub send_losses: u64,
}

/// Reason why automatic burst pacing changed its rate on a control tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PacingReason {
    /// Clean startup feedback allowed a tentative headroom step.
    StartupHeadroom,
    /// Clean post-recovery feedback allowed a tentative probe step.
    RecoveryProbe,
    /// A tentative increase met loss and restored the previous stable rate.
    ProbeLoss,
    /// Sustained or unrecoverable frame loss cut the pacing rate in half.
    PacketLoss,
    /// Mild loss on a wireless client without a known link rate stepped pacing down.
    MinorLoss,
    /// Sustained RTT queue growth on an unknown-link wireless route stepped pacing down.
    QueueDelay,
    /// Control-channel liveness or RTT feedback cleared on an unknown-link wireless route.
    HeadroomCleared,
}

impl PacingReason {
    /// Stable log key for this pacing transition reason.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StartupHeadroom => "startup_headroom",
            Self::RecoveryProbe => "recovery_probe",
            Self::ProbeLoss => "probe_loss",
            Self::PacketLoss => "packet_loss",
            Self::MinorLoss => "minor_loss",
            Self::QueueDelay => "queue_delay",
            Self::HeadroomCleared => "headroom_cleared",
        }
    }
}

/// One pacing rate step returned by [`AdaptivePacing::tick_step`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PacingStep {
    pub old_bps: u64,
    pub new_bps: u64,
    pub reason: PacingReason,
    /// True when `reason` differs from the previous logged step's reason.
    pub reason_changed: bool,
}

pub struct AdaptivePacing {
    enabled: bool,
    initialized: bool,
    startup: bool,
    wireless_unknown_link: bool,
    route_ceiling: u64,
    ceiling: u64,
    floor: u64,
    limit: Option<u64>,
    last_sample: Option<(Instant, Traffic)>,
    last_ping: Option<Instant>,
    /// Lowest RTT per second over [`RTT_FLOOR_WINDOW`], oldest first.
    rtt_floor: VecDeque<(Instant, Duration)>,
    /// Feedback at/before this frame describes the previous rate.
    after_frame: Option<u32>,
    /// Recent frames may report their blocks out of order. Retain whether a
    /// failed block was already counted, independently of a recovered block.
    reported_frames: Vec<(u32, bool)>,
    lost_frames: u32,
    failed_frames: u32,
    lost_packets: u64,
    any_loss: bool,
    clean_since: Option<Instant>,
    unhealthy_since: Option<(PacingReason, Instant)>,
    queue_watch: Option<QueueWatch>,
    last_logged_reason: Option<PacingReason>,
    /// A rate increase is tentative until its feedback interval has elapsed.
    probe: Option<Probe>,
    /// A changed wire budget invalidates feedback for the old rate. The
    /// control loop supplies the new sent-frame fence before sampling again.
    awaiting_fence: bool,
    recovery_hold: Duration,
}

#[derive(Clone, Copy)]
struct QueueWatch {
    started: Instant,
    min_excess: Duration,
}

struct Probe {
    stable: u64,
    started: Instant,
    settle: Duration,
}

impl Default for AdaptivePacing {
    fn default() -> Self {
        Self::new(false)
    }
}

impl AdaptivePacing {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            initialized: false,
            startup: enabled,
            wireless_unknown_link: false,
            route_ceiling: 0,
            ceiling: 0,
            floor: 0,
            limit: None,
            last_sample: None,
            last_ping: None,
            rtt_floor: VecDeque::new(),
            after_frame: None,
            reported_frames: Vec::new(),
            lost_frames: 0,
            failed_frames: 0,
            lost_packets: 0,
            any_loss: false,
            clean_since: None,
            unhealthy_since: None,
            queue_watch: None,
            last_logged_reason: None,
            probe: None,
            awaiting_fence: false,
            recovery_hold: Duration::from_secs(8),
        }
    }

    /// Mark whether this stream goes to a wireless client whose downstream link
    /// speed is unknown, enabling step-down when headroom triggers clear.
    pub fn set_wireless_unknown_link(&mut self, wireless_unknown_link: bool) {
        self.wireless_unknown_link = wireless_unknown_link;
        if !wireless_unknown_link {
            self.unhealthy_since = None;
            self.queue_watch = None;
        }
    }

    fn seed_rate(&self, encoder_kbps: u32) -> u64 {
        u64::from(encoder_kbps)
            .saturating_mul(2000)
            .clamp(self.floor, self.ceiling)
    }

    /// Whether this controller owns the stream's pacing.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    fn effective_ceiling(
        wireless_unknown_link: bool,
        route_ceiling: u64,
        needed: u64,
        encoder_kbps: u32,
    ) -> u64 {
        let route_ceiling = route_ceiling.max(1);
        if wireless_unknown_link {
            crate::network_pacing::wireless_unknown_link_ceiling_bps(
                route_ceiling,
                encoder_kbps,
                needed,
            )
        } else {
            route_ceiling
        }
    }

    /// Refresh on each frame: FEC, FPS, bitrate and the route can change.
    /// The physical/configured ceiling wins when the requested stream cannot
    /// fit. The caller reports that shortfall; this controller cannot fix it.
    pub fn rate(&mut self, route_ceiling: u64, needed: u64, encoder_kbps: u32) -> u64 {
        let previous_rate = self.current();
        let previous_route_ceiling = self.route_ceiling;
        let previous_ceiling = self.ceiling;
        let previous_floor = self.floor;
        self.route_ceiling = route_ceiling.max(1);
        self.ceiling = Self::effective_ceiling(
            self.wireless_unknown_link,
            self.route_ceiling,
            needed,
            encoder_kbps,
        );
        self.floor = needed
            .saturating_add(needed / 4)
            .max(1_000_000)
            .min(self.ceiling);
        if !self.enabled {
            return route_ceiling;
        }
        if !self.initialized {
            self.initialized = true;
            self.limit = Some(self.seed_rate(encoder_kbps));
        } else if self.route_ceiling != previous_route_ceiling
            || self.ceiling != previous_ceiling
            || self.floor != previous_floor
        {
            let route_changed = self.route_ceiling != previous_route_ceiling;
            if route_changed {
                // Route activity and RTT from before a changed route ceiling do
                // not validate probing the replacement route.
                self.last_ping = None;
                self.rtt_floor.clear();
                self.unhealthy_since = None;
                self.queue_watch = None;
            }
            if self.ceiling > previous_ceiling && self.limit.is_none() {
                // A newly faster ceiling says nothing about downstream
                // capacity. Keep the previously confirmed rate and probe the
                // extra headroom using fresh evidence, without reseeding.
                self.limit = Some(previous_rate);
                self.startup = !self.any_loss;
            }
            // A tentative faster rate has not earned promotion. Retain its
            // preceding rate, adjusted for the new demand, and gather fresh
            // evidence instead of confirming across a changed wire budget.
            let retracted_probe = if let Some(probe) = self.probe.take() {
                self.limit = Some(probe.stable);
                if self.any_loss {
                    // The same report may raise FEC before the control loop
                    // samples it. A changed wire floor must not erase this
                    // failed probe or restart fast startup probing.
                    self.startup = false;
                    self.recovery_hold = (self.recovery_hold * 2).min(Duration::from_secs(60));
                }
                true
            } else {
                false
            };
            if let Some(limit) = &mut self.limit {
                *limit = (*limit).clamp(self.floor, self.ceiling);
            }
            let next_rate = self.current();
            if route_changed {
                self.clean_since = None;
                self.last_sample = None;
                self.awaiting_fence = true;
            } else {
                if retracted_probe || next_rate != previous_rate {
                    self.clean_since = None;
                }
                if retracted_probe || next_rate > previous_rate {
                    self.awaiting_fence = true;
                }
            }
        }
        if let Some(limit) = &mut self.limit {
            let next = (*limit).clamp(self.floor, self.ceiling);
            if next != *limit {
                self.probe = None;
                self.clean_since = None;
            }
            *limit = next;
        }
        self.current()
    }

    /// Preview the rate for a possible frame without changing the sender's
    /// demand, tentative probe or feedback fence. Encoder recovery caps use
    /// more parity than ordinary frames; querying that budget must not reset
    /// the controller's evidence on every input picture.
    pub fn preview_rate(&self, route_ceiling: u64, needed: u64, encoder_kbps: u32) -> u64 {
        self.preview_rate_for(
            route_ceiling,
            needed,
            encoder_kbps,
            self.wireless_unknown_link,
        )
    }

    /// Like [`Self::preview_rate`], evaluating the effective ceiling for the
    /// caller's current `wireless_unknown_link` state without mutating `self`.
    pub fn preview_rate_for(
        &self,
        route_ceiling: u64,
        needed: u64,
        encoder_kbps: u32,
        wireless_unknown_link: bool,
    ) -> u64 {
        if !self.enabled {
            return route_ceiling;
        }
        let route_ceiling = route_ceiling.max(1);
        let ceiling =
            Self::effective_ceiling(wireless_unknown_link, route_ceiling, needed, encoder_kbps);
        let floor = needed
            .saturating_add(needed / 4)
            .max(1_000_000)
            .min(ceiling);
        let limit = if !self.initialized {
            Some(u64::from(encoder_kbps).saturating_mul(2000))
        } else if route_ceiling != self.route_ceiling
            || ceiling != self.ceiling
            || floor != self.floor
        {
            // A changed budget retracts an unconfirmed probe. A confirmed
            // fast route retains its previous ceiling when the route grows.
            self.probe.as_ref().map(|probe| probe.stable).or_else(|| {
                self.limit
                    .or_else(|| (ceiling > self.ceiling).then(|| self.current()))
            })
        } else {
            self.limit
        };
        limit.map_or(ceiling, |limit| limit.clamp(floor, ceiling))
    }

    fn current(&self) -> u64 {
        self.limit.unwrap_or(self.ceiling)
    }

    pub fn on_ping(&mut self, now: Instant, payload: &[u8]) {
        if self.enabled && payload.len() == 8 && payload[..4] == 4u32.to_le_bytes() {
            self.last_ping = Some(now);
        }
    }

    /// Call only after protocol validation and block deduplication. Count
    /// distinct lost frames, not their (up to four) block reports. Delayed
    /// reports must not penalize the replacement rate for an earlier burst.
    pub fn on_block(&mut self, frame: u32, last_sent: u32, period: Duration, block: Block) {
        let recent_frames =
            (Duration::from_secs(2).as_nanos() / period.as_nanos().max(1)).clamp(1, 1024) as u32;
        if !self.enabled
            || self.awaiting_fence
            || last_sent.wrapping_sub(frame) >= recent_frames
            || self.after_frame.is_some_and(|after| !newer(frame, after))
            || matches!(block, Block::Clean | Block::Recovered { lost: 0, .. })
        {
            return;
        }
        self.any_loss = true;
        self.clean_since = None;
        if let Block::Recovered { lost, .. } = block {
            self.lost_packets = self.lost_packets.saturating_add(u64::from(lost));
        }
        self.reported_frames
            .retain(|(seen, _)| last_sent.wrapping_sub(*seen) < recent_frames);
        let failed = matches!(block, Block::Unrecoverable);
        if let Some((_, was_failed)) = self
            .reported_frames
            .iter_mut()
            .find(|(seen, _)| *seen == frame)
        {
            if failed && !*was_failed {
                *was_failed = true;
                self.failed_frames = self.failed_frames.saturating_add(1);
            }
        } else {
            self.reported_frames.push((frame, failed));
            self.lost_frames = self.lost_frames.saturating_add(1);
            self.failed_frames = self.failed_frames.saturating_add(u32::from(failed));
        }
    }

    /// The control loop supplies counters and ENet RTT, never the send loop.
    /// Outside tentative probes, cuts require two distinct failed frames, or
    /// multiple recovered frames missing at least 2% of all sent packets.
    /// Sparse, fully repaired losses therefore don't ratchet every connection
    /// to the floor; any loss can reject an unconfirmed increase.
    pub fn tick(
        &mut self,
        now: Instant,
        traffic: Traffic,
        last_sent: Option<u32>,
        period: Duration,
        encoder_kbps: u32,
        rtt: Duration,
    ) -> Option<(u64, u64)> {
        self.tick_step(now, traffic, last_sent, period, encoder_kbps, rtt)
            .map(|step| (step.old_bps, step.new_bps))
    }

    /// Like [`Self::tick`], also reporting the trigger reason and whether the
    /// reason changed since the previous logged step.
    pub fn tick_step(
        &mut self,
        now: Instant,
        traffic: Traffic,
        last_sent: Option<u32>,
        period: Duration,
        encoder_kbps: u32,
        rtt: Duration,
    ) -> Option<PacingStep> {
        if !self.enabled || self.ceiling == 0 {
            return None;
        }
        if self.awaiting_fence {
            self.awaiting_fence = false;
            self.after_frame = last_sent;
            self.lost_frames = 0;
            self.failed_frames = 0;
            self.lost_packets = 0;
            self.any_loss = false;
        }
        let Some((previous, before)) = self.last_sample else {
            self.last_sample = Some((now, traffic));
            return None;
        };
        let elapsed = now.saturating_duration_since(previous);
        // A validated loss during a tentative increase need not wait for a
        // full sampling window before restoring the preceding rate.
        let probe_loss = self.probe.is_some() && self.any_loss;
        if elapsed < Duration::from_millis(500) && !probe_loss {
            return None;
        }
        self.last_sample = Some((now, traffic));
        let frames = traffic.frames.saturating_sub(before.frames);
        let packets = traffic.packets.saturating_sub(before.packets);
        let loss = std::mem::take(&mut self.any_loss) || traffic.send_losses > before.send_losses;
        let substantial = self.failed_frames >= 2
            || (self.lost_frames >= 2 && packets > 0 && self.lost_packets >= packets.div_ceil(50));
        self.lost_frames = 0;
        self.failed_frames = 0;
        self.lost_packets = 0;
        let old = self.current();
        let recent = elapsed <= Duration::from_secs(2);
        let ping = self
            .last_ping
            .is_some_and(|at| now.saturating_duration_since(at) < Duration::from_secs(2));
        if ping && !rtt.is_zero() {
            self.observe_rtt(now, rtt);
        }
        let rtt_floor = self.rtt_floor();
        let queued = rtt_floor.is_some_and(|floor| {
            !rtt.is_zero() && rtt > floor.saturating_add((floor / 2).max(Duration::from_millis(8)))
        });
        let mut reason = None;
        if (substantial || (self.probe.is_some() && loss)) && recent && (frames > 0 || probe_loss) {
            self.unhealthy_since = None;
            self.queue_watch = None;
            let next = if let Some(probe) = self.probe.take() {
                // A failed probe restores the last working rate, and increases
                // the quiet period before another attempt (up to a minute).
                self.recovery_hold = (self.recovery_hold * 2).min(Duration::from_secs(60));
                reason = Some(PacingReason::ProbeLoss);
                probe.stable
            } else {
                reason = Some(PacingReason::PacketLoss);
                old / 2
            };
            self.limit = Some(next.clamp(self.floor, self.ceiling));
            self.startup = false;
            self.after_frame = last_sent;
            self.clean_since = None;
        } else {
            let seconds = elapsed.as_secs_f64();
            let busy = recent
                && frames as f64 >= seconds / period.as_secs_f64().max(0.001) * 0.6
                && traffic.bytes.saturating_sub(before.bytes) as f64 * 8.
                    >= f64::from(encoder_kbps) * 1000. * seconds * 0.6;
            let queue_excess = (busy && ping && !rtt.is_zero())
                .then_some(rtt_floor)
                .flatten()
                .and_then(|floor| {
                    let excess = rtt.saturating_sub(floor);
                    (excess > (floor / 2).max(QUEUE_DELAY_MARGIN)).then_some(excess)
                });
            if loss || !ping || !busy || rtt.is_zero() || queued {
                self.clean_since = None;
                if self.wireless_unknown_link {
                    let clear_reason = if loss {
                        Some(PacingReason::MinorLoss)
                    } else if queue_excess.is_some() {
                        Some(PacingReason::QueueDelay)
                    } else if !ping || rtt.is_zero() {
                        Some(PacingReason::HeadroomCleared)
                    } else {
                        None
                    };
                    let prior_queue_started = if clear_reason != Some(PacingReason::QueueDelay) {
                        self.queue_watch.take().map(|watch| watch.started)
                    } else {
                        None
                    };
                    if let Some(clear_reason) = clear_reason {
                        let ready = if let Some(excess) =
                            queue_excess.filter(|_| clear_reason == PacingReason::QueueDelay)
                        {
                            let prior_headroom_started = match self.unhealthy_since.take() {
                                Some((PacingReason::HeadroomCleared, at)) => Some(at),
                                _ => None,
                            };
                            self.advance_queue_watch(now, excess, prior_headroom_started)
                        } else {
                            let started = match self.unhealthy_since {
                                Some((prev, at)) if prev == clear_reason => at,
                                _ => {
                                    let at = if clear_reason == PacingReason::HeadroomCleared {
                                        prior_queue_started.unwrap_or(now)
                                    } else {
                                        now
                                    };
                                    self.unhealthy_since = Some((clear_reason, at));
                                    at
                                }
                            };
                            let hold = match clear_reason {
                                PacingReason::MinorLoss => Duration::ZERO,
                                _ => Duration::from_secs(2),
                            };
                            now.saturating_duration_since(started) >= hold
                        };
                        let baseline = self.seed_rate(encoder_kbps);
                        if old > baseline && ready {
                            let step = (old / 8).max(1_000_000);
                            let next = old
                                .saturating_sub(step)
                                .max(baseline)
                                .clamp(self.floor, self.ceiling);
                            if next < old {
                                self.limit = Some(next);
                                self.probe = None;
                                self.after_frame = last_sent;
                                if let Some(excess) = queue_excess
                                    .filter(|_| clear_reason == PacingReason::QueueDelay)
                                {
                                    self.queue_watch = Some(QueueWatch {
                                        started: now,
                                        min_excess: excess,
                                    });
                                } else {
                                    self.unhealthy_since = Some((clear_reason, now));
                                }
                                if matches!(
                                    clear_reason,
                                    PacingReason::MinorLoss | PacingReason::QueueDelay
                                ) {
                                    self.startup = false;
                                }
                                reason = Some(clear_reason);
                            }
                        }
                    } else {
                        self.unhealthy_since = None;
                    }
                }
            } else {
                self.unhealthy_since = None;
                self.queue_watch = None;
                if self.limit.is_some() {
                    // The counters describe the interval just completed. Startup
                    // can use that first active interval; recovery retains its
                    // longer quiet hold after an established loss.
                    let clean =
                        *self
                            .clean_since
                            .get_or_insert(if self.startup { previous } else { now });
                    if self.probe.as_ref().is_some_and(|probe| {
                        let settle = if self.startup {
                            probe.settle.max(startup_feedback_wait(rtt))
                        } else {
                            probe.settle
                        };
                        now.saturating_duration_since(probe.started.max(clean)) >= settle
                    }) {
                        self.probe = None;
                        if old == self.ceiling {
                            self.limit = None;
                            self.startup = false;
                        }
                    }
                    let hold = if self.startup {
                        startup_feedback_wait(rtt)
                    } else {
                        self.recovery_hold
                    };
                    if self.probe.is_none() && now.saturating_duration_since(clean) >= hold {
                        if old < self.ceiling {
                            let step = if self.startup {
                                // Smaller probes leave ordinary FEC more room to
                                // repair losses while feedback returns. This is
                                // headroom, not a guarantee of loss-free probing.
                                (old / 8).max(1)
                            } else {
                                (old / 20).max(1_000_000)
                            };
                            reason = Some(if self.startup {
                                PacingReason::StartupHeadroom
                            } else {
                                PacingReason::RecoveryProbe
                            });
                            self.limit = Some(old.saturating_add(step).min(self.ceiling));
                            self.probe = Some(Probe {
                                stable: old,
                                started: now,
                                settle: if self.startup {
                                    hold
                                } else {
                                    Duration::from_secs(2)
                                },
                            });
                            self.after_frame = last_sent;
                        } else if self.startup {
                            // The initial wire-demand floor can already equal a
                            // low physical ceiling. Confirm it without probing.
                            self.limit = None;
                            self.startup = false;
                        }
                    }
                }
            }
        }
        let next = self.current();
        if next == old {
            return None;
        }
        let reason = reason.unwrap_or(PacingReason::RecoveryProbe);
        let reason_changed = self.last_logged_reason != Some(reason);
        self.last_logged_reason = Some(reason);
        Some(PacingStep {
            old_bps: old,
            new_bps: next,
            reason,
            reason_changed,
        })
    }

    fn observe_rtt(&mut self, now: Instant, rtt: Duration) {
        match self.rtt_floor.back_mut() {
            Some((at, low)) if now.saturating_duration_since(*at) < Duration::from_secs(1) => {
                *low = (*low).min(rtt);
            }
            _ => {
                if self.rtt_floor.len() >= RTT_FLOOR_BUCKETS {
                    self.rtt_floor.pop_front();
                }
                self.rtt_floor.push_back((now, rtt));
            }
        }
        while self
            .rtt_floor
            .front()
            .is_some_and(|(at, _)| now.saturating_duration_since(*at) > RTT_FLOOR_WINDOW)
        {
            self.rtt_floor.pop_front();
        }
    }

    fn rtt_floor(&self) -> Option<Duration> {
        self.rtt_floor.iter().map(|(_, low)| *low).min()
    }

    fn advance_queue_watch(
        &mut self,
        now: Instant,
        excess: Duration,
        prior_started: Option<Instant>,
    ) -> bool {
        let Some(watch) = &mut self.queue_watch else {
            let started = prior_started.unwrap_or(now);
            self.queue_watch = Some(QueueWatch {
                started,
                min_excess: excess,
            });
            return now.saturating_duration_since(started) >= QUEUE_DELAY_HOLD;
        };
        if excess < watch.min_excess {
            // ENet's smoothed RTT reached a new lower excess since the hold
            // started, so delay is actively draining toward the floor. Wait
            // instead of stepping pacing down through the decay tail.
            watch.started = now;
            watch.min_excess = excess;
            return false;
        }
        now.saturating_duration_since(watch.started) >= QUEUE_DELAY_HOLD
    }

    pub fn snapshot(&self) -> serde_json::Value {
        serde_json::json!({
            "automatic": self.enabled,
            "startup": self.startup,
            "wireless_unknown_link": self.wireless_unknown_link,
            "probing": self.probe.is_some(),
            "limited": self.enabled && self.current() < self.ceiling,
            "current_kbps": self.current() / 1000,
            "ceiling_kbps": self.ceiling / 1000,
            "wire_floor_kbps": self.floor / 1000,
            "recovery_hold_seconds": self.recovery_hold.as_secs(),
        })
    }
}

fn startup_feedback_wait(rtt: Duration) -> Duration {
    Duration::from_millis(500).max(
        rtt.saturating_mul(2)
            .saturating_add(Duration::from_millis(100)),
    )
}

fn newer(frame: u32, previous: u32) -> bool {
    let distance = frame.wrapping_sub(previous);
    distance > 0 && distance < (1 << 31)
}

#[cfg(test)]
mod tests;
