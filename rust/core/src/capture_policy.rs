//! Capture publication and idle-wait policy, independent of Windows/GPU APIs.
use std::time::{Duration, Instant};

/// Preserve short active polls and pending GPU handoffs. After 100 ms without
/// a frame, notifications can interrupt a longer wait; the 10 ms bound still
/// services helper commands, stop and device checks.
pub fn notification_wait_interval(idle: Duration, pending: bool, poll: Duration) -> Duration {
    if pending || idle < Duration::from_millis(100) {
        poll
    } else {
        poll.max(Duration::from_millis(10))
    }
}

/// How long a display's Windows Graphics Capture may deliver no picture while an
/// exclusive-fullscreen app is in front before Desktop Duplication takes over. Long enough
/// that a game leaving fullscreen, a mode change or a busy GPU is not mistaken for a
/// capture that cannot see the display, short enough that a frozen picture recovers on its
/// own.
pub const FULLSCREEN_STALL: Duration = Duration::from_secs(2);

/// What sits in front of the streamed display, for [`duplication_for_fullscreen`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InFront {
    /// Nothing that can present: the desktop, one of Windows' own shell windows
    /// (Progman, WorkerW, Shell_TrayWnd), one of the host's own windows, or no foreground
    /// window at all (minimised, cloaked).
    Nothing,
    /// Another program's window that does not cover the whole display, or one whose
    /// bounds cannot be read.
    Partial,
    /// Another program's window that covers the whole display.
    Covering,
}

/// The Capture method values that name a backend, and are therefore the user's
/// own choice: every other value, unset or unrecognised, is Automatic.
const EXPLICIT_METHODS: [&str; 6] = ["wgc", "wgcc", "ddx", "dxgi", "idd-push", "amd"];

/// Whether the Capture method setting lets the host choose the backend: Automatic, which
/// an unset or unrecognised value stands in for (as the frame limiter resolves them). An
/// explicit method is the user's, including for the one-reconnect Windows Graphics
/// Capture against Desktop Duplication comparison, and is never overridden.
pub fn automatic_capture(capture: &str) -> bool {
    !EXPLICIT_METHODS.contains(&capture.trim().to_ascii_lowercase().as_str())
}

/// Whether a display's stream should move from Windows Graphics Capture to Desktop
/// Duplication. WGC reads the compositor's output, so an app in exclusive fullscreen (as
/// some games and emulators are) presents past it: the picture stops with no error while
/// the display keeps changing, and Desktop Duplication, which reads what the display
/// shows, captures it. Every one of these is required:
///
/// - the Capture method is Automatic (an explicit method stays),
/// - the open capture is WGC of the display itself, not of one app's window,
/// - it has sent no picture for [`FULLSCREEN_STALL`],
/// - another program's window covers the whole display, and
/// - Windows reports an exclusive-fullscreen app (`QUNS_RUNNING_D3D_FULL_SCREEN`).
///
/// A static desktop, a paused borderless game, a windowed app and a window target never
/// qualify, and neither does a stream whose user named a method.
pub fn duplication_for_fullscreen(
    automatic: bool,
    backend: &str,
    display: bool,
    idle: Duration,
    in_front: InFront,
    exclusive_fullscreen: bool,
) -> bool {
    automatic
        && backend == "wgc"
        && display
        && idle >= FULLSCREEN_STALL
        && in_front == InFront::Covering
        && exclusive_fullscreen
}

#[derive(Default)]
pub struct Intervals {
    last: Option<i64>,
    samples: [i64; 16],
    count: usize,
    next: usize,
}

