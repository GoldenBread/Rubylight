//! The stream doctor: what the last 30 seconds of stats ticks show, the rhythm of network stalls
//! over 20 minutes, and the reversible bitrate trial it may offer.
//!
//! [`StreamHealth`] and [`period_ms`] are Android's (`StreamHealth.java`, `PeriodicStalls.java`).
//! [`BitrateTrial`] is the trial Android's `Game` runs (docs/stream-health.md). Platform facts such
//! as the thermal level arrive in each [`Stats`] tick; nothing here reads the device. Findings
//! describe evidence, not proven root causes.

use std::collections::VecDeque;

use crate::stats::Stats;

pub const HISTORY_MS: u64 = 30_000;
const MAX_SAMPLES: usize = 30;
/// Stall starts are kept longer than samples so a rhythm of several minutes can show.
pub const STALL_HISTORY_MS: u64 = 20 * 60_000;
const MAX_STALLS: usize = 64;
/// A tick closer than this to the previous sample repeats it.
const MIN_SAMPLE_GAP_MS: u64 = 900;
/// Advice needs a fresh sample at most this old.
const RECENT_MS: u64 = 2_500;

pub const MIN_PERIOD_MS: u64 = 30_000;
pub const MAX_PERIOD_MS: u64 = 10 * 60_000;
/// Events closer than this belong to one stall.
pub const BURST_GAP_MS: u64 = 2_500;
/// A rhythm needs this many stalls on its beat, i.e. at least three repeats.
pub const MIN_HITS: usize = 4;
const MIN_TOLERANCE_MS: u64 = 2_000;

/// The bitrate trial never goes below this.
pub const TRIAL_FLOOR_KBPS: u32 = 500;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Finding {
    Network,
    Decoder,
    Host,
    Presentation,
    Heat,
    Periodic,
    /// Frames arrived in bursts: late against the host's clock in many seconds (see
    /// [`BURST_MIN_VOTES`]).
    Bursts,
}

/// [`Finding::Bursts`] needs this many votes and a third of the samples. A sample with late frames
/// ([`Stats::late_frames`]) is a vote, but a run of consecutive late samples gives at most
/// [`BURST_RUN_VOTES`] unless it is longer than [`BURST_LONG_RUN`]: a launch or a switch to
/// fullscreen holds frames back for a few seconds in a row once, a Wi-Fi that bunches packets does
/// it again and again, or without a break.
pub const BURST_MIN_VOTES: u32 = 6;
/// The votes of one short run of late samples: one transition can never reach [`BURST_MIN_VOTES`].
pub const BURST_RUN_VOTES: u32 = 2;
/// A run of late samples longer than this is no transition and votes in full.
pub const BURST_LONG_RUN: u32 = 10;

/// The [`Finding::Bursts`] votes of a run of `run` consecutive late samples.
fn run_votes(run: u32) -> u32 {
    if run > BURST_LONG_RUN { run } else { run.min(BURST_RUN_VOTES) }
}

impl Finding {
    pub const ALL: [Finding; 7] = [
        Finding::Network,
        Finding::Decoder,
        Finding::Host,
        Finding::Presentation,
        Finding::Heat,
        Finding::Periodic,
        Finding::Bursts,
    ];

    /// Java's enum name, as the traces spell it.
    pub fn name(self) -> &'static str {
        match self {
            Finding::Network => "NETWORK",
            Finding::Decoder => "DECODER",
            Finding::Host => "HOST",
            Finding::Presentation => "PRESENTATION",
            Finding::Heat => "HEAT",
            Finding::Periodic => "PERIODIC",
            Finding::Bursts => "BURSTS",
        }
    }
}

