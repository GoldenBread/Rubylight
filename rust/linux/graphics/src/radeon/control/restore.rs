//! Putting journaled driver settings back: only the signed-in user's entries, and only while a setting still holds the value the host wrote.
use super::{
    Controller, Entry, Journal, Outcome, RadeonBackend, RestoreReport, RestoreResult, Scope,
    switched_off_by,
};
use crate::radeon::model::{self, Value};
use anyhow::Result;
use std::time::Instant;

impl<B: RadeonBackend> Controller<B> {
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
        // Requested changes first, newest first; then what the driver switched as a consequence.
        let mut order: Vec<Entry> = mine.into_iter().cloned().collect();
        order.reverse();
        order.sort_by_key(|e| e.side_effect);
        let mut results: Vec<(Entry, RestoreResult)> = vec![];
        let mut pending = order;
        for _ in 0..2 {
            let mut failed = vec![];
            for entry in pending {
                let result = if self.expired() {
                    RestoreResult::Failed("the restore time limit passed".into())
                } else {
                    self.restore_one(&mut journal, &entry, startup)
                };
                if matches!(result, RestoreResult::Failed(_)) {
                    failed.push(entry.clone());
                }
                results.retain(|(e, _)| !e.is(&entry.user, entry.gpu, &entry.id));
                results.push((entry, result));
            }
            pending = failed;
            if pending.is_empty() {
                break;
            }
        }
        let mut report = RestoreReport::default();
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
    ) -> RestoreResult {
        let now = match self.backend.read_value(entry.gpu, &entry.id) {
            Ok(now) => now,
            Err(e) => return RestoreResult::Failed(e.0),
        };
        if now == entry.original {
            if startup && let Err(e) = self.backend.write(entry.gpu, &entry.id, &entry.original) {
                // It reads as the original, and the write is only for safety.
                tracing::debug!(setting = %entry.id, error = %e, "startup write-back of the original");
            }
            return RestoreResult::AlreadyOriginal;
        }
        if now != entry.written {
            return RestoreResult::ChangedElsewhere {
                current: now,
                written: entry.written.clone(),
            };
        }
        if let Some(toggle) = model::feature_toggle(&entry.id) {
            match self.backend.read_value(entry.gpu, &toggle) {
                Ok(Value::Bool(false)) => return self.restore_through(journal, entry, &toggle),
                Ok(_) => {}
                Err(e) => return RestoreResult::Failed(e.0),
            }
        }
        match self.put_back(entry) {
            Ok(()) => RestoreResult::Restored,
            Err(message) => RestoreResult::Failed(message),
        }
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
    ) -> RestoreResult {
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
        let wanted = std::iter::once((toggle.to_string(), false, true))
            .chain(partners.iter().map(|id| (id.clone(), true, false)));
        for (id, original, written) in wanted {
            if journal
                .entries
                .iter()
                .any(|e| e.is(&entry.user, entry.gpu, &id))
            {
                continue;
            }
            temporary.push(id.clone());
            journal.entries.push(Entry {
                user: entry.user.clone(),
                gpu: entry.gpu,
                id,
                session: entry.session.clone(),
                original: Value::Bool(original),
                written: Value::Bool(written),
                side_effect: true,
                raw: None,
            });
        }
        let forget = |journal: &mut Journal| {
            journal.entries.retain(|e| {
                !(e.user == entry.user && e.gpu == entry.gpu && temporary.contains(&e.id))
            });
        };
        if let Err(e) = journal.save(&self.journal) {
            forget(journal);
            return RestoreResult::Failed(format!(
                "journaling the temporary switch of {toggle}: {e:#}"
            ));
        }
        if let Err(message) = self.through_toggle(entry, toggle, &partners) {
            // The temporary entries stay: the next restore puts the switches back.
            return RestoreResult::Failed(message);
        }
        forget(journal);
        if let Err(e) = journal.save(&self.journal) {
            tracing::warn!(
                error = format!("{e:#}"),
                "drop the temporary Radeon entries"
            );
        }
        RestoreResult::Restored
    }

    fn through_toggle(
        &mut self,
        entry: &Entry,
        toggle: &str,
        partners: &[String],
    ) -> Result<(), String> {
        let gpu = entry.gpu;
        let switch = |backend: &mut B, id: &str, on: bool| {
            backend
                .write(gpu, id, &Value::Bool(on))
                .map_err(|e| format!("{id}: {e}"))
        };
        switch(&mut self.backend, toggle, true)?;
        let put = self.put_back(entry);
        // Back to how the feature was whether or not the field went back: a failed put-back must
        // not leave the feature on.
        let tail = (|| {
            switch(&mut self.backend, toggle, false)?;
            for partner in partners {
                let now = self.backend.read_value(gpu, partner).map_err(|e| e.0)?;
                if now != Value::Bool(true) {
                    switch(&mut self.backend, partner, true)?;
                }
            }
            Ok(())
        })();
        match (put, tail) {
            (Ok(()), tail) => tail,
            (Err(put), Ok(())) => Err(format!("{put}; {toggle} was switched back")),
            (Err(put), Err(tail)) => Err(format!(
                "{put}; switching {toggle} back also failed ({tail}), the temporary entries stay journaled"
            )),
        }
    }
}
