use rubylight_protocol::phase_lock::{Report, SlackWindow};
use sdl3_sys::everything::*;

/// Windows in a row a new place of the frames takes to reach the host of a stream with no refreshes
/// to spare (`SteadyReports`): 2 s.
pub const STEADY_WINDOWS: u32 = 4;
/// Windows of a new lock that go out as they are: it has to find its place first.
const STEADY_AFTER: u32 = 6;

/// Keeps a passing change of place from the host, on a stream with no refreshes to spare.
///
/// A source faster than the stream takes frames a picture off their place now and then, for a second
/// or two, and they come back by themselves. The host answers a report from there by taking a later
/// picture, and it learns a false drift that moves them later again: they land past the desktop's
/// deadline, where a picture a refresh has none to spare and each miss repeats one and loses one. So a
/// report from a new place repeats the last place until `windows` in a row said the same. Frames that
/// came early only wait a little longer, and a refresh that missed goes through at once.
#[derive(Debug, Default)]
pub struct SteadyReports {
    kept: Option<(i32, u32)>,
    seen: u32,
    away: u32,
    missed: bool,
}

impl SteadyReports {
    /// A refresh repeated a picture that was late for it.
    pub fn missed(&mut self) { self.missed = true; }

    /// What to send for this window's `report`.
    pub fn report(&mut self, report: Report, windows: u32) -> Report {
        let missed = std::mem::take(&mut self.missed);
        self.seen = self.seen.saturating_add(1);
        let period = i64::from(report.period_ns);
        let moved = |kept: i32| (i64::from(report.lead_ns) - i64::from(kept)).rem_euclid(period);
        let held = self.kept.filter(|&(lead, _)| {
            let moved = moved(lead);
            let away = moved.min(period - moved) > period / 5 || i64::from(report.spread_ns) > period / 4;
            away && !missed && self.seen > STEADY_AFTER && self.away + 1 < windows
        });
        if let Some((lead_ns, spread_ns)) = held {
            self.away += 1;
            return Report { lead_ns, spread_ns, ..report };
        }
        self.away = 0;
        self.kept = Some((report.lead_ns, report.spread_ns));
        report
    }
}

#[derive(Default)]
pub struct Phase {
    window: SlackWindow,
    last_report: u64,
    last_frame: u64,
    pub lead_us: Option<i32>,
    steady: SteadyReports,
}

impl Phase {
    /// A refresh repeated a picture that was late for it.
    pub fn missed(&mut self) { self.steady.missed(); }

    /// `steady_windows`: how long a stream with no refreshes to spare keeps a new place from the
    /// host (`SteadyReports`), 0 for any other.
    pub fn sample(&mut self, ready_us: u64, now_us: u64, timing: Option<(i64, i64)>, steady_windows: u32) -> Option<Report> {
        let Some((latch_us, period_ns)) = timing.filter(|(_, period)| (2_000_000..=41_700_000).contains(period)) else {
            self.reset(); return None;
        };
        if now_us.saturating_sub(ready_us) > 250_000 { self.reset(); return None; }
        // A place kept before the stream stopped skipping is stale once it skips again.
        if steady_windows == 0 { self.steady = SteadyReports::default(); }
        if self.last_frame == 0 || now_us.saturating_sub(self.last_frame) > 250_000 { self.reset(); self.last_report = now_us; }
        self.last_frame = now_us;
        self.window.push((latch_us - ready_us as i64) * 1000, period_ns);
        let elapsed_us = now_us.saturating_sub(self.last_report);
        if elapsed_us < 500_000 { return None; }
        self.last_report = now_us;
        // The window's length tells the report whether a frame came every refresh.
        let report = self.window.report_over(10, elapsed_us.min(u32::MAX as u64) as i64 * 1000)?;
        self.lead_us = Some(report.lead_ns / 1000);
        Some(if steady_windows > 0 { self.steady.report(report, steady_windows) } else { report })
    }

    pub fn reset(&mut self) { *self = Self::default(); }
}

/// The latch moved `aim_us` ahead of the refresh: slack is reported to that point.
pub fn aimed(timing: Option<(i64, i64)>, aim_us: i64) -> Option<(i64, i64)> {
    timing.map(|(latch_us, period_ns)| (latch_us - aim_us, period_ns))
}

/// How long pictures in a row have to come `EARLY_SPARE_US` and a refresh ahead of the refresh that
/// opens their turn before the turns move a refresh earlier (`HeldAnchor::hold`). Longer than the
/// host's phase lock takes to move them later itself (a report or two), so the two never take turns.
pub const EARLY_FOR_US: u64 = 2_000_000;
/// See `EARLY_FOR_US`.
pub const EARLY_SPARE_US: u64 = 3_000;

/// How long after the refresh that opens a held picture's turn its present goes out, µs: the
/// picture is offered to the swap chain right after that refresh, so the chain has all but this guard
/// of the refresh to latch the flip into the turn. Aimed at the composed deadline instead (5 ms before
/// the refresh at 120 Hz) every present raced it: a picture that came a little late missed its turn,
/// and the anchor then counted the next one from the refresh it aimed at or from the one the display
/// showed, depending on the statistics, which gave holds of 3 and 1 in turn (3.4% of the frames at
/// 60 fps on 120 Hz). The guard keeps the present out of the scan of the refresh before the turn.
pub const HELD_LATCH_GUARD_US: u64 = 400;

/// The guard a held picture's present goes out after the refresh that opens its turn, µs:
/// `HELD_LATCH_GUARD_US` unless a measurement run sets `NIGHTJAR_HELD_GUARD_US` (read once when the
/// stream starts, `held_guard_from_env`), and a guard past half a refresh is clamped there at use
/// (`held_present_lead_us`, `HeldAnchor::hold`). The whole aim moves with it, so a run can A/B where
/// the present lands in its turn without a rebuild.
pub fn held_guard_us(set: Option<u64>) -> u64 { set.unwrap_or(HELD_LATCH_GUARD_US) }

/// `NIGHTJAR_HELD_GUARD_US` for a measurement run, read once when the stream starts: `held_guard_us`
/// of it, with one line in the log either way. A value that is not a number of microseconds is left
/// out and the default is used, so a typo cannot turn the hold into something else silently.
pub fn held_guard_from_env() -> u64 {
    let Some(set) = std::env::var_os("NIGHTJAR_HELD_GUARD_US") else { return held_guard_us(None); };
    let parsed = set.to_str().and_then(|us| us.parse().ok());
    match parsed {
        Some(us) => eprintln!("nightjar: NIGHTJAR_HELD_GUARD_US: fractional V-Sync's held presents go out {us} us after the refresh that opens their turn"),
        None => eprintln!("nightjar: NIGHTJAR_HELD_GUARD_US: {set:?} is not a number of microseconds; using {HELD_LATCH_GUARD_US} us"),
    }
    held_guard_us(parsed)
}

/// How long before the refresh that shows a held picture the picture must be ready, µs: its present
/// goes out `guard_us` after the refresh that opens the turn, and the report shifts its latch back
/// by this (`aimed`), so the host delivers the picture before the hold. It is the hold's own target
/// (`HeldAnchor::hold`): a steering lock delivers before it, the hold waits it out, and the present
/// goes out just after the opening refresh. Aimed at the desktop's deadline instead, the host
/// delivered after the present target and the hold never waited. A guard past half the refresh is
/// clamped to half, so the swap chain keeps about half a refresh to latch the flip and the aim never
/// reaches the turn.
pub fn held_present_lead_us(period_us: u64, guard_us: u64) -> u64 {
    period_us.saturating_sub(guard_us.min(period_us / 2))
}

/// Which refresh the last picture of a held stream was shown at (fractional V-Sync), and where the
/// next one's turn is. The swap chain's statistics name the last present shown, which may be a
/// redraw without a picture (the menu, the overlay, Frame Warp's shift): its turn would put the next
/// picture a hold behind it. So the picture's own present (`presented`) is watched until the
/// statistics show it, and until then its turn counts from the refresh its hold aimed it at, plus the
/// refreshes the display showed the last pictures after their aim (`late`).
#[derive(Debug, Default)]
pub struct HeldAnchor {
    picture: u32,
    shown_at: Option<u32>,
    /// The refresh the last picture's hold aimed it at, and the next picture's, until its present.
    aimed: Option<u32>,
    pending: Option<u32>,
    /// Refreshes the display showed the last picture after the refresh its hold aimed it at, 0 or 1
    /// (the composed pipeline's own refresh; a bigger difference is a stall, which moves the grid by
    /// at most this one refresh). A display whose composition lands a refresh later than aimed would
    /// otherwise count one picture's turn from the aim and the next one's from the display, and every
    /// other hold would land a refresh off: holds of 3 and 1 in turn, the residual 3% at 60 fps.
    late: u32,
    /// Since when pictures came a refresh early for their turn, in the clock's ticks.
    early_since: Option<u64>,
}

impl HeldAnchor {
    /// A picture went out as the swap chain's present `count`.
    pub fn presented(&mut self, count: u32) {
        (self.picture, self.shown_at, self.aimed) = (count, None, self.pending.take());
    }

