//! When changed driver settings go back, and the one setting streams share: Vertical sync forced
//! on while a stream shows Rubylight's virtual display (`nvidia_virtual_display_vsync`).
//!
//! The forced Vertical sync is journaled under [`VSYNC_SESSION`] instead of a device, so it stays
//! until the last stream holding it is gone, and the same worker, journal and startup recovery
//! put it back as every other driver setting.
//!
//! After Foundation Sunshine's `nvenc_vdd_force_vsync` (AlkaidLab/foundation-sunshine commit
//! 04d9188, PR #1155, GPL-3.0).
use super::{Inner, Message, READ_WAIT, locked};
use butterpollo_media_policy::radeon_profile;
use butterpollo_session::state::App;
use butterpollo_win_graphics::radeon::{
    Change, Driver, RestoreReport, control::RestoreResult, model,
};
use std::{collections::BTreeSet, sync::mpsc, time::Duration};

/// The journal's session id of the forced Vertical sync; no device has it.
pub const VSYNC_SESSION: &str = "virtual-display-vsync";
const VSYNC: &str = "nvidia_vsync.mode";

/// Whether a launch of `app` at `fps` forces Vertical sync on: the host setting is on, the stream
/// shows the virtual display and the game's own driver preset has no Vertical sync value.
pub fn wants_vsync(
    enabled: bool,
    virtual_display: bool,
    app: Option<&App>,
    fps: Option<u32>,
) -> bool {
    enabled
        && virtual_display
        && !app
            .and_then(|app| radeon_profile::selected(&app.extra, fps).ok().flatten())
            .map(|selected| selected.for_driver(true))
            .is_some_and(|selected| selected.changes.iter().any(|item| item.id == VSYNC))
}

/// `holding`: whether a device that holds the forced Vertical sync is still there.
fn gone_from(session: &str, present: &BTreeSet<String>, holding: bool) -> bool {
    if session == VSYNC_SESSION {
        !holding
    } else {
        !present.contains(session)
    }
}

/// A launch's hold on the forced Vertical sync: dropped without [`VsyncHold::keep`] (the launch
/// failed), a hold the launch took is given up again.
#[must_use]
pub struct VsyncHold<'a> {
    inner: &'a Inner,
    taken: Option<String>,
}
impl VsyncHold<'_> {
    pub fn keep(mut self) {
        self.taken = None;
    }
}
impl Drop for VsyncHold<'_> {
    fn drop(&mut self) {
        if let Some(device) = self.taken.take() {
            self.inner.release_vsync(&device);
        }
    }
}

impl Inner {
    /// A stream of `device` launches or resumes: with `wanted`, force Vertical sync on before its
    /// game starts and hold it for the device; otherwise give up an earlier hold. `Err` says why
    /// it is not forced. Nothing happens on a Radeon host.
    ///
    /// `replaced` says whether this launch replaces the session whose own Vertical sync value
    /// stands in the way: that value goes back only after the game started, so the hold is kept
    /// for [`Inner::after_restore`] and `Err` says the game needs a restart.
    pub fn hold_vsync(
        &self,
        device: &str,
        adapter: &str,
        wanted: bool,
        replaced: impl Fn(&str) -> bool,
    ) -> (VsyncHold<'_>, Result<(), String>) {
        let mut hold = VsyncHold {
            inner: self,
            taken: None,
        };
        if !wanted || self.driver() != Driver::Nvidia {
            self.release_vsync(device);
            return (hold, Ok(()));
        }
        adapter.clone_into(&mut locked(&self.vsync_adapter));
        {
            let mut holders = locked(&self.vsync_holders);
            if holders.is_empty() {
                *locked(&self.vsync_original) = None;
            }
            if holders.insert(device.to_owned()) {
                hold.taken = Some(device.to_owned());
            }
        }
        let result = match self.force_vsync(adapter, Some(&replaced)) {
            Ok(Delay::None) => Ok(()),
            Ok(Delay::UntilRestart) => Err(
                "an earlier session's value was still in place when the game \
                             started; it applies after a game restart"
                    .into(),
            ),
            Ok(Delay::UntilRelaunch) => Err(
                "an earlier session's Vertical sync value could not be put back yet, so it \n                 was not forced; relaunch the game from the client once it is"
                    .into(),
            ),
            Err(error) => {
                // A hold the device already had stays: its running stream still needs it.
                if let Some(device) = hold.taken.take() {
                    self.release_vsync(&device);
                }
                Err(error)
            }
        };
        (hold, result)
    }

