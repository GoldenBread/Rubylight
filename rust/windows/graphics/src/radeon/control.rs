//! Writes, the journal and restore for Radeon settings (plan A5; the rules are in
//! `docs/native-api.md`, "Radeon state and writes").
//!
//! A write request is validated whole, its originals are journaled and synced to disk, then it is
//! applied in order. If the driver refuses one change, everything the request did is undone. The
//! driver's read-back is what comes back to the caller. Restore puts a journaled setting back only
//! while it still holds the value the host wrote.
//!
//! `RadeonBackend` is the seam to ADLX: `adlx::AdlxBackend` is the real one, and the tests use a
//! fake, so all of this runs without a GPU.
use super::{
    model::{self, Kind, Setting, State, Value},
    owner::{self, Owner},
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fmt,
    path::{Path, PathBuf},
    time::Instant,
};

mod state;
mod types;

pub use types::{
    BackendError, Change, Entry, ErrorKind, GpuIdentity, GpuInfo, JOURNAL_FILE, Journal, Outcome,
    RadeonBackend, RawSettingKeys, RestoreReport, RestoreResult, Scope, WriteError,
};
use types::{error, find, predicted_effects, switched_off_by, validate};

pub struct Controller<B> {
    backend: B,
    journal: PathBuf,
    /// Restore stops starting new work after this; what is left stays journaled.
    deadline: Option<Instant>,
    /// True for machine-global driver stores (NVIDIA DRS) that must restore on startup
    /// regardless of which Windows user is signed in.
    machine_scoped: bool,
    /// The SID of the user whose settings the helpers read and write: the signed-in user, `None`
    /// while nobody is.
    user: Box<dyn Fn() -> Option<String> + Send>,
}

impl<B: RadeonBackend> Controller<B> {
    pub fn new(backend: B, journal: PathBuf) -> Self {
        Self {
            backend,
            journal,
            deadline: None,
            machine_scoped: false,
            user: Box::new(|| crate::process_info::own_sid().ok()),
        }
    }
    /// Mark journal entries as machine-global rather than per-Windows-user.
    pub fn with_machine_scope(mut self, machine_scoped: bool) -> Self {
        self.machine_scoped = machine_scoped;
        self
    }
    /// Name the user another way (when running as SYSTEM or in tests).
    pub fn with_user(mut self, user: impl Fn() -> Option<String> + Send + 'static) -> Self {
        self.user = Box::new(user);
        self
    }
    pub fn set_deadline(&mut self, deadline: Option<Instant>) {
        self.deadline = deadline;
    }
    pub fn backend(&mut self) -> &mut B {
        &mut self.backend
    }
    pub fn journal(&self) -> Result<Journal> {
        Journal::load(&self.journal)
    }
    /// What would still be put back for the signed-in user (or whole machine when `machine_scoped`).
    pub fn pending(&self) -> Result<Vec<Entry>> {
        let user = (!self.machine_scoped).then(|| (self.user)()).flatten();
        Ok(self
            .journal()?
            .entries
            .into_iter()
            .filter(|e| self.machine_scoped || user.as_deref() == Some(e.user.as_str()))
            .collect())
    }

    /// Take the journal's mutex and load it, unless another live host owns settings in it. The
    /// lock stays held until the guard drops, so load, change and save are one step.
    fn open(&self) -> Result<(owner::JournalLock, Journal)> {
        let lock = owner::lock(&self.journal)?;
        let journal = Journal::load(&self.journal)?;
        if let Some(other) = journal.owner
            && !journal.entries.is_empty()
            && other.is_other_and_alive()
        {
            bail!(
                "another host (process {}) owns the Radeon journal and has settings changed",
                other.pid
            );
        }
        Ok((lock, journal))
    }

