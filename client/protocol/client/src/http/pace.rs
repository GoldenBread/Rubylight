use super::Error;
use std::collections::BTreeMap;

pub const DURATION_MS: u32 = 2000;
pub const PACKET_SIZE: u32 = 1392;
const STEP: u32 = 50_000;

/// The packet size a PyroWave probe asks for at a PC that offers `offered` bytes: stock 1392
/// without an offer, else the offered size less the 32-byte encryption header the ANNOUNCE
/// excludes (and the probe's own framing adds back). Shared rule with the pre-flight.
pub fn packet_size_for(offered: u16) -> u32 {
    use rubylight_protocol::video_packet::STOCK_MAX_VIDEO_PACKET;
    if offered > STOCK_MAX_VIDEO_PACKET {
        u32::from(offered).saturating_sub(32)
    } else {
        PACKET_SIZE
    }
}

#[derive(Clone, Debug, Default)]
pub struct Measurement {
    pub expected: u32,
    pub sent: u32,
    pub received: u32,
    pub sender_ms: f64,
    pub loss: f64,
    pub worst_window_loss: f64,
    pub frames: u32,
    pub damaged_frames: u32,
    pub excessive_loss_frames: u32,
    pub missing_tail_frames: u32,
    pub worst_frame_loss: f64,
}

impl Measurement {
    pub fn measured(&self) -> bool {
        self.expected > 0
            && self.sent <= self.expected
            && self.sent as f64 >= self.expected as f64 * 0.98
            && self.received > 0
            && self.received <= self.sent
            && (1960.0..=2040.0).contains(&self.sender_ms)
            && self.loss.is_finite()
            && self.worst_window_loss.is_finite()
            && self.frames > 0
            && self.worst_frame_loss.is_finite()
    }

    pub fn tail_loss(&self) -> f64 {
        percent(self.missing_tail_frames, self.frames)
    }

    pub fn qualified(&self) -> bool {
        self.measured()
            && self.loss < 2.0
            && self.worst_window_loss < 5.0
            && self.excessive_loss_frames == 0
            && self.worst_frame_loss <= 10.0
            && self.tail_loss() < 2.0
    }

    fn ranking(&self) -> [f64; 6] {
        let violations = u32::from(self.loss >= 2.0)
            + u32::from(self.worst_window_loss >= 5.0)
            + u32::from(self.excessive_loss_frames != 0 || self.worst_frame_loss > 10.0)
            + u32::from(self.tail_loss() >= 2.0);
        [
            violations as f64,
            self.tail_loss(),
            percent(self.excessive_loss_frames, self.frames),
            self.worst_frame_loss,
            self.loss,
            self.worst_window_loss,
        ]
    }
}

fn percent(missing: u32, total: u32) -> f64 {
    if total == 0 {
        100.0
    } else {
        100.0 * missing as f64 / total as f64
    }
}

pub fn expected_packets(kbps: u32, packet_size: u32) -> u32 {
    (kbps as u64 * DURATION_MS as u64 / (8 * (packet_size as u64 + 134))) as u32
}

pub fn burst_group(pace_kbps: u32, packet_size: u32, ipv6: bool) -> u32 {
    (pace_kbps / 8 / (packet_size + 48 + 8 + if ipv6 { 40 } else { 20 } + 38)).max(1)
}

pub fn summarize(result: &mut Measurement, arrived: &[bool], fps: u32, group: u32) {
    result.frames = 0;
    result.damaged_frames = 0;
    result.excessive_loss_frames = 0;
    result.missing_tail_frames = 0;
    result.worst_frame_loss = 100.0;
    let expected = result.expected as usize;
    let count = fps as usize * DURATION_MS as usize / 1000;
    if expected > 0 && count > 0 {
        result.worst_frame_loss = 0.0;
        for frame in 0..count {
            let begin = expected * frame / count;
            let end = expected * (frame + 1) / count;
            if begin == end {
                continue;
            }
            let group = group.max(1) as usize;
            let tail = begin + (end - begin - 1) / group * group;
            let missing = (begin..end)
                .filter(|&seq| !arrived.get(seq).copied().unwrap_or(false))
                .count() as u32;
            let loss = percent(missing, (end - begin) as u32);
            result.frames += 1;
            result.damaged_frames += u32::from(missing != 0);
            result.excessive_loss_frames += u32::from(loss > 10.0);
            result.missing_tail_frames +=
                u32::from((tail..end).any(|seq| !arrived.get(seq).copied().unwrap_or(false)));
            result.worst_frame_loss = result.worst_frame_loss.max(loss);
        }
    }
    result.received = 0;
    result.worst_window_loss = 0.0;
    for window in 0..20 {
        let begin = expected * window / 20;
        let end = expected * (window + 1) / 20;
        let count = (begin..end)
            .filter(|&seq| arrived.get(seq).copied().unwrap_or(false))
            .count() as u32;
        result.received += count;
        if end > begin {
            result.worst_window_loss = result
                .worst_window_loss
                .max(percent((end - begin) as u32 - count, (end - begin) as u32));
        }
    }
    result.loss = percent(result.expected - result.received, result.expected);
}

