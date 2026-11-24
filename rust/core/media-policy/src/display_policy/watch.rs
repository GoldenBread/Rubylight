//! Heartbeat layout and primary-display watchers and error backoff throttling.

use super::Arrangement;

/// When the heartbeat checks [`super::switched_back_on`] and puts the stream's layout
/// back. A display must stay on for [`LayoutWatch::SETTLE`] first, so a game's
/// own mode switch passes without a fight; a failed attempt (the secure
/// desktop refuses display changes) waits [`LayoutWatch::RETRY`]. A monitor
/// that comes back [`LayoutWatch::RETURNS`] times within
/// [`LayoutWatch::RETURN_WINDOW`] is left on for the rest of the stream
/// (issue #56: a monitor that drops its HDMI link in standby reappears every
/// 30-60 s, and each switch-off blanked the stream); the others are still
/// switched off. Windows recalling its layout after the lock screen, a UAC
/// prompt or a session switch is not the monitor coming back.
#[derive(Debug, Default)]
pub struct LayoutWatch {
    next_check: Option<std::time::Instant>,
    since: Option<std::time::Instant>,
    /// Each display the layout was put back for, and when.
    returns: Vec<(String, std::time::Instant)>,
    /// Displays that kept coming back: left on for the rest of the stream.
    kept: Vec<String>,
    /// Until when a reapply follows the secure desktop or a session switch.
    excused_until: Option<std::time::Instant>,
    /// When to lay the displays out again after another stream's display left.
    departure: Option<std::time::Instant>,
}
/// What [`LayoutWatch::observe`] decides.
#[derive(Debug, PartialEq, Eq)]
pub enum LayoutAction {
    Wait,
    /// Put the layout back for `displays`, leaving [`LayoutWatch::kept`] on;
    /// `kept_now` are the displays this check found to keep coming back.
    Reapply {
        displays: Vec<String>,
        kept_now: Vec<String>,
    },
}
impl LayoutWatch {
    pub const INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);
    pub const SETTLE: std::time::Duration = std::time::Duration::from_millis(500);
    pub const RETRY: std::time::Duration = std::time::Duration::from_secs(2);
    pub const RETURNS: usize = 3;
    pub const RETURN_WINDOW: std::time::Duration = std::time::Duration::from_secs(300);
    /// How long after the secure desktop or a session switch a return is excused.
    pub const EXCUSE: std::time::Duration = std::time::Duration::from_secs(30);
    /// How long Windows gets to finish rearranging after a display is removed.
    pub const DEPARTURE_SETTLE: std::time::Duration = std::time::Duration::from_millis(1500);
    /// Whether to query the displays now.
    pub fn due(&mut self, now: std::time::Instant) -> bool {
        if self.next_check.is_some_and(|next| now < next) {
            return false;
        }
        self.next_check = Some(now + Self::INTERVAL);
        true
    }
    /// Another stream's display left. Windows rearranges what remains once it
    /// is gone: it can recall a layout with the physical monitors on, or make
    /// the remaining virtual display primary, and neither is this stream's.
    pub fn display_left(&mut self, now: std::time::Instant) {
        self.departure = Some(now + Self::DEPARTURE_SETTLE);
    }
    /// Whether to lay the displays out again for a display that left.
    pub fn departure_due(&self, now: std::time::Instant) -> bool {
        self.departure.is_some_and(|at| now >= at)
    }
    /// Whether a display left and the displays are yet to be laid out again for it.
    pub fn departure_pending(&self) -> bool {
        self.departure.is_some()
    }
    /// The displays were laid out again; a failed attempt keeps it due.
    pub fn departure_handled(&mut self) {
        self.departure = None;
    }
    /// The displays left on for the rest of the stream.
    pub fn kept(&self) -> &[String] {
        &self.kept
    }
    /// Record a check of the displays that came back on.
    pub fn observe(&mut self, switched_on: &[String], now: std::time::Instant) -> LayoutAction {
        let displays: Vec<String> = switched_on
            .iter()
            .filter(|display| !self.kept.contains(display))
            .cloned()
            .collect();
        if displays.is_empty() {
            self.since = None;
            return LayoutAction::Wait;
        }
        let since = *self.since.get_or_insert(now);
        if now.duration_since(since) < Self::SETTLE {
            return LayoutAction::Wait;
        }
        self.since = None;
        self.returns
            .retain(|(_, at)| now.duration_since(*at) < Self::RETURN_WINDOW);
        let excused = self.excused(now);
        let (kept_now, displays): (Vec<String>, Vec<String>) =
            displays.into_iter().partition(|display| {
                !excused
                    && self.returns.iter().filter(|(d, _)| d == display).count() + 1
                        >= Self::RETURNS
            });
        self.kept.extend(kept_now.iter().cloned());
        LayoutAction::Reapply { displays, kept_now }
    }
    /// The layout was put back for `displays`; a refused attempt, or one after
    /// the secure desktop or a session switch, is not a return.
    pub fn reapplied(&mut self, displays: &[String], now: std::time::Instant) {
        if self.excused(now) {
            return;
        }
        self.returns
            .extend(displays.iter().map(|display| (display.clone(), now)));
    }
    pub fn failed(&mut self, now: std::time::Instant) {
        self.since = None;
        self.next_check = Some(now + Self::RETRY);
        self.excuse(now);
    }
    /// The secure desktop is shown or the console session changed: Windows
    /// recalls its own layout afterwards.
    pub fn excuse(&mut self, now: std::time::Instant) {
        self.excused_until = Some(now + Self::EXCUSE);
    }
    pub(super) fn excused(&self, now: std::time::Instant) -> bool {
        self.excused_until.is_some_and(|until| now < until)
    }
}
/// Whether a layout's promised primary is gone: one of the layouts that make a stream's
/// display the primary while the user's displays stay on, and Windows shows that display
/// on but not as the primary (`stream_primary` is false). Windows or a game moving the
/// primary back to a monitor is not the user's choice, and the setting promises games
/// open on the stream (issue #33: a stream that learned the move kept it for the device).
/// None is the stream's display off or Windows still laying it out, which the display
/// recovery and the layout watch handle; there is nothing to put back yet.
pub fn primary_moved(planned: Arrangement, stream_primary: Option<bool>) -> bool {
    stream_primary == Some(false) && planned.primary_beside_the_users()
}