    /// Another refresh rate or stream rate (`Streaming::sync_vrr`): the turns count on a new grid,
    /// and a display whose mode changed may restart its refresh count, so the aims and lateness of
    /// the old one say nothing about the turns now. The next picture counts from the first one shown
    /// after the change.
    pub fn restart(&mut self) { *self = Self::default(); }

    /// A new rate (`refreshes`, `VrrPacing::refreshes` after `Streaming::sync_vrr` read it) against
    /// the one the turns count on (`before`) starts them over when the two differ: the old grid's
    /// aim and lateness say nothing about the new one, and a display mode may restart its refresh
    /// count. The same rate keeps them, which is every present of a stream that did not change.
    /// Returns whether the turns started over.
    pub fn restart_on_rate_change(&mut self, before: u32, refreshes: u32) -> bool {
        if refreshes == before { return false; }
        self.restart();
        true
    }

    /// The refresh the last picture shows at, from the last present shown (`present_count` at
    /// `present_refresh`) and the latest refresh, which the statistics cover unless `stale` (their
    /// vblank time is a refresh or more in the past): the one it was shown at, else the refresh its
    /// hold aimed it at plus the lateness the display showed (`late`), while it waits or once a
    /// redraw passed it unseen, else the latest refresh it may have reached, and None when it is not
    /// known (no picture yet, or an unaimed one a redraw passed unseen).
    #[cfg_attr(not(windows), allow(dead_code))] // Only Windows counts refreshes.
    pub fn last_refresh(&mut self, present_count: u32, present_refresh: u32, sync_refresh: u32, stale: bool) -> Option<u32> {
        if self.picture == 0 || present_refresh == 0 { return None; }
        if present_count == self.picture {
            self.shown_at = Some(present_refresh);
            self.late = self.aimed.map_or(0, |aimed| (present_refresh.wrapping_sub(aimed) as i32).clamp(0, 1) as u32);
        }
        if self.shown_at.is_some() { return self.shown_at; }
        let aimed = self.aimed.map(|aimed| aimed.wrapping_add(self.late));
        let waiting = (self.picture.wrapping_sub(present_count) as i32) > 0;
        if !waiting { return aimed; }
        // Still waiting: the refresh its aim names, plus the lateness the display showed, and past
        // that the latest refresh it may have reached; a picture without an aim shows at the next.
        // The aim counts while it is still ahead, and the latest refresh only where the statistics
        // are a refresh stale, so the picture may have been shown at one they do not cover. The
        // latest refresh they do cover is not a picture's: a present they leave out of
        // `present_count` was not shown at it, so a missed turn counts from the next refresh the
        // statistics cover, or from the latest one they name when they are stale, never from the one
        // it missed (that made the next turn a refresh early and the one after it a refresh short,
        // once a miss; holds of 3 and 1 in turn, the residual 3% at 60 fps).
        let next = sync_refresh.wrapping_add(1);
        Some(aimed.map_or(next, |aimed| {
            if (aimed.wrapping_sub(sync_refresh) as i32) > 0 { aimed } else if stale { sync_refresh } else { next }
        }))
    }

    /// Fractional V-Sync: how long a picture waits before its present, which then goes out at the
    /// next refresh, so it is shown at its turn: `held` refreshes after the refresh the last picture
    /// shows at (`last`, `last_refresh`). The present goes out `guard` into the refresh that opens
    /// the turn, so the swap chain has all but the guard of the refresh to latch the flip; a guard
    /// past half the refresh is clamped to half, so it keeps about half a refresh to latch it.
    /// `held_present_lead_us`
    /// (the same `period` and `guard`) is the target the phase lock aims the host's frames at, so a
    /// steering lock delivers the picture before its hold. `margin` is the desktop's deadline before
    /// a refresh, which only says which turn a picture that comes late takes (the first whose
    /// deadline it still makes, at most `held` refreshes and one more behind a last picture still
    /// waiting). Nothing for a picture later than that, which the next ones then count from, or with
    /// no hold or no known last picture. A swap chain's own sync interval holds the picture too, but
    /// a picture early for its turn waits in the swap chain's queue, where each one after it waits as
    /// well and the phase report, which counts within the hold, never sees it: 60 fps on 120 Hz went
    /// from 9 to 36 ms that way.
    ///
    /// Pictures that come `early_spare` and a refresh ahead of their turn's open for `early_for` in a
    /// row move the turns a refresh earlier, once: a source whose clock runs ahead of the display brings
    /// them earlier and earlier, each waiting longer, up to the whole hold, until a host's phase lock
    /// moves them later, and a host without one never does (0 and 1 show the move only).
    ///
    /// Ticks of the clock of `now` and `sync_ticks`, the latest refresh `sync_refresh`'s time, and of
    /// `period`, `margin`, `guard`, `early_spare` and `early_for`. The refresh it aims the picture at
    /// goes to the next `presented`.
    #[allow(clippy::too_many_arguments)]
    pub fn hold(&mut self, now: u64, sync_ticks: u64, sync_refresh: u32, last: Option<u32>, held: u32, period: u64, margin: u64,
        guard: u64, early_spare: u64, early_for: u64) -> u64 {
        let Some(last) = last.filter(|_| held > 1 && period > 0) else {
            (self.pending, self.early_since) = (None, None);
            return 0;
        };
        let (period, margin) = (period.min(i64::MAX as u64) as i64, margin.min(i64::MAX as u64) as i64);
        // A guard past half the refresh would leave the swap chain less than half a refresh to latch
        // the flip: clamp it, as the report's aim does.
        let guard = guard.min((period / 2) as u64) as i64;
        let mut ahead = i64::from(last.wrapping_add(held).wrapping_sub(sync_refresh) as i32);
        // The present goes out just after the refresh before the turn: the swap chain latches the
        // flip into the turn with all but the guard of a refresh to spare. The phase lock aims at the
        // same point (`held_present_lead_us`), so the picture is ready before this wait starts.
        let lead = held_present_lead_us(period as u64, guard as u64) as i64;
        let wait = |ahead: i64| sync_ticks as i64 + ahead * period - lead - now as i64;
        // A refresh earlier is still after the last picture's: `held` is 2 or more.
        if wait(ahead - 1) >= early_spare.min(i64::MAX as u64) as i64 {
            let since = *self.early_since.get_or_insert(now);
            if now.saturating_sub(since) >= early_for {
                ahead -= 1;
                self.early_since = None;
            }
        } else {
            self.early_since = None;
        }
        // The refresh it shows at: its turn, or the first whose deadline it still makes.
        let first = (now as i64 + margin - sync_ticks as i64 + period - 1).div_euclid(period);
        self.pending = Some(sync_refresh.wrapping_add(ahead.max(first) as u32));
        wait(ahead).clamp(0, i64::from(held + 1) * period) as u64
    }
}

/// The refresh a picture held for `held` refreshes is due at, and that many refreshes as the
/// period (fractional V-Sync): its turns count from the refresh the last picture was shown at
/// (`last`, `HeldAnchor::last_refresh`), so the latch is the latest refresh of that grid,
/// `sync_refresh` being the latest refresh at `sync_ticks`. The refresh itself, and its period, for
/// `held` 1 or no known refresh of the last picture. Ticks and periods in the clock's own units.
#[cfg_attr(not(windows), allow(dead_code))] // Only Windows counts refreshes.
pub fn held_latch(sync_ticks: u64, sync_refresh: u32, last: Option<u32>, held: u32, period: u64) -> (u64, u64) {
    let Some(last) = last.filter(|_| held > 1) else { return (sync_ticks, period) };
    // Signed: the last picture may still wait for a refresh after the latest one (`last_refresh`),
    // and a remainder of the unsigned difference is right only where `held` divides 2^32.
    let since = i64::from(sync_refresh.wrapping_sub(last) as i32).rem_euclid(i64::from(held)) as u64;
    (sync_ticks.saturating_sub(since * period), period * u64::from(held))
}

/// Fractional V-Sync where the display tells when a picture reached it as a time rather than a
/// refresh count: Wayland's presentation feedback, or a KMS commit, which returns at the refresh
/// that shows it. The turns are `HeldAnchor::hold`'s, counted in refreshes of the display's period
/// from the last picture's time (`shown`), or from the time its hold aimed it at until that comes.
/// Times on the session clock, µs.
#[derive(Debug, Default)]
pub struct HeldClock {
    picture: Option<u32>,
    /// The picture before `picture`, whose feedback may still come after `picture` went out.
    before: Option<u32>,
    shown_us: Option<u64>,
    aimed_us: Option<u64>,
    pending_us: Option<u64>,
    /// The last hold's refreshes and refresh period, µs.
    held: u32,
    period_us: u64,
    turns: HeldAnchor,
}

impl HeldClock {
    /// The picture of stream frame `frame` went out.
    pub fn presented(&mut self, frame: u32) {
        self.before = self.picture;
        (self.picture, self.shown_us, self.aimed_us) = (Some(frame), None, self.pending_us.take());
    }