pub fn wire_kbps(image_kbps: u32, fps: u32, packet_size: u32, fec: u32, min_parity: u32) -> u32 {
    if fps == 0 || image_kbps == 0 || packet_size <= 16 {
        return 0;
    }
    let parity = if fec == 0 {
        0
    } else {
        (1..=255u64)
            .map(|critical| {
                (
                    critical,
                    (critical * fec as u64).div_ceil(100).max(min_parity as u64),
                )
            })
            .filter(|(critical, parity)| critical + parity <= 255)
            .map(|(_, parity)| parity)
            .max()
            .unwrap_or(0)
    };
    let image_bytes = (image_kbps as u64 * 125).div_ceil(fps as u64);
    let data = (image_bytes + 8).div_ceil(packet_size as u64 - 16);
    let wire = ((data + parity) * (packet_size as u64 + 134) * fps as u64).div_ceil(125) + 3572;
    wire.min(u64::from(u32::MAX)) as u32
}

pub fn probe_kbps(image_kbps: u32, fps: u32, packet_size: u32, fec: u32, min_parity: u32) -> u32 {
    wire_kbps(image_kbps, fps, packet_size, fec, min_parity).clamp(5000, 3_000_000)
}

pub fn floor_kbps(probe_kbps: u32) -> u32 {
    probe_kbps / 4 * 5
}

#[derive(Clone, Debug)]
pub struct Outcome {
    pub pace_kbps: u32,
    pub qualified: bool,
    pub measurement: Measurement,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SweepPhase {
    Check,
    Confirm0,
    Confirm1,
}

#[derive(Clone, Debug)]
enum SearchState {
    CapProbe,
    CapConfirm(u8),
    BisectFloor,
    BisectStep {
        low: u32,
        high: u32,
        attempt: u8,
        next: u32,
    },
    MarginConfirm {
        pace: u32,
        step: u8,
    },
    Sweep {
        pace: u32,
        phase: SweepPhase,
    },
    FallbackFirst(u32),
    FallbackSecond(u32, Measurement),
    Done(Option<Outcome>),
}

/// Synchronous state machine driving a PyroWave pace search without callbacks.
#[derive(Clone, Debug)]
pub struct PaceSearch {
    cap: u32,
    floor: u32,
    measurements: BTreeMap<u32, Measurement>,
    state: SearchState,
}

fn step_down(n: u32) -> u32 {
    n / STEP * STEP
}

impl PaceSearch {
    pub fn new(link_kbps: u32, floor_kbps: u32) -> Self {
        let cap = step_down(link_kbps.min(9_999_999));
        if cap < STEP {
            return Self {
                cap,
                floor: STEP,
                measurements: BTreeMap::new(),
                state: SearchState::Done(None),
            };
        }
        let floor = (floor_kbps.min(cap).div_ceil(STEP) * STEP).clamp(STEP, cap);
        Self {
            cap,
            floor,
            measurements: BTreeMap::new(),
            state: SearchState::CapProbe,
        }
    }

    pub fn next_pace(&self) -> Option<u32> {
        match &self.state {
            SearchState::CapProbe | SearchState::CapConfirm(_) => Some(self.cap),
            SearchState::BisectFloor => Some(self.floor),
            SearchState::BisectStep { next, .. } => Some(*next),
            SearchState::MarginConfirm { pace, .. } => Some(*pace),
            SearchState::Sweep { pace, .. } => Some(*pace),
            SearchState::FallbackFirst(pace) | SearchState::FallbackSecond(pace, _) => Some(*pace),
            SearchState::Done(_) => None,
        }
    }

    pub fn outcome(&self) -> Option<&Outcome> {
        match &self.state {
            SearchState::Done(Some(outcome)) => Some(outcome),
            _ => None,
        }
    }

