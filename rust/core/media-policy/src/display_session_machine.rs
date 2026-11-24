//! Pure display session lifecycle state machine (launch -> pause -> roam -> second-screen ->
//! crash-restore) without Windows or I/O dependencies.
use crate::display_policy::{Arrangement, HostDisplay, LayoutWatch, PrimaryChange, PrimaryWatch};
use crate::framegen::Rate;
use anyhow::Result;
use std::time::{Duration, Instant};

/// Stream role as needed by pure display session decisions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionRole {
    Stream,
    RemoteMonitor,
    InputOnly,
}

/// Stream transport as needed by pure display session decisions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionTransport {
    Moonlight,
    WebRtc,
}

/// Lifecycle phase of a prepared display session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionPhase {
    /// One or more active transports are attached to this display session.
    Active { streams: usize },
    /// No transport is attached; the display is retained across a pause/disconnect.
    Paused,
    /// The display session is undergoing recovery after display recreation while
    /// preserving the number of attached active transports (`streams`).
    Recovering { streams: usize },
    /// The display session has been released and its saved state restored.
    Restored,
}

impl SessionPhase {
    /// Number of currently attached active transports in this phase.
    pub fn active_streams(self) -> usize {
        match self {
            Self::Active { streams } | Self::Recovering { streams } => streams,
            Self::Paused | Self::Restored => 0,
        }
    }
}

/// Mode tuple `(width, height, refresh_millihz, hdr, vrr_low_latency)` for display matching.
pub type StreamDisplayMode = (u32, u32, u32, bool, bool);

/// Resolution, refresh in millihertz and HDR a stream asked its display for.
pub type RequestedMode = (Option<(u32, u32)>, Option<u32>, Option<bool>);

/// What deciding on a join needs of the stream ([`joined_display`]).
#[derive(Clone, Copy, Debug)]
pub struct JoinAsk<'a> {
    pub client: &'a str,
    pub role: SessionRole,
    /// The launch was a `/resume` or the prompt's Stream here too.
    pub resumes: bool,
    /// The app it launched.
    pub launched: Option<u32>,
    pub virtual_mode: bool,
    /// `virtual_display_mode`, and the identity that gives this stream's display.
    pub display_mode: &'a str,
    pub identity: &'a str,
    /// A press of Continue here is under way: the game's display goes with the
    /// previous owner, and the device that pressed has none yet.
    pub claim_live: bool,
    /// Whether `client` is a device's second screen (Dual screen mode).
    pub second_screen: bool,
}

/// What deciding on a join needs of the running game ([`joined_display`]).
#[derive(Clone, Copy, Debug)]
pub struct JoinedGame<'a> {
    pub id: u32,
    /// Where the running game currently lives.
    pub display: Option<&'a str>,
    /// A background game has a display per player.
    pub background: bool,
    pub desktop: bool,
}

/// The identity of the display a stream shares because it asked for the running
/// game: that game's, where it is. Asked for means `/resume` or the prompt's Stream
/// here too, never for Desktop: a plain second stream, of Desktop or a game, has
/// a display of its own. A device coming back to the display the game is on needs
/// no asking: its display can only be live if others stream it. Only under
/// `per_client` with the device's own identity: elsewhere its identity already says
/// which display it shares. Never for a device's second screen (Dual screen mode): it
/// resumes the game its first screen plays, and is there to show a second display.
pub fn joined_display(ask: &JoinAsk<'_>, game: Option<JoinedGame<'_>>) -> Option<String> {
    if ask.role != SessionRole::Stream
        || !ask.virtual_mode
        || ask.display_mode != "per_client"
        || ask.identity != ask.client
        || ask.claim_live
        || ask.second_screen
    {
        return None;
    }
    let game = game?;
    let display = game.display?;
    (!game.background
        && !game.desktop
        && ask.launched == Some(game.id)
        && (ask.resumes || display == ask.identity))
        .then(|| display.to_owned())
}

