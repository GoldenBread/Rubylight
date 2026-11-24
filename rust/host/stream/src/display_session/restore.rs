//! Heartbeat maintenance, display recovery, layout/primary restoration, departure
//! repair, and final [`Prepared`] teardown on Windows.
use super::Prepared;
use super::apply::{applied_mode, failed_wake_layout};
use super::mode_report::{ModeReport, PrimaryStop};
use super::policy::{brings_back_game, take_profile};
use super::scale::scale_display;
use super::turn::follow_turn;
use anyhow::Result;
use butterpollo_media_policy::{
    display_policy::{LayoutAction, LayoutWatch, PrimaryAction, PrimaryChange, primary_moved},
    display_session_machine::{
        PausedTickAction, primary_change_arms, primary_taken_back, primary_watch_applies,
        resumed_primary_change,
    },
};
use butterpollo_win_display::{
    display::{self, Guard},
    display_arrangement, hdr_profile,
};

#[cfg(test)]
pub(super) use butterpollo_media_policy::display_session_machine::{
    RecoveryState as Recovery, departure_unsettled, notice_departure, wake_layout_passed,
};

/// Lay the displays out for a resume or a recovery. A plain layout records what is on as the
/// stream's, so with a departure `unsettled` (a paused stream misses its repair) what Windows
/// recalled would become what the repair compares with, and the monitor would stay dark or at
/// 60 Hz for the rest of the stream. The repair comes first then.
pub(super) fn reapply_settling(
    unsettled: bool,
    report: &ModeReport,
    reapply: impl FnOnce() -> Result<()>,
    after_departure: impl FnOnce() -> Result<Option<String>>,
) -> Result<()> {
    if unsettled {
        report.repaired(after_departure()?);
        Ok(())
    } else {
        reapply()
    }
}

/// Lay the displays out again after a display left, with what stays on: the remote
/// monitors, and what the watch leaves on for the rest of the stream. A failed layout stays
/// due for the next try; a repair that failed goes on the stream's card.
pub(super) fn lay_out_after_departure(
    watch: &mut LayoutWatch,
    mut retained: Vec<String>,
    now: std::time::Instant,
    report: &ModeReport,
    reapply: impl FnOnce(&[String]) -> Result<Option<String>>,
) -> Result<()> {
    retained.extend(watch.kept().iter().cloned());
    match reapply(&retained) {
        Ok(failure) => report.repaired(failure),
        Err(error) => {
            watch.failed(now);
            return Err(error);
        }
    }
    watch.departure_handled();
    Ok(())
}

impl Prepared {
    /// Log what the display layer and Windows report, with the console state
    /// and idle time that explain a display Windows switched off.
    pub(super) fn log_display_diagnostics(&mut self, stage: &str) {
        let secure = butterpollo_win_io::input::secure_desktop_shown();
        let idle = butterpollo_win_io::wake::idle_seconds().ok();
        match self
            .display
            .as_mut()
            .map(|display| display.diagnostics(stage))
        {
            Some(Ok(Some(diagnostics))) => tracing::info!(
                stage,
                swap_chain = diagnostics.status.swap_chain,
                ring = diagnostics.status.ring,
                frames_acquired = diagnostics.status.frames_acquired,
                frames_pushed = diagnostics.status.frames_pushed,
                ccd_active = ?diagnostics.ccd_active,
                secure_desktop = secure,
                idle_seconds = ?idle,
                "stream display status while waiting for the first frame"
            ),
            Some(Ok(None)) | None => tracing::info!(
                stage,
                secure_desktop = secure,
                idle_seconds = ?idle,
                "stream display status while waiting for the first frame (physical display)"
            ),
            Some(Err(error)) => tracing::warn!(
                stage,
                error = %format!("{error:#}"),
                secure_desktop = secure,
                idle_seconds = ?idle,
                "could not read the virtual display status while waiting for the first frame"
            ),
        }
    }

