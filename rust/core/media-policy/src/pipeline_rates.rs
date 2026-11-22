//! Pipeline frame rates for the console: how many frames each host stage
//! handled, the same numbers control message 0x553D reports to a client
//! (`rubylight_protocol::pipeline_rates`).
//!
//! Each stage is counted where the host already stamps a frame, with one
//! relaxed add and no lock: the capture slot counts its captures and the
//! pictures among them, the stream counts what it submits, and encoded frames
//! sent are `Stats::frames`. Rates are differences of those running counts,
//! taken by the control loop, the console and the 5 s timing log, off the
//! capture, encode and send paths.
//!
//! The same 5 s windows answer whether the game or desktop delivers what the
//! stream can show ([`SourceRate`]): the session warns, and logs the line, when
//! the source stays below it, because no host stage would show that. What the
//! stream can show is the lower of its rate and the captured display's refresh:
//! a 60 Hz monitor cannot present 120 frames a second, so a source at 60 is not
//! the limit there.
use rubylight_protocol::pipeline_rates::{
    PIPELINE_RATES_REPORT_BYTES, PipelineRatesReport, PipelineRatesRequest, UNKNOWN,
};
use serde::Serialize;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering, Ordering::Relaxed};
use std::time::{Duration, Instant};

pub use rubylight_protocol::pipeline_rates::PIPELINE_RATES_MESSAGE_TYPE;

/// The window the console's rates cover.
pub const CONSOLE_WINDOW: Duration = Duration::from_secs(1);

/// One stream's running counts of the stages it counts itself.
#[derive(Default)]
pub struct Counters {
    source: AtomicU64,
    capture: AtomicU64,
    submit: AtomicU64,
    repeat: AtomicU64,
}

impl Counters {
    /// New pictures and captures since the stream last looked at its capture
    /// slot (see [`Follow`]).
    pub fn captured(&self, pictures: u64, captures: u64) {
        self.source.fetch_add(pictures, Relaxed);
        self.capture.fetch_add(captures, Relaxed);
    }

    /// A frame submitted to the encoder; `repeat` when it has no new content
    /// (a static or recovery repeat).
    pub fn submitted(&self, repeat: bool) {
        self.submit.fetch_add(1, Relaxed);
        if repeat {
            self.repeat.fetch_add(1, Relaxed);
        }
    }

    /// The counts now, with `output` encoded frames sent.
    pub fn counts(&self, output: u64) -> Counts {
        Counts {
            source: self.source.load(Relaxed),
            capture: self.capture.load(Relaxed),
            submit: self.submit.load(Relaxed),
            output,
            repeat: self.repeat.load(Relaxed),
        }
    }
}

/// Running counts at one moment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub source: u64,
    pub capture: u64,
    pub submit: u64,
    pub output: u64,
    pub repeat: u64,
}

/// Follows a capture slot's running counts. A slot belongs to the capture,
/// which can predate a stream or be replaced under it, so the stream adds only
/// what changed since it last looked, and starts over on another slot.
#[derive(Default)]
pub struct Follow {
    slot: usize,
    pictures: u64,
    captures: u64,
}

impl Follow {
    /// The pictures and captures new since the last call for the same `slot`;
    /// none for the first look at a slot.
    pub fn advance(&mut self, slot: usize, pictures: u64, captures: u64) -> (u64, u64) {
        let new = if slot == self.slot {
            (
                pictures.saturating_sub(self.pictures),
                captures.saturating_sub(self.captures),
            )
        } else {
            (0, 0)
        };
        *self = Self {
            slot,
            pictures,
            captures,
        };
        new
    }
}

/// The rates between two readings `elapsed` apart, for a reader that asked
/// for windows of `asked`.
pub fn report(
    earlier: &Counts,
    later: &Counts,
    elapsed: Duration,
    asked: Duration,
) -> PipelineRatesReport {
    let window_ms = u32::try_from(elapsed.as_millis())
        .unwrap_or(u32::MAX)
        .max(1);
    let rate = |earlier: u64, later: u64| millihertz(later.saturating_sub(earlier), window_ms);
    PipelineRatesReport {
        short_window: elapsed < asked,
        window_ms,
        source_mhz: rate(earlier.source, later.source),
        capture_mhz: rate(earlier.capture, later.capture),
        submit_mhz: rate(earlier.submit, later.submit),
        output_mhz: rate(earlier.output, later.output),
        repeat_mhz: rate(earlier.repeat, later.repeat),
    }
}

/// Rates over windows of at least one length, from successive readings.
#[derive(Default)]
pub struct Sampler {
    last: Option<(Instant, Counts)>,
    rates: Option<PipelineRatesReport>,
}