    /// Validate, journal, apply in order, and return the driver's read-back. Either all of
    /// `changes` took effect or none did.
    pub fn apply(
        &mut self,
        session: &str,
        gpu: i32,
        changes: &[Change],
    ) -> Result<Vec<Setting>, WriteError> {
        let unavailable = |e: BackendError| error(ErrorKind::Unavailable, None, e.0);
        let journal_error = |e: anyhow::Error| error(ErrorKind::Journal, None, format!("{e:#}"));
        let (_lock, saved) = self.open().map_err(journal_error)?;
        let user = if self.machine_scoped {
            "machine".to_string()
        } else {
            let Some(user) = (self.user)() else {
                return Err(error(
                    ErrorKind::Unavailable,
                    None,
                    "nobody is signed in, so there is no user whose settings these would be",
                ));
            };
            user
        };
        let before = self.backend.read(gpu).map_err(unavailable)?;
        validate(&before, changes)?;
        let mut journal = saved.clone();
        let original = |id: &str| find(&before, id).and_then(|s| s.value.clone());
        for change in changes {
            let orig = original(&change.id).ok_or_else(|| {
                error(
                    ErrorKind::Unavailable,
                    Some(&change.id),
                    "the value cannot be read",
                )
            })?;
            let raw = self
                .backend
                .snapshot_keys(gpu, &change.id)
                .map_err(unavailable)?;
            let mut entry = Entry::new(
                user.clone(),
                gpu,
                change.id.clone(),
                session.into(),
                orig,
                change.value.clone(),
                false,
            );
            entry.raw = raw;
            journal.record(entry);
        }
        // The partners the driver is expected to switch off go in too. If it does not switch one,
        // the entry is harmless: restore finds it already at its original.
        let predicted = predicted_effects(&before, changes);
        for partner in predicted.iter().cloned() {
            journal.record(Entry::new(
                user.clone(),
                gpu,
                partner,
                session.into(),
                Value::Bool(true),
                Value::Bool(false),
                true,
            ));
        }
        // The originals reach the disk before the first byte changes in the driver.
        journal.save(&self.journal).map_err(journal_error)?;

        let mut applied: Vec<&Change> = vec![];
        for change in changes {
            if let Err(e) = self.backend.write(gpu, &change.id, &change.value) {
                let failure = error(ErrorKind::DriverRejected, Some(&change.id), e.0);
                return Err(self.undo(
                    session, &user, gpu, &before, changes, &applied, &saved, failure,
                ));
            }
            applied.push(change);
        }
        let after = match self.backend.read(gpu) {
            Ok(after) => after,
            Err(e) => {
                let failure = error(ErrorKind::Unavailable, None, e.0);
                return Err(self.undo(
                    session, &user, gpu, &before, changes, &applied, &saved, failure,
                ));
            }
        };
        for change in changes {
            let now = find(&after, &change.id).and_then(|s| s.value.as_ref());
            if now != Some(&change.value) {
                let failure = error(
                    ErrorKind::DriverRejected,
                    Some(&change.id),
                    format!(
                        "wrote {} and the driver reads back {}",
                        change.value.display(),
                        now.map_or("nothing".into(), Value::display)
                    ),
                );
                return Err(self.undo(
                    session, &user, gpu, &before, changes, &applied, &saved, failure,
                ));
            }
        }
        // A setting the host can neither write nor restore must not have moved, except next to
        // what the request itself changed (vsync.enabled follows vsync.mode).
        let touched: HashSet<&str> = changes
            .iter()
            .filter_map(|c| find(&before, &c.id).map(|s| s.group))
            .collect();
        for setting in &before {
            if setting.state == State::Configurable || touched.contains(setting.group) {
                continue;
            }
            let now = find(&after, setting.id).and_then(|s| s.value.as_ref());
            if let (Some(was), Some(now)) = (&setting.value, now)
                && was != now
                && model::derived_from(setting.id).is_none()
            {
                let failure = error(
                    ErrorKind::DriverRejected,
                    Some(setting.id),
                    format!(
                        "the request switched {} from {} to {}, and the host cannot write or restore it",
                        setting.id,
                        was.display(),
                        now.display()
                    ),
                );
                return Err(self.undo(
                    session, &user, gpu, &before, changes, &applied, &saved, failure,
                ));
            }
        }
        // What the driver changed on its own is journaled too, so restore can undo it.
        let requested: HashSet<&str> = changes.iter().map(|c| c.id.as_str()).collect();
        let mut effects = false;
        for setting in &before {
            if setting.state != State::Configurable || requested.contains(setting.id) {
                continue;
            }
            let (Some(was), Some(now)) = (
                &setting.value,
                find(&after, setting.id).and_then(|s| s.value.as_ref()),
            ) else {
                continue;
            };
            if was != now {
                effects = true;
                journal.record(Entry::new(
                    user.clone(),
                    gpu,
                    setting.id.into(),
                    session.into(),
                    was.clone(),
                    now.clone(),
                    true,
                ));
            }
        }
        // A predicted partner the driver did not switch (as far as this read-back shows) stays
        // journaled too. The driver may apply the switch after the read, and a restore finds the
        // partner at its original, which costs nothing.
        let saved_again = if effects {
            journal.save(&self.journal)
        } else {
            Ok(())
        };
        if let Err(e) = saved_again {
            tracing::error!(
                error = format!("{e:#}"),
                "journal the driver's side effects; undoing the request"
            );
            let failure = error(
                ErrorKind::Journal,
                None,
                format!(
                    "the driver's side effects could not be journaled ({e:#}), so the request was undone"
                ),
            );
            return Err(self.undo(
                session, &user, gpu, &before, changes, &applied, &saved, failure,
            ));
        }
        Ok(after)
    }