/// The PC monitor a device's second screen shows when its streams show physical
/// displays: the first of `active` (by display name, with whether it is one of
/// Rubylight's virtual displays) that is neither the monitor the first screen shows
/// (`first`) nor a virtual display. None when the PC has no other monitor: both
/// screens then show the same one.
pub fn pick_second_monitor<'a>(active: &[(&'a str, bool)], first: Option<&str>) -> Option<&'a str> {
    active
        .iter()
        .find(|(n, r)| !r && first.is_none_or(|f| !f.eq_ignore_ascii_case(n)))
        .map(|(n, _)| *n)
}

/// What setting up a stream may do to its display beyond the lease.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct DisplayChanges {
    /// Take the device's HDR profile.
    pub hdr_profile: bool,
    /// Set a virtual display's scale.
    pub scale: bool,
    /// Set the mode again after the layout.
    pub mode: bool,
}

/// One that joined a running game's display has it as it is: that display's HDR,
/// scale and mode stay the first stream's.
pub fn display_changes(joined: bool, hdr: bool, virtual_layout: bool) -> DisplayChanges {
    DisplayChanges {
        hdr_profile: hdr && !joined,
        scale: virtual_layout && !joined,
        mode: !joined,
    }
}

/// The HDR a joined stream wanted (`wanted`), when the display it shares has another.
pub fn joined_hdr_mismatch(
    joined: bool,
    wanted: Option<bool>,
    display: Option<bool>,
) -> Option<bool> {
    wanted.filter(|wanted| joined && display.is_some_and(|display| display != *wanted))
}

/// What a stream's display is asked for: its own mode and HDR, or for a stream
/// that joined (`joined`, the mode the display runs at) the display as it is. The
/// encoder scales from it, and its mode, HDR and scale stay the first stream's.
pub fn display_ask(
    joined: Option<(u32, u32, u32)>,
    own: (u32, u32),
    rate: Rate,
    hdr: Option<bool>,
) -> ((u32, u32), Rate, Option<bool>) {
    match joined {
        Some((width, height, refresh)) => ((width, height), Rate(refresh), None),
        None => (own, rate, hdr),
    }
}

/// The mode a joined stream checks its display against for the device's card: no
/// size, since the encoder scales, but the refresh and HDR it would have asked
/// for. A slower display or another HDR state is said out loud, and fails nothing.
pub fn joined_report(rate: Rate, hdr: Option<bool>) -> RequestedMode {
    (None, Some(rate.0), hdr)
}

/// The display the running game is on once a stream of it was set up (`own`: that
/// stream's virtual display, if any). It follows its owner's stream unless that
/// joined one. A display whose streams all left no longer holds it: Windows moved
/// the game to what is left, which the next stream's own display then is.
pub fn game_display_after(
    current: Option<&str>,
    current_live: bool,
    owns_game: bool,
    joined: bool,
    own: Option<&str>,
) -> Option<String> {
    let moved = owns_game || (current.is_some() && !current_live);
    if moved && !joined { own } else { current }.map(str::to_owned)
}

/// Whether `layout` turns off every monitor but the stream's, read as a stream's display setup
/// reads it: an unknown value falls back to exclusive for a virtual display and arranges nothing
/// for a physical one (`dd_configuration_option`).
pub fn turns_off_other_monitors(layout: &str, virtual_layout: bool) -> bool {
    if matches!(layout, "disabled" | "verify_only") {
        return false;
    }
    Arrangement::parse(layout).map_or(virtual_layout, |parsed| parsed == Arrangement::Exclusive)
}

/// Whether a WebRTC launch running on this PC runs [`Arrangement::Primary`] instead of `layout`.
/// Turning off monitors would switch off the browser showing the stream and its Disconnect button.
pub fn browser_keeps_monitors(
    transport: SessionTransport,
    browser_on_this_pc: bool,
    layout: &str,
    virtual_layout: bool,
) -> bool {
    transport == SessionTransport::WebRtc
        && browser_on_this_pc
        && turns_off_other_monitors(layout, virtual_layout)
}

/// Why this stream's layout is not the `configured` one, for the log.
pub fn layout_reason(monitors_kept: bool, configured: &str) -> Option<String> {
    monitors_kept.then(|| {
        format!("the browser runs on this PC, so its monitors stay on instead of `{configured}`")
    })
}

