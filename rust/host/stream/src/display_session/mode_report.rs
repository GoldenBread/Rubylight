//! Session display mode reporting and user-facing display warnings.
use super::{AppliedMode, RequestedMode};
use anyhow::Result;
use butterpollo_session::session::Warnings;
use std::sync::{Arc, Mutex};

/// What the display shows compared with the stream's request, on the card of
/// the launch that uses the display now. A retained game display moves to
/// the next launch, which sees the last finding at once.
#[derive(Clone)]
pub(super) struct ModeReport(Arc<Mutex<ModeReportState>>);

struct ModeReportState {
    warnings: Arc<Warnings>,
    requested: RequestedMode,
    stream_rate: u32,
    last: Option<std::result::Result<AppliedMode, String>>,
    /// Why the display could not follow the stream's last turn, until a turn follows.
    unfollowed: Option<String>,
    /// A shared virtual display: another stream on it may have raised its
    /// refresh above the requested one, which serves this stream too.
    shared: bool,
    /// The exclusive layout stopped switching a display off because it kept coming back.
    kept_on: bool,
    /// Why what Windows recalled when this stream's display arrived could not be put back.
    /// Nothing repairs it later: a repair after a departure only compares with what is on.
    unrepaired_at_start: Option<String>,
    /// Why what Windows recalled when a display left could not be put back, until a repair
    /// after a later departure goes through.
    unrepaired: Option<String>,
    /// Why the stream's display could not be made the primary one again, until it is.
    primary_refused: Option<String>,
    /// Why this stream leaves the primary where Windows puts it: Windows refused the layout,
    /// or kept taking the primary back.
    primary_stopped: Option<PrimaryStop>,
    /// The stream already fell back from HDR to SDR and replaced `"display_hdr"` with its own
    /// fallback explanation, so later mode reports must not overwrite or clear it.
    hdr_fell_back: bool,
}

/// Why the primary watch stopped for the stream, for the card that says so.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PrimaryStop {
    /// Windows refused the layout, or accepted it without moving the primary.
    Refused,
    /// Windows kept taking the primary back after restores.
    KeptTaking,
}

impl ModeReportState {
    fn report(&self) {
        match &self.last {
            Some(Ok(actual)) => {
                self.warnings.clear("display_verify");
                let mut requested = self.requested;
                if self.shared
                    && requested
                        .1
                        .is_some_and(|rate| actual.2.saturating_add(500) >= rate)
                {
                    requested.1 = None;
                }
                if self.hdr_fell_back {
                    butterpollo_media_policy::display_policy::report_mode_keeping_hdr(
                        &self.warnings,
                        requested,
                        *actual,
                        self.stream_rate,
                    );
                } else {
                    butterpollo_media_policy::display_policy::report_mode(
                        &self.warnings,
                        requested,
                        *actual,
                        self.stream_rate,
                    );
                }
            }
            Some(Err(error)) => self.warnings.set("display_verify", format!("Could not verify the applied display mode ({error}); refresh and HDR may differ from the request. Check Windows display settings and reconnect.")),
            None => {}
        }
        match &self.unfollowed {
            Some(error) => self.warnings.set("display_follow", format!("The display could not turn with the stream ({error}); the picture is scaled to fit instead. Check Windows display settings or turn the device back.")),
            None => self.warnings.clear("display_follow"),
        }
        if self.kept_on {
            self.warnings.set("display_kept_on", "Windows kept switching a monitor back on during the stream, so it stays on until the stream ends. A monitor that loses its signal in standby or scans its inputs (Auto Scan) reconnects like this; turn that off on the monitor, or use the Extended layout.");
        } else {
            self.warnings.clear("display_kept_on");
        }
        match self.unrepaired.as_ref().or(self.unrepaired_at_start.as_ref()) {
            Some(error) => self.warnings.set("display_repair", format!("Windows recalled a saved display layout over your displays and it could not be put back ({error}), so a monitor can stay off or at another refresh rate. Check Windows display settings and reconnect.")),
            None => self.warnings.clear("display_repair"),
        }
        match &self.primary_refused {
            Some(error) => self.warnings.set("display_primary", format!("Windows kept another display as the main screen and the virtual display could not be made the main one again ({error}), so games open on your monitor instead of the stream. Set the virtual display layout to Exclusive for this device, or make the virtual display the main screen in Windows display settings and reconnect.")),
            None => self.warnings.clear("display_primary"),
        }
        if let Some(stop) = self.primary_stopped {
            let what = match stop {
                PrimaryStop::Refused => {
                    "Windows refused to make the virtual display the main screen again"
                }
                PrimaryStop::KeptTaking => "Windows kept taking the main screen back",
            };
            self.warnings.set("display_primary_kept", format!("{what}, so Rubylight leaves it where Windows puts it for the rest of the stream. Games open on your monitor instead of the stream; choose Exclusive for this device, or keep the virtual display as the main screen in Windows display settings."));
        } else {
            self.warnings.clear("display_primary_kept");
        }
    }
}