/// Predict a nearby publication only after a stable source cadence is known.
/// A slow, irregular, restarted or already fresh source keeps the normal slot.
#[derive(Default)]
pub struct Freshness {
    last: Option<u64>,
    intervals: [u64; 16],
    delays: [u64; 16],
    count: usize,
    next: usize,
    median: Option<u64>,
    mean: Option<u64>,
    polling_window: Option<(u64, u64)>,
    /// The latest picture's first capture, and the intervals between the
    /// first captures of recent pictures: see `picture_interval`.
    picture: Option<u64>,
    pictures: [u64; 16],
    picture_count: usize,
    picture_next: usize,
    picture_median: Option<u64>,
    picture_mean: Option<u64>,
    picture_late: Option<u64>,
    /// The latest capture that changed the focus window, the first capture
    /// of its latest picture and the interval before that picture, and
    /// whether the latest capture only extends the focus window's latest
    /// picture: see `observe_focus`.
    focus_at: Option<u64>,
    focus_start: Option<u64>,
    focus_interval: Option<u64>,
    extends: bool,
    /// The latest publication repeated the previous picture (`capture = wgcc`):
    /// the next capture starts a picture of its own even less than
    /// `PICTURE_GAP` after it, since a real game frame can land that soon
    /// after a repeat and must not be absorbed into it.
    repeated: bool,
}
impl Freshness {
    /// A capture this soon after the previous one belongs to the same picture.
    const PICTURE_GAP: u64 = 2_000_000;
    /// The focus window animates while its pictures come at least this often;
    /// other windows' updates then extend its latest picture for twice its
    /// latest interval, and at least this long.
    const FOCUS_ANIMATING: u64 = 50_000_000;
    /// The median interval between pictures, once a few are known: a capture
    /// less than 2 ms after the previous one counts as part of its picture.
    /// WGC on a 240 Hz VRR display delivers each frame of a 120 Hz game with
    /// a second composition 0.1-1 ms later (and in some sessions another 3-4 ms
    /// later), so the capture intervals' median read 7 ms or as little as 3 ms
    /// for a game presenting every 8.3 ms. A later composition counts as a
    /// picture of its own: when many pictures have one, the median falls
    /// short of the game's interval and `late_picture_interval` still covers
    /// it. A 1000 Hz display is one long picture and has no picture interval.
    pub fn picture_interval(&self) -> Option<u64> {
        self.picture_median
    }
    /// The picture interval nearly all recent pictures arrived within: the
    /// second longest of the last 16. A game's frame times vary; a 120 Hz
    /// game's frames under VRR came 7.3 to 9.3 ms apart.
    pub fn late_picture_interval(&self) -> Option<u64> {
        self.picture_late
    }
    /// The mean interval between recent pictures. Next to `picture_interval`
    /// it tells a steady picture cadence from one alternating between two (a
    /// 240 Hz display showing a 160 fps game for one or two refreshes), where
    /// the mean of all captures cannot: a second composition of each picture
    /// halves that.
    pub fn picture_mean_interval(&self) -> Option<u64> {
        self.picture_mean
    }
    /// The mean of the recent picture intervals, a picture split in two (a
    /// third composition 2 ms or more after its start) counted once: two
    /// short neighbours that add up to the median within 1/8 are one
    /// picture, and a short one at either end of the history, whose other
    /// part may lie outside it, is left out. Any other short interval counts,
    /// so a source alternating between two cadences still shows.
    fn whole_picture_mean(&self, median: u64) -> Option<u64> {
        let len = self.pictures.len();
        let first = if self.picture_count < len {
            0
        } else {
            self.picture_next
        };
        let short = |interval: u64| interval < median - median / 4;
        let (mut sum, mut count) = (0u64, 0u64);
        // A short interval waiting for its neighbour, and whether it opened the history.
        let mut part: Option<(u64, bool)> = None;
        for i in 0..self.picture_count {
            let interval = self.pictures[(first + i) % len];
            if let Some((before, leading)) = part.take() {
                if short(interval) && (before + interval).abs_diff(median) <= median / 8 {
                    sum += before + interval;
                    count += 1;
                    continue;
                }
                if !leading {
                    sum += before;
                    count += 1;
                }
            }
            if short(interval) {
                part = Some((interval, i == 0));
            } else {
                sum += interval;
                count += 1;
            }
        }
        (count > 0).then(|| sum / count)
    }
    /// When the latest picture's first capture was presented.
    pub fn picture_start(&self) -> Option<u64> {
        self.picture
    }
    /// The median interval between recent source frames, once a few are known.
    /// The capture worker observes every frame, so this is the source cadence
    /// even when a consumer only looks at some of them.
    pub fn median_interval(&self) -> Option<u64> {
        self.median
    }
    /// The mean of the same intervals: the source's rate. A compositor that
    /// alternates between one and two refreshes per picture (4 and 8 ms on a
    /// 240 Hz display) has a median of either, but delivers about 160 frames a
    /// second either way.
    pub fn mean_interval(&self) -> Option<u64> {
        self.mean
    }
    /// Use a short polling window around one predicted publication. Once that
    /// window passes, a source that becomes static returns to the normal wait.
    pub fn poll_wait(&self, age: u64, normal: u64) -> u64 {
        let Some((early, late)) = self.polling_window else {
            return normal;
        };
        if age < early {
            normal.min(early - age)
        } else if age <= late {
            normal.min(100_000)
        } else {
            normal
        }
    }
    /// Whether the latest capture changed only pixels outside the focus window
    /// while that window kept changing (`observe_focus`): it extends the
    /// focus window's latest picture instead of starting one.
    pub fn extends_picture(&self) -> bool {
        self.extends
    }
    pub fn observe(&mut self, captured: u64, delay: u64) {
        self.observe_focus(captured, delay, None);
    }
    /// Observe a capture and whether it changed the focus window (the
    /// foreground window): `None` when the capture cannot tell. A 60 Hz game
    /// window on the 240 Hz VRR desktop next to a window animating at the
    /// display rate (the Claude desktop app presented 245 times a second)
    /// arrived as five captures per game frame, each a picture of its own:
    /// claims at the stream rate drifted through the game's frames and took
    /// some of them twice and skipped as many. A capture that changed only
    /// other windows extends the focus window's picture while that window
    /// animates (`FOCUS_ANIMATING`), and so does one that cannot tell then (a
    /// pointer move); a still or slowly changing focus window
    /// (typing, a caret) leaves every update a picture of its own, as without
    /// the distinction.
    pub fn observe_focus(&mut self, captured: u64, delay: u64, focus: Option<bool>) {
        let mut same_picture = self.record(captured, delay);
        // A real capture after a repeat is a new picture of the game, however
        // soon it comes: a game frame can land within 2 ms of a repeat.
        if std::mem::take(&mut self.repeated) {
            same_picture = false;
        }
        self.extends = false;
        let animating = match (self.focus_at, self.focus_interval) {
            (Some(at), Some(interval)) => {
                interval <= Self::FOCUS_ANIMATING
                    && captured.saturating_sub(at) <= (2 * interval).max(Self::FOCUS_ANIMATING)
            }
            _ => false,
        };
        match focus {
            Some(true) => {
                // A focus change right after another window's update is new;
                // one soon after the focus window's last change is part of it.
                same_picture = self
                    .focus_at
                    .is_some_and(|at| captured.saturating_sub(at) < Self::PICTURE_GAP);
                if !same_picture {
                    self.focus_interval =
                        self.focus_start.map(|start| captured.saturating_sub(start));
                    self.focus_start = Some(captured);
                }
                self.focus_at = Some(captured);
            }
            // While the focus window reports its changes, a composition
            // reporting none changed nothing but the cursor, which WGC draws
            // without a dirty region: it extends the picture too.
            Some(false) | None if self.picture.is_some() && animating => {
                same_picture = true;
                self.extends = true;
            }
            _ => {}
        }
        self.published(captured, same_picture);
    }
    /// Observe a picture a constant-rate capture published again at the
    /// stream period with nothing new composed (`capture = wgcc`): a picture
    /// of its own, so a consumer paces it like a game frame instead of
    /// re-encoding a still one at the still-picture rate. The focus history
    /// is dropped: the publication cadence is the capture's now, not the
    /// game's.
    pub fn observe_repeat(&mut self, captured: u64, delay: u64) {
        self.record(captured, delay);
        self.extends = false;
        self.focus_at = None;
        self.focus_start = None;
        self.focus_interval = None;
        self.repeated = true;
        self.published(captured, false);
    }
    /// Record the interval since the previous publication, and whether the
    /// capture belongs to the previous picture. Clears all history after a
    /// gap of half a second or more.
    fn record(&mut self, captured: u64, delay: u64) -> bool {
        let mut same_picture = false;
        if let Some(previous) = self.last {
            let interval = captured.saturating_sub(previous);
            if captured > previous && interval <= 500_000_000 {
                self.intervals[self.next] = interval;
                self.delays[self.next] = delay;
                self.next = (self.next + 1) % self.intervals.len();
                self.count = (self.count + 1).min(self.intervals.len());
                same_picture = interval < Self::PICTURE_GAP;
            } else {
                self.count = 0;
                self.next = 0;
                self.picture = None;
                self.focus_at = None;
                self.focus_start = None;
                self.focus_interval = None;
            }
        }
        same_picture
    }
    /// Account for one publication, once its picture is known.
    fn published(&mut self, captured: u64, same_picture: bool) {
        self.last = Some(captured);
        if !same_picture {
            match self.picture {
                Some(start) if captured - start <= 500_000_000 => {
                    self.pictures[self.picture_next] = captured - start;
                    self.picture_next = (self.picture_next + 1) % self.pictures.len();
                    self.picture_count = (self.picture_count + 1).min(self.pictures.len());
                }
                _ => {
                    self.picture_count = 0;
                    self.picture_next = 0;
                }
            }
            self.picture = Some(captured);
            self.picture_median = None;
            self.picture_mean = None;
            self.picture_late = None;
            if self.picture_count >= 4 {
                let mut pictures = self.pictures;
                pictures[..self.picture_count].sort_unstable();
                let median = pictures[self.picture_count / 2];
                self.picture_median = Some(median);
                self.picture_late = Some(pictures[self.picture_count * 7 / 8]);
                self.picture_mean = self.whole_picture_mean(median);
            }
        }
        // Polling reads the same history many times between frames. Recompute
        // its statistics only when that history changes, outside the wait loop.
        self.median = None;
        self.mean = None;
        self.polling_window = None;
        if self.count >= 4 {
            self.mean = Some(self.intervals[..self.count].iter().sum::<u64>() / self.count as u64);
            let mut intervals = self.intervals;
            intervals[..self.count].sort_unstable();
            let source = intervals[self.count / 2];
            self.median = Some(source);
            if self.count >= 8
                && source >= 3_000_000
                && intervals[0] >= source.saturating_mul(7) / 8
                && intervals[self.count - 1] <= source.saturating_mul(9) / 8
            {
                let mut delays = self.delays;
                delays[..self.count].sort_unstable();
                let expected = source.saturating_add(delays[(self.count - 1) / 10]);
                self.polling_window = Some((
                    expected.saturating_sub(1_000_000),
                    expected.saturating_add(500_000),
                ));
            }
        }
    }
}
impl Intervals {
    pub fn observe(&mut self, now: i64) -> i64 {
        if let Some(previous) = self.last {
            let period = now - previous;
            if (1..=500_000_000).contains(&period) {
                self.samples[self.next] = period;
                self.next = (self.next + 1) % self.samples.len();
                self.count = (self.count + 1).min(self.samples.len());
            }
        }
        self.last = Some(now);
        if self.count < 4 {
            return 0;
        }
        let mut sorted = self.samples;
        sorted[..self.count].sort_unstable();
        sorted[self.count / 2]
    }
}
/// Publication pacing for constant-rate capture (`capture = wgcc`): one
/// picture per stream slot, the last one published again when nothing new was
/// composed, and every publication stamped when it happens so a consumer
/// paces it like a game frame instead of repeating a still picture itself.
#[derive(Clone, Copy, Debug, Default)]
pub struct ConstantRate {
    last: Option<Instant>,
}