    /// Follows a stream that turned to `to` mid-stream when the device's
    /// display follows it.
    pub(super) fn follow_shape(&mut self, to: (u32, u32)) -> Result<()> {
        let Some(follow) = self.follow else {
            return Ok(());
        };
        let output = self.output.clone();
        let Some(display) = self.display.as_mut() else {
            return Ok(());
        };
        let Some(size) = follow_turn(display, follow, to, || {
            display::stream_display_rotation(&output)
        })?
        else {
            return Ok(());
        };
        tracing::info!(output = %self.output, width = size.0, height = size.1, "the stream's display turned with the stream");
        self.recovery_dimensions = size;
        self.mode_report.adopt(size);
        self.mode_report.publish(applied_mode(&self.output));
        Ok(())
    }

    pub(super) fn feed(&mut self, active: bool) -> Result<()> {
        if !active {
            self.display.as_mut().map_or(Ok(()), Guard::keep_alive)?;
            return match self.machine.paused_tick() {
                PausedTickAction::KeepAliveAndHoldLayout => self.keep_layout(false),
                PausedTickAction::KeepAliveOnly => Ok(()),
            };
        }
        let fed = self.display.as_mut().map_or(Ok(false), Guard::feed);
        self.finish_feed(fed)
    }

    pub(super) fn finish_feed(&mut self, fed: Result<bool>) -> Result<()> {
        if let (Ok(true), Some(display)) = (&fed, &self.display) {
            self.output = display.output.clone();
            self.revision = self.revision.wrapping_add(1);
            self.machine.begin_recovery(std::time::Instant::now());
        }
        if let Some(retained) = &self._retained {
            let (output, generation) = retained.capture_target();
            if generation != self.revision {
                self.output = output;
                self.revision = generation;
                self.machine.begin_recovery(std::time::Instant::now());
            }
        }
        if self.machine.recovery.due(std::time::Instant::now()) {
            scale_display(
                &self.output,
                self.recovery_scale,
                self.recovery_dimensions,
                display::virtual_scale,
            )?;
            if let Some(arrangement) = &self._arrangement {
                self.lay_out_again(arrangement)?;
            }
            if self.machine.recovery.take_mode() {
                if let Some(display) = &self.display {
                    display.apply_virtual_mode("after recovery layout");
                }
                self.mode_report.publish(applied_mode(&self.output));
            }
            if let Some(profile) = &self.recovery_profile {
                let output = &self.output;
                take_profile(
                    &mut self._profile,
                    Some(profile),
                    |held, profile| match held {
                        Some(lease) => lease.switch(output, profile).map(|()| None),
                        None => hdr_profile::Lease::acquire(output, profile).map(Some),
                    },
                )?;
            }
            self.machine.finish_recovery(1);
        } else {
            self.keep_layout(true)?;
        }
        fed.map(|_| ())
    }

    fn bring_back_game(&self) {
        let Some(host) = self.host.upgrade() else {
            return;
        };
        let Some(before) = display_arrangement::minimized_before_departure() else {
            return;
        };
        std::thread::spawn(move || {
            let processes = match host.current_app.lock().unwrap().as_ref() {
                Some(app) => app.departure_windows(),
                None => return,
            };
            match processes {
                Ok(processes) => {
                    for pid in processes {
                        if butterpollo_win_io::lossless::restore_minimized(pid, &before) {
                            tracing::info!(
                                pid,
                                "brought back the game's window Windows minimised when a display left"
                            );
                        }
                    }
                }
                Err(error) => {
                    tracing::warn!(error = %format!("{error:#}"), "could not find the game's windows after a display left");
                }
            }
        });
    }

    pub(super) fn lay_out_again(&self, arrangement: &display_arrangement::Lease) -> Result<()> {
        let retained = self.remote_monitors();
        reapply_settling(
            self.machine
                .departure_unsettled(display_arrangement::departures()),
            &self.mode_report,
            || arrangement.reapply(&self.output, &retained),
            || arrangement.reapply_after_departure(&self.output, &retained),
        )
    }