    /// Put every setting back to `before` after a failed request and say what it could not. What
    /// stays changed is journaled with the value it holds now, so a restore can still act on it.
    #[allow(clippy::too_many_arguments)]
    fn undo(
        &mut self,
        session: &str,
        user: &str,
        gpu: i32,
        before: &[Setting],
        changes: &[Change],
        applied: &[&Change],
        saved: &Journal,
        mut failure: WriteError,
    ) -> WriteError {
        for change in applied.iter().rev() {
            if let Some(value) = find(before, &change.id).and_then(|s| s.value.as_ref()) {
                let _ = self.backend.write(gpu, &change.id, value);
            }
        }
        // The driver may have switched other settings as a consequence; those go back too.
        if let Ok(now) = self.backend.read(gpu) {
            for setting in before.iter().filter(|s| s.state == State::Configurable) {
                let current = find(&now, setting.id).and_then(|s| s.value.as_ref());
                if let (Some(was), Some(current)) = (&setting.value, current)
                    && was != current
                {
                    let _ = self.backend.write(gpu, setting.id, was);
                }
            }
        }
        match self.backend.read(gpu) {
            Ok(now) => {
                let stuck: Vec<&Setting> = before
                    .iter()
                    .filter(|s| s.state == State::Configurable)
                    .filter(|s| s.value.as_ref() != find(&now, s.id).and_then(|n| n.value.as_ref()))
                    .collect();
                if stuck.is_empty() {
                    if let Err(e) = saved.save(&self.journal) {
                        tracing::error!(error = format!("{e:#}"), "reset the Radeon journal");
                    }
                    return failure;
                }
                let mut journal = saved.clone();
                for setting in &stuck {
                    let Some(was) = &setting.value else {
                        continue;
                    };
                    let requested = changes.iter().find(|c| c.id == setting.id);
                    let current = find(&now, setting.id)
                        .and_then(|n| n.value.clone())
                        .or_else(|| requested.map(|c| c.value.clone()))
                        .unwrap_or_else(|| was.clone());
                    journal.record(Entry::new(
                        user.into(),
                        gpu,
                        setting.id.into(),
                        session.into(),
                        was.clone(),
                        current,
                        requested.is_none(),
                    ));
                }
                failure.message = format!(
                    "{}; could not put back {}, which stays in the journal for the next restore",
                    failure.message,
                    stuck.iter().map(|s| s.id).collect::<Vec<_>>().join(", ")
                );
                if let Err(e) = journal.save(&self.journal) {
                    tracing::error!(
                        error = format!("{e:#}"),
                        "journal the settings the undo could not put back"
                    );
                    failure.kind = ErrorKind::Journal;
                    failure.message = format!(
                        "{}; the journal could not be updated either ({e:#})",
                        failure.message
                    );
                }
            }
            Err(e) => {
                // Nothing to compare with: the journal on disk still holds every change of the
                // request, so a restore finds them.
                tracing::error!(error = %e, "Radeon undo could not read the driver back");
                failure.message = format!(
                    "{}; the driver could not be read to check the undo, so the request stays in the journal",
                    failure.message
                );
            }
        }
        failure
    }