    /// `replaced` as in [`Inner::hold_vsync`] at a launch, `None` after a restore. A
    /// [`Delay`] other than `None`: not forced, because the value of a session being
    /// replaced is still there.
    fn force_vsync(
        &self,
        adapter: &str,
        replaced: Option<&dyn Fn(&str) -> bool>,
    ) -> Result<Delay, String> {
        let worker = self
            .worker()
            .ok_or("this host has no driver settings controls")?;
        let panel = worker
            .panel(Some(adapter), READ_WAIT)
            .map_err(|error| format!("the NVIDIA driver could not be read: {error}"))?
            .ok_or("this PC has no NVIDIA GPU")?;
        let mut leftover = None;
        if let Some(entry) = panel.pending.iter().find(|entry| entry.id == VSYNC) {
            // A device's preset or its own choice in the stream menu is not forced over;
            // [`Inner::after_restore`] forces again once that value is put back.
            if entry.session != VSYNC_SESSION {
                locked(&self.vsync_original).get_or_insert_with(|| entry.original.clone());
                tracing::info!("a device's own Vertical sync value stays");
                if !replaced.is_some_and(|replaced| replaced(&entry.session)) {
                    return Ok(Delay::None);
                }
                // A failed restore is retried by the worker, which forces nothing again.
                let failed = locked(&self.failures)
                    .get(&entry.session)
                    .is_some_and(|list| list.iter().any(|(id, _)| id == VSYNC || id == "*"));
                return Ok(if failed {
                    Delay::UntilRelaunch
                } else {
                    Delay::UntilRestart
                });
            }
            // Already forced: a change made on the PC since then is the user's and stays.
            if self.holds(VSYNC_SESSION) {
                return Ok(Delay::None);
            }
            // The force's own entry outlived its holders (its restore failed or still
            // waits): the PC's value is the one it recorded, not the On still in place.
            leftover = Some(entry.original.clone());
        }
        let setting = panel
            .settings
            .iter()
            .find(|setting| setting.id == VSYNC)
            .ok_or("the driver lists no Vertical sync setting")?;
        if setting.state != model::State::Configurable {
            tracing::info!("Vertical sync holds a custom value; it is left as it is");
            return Ok(Delay::None);
        }
        // After a restore, force only over the PC value the force replaced or waited for:
        // another one is a change made on the PC since, and it stays.
        if replaced.is_none()
            && locked(&self.vsync_original)
                .as_ref()
                .is_some_and(|original| setting.value.as_ref() != Some(original))
        {
            tracing::info!("Vertical sync was changed on the PC; it stays");
            return Ok(Delay::None);
        }
        match leftover {
            Some(original) => *locked(&self.vsync_original) = Some(original),
            None => locked(&self.vsync_original).clone_from(&setting.value),
        }
        let on = Change {
            id: VSYNC.into(),
            value: model::Value::Choice("on".into()),
        };
        let applied = self.apply(VSYNC_SESSION, panel.gpu.id, vec![on]);
        self.forget();
        applied.map_err(|error| error.to_string())?;
        tracing::info!("Vertical sync forced on while streaming to the virtual display");
        Ok(Delay::None)
    }

    /// `device` no longer needs the forced Vertical sync; the last one to go puts it back.
    pub fn release_vsync(&self, device: &str) {
        // One step under both locks (`written`, then the holders, as in `sync`): a launch that
        // takes a hold meanwhile finds either the old force or none, and its own comes after
        // this restore.
        let mut written = locked(&self.written);
        let mut holders = locked(&self.vsync_holders);
        if holders.remove(device) && holders.is_empty() && written.remove(VSYNC_SESSION) {
            self.dispatch(VSYNC_SESSION.to_owned());
        }
    }

    /// A restore of `device` ran: when it put back a Vertical sync value that stood in the way
    /// of the force and streams still hold it, force again, unless that value was set on the PC
    /// while the force was held.
    pub(super) fn after_restore(&self, device: &str, result: &anyhow::Result<RestoreReport>) {
        let restored = device != VSYNC_SESSION
            && result.as_ref().is_ok_and(|report| {
                report.outcomes.iter().any(|outcome| {
                    outcome.id == VSYNC
                        && matches!(
                            outcome.result,
                            RestoreResult::Restored | RestoreResult::AlreadyOriginal
                        )
                })
            });
        if !restored || locked(&self.vsync_holders).is_empty() {
            return;
        }
        let adapter = locked(&self.vsync_adapter).clone();
        if let Err(error) = self.force_vsync(&adapter, None) {
            tracing::error!(%error, "Vertical sync is not forced on again for the virtual display");
        }
        // The last holder may have left while it was written.
        let mut written = locked(&self.written);
        if locked(&self.vsync_holders).is_empty() && written.remove(VSYNC_SESSION) {
            self.dispatch(VSYNC_SESSION.to_owned());
        }
    }

