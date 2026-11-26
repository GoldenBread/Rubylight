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

/// Whether a low source rate on idd-push shows Rubylight's virtual display
/// itself delivering fewer frames than the stream can show.
///
/// `_display_mhz` is retained for signature compatibility with callers that
/// compute `limit_mhz` as `min(stream_mhz, display_mhz)` via
/// [`SourceRate::window`]; comparing `display_mhz >= limit_mhz` there would
/// always be true whenever a positive display rate is known and suppress every
/// virtual-display warning. Returns `false` when `capture_mhz` keeps up with
/// `limit_mhz`, or exceeds `source_mhz` by more than 20% (paired frames arriving
/// less than 2 ms apart).
pub fn virtual_display_limited(
    pushed: bool,
    capture_mhz: Option<u32>,
    _display_mhz: Option<u32>,
    source_mhz: u32,
    limit_mhz: u32,
) -> bool {
    if !pushed || limit_mhz == 0 {
        return false;
    }
    match capture_mhz {
        None => true,
        Some(capture) => {
            let below_limit =
                u64::from(capture) * 100 / u64::from(limit_mhz) < u64::from(LOW_PERCENT);
            let matches_source =
                u64::from(capture) <= u64::from(source_mhz) + u64::from(source_mhz) / 5;
            below_limit && matches_source
        }
    }
}

#[cfg(test)]
mod tests;