/// The device that keeps the layout this stream ends with, if any.
pub fn layout_owner(
    role: SessionRole,
    client_uuid: &str,
    learnable: bool,
    monitors_kept: bool,
    planned: Option<Arrangement>,
    in_force: Option<Arrangement>,
) -> Option<String> {
    (role == SessionRole::Stream && learnable && !monitors_kept && planned == in_force)
        .then(|| client_uuid.to_owned())
}

/// Both primary layouts make the stream's display the main screen.
pub fn primary_layout(layout: &str) -> bool {
    matches!(layout, "extended_primary" | "extended_primary_isolated")
}

/// Whether a monitor's return had displays put back since this stream last looked (`seen`).
pub fn wake_layout_passed(seen: &mut u64, now: u64) -> bool {
    std::mem::replace(seen, now) != now
}

/// Start the wait for Windows to settle when a display left another stream's layout
/// since this stream last looked (`seen`); true when it did.
pub fn notice_departure(
    seen: &mut u64,
    departures: u64,
    watch: &mut LayoutWatch,
    now: Instant,
) -> bool {
    if departures != *seen {
        *seen = departures;
        watch.display_left(now);
        return true;
    }
    false
}

/// Whether a display left that this stream has yet to lay the displays out again for. What is
/// on until then can be what Windows recalled.
pub fn departure_unsettled(seen: u64, departures: u64, watch: &LayoutWatch) -> bool {
    departures != seen || watch.departure_pending()
}

/// Whether a stream brings the game back after a display left. Not a remote monitor's,
/// whose display is someone else's to look at, nor a paused one's, which may hold its
/// layout but has nobody to show the game to.
pub fn brings_back_game(remote_monitor: bool, active: bool) -> bool {
    !remote_monitor && active
}

/// The HDR profile a kept display takes at its resume when the device's choice changed from
/// `launched` to `now` while it was paused: Some(the new one, or None to take it off), None
/// when nothing changes.
pub fn resumed_profile(
    applies: bool,
    launched: Option<&str>,
    now: Option<&str>,
) -> Option<Option<String>> {
    (applies && launched != now).then(|| now.map(str::to_owned))
}

/// Whether a primary restore may be looked at now.
pub fn primary_watch_applies(
    planned: Arrangement,
    lease_primary: bool,
    active: bool,
    secure: bool,
    due: bool,
) -> bool {
    planned.primary_beside_the_users() && lease_primary && active && !secure && due
}

/// Whether the layout put back for the primary moved it to the stream's display.
pub fn primary_taken_back<E>(shown: &std::result::Result<Option<bool>, E>) -> bool {
    matches!(shown, Ok(Some(true)))
}

/// Whether a change arms the primary watch: only when the last look found the stream's
/// display the primary one.
pub fn primary_change_arms(last_shown: Option<bool>, change: Option<PrimaryChange>) -> bool {
    change.is_some() && last_shown == Some(true)
}

/// The change to arm at a resume: one that arrived while the stream was paused, else the
/// one now.
pub fn resumed_primary_change(
    paused: Option<PrimaryChange>,
    change: Option<PrimaryChange>,
) -> Option<PrimaryChange> {
    paused.or(change)
}

/// Whether a refused layout attempt should be retried once after a pause.
pub fn should_retry_desktop_refused(refused: bool, secure: bool) -> bool {
    refused && !secure
}

/// Whether a retained display can be resumed for `stream_mode`: same size, HDR and VRR.
/// Refresh differences are allowed because an owned virtual display can be retimed.
pub fn mode_resumes(held: StreamDisplayMode, requested: StreamDisplayMode) -> bool {
    (held.0, held.1, held.3, held.4) == (requested.0, requested.1, requested.3, requested.4)
}

/// Whether a retained display still serves a launch with `requested` display and `layout`.
pub fn display_serves(
    held_client_display: Option<HostDisplay>,
    held_virtual_layout: Option<&str>,
    requested: Option<HostDisplay>,
    layout: &str,
) -> bool {
    HostDisplay::reusable(held_client_display, requested)
        && held_virtual_layout.is_none_or(|old| old == layout)
}

