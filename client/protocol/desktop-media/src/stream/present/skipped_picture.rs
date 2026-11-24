//! Refused-present hold adaptation (`SkippedPicture`) for no-wait video presentation.

/// Refused presents in a row before one waits after all. The display takes a picture every refresh,
/// so this many means the GPU is behind, and only a waiting present holds its work back.
pub(in crate::stream::present) const REFUSED_IN_A_ROW: u32 = 4;

/// Refused presents among the last 64 that say the hold is too short here. A picture every refresh
/// is refused rarely, where one is late (0.5 to 1%). Too short a hold refuses far more: a stream
/// arriving later than the hold but before the desktop's deadline loses every other picture.
pub(in crate::stream::present) const REFUSED_OF_64_WIDENS: u32 = 8;
/// How much longer the hold gets each time.
pub(in crate::stream::present) const HOLD_STEP_US: u64 = 500;
/// Pictures between looks at a hold that grew. A bad spell on the network passes, and half a minute
/// apart the cost on a display that needs the longer hold is small.
pub(in crate::stream::present) const PICTURES_SHORTEN: u32 = 4096;
/// Refused presents among those that let it get a step shorter: late pictures now and then (0.5 to
/// 1.6%) must not keep it long for good, where too short a hold refuses far more.
const REFUSED_SHORTEN: u32 = 64;

/// Refreshes that showed a picture again, among `MISSED_OF` pictures, before each picture waits its
/// turn (`SkippedPicture::missed`): 1.5%, two windows in a row. Once settled, a window at 120 Hz
/// missed at most 5 of 1024, and at 60 Hz 8, where borderless at 120 Hz missed 15 to 37. A spell on
/// Wi-Fi reached 17 and passed. The first `MISSED_OF` pictures don't count: everywhere they missed 11
/// to 23, and most of a run's misses are there.
pub(in crate::stream::present) const MISSED_WAIT: u32 = 15;
pub(in crate::stream::present) const MISSED_OF: u32 = 1024;

/// A stream faster than the display is refused by design: to wait there is the queue again.
fn a_picture_a_refresh(frame_us: u64, refresh_us: u64) -> bool {
    frame_us * 10 >= refresh_us * 9
}

/// A picture skipped for one that was still waiting. The next picture takes its place, but a still
/// desktop may send none: then this one is drawn after all, a little later than the next would
/// have come, or the screen would stay a picture behind.
#[derive(Debug)]
pub struct SkippedPicture {
    pub(in crate::stream::present) due_us: Option<u64>,
    in_a_row: u32,
    /// How long a refused present tries again.
    pub(in crate::stream::present) hold_us: u64,
    /// What the hold starts at, and comes back to.
    base_us: u64,
    /// One bit for each of the last pictures, set where it was refused.
    pub(in crate::stream::present) recent: u64,
    /// Pictures since the last look at the hold, and how many of them were refused.
    pub(in crate::stream::present) pictures: u32,
    pub(in crate::stream::present) refused: u32,
    /// Pictures toward `MISSED_OF`, and the misses among them.
    missed_of: u32,
    pub(in crate::stream::present) missed: u32,
    /// Whether the last window reached `MISSED_WAIT`.
    missed_often: bool,
    /// Pictures until misses count, on a new stream or display.
    pub(in crate::stream::present) settling: u32,
    /// Refusals kept coming even once the hold passed a whole refresh: wait for each picture's
    /// turn (`SDL_RenderPresent`) until a clean window lets no-wait presents try again.
    waiting: bool,
}

impl SkippedPicture {
    pub fn new(hold: std::time::Duration) -> Self {
        let hold_us = hold.as_micros() as u64;
        Self { due_us: None, in_a_row: 0, hold_us, base_us: hold_us, recent: 0, pictures: 0, refused: 0, missed_of: 0, missed: 0, missed_often: false, settling: MISSED_OF, waiting: false }
    }

    /// Another display or stream rate: what the hold grew to says nothing about its deadline.
    pub fn restart(&mut self) {
        self.hold_us = self.base_us;
        self.recent = 0;
        self.pictures = 0;
        self.refused = 0;
        self.missed_of = 0;
        self.missed = 0;
        self.missed_often = false;
        self.settling = MISSED_OF;
        self.waiting = false;
    }

