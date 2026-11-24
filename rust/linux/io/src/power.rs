//! Sleep, shutdown and resume watching on Linux, through systemd-logind: `CanSuspend` and
//! `CanHibernate` give the sleep states, `Suspend`, `Reboot` and `PowerOff` (never interactive,
//! so no password prompt can hang the host) act, and `PrepareForSleep` reports the resume.

pub(crate) mod logind;

use anyhow::Result;
use butterpollo_session::power_action::SleepStates;
use tracing::{debug, warn};

/// The sleep states this PC reports. logind names no S-state, so a PC that can suspend is
/// reported with S3, which the console treats as Sleep PC being available; hibernate is
/// reported when logind allows it. Only `yes` counts: `challenge` would ask for a password.
pub fn sleep_states() -> Result<SleepStates> {
    let yes = |action| logind::can(action).is_ok_and(|answer| answer == "yes");
    Ok(SleepStates {
        s3: yes("Suspend"),
        hibernate: yes("Hibernate"),
        ..SleepStates::default()
    })
}

/// Put the PC to sleep and return after it resumes.
pub fn suspend() -> Result<()> {
    logind::call_then_wait_resume(|| logind::power_call("Suspend"))
}

/// A registration for resume from sleep, ended when it drops.
pub struct ResumeWatch {
    stopped: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for ResumeWatch {
    fn drop(&mut self) {
        // The listener sees it at the next signal and ends then.
        self.stopped
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Call `on_resume` whenever the PC resumes from sleep, until the returned watch drops
/// (`PrepareForSleep(false)` from logind).
pub fn on_resume(on_resume: impl Fn() + Send + Sync + 'static) -> Result<ResumeWatch> {
    use std::sync::{Arc, atomic::AtomicBool, atomic::Ordering};
    let signals = logind::manager()?.receive_signal("PrepareForSleep")?;
    let stopped = Arc::new(AtomicBool::new(false));
    let watch = ResumeWatch {
        stopped: stopped.clone(),
    };
    std::thread::spawn(move || {
        for message in signals {
            if stopped.load(Ordering::Relaxed) {
                break;
            }
            if logind::flag(&message) == Some(false) {
                debug!("the PC resumed from sleep");
                on_resume();
            }
        }
    });
    Ok(watch)
}

/// Restart or shut down this PC without closing programs by force: logind's `Reboot` or
/// `PowerOff`, not interactive, so programs holding a delay inhibitor still get their time and
/// one holding a block inhibitor is not overridden.
pub fn power_off(restart: bool) -> Result<()> {
    let method = if restart { "Reboot" } else { "PowerOff" };
    warn!("asking logind for {method}");
    logind::power_call(method)
}