/// What a constant-rate capture does with the newest picture it holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Publish {
    /// A new composition is ready: publish it now.
    Fresh,
    /// Nothing new by the slot: publish the last picture again now.
    Repeat,
    /// Nothing to publish before `at`.
    Wait(Instant),
}

/// Where the slot that was due at `due` counts from when it was filled at
/// `now`: `due` itself while the fill was less than a period late (the next
/// slot stays on the grid), `now` otherwise, or when filled before `due`.
pub fn grid_next(due: Instant, now: Instant, period: Duration) -> Instant {
    if now >= due && now - due < period {
        due
    } else {
        now
    }
}

impl ConstantRate {
    /// The next slot: one period after the last publication. `None` before the
    /// first picture, so nothing is repeated that was never captured.
    pub fn due(&self, period: Duration) -> Option<Instant> {
        self.last.map(|last| last + period)
    }

    /// A picture was published at `at`; the next slot is one period after it.
    /// A publisher that ran late resumes with one picture, never a burst of
    /// the slots it missed.
    pub fn published(&mut self, at: Instant) {
        self.last = Some(at);
    }

    /// A repeat was published at `now`. The next slot follows the grid of the
    /// slot it filled, so the loop's wake-up latency does not add up to a
    /// slower stream; a repeat a period or more late (a stall) starts the grid
    /// over at `now`, still one picture and never a burst.
    pub fn repeated(&mut self, now: Instant, period: Duration) {
        self.last = Some(match self.due(period) {
            Some(due) => grid_next(due, now, period),
            None => now,
        });
    }

