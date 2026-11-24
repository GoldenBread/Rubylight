//! Leaving hid-steam's lizard mode. The kernel's driver keeps a Steam Deck or a Steam Controller
//! in lizard mode (the controller's own mouse and keyboard emulation, with its gamepad input held
//! back from games) until Menu is held for about half a second, as on a real controller; Steam
//! takes a controller over by opening the driver's hidraw node instead. A virtual controller
//! nobody holds Menu on would never play without Steam, so the pad does what the player would:
//! it shows Menu held to the driver and, once the driver has switched, lets go. The report
//! stream pauses between the two, so the driver never sees Menu in a report after it switched
//! (it would show games a Menu press).

use std::time::{Duration, Instant};

/// How long the reports show Menu held, so the driver notes the press.
const SHOW: Duration = Duration::from_millis(120);
/// How long the pad waits, sending no report, for the driver (which switches 450 ms after the
/// first report with Menu held) to say it has: it then clears the controller's mappings.
const SWITCH: Duration = Duration::from_millis(900);

/// What the pad's reports do for the driver now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Phase {
    /// Nothing special: the player's own state.
    Idle,
    /// Menu is held in every report.
    Showing,
    /// No report is sent, until the driver switches or the wait ends.
    Silent,
}

pub(super) struct Lizard {
    /// Whether the pad leaves lizard mode at all: not for a controller Steam takes over.
    allowed: bool,
    /// Whether the driver is in lizard mode, as far as the pad knows.
    wanted: bool,
    since: Option<Instant>,
    /// Whether the driver turned lizard mode on since the hold began: a program that opened
    /// the driver's hidraw node meanwhile made it drop the controller and register it again,
    /// which wastes the hold.
    reenabled: bool,
    /// When the driver last switched, for [`Lizard::quiet`].
    switched: Option<Instant>,
}

impl Lizard {
    pub(super) fn new(allowed: bool) -> Self {
        Self {
            allowed,
            wanted: allowed,
            since: None,
            reenabled: false,
            switched: None,
        }
    }
    pub(super) fn phase(&self) -> Phase {
        match self.since.map(|since| since.elapsed()) {
            Some(elapsed) if elapsed < SHOW => Phase::Showing,
            Some(elapsed) if elapsed < SWITCH => Phase::Silent,
            _ => Phase::Idle,
        }
    }
    /// Whether the driver is switching, or just did: the haptic pulses it makes to tell the
    /// player are not a game's, and must not reach the client as rumble.
    pub(super) fn quiet(&self) -> bool {
        self.since.is_some()
            || self
                .switched
                .is_some_and(|at| at.elapsed() < Duration::from_millis(600))
    }
    /// The driver turned lizard mode on (it does as it registers the controller).
    pub(super) fn driver_enabled(&mut self) {
        self.wanted = self.allowed;
        self.reenabled = true;
    }
    /// The driver turned lizard mode off: the hold is done.
    pub(super) fn driver_disabled(&mut self) {
        if self.since.is_some() {
            tracing::info!("hid-steam left lizard mode: the gamepad's input reaches games now");
            self.switched = Some(Instant::now());
        }
        self.since = None;
        self.wanted = false;
    }
    /// Starts a hold when one is due (`ready`: the driver is up; `menu`: the player holds Menu,
    /// which the driver would count as the same press) and ends one that ran out. Returns the
    /// phase now.
    pub(super) fn update(&mut self, ready: bool, menu: bool) -> Phase {
        if self.since.is_some_and(|since| since.elapsed() >= SWITCH) {
            // The driver did not switch: it is held by a program of Steam's kind, which needs
            // no hold, unless it started over meanwhile. A report with Menu let go comes
            // first: the driver counts presses, and takes a new one only after that.
            tracing::info!(
                again = self.reenabled,
                "hid-steam did not leave lizard mode within the wait (another program has the pad open?)"
            );
            self.since = None;
            self.wanted = self.reenabled;
        } else if self.wanted && self.since.is_none() && ready && !menu {
            tracing::info!("holding Menu for hid-steam to leave lizard mode");
            self.since = Some(Instant::now());
            self.reenabled = false;
        }
        self.phase()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hold_shows_menu_then_falls_silent_until_the_driver_switches() {
        let mut lizard = Lizard::new(true);
        // Not before the driver is up, nor while the player holds Menu.
        assert_eq!(lizard.update(false, false), Phase::Idle);
        assert_eq!(lizard.update(true, true), Phase::Idle);
        assert_eq!(lizard.update(true, false), Phase::Showing);
        std::thread::sleep(SHOW + Duration::from_millis(20));
        assert_eq!(lizard.update(true, false), Phase::Silent);
        lizard.driver_disabled();
        assert_eq!(lizard.update(true, false), Phase::Idle, "done for good");
        // The driver turning lizard mode on again asks for another.
        lizard.driver_enabled();
        assert_eq!(lizard.update(true, false), Phase::Showing);
    }

    #[test]
    fn a_hold_the_driver_never_answers_is_tried_again_only_if_the_driver_started_over() {
        let mut lizard = Lizard::new(true);
        assert_eq!(lizard.update(true, false), Phase::Showing);
        std::thread::sleep(SWITCH + Duration::from_millis(20));
        // Nothing came: no more holds.
        assert_eq!(lizard.update(true, false), Phase::Idle);
        assert_eq!(lizard.update(true, false), Phase::Idle);
        // A driver that registers the controller again asks for one.
        lizard.driver_enabled();
        assert_eq!(lizard.update(true, false), Phase::Showing);
        // The driver registered it again during the hold, which wasted it: another follows,
        // after a report with Menu let go.
        lizard.driver_enabled();
        std::thread::sleep(SWITCH + Duration::from_millis(20));
        assert_eq!(lizard.update(true, false), Phase::Idle);
        assert_eq!(lizard.update(true, false), Phase::Showing);
    }

    #[test]
    fn a_pad_steam_takes_over_never_holds() {
        let mut lizard = Lizard::new(false);
        assert_eq!(lizard.update(true, false), Phase::Idle);
        lizard.driver_enabled();
        assert_eq!(lizard.update(true, false), Phase::Idle);
    }
}
