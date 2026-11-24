//! Windows Advanced Color ICC associations, owned by the last streaming lease.
//! ABI: https://learn.microsoft.com/windows/win32/api/icm/nf-icm-colorprofilegetdisplaydefault

use crate::display::Monitor;
use anyhow::{Context, Result, bail};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use windows::{
    Win32::Foundation::{HLOCAL, LocalFree},
    core::HRESULT,
};

mod api;
mod calibration;

use api::Api;
pub use calibration::{Calibration, calibration_with, peak_luminance};
#[cfg(test)]
use calibration::{find_calibration, mhc2_peak};
/// A display's advanced-colour profile associations: mscms, or a fake in tests.
trait Associations {
    fn get(&self, monitor: &Monitor, system: bool) -> Result<Option<String>>;
    fn add(&self, monitor: &Monitor, system: bool, name: &str) -> Result<()>;
    fn remove(&self, monitor: &Monitor, system: bool, name: &str) -> Result<()>;
}
impl<T: Associations + ?Sized> Associations for &T {
    fn get(&self, monitor: &Monitor, system: bool) -> Result<Option<String>> {
        (**self).get(monitor, system)
    }
    fn add(&self, monitor: &Monitor, system: bool, name: &str) -> Result<()> {
        (**self).add(monitor, system, name)
    }
    fn remove(&self, monitor: &Monitor, system: bool, name: &str) -> Result<()> {
        (**self).remove(monitor, system, name)
    }
}
impl Associations for Api {
    fn get(&self, monitor: &Monitor, system: bool) -> Result<Option<String>> {
        // SAFETY: `pointer` receives a NUL-terminated name from LocalAlloc, read only up to its
        // terminator and freed once with LocalFree.
        unsafe {
            let mut pointer = std::ptr::null_mut();
            let status = HRESULT((self.get)(
                scope(system),
                monitor.adapter,
                monitor.source,
                0,
                8,
                &mut pointer,
            ));
            // Windows returns no association either as a null name or NOT_FOUND.
            if pointer.is_null() {
                if status.0 == 0x80070490u32 as i32 || status.0 == 0x80070002u32 as i32 {
                    return Ok(None);
                }
                status.ok()?;
                return Ok(None);
            }
            let result = (|| {
                status.ok()?;
                let length = (0..32768)
                    .find(|&n| *pointer.add(n) == 0)
                    .context("color profile name is too long")?;
                let name = String::from_utf16(std::slice::from_raw_parts(pointer, length))?;
                Ok((!name.is_empty()).then_some(name))
            })();
            let _ = LocalFree(Some(HLOCAL(pointer.cast())));
            result
        }
    }
    fn add(&self, monitor: &Monitor, system: bool, name: &str) -> Result<()> {
        let name = wide(name)?;
        // SAFETY: `name` is NUL-terminated and outlives the call, whose type matches the export.
        unsafe {
            HRESULT((self.add)(
                scope(system),
                name.as_ptr(),
                monitor.adapter,
                monitor.source,
                1,
                1,
            ))
            .ok()?;
        }
        Ok(())
    }
    fn remove(&self, monitor: &Monitor, system: bool, name: &str) -> Result<()> {
        let name = wide(name)?;
        // SAFETY: `name` is NUL-terminated and outlives the call, whose type matches the export.
        unsafe {
            HRESULT((self.remove)(
                scope(system),
                name.as_ptr(),
                monitor.adapter,
                monitor.source,
                1,
            ))
            .ok()?;
        }
        Ok(())
    }
}
fn scope(system: bool) -> u32 {
    if system { 0 } else { 1 }
}
fn wide(name: &str) -> Result<Vec<u16>> {
    if name.is_empty() || name.len() > 32767 || name.contains('\0') {
        bail!("invalid color profile name");
    }
    Ok(name.encode_utf16().chain(Some(0)).collect())
}
fn filename(selection: &str) -> Result<&str> {
    if selection.is_empty()
        || selection.contains(['/', '\\', ':', '\0'])
        || matches!(selection, "." | "..")
    {
        bail!("color profile must be an installed filename");
    }
    Ok(selection)
}
fn installed(selection: &str) -> Result<String> {
    let selection = filename(selection)?;
    let root = std::env::var_os("SystemRoot").context("Windows directory unavailable")?;
    let directory = PathBuf::from(root)
        .join("System32")
        .join("spool")
        .join("drivers")
        .join("color");
    let names = if Path::new(selection).extension().is_none() {
        vec![
            selection.into(),
            format!("{selection}.icm"),
            format!("{selection}.icc"),
        ]
    } else {
        vec![selection.into()]
    };
    names
        .into_iter()
        .find(|name| directory.join(name).is_file())
        .context("selected ICC profile is not installed")
}