impl ModeReport {
    pub(super) fn new(warnings: Arc<Warnings>, requested: RequestedMode, stream_rate: u32) -> Self {
        Self(Arc::new(Mutex::new(ModeReportState {
            warnings,
            requested,
            stream_rate,
            last: None,
            unfollowed: None,
            shared: false,
            kept_on: false,
            unrepaired_at_start: None,
            unrepaired: None,
            primary_refused: None,
            primary_stopped: None,
            hdr_fell_back: false,
        })))
    }
    /// Record that the stream fell back from HDR to SDR so later mode reports do
    /// not overwrite or clear the fallback explanation on `"display_hdr"`.
    pub(super) fn hdr_fell_back(&self) {
        self.0.lock().unwrap().hdr_fell_back = true;
    }
    /// Record that display HDR became active again and the stream returned to
    /// HDR: clears the fallback latch and `"display_hdr"` warning and updates
    /// any stale cached SDR reading so unrelated mode reports do not re-warn.
    pub(super) fn hdr_restored(&self) {
        let mut state = self.0.lock().unwrap();
        state.hdr_fell_back = false;
        if let Some(Ok(actual)) = &mut state.last {
            actual.3 = true;
        }
        state.warnings.clear("display_hdr");
    }
    /// What Windows recalled when this stream's display arrived could not be put back.
    pub(super) fn unrepaired_at_start(&self, failure: Option<&str>) {
        let mut state = self.0.lock().unwrap();
        state.unrepaired_at_start = failure.map(str::to_owned);
        state.report();
    }
    /// How making the stream's display the primary one again went.
    pub(super) fn primary_refused(&self, error: &str) {
        let mut state = self.0.lock().unwrap();
        state.primary_refused = Some(error.to_owned());
        state.report();
    }
    /// The stream's display is the primary one.
    pub(super) fn primary_restored(&self) {
        let mut state = self.0.lock().unwrap();
        if state.primary_refused.take().is_some() {
            state.report();
        }
    }
    /// The start's display_layout warning was about the primary only, which the stream has
    /// back: nothing there needs a reconnect.
    pub(super) fn start_layout_restored(&self) {
        self.0.lock().unwrap().warnings.clear("display_layout");
    }
    /// The primary watch stopped: this stream leaves the primary where Windows puts it, and
    /// the card says whether Windows refused the layout or kept taking the primary back.
    pub(super) fn primary_gave_up(&self, why: PrimaryStop) {
        let mut state = self.0.lock().unwrap();
        state.primary_stopped = Some(why);
        state.report();
    }
    /// How putting back what Windows recalled when a display left went.
    pub(super) fn repaired(&self, failure: Option<String>) {
        let mut state = self.0.lock().unwrap();
        state.unrepaired = failure;
        state.report();
    }
    /// The layout leaves a display that keeps coming back on for the rest of the stream.
    pub(super) fn keep_on(&self) {
        let mut state = self.0.lock().unwrap();
        state.kept_on = true;
        state.report();
    }
    /// The display is a shared virtual display: only a refresh below the
    /// requested one is reported.
    pub(super) fn shared(self) -> Self {
        self.0.lock().unwrap().shared = true;
        self
    }
    pub(super) fn publish(&self, actual: Result<AppliedMode>) {
        let mut state = self.0.lock().unwrap();
        state.last = Some(actual.map_err(|error| format!("{error:#}")));
        state.report();
    }
    pub(super) fn attach(&self, warnings: Arc<Warnings>) {
        let mut state = self.0.lock().unwrap();
        state.warnings = warnings;
        state.hdr_fell_back = false;
        state.report();
    }
    /// The stream asks for another rate than the launch the display was
    /// prepared for.
    pub(super) fn retime(&self, requested: RequestedMode, stream_rate: u32) {
        let mut state = self.0.lock().unwrap();
        state.requested = requested;
        state.stream_rate = stream_rate;
    }
    /// Whether the display followed the stream's last turn.
    pub(super) fn followed(&self, result: &Result<()>) {
        let mut state = self.0.lock().unwrap();
        state.unfollowed = result.as_ref().err().map(|error| format!("{error:#}"));
        state.report();
    }
    /// The display now runs at a resolution the user chose, which the stream keeps.
    pub(super) fn adopt(&self, resolution: (u32, u32)) {
        self.0.lock().unwrap().requested.0 = Some(resolution);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use butterpollo_media_policy::framegen::Rate;

    #[test]
    fn hdr_recovery_clears_fallback_latch_and_warning_without_restamping_stale_sdr() {
        let warnings = Arc::new(Warnings::default());
        let report = ModeReport::new(
            warnings.clone(),
            super::super::joined_report(Rate(144_000), Some(true)),
            144_000,
        );
        report.publish(Ok((2560, 1440, 144_000, false)));
        warnings.set("display_hdr", "fallback");
        report.hdr_fell_back();

        // Capture reports display HDR active again: clears warning and latch.
        report.hdr_restored();
        assert!(warnings.snapshot().is_empty());

        // An unrelated report refresh (e.g. primary restored) must not re-warn from stale SDR.
        report.primary_refused("denied");
        report.primary_restored();
        assert!(warnings.snapshot().is_empty());

        // A genuinely new SDR mode report warns again normally.
        report.publish(Ok((2560, 1440, 144_000, false)));
        let snap = warnings.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].code, "display_hdr");
    }
}