    /// The session machine moved. `present` makes the devices still part of the session (a
    /// stream, the owner, a retained display), or `None` while the session has not settled (a
    /// launch or a teardown runs): nothing is restored then, because a launch that fails hands
    /// the session back to its owner. Every device that wrote settings and is not present gets
    /// them restored: that is an ended session, whichever way it ended. A suspended session
    /// keeps its owner, so Disconnect restores nothing. `launching` makes the devices whose
    /// launch is pending: they keep their hold on the forced Vertical sync. Neither is called
    /// when no device wrote anything.
    pub fn sync(
        &self,
        launching: impl FnOnce() -> BTreeSet<String>,
        present: impl FnOnce() -> Option<BTreeSet<String>>,
    ) {
        let mut written = locked(&self.written);
        if written.is_empty() {
            return;
        }
        let mut holders = locked(&self.vsync_holders);
        // Pending first: a launch that connects in between is then in `present`.
        let launching = launching();
        let Some(present) = present() else {
            return;
        };
        holders.retain(|device| present.contains(device) || launching.contains(device));
        let ended: Vec<String> = written
            .iter()
            .filter(|device| gone_from(device, &present, !holders.is_empty()))
            .cloned()
            .collect();
        for device in ended {
            written.remove(&device);
            self.dispatch(device);
        }
    }
    /// Hand on the restore of every device that changed settings and is not in `present`, and
    /// wait (at most `wait`) until those, and any restore already handed on, are done. For a
    /// launch that takes the game over: the new game's settings are written only after the old
    /// ones are gone, so a late restore cannot undo them and the journal's originals are the
    /// PC's own. `launching` as in [`Inner::sync`].
    pub fn release_missing(
        &self,
        launching: impl FnOnce() -> BTreeSet<String>,
        present: impl FnOnce() -> BTreeSet<String>,
        wait: Duration,
    ) {
        {
            let mut written = locked(&self.written);
            let mut holders = locked(&self.vsync_holders);
            let launching = launching();
            let present = present();
            // Before dispatching: a device's restore must not force again for a holder that
            // is gone.
            holders.retain(|device| present.contains(device) || launching.contains(device));
            let gone: Vec<String> = written
                .iter()
                .filter(|device| gone_from(device, &present, !holders.is_empty()))
                .cloned()
                .collect();
            for device in gone {
                written.remove(&device);
                self.dispatch(device);
            }
        }
        // Also when nothing new was handed on: a session that ended a moment ago may have its
        // restore queued or running, and the new write must not overtake it.
        if !self.flush(wait) {
            tracing::error!(
                wait_seconds = wait.as_secs(),
                "the Radeon settings of the earlier session were still being put back; the new game's settings follow them in the worker's queue"
            );
        }
    }
    /// A device was unpaired or turned off: its settings go back even though its session may
    /// linger until the grace period ends.
    pub fn release_device(&self, device: &str) {
        self.release_vsync(device);
        if locked(&self.written).remove(device) {
            self.dispatch(device.to_owned());
        }
    }
    /// Every device's settings go back, whatever its session is doing (unpair all, shutdown).
    pub fn release_all(&self) {
        locked(&self.vsync_holders).clear();
        let devices: Vec<String> = std::mem::take(&mut *locked(&self.written))
            .into_iter()
            .collect();
        for device in devices {
            self.dispatch(device);
        }
    }
    /// A clean shutdown: put back everything this run changed and wait for it, at most `wait`.
    pub fn shutdown(&self, wait: Duration) {
        self.release_all();
        let Some(restorer) = self.restorer.get() else {
            return;
        };
        let (ack, done) = mpsc::channel();
        if restorer.send(Message::Flush(ack)).is_ok() && done.recv_timeout(wait).is_err() {
            tracing::error!(
                wait_seconds = wait.as_secs(),
                "Radeon settings were still being restored at shutdown; what is left stays journaled and the next host start restores it"
            );
        }
    }
}

/// Why [`Inner::force_vsync`] left Vertical sync alone at a launch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Delay {
    /// Forced, already forced, or nothing to force.
    None,
    /// A replaced session's value is still there; it is put back and forced after the launch.
    UntilRestart,
    /// A replaced session's value could not be put back; the worker's retry forces nothing.
    UntilRelaunch,
}

impl crate::state::Host {
    /// [`Inner::hold_vsync`] for a launch or resume of `device`. It replaces the device's own
    /// earlier preset, and the session of a device that neither streams nor launches.
    pub fn hold_vsync(
        &self,
        device: &str,
        adapter: &str,
        wanted: bool,
    ) -> (VsyncHold<'_>, Result<(), String>) {
        self.radeon.hold_vsync(device, adapter, wanted, |owner| {
            if owner == device {
                // A preset is put back after the launch; a choice in the stream menu stays.
                return self
                    .notes
                    .profile(device)
                    .is_some_and(|held| held.wanted.iter().any(|item| item.id == VSYNC));
            }
            let sessions = locked(&self.sessions);
            !sessions.streaming_devices().contains(owner)
                && !sessions.launching_devices().contains(owner)
        })
    }
}

#[cfg(test)]
mod tests;
