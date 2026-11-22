//! Pure UI state types and JSON report formatter for Settings → Advanced → Latency benchmark,
//! shared between `nightjar-desktop-ui::ui::latency_bench` and the real decoder/timer worker in
//! `nightjar-desktop-media::latency_bench`.

use crate::strings::en;

/// Samples kept per metric, as Android's `WINDOW_SIZE`.
pub const WINDOW_SIZE: usize = 2048;

/// What the timing test measures, in Android's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Metric {
    FrameInterval,
    FrameDelay,
    InputAge,
    TimerDelay,
}

impl Metric {
    pub const ALL: [Self; 4] = [Self::FrameInterval, Self::FrameDelay, Self::InputAge, Self::TimerDelay];

    pub fn label(self) -> &'static str {
        match self {
            Self::FrameInterval => en("Frame callback interval"),
            Self::FrameDelay => en("Frame callback delay"),
            Self::InputAge => en("Input dispatch age"),
            Self::TimerDelay => en("Timer wakeup delay"),
        }
    }

    /// Its name in the results text, as Android's `DeviceResults.METRICS`.
    pub fn json(self) -> &'static str {
        match self {
            Self::FrameInterval => "frameIntervalMs",
            Self::FrameDelay => "frameCallbackDelayMs",
            Self::InputAge => "inputDispatchAgeMs",
            Self::TimerDelay => "timerWakeupDelayMs",
        }
    }
}

/// A metric's samples: how many, and their mean, 95th and 99th percentile in milliseconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Summary {
    pub samples: usize,
    pub mean: f64,
    pub p95: f64,
    pub p99: f64,
}

impl Summary {
    /// Android's `latency_values`: "mean / p95 / p99 (n=N)".
    pub fn text(&self) -> String {
        format!("{:.2} / {:.2} / {:.2} (n={})", self.mean, self.p95, self.p99, self.samples)
    }
}

/// Android's `LatencyBenchmarkStats`: the latest [`WINDOW_SIZE`] samples of each metric.
#[derive(Clone, Debug, Default)]
pub struct Stats {
    samples: [Vec<u64>; 4],
    positions: [usize; 4],
    last_frame_ns: Option<i64>,
}

impl Stats {
    /// A frame at `frame_ns` (its vblank) whose callback ran at `callback_ns`. A repeated or older
    /// frame time, or a callback before its frame, is dropped, so mixed clocks can't corrupt the
    /// cadence.
    pub fn frame(&mut self, frame_ns: i64, callback_ns: i64) {
        if frame_ns < 0 || callback_ns < frame_ns || self.last_frame_ns.is_some_and(|last| frame_ns <= last) {
            return;
        }
        if let Some(last) = self.last_frame_ns {
            self.add(Metric::FrameInterval, frame_ns - last);
        }
        self.last_frame_ns = Some(frame_ns);
        self.add(Metric::FrameDelay, callback_ns - frame_ns);
    }

    pub fn add(&mut self, metric: Metric, duration_ns: i64) {
        let Ok(duration) = u64::try_from(duration_ns) else { return };
        let (samples, position) = (&mut self.samples[metric as usize], &mut self.positions[metric as usize]);
        if samples.len() < WINDOW_SIZE {
            samples.push(duration);
        } else {
            samples[*position] = duration;
        }
        *position = (*position + 1) % WINDOW_SIZE;
    }

    /// `None` until a sample came: no data is not zero latency.
    pub fn summary(&self, metric: Metric) -> Option<Summary> {
        let mut values = self.samples[metric as usize].clone();
        if values.is_empty() {
            return None;
        }
        values.sort_unstable();
        let n = values.len();
        let at = |share: f64| values[((n as f64 * share).ceil() as usize).clamp(1, n) - 1] as f64 / 1e6;
        Some(Summary {
            samples: n,
            mean: values.iter().map(|&v| v as f64).sum::<f64>() / n as f64 / 1e6,
            p95: at(0.95),
            p99: at(0.99),
        })
    }
}

/// A codec of the decoder benchmark, in Android's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    H264,
    Hevc,
    Av1,
}

impl Codec {
    pub const ALL: [Self; 3] = [Self::H264, Self::Hevc, Self::Av1];

    pub fn name(self) -> &'static str {
        match self {
            Self::H264 => "H.264",
            Self::Hevc => "HEVC",
            Self::Av1 => "AV1",
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            Self::H264 => "video/avc",
            Self::Hevc => "video/hevc",
            Self::Av1 => "video/av01",
        }
    }
}

/// A decoder's time per frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Measured {
    pub median_us: u64,
    /// FFmpeg's decoder.
    pub decoder: String,
    /// The backend that decoded: a hardware backend's name, or none for software.
    pub hardware: Option<&'static str>,
}

/// One codec's benchmark: its time, or why no decoder here decodes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecoderResult {
    pub codec: Codec,
    pub outcome: Result<Measured, String>,
}

/// What the page shows about this device before and after a test.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Device {
    /// The display mode's refresh rate, when SDL reports one.
    pub panel_hz: Option<f32>,
    /// The default output device's preferred rate and buffer, in sample frames.
    pub audio: Option<(u32, u32)>,
    /// The GPU the renderer draws on (Windows).
    pub gpu: Option<String>,
}