/// Whether a retained display shows `game`, the display a launch asking for the running
/// game would join.
pub fn display_shows_game(held_identity: Option<&str>, game: Option<&str>) -> bool {
    game.is_none_or(|game| held_identity.is_none_or(|own| own == game))
}

/// A display recovery's steps after the heartbeat brought the display back.
/// The layout and HDR profile are retried each second until they apply. The
/// stream's mode is set once per recovery, after the layout: a mode Windows
/// refuses is reported on the stream card, not retried against Windows.
#[derive(Clone, Debug)]
pub struct RecoveryState {
    pub pending: bool,
    due: Instant,
    mode: bool,
}

impl RecoveryState {
    pub fn new() -> Self {
        Self::at(Instant::now())
    }

    pub fn at(now: Instant) -> Self {
        Self {
            pending: false,
            due: now,
            mode: false,
        }
    }

    pub fn start(&mut self, now: Instant) {
        self.pending = true;
        self.due = now;
        self.mode = true;
    }

    /// Whether to run the steps now; a failed attempt waits a second.
    pub fn due(&mut self, now: Instant) -> bool {
        if self.pending && now >= self.due {
            self.due = now + Duration::from_secs(1);
            return true;
        }
        false
    }

    pub fn take_mode(&mut self) -> bool {
        std::mem::take(&mut self.mode)
    }

    pub fn finish(&mut self) {
        self.pending = false;
    }
}

impl Default for RecoveryState {
    fn default() -> Self {
        Self::new()
    }
}

/// Streams share the saved layout's restoration, like the arrangement and
/// display settings leases: the first records it for crash recovery and the
/// last restores it. One client's end must not switch off another client's
/// display or reset its mode and HDR, nor clear the journal under it.
#[derive(Debug)]
pub struct BaselineJournal<T> {
    users: usize,
    held: Option<T>,
}

impl<T> BaselineJournal<T> {
    pub const fn new() -> Self {
        Self {
            users: 0,
            held: None,
        }
    }

    /// Number of active sessions sharing the recorded crash-restore baseline.
    pub fn users(&self) -> usize {
        self.users
    }

    /// The first user records the layout; later users join it.
    pub fn acquire(&mut self, record: impl FnOnce() -> Result<T>) -> Result<()> {
        if self.users == 0 {
            self.held = Some(record()?);
        }
        self.users += 1;
        Ok(())
    }

    /// Only the last user restores the layout.
    pub fn release(&mut self, restore: impl FnOnce(T)) {
        self.users -= 1;
        if self.users == 0
            && let Some(held) = self.held.take()
        {
            restore(held);
        }
    }
}

impl<T> Default for BaselineJournal<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Effect of detaching a transport from a display session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DetachEffect {
    /// True only when the last active stream disconnects (`1 -> 0`), so chosen
    /// layout/resolution/scale are captured once at disconnect and never while paused.
    pub capture_chosen_preferences: bool,
    pub phase: SessionPhase,
}

/// What one paused heartbeat tick should do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PausedTickAction {
    /// Renew driver lease only; leave local Win+P changes untouched.
    KeepAliveOnly,
    /// Renew driver lease and guard exclusive layout while paused.
    KeepAliveAndHoldLayout,
}

/// Pure display session state machine tracking a prepared display across
/// launch, active streaming, pause/resume, roam/departure, second-screen
/// isolation, and crash-restore recovery.
#[derive(Debug)]
pub struct DisplaySessionMachine {
    pub phase: SessionPhase,
    pub recovery: RecoveryState,
    pub layout_watch: LayoutWatch,
    pub primary_watch: PrimaryWatch,
    /// Monotonic departure epoch when this stream last looked.
    pub departures: u64,
    /// Monotonic monitor-wake layout epoch when this stream last looked.
    pub wake_layouts: u64,
    /// Console session ID at the last layout check.
    pub console_session: Option<u32>,
    /// Whether the secure desktop showed at the last layout check.
    pub secure_desktop: bool,
    /// Active display count at the last primary check.
    pub displays_seen: Option<usize>,
    /// Whether the last primary look found this stream's display primary.
    pub shown_primary: Option<bool>,
    /// A change that arrived while the stream was paused, to arm at resume.
    pub paused_change: Option<PrimaryChange>,
    /// True if start applied the layout but Windows kept the primary elsewhere.
    pub start_primary_mismatch: bool,
    /// Keep switching off physical monitors of an exclusive layout while paused.
    pub hold_layout_while_paused: bool,
}