    pub fn is_done(&self) -> bool {
        matches!(self.state, SearchState::Done(_))
    }

    pub fn record(&mut self, pace: u32, result: Measurement) {
        if self.is_done() {
            return;
        }
        if self.measurements.get(&pace).is_none_or(|old| {
            !result.measured() || (old.measured() && old.ranking() < result.ranking())
        }) {
            self.measurements.insert(pace, result.clone());
        }
        let current = std::mem::replace(&mut self.state, SearchState::Done(None));
        self.state = match current {
            SearchState::CapProbe => {
                if result.qualified() {
                    SearchState::CapConfirm(0)
                } else {
                    self.start_bisect()
                }
            }
            SearchState::CapConfirm(0) => {
                if result.qualified() {
                    SearchState::CapConfirm(1)
                } else {
                    self.start_bisect()
                }
            }
            SearchState::CapConfirm(_) => {
                if result.qualified() {
                    SearchState::Done(Some(Outcome {
                        pace_kbps: self.cap,
                        qualified: true,
                        measurement: result,
                    }))
                } else {
                    self.start_bisect()
                }
            }
            SearchState::BisectFloor => {
                if result.qualified() {
                    self.advance_bisect(self.floor, self.cap, 0)
                } else {
                    self.start_sweep_from(self.cap)
                }
            }
            SearchState::BisectStep {
                low,
                high,
                attempt,
                next,
            } => {
                if result.qualified() {
                    self.advance_bisect(next, high, attempt + 1)
                } else {
                    self.advance_bisect(low, next, attempt + 1)
                }
            }
            SearchState::MarginConfirm { pace, step: 0 } => {
                if result.qualified() {
                    SearchState::MarginConfirm { pace, step: 1 }
                } else {
                    self.advance_margin(pace)
                }
            }
            SearchState::MarginConfirm { pace, step: _ } => {
                if result.qualified() {
                    SearchState::Done(Some(Outcome {
                        pace_kbps: pace,
                        qualified: true,
                        measurement: result,
                    }))
                } else {
                    self.advance_margin(pace)
                }
            }
            SearchState::Sweep {
                pace,
                phase: SweepPhase::Check,
            } => {
                if result.qualified() {
                    SearchState::Sweep {
                        pace,
                        phase: SweepPhase::Confirm0,
                    }
                } else {
                    self.start_sweep_from(pace)
                }
            }
            SearchState::Sweep {
                pace,
                phase: SweepPhase::Confirm0,
            } => {
                if result.qualified() {
                    SearchState::Sweep {
                        pace,
                        phase: SweepPhase::Confirm1,
                    }
                } else {
                    self.start_sweep_from(pace)
                }
            }
            SearchState::Sweep {
                pace,
                phase: SweepPhase::Confirm1,
            } => {
                if result.qualified() {
                    SearchState::Done(Some(Outcome {
                        pace_kbps: pace,
                        qualified: true,
                        measurement: result,
                    }))
                } else {
                    self.start_sweep_from(pace)
                }
            }
            SearchState::FallbackFirst(pace) => SearchState::FallbackSecond(pace, result),
            SearchState::FallbackSecond(pace, first) => {
                if !first.measured() || !result.measured() {
                    SearchState::Done(None)
                } else {
                    SearchState::Done(Some(Outcome {
                        pace_kbps: pace,
                        qualified: first.qualified() && result.qualified(),
                        measurement: if first.ranking() > result.ranking() {
                            first
                        } else {
                            result
                        },
                    }))
                }
            }
            SearchState::Done(outcome) => SearchState::Done(outcome),
        };
    }

    fn start_bisect(&self) -> SearchState {
        if self.floor < self.cap {
            SearchState::BisectFloor
        } else {
            self.start_sweep_from(self.cap)
        }
    }

    fn advance_bisect(&self, low: u32, high: u32, attempt: u8) -> SearchState {
        if attempt < 16 && high - low > STEP {
            let next = (low + STEP).max(step_down((low + high) / 2));
            if next < high {
                return SearchState::BisectStep {
                    low,
                    high,
                    attempt,
                    next,
                };
            }
        }
        let pace = self.floor.max(step_down(low * 95 / 100));
        SearchState::MarginConfirm { pace, step: 0 }
    }