/// The frame interval, or NaN without a frame rate.
fn budget_ms(s: &Stats) -> f32 {
    if s.fps > 0 {
        1000.0 / s.fps as f32
    } else {
        f32::NAN
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub findings: Vec<Finding>,
    pub samples: usize,
    pub recent: bool,
    /// The worst values among the counted samples, or -1.
    pub worst_loss_percent: f32,
    pub worst_decode_ms: f32,
    pub worst_host_p95_ms: f32,
    pub worst_display_p95_ms: f32,
    /// Network stalls started in the last [`STALL_HISTORY_MS`], and their rhythm (0 when none).
    pub stalls: usize,
    pub stall_period_ms: u64,
    /// True when `worst_host_p95_ms` comes from a sample whose host time includes the send.
    pub host_includes_send: bool,
    /// Counted samples with late frames, the evidence of [`Finding::Bursts`].
    pub late_samples: usize,
}

impl Report {
    pub fn can_suggest_bitrate(&self) -> bool {
        self.recent && self.findings.contains(&Finding::Network)
    }
}

/// Bounded, identifier-free observations, fed once per stats tick.
#[derive(Clone, Debug, Default)]
pub struct StreamHealth {
    samples: VecDeque<Stats>,
    stall_starts: Vec<u64>,
    last_stall_ms: Option<u64>,
    stall_period_ms: u64,
}

impl StreamHealth {
    /// Records a stats tick. The counters count only while `recent_video` says they are current.
    pub fn add(&mut self, stats: &Stats) {
        if self
            .samples
            .back()
            .is_some_and(|last| stats.now_ms < last.now_ms.saturating_add(MIN_SAMPLE_GAP_MS))
        {
            return;
        }
        self.expire(stats.now_ms);
        while self.samples.len() >= MAX_SAMPLES {
            self.samples.pop_front();
        }
        self.samples.push_back(*stats);
        if stats.recent_video && budget_ms(stats).is_finite() && network_evidence(stats) {
            if self.last_stall_ms.is_none_or(|last| {
                stats.now_ms.saturating_sub(last) > BURST_GAP_MS
            }) {
                if self.stall_starts.len() == MAX_STALLS {
                    self.stall_starts.remove(0);
                }
                self.stall_starts.push(stats.now_ms);
                self.stall_period_ms = period_ms(&self.stall_starts);
            }
            self.last_stall_ms = Some(stats.now_ms);
        }
    }

    fn expire(&mut self, now_ms: u64) {
        while self
            .samples
            .front()
            .is_some_and(|first| now_ms.saturating_sub(first.now_ms) > HISTORY_MS)
        {
            self.samples.pop_front();
        }
        let old = self
            .stall_starts
            .iter()
            .take_while(|&&at| now_ms.saturating_sub(at) > STALL_HISTORY_MS)
            .count();
        if old > 0 {
            self.stall_starts.drain(..old);
            self.stall_period_ms = period_ms(&self.stall_starts);
        }
    }

    /// The rhythm of recent network stalls in ms, or 0. Cheap: updated only when a stall starts
    /// or expires.
    pub fn stall_period_ms(&self) -> u64 {
        self.stall_period_ms
    }

    pub fn report(&mut self, now_ms: u64) -> Report {
        self.expire(now_ms);
        let mut votes = [0u32; Finding::ALL.len()];
        let mut valid = 0usize;
        let (mut loss, mut decode, mut host, mut display) = (-1f32, -1f32, -1f32, -1f32);
        let mut host_includes_send = false;
        // Burst votes per run of late samples (`late_samples` keeps every late sample).
        let (mut burst_votes, mut run) = (0u32, 0u32);
        for s in &self.samples {
            let budget = budget_ms(s);
            if !s.recent_video || !budget.is_finite() {
                continue;
            }
            valid += 1;
            if let Some(value) = measured(s.loss_percent) {
                loss = loss.max(value);
            }
            if let Some(value) = measured(s.decode_ms) {
                decode = decode.max(value);
            }
            if let Some(value) = measured(s.host_p95_ms).filter(|value| *value >= host) {
                host = value;
                host_includes_send = s.host_includes_send;
            }
            if let Some(value) = measured(s.display_p95_ms) {
                display = display.max(value);
            }
            if network_evidence(s) {
                votes[Finding::Network as usize] += 1;
            }
            if measured(s.decode_ms).is_some_and(|decode| decode >= budget) {
                votes[Finding::Decoder as usize] += 1;
            }
            if measured(s.host_p95_ms).is_some_and(|host| host >= budget * 1.25) {
                votes[Finding::Host as usize] += 1;
            }
            // Compare observed stages, not requested FPS: a static/VRR desktop is not a dropped-frame fault.
            if let (Some(shown), Some(released), Some(display)) = (
                measured(s.shown_fps),
                measured(s.released_fps),
                measured(s.display_p95_ms),
            ) && released >= 10.0
                && shown < released * 0.85
                && display > budget * 2.0
            {
                votes[Finding::Presentation as usize] += 1;
            }
            if s.thermal_level >= 2 {
                votes[Finding::Heat as usize] += 1;
            }
            if s.late_frames.is_some_and(|late| late > 0) {
                votes[Finding::Bursts as usize] += 1;
                run += 1;
            } else {
                burst_votes += run_votes(std::mem::take(&mut run));
            }
        }
        burst_votes += run_votes(run);
        let bursts = BURST_MIN_VOTES.max(valid.div_ceil(3) as u32);
        let mut findings: Vec<Finding> = Finding::ALL
            .into_iter()
            .filter(|&finding| {
                let (have, needed) = if finding == Finding::Bursts {
                    (burst_votes, bursts)
                } else {
                    (votes[finding as usize], 2)
                };
                valid >= 3 && have >= needed
            })
            .collect();
        // A rhythm spans minutes, so it stands on its own rather than on the last 30 seconds.
        if self.stall_period_ms > 0 {
            findings.push(Finding::Periodic);
        }
        let recent = self.samples.back().is_some_and(|last| {
            last.recent_video && now_ms.saturating_sub(last.now_ms) <= RECENT_MS
        });
        Report {
            findings,
            samples: valid,
            recent,
            worst_loss_percent: loss,
            worst_decode_ms: decode,
            worst_host_p95_ms: host,
            worst_display_p95_ms: display,
            stalls: self.stall_starts.len(),
            stall_period_ms: self.stall_period_ms,
            host_includes_send,
            late_samples: votes[Finding::Bursts as usize] as usize,
        }
    }
}

fn network_evidence(s: &Stats) -> bool {
    measured(s.loss_percent).is_some_and(|loss| loss >= 2.0)
        || measured(s.jitter_ms).is_some_and(|jitter| jitter >= budget_ms(s).max(8.0))
}

// As in Java, a counter that is negative or not finite is unavailable.
fn measured(value: Option<f32>) -> Option<f32> {
    value.filter(|v| v.is_finite() && *v >= 0.0)
}

/// Tolerance for one beat: the larger of two seconds and 4% of the period. Scan timers are steady.
pub fn tolerance(period_ms: u64) -> u64 {
    MIN_TOLERANCE_MS.max(period_ms * 4 / 100)
}

/// Finds a regular rhythm in stall event times (ascending, any monotonic clock), such as the "lag
/// spike every 2 minutes" that background Wi-Fi scans cause. Returns the period in ms, or 0 when
/// the stalls show no regular rhythm. Pure arithmetic; run it only when a new stall starts.
pub fn period_ms(times: &[u64]) -> u64 {
    // Collapse runs of events into stall starts.
    let mut starts = Vec::with_capacity(times.len());
    for (i, &time) in times.iter().enumerate() {
        if starts.is_empty() || time.saturating_sub(times[i - 1]) > BURST_GAP_MS {
            starts.push(time);
        }
    }
    let n = starts.len();
    if n < MIN_HITS {
        return 0;
    }
    let (mut best, mut best_hits) = (0, 0);
    // Candidate periods come from gaps between a stall and its next two successors, so one
    // unrelated stall between two beats cannot hide the rhythm.
    for i in 0..n {
        for j in i + 1..n.min(i + 3) {
            // Times out of order give Java a negative gap, which is no candidate.
            let candidate = starts[j].saturating_sub(starts[i]);
            if !(MIN_PERIOD_MS..=MAX_PERIOD_MS).contains(&candidate) {
                continue;
            }
            let (hits, period) = chain(&starts, i, candidate);
            if hits > best_hits || (hits == best_hits && hits > 0 && period < best) {
                best_hits = hits;
                best = period;
            }
        }
    }
    if best_hits >= MIN_HITS {
        best
    } else {
        0
    }
}

/// Follows the beat from `first`, tolerating one missed beat at a time and re-anchoring on each
/// hit so jitter does not accumulate. Returns (hits, refined period), or (0, 0) when the chain is
/// too sparse to be a rhythm.
fn chain(starts: &[u64], first: usize, period: u64) -> (usize, u64) {
    let n = starts.len();
    let tolerance = tolerance(period);
    let (mut hits, mut slots) = (1usize, 1usize);
    let mut last = starts[first];
    let mut last_index = first;
    let mut index = first + 1;
    while index < n {
        let mut hit = None;
        for skip in 1..=2 {
            let expected = last + period * skip;
            let found = (index..n)
                .take_while(|&k| starts[k] <= expected + tolerance)
                .find(|&k| starts[k].abs_diff(expected) <= tolerance);
            if let Some(k) = found {
                hit = Some((k, skip as usize - 1));
                break;
            }
        }
        let Some((k, missed)) = hit else { break };
        hits += 1;
        slots += missed + 1;
        last = starts[k];
        last_index = k;
        index = k + 1;
    }
    if hits < MIN_HITS || hits * 4 < slots * 3 {
        return (0, 0);
    }
    // With many other stalls some land on a beat by luck. Spread the stalls off the beat at random:
    // a chain this long must be unlikely across all candidates tried (about two per stall).
    // Java's signed max(1, last - first): times out of order span 1 ms.
    let span = starts[n - 1].saturating_sub(starts[0]).max(1) as f64;
    let chance = ((n - hits).max(1) as f64 / span * 2.0 * tolerance as f64).min(0.999);
    if (hits - 1) as f64 * chance.ln() + (2.0 * n as f64).ln() > 0.001f64.ln() {
        return (0, 0);
    }
    // At most one unrelated stall per two beats inside the rhythm's span.
    let in_span = last_index - first + 1;
    if hits * 3 < in_span * 2 {
        return (0, 0);
    }
    (hits, last.saturating_sub(starts[first]) / (slots as u64 - 1))
}

/// What the doctor offers next to its report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrialOffer {
    None,
    /// Lower the bitrate by a fifth for this session.
    Try,
    /// Return to the bitrate before the trial.
    Undo,
}