impl DisplaySessionMachine {
    /// Create a freshly launched display session machine at `now`.
    pub fn launch(
        now: Instant,
        departures: u64,
        wake_layouts: u64,
        start_primary_mismatch: bool,
        hold_layout_while_paused: bool,
    ) -> Self {
        Self {
            phase: SessionPhase::Active { streams: 1 },
            recovery: RecoveryState::at(now),
            layout_watch: LayoutWatch::default(),
            primary_watch: PrimaryWatch::starting(now),
            departures,
            wake_layouts,
            console_session: None,
            secure_desktop: false,
            displays_seen: None,
            shown_primary: None,
            paused_change: None,
            start_primary_mismatch,
            hold_layout_while_paused,
        }
    }

    /// Create an idle/test machine with default watches and zeroed epochs.
    pub fn bare(now: Instant) -> Self {
        Self {
            phase: SessionPhase::Active { streams: 1 },
            recovery: RecoveryState::at(now),
            layout_watch: LayoutWatch::default(),
            primary_watch: PrimaryWatch::default(),
            departures: 0,
            wake_layouts: 0,
            console_session: None,
            secure_desktop: false,
            displays_seen: None,
            shown_primary: None,
            paused_change: None,
            start_primary_mismatch: false,
            hold_layout_while_paused: false,
        }
    }

    /// Record another active stream attaching (resume or concurrent join).
    pub fn attach_stream(&mut self) -> SessionPhase {
        let streams = match self.phase {
            SessionPhase::Active { streams } | SessionPhase::Recovering { streams } => {
                streams.saturating_add(1)
            }
            SessionPhase::Paused | SessionPhase::Restored => 1,
        };
        self.phase = if self.recovery.pending {
            SessionPhase::Recovering { streams }
        } else {
            SessionPhase::Active { streams }
        };
        self.phase
    }

    /// Record a stream detaching; only the `1 -> 0` transition pauses and triggers
    /// preference capture.
    pub fn detach_stream(&mut self) -> DetachEffect {
        match self.phase {
            SessionPhase::Active { streams } if streams > 1 => {
                self.phase = SessionPhase::Active {
                    streams: streams - 1,
                };
                DetachEffect {
                    capture_chosen_preferences: false,
                    phase: self.phase,
                }
            }
            SessionPhase::Recovering { streams } if streams > 1 => {
                self.phase = SessionPhase::Recovering {
                    streams: streams - 1,
                };
                DetachEffect {
                    capture_chosen_preferences: false,
                    phase: self.phase,
                }
            }
            SessionPhase::Active { streams } | SessionPhase::Recovering { streams } => {
                self.phase = SessionPhase::Paused;
                DetachEffect {
                    capture_chosen_preferences: streams == 1,
                    phase: self.phase,
                }
            }
            SessionPhase::Paused | SessionPhase::Restored => DetachEffect {
                capture_chosen_preferences: false,
                phase: self.phase,
            },
        }
    }

    /// Decide what a paused heartbeat tick should do.
    pub fn paused_tick(&self) -> PausedTickAction {
        if self.hold_layout_while_paused {
            PausedTickAction::KeepAliveAndHoldLayout
        } else {
            PausedTickAction::KeepAliveOnly
        }
    }

    /// Start recovery after the display or retained display generation changed.
    pub fn begin_recovery(&mut self, now: Instant) {
        self.recovery.start(now);
        match self.phase {
            SessionPhase::Active { streams } => {
                self.phase = SessionPhase::Recovering { streams };
            }
            SessionPhase::Recovering { .. } | SessionPhase::Paused | SessionPhase::Restored => {}
        }
    }