    fn advance_margin(&self, pace: u32) -> SearchState {
        if pace == self.floor {
            self.start_sweep_from(self.cap)
        } else {
            let next = self.floor.max(step_down(pace * 90 / 100));
            SearchState::MarginConfirm {
                pace: next,
                step: 0,
            }
        }
    }

    fn start_sweep_from(&self, exclusive_upper: u32) -> SearchState {
        let mut pace = exclusive_upper;
        while pace > self.floor {
            pace -= STEP;
            match self.measurements.get(&pace) {
                Some(prev) if !prev.qualified() => continue,
                Some(_) => {
                    return SearchState::Sweep {
                        pace,
                        phase: SweepPhase::Confirm0,
                    };
                }
                None => {
                    return SearchState::Sweep {
                        pace,
                        phase: SweepPhase::Check,
                    };
                }
            }
        }
        let best = self
            .measurements
            .iter()
            .filter(|(_, result)| result.measured())
            .min_by(|(pace_a, a), (pace_b, b)| {
                a.ranking()
                    .partial_cmp(&b.ranking())
                    .unwrap()
                    .then_with(|| pace_b.cmp(pace_a))
            })
            .map(|(&pace, _)| pace);
        match best {
            Some(pace) => SearchState::FallbackFirst(pace),
            None => SearchState::Done(None),
        }
    }
}

pub fn search(
    link_kbps: u32,
    floor_kbps: u32,
    mut probe: impl FnMut(u32) -> Result<Measurement, Error>,
    cancelled: impl Fn() -> bool,
) -> Result<Option<Outcome>, Error> {
    let mut state = PaceSearch::new(link_kbps, floor_kbps);
    while let Some(pace) = state.next_pace() {
        if cancelled() {
            return Err(Error::Cancelled);
        }
        let result = probe(pace)?;
        state.record(pace, result);
    }
    if cancelled() {
        return Err(Error::Cancelled);
    }
    Ok(state.outcome().cloned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(tail_loss: bool) -> Measurement {
        let mut arrived = vec![true; 24_000];
        if tail_loss {
            for i in 0..24 {
                arrived[i * 1000 + 99] = false;
            }
        }
        let mut result = Measurement {
            expected: 24_000,
            sent: 24_000,
            sender_ms: 2000.0,
            ..Default::default()
        };
        summarize(&mut result, &arrived, 120, 10);
        result
    }

    #[test]
    fn frame_tails_and_sender_duration_match_android() {
        let mut result = probe(false);
        assert_eq!(result.frames, 240);
        assert!(result.qualified());
        result.sender_ms = 1500.0;
        assert!(!result.measured());
        let result = probe(true);
        assert_eq!(result.missing_tail_frames, 24);
        assert!(result.loss < 2.0);
        assert!(!result.qualified());
        assert_eq!(expected_packets(1_000_000, 1392), 163826);
        assert_eq!(
            burst_group(1_000_000, 1392, false),
            1_000_000 / 8 / (1440 + 8 + 20 + 38)
        );
        // The probe's size follows the PC's offer, less the encryption header.
        assert_eq!(packet_size_for(0), 1392);
        assert_eq!(packet_size_for(1424), 1392);
        assert_eq!(packet_size_for(8192), 8160);
        assert!((1_000_000..1_300_000).contains(&probe_kbps(1_000_000, 120, 1392, 20, 2)));
        assert_eq!(probe_kbps(5_000_000, 60, 1392, 20, 2), 3_000_000);
    }

    #[test]
    fn search_confirms_twice_and_keeps_the_same_margin_and_fallback() {
        let mut tested = Vec::new();
        let outcome = search(
            2_500_000,
            300_000,
            |pace| {
                tested.push(pace);
                Ok(probe(false))
            },
            || false,
        )
        .unwrap()
        .unwrap();
        assert!(outcome.qualified);
        assert_eq!(tested, [2_500_000; 3]);
        let outcome = search(
            2_500_000,
            300_000,
            |pace| Ok(probe(pace > 2_200_000)),
            || false,
        )
        .unwrap()
        .unwrap();
        assert_eq!(outcome.pace_kbps, 2_050_000);
        assert!(outcome.qualified);
        let outcome = search(500_000, 100_000, |_| Ok(probe(true)), || false)
            .unwrap()
            .unwrap();
        assert_eq!(outcome.pace_kbps, 500_000);
        assert!(!outcome.qualified);
        assert!(matches!(
            search(500_000, 100_000, |_| panic!("cancelled probe ran"), || true),
            Err(Error::Cancelled)
        ));
    }
}