    fn remote_monitors(&self) -> Vec<String> {
        self.host
            .upgrade()
            .map(|h| crate::remote_display::outputs(&h))
            .unwrap_or_default()
    }

    fn keep_layout(&mut self, active: bool) -> Result<()> {
        let now = std::time::Instant::now();
        if self.machine.recovery.pending || !self.machine.layout_watch.due(now) {
            return Ok(());
        }
        let departed = self
            .machine
            .notice_departure(display_arrangement::departures(), now);
        if self.machine.layout_watch.departure_due(now) {
            let Some(arrangement) = &self._arrangement else {
                return Ok(());
            };
            tracing::info!(output = %self.output, "another stream's display left; laying this stream's displays out again");
            let monitors = self.remote_monitors();
            lay_out_after_departure(
                &mut self.machine.layout_watch,
                monitors,
                now,
                &self.mode_report,
                |retained| arrangement.reapply_after_departure(&self.output, retained),
            )?;
            if let Some(display) = &self.display {
                display.apply_virtual_mode("after a display left");
            }
            if brings_back_game(self._retained.is_some(), active) {
                self.bring_back_game();
            }
            return Ok(());
        }
        {
            let Some(arrangement) = &self._arrangement else {
                return Ok(());
            };
            match arrangement.follow_wake() {
                Ok(false) => {}
                Ok(true) => self.mode_report.repaired(None),
                Err(error) => self.mode_report.repaired(Some(failed_wake_layout(error))),
            }
        }
        let woke = self
            .machine
            .notice_wake_layout(display_arrangement::wake_layouts());
        if woke && let Some(display) = &self.display {
            display.apply_virtual_mode("after a monitor returned");
        }
        let secure = butterpollo_win_io::input::secure_desktop_shown();
        let session = butterpollo_win_io::process::console_session();
        let change = self
            .machine
            .observe_desktop_change(secure, session, departed, woke, now);
        self.keep_primary(now, change, active, secure);
        let Some(arrangement) = &self._arrangement else {
            return Ok(());
        };
        let switched_on = match arrangement.switched_back_on() {
            Ok(switched_on) => switched_on,
            Err(error) => {
                self.machine.layout_watch.failed(now);
                return Err(error);
            }
        };
        let (displays, kept_now) = match self.machine.layout_watch.observe(&switched_on, now) {
            LayoutAction::Wait => return Ok(()),
            LayoutAction::Reapply { displays, kept_now } => (displays, kept_now),
        };
        if !kept_now.is_empty() {
            tracing::warn!(
                displays = ?kept_now,
                "a display the stream layout switched off keeps coming back on; leaving it on for the rest of the stream"
            );
            self.mode_report.keep_on();
        }
        if displays.is_empty() {
            return Ok(());
        }
        let mut retained = self.remote_monitors();
        retained.extend(self.machine.layout_watch.kept().iter().cloned());
        tracing::info!(displays = ?displays, kept_on = ?self.machine.layout_watch.kept(), "displays the stream layout switched off came back on; reapplying it");
        let result = arrangement.reapply(&self.output, &retained);
        match &result {
            Ok(()) => self.machine.layout_watch.reapplied(&displays, now),
            Err(_) => self.machine.layout_watch.failed(now),
        }
        result
    }

    #[cfg(test)]
    pub(super) fn primary_watch_applies(
        planned: butterpollo_media_policy::display_policy::Arrangement,
        lease_primary: bool,
        active: bool,
        secure: bool,
        due: bool,
    ) -> bool {
        primary_watch_applies(planned, lease_primary, active, secure, due)
    }

    pub(super) fn primary_taken_back(shown: &Result<Option<bool>>) -> bool {
        primary_taken_back(shown)
    }

    #[cfg(test)]
    pub(super) fn primary_change_arms(
        last_shown: Option<bool>,
        change: Option<PrimaryChange>,
    ) -> bool {
        primary_change_arms(last_shown, change)
    }