/// Why a trial step leaves the bitrate alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrialRefused {
    /// The host reported no live encoder bitrate. The trial is kept; show the error.
    Unavailable,
    /// The encoder already runs at [`TRIAL_FLOOR_KBPS`] or below.
    Minimum,
    /// Something else changed the bitrate since the trial, which is dropped.
    Replaced,
}

/// The doctor's reversible bitrate trial: 20% lower with a 500 kbps floor, undone on request.
/// Automatic bitrate or quality and manual changes own the bitrate; the trial does not compete.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BitrateTrial {
    original_kbps: u32,
    trial_kbps: u32,
}

impl BitrateTrial {
    /// `can_suggest` is [`Report::can_suggest_bitrate`], `runtime_bitrate` whether the host
    /// changes its bitrate mid-stream, `automatic` whether automatic bitrate or quality runs, and
    /// `busy` whether another host action is in flight.
    pub fn offer(
        &self,
        can_suggest: bool,
        current_kbps: u32,
        runtime_bitrate: bool,
        automatic: bool,
        busy: bool,
    ) -> TrialOffer {
        if self.original_kbps > 0 && current_kbps == self.trial_kbps && !automatic {
            TrialOffer::Undo
        } else if can_suggest
            && runtime_bitrate
            && !automatic
            && current_kbps > TRIAL_FLOOR_KBPS
            && !busy
        {
            TrialOffer::Try
        } else {
            TrialOffer::None
        }
    }