    /// Mark recovery complete once scale, layout, mode and HDR profile succeeded.
    /// Restores [`SessionPhase::Active`] with the preserved stream count when
    /// active streams are attached, or remains [`SessionPhase::Paused`] when none are.
    pub fn finish_recovery(&mut self) {
        self.recovery.finish();
        match self.phase {
            SessionPhase::Recovering { streams } if streams > 0 => {
                self.phase = SessionPhase::Active { streams };
            }
            SessionPhase::Recovering { .. } => {
                self.phase = SessionPhase::Paused;
            }
            SessionPhase::Active { .. } | SessionPhase::Paused | SessionPhase::Restored => {}
        }
    }

    /// Mark the session permanently restored/torn down (crash-restore or final drop).
    pub fn mark_restored(&mut self) {
        self.recovery.finish();
        self.phase = SessionPhase::Restored;
    }

    /// Observe whether another stream's display departed since last check.
    pub fn notice_departure(&mut self, departures: u64, now: Instant) -> bool {
        notice_departure(
            &mut self.departures,
            departures,
            &mut self.layout_watch,
            now,
        )
    }

    /// Whether a departure has not yet been repaired for this session.
    pub fn departure_unsettled(&self, departures: u64) -> bool {
        departure_unsettled(self.departures, departures, &self.layout_watch)
    }

    /// Observe whether a monitor wake re-layout happened since last check.
    pub fn notice_wake_layout(&mut self, wake_layouts: u64) -> bool {
        wake_layout_passed(&mut self.wake_layouts, wake_layouts)
    }

    /// Observe secure desktop / console session transitions and classify any
    /// resulting Windows rearrangement for the primary watch.
    pub fn observe_desktop_change(
        &mut self,
        secure: bool,
        console_session: u32,
        departed: bool,
        woke: bool,
        now: Instant,
    ) -> Option<PrimaryChange> {
        let session_changed = self
            .console_session
            .replace(console_session)
            .is_some_and(|last| last != console_session);
        if secure || session_changed {
            self.layout_watch.excuse(now);
        }
        let returned_from_secure = std::mem::replace(&mut self.secure_desktop, secure) && !secure;
        if departed || woke {
            Some(PrimaryChange::Display)
        } else if returned_from_secure {
            Some(PrimaryChange::Secure)
        } else if session_changed {
            Some(PrimaryChange::Session)
        } else {
            None
        }
    }

    /// Update primary-watch state across pause, resume, and secure desktop before
    /// querying Windows displays; returns true when the caller should probe displays.
    pub fn prepare_primary_check(
        &mut self,
        now: Instant,
        change: Option<PrimaryChange>,
        active: bool,
        secure: bool,
        planned: Arrangement,
        lease_primary: bool,
    ) -> bool {
        if !planned.primary_beside_the_users() || !lease_primary {
            return false;
        }
        if !active {
            self.primary_watch.interrupted();
            if change.is_some() {
                self.paused_change = change;
            }
            return false;
        }
        let change = resumed_primary_change(self.paused_change.take(), change);
        if primary_change_arms(self.shown_primary, change)
            && let Some(change) = change
        {
            self.primary_watch.changed(change, now);
        }
        if secure {
            self.primary_watch.interrupted();
        }
        primary_watch_applies(
            planned,
            lease_primary,
            active,
            secure,
            self.primary_watch.due(now),
        )
    }

    /// Record an active display count sample; arms the primary watch if the count
    /// changed while this stream's display held the primary.
    pub fn observe_display_count(&mut self, count: usize, now: Instant) -> bool {
        let changed = self
            .displays_seen
            .replace(count)
            .is_some_and(|seen| seen != count);
        if changed && primary_change_arms(self.shown_primary, Some(PrimaryChange::Display)) {
            self.primary_watch.changed(PrimaryChange::Display, now);
        }
        changed
    }

    /// Consume the start primary mismatch flag when the primary is back.
    pub fn take_start_primary_mismatch(&mut self) -> bool {
        std::mem::replace(&mut self.start_primary_mismatch, false)
    }
}

#[cfg(test)]
mod tests;