impl Sampler {
    /// The rates since the last reading once `window` has passed since it,
    /// which then becomes the last reading; until then the previous rates, and
    /// nothing before the first window is complete.
    pub fn sample(
        &mut self,
        now: Instant,
        counts: Counts,
        window: Duration,
    ) -> Option<PipelineRatesReport> {
        match self.last {
            None => self.last = Some((now, counts)),
            Some((at, earlier)) if now.saturating_duration_since(at) >= window => {
                self.rates = Some(report(
                    &earlier,
                    &counts,
                    now.saturating_duration_since(at),
                    window,
                ));
                self.last = Some((now, counts));
            }
            Some(_) => {}
        }
        self.rates
    }
}

/// A client's 0x553D request: reports every interval it asked for, sent by
/// the control loop.
#[derive(Default)]
pub struct Reporter {
    /// Whether a report is on or due, read without the lock: the control
    /// loop takes the lock only when this says so. Set with the request that
    /// turns reports on, and cleared by `take` once they are off. While they
    /// are on it stays set: a report comes due with time alone, which only a
    /// take can see.
    queued: AtomicBool,
    inner: Mutex<Asking>,
}

#[derive(Default)]
struct Asking {
    /// The interval and when the next report is due, while reports are on.
    asked: Option<(Duration, Instant)>,
    sampler: Sampler,
}

impl Reporter {
    /// Applies a request, starting a new window. Returns whether reports are
    /// on, or `None` for a malformed request, which changes nothing.
    pub fn on_request(&self, now: Instant, payload: &[u8], counts: Counts) -> Option<bool> {
        let request = PipelineRatesRequest::decode(payload)?;
        let mut inner = self.inner.lock().unwrap();
        inner.sampler = Sampler::default();
        inner.asked = request.enabled.then(|| {
            let interval = Duration::from_millis(u64::from(request.interval_ms));
            inner.sampler.sample(now, counts, interval);
            (interval, now + interval)
        });
        self.queued.store(true, Ordering::Release);
        Some(request.enabled)
    }

    /// The encoded report due at `now`, if one is.
    pub fn take(
        &self,
        now: Instant,
        counts: impl FnOnce() -> Counts,
    ) -> Option<[u8; PIPELINE_RATES_REPORT_BYTES]> {
        let mut inner = self.inner.lock().unwrap();
        let Some((interval, due)) = inner.asked else {
            self.queued.store(false, Ordering::Release);
            return None;
        };
        if now < due {
            return None;
        }
        inner.asked = Some((interval, now + interval));
        inner
            .sampler
            .sample(now, counts(), interval)
            .map(|report| report.encode())
    }

    /// Whether the control loop should look for a report.
    pub fn queued(&self) -> bool {
        self.queued.load(Ordering::Acquire)
    }
}

/// One window's rates in millihertz; `None` (JSON `null`) for a stage the host
/// can't count.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct PipelineRates {
    pub window_ms: u32,
    /// Frames the game or desktop presented.
    pub source_mhz: Option<u32>,
    /// Frames captured.
    pub capture_mhz: Option<u32>,
    /// Frames submitted to the encoder.
    pub submit_mhz: Option<u32>,
    /// Encoded frames sent.
    pub output_mhz: Option<u32>,
    /// Frames resent without new content.
    pub repeat_mhz: Option<u32>,
}

impl From<&PipelineRatesReport> for PipelineRates {
    fn from(report: &PipelineRatesReport) -> Self {
        let known = |rate: u32| (rate != UNKNOWN).then_some(rate);
        Self {
            window_ms: report.window_ms,
            source_mhz: known(report.source_mhz),
            capture_mhz: known(report.capture_mhz),
            submit_mhz: known(report.submit_mhz),
            output_mhz: known(report.output_mhz),
            repeat_mhz: known(report.repeat_mhz),
        }
    }
}

/// `frames` over `window_ms` in millihertz, saturating; [`UNKNOWN`] for an
/// empty window.
pub fn millihertz(frames: u64, window_ms: u32) -> u32 {
    if window_ms == 0 {
        return UNKNOWN;
    }
    let rate = u128::from(frames) * 1_000_000 / u128::from(window_ms);
    u32::try_from(rate).unwrap_or(UNKNOWN - 1).min(UNKNOWN - 1)
}

