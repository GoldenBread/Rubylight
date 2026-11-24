//! Managed game file journal persistence and apply/restore transactions.
use super::{
    Access, Report, Target,
    ini::{set_value, with_value},
    io::{Found, left_behind_error, look_up, pin, reach, read, replace},
    skip, value_of,
};
use crate::state;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fmt,
    path::{Path, PathBuf},
};

/// One key the host changed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    /// The device whose session wrote it.
    pub session: String,
    /// The file, resolved when it was written.
    pub path: PathBuf,
    /// The app's folder it was in ([`Target::folder`]); empty, only the file's own folder is
    /// held at its restore.
    #[serde(default)]
    pub folder: PathBuf,
    pub section: String,
    pub key: String,
    /// The value before the host's first write; it goes back while the key holds `written`.
    pub original: String,
    pub written: String,
}
impl Change {
    fn is(&self, path: &Path, section: &str, key: &str) -> bool {
        self.path == path
            && self.section.trim().eq_ignore_ascii_case(section.trim())
            && self.key.trim().eq_ignore_ascii_case(key.trim())
    }
}
impl fmt::Display for Change {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} [{}] {}",
            self.path.display(),
            self.section.trim(),
            self.key.trim()
        )
    }
}

/// The keys the host changed and has not put back yet.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Journal {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub changes: Vec<Change>,
}
impl Journal {
    /// A missing file is an empty journal. An unreadable one is an error: the originals in it
    /// must not be forgotten, and nothing is written while it cannot be read.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => match state::json_bytes(&bytes) {
                None => Ok(Self::default()),
                Some(json) => serde_json::from_slice(json)
                    .with_context(|| format!("invalid game files journal {}", path.display())),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
        }
    }
    /// Atomic, and synced before it returns.
    pub fn save(&self, path: &Path) -> Result<()> {
        state::write_json(
            path,
            &Self {
                version: 1,
                changes: self.changes.clone(),
            },
        )
    }
}

/// Put `change`'s original back into `bytes` if the key still holds what the host wrote.
fn put_back(bytes: &mut Vec<u8>, change: &Change, report: &mut Report) {
    match value_of(bytes, &change.section, &change.key) {
        Ok(Some(current)) if current == change.written => {
            match set_value(bytes, &change.section, &change.key, &change.original) {
                Ok(Some(next)) => {
                    *bytes = next;
                    report.restored.push(change.to_string());
                }
                Ok(None) => report.kept.push(format!("{change}: no longer in the file")),
                Err(error) => report.skipped.push(format!("{change}: {error:#}")),
            }
        }
        Ok(Some(current)) => {
            tracing::info!(key = %change, value = %current, written = %change.written, "a managed game file key changed since the host wrote it; the new value stays");
            report.kept.push(format!("{change}: changed to {current}"));
        }
        Ok(None) => report.kept.push(format!("{change}: no longer in the file")),
        Err(error) => report.skipped.push(format!("{change}: {error:#}")),
    }
}

/// Set `targets` for `session`'s stream (a device's uuid). For each file, the journal takes
/// every key's original before the file changes. Another session's change to one of these keys
/// goes back first, so the original recorded is the PC's own value. A key the file does not
/// have, or a file that cannot be read, is skipped with a warning; a file that cannot be
/// written is left as it was. The files are reached through `access`. Errors only when the
/// journal cannot be read: nothing is written.
pub fn apply(
    journal_path: &Path,
    session: &str,
    targets: &[Target],
    access: Access,
) -> Result<Report> {
    let mut journal = Journal::load(journal_path)?;
    let mut report = Report::default();
    let mut files: Vec<&Path> = Vec::new();
    for target in targets {
        if !files.contains(&target.file.as_path()) {
            files.push(&target.file);
        }
    }
    for file in files {
        let wanted: Vec<&Target> = targets.iter().filter(|t| t.file == file).collect();
        let folder = wanted[0].folder.as_path();
        let found = reach(access, || {
            let _pins = pin(folder, file)?;
            let bytes = read(file)?;
            // A file that is still the one resolved: a link put in its place since is not followed.
            if std::fs::canonicalize(file)? != file {
                bail!("the file was replaced by a link");
            }
            Ok(bytes)
        })
        .map_err(anyhow::Error::from)
        .and_then(|found| found);
        let bytes = match found {
            Ok(bytes) => bytes,
            Err(error) => {
                for target in &wanted {
                    skip(&mut report, &target.to_string(), &format!("{error:#}"));
                }
                continue;
            }
        };
        let mut next = journal.clone();
        let mut edited = bytes.clone();
        let mut done = Report::default();
        let others: Vec<Change> = next
            .changes
            .iter()
            .filter(|change| {
                change.session != session
                    && wanted
                        .iter()
                        .any(|target| change.is(file, &target.section, &target.key))
            })
            .cloned()
            .collect();
        for other in &others {
            put_back(&mut edited, other, &mut done);
            next.changes.retain(|change| change != other);
        }
        for target in &wanted {
            let current = match value_of(&edited, &target.section, &target.key) {
                Ok(Some(current)) => current,
                Ok(None) => {
                    skip(
                        &mut done,
                        &target.to_string(),
                        "the file has no such key in that section; it is not added",
                    );
                    continue;
                }
                Err(error) => {
                    skip(&mut done, &target.to_string(), &format!("{error:#}"));
                    continue;
                }
            };
            if current == target.value {
                continue;
            }
            let own = next.changes.iter_mut().find(|change| {
                change.session == session && change.is(file, &target.section, &target.key)
            });
            match own {
                // Written before in this session: the first original stays, unless the key was
                // changed since, which makes that value the one to go back to.
                Some(own) => {
                    if own.written != current {
                        own.original = current;
                    }
                    own.written = target.value.clone();
                }
                None => next.changes.push(Change {
                    session: session.to_owned(),
                    path: file.to_path_buf(),
                    folder: folder.to_path_buf(),
                    section: target.section.clone(),
                    key: target.key.clone(),
                    original: current,
                    written: target.value.clone(),
                }),
            }
            match with_value(&edited, &target.section, &target.key, &target.value) {
                Ok(Some(changed)) => {
                    edited = changed;
                    done.written.push(format!("{target} = {}", target.value));
                }
                Ok(None) => unreachable!("the key was just found"),
                Err(error) => skip(&mut done, &target.to_string(), &format!("{error:#}")),
            }
        }
        // The journal first, still with the other sessions' entries this write puts back: a
        // crash before the swap finds the file as it was, and those entries then still match it
        // (a restore puts their originals back and finds this session's keys unwritten). After
        // the swap their keys hold this session's values, so a left-over entry is left alone.
        let mut before = next.clone();
        before.changes.extend(others.iter().cloned());
        if before != journal
            && let Err(error) = before.save(journal_path)
        {
            for target in &wanted {
                skip(
                    &mut report,
                    &target.to_string(),
                    &format!("the journal could not be written: {error:#}"),
                );
            }
            continue;
        }
        if edited != bytes
            && let Err(error) = reach(access, || {
                let _pins = pin(folder, file)?;
                replace(file, &edited)
            })
            .and_then(|done| done)
        {
            for target in &wanted {
                skip(
                    &mut report,
                    &target.to_string(),
                    &format!("the file could not be written: {error}"),
                );
            }
            // Gone, with the new contents beside it: the entries stay for the restore.
            if left_behind_error(&error) {
                journal = before;
                continue;
            }
            if before != journal
                && let Err(error) = journal.save(journal_path)
            {
                tracing::error!(
                    error = format!("{error:#}"),
                    "the game files journal could not be put back after a failed write; its entries for that file are dropped at their restore"
                );
            }
            continue;
        }
        if next != before
            && let Err(error) = next.save(journal_path)
        {
            tracing::warn!(
                error = format!("{error:#}"),
                "the game files journal still lists keys another device's stream had changed; their restore finds the new values and leaves them"
            );
        }
        journal = next;
        report.written.append(&mut done.written);
        report.restored.append(&mut done.restored);
        report.kept.append(&mut done.kept);
        report.skipped.append(&mut done.skipped);
    }
    Ok(report)
}

