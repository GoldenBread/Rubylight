//! Which host stage took the time that new pictures lost.
//!
//! The video thread already stamps every new picture on its way through
//! capture, conversion, encoding, packetizing and sending
//! ([`crate::performance::Stamps`]). This turns those stamps into one report
//! per stream: each picture is compared with the stream's *own* usual path, the
//! median of the previous windows, so a capture or encoder that is always slow
//! is not reported as a stutter. A picture at least half a frame later than
//! usual is late; the time it lost is attributed to the stages that were above
//! their usual share of it.
//!
//! The network and the device come after the last packet handed to the socket,
//! so this report names host stages only. Game intervals are the repeats the
//! pipeline counters already show: a frame repeated an unchanged picture is the
//! host filling a slot the game did not present. The console says in words what
//! the report covers.

use std::{collections::VecDeque, time::Duration};

/// A picture this much later than usual is late even on a stream fast enough
/// that half its frame time is less than a millisecond.
pub const MIN_LATE: Duration = Duration::from_millis(1);
/// Windows of five-second stage samples the baseline is the median of: a
/// minute. A stream that stays slow for longer than that has a new usual, and
/// its slowness is no longer a stutter against itself.
pub const BASELINE_WINDOWS: usize = 12;
/// Fewer pictures than this cannot show a usual path or a stutter.
pub const MIN_PICTURES: usize = 10;
/// The lost time a verdict needs: below one millisecond no stage is named.
pub const MIN_LOST_US: u64 = 1_000;

/// The stages of the host path the report names, in path order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stage {
    /// Windows' present through the stream's claim: the capture's age when the
    /// picture is taken, including the wait for its turn.
    Capture,
    /// The claim through the encoder accepting the converted picture.
    Convert,
    /// The encoder's submission through the encoded frame in the host's hands.
    Encode,
    /// The encoded frame through completed packetization: the host's own queue
    /// and packetizing work, before the paced sender.
    Packetize,
    /// Completed packetization through the last packet handed to the socket:
    /// the sender's queue, pacing and Windows' send calls.
    Send,
}

impl Stage {
    pub const ALL: [Stage; 5] = [
        Stage::Capture,
        Stage::Convert,
        Stage::Encode,
        Stage::Packetize,
        Stage::Send,
    ];

    /// The identifier the console and the log use.
    pub fn name(self) -> &'static str {
        match self {
            Stage::Capture => "capture",
            Stage::Convert => "convert",
            Stage::Encode => "encode",
            Stage::Packetize => "packetize",
            Stage::Send => "send",
        }
    }
}

/// One new picture's time in the report's stages, in microseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Picture {
    capture: u64,
    convert: u64,
    encode: u64,
    packetize: u64,
    send: u64,
}

impl Picture {
    pub fn new(capture: u64, convert: u64, encode: u64, packetize: u64, send: u64) -> Self {
        Self {
            capture,
            convert,
            encode,
            packetize,
            send,
        }
    }

    /// Groups the nine stage samples [`crate::performance::STAGES`] taken for a
    /// picture (the longer slice holds the total and the capture ages, which
    /// are not stages).
    pub fn of(stages: &[u32]) -> Self {
        let at = |index: usize| u64::from(stages.get(index).copied().unwrap_or(0));
        Self {
            capture: at(0) + at(1) + at(2),
            convert: at(3),
            encode: at(4),
            packetize: at(5) + at(6),
            send: at(7) + at(8),
        }
    }

    pub fn get(self, stage: Stage) -> u64 {
        match stage {
            Stage::Capture => self.capture,
            Stage::Convert => self.convert,
            Stage::Encode => self.encode,
            Stage::Packetize => self.packetize,
            Stage::Send => self.send,
        }
    }

    /// Present to the last packet on the host.
    pub fn total(self) -> u64 {
        self.capture + self.convert + self.encode + self.packetize + self.send
    }
}