    /// What to do with a picture at `now`. A new composition is never held
    /// back: holding it would add up to a period of delay to every frame of a
    /// slow game. A repeat only fills a slot the last publication has already
    /// left empty, so the publications never exceed one per period on a still
    /// source. `None` before the first publication, when there is no picture
    /// to repeat and no slot to fill.
    pub fn decide(&self, now: Instant, period: Duration, fresh: bool) -> Option<Publish> {
        if fresh {
            return Some(Publish::Fresh);
        }
        let due = self.due(period)?;
        Some(if now >= due {
            Publish::Repeat
        } else {
            Publish::Wait(due)
        })
    }
}

/// Defer a copy only when a newer composition can precede the same encode slot.
/// The deadline preserves a lone update if the source becomes static.
pub fn publication_deadline(
    period: i64,
    now: i64,
    composition: i64,
    last_publish: i64,
) -> Option<i64> {
    if period <= 0 || composition <= 0 || composition.saturating_mul(5) > period.saturating_mul(3) {
        return None;
    }
    let next = if now < 0 {
        0
    } else {
        (now / period + 1).saturating_mul(period)
    };
    let previous = next - period;
    if now - previous < 6_000_000 && last_publish <= previous - period {
        return None;
    }
    if now.saturating_add(composition).saturating_add(200_000) > next {
        return None;
    }
    let deadline = next - 250_000;
    (deadline > now).then_some(deadline)
}
/// The focus window's bounds `window` in the pixels of the captured display
/// `display`, both [left, top, right, bottom] in desktop coordinates: `None`
/// when the window does not cover any of the display.
pub fn focus_on_display(window: [i32; 4], display: [i32; 4]) -> Option<[i32; 4]> {
    let left = window[0].max(display[0]);
    let top = window[1].max(display[1]);
    let right = window[2].min(display[2]);
    let bottom = window[3].min(display[3]);
    (left < right && top < bottom).then(|| {
        [
            left - display[0],
            top - display[1],
            right - display[0],
            bottom - display[1],
        ]
    })
}
/// Whether a composition with the dirty rectangles `dirty` changed the focus
/// window `focus` ([left, top, right, bottom] in the display's pixels), for
/// `Freshness::observe_focus`. `None` without a focus window on the display
/// or without dirty rectangles (only the cursor moved): the composition
/// cannot tell, and extends a picture only while the focus window animates.
pub fn focus_changed(dirty: &[[i32; 4]], focus: Option<[i32; 4]>) -> Option<bool> {
    let focus = focus?;
    (!dirty.is_empty()).then(|| {
        dirty.iter().any(|rect| {
            rect[0].max(focus[0]) < rect[2].min(focus[2])
                && rect[1].max(focus[1]) < rect[3].min(focus[3])
        })
    })
}
/// The focus label of compositions published as one picture (a capture
/// drops older ones for the newest): a change of the focus window in any of
/// them is a change of the picture.
pub fn merge_focus(earlier: Option<bool>, later: Option<bool>) -> Option<bool> {
    match (earlier, later) {
        (Some(true), _) | (_, Some(true)) => Some(true),
        (Some(false), _) | (_, Some(false)) => Some(false),
        _ => None,
    }
}
#[cfg(test)]
mod tests;