    /// The bitrate to ask the host for. `live_kbps` is the host's live encoder bitrate (0 when
    /// unknown): the saved preference may include FEC and audio or predate host adaptation, so
    /// only the live rate is a baseline.
    pub fn begin(&mut self, undo: bool, live_kbps: u32) -> Result<u32, TrialRefused> {
        if live_kbps == 0 {
            return Err(TrialRefused::Unavailable);
        }
        if undo && live_kbps != self.trial_kbps {
            *self = Self::default();
            return Err(TrialRefused::Replaced);
        }
        if !undo && live_kbps <= TRIAL_FLOOR_KBPS {
            *self = Self::default();
            return Err(TrialRefused::Minimum);
        }
        if undo {
            return Ok(self.original_kbps);
        }
        Ok((u64::from(live_kbps) * 4 / 5).max(u64::from(TRIAL_FLOOR_KBPS)) as u32)
    }

    /// Records the bitrate the host applied for [`begin`](Self::begin) from `previous_kbps`.
    pub fn applied(&mut self, undo: bool, previous_kbps: u32, applied_kbps: u32) {
        *self = if undo {
            Self::default()
        } else {
            Self {
                original_kbps: previous_kbps,
                trial_kbps: applied_kbps,
            }
        };
    }

    /// Drops the trial when automatic bitrate or quality, a manual change or a new connection
    /// takes over.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn original_kbps(&self) -> u32 {
        self.original_kbps
    }

    pub fn trial_kbps(&self) -> u32 {
        self.trial_kbps
    }
}

mod trace;
pub(crate) use trace::{traced_health, traced_stalls, traced_trial};

#[cfg(test)]
mod tests;