    /// Frame `frame` reached the display at `at_us`. The last picture's time is where the next turn
    /// counts from. The feedback of the picture before it may come only after the last one went out
    /// (the compositor answers at the refresh, the poll runs after a present): with nothing else
    /// known of the last picture, it shows a hold after that one.
    pub fn shown(&mut self, frame: u32, at_us: u64) {
        if self.picture == Some(frame) {
            self.shown_us = Some(at_us);
        } else if self.before == Some(frame) && self.shown_us.is_none() && self.aimed_us.is_none() && self.held > 1 {
            self.aimed_us = Some(at_us + u64::from(self.held) * self.period_us);
        }
    }

    fn last_us(&self) -> Option<u64> { self.shown_us.or(self.aimed_us) }

    /// `HeldAnchor::hold` for the next picture, `period_us` a refresh: how long it waits before its
    /// present, `margin_us` the desktop's deadline and `guard_us` (`held_guard_us`, µs) how long after
    /// the refresh that opens its turn the present goes out, whose lead the phase report aims at
    /// (`held_present_lead_us`).
    pub fn hold(&mut self, now_us: u64, held: u32, period_us: u64, margin_us: u64, guard_us: u64) -> u64 {
        (self.held, self.period_us) = (held, period_us);
        let Some(last) = self.last_us().filter(|_| held > 1 && period_us > 0) else {
            self.pending_us = None;
            return self.turns.hold(now_us, 0, 0, None, held, period_us, margin_us, guard_us, EARLY_SPARE_US, EARLY_FOR_US);
        };
        let wait = self.turns.hold(now_us, last, 0, Some(0), held, period_us, margin_us, guard_us, EARLY_SPARE_US, EARLY_FOR_US);
        self.pending_us = self.turns.pending.take().map(|refresh| last + u64::from(refresh) * period_us);
        wait
    }

    /// The phase report's latch and period for a held stream (`timing`'s): the latest refresh of the
    /// grid the turns count on, at or before `now_us`, and `held` refreshes. None without a hold or a
    /// last picture, or with one long gone.
    pub fn timing(&self, now_us: u64, held: u32, period_us: u64) -> Option<(i64, i64)> {
        let last = self.last_us().filter(|_| held > 1 && period_us > 0)?;
        let grid = u64::from(held) * period_us;
        let latch = if now_us >= last { last + (now_us - last) / grid * grid } else { last.checked_sub((last - now_us).div_ceil(grid) * grid)? };
        (now_us - latch <= 100_000 + grid).then_some((latch as i64, (grid * 1000) as i64))
    }
}

/// Whether the display tells when it showed a picture, which fractional V-Sync's turns count from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeldTimes {
    /// It does: pictures are held for their turns.
    Told,
    /// Not known yet (Wayland's feedback before the first answer was due): nothing held, nothing said.
    Unknown,
    /// It does not (X11, a compositor without timed feedback, swap chain statistics failing): each
    /// picture shows at the next refresh, and the log says why once.
    Untold,
}

/// How long a stream's first feedback request may wait for a timed answer before the display
/// counts as giving none (`glass_times`), µs.
pub const FEEDBACK_DUE_US: u64 = 1_000_000;

/// `HeldTimes` of Wayland's presentation feedback: `offered` None before the first request,
/// Some(false) where it is not offered or carries no times, else timed answers since
/// `last_timed_us`, the first request at `first_request_us`. Times on the session clock, µs.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))] // Only Linux has the feedback.
pub fn glass_times(offered: Option<bool>, first_request_us: Option<u64>, last_timed_us: Option<u64>, now_us: u64) -> HeldTimes {
    match offered {
        None => HeldTimes::Unknown,
        Some(false) => HeldTimes::Untold,
        Some(true) if last_timed_us.is_some_and(|at| now_us.saturating_sub(at) < FEEDBACK_DUE_US) => HeldTimes::Told,
        // Answers that stopped are as overdue as a first one that never came.
        Some(true) if now_us.saturating_sub(last_timed_us.or(first_request_us).unwrap_or(now_us)) < FEEDBACK_DUE_US => HeldTimes::Unknown,
        Some(true) => HeldTimes::Untold,
    }
}

/// How long a swap chain whose statistics failed holds nothing before it is tried again
/// (`StatsWatch`), µs.
pub const STATS_RETRY_US: u64 = 2_000_000;

/// Whether the swap chain's statistics, which fractional V-Sync's turns count from, can be read. A
/// single failure passes (the first read after a mode change may fail); two in a row hold nothing
/// for `STATS_RETRY_US`, then the next held picture tries again.
#[derive(Debug, Default)]
pub struct StatsWatch {
    fails: u32,
    since_us: Option<u64>,
}

impl StatsWatch {
    /// A held picture read the statistics (`ok`) or could not.
    pub fn read(&mut self, ok: bool, now_us: u64) {
        if ok { *self = Self::default(); return; }
        self.fails = self.fails.saturating_add(1);
        // A failed try after the wait starts a new one.
        if self.since_us.is_none_or(|since| now_us.saturating_sub(since) >= STATS_RETRY_US) { self.since_us = Some(now_us); }
    }

    #[cfg_attr(not(windows), allow(dead_code))] // Only Windows reads the statistics.
    pub fn times(&self, now_us: u64) -> HeldTimes {
        match self.since_us {
            Some(since) if self.fails >= 2 && now_us.saturating_sub(since) < STATS_RETRY_US => HeldTimes::Untold,
            _ => HeldTimes::Told,
        }
    }
}

/// Whether a swap chain's statistics are a refresh stale: the vblank they name (`SyncQPCTime`, QPC)
/// is `period` or more in the past, so a later refresh may have passed that they do not cover
/// (`HeldAnchor::last_refresh`). Callers check the time is positive.
#[cfg(windows)]
fn stats_stale(sync_qpc_time: i64, period: u64) -> bool {
    // SAFETY: a plain query of the clock.
    let now = unsafe { SDL_GetPerformanceCounter() };
    now.saturating_sub(sync_qpc_time as u64) > period
}

/// `HeldAnchor::hold` for the renderer's swap chain, in microseconds; None when its statistics
/// cannot be read (`StatsWatch`). `guard_us` is `held_guard_us`.
#[cfg(windows)]
pub fn held_wait_us(renderer: *mut SDL_Renderer, refresh_hz: f32, held: u32, margin_us: u64, guard_us: u64, anchor: &mut HeldAnchor) -> Option<u64> {
    use windows::core::Interface;
    use windows::Win32::Graphics::Dxgi::{IDXGISwapChain, DXGI_FRAME_STATISTICS};
    if held <= 1 || refresh_hz <= 0.0 { return Some(0); }
    // SAFETY: the chain pointer comes from the live renderer and is only borrowed for these calls.
    unsafe {
        let chain = crate::sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_D3D11_SWAPCHAIN_POINTER);
        let chain = IDXGISwapChain::from_raw_borrowed(&chain)?;
        let mut stats = DXGI_FRAME_STATISTICS::default();
        if chain.GetFrameStatistics(&mut stats).is_err() || stats.SyncQPCTime <= 0 { return None; }
        let freq = SDL_GetPerformanceFrequency() as f64;
        let ticks = |us: f64| (us * freq / 1e6) as u64;
        let period = ticks(1e6 / f64::from(refresh_hz));
        let last = anchor.last_refresh(stats.PresentCount, stats.PresentRefreshCount, stats.SyncRefreshCount,
            stats_stale(stats.SyncQPCTime, period));
        let wait = anchor.hold(SDL_GetPerformanceCounter(), stats.SyncQPCTime as u64, stats.SyncRefreshCount, last,
            held, period, ticks(margin_us as f64), ticks(guard_us as f64),
            ticks(EARLY_SPARE_US as f64), ticks(EARLY_FOR_US as f64));
        Some((wait as f64 * 1e6 / freq) as u64)
    }
}

/// The swap chain's count of the last present, for `HeldAnchor::presented`; 0 without one.
#[cfg(windows)]
pub fn last_present_count(renderer: *mut SDL_Renderer) -> u32 {
    use windows::core::Interface;
    use windows::Win32::Graphics::Dxgi::IDXGISwapChain;
    // SAFETY: the chain pointer comes from the live renderer and is only borrowed for this call.
    unsafe {
        let chain = crate::sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_D3D11_SWAPCHAIN_POINTER);
        IDXGISwapChain::from_raw_borrowed(&chain).and_then(|chain| chain.GetLastPresentCount().ok()).unwrap_or(0)
    }
}

/// Only Windows has the count.
#[cfg(not(windows))]
pub fn last_present_count(_: *mut SDL_Renderer) -> u32 { 0 }

/// Only Windows holds pictures for their turn.
#[cfg(not(windows))]
pub fn held_wait_us(_: *mut SDL_Renderer, _: f32, _: u32, _: u64, _: u64, _: &mut HeldAnchor) -> Option<u64> { Some(0) }

#[cfg(any(windows, test))]
fn latch_time_us(now_us: u64, counter: u64, latch: u64, freq: u64, period_ns: i64) -> Option<(i64, i64)> {
    if freq == 0 { return None; }
    let delta_ticks = counter as i128 - latch as i128;
    let age_us = delta_ticks as f64 * 1_000_000.0 / freq as f64;
    if age_us.abs() > 100_000.0 { return None; }
    Some((now_us as i64 - age_us as i64, period_ns))
}