struct State {
    users: usize,
    previous: Option<String>,
    applied: String,
    system: bool,
}
/// One slot per display. The map's lock only finds or makes a slot; the Windows colour calls
/// (the MHC round trip, which can block for seconds) and the journal write run under the one
/// display's own lock, so a stream on another display is not held up behind them. A display's
/// slot stays locked from deciding to lease it until its profile is applied, so two clients
/// cannot both lease the same display.
type Slot = Arc<Mutex<Option<State>>>;
static LEASES: Mutex<BTreeMap<String, Slot>> = Mutex::new(BTreeMap::new());
fn slot_for(id: &str) -> Slot {
    LEASES
        .lock()
        .unwrap()
        .entry(id.to_owned())
        .or_default()
        .clone()
}
/// Runs `work` on the display's lease state under the display's own lock only.
fn with_slot<T>(id: &str, work: impl FnOnce(&mut Option<State>) -> Result<T>) -> Result<T> {
    let slot = slot_for(id);
    let result = work(&mut slot.lock().unwrap());
    drop(slot);
    if result.is_err() {
        forget_slot(id);
    }
    result
}
/// Forgets the display's slot once nothing holds or waits for it.
fn forget_slot(id: &str) {
    let mut leases = LEASES.lock().unwrap();
    if leases
        .get(id)
        .is_some_and(|slot| Arc::strong_count(slot) == 1 && slot.lock().unwrap().is_none())
    {
        leases.remove(id);
    }
}
pub struct Lease {
    identity: String,
}
impl Lease {
    pub fn acquire(output: &str, selection: &str) -> Result<Self> {
        let applied = installed(selection)?;
        let monitor = crate::display::monitors()?
            .into_iter()
            .find(|m| m.display_name == output || m.device_id == output)
            .context("ICC output disappeared")?;
        let system = crate::desktop::is_system();
        with_slot(&monitor.device_id, |held| {
            acquire_state(
                held,
                &monitor,
                applied,
                system,
                Api::load,
                |before, applied| {
                    crate::display_recovery::profile(
                        &monitor.device_id,
                        &monitor.display_name,
                        before,
                        applied,
                        system,
                    )
                },
                || crate::display_recovery::pending(&monitor.device_id).profile,
                || crate::display_recovery::release_profile(&monitor.device_id),
            )
        })?;
        Ok(Self {
            identity: monitor.device_id,
        })
    }
    /// Makes this lease hold `selection` on `output` without letting go of what it holds
    /// first: a failure leaves the lease, its profile and its journal entry as they were.
    /// Taking the lease off before acquiring the new one left a stream that ran with no
    /// calibration and no journal entry when the acquisition failed.
    pub fn switch(&mut self, output: &str, selection: &str) -> Result<()> {
        let applied = installed(selection)?;
        let monitor = crate::display::monitors()?
            .into_iter()
            .find(|m| m.display_name == output || m.device_id == output)
            .context("ICC output disappeared")?;
        if monitor.device_id != self.identity {
            // Another display: its lease first, then this one's goes.
            *self = Self::acquire(output, selection)?;
            return Ok(());
        }
        let slot = slot_for(&self.identity);
        let mut held = slot.lock().unwrap();
        let state = held.as_mut().context("the HDR profile lease is gone")?;
        let api = Api::load()?;
        let system = state.system;
        switch_state(&api, &monitor, state, applied, |before, applied| {
            crate::display_recovery::profile(
                &monitor.device_id,
                &monitor.display_name,
                before,
                applied,
                system,
            )
        })
    }
}
/// Leases the display whose slot is `held`: another client's lease of the same profile is
/// shared, of another one refused; with none, `applied` is put on the display, journalled
/// before Windows changes. The caller holds the display's slot for the whole call.
#[allow(clippy::too_many_arguments)] // The Windows calls, each a closure so tests can stand in.
fn acquire_state<A: Associations>(
    held: &mut Option<State>,
    monitor: &Monitor,
    applied: String,
    system: bool,
    load: impl FnOnce() -> Result<A>,
    record: impl Fn(Option<String>, &str) -> Result<()>,
    pending: impl FnOnce() -> Option<Option<String>>,
    release: impl FnOnce() -> Result<()>,
) -> Result<()> {
    if let Some(state) = held {
        if !state.applied.eq_ignore_ascii_case(&applied) {
            bail!("another client owns a different ICC profile on this output");
        }
        state.users += 1;
        return Ok(());
    }
    let api = load()?;
    let current = api.get(monitor, system)?;
    let pending_entry = pending();
    let had_pending = pending_entry.is_some();
    let previous = previous_profile(pending_entry, current.clone());
    if !current
        .as_ref()
        .is_some_and(|name| name.eq_ignore_ascii_case(&applied))
    {
        record(current, &applied)?;
        // Preserve the recovery entry if a failed call partially changes Windows,
        // or if a prior crashed stream's entry was already pending.
        if let Err(error) = api.add(monitor, system, &applied) {
            if !had_pending {
                if restore_with(&api, monitor, previous.as_deref(), &applied, system).is_ok() {
                    let _ = release();
                }
            } else if let Ok(Some(left)) = api.get(monitor, system)
                && !left.eq_ignore_ascii_case(&applied)
            {
                let _ = record(previous, &left);
            }
            return Err(error);
        }
    }
    *held = Some(State {
        users: 1,
        previous,
        applied,
        system,
    });
    Ok(())
}
/// `state`'s profile becomes `applied` on `monitor`, journalled before Windows changes. A
/// lease another client shares keeps its profile. When the change fails, the profile the
/// display had and the journal entry that goes with it are put back.
fn switch_state(
    api: &impl Associations,
    monitor: &Monitor,
    state: &mut State,
    applied: String,
    record: impl Fn(Option<String>, &str) -> Result<()>,
) -> Result<()> {
    let current = api.get(monitor, state.system)?;
    let has = current
        .as_deref()
        .is_some_and(|name| name.eq_ignore_ascii_case(&applied));
    if state.applied.eq_ignore_ascii_case(&applied) && has {
        return Ok(());
    }
    if state.users > 1 && !state.applied.eq_ignore_ascii_case(&applied) {
        bail!("another client owns a different ICC profile on this output");
    }
    record(state.previous.clone(), &applied)?;
    if !has && let Err(error) = api.add(monitor, state.system, &applied) {
        let _ = restore_with(api, monitor, current.as_deref(), &applied, state.system);
        let _ = record(state.previous.clone(), &state.applied);
        return Err(error);
    }
    state.applied = applied;
    Ok(())
}
/// The profile to put back after streaming. A display that was away when an
/// earlier stream ended still has that stream's profile, and its journal
/// entry holds the original: that wins, or this lease's release would clear
/// the entry and leave the stream's profile on the display for good.
fn previous_profile(pending: Option<Option<String>>, current: Option<String>) -> Option<String> {
    pending.unwrap_or(current)
}
/// Used by the separate recovery process as well as normal lease teardown.
pub fn restore(
    monitor: &Monitor,
    previous: Option<&str>,
    applied: &str,
    system: bool,
) -> Result<()> {
    if previous.is_some_and(|name| name.eq_ignore_ascii_case(applied)) {
        return Ok(());
    }
    restore_with(&Api::load()?, monitor, previous, applied, system)
}
/// Puts `previous` back, or removes `applied`, unless the display has since
/// been given another profile.
fn restore_with(
    api: &impl Associations,
    monitor: &Monitor,
    previous: Option<&str>,
    applied: &str,
    system: bool,
) -> Result<()> {
    if previous.is_some_and(|name| name.eq_ignore_ascii_case(applied)) {
        return Ok(());
    }
    let current = api.get(monitor, system)?;
    if current
        .as_deref()
        .is_some_and(|current| current.eq_ignore_ascii_case(applied))
    {
        if let Some(previous) = previous {
            api.add(monitor, system, previous)?;
        } else {
            api.remove(monitor, system, applied)?;
        }
    } else {
        tracing::info!(
            output = %monitor.display_name,
            previous = previous.unwrap_or("none"),
            applied,
            current = current.as_deref().unwrap_or("none"),
            "leaving the HDR profile as it is; the display no longer has the profile the stream applied"
        );
    }
    Ok(())
}
impl Drop for Lease {
    fn drop(&mut self) {
        let slot = slot_for(&self.identity);
        let result = {
            let mut held = slot.lock().unwrap();
            let Some(state) = held.as_mut() else {
                return;
            };
            state.users -= 1;
            if state.users != 0 {
                return;
            }
            // Under the display's lock: a stream that wants the display meanwhile waits
            // and then finds it restored, not half way.
            let result = restore_lease(&self.identity, state);
            *held = None;
            result
        };
        drop(slot);
        forget_slot(&self.identity);
        if let Err(error) = result {
            tracing::warn!(%error, "HDR profile restoration deferred to recovery");
        }
    }
}
fn restore_lease(identity: &str, state: &State) -> Result<()> {
    // Windows keeps the profile with the display. One that is off or
    // unplugged now keeps its journal entry for the next recovery.
    let Some(monitor) = crate::display::monitors()?
        .into_iter()
        .find(|m| m.device_id == identity)
    else {
        tracing::info!(display = %identity, "display is gone; its colour profile is restored when it returns");
        return Ok(());
    };
    restore(
        &monitor,
        state.previous.as_deref(),
        &state.applied,
        state.system,
    )?;
    crate::display_recovery::release_profile(identity)
}

#[cfg(test)]
mod tests;
