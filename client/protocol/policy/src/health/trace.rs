//! Trace replay adapters for [`StreamHealth`], [`period_ms`], and [`BitrateTrial`].

use serde_json::Value;

use super::{period_ms, BitrateTrial, StreamHealth, TrialOffer, TrialRefused};
use crate::stats::Stats;
use crate::trace::{Args, Subject};

fn float(args: &Value, i: usize) -> Result<Option<f32>, String> {
    // Java converts with floatValue(), which also turns 1e39 into infinity: JSON has no NaN or
    // infinity of its own. Unavailable counters reach StreamHealth as Java passes them (-1).
    args[i]
        .as_f64()
        .map(|value| Some(value as f32))
        .ok_or_else(|| format!("argument {i} is not a number: {}", args[i]))
}

fn flag(args: &Value, i: usize) -> Result<bool, String> {
    args[i]
        .as_bool()
        .ok_or_else(|| format!("argument {i} is not a boolean: {}", args[i]))
}

/// A tick from trace arguments `at` onwards, which leave out its time.
fn traced_sample(a: &Args, args: &Value, at: usize, now_ms: u64) -> Result<Stats, String> {
    Ok(Stats {
        now_ms,
        recent_video: flag(args, at)?,
        fps: a.int(at + 1)?,
        loss_percent: float(args, at + 2)?,
        jitter_ms: float(args, at + 3)?,
        decode_ms: float(args, at + 4)?,
        host_p95_ms: float(args, at + 5)?,
        display_p95_ms: float(args, at + 6)?,
        received_fps: float(args, at + 7)?,
        released_fps: float(args, at + 8)?,
        shown_fps: float(args, at + 9)?,
        thermal_level: a.int(at + 10)?,
        host_includes_send: flag(args, at + 11)?,
        ..Stats::default()
    })
}

pub(crate) fn traced_health(args: &Value) -> Result<Box<dyn Subject>, String> {
    Args::new(args, 0)?;
    Ok(Box::new(StreamHealth::default()))
}

/// Android replays these through `StreamHealthTrace`, which takes the same plain values.
impl Subject for StreamHealth {
    fn call(&mut self, method: &str, args: &Value) -> Result<Value, String> {
        if let Some(field) = method.strip_prefix("report_") {
            let report = self.report(Args::new(args, 1)?.int(0)?);
            return Ok(match field {
                "findings" => {
                    let names: Vec<&str> = report.findings.iter().map(|f| f.name()).collect();
                    names.join(",").into()
                }
                "samples" => report.samples.into(),
                "recent" => report.recent.into(),
                "worst_loss_percent" => f64::from(report.worst_loss_percent).into(),
                "worst_decode_ms" => f64::from(report.worst_decode_ms).into(),
                "worst_host_p95_ms" => f64::from(report.worst_host_p95_ms).into(),
                "worst_display_p95_ms" => f64::from(report.worst_display_p95_ms).into(),
                "stalls" => report.stalls.into(),
                "stall_period_ms" => report.stall_period_ms.into(),
                "host_includes_send" => report.host_includes_send.into(),
                "late_samples" => report.late_samples.into(),
                "can_suggest_bitrate" => report.can_suggest_bitrate().into(),
                _ => return Err(format!("unknown method {method}")),
            });
        }
        match method {
            "add" => {
                let a = Args::new(args, 13)?;
                self.add(&traced_sample(&a, args, 1, a.int(0)?)?);
                Ok(Value::Null)
            }
            // `add` with the tick's late frames last (Java's -1 when unknown).
            "add_late" => {
                let a = Args::new(args, 14)?;
                let late: i64 = a.int(13)?;
                self.add(&Stats {
                    late_frames: u32::try_from(late).ok(),
                    ..traced_sample(&a, args, 1, a.int(0)?)?
                });
                Ok(Value::Null)
            }
            // The same sample `count` times, `step_ms` apart, as steady stats ticks.
            "add_series" => {
                let a = Args::new(args, 15)?;
                let (first_ms, count, step_ms): (u64, u64, u64) = (a.int(0)?, a.int(1)?, a.int(2)?);
                let sample = traced_sample(&a, args, 3, first_ms)?;
                for i in 0..count {
                    self.add(&Stats {
                        now_ms: first_ms + i * step_ms,
                        ..sample
                    });
                }
                Ok(Value::Null)
            }
            "stall_period_ms" => Args::new(args, 0).map(|_| self.stall_period_ms().into()),
            _ => Err(format!("unknown method {method}")),
        }
    }
}

struct TracedStalls;

pub(crate) fn traced_stalls(args: &Value) -> Result<Box<dyn Subject>, String> {
    match args {
        Value::Null => Ok(Box::new(TracedStalls)),
        _ => Err("PeriodicStalls has only static functions (new must be null)".into()),
    }
}

/// Stall times travel as one space-separated string: trace arguments are plain values.
impl Subject for TracedStalls {
    fn call(&mut self, method: &str, args: &Value) -> Result<Value, String> {
        match method {
            "period_ms" => {
                let a = Args::new(args, 2)?;
                let times = a
                    .text(0)?
                    .ok_or("argument 0 is null")?
                    .split_whitespace()
                    .map(|time| time.parse::<u64>().map_err(|e| format!("{time}: {e}")))
                    .collect::<Result<Vec<_>, _>>()?;
                let count: usize = a.int(1)?;
                let counted = times
                    .get(..count)
                    .ok_or_else(|| format!("count {count} exceeds {} times", times.len()))?;
                Ok(period_ms(counted).into())
            }
            _ => Err(format!("unknown method {method}")),
        }
    }
}

pub(crate) fn traced_trial(args: &Value) -> Result<Box<dyn Subject>, String> {
    Args::new(args, 0)?;
    Ok(Box::new(BitrateTrial::default()))
}

impl Subject for BitrateTrial {
    fn call(&mut self, method: &str, args: &Value) -> Result<Value, String> {
        match method {
            "offer" => {
                let a = Args::new(args, 5)?;
                let offer = self.offer(
                    flag(args, 0)?,
                    a.int(1)?,
                    flag(args, 2)?,
                    flag(args, 3)?,
                    flag(args, 4)?,
                );
                Ok(match offer {
                    TrialOffer::None => "none",
                    TrialOffer::Try => "try",
                    TrialOffer::Undo => "undo",
                }
                .into())
            }
            "begin" => {
                let a = Args::new(args, 2)?;
                Ok(match self.begin(flag(args, 0)?, a.int(1)?) {
                    Ok(kbps) => kbps.into(),
                    Err(TrialRefused::Unavailable) => "unavailable".into(),
                    Err(TrialRefused::Minimum) => "minimum".into(),
                    Err(TrialRefused::Replaced) => "replaced".into(),
                })
            }
            "applied" => {
                let a = Args::new(args, 3)?;
                self.applied(flag(args, 0)?, a.int(1)?, a.int(2)?);
                Ok(Value::Null)
            }
            "clear" => {
                Args::new(args, 0)?;
                self.clear();
                Ok(Value::Null)
            }
            "original_kbps" => Args::new(args, 0).map(|_| self.original_kbps().into()),
            "trial_kbps" => Args::new(args, 0).map(|_| self.trial_kbps().into()),
            _ => Err(format!("unknown method {method}")),
        }
    }
}