    /// Put journaled settings back. A setting is only touched while it still holds the value the
    /// host wrote; anything else was changed on the PC and is left alone. Entries that fail stay
    /// in the journal.
    pub fn restore(&mut self, scope: Scope) -> Result<RestoreReport> {
        self.restore_entries(scope, false)
    }

    /// For a host start: restore whatever a crashed run left in the journal. A setting that reads
    /// as already original gets the original written once more, which changes nothing when the
    /// read was right and covers a write that had not become visible yet.
    pub fn recover_on_startup(&mut self) -> Result<RestoreReport> {
        self.restore_entries(Scope::All, true)
    }

    fn expired(&self) -> bool {
        self.deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
    }

    fn restore_entries(&mut self, scope: Scope, startup: bool) -> Result<RestoreReport> {
        let (_lock, mut journal) = self.open()?;
        let selected = |e: &Entry| match scope {
            Scope::All => true,
            Scope::Session(session) => e.session == session,
            Scope::Settings(session, ids) => {
                e.session == session
                    && (ids.contains(&e.id) || (e.side_effect && switched_off_by(ids, &e.id)))
            }
        };
        // Machine-global stores belong to the machine; per-user stores only restore the
        // signed-in user's entries.
        let user = (!self.machine_scoped).then(|| (self.user)()).flatten();
        let (mine, others): (Vec<&Entry>, Vec<&Entry>) = if self.machine_scoped {
            (
                journal.entries.iter().filter(|e| selected(e)).collect(),
                Vec::new(),
            )
        } else {
            journal
                .entries
                .iter()
                .filter(|e| selected(e))
                .partition(|e| user.as_deref() == Some(e.user.as_str()))
        };
        if !others.is_empty() {
            tracing::error!(
                settings = %others.iter().map(|e| e.id.as_str()).collect::<Vec<_>>().join(", "),
                owners = %others.iter().map(|e| e.user.as_str()).collect::<std::collections::BTreeSet<_>>().into_iter().collect::<Vec<_>>().join(", "),
                signed_in = user.as_deref().unwrap_or("nobody"),
                "Radeon journal entries belong to another user; left in place until that user is signed in"
            );
        }
        let deferred_other_users = others.len();
        // Requested changes first, newest first; then what the driver switched as a consequence.
        let mut order: Vec<Entry> = mine.into_iter().cloned().collect();
        order.reverse();
        order.sort_by_key(|e| e.side_effect);
        let mut results: Vec<(Entry, RestoreResult)> = vec![];
        let mut pending = order;
        for pass in 0..2 {
            let mut failed = vec![];
            let mut follow_up = vec![];
            let mut extra_failed = vec![];
            for entry in pending {
                let entry = journal
                    .entries
                    .iter()
                    .find(|e| e.is(&entry.user, entry.gpu, &entry.id))
                    .cloned()
                    .unwrap_or(entry);
                let (result, mut extra, mut sticky_failures) = if self.expired() {
                    (
                        RestoreResult::Failed("the restore time limit passed".into()),
                        vec![],
                        vec![],
                    )
                } else {
                    self.restore_one(&mut journal, &entry, startup)
                };
                if matches!(result, RestoreResult::Failed(_)) {
                    failed.push(entry.clone());
                }
                follow_up.append(&mut extra);
                extra_failed.append(&mut sticky_failures);
                results.retain(|(e, _)| !e.is(&entry.user, entry.gpu, &entry.id));
                results.push((entry, result));
            }
            for (extra_entry, message) in extra_failed {
                results.retain(|(e, _)| !e.is(&extra_entry.user, extra_entry.gpu, &extra_entry.id));
                results.push((extra_entry, RestoreResult::Failed(message)));
            }
            for extra_entry in follow_up {
                if failed
                    .iter()
                    .any(|e| e.is(&extra_entry.user, extra_entry.gpu, &extra_entry.id))
                {
                    continue;
                }
                if self
                    .backend
                    .read_value(extra_entry.gpu, &extra_entry.id)
                    .ok()
                    .as_ref()
                    == Some(&extra_entry.original)
                {
                    continue;
                }
                if pass == 1 {
                    // Final pass: temporary switches that could not be reverted must
                    // stay journaled and mark the report unclean so the worker retries.
                    results.retain(|(e, _)| {
                        !e.is(&extra_entry.user, extra_entry.gpu, &extra_entry.id)
                    });
                    results.push((
                        extra_entry,
                        RestoreResult::Failed(
                            "temporary feature switch could not be restored".into(),
                        ),
                    ));
                } else {
                    failed.push(extra_entry);
                }
            }
            pending = failed;
            if pending.is_empty() {
                break;
            }
        }
        let mut report = RestoreReport {
            deferred_other_users,
            ..RestoreReport::default()
        };
        let mut changed = false;
        for (entry, result) in results {
            match &result {
                RestoreResult::Failed(message) => tracing::error!(
                    setting = %entry.id,
                    original = %entry.original.display(),
                    error = %message,
                    "Radeon setting could not be restored; it stays in the journal"
                ),
                RestoreResult::ChangedElsewhere { current, written } => tracing::warn!(
                    setting = %entry.id,
                    original = %entry.original.display(),
                    current = %current.display(),
                    written = %written.display(),
                    "Radeon setting was changed on the PC since the host wrote it; leaving it"
                ),
                RestoreResult::Restored => tracing::info!(
                    setting = %entry.id,
                    value = %entry.original.display(),
                    "Radeon setting restored"
                ),
                RestoreResult::AlreadyOriginal => {}
            }
            if !matches!(result, RestoreResult::Failed(_)) {
                journal
                    .entries
                    .retain(|e| !e.is(&entry.user, entry.gpu, &entry.id));
                changed = true;
            }
            report.outcomes.push(Outcome {
                gpu: entry.gpu,
                id: entry.id,
                result,
            });
        }
        if changed {
            journal.save(&self.journal)?;
        }
        Ok(report)
    }