/// Which journaled changes a restore puts back.
#[derive(Clone, Copy, Debug)]
pub enum Scope<'a> {
    /// Every change, for a host start after a crash or a shutdown.
    All,
    /// The changes of one device's session.
    Session(&'a str),
    /// Every change but those of these devices' sessions, which still run.
    Except(&'a BTreeSet<String>),
}
impl Scope<'_> {
    fn matches(&self, change: &Change) -> bool {
        match self {
            Self::All => true,
            Self::Session(session) => change.session == *session,
            Self::Except(running) => !running.contains(&change.session),
        }
    }
}

/// Put back the changes `scope` selects: an original goes back only while its key still holds
/// what the host wrote. A key changed since, a file gone or a file that is now a link leaves
/// the file as it is and drops the change. A file that cannot be reached, read or written
/// (through `access`) keeps its changes journaled for the next restore. Errors only when the
/// journal cannot be read or saved.
pub fn restore(journal_path: &Path, scope: Scope<'_>, access: Access) -> Result<Report> {
    let mut journal = Journal::load(journal_path)?;
    let loaded = journal.clone();
    let mut report = Report::default();
    let mut files: Vec<PathBuf> = Vec::new();
    for change in journal.changes.iter().filter(|c| scope.matches(c)) {
        if !files.contains(&change.path) {
            files.push(change.path.clone());
        }
    }
    if files.is_empty() {
        return Ok(report);
    }
    for file in files {
        let changes: Vec<Change> = journal
            .changes
            .iter()
            .filter(|c| c.path == file && scope.matches(c))
            .cloned()
            .collect();
        let drop_them = |journal: &mut Journal| {
            journal
                .changes
                .retain(|c| !(c.path == file && scope.matches(c)));
        };
        let failed = |report: &mut Report, error: &dyn fmt::Display| {
            for change in &changes {
                tracing::error!(key = %change, %error, "a managed game file key could not be put back; it stays journaled");
                report.failed.push(format!("{change}: {error}"));
            }
        };
        let folder = changes[0].folder.clone();
        let bytes = match reach(access, || look_up(&folder, &file)).and_then(|found| found) {
            Ok(Found::File(bytes)) => bytes,
            Ok(Found::Link) => {
                for change in &changes {
                    skip(
                        &mut report,
                        &change.to_string(),
                        "the file or a folder on the way to it was replaced by a link; it is left alone",
                    );
                }
                drop_them(&mut journal);
                continue;
            }
            Ok(Found::Gone) => {
                for change in &changes {
                    skip(&mut report, &change.to_string(), "the file is gone");
                }
                drop_them(&mut journal);
                continue;
            }
            Err(error) => {
                failed(&mut report, &error);
                continue;
            }
        };
        let mut edited = bytes.clone();
        let mut done = Report::default();
        for change in &changes {
            put_back(&mut edited, change, &mut done);
        }
        if edited != bytes
            && let Err(error) = reach(access, || {
                let _pins = pin(&folder, &file)?;
                replace(&file, &edited)
            })
            .and_then(|done| done)
        {
            failed(&mut report, &error);
            continue;
        }
        drop_them(&mut journal);
        report.restored.append(&mut done.restored);
        report.kept.append(&mut done.kept);
        report.skipped.append(&mut done.skipped);
    }
    if journal != loaded {
        journal.save(journal_path)?;
    }
    Ok(report)
}
