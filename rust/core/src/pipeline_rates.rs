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
//! The same 5 s windows answer whether the game or desktop delivers the
//! stream's rate ([`SourceRate`]): the session warns, and logs the line, when
//! the source stays below it, because no host stage would show that.
use rubylight_protocol::pipeline_rates::{
    PIPELINE_RATES_REPORT_BYTES, PipelineRatesReport, PipelineRatesRequest, UNKNOWN,
};
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
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
    /// The interval and when the next report is due, while reports are on.
    asked: Option<(Duration, Instant)>,
    sampler: Sampler,
}

impl Reporter {
    /// Applies a request, starting a new window. Returns whether reports are
    /// on, or `None` for a malformed request, which changes nothing.
    pub fn on_request(&mut self, now: Instant, payload: &[u8], counts: Counts) -> Option<bool> {
        let request = PipelineRatesRequest::decode(payload)?;
        self.sampler = Sampler::default();
        self.asked = request.enabled.then(|| {
            let interval = Duration::from_millis(u64::from(request.interval_ms));
            self.sampler.sample(now, counts, interval);
            (interval, now + interval)
        });
        Some(request.enabled)
    }

    /// The encoded report due at `now`, if one is.
    pub fn take(
        &mut self,
        now: Instant,
        counts: impl FnOnce() -> Counts,
    ) -> Option<[u8; PIPELINE_RATES_REPORT_BYTES]> {
        let (interval, due) = self.asked?;
        if now < due {
            return None;
        }
        self.asked = Some((interval, now + interval));
        self.sampler
            .sample(now, counts(), interval)
            .map(|report| report.encode())
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

/// How many complete timing windows below 95% of the stream rate the source
/// must stay before the host says so. Three 5 s windows keep a hitch, a level
/// load or a scene change from being reported as a limit.
pub const LOW_WINDOWS: u32 = 3;
/// How many complete windows at the stream rate clear the warning: 30 s, so a
/// rate that merely touches the threshold does not make it flicker.
pub const CLEAR_WINDOWS: u32 = 6;
/// A source below this share of the stream rate counts as the limit.
const LOW_PERCENT: u32 = 95;
/// Below two new pictures a second there is nothing to judge: the picture is
/// static and the host repeats what it has, which is not a slow game.
const STATIC_MHZ: u32 = 2_000;
/// A standing warning is re-worded at most this often, when the rate moved.
pub const REWORD: Duration = Duration::from_secs(30);

/// What a complete timing window says about the source rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceRateWarning {
    /// The game or desktop is the limit: its rate rounded to 5 fps, in
    /// millihertz, to show.
    Below { source_mhz: u32 },
    /// The source keeps up with the stream again.
    Cleared,
}

/// Whether the game or desktop delivers the stream's frame rate, over the
/// windows of the 5 s `stream timings` line. A source that is the limit makes
/// the client's picture judder while every host stage looks healthy; without
/// this the console and the log say nothing about it.
#[derive(Default)]
pub struct SourceRate {
    target_mhz: u32,
    low_windows: u32,
    good_windows: u32,
    /// The rate last shown while the warning holds; `None` when it does not.
    warned: Option<u32>,
    /// When the warning was last shown or re-worded.
    told: Option<Instant>,
}

impl SourceRate {
    /// One complete window: the stream's rate and the source's, both in
    /// millihertz (`None` when the host cannot count the source stage).
    /// Returns a warning to show or clear, at most once a streak.
    pub fn window(
        &mut self,
        now: Instant,
        target_mhz: u32,
        source_mhz: Option<u32>,
    ) -> Option<SourceRateWarning> {
        if target_mhz == 0 {
            return None;
        }
        if target_mhz != self.target_mhz {
            // The stream changed rate: its old target no longer describes the
            // warning, and the new one needs its own streaks.
            self.target_mhz = target_mhz;
            self.low_windows = 0;
            self.good_windows = 0;
            self.told = None;
            return self.warned.take().map(|_| SourceRateWarning::Cleared);
        }
        let source = source_mhz?;
        if source < STATIC_MHZ {
            // A still picture, a loading screen or the desktop with nothing
            // moving presents no pictures to judge.
            return None;
        }
        let percent = u64::from(source) * 100 / u64::from(target_mhz);
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
        let mut reporter = Reporter::default();
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
    fn millihertz_counts_frames_per_window() {
        assert_eq!(millihertz(60, 1000), 60_000);
        assert_eq!(millihertz(599, 10_000), 59_900);
        assert_eq!(millihertz(0, 1000), 0);
        assert_eq!(millihertz(1, 0), UNKNOWN);
        // A count that would overflow stays below the unknown marker.
        assert_eq!(millihertz(u64::MAX, 1), UNKNOWN - 1);
    }

    const TARGET: u32 = 120_000;

    fn below(fps: u32) -> Option<SourceRateWarning> {
        Some(SourceRateWarning::Below {
            source_mhz: fps * 1000,
        })
    }

    fn at(
        watch: &mut SourceRate,
        start: Instant,
        n: u64,
        target: u32,
        source: u32,
    ) -> Option<SourceRateWarning> {
        watch.window(start + Duration::from_secs(5 * n), target, Some(source))
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
        assert_eq!(at(&mut watch, t, 5, TARGET, 90_000), below(90));
        // A source at exactly 95% of the stream is not below it.
        let mut watch = SourceRate::default();
        for n in 0..10 {
            assert_eq!(at(&mut watch, t, n, TARGET, 114_000), None);
        }
    }

    #[test]
    fn a_sustained_slow_source_warns_after_three_windows_and_rewords_slowly() {
        let mut watch = SourceRate::default();
        let t = Instant::now();
        assert_eq!(at(&mut watch, t, 0, TARGET, 90_000), None);
        assert_eq!(at(&mut watch, t, 1, TARGET, 90_000), None);
        assert_eq!(at(&mut watch, t, 2, TARGET, 90_000), below(90));
        // The number follows the rate, but not before `REWORD`.
        assert_eq!(at(&mut watch, t, 3, TARGET, 60_000), None);
        assert_eq!(at(&mut watch, t, 4, TARGET, 62_000), None);
        assert_eq!(at(&mut watch, t, 8, TARGET, 60_000), below(60));
        // Once shown, a rate in the same 5 fps step says nothing new.
        assert_eq!(at(&mut watch, t, 15, TARGET, 59_000), None);
        assert_eq!(at(&mut watch, t, 16, TARGET, 31_000), below(30));
    }

    #[test]
    fn the_warning_clears_only_after_thirty_seconds_at_the_rate() {
        let mut watch = SourceRate::default();
        let t = Instant::now();
        assert_eq!(at(&mut watch, t, 0, TARGET, 60_000), None);
        assert_eq!(at(&mut watch, t, 1, TARGET, 60_000), None);
        assert_eq!(at(&mut watch, t, 2, TARGET, 60_000), below(60));
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
        assert_eq!(at(&mut watch, t, 11, TARGET, 60_000), below(60));
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
    fn a_static_scene_neither_warns_nor_clears_a_standing_warning() {
        let mut watch = SourceRate::default();
        let t = Instant::now();
        // A paused picture: no new frames are expected, however long it lasts.
        for n in 0..20 {
            assert_eq!(at(&mut watch, t, n, TARGET, 0), None);
        }
        assert_eq!(at(&mut watch, t, 20, TARGET, 60_000), None);
        assert_eq!(at(&mut watch, t, 21, TARGET, 60_000), None);
        assert_eq!(at(&mut watch, t, 22, TARGET, 60_000), below(60));
        // The warning holds through a still scene instead of clearing.
        for n in 23..40 {
            assert_eq!(at(&mut watch, t, n, TARGET, 0), None);
        }
        // And a slow picture after the still one is still the same warning.
        assert_eq!(at(&mut watch, t, 40, TARGET, 60_000), None);
    }

    #[test]
    fn unjudgeable_windows_change_nothing_and_a_new_stream_rate_starts_over() {
        let mut watch = SourceRate::default();
        let t = Instant::now();
        for n in 0..10 {
            assert_eq!(
                watch.window(t + Duration::from_secs(5 * n), TARGET, None),
                None
            );
        }
        assert_eq!(watch.window(t, 0, Some(0)), None);
        assert_eq!(at(&mut watch, t, 0, TARGET, 60_000), None);
        assert_eq!(at(&mut watch, t, 1, TARGET, 60_000), None);
        assert_eq!(at(&mut watch, t, 2, TARGET, 60_000), below(60));
        // The stream drops to 60: the old target no longer describes it.
        assert_eq!(
            at(&mut watch, t, 3, 60_000, 50_000),
            Some(SourceRateWarning::Cleared)
        );
        assert_eq!(at(&mut watch, t, 4, 60_000, 50_000), None);
        assert_eq!(at(&mut watch, t, 5, 60_000, 50_000), None);
        assert_eq!(at(&mut watch, t, 6, 60_000, 50_000), below(50));
    }
}
