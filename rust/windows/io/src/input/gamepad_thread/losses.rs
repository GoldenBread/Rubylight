//! Driver-loss tracking, bounded reopen scheduling and PnP recovery wiring for
//! [`super::GamepadThread`].

use super::*;

/// The driver's refusals of the pads' handle since pads last worked for
/// LOST_SPAN, and failed opens among them (#138).
#[derive(Default)]
pub(super) struct Losses {
    pub(super) count: usize,
    pub(super) opened: Option<Instant>,
    pub(super) warned: bool,
    pub(super) recovering: bool,
    pub(super) memory: Option<PadMemory>,
}

impl Losses {
    pub(super) fn opened(&mut self, now: Instant) {
        self.opened = Some(now);
    }

    /// Whether the pads now open have worked for LOST_SPAN.
    pub(super) fn worked(&self, now: Instant) -> bool {
        self.opened
            .is_some_and(|at| now.saturating_duration_since(at) >= LOST_SPAN)
    }

    /// Records a refusal at `now`. Returns how long to wait before opening
    /// the driver again: `poll.reopen`, from the LOST_LIMIT'th `poll.retry`,
    /// and from the STOP_LIMIT'th SLOW_RETRY times that, at input only.
    pub(super) fn record(&mut self, now: Instant, poll: &Polling) -> Duration {
        if self.worked(now) {
            self.count = 0;
        }
        self.opened = None;
        self.count += 1;
        self.warned |= self.count >= LOST_LIMIT;
        if self.count >= STOP_LIMIT {
            poll.retry * SLOW_RETRY
        } else if self.count >= LOST_LIMIT {
            poll.retry
        } else {
            poll.reopen
        }
    }

    /// Whether the driver is opened again only at input, and seldom, until
    /// the next stream.
    pub(super) fn stopped(&self) -> bool {
        self.count >= STOP_LIMIT
    }

    /// Whether the warning should go: true once, when pads opened after it
    /// worked for LOST_SPAN.
    pub(super) fn recovered(&mut self, now: Instant) -> bool {
        if !self.warned || !self.worked(now) {
            return false;
        }
        self.warned = false;
        self.count = 0;
        true
    }
}

/// The console warning for a driver that keeps refusing the pads' handle,
/// with how many more times it is opened again.
pub(super) fn lost_warning(present: bool, left: usize, retry: Duration) -> String {
    let gone = if present {
        ""
    } else {
        ", and its device is gone"
    };
    let slow = (retry * SLOW_RETRY).as_secs();
    let next = match left {
        0 => format!(
            "Rubylight now opens it again only when a controller is used, at most once every {slow} seconds, until the next stream or game launch starts; controllers may not work meanwhile, while keyboard and mouse remain available"
        ),
        left => format!(
            "Rubylight opens it again up to {left} more time{}, at least {} seconds apart, then only when a controller is used, at most once every {slow} seconds, until the next stream or game launch starts; meanwhile controllers may not respond, while keyboard and mouse remain available",
            if left == 1 { "" } else { "s" },
            retry.as_secs()
        ),
    };
    format!(
        "The virtual gamepad driver (Rubylight Virtual Gamepad) keeps refusing the host's connection (0x80070006){gone}. {next}. If controllers stay unavailable, disable and re-enable Rubylight Virtual Gamepad under System devices in Device Manager, or run Rubylight setup again."
    )
}

/// Counts a refusal and sets when the driver is opened again; the console
/// warns from the LOST_LIMIT'th.
pub(super) fn count_refusal(
    shared: &Shared,
    losses: &mut Losses,
    retry: &mut Option<Instant>,
    present: bool,
    poll: &Polling,
) -> Duration {
    let now = Instant::now();
    let wait = losses.record(now, poll);
    *retry = Some(now + wait);
    if losses.warned {
        shared.warnings.lock().unwrap().set(
            DRIVER_LOST,
            lost_warning(present, STOP_LIMIT.saturating_sub(losses.count), poll.retry),
        );
    }
    wait
}

/// Closes pads whose driver refused their handle and schedules reopen (#138).
pub(super) fn forget_lost<P: Pads>(
    shared: &Shared,
    pads: &mut Option<P>,
    losses: &mut Losses,
    retry: &mut Option<Instant>,
    poll: &Polling,
) -> bool {
    if !pads.as_ref().is_some_and(P::lost) {
        return false;
    }
    let present = pads.as_ref().is_some_and(P::driver_present);
    losses.memory = pads.as_ref().map(P::memory);
    // Drop first so Windows can tear down/restart the devnode without a held handle.
    *pads = None;
    shared.audio_ready.store(0, Ordering::Release);
    if !present {
        losses.recovering = P::recover_driver();
    }
    let wait = count_refusal(shared, losses, retry, present, poll);
    tracing::warn!(
        device = if present { "present" } else { "gone" },
        refusals = losses.count,
        reopen_in = ?wait,
        "the virtual gamepad driver refused the host's handle (0x80070006): its controllers are closed and the driver will be opened again{}",
        if losses.stopped() {
            " at input only, until the next stream or game launch starts"
        } else {
            ""
        }
    );
    true
}

/// Opens the driver, retrying faster while bounded PnP recovery is active.
pub(super) fn open_driver<P: Pads>(
    shared: &Shared,
    open: &mut impl FnMut() -> Result<P>,
    losses: &mut Losses,
    retry: &mut Option<Instant>,
    poll: &Polling,
) -> Option<P> {
    let now = Instant::now();
    match open() {
        Ok(mut opened) => {
            shared.warnings.lock().unwrap().clear("input_gamepad");
            losses.recovering = false;
            losses.opened(now);
            if let Some(memory) = losses.memory.take() {
                opened.remember(memory);
            }
            Some(opened)
        }
        Err(error) => {
            losses.recovering = P::recovering_driver();
            shared.warnings.lock().unwrap().set(
                "input_gamepad",
                super::super::gamepad_backend::unavailable(&error),
            );
            *retry = Some(
                now + if losses.recovering {
                    poll.reopen
                } else {
                    poll.retry
                },
            );
            None
        }
    }
}

/// Next timer-driven reopen while a controller waits to arrive again.
pub(super) fn reopen_at(
    open: bool,
    retry: Option<Instant>,
    setup: &[Event],
    losses: &Losses,
) -> Option<Instant> {
    if open
        || (losses.count == 0 && !losses.recovering)
        || losses.stopped()
        || !setup
            .iter()
            .any(|kept| matches!(kept, Event::Arrival { .. }))
    {
        return None;
    }
    retry
}
