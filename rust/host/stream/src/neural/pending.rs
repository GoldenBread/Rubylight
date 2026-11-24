//! Deferred game-folder removals for disconnected drives, dropped library
//! games, and interrupted installs.
use super::{
    PENDING, PENDING_RETRY, Pending, Removed, SETTINGS_FILE, folder,
    game_check::placed_exes,
    install::{Removing, remove_at},
    record::{settings, update_settings},
};
use crate::state::Shared;
use anyhow::{Context, Result};
use butterpollo_session::{neural, state::App};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::PoisonError,
    time::Instant,
};

/// The record's removals, none when it has no list yet. A list that can't
/// be read is an error, never taken for empty and written over.
fn pending_in(record: &Value) -> Result<Vec<Pending>> {
    match &record[PENDING] {
        Value::Null => Ok(Vec::new()),
        list => serde_json::from_value(list.clone())
            .with_context(|| format!("{SETTINGS_FILE}'s pending removals are damaged")),
    }
}

/// [`pending_in`] for a reader, empty and logged when it can't be read.
pub fn pending(h: &Shared) -> Vec<Pending> {
    pending_in(&settings(h)).unwrap_or_else(|error| {
        tracing::warn!(error = %format!("{error:#}"), "Neural Rendering: its pending removals can't be read");
        Vec::new()
    })
}

/// Take the files placed beside `exe` out again (true), or leave their
/// removal pending when that fails (false); Err only when neither worked.
/// Blocks.
pub(super) fn take_back(h: &Shared, uuid: &str, name: &str, exe: &Path) -> Result<bool> {
    let why = match remove_at(name, exe) {
        Ok(Removed::Done(_)) => return Ok(true),
        Ok(Removed::Unreachable(folder)) => format!("{folder} can't be reached"),
        Err(error) => format!("{error:#}"),
    };
    let pending = Pending {
        name: name.into(),
        exe: exe.into(),
        uuid: Some(uuid.into()),
    };
    record_pending(h, std::slice::from_ref(&pending)).with_context(|| {
        format!(
            "frame generation could not be taken out of {} ({why}), and its removal could not be saved",
            exe.parent().unwrap_or(exe).display()
        )
    })?;
    tracing::warn!(app = %name, reason = %why, "Neural Rendering: taking the files out again failed; their removal is pending");
    Ok(false)
}

/// Drop the removal beside `exe` from the record: the files are meant to be
/// there, or nothing was written.
pub fn clear_pending(h: &Shared, exe: &Path) {
    let saved = update_settings(h, |value| {
        let mut list = pending_in(value)?;
        list.retain(|removal| removal.exe != exe);
        value[PENDING] = json!(list);
        Ok(())
    });
    if let Err(error) = saved {
        tracing::warn!(error = %format!("{error:#}"), "Neural Rendering: the pending removals could not be saved");
    }
}

/// Add removals to the record (once each).
pub fn record_pending(h: &Shared, removals: &[Pending]) -> Result<()> {
    update_settings(h, |value| {
        let mut list = pending_in(value)?;
        neural::merge_pending(&mut list, removals);
        value[PENDING] = json!(list);
        Ok(())
    })
}

/// The apps with files placed that a library change left out: an
/// uninstalled game the launcher sync dropped.
pub fn dropped(before: &[App], after: &[App]) -> Vec<App> {
    neural::dropped_apps(before, after)
}

/// Take the files out of the folders of apps the library sync dropped, as a
/// delete does. Each is recorded first; one that fails (a folder on a drive
/// that is not connected) stays recorded for [`remove_pending`]. Blocks.
pub fn remove_dropped(h: &Shared, apps: &[App]) {
    let removals: Vec<Pending> = apps.iter().flat_map(Pending::of).collect();
    if removals.is_empty() {
        return;
    }
    for removal in &removals {
        tracing::info!(app = %removal.name, "Neural Rendering: the library sync dropped the game; removing its files");
    }
    if let Err(error) = record_pending(h, &removals) {
        tracing::warn!(error = %format!("{error:#}"), "Neural Rendering: the pending removals could not be saved");
    }
    try_pending(h, &removals);
}

/// Whether the maintenance loop should try the pending removals again.
pub fn removal_due(h: &Shared) -> bool {
    {
        let mut state = h.neural.lock();
        if state.removal_at.is_some_and(|at| Instant::now() < at) {
            return false;
        }
        state.removal_at = Some(Instant::now() + PENDING_RETRY);
    }
    !pending(h).is_empty()
}

/// When the loop looks for pending removals again, for its idle sleep: the
/// retry cadence `removal_due` keeps, now, when none has been tried yet.
pub fn removal_due_at(h: &Shared) -> Instant {
    h.neural.lock().removal_at.unwrap_or_else(Instant::now)
}

/// Try the recorded removals again. Blocks.
pub fn remove_pending(h: &Shared) {
    try_pending(h, &[]);
}

/// Try the recorded removals and `also` (in case the record could not be
/// saved); keep the ones that are still to do.
pub fn try_pending(h: &Shared, also: &[Pending]) {
    let _one = h
        .neural
        .removing
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let mut removals = pending(h);
    neural::merge_pending(&mut removals, also);
    // The files an app has on stay: such a record is stale. The same folder
    // also when one path goes through a link (a game folder moved and linked
    // back) and the other doesn't.
    let wanted: Vec<PathBuf> = h
        .apps
        .read()
        .unwrap()
        .iter()
        .flat_map(placed_exes)
        .collect();
    let real = |exe: &Path| folder::real(exe.parent().unwrap_or(exe));
    let mut done = Vec::new();
    for removal in &removals {
        let stale = wanted.iter().any(|exe| {
            *exe == removal.exe || real(exe).is_some_and(|at| Some(at) == real(&removal.exe))
        });
        if stale {
            done.push(removal.exe.clone());
            continue;
        }
        let Ok(_slot) = Removing::take(h, removal.uuid.as_deref()) else {
            tracing::info!(app = %removal.name, "Neural Rendering is busy with the game; its removal stays pending");
            continue;
        };
        match remove_at(&removal.name, &removal.exe) {
            Ok(Removed::Done(_)) => done.push(removal.exe.clone()),
            Ok(Removed::Unreachable(folder)) => {
                tracing::info!(app = %removal.name, %folder, "Neural Rendering: the game's folder can't be reached; its removal stays pending");
            }
            Err(error) => {
                tracing::warn!(app = %removal.name, error = %format!("{error:#}"), "Neural Rendering: the removal stays pending");
            }
        }
    }
    if done.is_empty() && also.is_empty() {
        return;
    }
    let saved = update_settings(h, |value| {
        let mut list = pending_in(value)?;
        neural::merge_pending(&mut list, also);
        neural::retain_pending_not_done(&mut list, &done);
        value[PENDING] = json!(list);
        Ok(())
    });
    if let Err(error) = saved {
        tracing::warn!(error = %format!("{error:#}"), "Neural Rendering: the pending removals could not be saved");
    }
}