#[cfg(windows)]
pub fn timing(renderer: *mut SDL_Renderer, refresh_hz: f32, held: u32, anchor: &mut HeldAnchor) -> Option<(i64, i64)> {
    use windows::core::Interface;
    use windows::Win32::Graphics::Dxgi::{IDXGISwapChain, DXGI_FRAME_STATISTICS};
    use windows::Win32::Graphics::Dwm::{DwmGetCompositionTimingInfo, DWM_TIMING_INFO};
    unsafe {
        let period_ns = (1_000_000_000.0 / f64::from(refresh_hz)) as i64;
        let chain = crate::sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_D3D11_SWAPCHAIN_POINTER);
        let stats = IDXGISwapChain::from_raw_borrowed(&chain).and_then(|chain| {
            let mut stats = DXGI_FRAME_STATISTICS::default();
            chain.GetFrameStatistics(&mut stats).ok().map(|_| stats)
        });
        let held_stats = stats.filter(|s| s.SyncQPCTime > 0).map(|s| {
            let ticks_per_refresh = (period_ns as f64 * SDL_GetPerformanceFrequency() as f64 / 1e9) as u64;
            let stale = stats_stale(s.SyncQPCTime, ticks_per_refresh);
            let last = if held > 1 { anchor.last_refresh(s.PresentCount, s.PresentRefreshCount, s.SyncRefreshCount, stale) } else { None };
            let (latch, _) = held_latch(s.SyncQPCTime as u64, s.SyncRefreshCount, last, held, ticks_per_refresh);
            let (_, period) = held_latch(0, s.SyncRefreshCount, last, held, period_ns as u64);
            (latch, period as i64)
        });
        // DWM's clock knows no refresh of the last picture, so it keeps the refresh itself.
        let (latch, period_ns) = if let Some(held) = held_stats { held } else {
            let window = SDL_GetRenderWindow(renderer);
            // DWM exposes the primary compositor's clock, not the phase of another monitor.
            if SDL_GetDisplayForWindow(window) != SDL_GetPrimaryDisplay() { return None; }
            let mut info = DWM_TIMING_INFO { cbSize: std::mem::size_of::<DWM_TIMING_INFO>() as u32, ..Default::default() };
            DwmGetCompositionTimingInfo(windows::Win32::Foundation::HWND::default(), &mut info).ok()?;
            let measured = (info.qpcRefreshPeriod as f64 * 1_000_000_000.0 / SDL_GetPerformanceFrequency() as f64) as i64;
            if (measured - period_ns).abs() > period_ns / 100 { return None; }
            (info.qpcVBlank, measured)
        };
        latch_time_us(nightjar_core::session::now_us(), SDL_GetPerformanceCounter(), latch, SDL_GetPerformanceFrequency(), period_ns)
    }
}

/// The swap chain's counters and last vblank time right after a present call, for the timing CSV
/// (`timing_csv::Flip`). Empty when the renderer has no DXGI swap chain.
#[cfg(windows)]
pub fn present_stats(renderer: *mut SDL_Renderer) -> crate::timing_csv::Flip {
    use windows::core::Interface;
    use windows::Win32::Graphics::Dxgi::{IDXGISwapChain, DXGI_FRAME_STATISTICS};
    // SAFETY: the chain pointer comes from the live renderer and is only borrowed for these calls.
    unsafe {
        let chain = crate::sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_D3D11_SWAPCHAIN_POINTER);
        let Some(chain) = IDXGISwapChain::from_raw_borrowed(&chain) else { return Default::default() };
        let mut flip = crate::timing_csv::Flip { present_count: chain.GetLastPresentCount().unwrap_or(0), ..Default::default() };
        let mut stats = DXGI_FRAME_STATISTICS::default();
        if chain.GetFrameStatistics(&mut stats).is_ok() {
            flip.stat_present_count = stats.PresentCount;
            flip.stat_present_refresh = stats.PresentRefreshCount;
            flip.stat_sync_refresh = stats.SyncRefreshCount;
            let age = SDL_GetPerformanceCounter().checked_sub(stats.SyncQPCTime as u64).filter(|_| stats.SyncQPCTime > 0);
            if let Some(age) = age {
                let age_us = (age as f64 * 1_000_000.0 / SDL_GetPerformanceFrequency() as f64) as u64;
                flip.stat_sync_us = nightjar_core::session::now_us().saturating_sub(age_us);
            }
        }
        flip
    }
}

#[cfg(not(windows))]
pub fn present_stats(_: *mut SDL_Renderer) -> crate::timing_csv::Flip { Default::default() }