/// How many complete timing windows below 97% of what the stream can show the
/// source must stay before the host says so. Three 5 s windows (15 s) keep a
/// hitch, a level load or a scene change from being reported as a limit.
pub const LOW_WINDOWS: u32 = 3;
/// How many complete windows at that rate clear the warning: 30 s, so a rate
/// that merely touches the threshold does not make it flicker. A still picture
/// counts toward it, since an idle screen must not keep a warning up.
pub const CLEAR_WINDOWS: u32 = 6;
/// A source below this share of what the stream can show counts as the limit:
/// 116 of 120 fps is below it (the issue #124 game), a healthy 119.x is not.
const LOW_PERCENT: u32 = 97;
/// Below ten new pictures a second there is nothing to judge: the picture is a
/// menu, a loading screen or a still desktop, not a game too slow to keep up,
/// and its advice to raise the game's frame cap would be wrong.
const STATIC_MHZ: u32 = 10_000;
/// A standing warning is re-worded at most this often, when the rate moved, so
/// a game that hovers at its limit does not fill the log.
pub const REWORD: Duration = Duration::from_secs(60);

/// What a complete timing window says about the source rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceRateWarning {
    /// The game or desktop is the limit: its rate, rounded to 5 fps for the
    /// message, and the exact rate it was measured against (the stream's, or
    /// the display's when that is lower), both in millihertz.
    Below { source_mhz: u32, limit_mhz: u32 },
    /// The source keeps up with the stream again.
    Cleared,
}

/// Whether the game or desktop delivers what the stream can show, over the
/// windows of the 5 s `stream timings` line: the lower of the stream's rate and
/// the captured display's refresh, since a 60 Hz monitor streamed at 120 fps
/// presents no more than 60 and its source is not the limit then. A source
/// below that rate makes the client's picture judder while every host stage
/// looks healthy; without this the console and the log say nothing about it.
#[derive(Default)]
pub struct SourceRate {
    limit_mhz: u32,
    low_windows: u32,
    good_windows: u32,
    /// The rate last shown while the warning holds; `None` when it does not.
    warned: Option<u32>,
    /// When the warning was last shown or re-worded.
    told: Option<Instant>,
}

impl SourceRate {
    /// One complete window: the stream's rate, the captured display's refresh
    /// (`None` when the host cannot read it) and the source's, all in
    /// millihertz. Returns a warning to show or clear, at most once a streak.
    pub fn window(
        &mut self,
        now: Instant,
        stream_mhz: u32,
        display_mhz: Option<u32>,
        source_mhz: Option<u32>,
    ) -> Option<SourceRateWarning> {
        // What the stream can show here: the source below either one is the
        // limit. A display rate the host could not read is no limit.
        let limit_mhz = display_mhz
            .filter(|display| *display > 0)
            .map_or(stream_mhz, |display| stream_mhz.min(display));
        if limit_mhz == 0 {
            return None;
        }
        if limit_mhz != self.limit_mhz {
            // The stream changed rate, or its display's: the old target no
            // longer describes the warning, and the new one needs its own
            // streaks. A standing warning is cleared; with none, this first
            // complete window counts against the new target instead of being
            // discarded.
            self.limit_mhz = limit_mhz;
            self.low_windows = 0;
            self.good_windows = 0;
            self.told = None;
            if self.warned.take().is_some() {
                return Some(SourceRateWarning::Cleared);
            }
        }
        let source = source_mhz?;
        if source < STATIC_MHZ {
            // A still picture, a loading screen or the desktop with nothing
            // moving presents no new pictures to judge, so it never warns and
            // breaks any streak toward one. It is also what an idle screen
            // shows after a warning, which must not then stand forever, so it
            // counts toward clearing one.
            self.low_windows = 0;
            if self.warned.is_some() {
                self.good_windows += 1;
                if self.good_windows >= CLEAR_WINDOWS {
                    self.warned = None;
                    self.told = None;
                    self.good_windows = 0;
                    self.low_windows = 0;
                    return Some(SourceRateWarning::Cleared);
                }
            }
            return None;
        }
        let percent = u64::from(source) * 100 / u64::from(limit_mhz);
        if let Some(shown) = self.warned {
            if percent >= u64::from(LOW_PERCENT) {
                self.low_windows = 0;
                self.good_windows += 1;
                if self.good_windows >= CLEAR_WINDOWS {
                    self.warned = None;
                    self.told = None;
                    self.good_windows = 0;
                    self.low_windows = 0;
                    return Some(SourceRateWarning::Cleared);
                }
                return None;
            }
            self.good_windows = 0;
            // Still below: keep the number current, but no more often than
            // `REWORD`, so a steady game does not fill the log.
            let rounded = rounded_fps(source);
            if rounded != shown
                && self
                    .told
                    .is_none_or(|at| now.saturating_duration_since(at) >= REWORD)
            {
                self.warned = Some(rounded);
                self.told = Some(now);
                return Some(SourceRateWarning::Below {
                    source_mhz: rounded,
                    limit_mhz,
                });
            }
            return None;
        }
        if percent < u64::from(LOW_PERCENT) {
            self.low_windows += 1;
            if self.low_windows >= LOW_WINDOWS {
                let rounded = rounded_fps(source);
                self.warned = Some(rounded);
                self.told = Some(now);
                self.low_windows = 0;
                self.good_windows = 0;
                return Some(SourceRateWarning::Below {
                    source_mhz: rounded,
                    limit_mhz,
                });
            }
        } else {
            self.low_windows = 0;
        }
        None
    }
}

