//! Per-frame measurements and their summary: percentiles over a run, and the
//! rows of the CSV a run can leave behind.
use std::{fmt::Write as _, time::Duration};

/// One encoded picture.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub index: u64,
    pub keyframe: bool,
    pub bytes: usize,
    /// Time inside the calls that hand the picture to the encoder: what the
    /// submitting thread is blocked for.
    pub submit: Duration,
    /// From the start of the submit to the encoded picture copied out.
    pub total: Duration,
    /// How late the submit started against its schedule (paced runs).
    pub late: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Summary {
    pub count: usize,
    pub mean: f64,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
}

/// Summary of durations in milliseconds. Percentiles take the nearest rank.
pub fn summarize(values: impl Iterator<Item = Duration>) -> Summary {
    let mut ms: Vec<f64> = values.map(|d| d.as_secs_f64() * 1000.0).collect();
    ms.sort_by(f64::total_cmp);
    let count = ms.len();
    if count == 0 {
        return Summary {
            count,
            mean: 0.0,
            p50: 0.0,
            p95: 0.0,
            p99: 0.0,
            max: 0.0,
        };
    }
    let rank = |p: f64| ms[((p * count as f64).ceil() as usize).clamp(1, count) - 1];
    Summary {
        count,
        mean: ms.iter().sum::<f64>() / count as f64,
        p50: rank(0.50),
        p95: rank(0.95),
        p99: rank(0.99),
        max: ms[count - 1],
    }
}

impl Summary {
    pub fn line(&self) -> String {
        format!(
            "mean {:.2}  p50 {:.2}  p95 {:.2}  p99 {:.2}  max {:.2} ms",
            self.mean, self.p50, self.p95, self.p99, self.max
        )
    }
}

pub fn csv(frames: &[Frame]) -> String {
    let mut out = String::from("frame,keyframe,bytes,submit_us,total_us,late_us\n");
    for f in frames {
        let _ = writeln!(
            out,
            "{},{},{},{},{},{}",
            f.index,
            u8::from(f.keyframe),
            f.bytes,
            f.submit.as_micros(),
            f.total.as_micros(),
            f.late.as_micros()
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_take_the_nearest_rank() {
        let s = summarize((1..=100).map(Duration::from_millis));
        assert_eq!(s.count, 100);
        assert_eq!(s.p50, 50.0);
        assert_eq!(s.p95, 95.0);
        assert_eq!(s.p99, 99.0);
        assert_eq!(s.max, 100.0);
        assert_eq!(s.mean, 50.5);
    }

    #[test]
    fn an_empty_run_summarizes_to_zero() {
        assert_eq!(summarize(std::iter::empty()).count, 0);
    }

    #[test]
    fn one_value_is_every_percentile() {
        let s = summarize(std::iter::once(Duration::from_millis(7)));
        assert_eq!((s.p50, s.p95, s.p99, s.max), (7.0, 7.0, 7.0, 7.0));
    }
}