#[cfg(not(windows))]
pub fn timing(renderer: *mut SDL_Renderer, _: f32, _held: u32, _anchor: &mut HeldAnchor) -> Option<(i64, i64)> {
    type Sync = unsafe extern "C" fn(*mut std::ffi::c_void, libc::c_ulong, *mut i64, *mut i64, *mut i64) -> i32;
    type Rate = unsafe extern "C" fn(*mut std::ffi::c_void, libc::c_ulong, *mut i32, *mut i32) -> i32;
    // Looked up once, when a context has them; a missing one is looked up again next time.
    static PROCS: std::sync::OnceLock<(Sync, Rate)> = std::sync::OnceLock::new();
    unsafe {
        if SDL_GL_GetCurrentContext().is_null() { return None; }
        let props = SDL_GetWindowProperties(SDL_GetRenderWindow(renderer));
        let display = SDL_GetPointerProperty(props, SDL_PROP_WINDOW_X11_DISPLAY_POINTER, std::ptr::null_mut());
        let drawable = SDL_GetNumberProperty(props, SDL_PROP_WINDOW_X11_WINDOW_NUMBER, 0) as libc::c_ulong;
        if display.is_null() || drawable == 0 { return None; }
        let (sync, rate) = match PROCS.get() {
            Some(&procs) => procs,
            None => {
                let sync: Sync = std::mem::transmute(SDL_GL_GetProcAddress(c"glXGetSyncValuesOML".as_ptr())?);
                let rate: Rate = std::mem::transmute(SDL_GL_GetProcAddress(c"glXGetMscRateOML".as_ptr())?);
                *PROCS.get_or_init(|| (sync, rate))
            }
        };
        let (mut ust, mut msc, mut sbc, mut numerator, mut denominator) = (0, 0, 0, 0, 0);
        if sync(display, drawable, &mut ust, &mut msc, &mut sbc) == 0
            || rate(display, drawable, &mut numerator, &mut denominator) == 0 || numerator <= 0 || denominator <= 0 { return None; }
        // Mesa's UST uses CLOCK_MONOTONIC. Reject other epochs instead of inventing a phase.
        let mut now: libc::timespec = std::mem::zeroed();
        if libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) != 0 { return None; }
        let age_us = now.tv_sec * 1_000_000 + now.tv_nsec / 1000 - ust;
        if !(0..100_000).contains(&age_us) { return None; }
        Some((nightjar_core::session::now_us() as i64 - age_us, 1_000_000_000 * i64::from(denominator) / i64::from(numerator)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measured_latch_reports_are_bounded_and_reset_after_missing_timing() {
        let mut phase = Phase::default();
        let mut report = None;
        for n in 1..=61 {
            let now = n * 10_000;
            report = phase.sample(now - 2000, now, Some((now as i64, 10_000_000)), 0).or(report);
        }
        let report = report.unwrap();
        assert_eq!(report.lead_ns, 2_000_000);
        assert!(Report::decode(&report.encode()).is_some());
        assert_eq!(phase.sample(700_000, 700_000, None, 0), None);
        assert_eq!(phase.lead_us, None);
        assert_eq!(phase.sample(800_000, 800_000, Some((800_000, 10_000_000)), 0), None);
        assert_eq!(phase.sample(800_000, 1_100_000, Some((1_100_000, 10_000_000)), 0), None);
    }

    fn at(lead_ns: i32) -> Report {
        Report { frames: 60, period_ns: 8_333_333, lead_ns, spread_ns: 1_300_000 }
    }

    fn settled() -> SteadyReports {
        let mut steady = SteadyReports::default();
        for _ in 0..=STEADY_AFTER { steady.report(at(2_000_000), STEADY_WINDOWS); }
        steady
    }

    /// Reports of real runs: frames a picture early for two windows, then back.
    #[test]
    fn a_passing_change_of_place_never_reaches_the_host() {
        let mut steady = settled();
        for lead in [-2_640_000, -1_840_000] {
            assert_eq!(steady.report(at(lead), STEADY_WINDOWS).lead_ns, 2_000_000);
        }
        assert_eq!(steady.report(at(2_490_000), STEADY_WINDOWS).lead_ns, 2_490_000, "near the last place: as it is");
    }

    #[test]
    fn a_window_across_a_change_of_place_keeps_the_last_spread_too() {
        let mut steady = settled();
        let across = steady.report(Report { spread_ns: 4_900_000, ..at(2_100_000) }, STEADY_WINDOWS);
        assert_eq!((across.lead_ns, across.spread_ns), (2_000_000, 1_300_000));
    }

    #[test]
    fn a_new_place_that_lasts_reaches_the_host() {
        let mut steady = settled();
        for _ in 1..STEADY_WINDOWS {
            assert_eq!(steady.report(at(-2_500_000), STEADY_WINDOWS).lead_ns, 2_000_000);
        }
        assert_eq!(steady.report(at(-2_500_000), STEADY_WINDOWS).lead_ns, -2_500_000);
        assert_eq!(steady.report(at(-2_400_000), STEADY_WINDOWS).lead_ns, -2_400_000, "the new place is the place now");
    }

    #[test]
    fn a_missed_refresh_goes_through_at_once() {
        let mut steady = settled();
        steady.missed();
        assert_eq!(steady.report(at(-1_000_000), STEADY_WINDOWS).lead_ns, -1_000_000);
        assert_eq!(steady.report(at(2_000_000), STEADY_WINDOWS).lead_ns, -1_000_000, "the miss counts once");
    }

    #[test]
    fn a_new_lock_finds_its_place_unheld() {
        let mut steady = SteadyReports::default();
        for lead in [-1_530_000, 3_020_000, -210_000, -3_850_000, 2_130_000, -2_000_000] {
            assert_eq!(steady.report(at(lead), STEADY_WINDOWS).lead_ns, lead);
        }
    }

    #[test]
    fn across_the_wrap_of_a_refresh_is_no_change_of_place() {
        let mut steady = settled();
        assert_eq!(steady.report(at(3_600_000), STEADY_WINDOWS).lead_ns, 3_600_000);
        assert_eq!(steady.report(at(4_100_000), STEADY_WINDOWS).lead_ns, 4_100_000);
        assert_eq!(steady.report(at(-4_166_000), STEADY_WINDOWS).lead_ns, -4_166_000);
    }

    #[test]
    fn a_stream_with_refreshes_to_spare_sends_every_window_as_it_is() {
        let mut phase = Phase::default();
        let mut leads = Vec::new();
        for n in 1..=400u64 {
            let now = n * 10_000;
            // Frames move a quarter of a refresh every second.
            let ready = now - if n < 200 { 2000 } else { 4500 };
            if let Some(report) = phase.sample(ready, now, Some((now as i64, 10_000_000)), 0) { leads.push(report.lead_ns); }
        }
        assert_eq!(leads.last(), Some(&4_500_000));
    }

    #[test]
    fn a_stream_that_skips_again_finds_its_place_unheld() {
        let mut phase = Phase::default();
        let mut leads = Vec::new();
        for n in 1..=700u64 {
            let now = n * 10_000;
            // Settled while skipping, then 2 s with refreshes to spare, then skipping a quarter of a
            // refresh later.
            let (ready, windows) = match n {
                ..=400 => (now - 2000, STEADY_WINDOWS),
                401..=600 => (now - 2000, 0),
                _ => (now - 4500, STEADY_WINDOWS),
            };
            if let Some(report) = phase.sample(ready, now, Some((now as i64, 10_000_000)), windows) { leads.push(report.lead_ns); }
        }
        assert_eq!(leads.last(), Some(&4_500_000));
    }

    #[test]
    fn slack_is_reported_to_the_point_the_lock_aims_at() {
        let mut phase = Phase::default();
        let mut report = None;
        for n in 1..=61 {
            let now = n * 10_000;
            report = phase.sample(now - 7000, now, aimed(Some((now as i64, 10_000_000)), 5_000), 0).or(report);
        }
        assert_eq!(report.unwrap().lead_ns, 2_000_000, "ready 7 ms before the refresh is 2 ms before a deadline 5 ms ahead of it");
    }

    #[test]
    fn a_held_picture_latches_on_the_refresh_its_hold_ends_on() {
        // Shown at refresh 100, held for 2: the grid is 100, 102, 104...
        assert_eq!(held_latch(10_000, 105, Some(100), 2, 1000), (9_000, 2000), "latest refresh 105: the grid's 104");
        assert_eq!(held_latch(10_000, 104, Some(100), 2, 1000), (10_000, 2000));
        assert_eq!(held_latch(10_000, 107, Some(100), 3, 1000), (9_000, 3000), "held 3: 106");
        assert_eq!(held_latch(10_000, 100, Some(100), 4, 1000), (10_000, 4000), "the refresh it was shown at");
        assert_eq!(held_latch(10_000, 1, Some(u32::MAX), 2, 1000), (10_000, 2000), "across the counter's wrap");
        assert_eq!(held_latch(10_000, 2, Some(u32::MAX), 3, 1000), (10_000, 3000), "held 3 across the counter's wrap");
    }

    #[test]
    fn a_last_picture_still_waiting_latches_on_its_grids_refresh_before() {
        // The last picture waits for 101, the latest refresh is 100: held 3, its grid is 95, 98, 101.
        assert_eq!(held_latch(10_000, 100, Some(101), 3, 1000), (8_000, 3000), "98, not 100");
        assert_eq!(held_latch(10_000, 100, Some(101), 2, 1000), (9_000, 2000), "held 2: 99");
        assert_eq!(held_latch(10_000, 100, Some(102), 4, 1000), (8_000, 4000), "held 4: 98");
    }

    #[test]
    fn without_a_hold_or_a_known_refresh_the_latch_is_the_refresh() {
        assert_eq!(held_latch(10_000, 105, Some(100), 1, 1000), (10_000, 1000));
        assert_eq!(held_latch(10_000, 105, None, 2, 1000), (10_000, 1000), "no picture shown yet");
    }

    #[test]
    fn a_held_stream_reports_its_slack_over_the_held_period() {
        let mut phase = Phase::default();
        let mut report = None;
        for n in 1..=61 {
            let now = n * 16_667;
            // Latched every 16.7 ms (60 fps held 2 refreshes on 120 Hz), ready 3 ms before.
            report = phase.sample(now - 3000, now, Some((now as i64, 16_666_666)), 0).or(report);
        }
        let report = report.unwrap();
        assert_eq!((report.period_ns, report.lead_ns), (16_666_666, 3_000_000));
        assert!(Report::decode(&report.encode()).is_some(), "the host takes a period of a stream frame");
    }

    /// `HeldAnchor::hold` of a fresh anchor, which never moves the turns earlier: a guard of 50
    /// ticks into the refresh before the turn.
    fn held_wait(now: u64, sync_ticks: u64, sync_refresh: u32, last: Option<u32>, held: u32, period: u64, margin: u64) -> u64 {
        HeldAnchor::default().hold(now, sync_ticks, sync_refresh, last, held, period, margin, 50, u64::MAX, 0)
    }

    #[test]
    fn a_held_picture_waits_for_its_turn_and_a_late_one_goes_at_once() {
        // Refresh 100 at tick 10_000, 1000 ticks a refresh, margin 300; last picture shown at 100.
        assert_eq!(held_wait(10_100, 10_000, 100, Some(100), 2, 1000, 300), 950, "turn 102: after refresh 101 at 11_000");
        assert_eq!(held_wait(10_100, 10_000, 101, Some(100), 2, 1000, 300), 0, "the refresh before the turn has come: at once");
        assert_eq!(held_wait(10_100, 10_000, 100, Some(100), 3, 1000, 300), 1_950, "held 3");
        assert_eq!(held_wait(10_800, 10_000, 102, Some(100), 2, 1000, 300), 0, "past its turn: at once");
        assert_eq!(held_wait(10_100, 10_000, 100, Some(101), 2, 1000, 300), 1_950, "the last one still waits for 101");
        assert_eq!(held_wait(10_100, 10_000, 0, Some(u32::MAX), 2, 1000, 300), 0, "across the counter's wrap: refresh 1 at 11_000");
    }

    /// The present goes out just after the refresh that opens the turn, not at the desktop's
    /// deadline: the swap chain then has all but the guard of the refresh to latch the flip into the
    /// turn.
    #[test]
    fn a_held_pictures_present_goes_out_after_the_refresh_that_opens_its_turn() {
        // 120 Hz in µs: refresh 100 at 833_300, 8_333 a refresh, the deadline 5_000 before a
        // refresh, the guard 400. The last picture was shown at refresh 100, so the turn is 102.
        let (sync, ticks) = (100, 833_300);
        let (period, margin) = (8_333, 5_000);
        let hold = |now| HeldAnchor::default().hold(now, ticks, sync, Some(100), 2, period, margin, 400, u64::MAX, 0);
        assert_eq!(hold(835_300), period + 400 - 2_000, "the present goes out 400 µs after refresh 101, 7.9 ms before the turn");
        // Ready inside the refresh that opens its turn: at once, which latches the turn.
        assert_eq!(hold(842_500), 0);
        // Later than the turn's deadline: the first turn it still makes, and at once.
        let mut anchor = HeldAnchor::default();
        assert_eq!(anchor.hold(849_000, ticks, sync, Some(100), 2, period, margin, 400, u64::MAX, 0), 0);
        assert_eq!(anchor.pending, Some(103), "past 102's deadline: 103");
    }

    #[test]
    fn without_a_hold_or_a_known_last_picture_nothing_waits() {
        assert_eq!(held_wait(10_100, 10_000, 100, Some(100), 1, 1000, 300), 0);
        assert_eq!(held_wait(10_100, 10_000, 100, None, 2, 1000, 300), 0);
        assert_eq!(held_wait(0, 50_000, 100, Some(100), 2, 1000, 0), 3_000, "never more than the hold and one refresh");
    }

    /// The guard is the constant unless a measurement sets one (`NIGHTJAR_HELD_GUARD_US`), and the
    /// aim moves with it, so a device run can A/B where the present lands in its turn.
    #[test]
    fn the_guard_is_the_constant_unless_a_measurement_sets_one() {
        assert_eq!(held_guard_us(None), HELD_LATCH_GUARD_US);
        assert_eq!(held_guard_us(Some(0)), 0);
        assert_eq!(held_guard_us(Some(1_000)), 1_000);
        assert_eq!(held_present_lead_us(8_333, held_guard_us(Some(1_000))), 7_333);
    }

    /// A guard at or past the period would aim the present at the turn itself, with nothing of the
    /// refresh left to latch it: the clamp keeps the lead at half a refresh, in the report and in the
    /// hold alike.
    #[test]
    fn a_guard_at_or_past_the_period_is_clamped_to_half_a_refresh() {
        assert_eq!(held_present_lead_us(8_334, 8_334), 4_167, "a guard of the whole period");
        assert_eq!(held_present_lead_us(8_334, 8_334 * 2), 4_167, "past it");
        assert_eq!(held_present_lead_us(8_334, u64::MAX), 4_167, "a typo's");
        assert_eq!(held_present_lead_us(8_334, 4_167), 4_167, "half is the most");
        assert_eq!(held_present_lead_us(8_334, 400), 7_934, "the default is well below it");
        // 120 Hz: refresh 100 at 833_300, the turn 102 at 849_966; half a refresh of guard puts the
        // present at 845_799, and the picture arrived at 834_000 waits for it.
        let mut anchor = HeldAnchor::default();
        let wait = anchor.hold(834_000, 833_300, 100, Some(100), 2, 8_333, 5_000, u64::MAX, u64::MAX, 0);
        assert_eq!(wait, 11_799, "half a refresh after the refresh that opens the turn, less the arrival");
        assert_eq!(anchor.pending, Some(102));
    }

    /// The phase lock and the hold are one: the aim the report sends is the hold's own target
    /// (`held_present_lead_us`), so a steering host delivers the picture before the present and the
    /// hold waits the lock's margin. Aimed at the deadline the turn must make instead, the lock
    /// delivered after the present target and the hold clamped to nothing.
    #[test]
    fn a_steering_lock_delivers_at_the_present_lead_and_the_hold_waits() {
        use rubylight_protocol::phase_lock::PhaseLock;
        // 120 Hz: refresh 100 at 833_300 (the last picture's), 101 at 841_633, the turn 102 at
        // 849_966; the deadline 5_000 before a refresh, the report's period 2 refreshes.
        let (period, guard, margin, laps) = (8_333u64, HELD_LATCH_GUARD_US, 5_000u64, 16_666u64);
        let lead = held_present_lead_us(period, guard);
        assert_eq!(lead, period - guard);
        // The lock puts the early edge `DEFAULT_MARGIN_NS` past the reported aim, µs.
        let lock_us = PhaseLock::DEFAULT_MARGIN_NS / 1_000;
        let (turn, latch) = (849_966i64, 833_300i64);
        let ready = turn - lead as i64 - lock_us;
        let (aimed_latch, report_period_ns) = aimed(Some((latch, (laps * 1_000) as i64)), lead as i64).unwrap();
        assert_eq!(aimed_latch - ready + report_period_ns / 1_000, lock_us, "the lock reads its own margin, one report period on");
        let mut anchor = HeldAnchor::default();
        let wait = anchor.hold(ready as u64, latch as u64, 100, Some(100), 2, period, margin, guard, u64::MAX, 0);
        assert_eq!(wait as i64, lock_us, "delivered before its present: the hold waits");
        assert_eq!(ready + wait as i64, turn - lead as i64, "just after the refresh that opens the turn");
        assert_eq!(anchor.pending, Some(102));
        // Aimed at the deadline instead: ready after the present target, and nothing waits.
        let late_ready = turn - margin as i64 - lock_us;
        let late_wait = HeldAnchor::default().hold(late_ready as u64, 841_633, 101, Some(100), 2, period, margin, guard, u64::MAX, 0);
        assert_eq!(late_wait, 0, "the present goes out on the draw's completion, after the refresh that opens the turn");
        assert!(late_ready > turn - lead as i64, "after the present target");
    }

    #[test]
    fn the_anchor_is_the_last_pictures_refresh_not_a_redraws() {
        let mut anchor = HeldAnchor::default();
        assert_eq!(anchor.last_refresh(5, 100, 100, false), None, "no picture yet");
        anchor.presented(7);
        assert_eq!(anchor.last_refresh(6, 100, 101, false), Some(102), "still waiting: the next refresh");
        assert_eq!(anchor.last_refresh(7, 102, 102, false), Some(102), "shown");
        // A redraw (present 8) shown at 103 leaves the turns counting from 102.
        assert_eq!(anchor.last_refresh(8, 103, 103, false), Some(102));
        anchor.presented(10);
        assert_eq!(anchor.last_refresh(9, 104, 104, false), Some(105), "the next picture waits");
        assert_eq!(anchor.last_refresh(11, 106, 106, false), None, "passed by a redraw unseen: not known");
        anchor.presented(1);
        assert_eq!(anchor.last_refresh(u32::MAX, 106, 106, false), Some(107), "waiting across the counter's wrap");
    }

    /// Another display or stream rate: the next picture counts from the first one shown after it,
    /// not from the old grid's aim or lateness. The rate the turns count on is compared: the same
    /// rate keeps them, which is every present of a stream that did not change.
    #[test]
    fn a_rate_change_starts_the_turns_over() {
        let mut anchor = HeldAnchor::default();
        anchor.presented(7);
        assert_eq!(anchor.last_refresh(7, 100, 100, false), Some(100));
        anchor.hold(100_500, 100_000, 100, Some(100), 2, 1000, 300, 50, u64::MAX, 0);
        anchor.presented(8);
        assert_eq!(anchor.last_refresh(7, 100, 101, false), Some(102), "the old grid's aim");
        assert!(!anchor.restart_on_rate_change(2, 2), "the same rate keeps the turns");
        assert_eq!(anchor.last_refresh(7, 100, 101, false), Some(102), "nothing started over");
        assert!(anchor.restart_on_rate_change(2, 3), "a new rate starts them over");
        assert_eq!(anchor.last_refresh(7, 100, 101, false), None, "nothing known of a picture after the change");
        assert_eq!(anchor.pending, None);
        assert_eq!(anchor.hold(101_500, 101_000, 101, None, 3, 1000, 300, 50, u64::MAX, 0), 0, "nothing held yet");
        anchor.presented(9);
        assert_eq!(anchor.last_refresh(8, 102, 102, false), Some(103), "an unaimed picture shows at the next refresh");
    }

    #[test]
    fn a_held_picture_counts_from_the_refresh_its_hold_aimed_it_at_until_it_is_seen() {
        let mut anchor = HeldAnchor::default();
        // Last picture shown at 100 (present 7); the next one's turn is 102.
        anchor.presented(7);
        assert_eq!(anchor.last_refresh(7, 100, 100, false), Some(100));
        assert_eq!(anchor.hold(10_100, 10_000, 100, Some(100), 2, 1000, 300, 50, u64::MAX, 0), 950);
        anchor.presented(8);
        // The statistics still name present 7 at the latest refresh 101: its turn, not 102.
        assert_eq!(anchor.last_refresh(7, 100, 101, false), Some(102), "aimed at 102");
        assert_eq!(anchor.last_refresh(7, 100, 100, false), Some(102), "even read a refresh behind");
        assert_eq!(anchor.last_refresh(7, 100, 102, false), Some(103), "102 is covered and the presents leave it out: it missed");
        assert_eq!(anchor.last_refresh(7, 100, 102, true), Some(102), "read a refresh stale: it may be showing at its aim");
        assert_eq!(anchor.last_refresh(7, 100, 103, false), Some(104), "103 came and it is still unseen: 104 at the earliest");
        // A redraw (present 9) passed it unseen: still its aimed refresh, not unknown.
        assert_eq!(anchor.last_refresh(9, 103, 103, false), Some(102));
        // Seen a refresh late: the refresh it was shown at.
        anchor.presented(10);
        assert_eq!(anchor.last_refresh(10, 105, 105, false), Some(105), "the statistics win");
    }

    /// A display whose composition lands a picture a refresh after the refresh its hold aimed it at
    /// shows it there; while the statistics lag, the turns must count from that refresh too. Counting
    /// a picture from its aim and the next one from the display gave holds of 3 and 1 in turn.
    #[test]
    fn a_picture_the_statistics_have_not_named_yet_counts_from_where_the_display_shows_it() {
        let mut anchor = HeldAnchor::default();
        anchor.presented(1);
        // Picture 2's hold aims it at 102; the display shows it at 103, a refresh after the aim.
        assert_eq!(anchor.hold(100_500, 100_000, 100, Some(100), 2, 1000, 300, 50, u64::MAX, 0), 550);
        anchor.presented(2);
        assert_eq!(anchor.last_refresh(2, 103, 103, false), Some(103), "seen a refresh after its aim");
        // Picture 3 is aimed at 105, and the display will show it at 106. While the statistics still
        // name picture 2, its turn counts from 106, not from the aim 105.
        assert_eq!(anchor.hold(103_500, 103_000, 103, Some(103), 2, 1000, 300, 50, u64::MAX, 0), 550);
        anchor.presented(3);
        assert_eq!(anchor.last_refresh(2, 103, 104, false), Some(106));
        // A display that shows pictures where they were aimed counts from the aim.
        let mut on_time = HeldAnchor::default();
        on_time.presented(1);
        assert_eq!(on_time.hold(100_500, 100_000, 100, Some(100), 2, 1000, 300, 50, u64::MAX, 0), 550);
        on_time.presented(2);
        assert_eq!(on_time.last_refresh(2, 102, 102, false), Some(102));
        assert_eq!(on_time.hold(102_500, 102_000, 102, Some(102), 2, 1000, 300, 50, u64::MAX, 0), 550);
        on_time.presented(3);
        assert_eq!(on_time.last_refresh(2, 102, 103, false), Some(104), "the aim");
    }

    #[test]
    fn a_late_picture_is_aimed_at_the_first_refresh_whose_deadline_it_makes() {
        let mut anchor = HeldAnchor::default();
        // Turn 102 at 12_000 passed (now 12_900, margin 300), and 103's deadline (12_700) too: 104.
        assert_eq!(anchor.hold(12_900, 12_000, 102, Some(100), 2, 1000, 300, 50, u64::MAX, 0), 0);
        anchor.presented(5);
        assert_eq!(anchor.last_refresh(4, 101, 102, false), Some(104));
    }

    /// A stream faster than the held rate must not shorten a hold: a picture takes its own turn,
    /// `held` refreshes after the last one, and the pacer drops the picture it replaces instead
    /// (`Pacer::take_due`).
    #[test]
    fn a_stream_faster_than_the_held_rate_still_takes_a_whole_hold() {
        let mut anchor = HeldAnchor::default();
        // 120 Hz and 60 fps in µs: refresh 100 at 833_300, the last picture shown there.
        let (period, margin) = (8_333, 5_000);
        // A picture ready right after the refresh, where a faster source's next picture would come:
        // its turn is 102, two refreshes after 100, and it waits for that turn's open.
        assert_eq!(anchor.hold(834_000, 833_300, 100, Some(100), 2, period, margin, 400, EARLY_SPARE_US, EARLY_FOR_US), period + 400 - 700);
        assert_eq!(anchor.pending, Some(102));
        // Ready just after the last picture's own turn: still two refreshes after it, not one.
        anchor.presented(2);
        assert_eq!(anchor.hold(850_500, 849_966, 102, Some(102), 2, period, margin, 400, EARLY_SPARE_US, EARLY_FOR_US), period + 400 - 534);
        assert_eq!(anchor.pending, Some(104));
    }

    /// Every picture takes a refresh of the same phase on the display's grid (`held` after the one
    /// before), whatever phase of a refresh it comes in at, so the holds all come out equal.
    #[test]
    fn the_turns_keep_one_phase_on_the_refresh_grid() {
        // 40 fps on 120 Hz: a picture every 3 refreshes of 1000 ticks, margin 300, guard 50.
        let mut anchor = HeldAnchor::default();
        anchor.presented(1);
        let mut shown = vec![100u32];
        for (sync, offset) in [(100u32, 500u64), (103, 600), (106, 400), (109, 900)] {
            let last = anchor.last_refresh(shown.len() as u32, *shown.last().unwrap(), sync, false);
            let wait = anchor.hold(sync as u64 * 1000 + offset, u64::from(sync) * 1000, sync, last, 3, 1000, 300, 50, u64::MAX, 0);
            assert_eq!(wait + offset, 2_050, "waits for the refresh that opens its turn, whatever its arrival");
            let aim = anchor.pending.unwrap();
            assert_eq!(aim, last.unwrap() + 3, "aimed on the grid");
            anchor.presented(shown.len() as u32 + 1);
            shown.push(aim);
        }
        assert_eq!(shown, [100, 103, 106, 109, 112]);
    }

    #[test]
    fn a_clocked_hold_counts_its_turns_from_the_last_pictures_time() {
        let mut clock = HeldClock::default();
        assert_eq!(clock.hold(10_000, 2, 8_333, 5_000, HELD_LATCH_GUARD_US), 0, "no picture yet");
        assert_eq!(clock.timing(10_000, 2, 8_333), None);
        // Shown at 100_000; the next one's turn 116_666, opened by the refresh at 108_333.
        clock.presented(1);
        clock.shown(1, 100_000);
        clock.shown(9, 105_000);
        assert_eq!(clock.hold(103_000, 2, 8_333, 5_000, HELD_LATCH_GUARD_US), 5_733, "after the refresh that opens the turn at 108_333");
        assert_eq!(clock.timing(103_000, 2, 8_333), Some((100_000, 16_666_000)));
        assert_eq!(clock.timing(130_000, 2, 8_333), Some((116_666, 16_666_000)));
        // Until its feedback comes, the next counts from the refresh the hold aimed this one at.
        clock.presented(2);
        assert_eq!(clock.hold(118_000, 2, 8_333, 5_000, HELD_LATCH_GUARD_US), 7_399, "from 116_666: turn 133_332, opened at 124_999");
        assert_eq!(clock.timing(118_000, 2, 8_333), Some((116_666, 16_666_000)));
        // The feedback says it was a refresh late: from there.
        clock.shown(2, 124_999);
        assert_eq!(clock.hold(126_000, 2, 8_333, 5_000, HELD_LATCH_GUARD_US), 7_732);
        // A late picture aims at the first refresh whose deadline it makes, and counts on from it.
        let mut clock = HeldClock::default();
        clock.presented(1);
        clock.shown(1, 100_000);
        assert_eq!(clock.hold(113_000, 2, 8_333, 5_000, HELD_LATCH_GUARD_US), 0);
        clock.presented(2);
        assert_eq!(clock.timing(130_000, 2, 8_333), Some((124_999, 16_666_000)), "aimed at 124_999");
    }

    /// Wayland in the order the renderer runs: the hold, the present, and the poll of the answers,
    /// which comes before the next picture's hold.
    #[test]
    fn a_clocked_hold_engages_in_the_renderers_order() {
        const P: u64 = 8_333;
        let mut clock = HeldClock::default();
        // Picture 1 at 100_000 (nothing known: at once), shown at the refresh 108_333.
        assert_eq!(clock.hold(100_000, 2, P, 5_000, HELD_LATCH_GUARD_US), 0);
        clock.presented(1);
        // Picture 2, 16.7 ms on: the poll before its hold brings 1's answer.
        clock.shown(1, 108_333);
        assert_eq!(clock.hold(116_000, 2, P, 5_000, HELD_LATCH_GUARD_US), 116_666 + 400 - 116_000, "after the refresh that opens the turn 108_333 + 2 refreshes");
        assert_eq!(clock.timing(116_000, 2, P), Some((108_333, 16_666_000)), "the report has a grid");
        clock.presented(2);
        // Picture 3 before 2's answer came: from the refresh 2 was aimed at.
        assert_eq!(clock.hold(132_000, 2, P, 5_000, HELD_LATCH_GUARD_US), 133_332 + 400 - 132_000);
        clock.presented(3);
        assert!(clock.timing(140_000, 2, P).is_some());
    }

    /// Answers that come only after the next picture went out (the old order, or a compositor that
    /// answers late) still start the turns: the last picture shows a hold after the one answered.
    #[test]
    fn a_late_answer_for_the_picture_before_starts_the_turns() {
        const P: u64 = 8_333;
        let mut clock = HeldClock::default();
        assert_eq!(clock.hold(100_000, 2, P, 5_000, HELD_LATCH_GUARD_US), 0);
        clock.presented(1);
        assert_eq!(clock.hold(116_000, 2, P, 5_000, HELD_LATCH_GUARD_US), 0, "nothing known yet");
        clock.presented(2);
        clock.shown(1, 108_333);
        // 2 counts as shown at 108_333 + 2 refreshes = 124_999: 3's turn 141_665, opened at 133_332.
        assert_eq!(clock.hold(132_000, 2, P, 5_000, HELD_LATCH_GUARD_US), 133_332 + 400 - 132_000);
        assert_eq!(clock.timing(132_000, 2, P), Some((124_999, 16_666_000)));
        // An answer for an older picture, or one the last picture already has, changes nothing.
        clock.presented(3);
        clock.shown(1, 200_000);
        clock.shown(2, 124_999);
        assert_eq!(clock.timing(145_000, 2, P), Some((141_665, 16_666_000)));
    }

    #[test]
    fn the_feedback_is_unknown_until_an_answer_was_due() {
        assert_eq!(glass_times(None, None, None, 5_000_000), HeldTimes::Unknown, "no picture yet");
        assert_eq!(glass_times(Some(true), Some(1_000_000), None, 1_500_000), HeldTimes::Unknown, "the first answer not due yet");
        assert_eq!(glass_times(Some(true), Some(1_000_000), None, 2_000_000), HeldTimes::Untold, "due and absent");
        assert_eq!(glass_times(Some(true), Some(1_000_000), Some(1_020_000), 1_500_000), HeldTimes::Told);
        assert_eq!(glass_times(Some(true), Some(1_000_000), Some(1_020_000), 2_100_000), HeldTimes::Untold, "answers stopped");
        assert_eq!(glass_times(Some(false), Some(1_000_000), None, 1_000_000), HeldTimes::Untold, "not offered, or untimed");
    }

    #[test]
    fn statistics_that_keep_failing_hold_nothing_for_a_while() {
        let mut stats = StatsWatch::default();
        assert_eq!(stats.times(0), HeldTimes::Told);
        stats.read(false, 1_000);
        assert_eq!(stats.times(2_000), HeldTimes::Told, "a single failure passes");
        stats.read(true, 3_000);
        stats.read(false, 4_000);
        assert_eq!(stats.times(5_000), HeldTimes::Told, "failures not in a row");
        stats.read(false, 6_000);
        assert_eq!(stats.times(7_000), HeldTimes::Untold);
        assert_eq!(stats.times(4_000 + STATS_RETRY_US), HeldTimes::Told, "tried again");
        stats.read(false, 4_000 + STATS_RETRY_US);
        assert_eq!(stats.times(5_000 + STATS_RETRY_US), HeldTimes::Untold, "still failing: another wait");
        stats.read(true, 6_000 + STATS_RETRY_US);
        stats.read(false, 7_000 + STATS_RETRY_US);
        assert_eq!(stats.times(8_000 + STATS_RETRY_US), HeldTimes::Told);
    }

    /// The span of `source` from `from` to the next `fn ` at the same indent.
    fn function<'a>(source: &'a str, from: &str) -> &'a str {
        let start = source.find(from).unwrap_or_else(|| panic!("{from}"));
        let rest = &source[start + from.len()..];
        &rest[..rest.find("\n    pub fn ").or_else(|| rest.find("\n    fn ")).unwrap_or(rest.len())]
    }

    #[test]
    fn the_answers_are_polled_before_the_hold_and_the_present() {
        let present = function(include_str!("render.rs"), "pub fn present(");
        let poll = present.find("self.glass.poll(").expect("present polls the answers");
        assert!(poll < present.find("self.hold_for_turn_lending(").unwrap(), "before the hold with Frame Warp");
        assert!(poll < present.find("self.kms_flip(").unwrap(), "before the present and its hold");
        assert!(present.find("self.held_clock.presented(").unwrap() > present.find("self.kms_flip(").unwrap());
    }

    #[test]
    fn with_frame_warp_the_hold_comes_before_its_shift() {
        let render = include_str!("render.rs");
        let present = function(render, "pub fn present(");
        assert!(present.find("self.hold_for_turn_lending(").unwrap() < present.find("warp.draw(None)").unwrap());
        // Held once: the presents themselves hold only without Frame Warp (KMS's commit lends the input with Frame Warp too, but holds only without it).
        for flip in ["fn render_flip(", "fn kms_flip(&mut self, window"] {
            let body = function(render, flip);
            for (at, _) in body.match_indices("self.hold_for_turn(") {
                let line = body[..at].rsplit('\n').next().unwrap();
                assert!(line.contains("self.frame_warp.is_none()"), "{flip}");
            }
        }
    }

    /// Keys, clicks and controllers leave from their own threads while a hold waits (`direct_motion`),
    /// also where the present has not lent them yet: Frame Warp's hold before its draw, and KMS's.
    #[test]
    fn input_is_lent_while_a_hold_waits() {
        let render = include_str!("render.rs");
        let lending = function(render, "fn hold_for_turn_lending(");
        let lent = lending.find("self.direct_motion.presenting(Some(").expect("lends the input");
        let hold = lending.find("self.hold_for_turn(").unwrap();
        assert!(lent < hold && hold < lending.find("self.direct_motion.presenting(None)").unwrap());
        let kms = function(render, "fn kms_flip(&mut self, window");
        let lent = kms.find("self.direct_motion.presenting(Some(").expect("KMS lends the input");
        let hold = kms.find("self.hold_for_turn(").unwrap();
        let commit = kms.find("Scanout::commit").unwrap();
        let back = kms.find("self.direct_motion.presenting(None)").unwrap();
        assert!(lent < hold && hold < commit && commit < back, "lent from the hold until the commit returns");
        assert!(back < kms.find("if committed.is_err()").unwrap(), "given back on the failed commit's path too");
        // With Frame Warp too: its hold before the shift gave the input back (never nested), and the
        // commit still waits for the turn's refresh.
        assert!(kms.contains("let holds = picture && self.vrr.held(") && kms.contains("let lends = holds || "), "lent whether or not Frame Warp held before");
        let lend_line = kms[..lent].rsplit('\n').next().unwrap();
        assert!(lend_line.contains("if lends {"), "{lend_line}");
        let back_line = kms[..back].rsplit('\n').next().unwrap();
        assert!(back_line.contains("if lends {"), "given back on the same condition: {back_line}");
        // Without Frame Warp the renderer's present lends it before its own hold.
        let flip = function(render, "fn render_flip(");
        assert!(flip.find("self.direct_motion.presenting(Some(").unwrap() < flip.find("self.hold_for_turn(").unwrap());
    }

    #[test]
    fn a_display_change_starts_the_held_turns_over() {
        let changed = function(include_str!("mod.rs"), "pub fn display_changed(");
        assert!(changed.contains("(self.held_anchor, self.held_clock, self.held_stats) = Default::default();"));
    }

    /// The condition reaches the turns through `sync_vrr`, which reads the rate the pacer holds and
    /// sets the new one after it: a restart moved outside that comparison would reset the anchor
    /// every present and stop all holding.
    #[test]
    fn the_rate_change_reaches_the_turns_before_the_new_rate_holds() {
        let sync = function(include_str!("render.rs"), "fn sync_vrr(");
        let restarted = sync.find("restart_on_rate_change(self.vrr.refreshes, refreshes)").expect("looks at the rate");
        assert!(restarted < sync.find("self.vrr.set_refreshes(refreshes)").unwrap(), "before the new rate holds");
    }

    #[test]
    fn pictures_a_refresh_early_for_long_enough_move_the_turns_a_refresh_earlier() {
        let mut anchor = HeldAnchor::default();
        // 40 fps held 3 refreshes of 1000 ticks, margin 300, guard 50: the last picture at 100 (tick
        // 10_000), turn 103 at 13_000, opened by refresh 102 at 12_000. A picture ready at 10_000 is
        // early enough for 102's open too, with 1_050 to spare.
        let hold = |anchor: &mut HeldAnchor, now| anchor.hold(now, 10_000, 100, Some(100), 3, 1000, 300, 50, 1_000, 5_000);
        assert_eq!(hold(&mut anchor, 10_000), 2_050, "the turn, 103");
        assert_eq!(hold(&mut anchor, 14_000), 0, "a late one: the run starts over");
        assert_eq!(hold(&mut anchor, 10_000), 2_050, "early since 10_000");
        assert_eq!(hold(&mut anchor, 15_400), 0, "4900 later and late");
        assert_eq!(hold(&mut anchor, 10_000), 2_050);
        // Early all the way from 10_000 to 15_000: one move, to 102.
        let mut anchor = HeldAnchor::default();
        assert_eq!(anchor.hold(10_000, 10_000, 100, Some(100), 3, 1000, 300, 50, 1_000, 5_000), 2_050);
        assert_eq!(anchor.hold(15_000, 15_000, 105, Some(105), 3, 1000, 300, 50, 1_000, 5_000), 1_050, "moved: 107 for 108");
        anchor.presented(9);
        assert_eq!(anchor.last_refresh(8, 105, 105, false), Some(107));
        assert_eq!(anchor.hold(15_000, 15_000, 105, Some(105), 3, 1000, 300, 50, 1_000, 5_000), 2_050, "the run starts over");
        // Too little to spare (250 of 1000) never moves them.
        let mut anchor = HeldAnchor::default();
        for now in [10_600, 13_000, 20_000] {
            assert_eq!(anchor.hold(now, now - 800, 100, Some(100), 3, 1000, 300, 50, 1_000, 5_000), 1_250);
        }
    }

    #[test]
    fn latch_time_accepts_past_and_upcoming_qpc_anchors_within_100ms() {
        let freq = 10_000_000u64; // 10 MHz QPC = 10 ticks/us
        let now_us = 5_000_000u64;
        let period_ns = 8_333_333i64;
        // Past anchor (4 ms ago)
        assert_eq!(latch_time_us(now_us, 1_000_000, 960_000, freq, period_ns), Some((4_996_000, period_ns)));
        // Upcoming DWM qpcVBlank (4 ms in the future: latch > counter)
        assert_eq!(latch_time_us(now_us, 1_000_000, 1_040_000, freq, period_ns), Some((5_004_000, period_ns)));
        // More than 100 ms in the past or future is rejected
        assert_eq!(latch_time_us(now_us, 2_000_000, 999_990, freq, period_ns), None);
        assert_eq!(latch_time_us(now_us, 1_000_000, 2_000_010, freq, period_ns), None);
        assert_eq!(latch_time_us(now_us, 1_000_000, 1_000_000, 0, period_ns), None);
    }
}