/// Snapshot of the benchmark state displayed by `ui::latency_bench` and updated from the media worker.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bench {
    pub timing: bool,
    pub running: bool,
    pub measured: bool,
    pub decoders_measured: bool,
    pub shown: [Option<Summary>; 4],
    pub device: Option<Device>,
    pub results: Vec<DecoderResult>,
}

impl Bench {
    pub fn timing(&self) -> bool {
        self.timing
    }

    pub fn running(&self) -> bool {
        self.running
    }

    pub fn start_stub(&mut self) {
        self.timing = true;
        self.running = true;
        self.measured = true;
        self.decoders_measured = false;
        self.shown = [None; 4];
        self.results.clear();
    }

    pub fn finish(&mut self) {
        self.timing = false;
        self.running = false;
    }

    /// What the page shows for `metric`: updated twice a second while the test runs.
    pub fn shown(&self, metric: Metric) -> Option<Summary> {
        self.shown[metric as usize]
    }

    /// `codec`'s result: none before it was measured.
    pub fn result(&self, codec: Codec) -> Option<&DecoderResult> {
        self.results.iter().find(|result| result.codec == codec)
    }

    /// The results as Android's `DeviceResults` text, for the user to paste where they like; the
    /// fields only a phone has are null. No PC name, address or account goes in.
    pub fn json(&self, version: &str) -> String {
        use serde_json::{json, Value};
        let device = self.device.clone().unwrap_or_default();
        let summary = |metric: Metric| match self.shown(metric) {
            Some(s) if self.measured => json!({ "samples": s.samples, "mean": round(s.mean), "p95": round(s.p95), "p99": round(s.p99) }),
            _ => Value::Null,
        };
        let decoders: Vec<Value> = self
            .results
            .iter()
            .filter_map(|result| result.outcome.as_ref().ok().map(|m| (result.codec, m)))
            .map(|(codec, m)| {
                json!({
                    "codec": codec.name(),
                    "mime": codec.mime(),
                    "decoder": m.decoder,
                    "backend": m.hardware.unwrap_or("software"),
                    "medianDecodeMs": round(m.median_us as f64 / 1000.0),
                    "featureLowLatency": Value::Null,
                    "vendorLowLatencyKeys": Value::Null,
                })
            })
            .collect();
        let measured = self.decoders_measured || !decoders.is_empty();
        let mut benchmark = serde_json::Map::new();
        benchmark.insert("measured".into(), measured.into());
        if measured {
            benchmark.insert("clip".into(), "1920x1080@60".into());
        }
        benchmark.insert("results".into(), decoders.into());
        let latency = if self.measured {
            let mut test = serde_json::Map::new();
            test.insert("panelHz".into(), device.panel_hz.map_or(Value::Null, |hz| round(f64::from(hz)).into()));
            test.insert("audioSampleRate".into(), device.audio.map_or(Value::Null, |(rate, _)| rate.into()));
            test.insert("audioFramesPerBuffer".into(), device.audio.map_or(Value::Null, |(_, frames)| frames.into()));
            for metric in Metric::ALL {
                test.insert(metric.json().into(), summary(metric));
            }
            Value::Object(test)
        } else {
            Value::Null
        };
        let results = json!({
            "format": "nightjar-device-results",
            "version": 1,
            "app": { "name": "Nightjar", "version": version, "build": Value::Null },
            "device": {
                "manufacturer": Value::Null, "model": Value::Null, "codename": Value::Null,
                "socManufacturer": Value::Null, "socModel": Value::Null, "hardware": Value::Null, "board": Value::Null,
                "android": Value::Null, "sdk": Value::Null,
                "gpu": device.gpu,
                "os": std::env::consts::OS, "arch": std::env::consts::ARCH,
            },
            "decoderBenchmark": Value::Object(benchmark),
            "latencyTest": latency,
        });
        serde_json::to_string_pretty(&results).unwrap_or_default()
    }

    /// A finished test with fixed values, for the page's screenshots.
    pub fn fixture() -> Self {
        Self {
            timing: false,
            running: false,
            measured: true,
            decoders_measured: true,
            device: Some(Device {
                panel_hz: Some(143.998),
                audio: Some((48_000, 480)),
                gpu: Some("AMD Radeon RX 7900 XT".into()),
            }),
            shown: [
                Some(Summary { samples: 1439, mean: 6.94, p95: 7.12, p99: 7.61 }),
                Some(Summary { samples: 1440, mean: 0.21, p95: 0.48, p99: 1.02 }),
                Some(Summary { samples: 212, mean: 1.37, p95: 3.05, p99: 6.48 }),
                Some(Summary { samples: 99, mean: 0.52, p95: 0.98, p99: 1.21 }),
            ],
            results: vec![
                DecoderResult { codec: Codec::H264, outcome: Ok(Measured { median_us: 1180, decoder: "h264".into(), hardware: Some("D3D11VA") }) },
                DecoderResult { codec: Codec::Hevc, outcome: Ok(Measured { median_us: 1040, decoder: "hevc".into(), hardware: Some("D3D11VA") }) },
                DecoderResult { codec: Codec::Av1, outcome: Err("no hardware decoder decodes it here".into()) },
            ],
        }
    }
}

fn round(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}
