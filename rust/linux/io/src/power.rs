//! Sleep, shutdown and resume watching on Linux. None of it is available yet: the PC reports no sleep state, sleeping and shutting down fail saying so, and the resume watch is held but never fires.

use anyhow::Result;
use butterpollo_session::power_action::SleepStates;

/// The sleep states this PC reports: none, so the console offers no Sleep PC.
pub fn sleep_states() -> Result<SleepStates> {
    Ok(SleepStates::default())
}

/// Put the PC to sleep and return after it resumes.
pub fn suspend() -> Result<()> {
    Err(crate::unsupported("Sleeping the PC"))
}

/// A registration for resume from sleep, ended when it drops.
pub struct ResumeWatch {
    _callback: Box<dyn Fn() + Send + Sync>,
}

/// Call `on_resume` whenever the PC resumes from sleep, until the returned watch drops. The
/// call never comes yet.
pub fn on_resume(on_resume: impl Fn() + Send + Sync + 'static) -> Result<ResumeWatch> {
    Ok(ResumeWatch {
        _callback: Box::new(on_resume),
    })
}

/// Restart or shut down this PC without closing programs by force.
pub fn power_off(restart: bool) -> Result<()> {
    let _ = restart;
    Err(crate::unsupported("Shutting down or restarting the PC"))
}