    #[cfg(test)]
    pub(super) fn resumed_primary_change(
        paused: Option<PrimaryChange>,
        change: Option<PrimaryChange>,
    ) -> Option<PrimaryChange> {
        resumed_primary_change(paused, change)
    }

    fn keep_primary(
        &mut self,
        now: std::time::Instant,
        change: Option<PrimaryChange>,
        active: bool,
        secure: bool,
    ) {
        let Some(arrangement) = &self._arrangement else {
            return;
        };
        let planned = arrangement.arrangement();
        let lease_primary = arrangement.makes_primary();
        if !self
            .machine
            .prepare_primary_check(now, change, active, secure, planned, lease_primary)
        {
            return;
        }
        match display::active_displays() {
            Ok(count) => {
                self.machine.observe_display_count(count, now);
            }
            Err(error) => {
                tracing::debug!(error = %format!("{error:#}"), "the displays could not be counted");
            }
        }
        let shown = match display::shown_primary(&self.output) {
            Ok(shown) => shown,
            Err(error) => {
                tracing::debug!(error = %format!("{error:#}"), "the primary display could not be read");
                return;
            }
        };
        if shown.is_some() {
            self.machine.shown_primary = shown;
        }
        if !primary_moved(planned, shown) {
            self.machine.primary_watch.settled(now);
            if shown == Some(true) {
                self.mode_report.primary_restored();
                self.clear_start_primary_warning();
            }
            return;
        }
        if let Some(delay) = self.machine.primary_watch.change_delay(now) {
            tracing::info!(output = %self.output, delay_ms = %delay.as_millis(), "the stream's display lost the primary one after a Windows change");
        }
        if self.machine.primary_watch.observe(true, now) == PrimaryAction::Wait {
            return;
        }
        tracing::info!(output = %self.output, "the stream's display lost the primary one; laying the display out as primary again");
        let mut retained = self.remote_monitors();
        retained.extend(self.machine.layout_watch.kept().iter().cloned());
        match arrangement.reapply(&self.output, &retained) {
            Ok(()) => {
                let after = display::shown_primary(&self.output);
                if Self::primary_taken_back(&after) {
                    self.machine.shown_primary = Some(true);
                    let stopped = self.machine.primary_watch.restored(now);
                    self.mode_report.primary_restored();
                    self.clear_start_primary_warning();
                    if stopped {
                        tracing::warn!(output = %self.output, "Windows keeps taking the main screen back; leaving the primary where Windows moves it for the rest of the stream");
                        self.mode_report.primary_gave_up(PrimaryStop::KeptTaking);
                    }
                } else {
                    let problem = match after {
                        Ok(_) => {
                            "the layout went through but the primary stayed where it was".to_owned()
                        }
                        Err(error) => format!("{error:#}"),
                    };
                    self.primary_failed(now, problem);
                }
            }
            Err(error) => {
                self.primary_failed(now, format!("{error:#}"));
            }
        }
    }

    fn clear_start_primary_warning(&mut self) {
        if self.machine.take_start_primary_mismatch() {
            self.mode_report.start_layout_restored();
        }
    }

    fn primary_failed(&mut self, now: std::time::Instant, problem: String) {
        tracing::warn!(%problem, "the layout did not make the stream's display the primary one");
        let stopped = self.machine.primary_watch.failed(now);
        self.mode_report.primary_refused(&problem);
        if stopped {
            tracing::warn!(output = %self.output, "Windows keeps refusing the main screen; leaving the primary where Windows puts it for the rest of the stream");
            self.mode_report.primary_gave_up(PrimaryStop::Refused);
        }
    }
}

impl Drop for Prepared {
    fn drop(&mut self) {
        self.machine.mark_restored();
        butterpollo_win_io::input::on_input_desktop(|| {
            self._profile.take();
            let remains = (
                self.display.take(),
                self._activation.take(),
                self._golden.take(),
            );
            match self._arrangement.take() {
                Some(lease) => lease.end(remains),
                None => drop(remains),
            }
            self._retained.take();
            self._vulkan.take();
        });
    }
}