/// One stream's stutter report so far.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    windows: u32,
    pictures: u64,
    late: u64,
    lost: [u64; Stage::ALL.len()],
}

impl Report {
    /// Windows of stage samples that went into the report.
    pub fn windows(&self) -> u32 {
        self.windows
    }

    /// New pictures measured. Static repeats have no present and are not counted.
    pub fn pictures(&self) -> u64 {
        self.pictures
    }

    /// Pictures at least half a frame (or a millisecond) later than the
    /// stream's usual path.
    pub fn late_pictures(&self) -> u64 {
        self.late
    }

    pub fn lost_us(&self, stage: Stage) -> u64 {
        self.lost[stage as usize]
    }

    pub fn total_lost_us(&self) -> u64 {
        self.lost.iter().sum()
    }

    /// The stage with the most lost time, or `None` when too little was lost or
    /// no single stage stands out. A tie names the earlier stage of the path,
    /// as [`snapshot`](Self::snapshot) lists them.
    pub fn dominant(&self) -> Option<Stage> {
        if self.late == 0 || self.total_lost_us() < MIN_LOST_US {
            return None;
        }
        let mut best: Option<Stage> = None;
        for stage in Stage::ALL {
            if self.lost_us(stage) > best.map_or(0, |best| self.lost_us(best)) {
                best = Some(stage);
            }
        }
        best
    }

    /// The stage's share of the lost time, in whole percent.
    pub fn share_percent(&self, stage: Stage) -> u32 {
        let total = self.total_lost_us();
        if total == 0 {
            0
        } else {
            (u128::from(self.lost_us(stage)) * 100 / u128::from(total)) as u32
        }
    }

    /// The report as the session API and the console read it.
    pub fn snapshot(&self) -> serde_json::Value {
        let mut stages: Vec<(Stage, u64)> = Stage::ALL
            .into_iter()
            .map(|stage| (stage, self.lost_us(stage)))
            .filter(|(_, lost)| *lost > 0)
            .collect();
        stages.sort_by_key(|(_, lost)| std::cmp::Reverse(*lost));
        serde_json::json!({
            "windows": self.windows,
            "pictures": self.pictures,
            "late_pictures": self.late,
            "lost_ms": millis(self.total_lost_us()),
            "dominant": self.dominant().map(Stage::name),
            "stages": stages.into_iter().map(|(stage, lost)| serde_json::json!({
                "id": stage.name(),
                "lost_ms": millis(lost),
                "percent": self.share_percent(stage),
            })).collect::<Vec<_>>(),
            // The other stages of the stream's path. The host cannot see them:
            // the game's own intervals show as repeats in the pipeline rates,
            // and the network, decoding and display happen after its last packet.
            "not_measured": ["game", "network", "decode", "present"],
        })
    }
}

/// Milliseconds with one decimal, for the console and the log.
fn millis(us: u64) -> f64 {
    (us as f64 / 100.0).round() / 10.0
}

/// A stream's running stutter totals.
///
/// Fed one window of pictures at a time, in picture order. Each window's
/// per-stage medians become the baseline for the windows after it.
#[derive(Clone, Debug)]
pub struct Totals {
    windows: u32,
    pictures: u64,
    late: u64,
    lost: [u64; Stage::ALL.len()],
    /// Each of the previous windows' medians per stage, oldest first.
    baselines: [VecDeque<u64>; Stage::ALL.len()],
}

impl Default for Totals {
    fn default() -> Self {
        Self {
            windows: 0,
            pictures: 0,
            late: 0,
            lost: [0; Stage::ALL.len()],
            baselines: std::array::from_fn(|_| VecDeque::new()),
        }
    }
}