    fn restore_one(
        &mut self,
        journal: &mut Journal,
        entry: &Entry,
        startup: bool,
    ) -> (RestoreResult, Vec<Entry>, Vec<(Entry, String)>) {
        let now = match self.backend.read_value(entry.gpu, &entry.id) {
            Ok(now) => now,
            Err(e) => return (RestoreResult::Failed(e.0), vec![], vec![]),
        };
        if now == entry.original {
            if startup {
                match self.backend.restore_raw(entry) {
                    Ok(true) => {}
                    Ok(false) => {
                        if let Err(e) = self.backend.write(entry.gpu, &entry.id, &entry.original) {
                            tracing::debug!(setting = %entry.id, error = %e, "startup write-back of the original");
                        }
                    }
                    Err(e) => {
                        tracing::debug!(setting = %entry.id, error = %e, "startup raw write-back of the original");
                    }
                }
            }
            return (RestoreResult::AlreadyOriginal, vec![], vec![]);
        }
        if now != entry.written {
            return (
                RestoreResult::ChangedElsewhere {
                    current: now,
                    written: entry.written.clone(),
                },
                vec![],
                vec![],
            );
        }
        if let Some(toggle) = model::feature_toggle(&entry.id) {
            match self.backend.read_value(entry.gpu, &toggle) {
                Ok(Value::Bool(false)) => return self.restore_through(journal, entry, &toggle),
                Ok(_) => {}
                Err(e) => return (RestoreResult::Failed(e.0), vec![], vec![]),
            }
        }
        let result = match self.put_back(entry) {
            Ok(()) => RestoreResult::Restored,
            Err(message) => RestoreResult::Failed(message),
        };
        (result, vec![], vec![])
    }

    /// Write the original and check the read-back.
    fn put_back(&mut self, entry: &Entry) -> Result<(), String> {
        if self.backend.restore_raw(entry).map_err(|e| e.0)? {
            return Ok(());
        }
        self.backend
            .write(entry.gpu, &entry.id, &entry.original)
            .map_err(|e| e.0)?;
        let value = self
            .backend
            .read_value(entry.gpu, &entry.id)
            .map_err(|e| e.0)?;
        if value != entry.original {
            return Err(format!(
                "wrote {} and the driver reads back {}",
                entry.original.display(),
                value.display()
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