    /// A refresh showed the last picture again although this one was on its way: it came too late
    /// for the desktop's deadline. Now and then that is the price of no queue in front of the
    /// display. Often, on a display whose deadline the stream keeps crossing, each picture waits its
    /// turn instead, a hold of a whole refresh: a picture behind, which never repeats one and still
    /// leaves the decoder alone. It comes back a step at a time, like any hold that grew. Returns
    /// the new hold when it grew.
    pub fn missed(&mut self, frame_us: u64, refresh_us: u64) -> Option<std::time::Duration> {
        if self.settling > 0 || !a_picture_a_refresh(frame_us, refresh_us) { return None; }
        self.missed += 1;
        if self.missed < MISSED_WAIT || !self.missed_often || self.hold_us > refresh_us { return None; }
        self.missed = 0;
        self.hold_us = refresh_us + HOLD_STEP_US;
        Some(self.hold())
    }

    /// One more picture toward the next look at the hold.
    pub(in crate::stream::present) fn count(&mut self, refused: bool) {
        self.settling = self.settling.saturating_sub(1);
        self.missed_of += 1;
        if self.missed_of >= MISSED_OF {
            self.missed_of = 0;
            self.missed_often = self.missed >= MISSED_WAIT;
            self.missed = 0;
        }
        self.pictures += 1;
        self.refused += u32::from(refused);
        if self.pictures >= PICTURES_SHORTEN {
            if self.refused <= REFUSED_SHORTEN {
                if self.waiting {
                    self.waiting = false;
                } else {
                    self.hold_us = self.hold_us.saturating_sub(HOLD_STEP_US).max(self.base_us);
                }
            }
            self.pictures = 0;
            self.refused = 0;
        }
    }

    pub fn hold(&self) -> std::time::Duration {
        std::time::Duration::from_micros(self.hold_us)
    }

    /// A picture was refused. Refused often on a stream with a picture a refresh, the hold grows
    /// until they stop; if refusals still keep coming past a whole refresh (alternating refused
    /// and shown frames never hit `REFUSED_IN_A_ROW`), wait for each picture's turn until a clean
    /// window lets no-wait presents try again. A stream faster than the display is refused by
    /// design. Returns the new hold when it grew.
    pub fn skipped(&mut self, now_us: u64, frame_us: u64, refresh_us: u64) -> Option<std::time::Duration> {
        self.due_us = Some(now_us + frame_us + frame_us / 2);
        self.in_a_row += 1;
        self.count(true);
        self.recent = self.recent << 1 | 1;
        if self.recent.count_ones() < REFUSED_OF_64_WIDENS || !a_picture_a_refresh(frame_us, refresh_us) {
            return None;
        }
        self.recent = 0;
        if self.hold_us > refresh_us {
            self.waiting = true;
            self.pictures = 0;
            self.refused = 0;
            return None;
        }
        self.hold_us += HOLD_STEP_US;
        Some(self.hold())
    }

    /// A picture was presented: nothing is left to draw. Returns whether a clean window ended
    /// waiting mode so no-wait presents can try again.
    pub fn shown(&mut self) -> bool {
        let was_waiting = self.waiting;
        self.due_us = None;
        self.in_a_row = 0;
        self.recent <<= 1;
        self.count(false);
        was_waiting && !self.waiting
    }

    /// Whether presents have fallen back to waiting for each picture's turn because no-wait
    /// presents kept being refused even past a whole refresh.
    pub fn waits(&self) -> bool {
        self.waiting
    }

    /// Whether the next present waits for its turn.
    pub fn must_wait(&self) -> bool {
        self.waiting || self.in_a_row >= REFUSED_IN_A_ROW
    }

    pub fn due_us(&self) -> Option<u64> {
        self.due_us
    }

    /// Whether the skipped picture is to be drawn now, once. That present waits for its turn: no
    /// later picture would take its place.
    pub fn take_due(&mut self, now_us: u64) -> bool {
        let due = self.due_us.is_some_and(|due| now_us >= due);
        if due {
            self.due_us = None;
            self.in_a_row = REFUSED_IN_A_ROW;
        }
        due
    }
}
