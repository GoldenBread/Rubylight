//! Feedback polling, coalescing, failure rate-limiting, and Steam Controller routing for [`Gamepads`].

use super::*;

#[derive(Default)]
pub(super) struct FeedbackFailure {
    pub(super) unreported: u64,
    pub(super) reported: Option<Instant>,
}

/// Feedback reports read from one pad per poll at most. The VHF driver hands
/// out one pending report per call, so a game writing faster than the poll
/// left a backlog, and its rumble lagged or ran on after it stopped.
pub(in crate::input) const FEEDBACK_DRAIN: usize = 8;

/// What a feedback report sets, as bits: rumble 1, lights 2, trigger
/// effects 4. A PlayStation report (kind 5) always carries rumble, and its
/// lightbar and trigger effects only with flag 1 and 2 of byte 7
/// (`feedback_packets` in the host's control stream); any other report
/// replaces all of its kind.
fn feedback_parts(kind: u16, data: &[u8]) -> u8 {
    match kind {
        5 if data.len() >= 8 => 1 | (data[7] & 3) << 1,
        _ => 7,
    }
}

/// Steam Controller haptics and settings, which are never coalesced or
/// deduplicated: Steam re-sends a click to keep a buzz going, and one chunk
/// of a settings write cannot replace another.
fn steam_report(kind: u16) -> bool {
    matches!(
        kind,
        steam_pad::DRIVER_STEAM_HAPTIC | steam_pad::DRIVER_STEAM_SETTINGS
    )
}

pub(super) fn poll_feedback(
    active: &BTreeMap<u16, u16>,
    last: &mut BTreeMap<u16, (u16, Vec<u8>)>,
    failures: &mut BTreeMap<u16, FeedbackFailure>,
    backend: &str,
    now: Instant,
    mut poll: impl FnMut(u32) -> Result<Option<(u16, Vec<u8>)>>,
) -> Vec<(u16, u16, Vec<u8>)> {
    let mut output = vec![];
    let due = |failure: &FeedbackFailure| {
        failure
            .reported
            .is_none_or(|at| now.duration_since(at) >= Duration::from_secs(5))
    };
    // What a pad had pending, in arrival order, without the reports a newer
    // one of the same kind fully replaces: an older rumble the game already
    // changed is not worth sending, but older lights or trigger effects a
    // rumble-only report leaves out still go, before it.
    let mut pending: Vec<(u16, Vec<u8>)> = Vec::new();
    for (&id, &global) in active {
        pending.clear();
        let mut failed = None;
        for _ in 0..FEEDBACK_DRAIN {
            match poll(u32::from(global)) {
                Ok(Some(report)) => {
                    if !steam_report(report.0) {
                        let parts = feedback_parts(report.0, &report.1);
                        pending.retain(|(kind, data)| {
                            *kind != report.0 || feedback_parts(*kind, data) & !parts != 0
                        });
                    }
                    pending.push(report);
                }
                Ok(None) => break,
                Err(error) => {
                    failed = Some(error);
                    break;
                }
            }
        }
        match failed {
            None => {
                // Report the failures the interval held back once the pad
                // recovers, under the same interval, so none go unlogged.
                if let Some(failure) = failures.get_mut(&id)
                    && failure.unreported > 0
                    && due(failure)
                {
                    tracing::warn!(
                        controller = id,
                        slot = global,
                        backend,
                        failures = failure.unreported,
                        "controller feedback polls recovered after failing"
                    );
                    failure.unreported = 0;
                    failure.reported = Some(now);
                }
            }
            Some(error) => {
                let failure = failures.entry(id).or_default();
                failure.unreported += 1;
                if due(failure) {
                    tracing::warn!(controller = id, slot = global, backend, failures = failure.unreported, error = %format!("{error:#}"), "controller feedback poll failed");
                    failure.unreported = 0;
                    failure.reported = Some(now);
                }
            }
        }
        // Reports read before a failing call still go out.
        for report in pending.drain(..) {
            if steam_report(report.0) {
                output.push((id, report.0, report.1));
            } else if last.get(&id) != Some(&report) {
                last.insert(id, report.clone());
                output.push((id, report.0, report.1));
            }
        }
    }
    last.retain(|id, _| active.contains_key(id));
    // Keep the warning interval across successful polls too: an intermittent
    // failure must not flood the log. Unplugging starts a fresh pad lifetime.
    failures.retain(|id, _| active.contains_key(id));
    output
}

impl Gamepads {
    /// New feedback (rumble, lights, trigger effects) for each controller.
    /// A controller with nothing pending or a failed poll does not hide the
    /// others' feedback, and a repeated report is not sent again.
    pub fn feedback(&mut self) -> Vec<(u16, u16, Vec<u8>)> {
        let mut drained = 0;
        let mut removed = Vec::new();
        let mut lost = false;
        let mut reports = poll_feedback(
            &self.active,
            &mut self.last_feedback,
            &mut self.feedback_failures,
            self.backend.name(),
            Instant::now(),
            |slot| {
                let report = self.backend.feedback(slot);
                drained += usize::from(matches!(report, Ok(Some(_))));
                if report.as_ref().is_err_and(device_removed) {
                    removed.push(slot);
                }
                lost |= report.as_ref().is_err_and(gamepad_backend::driver_lost);
                report
            },
        );
        self.feedback_drained += drained;
        // A refused handle is no pad to create again: the gamepad thread
        // closes these pads and opens the driver anew.
        self.lost |= lost;
        if !self.lost && !removed.is_empty() {
            let now = Instant::now();
            for slot in removed {
                let id = self
                    .active
                    .iter()
                    .find_map(|(&id, &global)| (u32::from(global) == slot).then_some(id));
                let Some(id) = id else {
                    continue;
                };
                let global = self.active[&id];
                if self.dropped.contains(&id) {
                    if let Err(error) = self.backend.unplug(u32::from(global)) {
                        self.lost |= gamepad_backend::driver_lost(&error);
                        tracing::debug!(
                            controller = id,
                            error = format!("{error:#}"),
                            "a virtual controller the client removed still will not unplug"
                        );
                    } else {
                        self.forget_pad(id, global);
                    }
                    continue;
                }
                if !self.recreate_due(id, now) {
                    continue;
                }
                if let Err(error) = self.recreate(id) {
                    self.lost |= gamepad_backend::driver_lost(&error);
                    tracing::debug!(
                        controller = id,
                        error = format!("{error:#}"),
                        "a virtual controller could not be re-created"
                    );
                }
            }
        }
        let arrivals = &self.arrivals;
        let steam_haptics = &mut self.steam_haptics;
        reports.retain_mut(|(id, kind, data)| {
            let (_, capabilities) = arrivals.get(id).copied().unwrap_or_default();
            match steam_pad::route(*id, *kind, data, capabilities) {
                None => true,
                Some(Some(steam_pad::Feedback::Forward(packet, payload))) => {
                    *kind = packet;
                    *data = payload;
                    true
                }
                Some(Some(steam_pad::Feedback::Rumble(haptic))) => {
                    steam_haptics.push(haptic);
                    false
                }
                Some(None) => false,
            }
        });
        for (id, deck) in &mut self.decks {
            if let Some((kind, data)) = deck.feedback() {
                reports.push((*id, kind, data));
            }
        }
        for (id, pad) in &mut self.switch2s {
            if let Some((kind, data)) = pad.feedback() {
                reports.push((*id, kind, data));
            }
        }
        for (id, pad) in &self.dualsenses {
            reports.extend(
                pad.feedback()
                    .into_iter()
                    .map(|(kind, data)| (*id, kind, data)),
            );
        }
        reports
    }
}
