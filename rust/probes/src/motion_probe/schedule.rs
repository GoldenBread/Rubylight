//! Present-cadence schedules and scene-cut timing for `motion_probe`.

use anyhow::{Result, bail};
use std::time::Duration;

/// Intervals between the probe's presents.
#[derive(Clone, Debug, PartialEq)]
pub enum Schedule {
    Fixed(Duration),
    Cycle(Vec<Duration>),
    Hitch {
        period: Duration,
        hitch: Duration,
        every: u64,
    },
}

fn millis(text: &str) -> Result<Duration> {
    let ms: f64 = text.trim().parse()?;
    if !(0.5..=5000.0).contains(&ms) {
        bail!("interval {ms} ms must be 0.5..5000 ms");
    }
    Ok(Duration::from_secs_f64(ms / 1000.0))
}

impl Schedule {
    pub fn parse(text: &str) -> Result<Self> {
        if let Some(list) = text.strip_prefix("jitter:") {
            let cycle = list.split(',').map(millis).collect::<Result<Vec<_>>>()?;
            if cycle.len() < 2 {
                bail!("jitter needs at least two intervals, e.g. jitter:8,14,9,13");
            }
            return Ok(Self::Cycle(cycle));
        }
        if let Some(rest) = text.strip_prefix("hitch:") {
            let mut parts = rest.split(':');
            let hz: f64 = parts.next().unwrap_or("").parse()?;
            if !(1.0..=1000.0).contains(&hz) {
                bail!("hitch rate must be 1..1000 Hz");
            }
            let hitch = millis(parts.next().unwrap_or(""))?;
            let every = match parts.next() {
                Some(n) => n.parse()?,
                None => (hz * 2.0).round() as u64,
            };
            if every < 2 || parts.next().is_some() {
                bail!("usage: hitch:HZ:HITCH_MS[:EVERY_FRAMES>=2]");
            }
            return Ok(Self::Hitch {
                period: Duration::from_secs_f64(1.0 / hz),
                hitch,
                every,
            });
        }
        let hz: u32 = text.parse()?;
        if !(1..=1000).contains(&hz) {
            bail!("animation rate must be 1..1000 Hz");
        }
        Ok(Self::Fixed(Duration::from_secs_f64(1.0 / f64::from(hz))))
    }

    /// The wait before frame `index` (counted from zero).
    pub fn interval(&self, index: u64) -> Duration {
        match self {
            Self::Fixed(period) => *period,
            Self::Cycle(cycle) => cycle[(index % cycle.len() as u64) as usize],
            Self::Hitch {
                period,
                hitch,
                every,
            } => {
                if index % every == every - 1 {
                    *hitch
                } else {
                    *period
                }
            }
        }
    }

    pub fn fixed_hz(&self) -> Option<u32> {
        match self {
            Self::Fixed(period) => Some((1.0 / period.as_secs_f64()).round() as u32),
            _ => None,
        }
    }
}

/// Scene cuts: the whole picture changes every `every`, and `fresh_rows`
/// change every frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Cuts {
    pub every: Duration,
    pub fresh_rows: u32,
}

impl Cuts {
    /// `MS[:FRESH_ROWS]`, the text after `cuts:`.
    pub fn parse(text: &str) -> Result<Self> {
        let mut parts = text.split(':');
        let every = millis(parts.next().unwrap_or(""))?;
        let fresh_rows = parts.next().map(str::parse).transpose()?.unwrap_or(0);
        if parts.next().is_some() {
            bail!("usage: cuts:MS[:FRESH_ROWS]");
        }
        Ok(Self { every, fresh_rows })
    }

    /// The picture's seed `elapsed` after the start, from 1: the same
    /// between cuts and one higher after each.
    pub fn seed(&self, elapsed: Duration) -> u32 {
        (elapsed.as_nanos() / self.every.as_nanos()) as u32 + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn us(d: Duration) -> u128 {
        d.as_micros()
    }

    #[test]
    fn cuts_change_the_seed_once_per_interval() {
        let cuts = Cuts::parse("2000:96").unwrap();
        assert_eq!(cuts.fresh_rows, 96);
        assert_eq!(cuts.seed(Duration::ZERO), 1);
        assert_eq!(cuts.seed(Duration::from_millis(1999)), 1);
        assert_eq!(cuts.seed(Duration::from_millis(2000)), 2);
        assert_eq!(cuts.seed(Duration::from_millis(5000)), 3);
        assert_eq!(Cuts::parse("500").unwrap().fresh_rows, 0);
        assert!(Cuts::parse("").is_err());
        assert!(Cuts::parse("2000:x").is_err());
        assert!(Cuts::parse("2000:1:2").is_err());
    }

    #[test]
    fn a_rate_is_a_fixed_interval() {
        let fixed = Schedule::parse("90").unwrap();
        assert_eq!(fixed.fixed_hz(), Some(90));
        assert_eq!(us(fixed.interval(0)), 11_111);
        assert_eq!(us(fixed.interval(12_345)), 11_111);
        assert!(Schedule::parse("0").is_err());
        assert!(Schedule::parse("1001").is_err());
        assert!(Schedule::parse("fast").is_err());
    }

    #[test]
    fn jitter_cycles_through_its_intervals() {
        let script = Schedule::parse("jitter:8,14,9,13.5").unwrap();
        let seen: Vec<_> = (0..6).map(|i| us(script.interval(i))).collect();
        assert_eq!(seen, [8000, 14000, 9000, 13500, 8000, 14000]);
        assert_eq!(script.fixed_hz(), None);
        assert!(Schedule::parse("jitter:8").is_err());
        assert!(Schedule::parse("jitter:8,x").is_err());
        assert!(Schedule::parse("jitter:8,0.1").is_err());
    }

    #[test]
    fn a_hitch_comes_every_n_frames_and_replaces_one_interval() {
        let script = Schedule::parse("hitch:90:100:4").unwrap();
        let seen: Vec<_> = (0..8).map(|i| us(script.interval(i))).collect();
        assert_eq!(
            seen,
            [
                11_111, 11_111, 11_111, 100_000, 11_111, 11_111, 11_111, 100_000
            ]
        );
        // Two seconds of frames by default.
        let default = Schedule::parse("hitch:90:100").unwrap();
        assert_eq!(us(default.interval(179)), 100_000);
        assert_eq!(us(default.interval(178)), 11_111);
        assert!(Schedule::parse("hitch:90").is_err());
        assert!(Schedule::parse("hitch:90:100:1").is_err());
        assert!(Schedule::parse("hitch:90:100:4:5").is_err());
    }
}