/// A rate in millihertz rounded to the nearest 5 fps, and never reported as
/// zero: jitter in one window must not re-word a standing warning.
fn rounded_fps(mhz: u32) -> u32 {
    (mhz.saturating_add(2_500) / 5_000 * 5_000).max(1_000)
}

/// The stream card's wording for [`SourceRateWarning::Below`]. `pushed`: the
/// capture is Rubylight's own virtual display (`idd-push`), where either the
/// virtual display's delivery or the game can limit the rate; `game`: a game is
/// streaming, not the desktop. `source_mhz` and `limit_mhz` are in millihertz: a
/// 144 fps stream says 144 and 59.94 fps rounds to 60.
pub fn below_message(pushed: bool, game: bool, source_mhz: u32, limit_mhz: u32) -> String {
    let source_fps = source_mhz.saturating_add(500) / 1000;
    let limit_fps = limit_mhz.saturating_add(500) / 1000;
    if pushed {
        format!(
            "Rubylight's virtual display is delivering about {source_fps} fps, below the {limit_fps} fps this stream can show. Check the virtual display mode or driver and the game's frame rate, or lower the stream's frame rate to match."
        )
    } else if game {
        format!(
            "The game is rendering about {source_fps} fps, below the {limit_fps} fps this stream can show, so the picture cannot run smoother than that. Raise the game's own frame-rate cap, lower its resolution or graphics settings, or lower the stream's frame rate to match."
        )
    } else {
        format!(
            "The PC's screen is updating about {source_fps} times a second, below the {limit_fps} fps this stream can show."
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_stages_are_null_in_the_console_json() {
        let report = PipelineRatesReport {
            short_window: false,
            window_ms: 1000,
            source_mhz: 59_940,
            capture_mhz: 60_000,
            submit_mhz: UNKNOWN,
            output_mhz: 60_000,
            repeat_mhz: 0,
        };
        assert_eq!(
            serde_json::to_value(PipelineRates::from(&report)).unwrap(),
            serde_json::json!({
                "window_ms": 1000,
                "source_mhz": 59940,
                "capture_mhz": 60000,
                "submit_mhz": null,
                "output_mhz": 60000,
                "repeat_mhz": 0,
            })
        );
    }

    fn counts(source: u64, capture: u64, submit: u64, output: u64, repeat: u64) -> Counts {
        Counts {
            source,
            capture,
            submit,
            output,
            repeat,
        }
    }

    #[test]
    fn counters_count_submits_repeats_and_captures() {
        let counters = Counters::default();
        counters.captured(2, 3);
        counters.submitted(false);
        counters.submitted(true);
        assert_eq!(counters.counts(7), counts(2, 3, 2, 7, 1));
    }

    #[test]
    fn a_slot_counts_only_what_changed_and_a_new_slot_starts_over() {
        let mut follow = Follow::default();
        // Captures before the stream looked are not its own.
        assert_eq!(follow.advance(1, 40, 90), (0, 0));
        assert_eq!(follow.advance(1, 41, 92), (1, 2));
        assert_eq!(follow.advance(1, 41, 92), (0, 0));
        // A recreated capture's slot counts from zero again.
        assert_eq!(follow.advance(2, 5, 5), (0, 0));
        assert_eq!(follow.advance(2, 6, 7), (1, 2));
    }

    #[test]
    fn rates_are_differences_over_the_window() {
        let rates = report(
            &counts(0, 10, 10, 10, 0),
            &counts(120, 250, 240, 240, 120),
            Duration::from_millis(2000),
            Duration::from_secs(1),
        );
        assert_eq!(
            rates,
            PipelineRatesReport {
                short_window: false,
                window_ms: 2000,
                source_mhz: 60_000,
                capture_mhz: 120_000,
                submit_mhz: 115_000,
                output_mhz: 115_000,
                repeat_mhz: 60_000,
            }
        );
        let short = report(
            &Counts::default(),
            &counts(0, 0, 0, 30, 0),
            Duration::from_millis(500),
            Duration::from_secs(1),
        );
        assert!(short.short_window);
        assert_eq!(short.output_mhz, 60_000);
    }

    #[test]
    fn the_sampler_holds_its_rates_until_the_next_window_is_up() {
        let start = Instant::now();
        let second = Duration::from_secs(1);
        let mut sampler = Sampler::default();
        assert_eq!(sampler.sample(start, Counts::default(), second), None);
        assert_eq!(
            sampler.sample(start + second / 2, counts(0, 0, 0, 30, 0), second),
            None
        );
        let first = sampler
            .sample(start + second, counts(0, 0, 0, 60, 0), second)
            .unwrap();
        assert_eq!(first.output_mhz, 60_000);
        // A second reader soon after sees the same rates, not a short window.
        assert_eq!(
            sampler.sample(start + second + second / 10, counts(0, 0, 0, 66, 0), second),
            Some(first)
        );
        let next = sampler
            .sample(start + second * 2, counts(0, 0, 0, 90, 0), second)
            .unwrap();
        assert_eq!(next.output_mhz, 30_000);
    }

    #[test]
    fn reports_follow_the_requested_interval_and_stop() {
        let start = Instant::now();
        let reporter = Reporter::default();
        assert_eq!(reporter.take(start, Counts::default), None);
        let on = PipelineRatesRequest {
            enabled: true,
            interval_ms: 500,
        };
        assert_eq!(
            reporter.on_request(start, &on.encode(), counts(0, 0, 0, 100, 0)),
            Some(true)
        );
        let half = Duration::from_millis(500);
        assert_eq!(
            reporter.take(start + half / 2, || unreachable!("not due")),
            None
        );
        let encoded = reporter
            .take(start + half, || counts(60, 60, 60, 160, 0))
            .unwrap();
        let report = PipelineRatesReport::decode(&encoded).unwrap();
        assert_eq!((report.window_ms, report.output_mhz), (500, 120_000));
        assert!(!report.short_window);
        assert_eq!(
            reporter.take(start + half + half / 2, || unreachable!("not due")),
            None
        );
        // A malformed request changes nothing.
        assert_eq!(
            reporter.on_request(start, &[1, 1, 0, 0], Counts::default()),
            None
        );
        assert!(
            reporter
                .take(start + half * 2, || counts(60, 60, 60, 220, 0))
                .is_some()
        );
        let off = PipelineRatesRequest {
            enabled: false,
            interval_ms: 500,
        };
        assert_eq!(
            reporter.on_request(start, &off.encode(), Counts::default()),
            Some(false)
        );
        assert_eq!(reporter.take(start + half * 10, Counts::default), None);
    }

    #[test]
    fn the_gate_is_set_while_reports_are_on_and_clears_when_they_stop() {
        let start = Instant::now();
        let reporter = Reporter::default();
        assert!(!reporter.queued());
        assert_eq!(reporter.take(start, Counts::default), None);
        assert!(!reporter.queued(), "a quiet pass reads the gate clear");
        let on = PipelineRatesRequest {
            enabled: true,
            interval_ms: 500,
        };
        reporter.on_request(start, &on.encode(), counts(0, 0, 0, 100, 0));
        assert!(reporter.queued(), "reports are on");
        assert_eq!(
            reporter.take(start + Duration::from_millis(100), || {
                unreachable!("not due")
            }),
            None
        );
        assert!(
            reporter.queued(),
            "the next report comes due with time alone"
        );
        assert!(
            reporter
                .take(start + Duration::from_millis(500), || {
                    counts(60, 60, 60, 160, 0)
                })
                .is_some()
        );
        assert!(reporter.queued(), "the report after keeps the gate set");
        let off = PipelineRatesRequest {
            enabled: false,
            interval_ms: 500,
        };
        reporter.on_request(start, &off.encode(), Counts::default());
        assert_eq!(
            reporter.take(start + Duration::from_secs(10), Counts::default),
            None
        );
        assert!(!reporter.queued(), "reports off: the gate clears");
    }

    #[test]
    fn millihertz_counts_frames_per_window() {
        assert_eq!(millihertz(60, 1000), 60_000);
        assert_eq!(millihertz(599, 10_000), 59_900);
        assert_eq!(millihertz(0, 1000), 0);
        assert_eq!(millihertz(1, 0), UNKNOWN);
        // A count that would overflow stays below the unknown marker.
        assert_eq!(millihertz(u64::MAX, 1), UNKNOWN - 1);
    }

    const TARGET: u32 = 120_000;
    /// A display whose refresh the host cannot read: the stream's rate is the
    /// only limit.
    const NO_DISPLAY: Option<u32> = None;

    fn below(source_fps: u32, limit_fps: u32) -> Option<SourceRateWarning> {
        Some(SourceRateWarning::Below {
            source_mhz: source_fps * 1000,
            limit_mhz: limit_fps * 1000,
        })
    }

    fn at(
        watch: &mut SourceRate,
        start: Instant,
        n: u64,
        target: u32,
        source: u32,
    ) -> Option<SourceRateWarning> {
        at_display(watch, start, n, target, NO_DISPLAY, source)
    }

    /// A window of a stream at `stream` on a display refreshing at `display`
    /// (`None` when unreadable).
    fn at_display(
        watch: &mut SourceRate,
        start: Instant,
        n: u64,
        stream: u32,
        display: Option<u32>,
        source: u32,
    ) -> Option<SourceRateWarning> {
        watch.window(
            start + Duration::from_secs(5 * n),
            stream,
            display,
            Some(source),
        )
    }

    #[test]
    fn a_brief_dip_below_the_stream_rate_is_not_a_warning() {
        let mut watch = SourceRate::default();
        let t = Instant::now();
        assert_eq!(at(&mut watch, t, 0, TARGET, 90_000), None);
        assert_eq!(at(&mut watch, t, 1, TARGET, 88_000), None);
        // One healthy window starts the streak over.
        assert_eq!(at(&mut watch, t, 2, TARGET, 120_000), None);
        assert_eq!(at(&mut watch, t, 3, TARGET, 90_000), None);
        assert_eq!(at(&mut watch, t, 4, TARGET, 90_000), None);
        assert_eq!(at(&mut watch, t, 5, TARGET, 90_000), below(90, 120));
        // A source at exactly 97% of the stream is not below it.
        let mut watch = SourceRate::default();
        for n in 0..10 {
            assert_eq!(at(&mut watch, t, n, TARGET, 116_400), None);
        }
    }

    #[test]
    fn issue_124s_116_of_120_fps_warns_but_a_healthy_capped_119_does_not() {
        // The game in issue #124 rendered 116 of a 120 fps stream (96.7%): the
        // warning must fire after three windows, not stay silent as it did at
        // a 95% threshold.
        let mut slow = SourceRate::default();
        let t = Instant::now();
        assert_eq!(at(&mut slow, t, 0, TARGET, 116_000), None);
        assert_eq!(at(&mut slow, t, 1, TARGET, 116_000), None);
        assert_eq!(at(&mut slow, t, 2, TARGET, 116_000), below(115, 120));
        // A game capped at 119.x of 120 fps (99.5%) is healthy: no warning,
        // however long it runs.
        let mut capped = SourceRate::default();
        for n in 0..20 {
            assert_eq!(at(&mut capped, t, n, TARGET, 119_400), None);
        }
        // 116.5 of 120 (97.1%, the bundle's median window) is above the
        // threshold on its own; the sustained dips are what warn there.
        let mut median = SourceRate::default();
        for n in 0..20 {
            assert_eq!(at(&mut median, t, n, TARGET, 116_500), None);
        }
    }

    #[test]
    fn a_slow_display_is_the_limit_and_its_source_does_not_warn() {
        let t = Instant::now();
        // A 60 Hz monitor streamed at 120 fps presents 60 pictures a second at
        // best: a desktop or game at 60 is not the limit, however long it runs.
        let mut watch = SourceRate::default();
        for n in 0..20 {
            assert_eq!(
                at_display(&mut watch, t, n, TARGET, Some(60_000), 59_940),
                None,
                "{n}"
            );
        }
        // Neither is one at the same rate on a display Windows reports below
        // the stream's, such as a 99 Hz panel.
        for n in 0..20 {
            assert_eq!(
                at_display(&mut watch, t, n, TARGET, Some(99_000), 99_000),
                None,
                "{n}"
            );
        }
        // The same 60 fps source is the limit of a 120 Hz display's stream.
        let mut watch = SourceRate::default();
        assert_eq!(
            at_display(&mut watch, t, 0, TARGET, Some(120_000), 60_000),
            None
        );
        assert_eq!(
            at_display(&mut watch, t, 1, TARGET, Some(120_000), 60_000),
            None
        );
        assert_eq!(
            at_display(&mut watch, t, 2, TARGET, Some(120_000), 60_000),
            below(60, 120)
        );
        // The 1000 Hz display of a VRR stream leaves the stream's own rate as
        // the limit: the issue #124 game warns there too.
        let mut watch = SourceRate::default();
        assert_eq!(
            at_display(&mut watch, t, 0, TARGET, Some(1_000_000), 116_000),
            None
        );
        assert_eq!(
            at_display(&mut watch, t, 1, TARGET, Some(1_000_000), 116_000),
            None
        );
        assert_eq!(
            at_display(&mut watch, t, 2, TARGET, Some(1_000_000), 116_000),
            below(115, 120)
        );
    }

    #[test]
    fn a_sustained_slow_source_warns_after_three_windows_and_rewords_slowly() {
        let mut watch = SourceRate::default();
        let t = Instant::now();
        assert_eq!(at(&mut watch, t, 0, TARGET, 90_000), None);
        assert_eq!(at(&mut watch, t, 1, TARGET, 90_000), None);
        assert_eq!(at(&mut watch, t, 2, TARGET, 90_000), below(90, 120));
        // The number follows the rate, but not before `REWORD`.
        assert_eq!(at(&mut watch, t, 3, TARGET, 60_000), None);
        assert_eq!(at(&mut watch, t, 4, TARGET, 62_000), None);
        assert_eq!(at(&mut watch, t, 13, TARGET, 60_000), None);
        assert_eq!(at(&mut watch, t, 14, TARGET, 60_000), below(60, 120));
        // Once shown, a rate in the same 5 fps step says nothing new.
        assert_eq!(at(&mut watch, t, 15, TARGET, 59_000), None);
        // A re-word waits for `REWORD` from the last one.
        assert_eq!(at(&mut watch, t, 16, TARGET, 31_000), None);
        assert_eq!(at(&mut watch, t, 26, TARGET, 31_000), below(30, 120));
    }

    #[test]
    fn the_warning_clears_only_after_thirty_seconds_at_the_rate() {
        let mut watch = SourceRate::default();
        let t = Instant::now();
        assert_eq!(at(&mut watch, t, 0, TARGET, 60_000), None);
        assert_eq!(at(&mut watch, t, 1, TARGET, 60_000), None);
        assert_eq!(at(&mut watch, t, 2, TARGET, 60_000), below(60, 120));
        // Five healthy windows do not clear it.
        for n in 3..8 {
            assert_eq!(at(&mut watch, t, n, TARGET, 120_000), None, "{n}");
        }
        // The sixth does.
        assert_eq!(
            at(&mut watch, t, 8, TARGET, 119_000),
            Some(SourceRateWarning::Cleared)
        );
        // A dip after the clear starts a new streak, not a warning.
        assert_eq!(at(&mut watch, t, 9, TARGET, 60_000), None);
        assert_eq!(at(&mut watch, t, 10, TARGET, 60_000), None);
        assert_eq!(at(&mut watch, t, 11, TARGET, 60_000), below(60, 120));
        // The new warning takes the full six healthy windows again.
        for n in 12..17 {
            assert_eq!(at(&mut watch, t, n, TARGET, 120_000), None, "{n}");
        }
        assert_eq!(
            at(&mut watch, t, 17, TARGET, 120_000),
            Some(SourceRateWarning::Cleared)
        );
    }

    #[test]
    fn a_still_picture_never_warns_and_clears_a_standing_one() {
        let mut watch = SourceRate::default();
        let t = Instant::now();
        // A paused picture: no new frames are expected, however long it lasts.
        for n in 0..20 {
            assert_eq!(at(&mut watch, t, n, TARGET, 0), None);
        }
        assert_eq!(at(&mut watch, t, 20, TARGET, 60_000), None);
        assert_eq!(at(&mut watch, t, 21, TARGET, 60_000), None);
        assert_eq!(at(&mut watch, t, 22, TARGET, 60_000), below(60, 120));
        // A still screen after it is what the user sees once the game stops
        // updating: it counts toward clearing the warning, over the same six
        // windows, so the console does not keep a warning on an idle screen.
        for n in 23..28 {
            assert_eq!(at(&mut watch, t, n, TARGET, 0), None, "{n}");
        }
        assert_eq!(
            at(&mut watch, t, 28, TARGET, 0),
            Some(SourceRateWarning::Cleared)
        );
        // A slow picture after the clear starts a new streak, not a warning.
        assert_eq!(at(&mut watch, t, 29, TARGET, 60_000), None);
    }

    /// Five new pictures a second is a menu, a spinner or a loading screen, not a game whose
    /// frame cap the user should raise: below the static floor it never warns.
    #[test]
    fn a_five_fps_menu_is_not_a_slow_game() {
        let mut watch = SourceRate::default();
        let t = Instant::now();
        for n in 0..20 {
            assert_eq!(at(&mut watch, t, n, TARGET, 5_000), None, "{n}");
        }
        // Ten fps is moving content: three complete windows still warn.
        assert_eq!(at(&mut watch, t, 20, TARGET, 10_000), None);
        assert_eq!(at(&mut watch, t, 21, TARGET, 10_000), None);
        assert_eq!(at(&mut watch, t, 22, TARGET, 10_000), below(10, 120));
    }

    /// The limit the warning carries is the stream's exact rate: a 144 fps stream is 144, not
    /// the 145 that rounding its 1000 Hz display's refresh produced.
    #[test]
    fn the_limit_is_the_streams_exact_rate() {
        let mut watch = SourceRate::default();
        let t = Instant::now();
        assert_eq!(at(&mut watch, t, 0, 144_000, 125_000), None);
        assert_eq!(at(&mut watch, t, 1, 144_000, 125_000), None);
        assert_eq!(at(&mut watch, t, 2, 144_000, 125_000), below(125, 144));
        let mut vrr = SourceRate::default();
        assert_eq!(
            at_display(&mut vrr, t, 0, 144_000, Some(1_000_000), 70_000),
            None
        );
        assert_eq!(
            at_display(&mut vrr, t, 1, 144_000, Some(1_000_000), 70_000),
            None
        );
        assert_eq!(
            at_display(&mut vrr, t, 2, 144_000, Some(1_000_000), 70_000),
            below(70, 144)
        );
    }

    /// `idd-push` points to the virtual display capture rather than blaming the game alone, and
    /// exact stream rates like 144 and 59.94 show as 144 and 60 fps.
    #[test]
    fn wording_matches_the_capture_backend_and_exact_stream_rate() {
        let pushed = below_message(true, true, 70_000, 144_000);
        assert!(
            pushed.contains("70 fps") && pushed.contains("144 fps"),
            "{pushed}"
        );
        assert!(
            pushed.contains("virtual display") && pushed.contains("game's frame rate"),
            "{pushed}"
        );
        assert!(!pushed.contains("The game is rendering"), "{pushed}");
        assert!(!pushed.contains("Raise the game's own"), "{pushed}");

        let game = below_message(false, true, 50_000, 59_940);
        assert!(game.contains("50 fps") && game.contains("60 fps"), "{game}");
        assert!(game.contains("Raise the game's own"), "{game}");

        let desktop = below_message(false, false, 30_000, 144_000);
        assert!(
            desktop.contains("30 times a second") && desktop.contains("144 fps"),
            "{desktop}"
        );
    }

    #[test]
    fn unjudgeable_windows_change_nothing_and_a_new_stream_rate_starts_over() {
        let mut watch = SourceRate::default();
        let t = Instant::now();
        for n in 0..10 {
            assert_eq!(
                watch.window(t + Duration::from_secs(5 * n), TARGET, NO_DISPLAY, None),
                None
            );
        }
        assert_eq!(watch.window(t, 0, NO_DISPLAY, Some(0)), None);
        assert_eq!(at(&mut watch, t, 0, TARGET, 60_000), None);
        assert_eq!(at(&mut watch, t, 1, TARGET, 60_000), None);
        assert_eq!(at(&mut watch, t, 2, TARGET, 60_000), below(60, 120));
        // The stream drops to 60: the old target no longer describes it.
        assert_eq!(
            at(&mut watch, t, 3, 60_000, 50_000),
            Some(SourceRateWarning::Cleared)
        );
        assert_eq!(at(&mut watch, t, 4, 60_000, 50_000), None);
        assert_eq!(at(&mut watch, t, 5, 60_000, 50_000), None);
        assert_eq!(at(&mut watch, t, 6, 60_000, 50_000), below(50, 60));
    }

    #[test]
    fn a_display_that_changes_its_refresh_starts_the_streak_over() {
        let t = Instant::now();
        let mut watch = SourceRate::default();
        assert_eq!(
            at_display(&mut watch, t, 0, TARGET, Some(120_000), 100_000),
            None
        );
        assert_eq!(
            at_display(&mut watch, t, 1, TARGET, Some(120_000), 100_000),
            None
        );
        // The display drops to 100 Hz: 100 fps is its rate now, not a limit,
        // and the two windows of the old one count for nothing.
        assert_eq!(
            at_display(&mut watch, t, 2, TARGET, Some(100_000), 100_000),
            None
        );
        for n in 3..12 {
            assert_eq!(
                at_display(&mut watch, t, n, TARGET, Some(100_000), 100_000),
                None,
                "{n}"
            );
        }
        // A standing warning is cleared when the limit moves, the same as a
        // stream rate change.
        let mut watch = SourceRate::default();
        assert_eq!(
            at_display(&mut watch, t, 0, TARGET, Some(120_000), 60_000),
            None
        );
        assert_eq!(
            at_display(&mut watch, t, 1, TARGET, Some(120_000), 60_000),
            None
        );
        assert_eq!(
            at_display(&mut watch, t, 2, TARGET, Some(120_000), 60_000),
            below(60, 120)
        );
        assert_eq!(
            at_display(&mut watch, t, 3, TARGET, Some(60_000), 60_000),
            Some(SourceRateWarning::Cleared)
        );
    }
}
