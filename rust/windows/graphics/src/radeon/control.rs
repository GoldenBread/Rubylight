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
    BackendError, Change, Entry, ErrorKind, GpuInfo, JOURNAL_FILE, Journal, Outcome, RadeonBackend,
    RestoreReport, RestoreResult, Scope, WriteError,
};
use types::{error, find, predicted_effects, switched_off_by, validate};

pub struct Controller<B> {
    backend: B,
    journal: PathBuf,
    /// Restore stops starting new work after this; what is left stays journaled.
    deadline: Option<Instant>,
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
            user: Box::new(|| crate::process_info::own_sid().ok()),
        }
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
    /// What would still be put back for the signed-in user: the journal's entries of their
    /// settings. Another user's entries are not this user's to show or restore.
    pub fn pending(&self) -> Result<Vec<Entry>> {
        let user = (self.user)();
        Ok(self
            .journal()?
            .entries
            .into_iter()
            .filter(|e| user.as_deref() == Some(e.user.as_str()))
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
        let Some(user) = (self.user)() else {
            return Err(error(
                ErrorKind::Unavailable,
                None,
                "nobody is signed in, so there is no user whose settings these would be",
            ));
        };
        let before = self.backend.read(gpu).map_err(unavailable)?;
        validate(&before, changes)?;
        let mut journal = saved.clone();
        let original = |id: &str| find(&before, id).and_then(|s| s.value.clone());
        for change in changes {
            journal.record(Entry {
                user: user.clone(),
                gpu,
                id: change.id.clone(),
                session: session.into(),
                original: original(&change.id).ok_or_else(|| {
                    error(
                        ErrorKind::Unavailable,
                        Some(&change.id),
                        "the value cannot be read",
                    )
                })?,
                written: change.value.clone(),
                side_effect: false,
            });
        }
        // The partners the driver is expected to switch off go in too. If it does not switch one,
        // the entry is harmless: restore finds it already at its original.
        let predicted = predicted_effects(&before, changes);
        for partner in predicted.iter().cloned() {
            journal.record(Entry {
                user: user.clone(),
                gpu,
                id: partner,
                session: session.into(),
                original: Value::Bool(true),
                written: Value::Bool(false),
                side_effect: true,
            });
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
                journal.record(Entry {
                    user: user.clone(),
                    gpu,
                    id: setting.id.into(),
                    session: session.into(),
                    original: was.clone(),
                    written: now.clone(),
                    side_effect: true,
                });
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
                    journal.record(Entry {
                        user: user.into(),
                        gpu,
                        id: setting.id.into(),
                        session: session.into(),
                        original: was.clone(),
                        written: current,
                        side_effect: requested.is_none(),
                    });
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
        // Only the signed-in user's entries: another user's values (or the defaults of a view
        // that is not theirs) must never be compared with or written into this user's settings.
        let user = (self.user)();
        let (mine, others): (Vec<&Entry>, Vec<&Entry>) = journal
            .entries
            .iter()
            .filter(|e| selected(e))
            .partition(|e| user.as_deref() == Some(e.user.as_str()));
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
            if startup && let Err(e) = self.backend.write(entry.gpu, &entry.id, &entry.original) {
                // It reads as the original, and the write is only for safety.
                tracing::debug!(setting = %entry.id, error = %e, "startup write-back of the original");
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

    /// The driver takes the field's value only while its feature is on, and the feature is off
    /// now: switch it on (journaled first, with whatever that switches off), put the field back,
    /// then put the feature back.
    fn restore_through(
        &mut self,
        journal: &mut Journal,
        entry: &Entry,
        toggle: &str,
    ) -> (RestoreResult, Vec<Entry>, Vec<(Entry, String)>) {
        let group = toggle.strip_suffix(".enabled").unwrap_or(toggle);
        let partners: Vec<String> = model::excluded_by(group)
            .map(|other| format!("{other}.enabled"))
            .filter(|id| {
                matches!(
                    self.backend.read_value(entry.gpu, id),
                    Ok(Value::Bool(true))
                )
            })
            .collect();
        let mut temporary: Vec<String> = vec![];
        // Existing journal entries whose `written` is temporarily updated to match the
        // driver state while the feature is toggled, so a failure or crash cannot make a
        // subsequent restore misclassify our temporary switch as `ChangedElsewhere`.
        let mut saved_written: Vec<(String, Value, Value)> = vec![];
        let wanted: Vec<(String, bool, bool)> = std::iter::once((toggle.to_string(), false, true))
            .chain(partners.iter().map(|id| (id.clone(), true, false)))
            .collect();
        for (id, original, written) in &wanted {
            if let Some(existing) = journal
                .entries
                .iter_mut()
                .find(|e| e.is(&entry.user, entry.gpu, id))
            {
                saved_written.push((id.clone(), existing.written.clone(), Value::Bool(*original)));
                existing.written = Value::Bool(*written);
                continue;
            }
            temporary.push(id.clone());
            journal.entries.push(Entry {
                user: entry.user.clone(),
                gpu: entry.gpu,
                id: id.clone(),
                session: entry.session.clone(),
                original: Value::Bool(*original),
                written: Value::Bool(*written),
                side_effect: true,
            });
        }
        let restore_saved_written = |journal: &mut Journal, only_ids: Option<&[String]>| {
            for (id, previous_written, _) in &saved_written {
                if only_ids.is_some_and(|ids| !ids.iter().any(|wanted_id| wanted_id == id)) {
                    continue;
                }
                if let Some(existing) = journal
                    .entries
                    .iter_mut()
                    .find(|e| e.is(&entry.user, entry.gpu, id))
                {
                    existing.written = previous_written.clone();
                }
            }
        };
        let rollback = |journal: &mut Journal| {
            journal.entries.retain(|e| {
                !(e.user == entry.user && e.gpu == entry.gpu && temporary.contains(&e.id))
            });
            restore_saved_written(journal, None);
        };
        if let Err(e) = journal.save(&self.journal) {
            rollback(journal);
            return (
                RestoreResult::Failed(format!(
                    "journaling the temporary switch of {toggle}: {e:#}"
                )),
                vec![],
                vec![],
            );
        }
        match self.through_toggle(entry, toggle, &partners) {
            Ok(()) => {
                rollback(journal);
                if let Err(e) = journal.save(&self.journal) {
                    tracing::warn!(
                        error = format!("{e:#}"),
                        "drop the temporary Radeon entries"
                    );
                }
                (RestoreResult::Restored, vec![], vec![])
            }
            Err((message, false)) => {
                // `entry` failed to restore, but the temporary switch itself was reverted (or
                // never flipped). Restore pre-existing entries' `written` values while leaving
                // temporary entries journaled for the next recovery pass.
                if !saved_written.is_empty() {
                    restore_saved_written(journal, None);
                    if let Err(e) = journal.save(&self.journal) {
                        tracing::warn!(
                            error = format!("{e:#}"),
                            "restore pre-existing Radeon journal entries"
                        );
                    }
                }
                (RestoreResult::Failed(message), vec![], vec![])
            }
            Err((message, true)) => {
                // Switching the feature or its partners back failed: keep displaced entries
                // journaled (with their updated `written`) and schedule pass-2 retries / sticky
                // failures so pass 2 cannot mask the failure into `is_clean() == true`.
                let mut reverted_existing = vec![];
                let mut follow_up = vec![];
                let mut sticky_failures = vec![];
                for (id, desired_original, _) in &wanted {
                    let at_desired = self.backend.read_value(entry.gpu, id).ok()
                        == Some(Value::Bool(*desired_original));
                    if at_desired {
                        reverted_existing.push(id.clone());
                        continue;
                    }
                    let Some(tracked) = journal
                        .entries
                        .iter()
                        .find(|e| e.is(&entry.user, entry.gpu, id))
                        .cloned()
                    else {
                        continue;
                    };
                    if tracked.original == Value::Bool(*desired_original) {
                        follow_up.push(tracked);
                    } else {
                        sticky_failures.push((tracked, message.clone()));
                    }
                }
                if !reverted_existing.is_empty() && !saved_written.is_empty() {
                    restore_saved_written(journal, Some(&reverted_existing));
                    if let Err(e) = journal.save(&self.journal) {
                        tracing::warn!(
                            error = format!("{e:#}"),
                            "update reverted Radeon journal entries"
                        );
                    }
                }
                (RestoreResult::Failed(message), follow_up, sticky_failures)
            }
        }
    }

    /// Returns `Err((message, tail_failed))` where `tail_failed` is true if reverting `toggle` or
    /// its excluded `partners` failed and may have left them displaced in the driver.
    fn through_toggle(
        &mut self,
        entry: &Entry,
        toggle: &str,
        partners: &[String],
    ) -> Result<(), (String, bool)> {
        let gpu = entry.gpu;
        let switch = |backend: &mut B, id: &str, on: bool| {
            backend
                .write(gpu, id, &Value::Bool(on))
                .map_err(|e| format!("{id}: {e}"))
        };
        let switch_checked = |backend: &mut B, id: &str, on: bool| -> Result<(), String> {
            let target = Value::Bool(on);
            backend
                .write(gpu, id, &target)
                .map_err(|e| format!("{id}: {e}"))?;
            let actual = backend
                .read_value(gpu, id)
                .map_err(|e| format!("{id}: {e}"))?;
            if actual != target {
                return Err(format!(
                    "{id}: wrote {} and the driver reads back {}",
                    target.display(),
                    actual.display()
                ));
            }
            Ok(())
        };
        switch(&mut self.backend, toggle, true).map_err(|e| (e, false))?;
        let put = self.put_back(entry);
        // Back to how the feature was whether or not the field went back: a failed put-back must
        // not leave the feature on.
        let tail = (|| {
            switch_checked(&mut self.backend, toggle, false)?;
            for partner in partners {
                let now = self.backend.read_value(gpu, partner).map_err(|e| e.0)?;
                if now != Value::Bool(true) {
                    switch_checked(&mut self.backend, partner, true)?;
                }
            }
            Ok(())
        })();
        match (put, tail) {
            (Ok(()), Ok(())) => Ok(()),
            (Ok(()), Err(tail)) => Err((tail, true)),
            (Err(put), Ok(())) => Err((format!("{put}; {toggle} was switched back"), false)),
            (Err(put), Err(tail)) => Err((
                format!(
                    "{put}; switching {toggle} back also failed ({tail}), the temporary entries stay journaled"
                ),
                true,
            )),
        }
    }
}

#[cfg(test)]
mod tests;
