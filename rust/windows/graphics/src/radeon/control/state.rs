//! Atomic journal JSON helpers local to `radeon::control`.
use anyhow::{Context, Result};
use serde::Serialize;
use std::{fs::OpenOptions, io::Write, path::Path};

pub(super) fn json_bytes(bytes: &[u8]) -> Option<&[u8]> {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    bytes
        .iter()
        .any(|b| !b" \t\r\n\x0b\x0c".contains(b))
        .then_some(bytes)
}

pub(super) fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .context("journal path missing file name")?
        .to_string_lossy();
    let tmp = parent.join(format!("{file_name}.{}.tmp", std::process::id()));
    let write_result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        let wide_tmp = crate::text::to_wide(&tmp.to_string_lossy());
        let wide_dst = crate::text::to_wide(&path.to_string_lossy());
        // SAFETY: Both paths are NUL-terminated UTF-16 buffers alive for the call.
        unsafe {
            windows::Win32::Storage::FileSystem::MoveFileExW(
                windows::core::PCWSTR(wide_tmp.as_ptr()),
                windows::core::PCWSTR(wide_dst.as_ptr()),
                windows::Win32::Storage::FileSystem::MOVEFILE_REPLACE_EXISTING
                    | windows::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH,
            )?;
        }
        Ok(())
    })();
    if write_result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    write_result
}

impl<B: super::RadeonBackend> super::Controller<B> {
    /// The driver takes the field's value only while its feature is on, and the feature is off
    /// now: switch it on (journaled first, with whatever that switches off), put the field back,
    /// then put the feature back.
    pub(super) fn restore_through(
        &mut self,
        journal: &mut super::Journal,
        entry: &super::Entry,
        toggle: &str,
    ) -> (
        super::RestoreResult,
        Vec<super::Entry>,
        Vec<(super::Entry, String)>,
    ) {
        let group = toggle.strip_suffix(".enabled").unwrap_or(toggle);
        let partners: Vec<String> = super::model::excluded_by(group)
            .map(|other| format!("{other}.enabled"))
            .filter(|id| {
                matches!(
                    self.backend.read_value(entry.gpu, id),
                    Ok(super::Value::Bool(true))
                )
            })
            .collect();
        let mut temporary: Vec<String> = vec![];
        // Existing journal entries whose `written` is temporarily updated to match the
        // driver state while the feature is toggled, so a failure or crash cannot make a
        // subsequent restore misclassify our temporary switch as `ChangedElsewhere`.
        let mut saved_written: Vec<(String, super::Value, super::Value)> = vec![];
        let wanted: Vec<(String, bool, bool)> = std::iter::once((toggle.to_string(), false, true))
            .chain(partners.iter().map(|id| (id.clone(), true, false)))
            .collect();
        for (id, original, written) in &wanted {
            if let Some(existing) = journal
                .entries
                .iter_mut()
                .find(|e| e.is(&entry.user, entry.gpu, id))
            {
                saved_written.push((
                    id.clone(),
                    existing.written.clone(),
                    super::Value::Bool(*original),
                ));
                existing.written = super::Value::Bool(*written);
                continue;
            }
            temporary.push(id.clone());
            journal.entries.push(super::Entry::new(
                entry.user.clone(),
                entry.gpu,
                id.clone(),
                entry.session.clone(),
                super::Value::Bool(*original),
                super::Value::Bool(*written),
                true,
            ));
        }
        let restore_saved_written = |journal: &mut super::Journal, only_ids: Option<&[String]>| {
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
        let rollback = |journal: &mut super::Journal| {
            journal.entries.retain(|e| {
                !(e.user == entry.user && e.gpu == entry.gpu && temporary.contains(&e.id))
            });
            restore_saved_written(journal, None);
        };
        if let Err(e) = journal.save(&self.journal) {
            rollback(journal);
            return (
                super::RestoreResult::Failed(format!(
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
                (super::RestoreResult::Restored, vec![], vec![])
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
                (super::RestoreResult::Failed(message), vec![], vec![])
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
                        == Some(super::Value::Bool(*desired_original));
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
                    if tracked.original == super::Value::Bool(*desired_original) {
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
                (
                    super::RestoreResult::Failed(message),
                    follow_up,
                    sticky_failures,
                )
            }
        }
    }

    /// Returns `Err((message, tail_failed))` where `tail_failed` is true if reverting `toggle` or
    /// its excluded `partners` failed and may have left them displaced in the driver.
    fn through_toggle(
        &mut self,
        entry: &super::Entry,
        toggle: &str,
        partners: &[String],
    ) -> Result<(), (String, bool)> {
        let gpu = entry.gpu;
        let switch = |backend: &mut B, id: &str, on: bool, verify: bool| -> Result<(), String> {
            let target = super::Value::Bool(on);
            backend
                .write(gpu, id, &target)
                .map_err(|e| format!("{id}: {e}"))?;
            if verify {
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
            }
            Ok(())
        };
        if let Err(e) = switch(&mut self.backend, toggle, true, false) {
            return match self.backend.read_value(gpu, toggle) {
                Ok(super::Value::Bool(false)) => Err((e, false)),
                Ok(_) if switch(&mut self.backend, toggle, false, true).is_ok() => {
                    Err((format!("{e}; {toggle} was switched back"), false))
                }
                _ => Err((e, true)),
            };
        }
        let put = self.put_back(entry);
        // Back to how the feature was whether or not the field went back: a failed put-back must
        // not leave the feature on.
        let tail = (|| {
            switch(&mut self.backend, toggle, false, true)?;
            for partner in partners {
                let now = self.backend.read_value(gpu, partner).map_err(|e| e.0)?;
                if now != super::Value::Bool(true) {
                    switch(&mut self.backend, partner, true, true)?;
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
