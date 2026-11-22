//! Pure Connection-test result formatting and verdicts, decoupled from HostClient network I/O.

use nightjar_policy::connection_test::{self as plan, Speed};
use nightjar_policy::http::ConnectionTestStats;

use crate::strings::{en, tr, trf};

/// What Connection test found: the delay, and the speed or why it was not measured.
#[derive(Clone, Debug, PartialEq)]
pub struct Connection {
    pub latency_ms: Option<f64>,
    pub jitter_ms: Option<f64>,
    pub loss_percent: Option<f64>,
    pub speed: Option<Speed>,
}

impl Connection {
    pub fn new(stats: &ConnectionTestStats, speed: Option<Speed>) -> Self {
        Self {
            latency_ms: stats.latency_ms(),
            jitter_ms: stats.jitter_ms(),
            loss_percent: stats.loss_percent(),
            speed,
        }
    }

    /// "Ping 63 ms · Jitter 41 ms · Loss 0.0 %", Android's `connection_test_stats`.
    pub fn delay_line(&self) -> String {
        let ms = |value| {
            plan::milliseconds(value)
                .map_or_else(|| plan::UNAVAILABLE.to_owned(), |n| trf("{} ms", &[&local(n)]))
        };
        let loss = plan::percent(self.loss_percent)
            .map_or_else(|| plan::UNAVAILABLE.to_owned(), |n| trf("{} %", &[&local(n)]));
        trf("Ping {} · Jitter {} · Loss {}", &[&ms(self.latency_ms), &ms(self.jitter_ms), &loss])
    }

    /// The second line: the speed and the PC's link, or why the speed was not measured.
    pub fn speed_line(&self) -> Option<String> {
        Some(match self.speed? {
            Speed::Measured { mbps, link_mbps } => {
                let speed = trf("Speed {} Mbps", &[&local(plan::megabits(mbps))]);
                if link_mbps > 0 {
                    format!("{speed} · {}", trf("PC link {} Mbps", &[&link_mbps]))
                } else {
                    speed
                }
            }
            Speed::NeedsPairing => tr("Speed not measured: pair this PC first").to_owned(),
            Speed::NoAnswer => tr("Speed not measured: the PC didn’t answer").to_owned(),
            Speed::Unsupported => tr("Speed not measured: this PC has no speed test").to_owned(),
            Speed::Failed => tr("Speed not measured: the test failed").to_owned(),
        })
    }

    /// Good for streaming, will stutter, or cannot connect, for a stream of `bitrate_kbps`.
    pub fn verdict(&self, bitrate_kbps: u32) -> Verdict {
        let finite = |v: Option<f64>| v.filter(|v| v.is_finite());
        let loss = finite(self.loss_percent);
        if finite(self.latency_ms).is_none() || loss.is_some_and(|l| l >= 100.0) {
            return Verdict {
                title: en("Can’t connect to this PC"),
                step: Some(en("Check that the PC is on and on this network, then try again.")),
            };
        }
        let stutters = en("Streaming will stutter");
        if loss.is_some_and(|l| l > LOSS_STUTTERS) {
            return Verdict {
                title: stutters,
                step: Some(en("The network loses data. Connect this device or the PC by cable, or move closer to the router.")),
            };
        }
        if finite(self.jitter_ms).is_some_and(|j| j > JITTER_STUTTERS) {
            return Verdict {
                title: stutters,
                step: Some(en("The delay keeps changing. Connect by cable, or pause other downloads on this network.")),
            };
        }
        if let Some(Speed::Measured { mbps, .. }) = self.speed
            && mbps * 1000.0 < f64::from(bitrate_kbps) * SPEED_HEADROOM
        {
            return Verdict {
                title: stutters,
                step: Some(en("The connection is slower than the stream needs. Lower the bitrate in Settings.")),
            };
        }
        Verdict { title: en("Good for streaming"), step: None }
    }
}

/// What Connection test means for streaming, in plain words, with one next step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Verdict {
    pub title: &'static str,
    pub step: Option<&'static str>,
}

const LOSS_STUTTERS: f64 = 1.0;
const JITTER_STUTTERS: f64 = 10.0;
const SPEED_HEADROOM: f64 = 1.5;

fn local(number: String) -> String {
    if crate::strings::current().decimal_comma() {
        number.replace('.', ",")
    } else {
        number
    }
}