impl Totals {
    /// Adds the pictures of the window that just ended; `period` is the
    /// stream's frame interval when they were sent. An empty window adds
    /// nothing, so a caller may offer one on every tick.
    pub fn add(&mut self, pictures: &[Picture], period: Duration) {
        if pictures.is_empty() {
            return;
        }
        let medians = Self::medians(pictures);
        let baseline: [u64; Stage::ALL.len()] =
            if self.baselines.iter().all(|queue| queue.is_empty()) {
                // The first window of a stream, or the first after a new mode, has
                // no earlier one to be judged against; its own usual path is the
                // best there is. The next window compares against it.
                medians
            } else {
                std::array::from_fn(|stage| {
                    let mut values: Vec<u64> = self.baselines[stage].iter().copied().collect();
                    median(&mut values)
                })
            };
        let baseline_total: u64 = baseline.iter().sum();
        let threshold = u64::try_from(period.as_micros())
            .unwrap_or(u64::MAX)
            .saturating_div(2)
            .max(u64::try_from(MIN_LATE.as_micros()).unwrap_or(u64::MAX));
        let minimum = baseline_total.saturating_add(threshold);
        for picture in pictures {
            let total = picture.total();
            if total < minimum {
                continue;
            }
            let overdue = total - baseline_total;
            let excess: [u64; Stage::ALL.len()] = std::array::from_fn(|stage| {
                picture
                    .get(Stage::ALL[stage])
                    .saturating_sub(baseline[stage])
            });
            let excess_total: u64 = excess.iter().sum();
            let mut shares = [0; Stage::ALL.len()];
            if excess_total == 0 {
                // Being later in total than the baseline can only come from a
                // stage above it; the fallback keeps a rounding surprise out.
                shares[largest_index(&excess)] = overdue;
            } else {
                // Each stage's excess over its usual time, in proportion; the
                // largest gets the remainder so the shares add up to `overdue`.
                let largest = largest_index(&excess);
                let mut assigned = 0;
                for (stage, share) in shares.iter_mut().enumerate() {
                    if stage == largest {
                        continue;
                    }
                    *share = (u128::from(overdue) * u128::from(excess[stage])
                        / u128::from(excess_total)) as u64;
                    assigned += *share;
                }
                shares[largest] = overdue - assigned;
            }
            for (lost, share) in self.lost.iter_mut().zip(shares) {
                *lost = lost.saturating_add(share);
            }
            self.late = self.late.saturating_add(1);
        }
        self.pictures = self
            .pictures
            .saturating_add(u64::try_from(pictures.len()).unwrap_or(u64::MAX));
        self.windows = self.windows.saturating_add(1);
        for (stage, queue) in self.baselines.iter_mut().enumerate() {
            if queue.len() == BASELINE_WINDOWS {
                queue.pop_front();
            }
            queue.push_back(medians[stage]);
        }
    }

    /// Forgets the usual path, for a stream that changed size or rate: the
    /// next window is its own baseline, so the new mode's first slower
    /// windows are not a stutter against the old one. The totals stay.
    pub fn forget_usual(&mut self) {
        for queue in &mut self.baselines {
            queue.clear();
        }
    }

    pub fn report(&self) -> Report {
        Report {
            windows: self.windows,
            pictures: self.pictures,
            late: self.late,
            lost: self.lost,
        }
    }

    /// Each stage's median over the window, in microseconds.
    fn medians(pictures: &[Picture]) -> [u64; Stage::ALL.len()] {
        std::array::from_fn(|stage| {
            let mut values: Vec<u64> = pictures
                .iter()
                .map(|picture| picture.get(Stage::ALL[stage]))
                .collect();
            median(&mut values)
        })
    }
}

/// The middle value (the lower one of an even count) of `values`, which are
/// sorted in place. Zero for none.
fn median(values: &mut [u64]) -> u64 {
    if values.is_empty() {
        return 0;
    }
    values.sort_unstable();
    values[(values.len() - 1) / 2]
}

/// The index of the largest value; the first one on a tie.
fn largest_index(values: &[u64]) -> usize {
    let mut largest = 0;
    for (index, value) in values.iter().enumerate() {
        if *value > values[largest] {
            largest = index;
        }
    }
    largest
}

#[cfg(test)]
mod tests;