/// What [`PrimaryWatch::observe`] decides.
#[derive(Debug, PartialEq, Eq)]
pub enum PrimaryAction {
    Wait,
    Restore,
}

/// What changed the displays under the stream, and whether a restore for it is the display
/// fighting back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrimaryChange {
    /// The stream just started: Windows took the layout as it began.
    Start,
    /// A display arrived or left: Windows rearranged the desktop.
    Display,
    /// The console session changed.
    Session,
    /// The secure desktop went away.
    Secure,
}

/// Watches the primary a primary layout made, as [`LayoutWatch`] watches the displays a
/// layout switched off. Windows changes the layout by itself when a display arrives or
/// leaves, when the session changes or the stream starts, and can leave the primary on a
/// monitor; only such a change starts the watch ([`PrimaryWatch::changed`]), and its window
/// ends a move that does not come. A look that sees the display still primary only clears a
/// pending settle, so a move later in the window is still Windows'; once the window is over,
/// a primary picked is the user's or the game's and is left alone (issue #33: a stream that
/// learned such a move kept it for the device). A move must hold for [`LayoutWatch::SETTLE`]
/// before the layout is put back, a restore Windows refused or one that did not move the
/// primary waits [`LayoutWatch::RETRY`], and one arm tries at most [`PrimaryWatch::ATTEMPTS`]
/// restores, so a change Windows keeps refusing cannot lay the display out for the rest of
/// the stream. A display that fights back stops the watch for the rest of the stream after
/// [`PrimaryWatch::RETURNS`] restores or refusals that followed a previous one quickly
/// ([`PrimaryWatch::RETURN_WINDOW`]); the start's restore and the ones after the lock screen
/// or a session switch are one-offs and do not count, as a monitor that keeps coming back on
/// is left on ([`LayoutWatch`], issue #56).
#[derive(Debug, Default)]
pub struct PrimaryWatch {
    since: Option<std::time::Instant>,
    next_check: Option<std::time::Instant>,
    /// Until when a change Windows made, or the stream's start, explains a primary move.
    armed_until: Option<std::time::Instant>,
    /// Why the watch is armed, until a restore or the window ends.
    change: Option<PrimaryChange>,
    /// When that change happened, for the log that tunes [`Self::WINDOW`].
    change_at: Option<std::time::Instant>,
    change_logged: bool,
    /// Restores this arm has tried, for [`Self::ATTEMPTS`].
    attempts: usize,
    /// This arm's window was extended once for its retry.
    retried: bool,
    /// The last restore or refusal a display change caused, to tell fighting back from a
    /// one-off.
    last_attempt: Option<std::time::Instant>,
    /// Restores or refusals that followed a previous one quickly, for [`Self::RETURNS`].
    restores: Vec<std::time::Instant>,
    /// The last look decided to put the primary back: the next [`PrimaryWatch::restored`]
    /// counts it. A look that simply finds a display off or the primary elsewhere does not.
    attempt: bool,
    /// Windows kept taking the primary back: this stream leaves it where Windows puts it.
    stopped: bool,
    /// This stream just started: its first look decides, without the event window, which can
    /// be older than the first heartbeat.
    starting: bool,
}
impl PrimaryWatch {
    /// Restores and refusals within [`Self::RETURN_WINDOW`] after which the watch stops for
    /// the stream.
    pub const RETURNS: usize = LayoutWatch::RETURNS;
    pub const RETURN_WINDOW: std::time::Duration = LayoutWatch::RETURN_WINDOW;
    /// Restores one arm may try: the first, and two retries after Windows refused one. The
    /// start, a session change and the lock screen's return are one-offs that never count
    /// towards [`Self::RETURNS`], so without this limit a refused one would lay the stream's
    /// display out every [`Self::WINDOW`] or so for the whole stream.
    pub const ATTEMPTS: usize = 3;
    /// How long a display arriving or leaving, a session change or the stream's start still
    /// explains a primary move. Windows settles its rearrangement within a couple of seconds;
    /// a look that still sees the display primary does not end the window, so a move that
    /// follows it is still caught.
    pub const WINDOW: std::time::Duration = std::time::Duration::from_secs(5);
    /// Whether to look at the displays now: a refused restore waits before its next try.
    pub fn due(&self, now: std::time::Instant) -> bool {
        self.next_check.is_none_or(|next| now >= next)
    }
    /// A watch for a stream that is starting: Windows has just been given the layout, which is
    /// a change of its own, so a primary it did not take is put back (issue #33: the reporter's
    /// stream opened with the virtual display on but not the main screen). Its first look
    /// decides, so a slow first heartbeat does not lose the start.
    pub fn starting(now: std::time::Instant) -> Self {
        let mut watch = Self::default();
        watch.arm(PrimaryChange::Start, now);
        watch.starting = true;
        watch
    }
    /// Whether Windows kept taking the primary back and this stream leaves it there.
    pub fn stopped(&self) -> bool {
        self.stopped
    }
    /// A display arrived or left, Windows changed the session, or the secure desktop went
    /// away: for [`Self::WINDOW`] a primary move is Windows rearranging the desktop, not the
    /// user's choice.
    pub fn changed(&mut self, change: PrimaryChange, now: std::time::Instant) {
        self.arm(change, now);
    }
    fn arm(&mut self, change: PrimaryChange, now: std::time::Instant) {
        self.armed_until = Some(now + Self::WINDOW);
        self.change = Some(change);
        self.change_at = Some(now);
        self.change_logged = false;
        self.attempts = 0;
        self.retried = false;
    }
    /// The look was interrupted, by a pause or the secure desktop: a move it first saw must
    /// start its settle again after the resume or unlock, instead of restoring at once
    /// because the wait was long.
    pub fn interrupted(&mut self) {
        self.since = None;
    }
    /// The window a change opened is over, or nothing armed the watch: a primary picked now
    /// is the user's or a game's.
    fn expire(&mut self, now: std::time::Instant) {
        if self.armed_until.is_some_and(|until| now >= until) {
            self.armed_until = None;
            self.change = None;
            self.change_at = None;
        }
    }
    /// The stream's display is still the primary one, or has it back: nothing waits for a
    /// restore any more, but the change's window stays open for a move that follows it.
    pub fn settled(&mut self, now: std::time::Instant) {
        self.since = None;
        self.attempt = false;
        self.starting = false;
        self.expire(now);
    }
    /// The stream's display has the primary one again, or the watch is done: disarmed.
    pub fn disarm(&mut self) {
        self.since = None;
        self.armed_until = None;
        self.change = None;
        self.change_at = None;
        self.attempt = false;
        self.starting = false;
    }
    /// Record what this look found: the stream's display is on but not the primary one
    /// (`demoted`), or it is.
    pub fn observe(&mut self, demoted: bool, now: std::time::Instant) -> PrimaryAction {
        let starting = std::mem::replace(&mut self.starting, false);
        if !demoted || self.stopped {
            self.since = None;
            self.expire(now);
            return PrimaryAction::Wait;
        }
        if starting {
            // The first look after a start: a demotion now is the start's, however long the
            // stream took to get here.
            self.arm(PrimaryChange::Start, now);
        } else if self.since.is_none() && self.armed_until.is_none_or(|until| now >= until) {
            // The first look at this move, and nothing changed the displays: the primary is
            // where the user or a game put it. A move already seen while armed keeps its
            // settle even when the window ends meanwhile.
            self.expire(now);
            self.since = None;
            return PrimaryAction::Wait;
        }
        let since = *self.since.get_or_insert(now);
        if now.duration_since(since) < LayoutWatch::SETTLE {
            return PrimaryAction::Wait;
        }
        self.since = None;
        self.attempt = true;
        self.attempts += 1;
        PrimaryAction::Restore
    }
    /// The stream's display has the primary one again: the watch disarms, and true when the
    /// restores and refusals reached [`Self::RETURNS`] within [`Self::RETURN_WINDOW`] and the
    /// watch stops for the stream.
    pub fn restored(&mut self, now: std::time::Instant) -> bool {
        self.since = None;
        self.starting = false;
        // A look that found nothing to restore is not a restore: only a layout put back for
        // the primary counts, or every quiet heartbeat would reach the limit.
        if !std::mem::replace(&mut self.attempt, false) {
            return self.stopped;
        }
        let change = self.change;
        let stopped = self.count(change, now);
        self.armed_until = None;
        self.change = None;
        self.change_at = None;
        stopped
    }
    /// Windows did not take the layout back, or accepted it without moving the primary: wait
    /// [`LayoutWatch::RETRY`] before asking again, and count the refusal the way a restore is
    /// counted: true when the watch stops for the rest of the stream. This arm's window is
    /// extended once so a retry has its settle, and the arm stops after [`Self::ATTEMPTS`]
    /// restores whatever [`Self::RETURNS`] would say, as a start or session change never
    /// reaches it.
    pub fn failed(&mut self, now: std::time::Instant) -> bool {
        self.since = None;
        self.attempt = false;
        self.starting = false;
        self.next_check = Some(now + LayoutWatch::RETRY);
        // Only the first refusal extends the window: a refusal that extended it every time
        // would keep laying the display out for the whole stream.
        if !self.retried {
            self.retried = true;
            if let Some(until) = self.armed_until {
                self.armed_until = Some(until.max(now + LayoutWatch::RETRY + Self::WINDOW));
            }
        }
        let counted = self.count(self.change, now);
        if self.attempts >= Self::ATTEMPTS {
            self.stopped = true;
        }
        counted || self.stopped
    }
    /// The time from the change this arm came from to now, once per change, for the log that
    /// tunes [`Self::WINDOW`] on a real rig.
    pub fn change_delay(&mut self, now: std::time::Instant) -> Option<std::time::Duration> {
        let at = self.change_at?;
        if self.change_logged {
            return None;
        }
        self.change_logged = true;
        Some(now.duration_since(at))
    }
    /// One restore or refusal for [`Self::RETURNS`]; only a display change that follows on a
    /// previous one quickly counts, so the start and the lock screen's own rearranging do not
    /// end the watch early. True when the watch stops.
    fn count(&mut self, change: Option<PrimaryChange>, now: std::time::Instant) -> bool {
        if self.stopped {
            return true;
        }
        if change != Some(PrimaryChange::Display) {
            // The start and the session's own rearrangements are one-offs, not the display
            // fighting back.
            return false;
        }
        let quick = self
            .last_attempt
            .is_some_and(|at| now.duration_since(at) < Self::RETURN_WINDOW);
        self.last_attempt = Some(now);
        if !quick {
            return false;
        }
        self.restores
            .retain(|at| now.duration_since(*at) < Self::RETURN_WINDOW);
        self.restores.push(now);
        self.stopped = self.restores.len() >= Self::RETURNS;
        self.stopped
    }
}

/// Deduplicates repeated display heartbeat or recovery errors and backs off
/// retries exponentially while an identical failure persists.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HeartbeatThrottle {
    last_error: Option<String>,
    failures: u32,
}

impl HeartbeatThrottle {
    /// Record a successful attempt. Returns `true` when recovering from an error.
    pub fn note_ok(&mut self) -> bool {
        self.failures = 0;
        self.last_error.take().is_some()
    }

    /// Record a failure. Returns `true` when this error is new and should warn,
    /// resetting backoff so a new failure cause retries without delay.
    pub fn note_error(&mut self, error: &str) -> bool {
        if self.last_error.as_deref() == Some(error) {
            self.failures = self.failures.saturating_add(1);
            false
        } else {
            self.last_error = Some(error.to_owned());
            self.failures = 1;
            true
        }
    }

    /// Interval before the next attempt, doubling on repeated identical failures
    /// up to `max_backoff`.
    pub fn next_tick(
        &self,
        base: std::time::Duration,
        max_backoff: std::time::Duration,
    ) -> std::time::Duration {
        if self.failures <= 1 {
            return base;
        }
        let shift = (self.failures - 1).min(16);
        base.saturating_mul(1u32 << shift)
            .min(max_backoff)
            .max(base)
    }
}
