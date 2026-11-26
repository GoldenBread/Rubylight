//! Game settings files per device (issues #72 and #69): a game's own settings files (its
//! resolution and quality) swapped for the copy of the device that streams it.
//!
//! An app lists whole files in `game-settings-files`: absolute paths, `%VAR%` names from the
//! Windows user's environment allowed (`%LOCALAPPDATA%\Game\Settings.ini`). When a device
//! starts the app, the host keeps the PC's own files (the originals) and puts that device's
//! saved copies in their place. When the app stops (End session, Close game, the game exiting)
//! the files, with whatever the game changed, become the device's copies and the originals go
//! back. A device's first launch keeps the files as they are and saves them as its copies at the
//! end. Finding a game's settings files is up to the user; the host does not look for them.
//!
//! Nothing is lost on the way: the originals and every device copy live under the host's
//! config folder (`game-settings`, [`FOLDER`]), a device's previous copy is kept beside its
//! new one (`.previous`), and the journal ([`JOURNAL_FILE`]) records each file's step before
//! the step is taken. A swap a crash or a failure left unfinished is finished later from the
//! journal ([`Store::recover`]): a file still holding the device's copy is saved as that copy
//! first, then the original goes back. A file that is missing, locked or unreadable fails the
//! launch with the reason, before the game starts; one that cannot be saved or put back when
//! the app stops stays journaled, and nothing of it is overwritten until it can be.
//!
//! One swap holds a file at a time. A launch whose file another swap still holds finishes that
//! swap first; while that swap's app runs, or when it cannot be finished, the launch fails.
//!
//! Each swap keeps its own originals (`apps/<app>/swaps/<id>/`): a later launch never
//! overwrites an original that is not back yet. Once every file is back, they move to
//! `apps/<app>/original/` as the backup of the PC's last own files. A swap also records the
//! game's processes as the host sees them ([`Store::watch`]), each by its id and creation time,
//! so a later process of the same program (a shared helper, the game started at the desk) is
//! never taken for it. While one of them still runs, no file is saved or put back (the game
//! would write over the PC's file as it exits), and the swap stays journaled until it has
//! closed, or until the user puts the files back from the console ([`Store::release`]). The
//! last [`BACKUPS`] originals put back stay as backups.
//!
//! The host runs as SYSTEM: the game's files are read and written through an
//! [`Access`] (as the signed-in Windows user, as for managed game files), so a link in the
//! user's folders leads nowhere the user could not write anyway. The store is the host's own.
use crate::{
    game_files::{self, Access},
    state,
};
use anyhow::{Context, Result, anyhow, bail};
#[cfg(test)]
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard, PoisonError},
};

mod paths;
mod types;

pub use paths::{app_key, check_path, expand, file_key, paths};
use paths::{normal, store_name};
pub use types::{
    BACKUPS, Begun, FileSwap, Journal, Report, Running, Step, StillRunning, Swap, Watched,
    still_running,
};
use types::{Mode, Unread, is_missing};

/// The app setting: a list of paths.
pub const SETTING: &str = "game-settings-files";
/// The host's folder for the copies, in its config folder.
pub const FOLDER: &str = "game-settings";
/// The journal, in [`FOLDER`].
pub const JOURNAL_FILE: &str = "journal.json";
/// Files per app.
pub const MAX_FILES: usize = 16;
/// The largest file taken: settings, not saves or caches.
pub const MAX_FILE_BYTES: u64 = 16 << 20;

/// Held through every journal change, so two swaps never interleave.
static LOCK: Mutex<()> = Mutex::new(());
fn lock() -> MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The copies and the journal under a config folder.
pub struct Store {
    root: PathBuf,
    access: Access,
    running: Running,
}

impl Store {
    /// A store that takes every recorded program as closed; see [`Self::running`].
    pub fn new(directory: &Path, access: Access) -> Self {
        Self {
            root: directory.join(FOLDER),
            access,
            running: Box::new(|_| None),
        }
    }
    /// Tell the store which recorded processes still run.
    pub fn running(mut self, running: Running) -> Self {
        self.running = running;
        self
    }
    fn journal_path(&self) -> PathBuf {
        self.root.join(JOURNAL_FILE)
    }
    fn app_folder(&self, app: &str) -> PathBuf {
        self.root.join("apps").join(store_name(app))
    }
    /// A device's saved copy of a file.
    pub fn device_copy(&self, app: &str, device: &str, key: &str) -> PathBuf {
        self.app_folder(app)
            .join("devices")
            .join(store_name(device))
            .join(key)
    }
    /// The PC's last own file put back, kept as a backup.
    pub fn original(&self, app: &str, key: &str) -> PathBuf {
        self.app_folder(app).join("original").join(key)
    }
    /// The folder of swap `id`'s originals, until all are back.
    fn swap_folder(&self, app: &str, id: &str) -> PathBuf {
        self.app_folder(app).join("swaps").join(store_name(id))
    }
    /// Whether `device` has a saved copy of the file the app lists as `written`.
    pub fn saved(&self, app: &str, device: &str, written: &str) -> bool {
        self.device_copy(app, device, &file_key(written)).is_file()
    }

    /// The journal. Missing is empty; unreadable is an error, so nothing is swapped while the
    /// originals in it could be forgotten.
    pub fn journal(&self) -> Result<Journal> {
        let path = self.journal_path();
        match std::fs::read(&path) {
            Ok(bytes) => match state::json_bytes(&bytes) {
                None => Ok(Journal::default()),
                Some(json) => serde_json::from_slice(json)
                    .with_context(|| format!("invalid game settings journal {}", path.display())),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Journal::default()),
            Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
        }
    }
    fn save(&self, journal: &Journal) -> Result<()> {
        state::write_json(
            &self.journal_path(),
            &Journal {
                version: 1,
                swaps: journal.swaps.clone(),
            },
        )
    }

    /// `device` (its name for messages) starts `app`: keep the originals of `files` (as listed,
    /// expanded) and put the device's copies in, as swap `id`. A swap that still holds one of
    /// the files and is not in `live` (running apps' swaps, read under the store's lock, so a
    /// swap registered before its `begin` is never taken for a stale one) is finished first.
    /// Fails, with every file as it was, when a file is missing or cannot be read or written,
    /// or another swap holds it.
    #[allow(clippy::too_many_arguments)]
    pub fn begin(
        &self,
        id: &str,
        app: &str,
        app_name: &str,
        device: &str,
        device_name: &str,
        files: &[(String, PathBuf)],
        live: &dyn Fn() -> BTreeSet<String>,
    ) -> Result<Begun> {
        let mut seen: BTreeMap<String, &str> = BTreeMap::new();
        for (written, path) in files {
            if let Some(other) = seen.insert(normal(&path.to_string_lossy()), written) {
                bail!(
                    "{other} and {written} are the same file ({})",
                    path.display()
                );
            }
        }
        let _lock = lock();
        let live = live();
        let mut journal = self.journal()?;
        if journal.swaps.iter().any(|swap| swap.id == id) {
            bail!("game settings swap {id} exists already");
        }
        let stale: Vec<String> = journal
            .swaps
            .iter()
            .filter(|swap| files.iter().any(|(_, path)| swap.holds(path)))
            .map(|swap| swap.id.clone())
            .collect();
        for id in stale {
            let swap = journal.swaps.iter().find(|swap| swap.id == id).unwrap();
            let who = swap.who();
            if live.contains(&id) {
                bail!("the settings files are in use by {who}, which is still running");
            }
            self.finish_in(&mut journal, &id, Mode::Finish)
                .with_context(|| {
                    format!("the settings files {who} used could not be put back first")
                })?;
        }
        // The originals are kept first, in this swap's own folder: from here a crash only ever
        // puts them back, and no other swap's are touched.
        let originals = self.swap_folder(app, id);
        let mut swapped = Vec::new();
        for (written, path) in files {
            let key = file_key(written);
            let kept = self.read_game(written, path).and_then(|bytes| {
                state::atomic_write(&originals.join(&key), &bytes)
                    .with_context(|| format!("keeping the original of {written}"))
            });
            if let Err(error) = kept {
                let _ = std::fs::remove_dir_all(&originals);
                return Err(error);
            }
            swapped.push(FileSwap {
                written: written.clone(),
                path: path.clone(),
                key,
                step: Step::Placing,
            });
        }
        let id = id.to_owned();
        journal.swaps.push(Swap {
            id: id.clone(),
            app: app.to_owned(),
            app_name: app_name.to_owned(),
            device: device.to_owned(),
            device_name: device_name.to_owned(),
            files: swapped,
            processes: Vec::new(),
            error: None,
        });
        if let Err(error) = self.save(&journal) {
            let _ = std::fs::remove_dir_all(&originals);
            return Err(error);
        }
        let index = journal.swaps.len() - 1;
        let mut report = Report::default();
        let mut failure = None;
        for file in 0..journal.swaps[index].files.len() {
            let FileSwap {
                written, path, key, ..
            } = journal.swaps[index].files[file].clone();
            let copy = self.device_copy(app, device, &key);
            let placed = match read_store(&copy) {
                Ok(Some(bytes)) => self
                    .write_game(&path, &bytes)
                    .map(|()| report.placed.push(written.clone()))
                    .with_context(|| {
                        format!(
                            "{written} ({}) could not be replaced by the copy of {device_name}",
                            path.display()
                        )
                    }),
                Ok(None) => {
                    report.first.push(written.clone());
                    Ok(())
                }
                Err(error) => Err(anyhow!(error))
                    .with_context(|| format!("reading the copy of {written} for {device_name}")),
            };
            match placed {
                Ok(()) => {
                    journal.swaps[index].files[file].step = Step::Playing;
                    if let Err(error) = self.save(&journal) {
                        failure = Some(error);
                        break;
                    }
                }
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            }
        }
        let Some(error) = failure else {
            return Ok(Begun { id, report });
        };
        // The game never ran: the copies that went in are unchanged, so only the originals go back.
        for file in &mut journal.swaps[index].files {
            if file.step == Step::Playing {
                file.step = Step::Saved;
            }
        }
        let save_error = self.save(&journal).err();
        match self.finish_in(&mut journal, &id, Mode::Cancel) {
            Ok(_) => Err(match save_error {
                Some(save_err) => error.context(format!("journal save failed: {save_err:#}")),
                None => error,
            }),
            Err(again) => Err(error.context(format!(
                "and the originals could not all be put back yet ({again:#}); the host tries again"
            ))),
        }
    }

    /// The app of swap `id` stopped: save its files as the device's copies and put the
    /// originals back. A file that fails stays journaled for [`Self::recover`].
    pub fn finish(&self, id: &str) -> Result<Report> {
        let _lock = lock();
        let mut journal = self.journal()?;
        self.finish_in(&mut journal, id, Mode::Finish)
    }

    /// The user puts swap `id`'s files back from the console although its game may still run
    /// (a process the host took for the game that is not): as [`Self::finish`] otherwise.
    pub fn release(&self, id: &str) -> Result<Report> {
        let _lock = lock();
        let mut journal = self.journal()?;
        self.finish_in(&mut journal, id, Mode::Release)
    }

    /// The launch of swap `id` failed after its files went in, before the game ran: only the
    /// originals go back; the device's copies stay as they were.
    pub fn cancel(&self, id: &str) -> Result<Report> {
        let _lock = lock();
        let mut journal = self.journal()?;
        let swap = journal
            .swaps
            .iter_mut()
            .find(|swap| swap.id == id)
            .with_context(|| format!("no game settings swap {id}"))?;
        for file in &mut swap.files {
            if file.step == Step::Playing {
                file.step = Step::Saved;
            }
        }
        self.save(&journal)?;
        self.finish_in(&mut journal, id, Mode::Cancel)
    }

    /// The game of swap `id` moved to `device` (its name for messages) while it runs: the
    /// files as they are now become the previous device's copies, and from here the swap is
    /// the new device's, so what it changes in the game is saved for it. The files in the
    /// game's folder stay as they are, under the running game. A file that is gone keeps the
    /// previous copy; one that cannot be read or saved leaves the swap with the previous
    /// device, whose copy then takes everything at the end rather than lose its part.
    pub fn hand_over(&self, id: &str, device: &str, device_name: &str) -> Result<Report> {
        let _lock = lock();
        let mut journal = self.journal()?;
        let index = journal
            .swaps
            .iter()
            .position(|swap| swap.id == id)
            .with_context(|| format!("no game settings swap {id}"))?;
        let swap = journal.swaps[index].clone();
        let mut report = Report::default();
        if swap.device == device {
            return Ok(report);
        }
        let who = swap.who();
        for file in swap.files.iter().filter(|file| file.step == Step::Playing) {
            match self.read_game(&file.written, &file.path) {
                Ok(bytes) => {
                    self.keep_copy(&swap.app, &swap.device, &file.key, &bytes)
                        .with_context(|| format!("saving {} for {who}", file.written))?;
                    report.saved.push(file.written.clone());
                }
                Err(error) if is_missing(&error) => report.warnings.push(format!(
                    "{} ({}) was gone when the game moved on from {who}, so its saved copy stays as it was",
                    file.written,
                    file.path.display()
                )),
                Err(error) => return Err(error),
            }
        }
        journal.swaps[index].device = device.to_owned();
        journal.swaps[index].device_name = device_name.to_owned();
        self.save(&journal)?;
        Ok(report)
    }

    /// Record `processes` as swap `id`'s game: its files stay while one runs.
    pub fn watch(&self, id: &str, processes: &[Watched]) -> Result<()> {
        let _lock = lock();
        let mut journal = self.journal()?;
        let Some(swap) = journal.swaps.iter_mut().find(|swap| swap.id == id) else {
            return Ok(());
        };
        let mut changed = false;
        for process in processes {
            if process.started != 0
                && !swap
                    .processes
                    .iter()
                    .any(|known| known.pid == process.pid && known.started == process.started)
            {
                swap.processes.push(process.clone());
                changed = true;
            }
        }
        if changed {
            self.save(&journal)?;
        }
        Ok(())
    }

    /// Finish every journaled swap not in `live` (read under the store's lock): at host start,
    /// and again while one fails or its game still runs.
    pub fn recover(
        &self,
        live: &dyn Fn() -> BTreeSet<String>,
    ) -> Result<Vec<(Swap, Result<Report>)>> {
        let _lock = lock();
        let live = live();
        let mut journal = self.journal()?;
        let ids: Vec<String> = journal
            .swaps
            .iter()
            .filter(|swap| !live.contains(&swap.id))
            .map(|swap| swap.id.clone())
            .collect();
        let mut results = Vec::new();
        for id in ids {
            let swap = journal
                .swaps
                .iter()
                .find(|swap| swap.id == id)
                .cloned()
                .unwrap();
            let result = self.finish_in(&mut journal, &id, Mode::Finish);
            results.push((swap, result));
        }
        Ok(results)
    }

    fn finish_in(&self, journal: &mut Journal, id: &str, mode: Mode) -> Result<Report> {
        let index = journal
            .swaps
            .iter()
            .position(|swap| swap.id == id)
            .with_context(|| format!("no game settings swap {id}"))?;
        let (app, device, who) = {
            let swap = &journal.swaps[index];
            (swap.app.clone(), swap.device.clone(), swap.who())
        };
        // A game still running writes its settings as it exits: over the PC's file, were it
        // back, and past the device's copy. Nothing moves until it has closed.
        if mode != Mode::Release
            && let Some(program) = (self.running)(&journal.swaps[index].processes)
        {
            let text = format!(
                "{program} is still running; the files go back once it closes, or with Put back now in the app's editor"
            );
            journal.swaps[index].error = Some(text.clone());
            self.save(journal)?;
            return Err(anyhow!(StillRunning).context(format!("{who}: {text}")));
        }
        let originals = self.swap_folder(&app, id);
        let mut report = Report::default();
        if mode == Mode::Finish && journal.swaps[index].processes.is_empty() {
            let warning = format!(
                "no process of {who} was seen, so the host could not wait for the game to close; if it still runs, it may write its settings over the PC's files. The last {BACKUPS} PC files put back are kept in {}",
                self.app_folder(&app).join("original").display()
            );
            report.warnings.push(warning.clone());
            report.unwatched = Some(warning);
        }
        let mut failures = Vec::new();
        for file in 0..journal.swaps[index].files.len() {
            let FileSwap {
                written,
                path,
                key,
                step,
            } = journal.swaps[index].files[file].clone();
            if step == Step::Playing {
                match self.read_game(&written, &path) {
                    Ok(bytes) => {
                        if let Err(error) = self.keep_copy(&app, &device, &key, &bytes) {
                            // The file is the only place the session's changes are: it stays.
                            failures.push(format!("{written}: saving the copy failed: {error:#}"));
                            continue;
                        }
                        report.saved.push(written.clone());
                    }
                    Err(error) if is_missing(&error) => report.warnings.push(format!(
                        "{written} ({}) was gone when {who} ended, so the saved copy stays as it was; the PC's own file is put back",
                        path.display()
                    )),
                    Err(error) => {
                        failures.push(format!("{error:#}"));
                        continue;
                    }
                }
                journal.swaps[index].files[file].step = Step::Saved;
                self.save(journal)?;
            }
            if matches!(step, Step::Playing | Step::Saved | Step::Placing) {
                let original = originals.join(&key);
                match read_store(&original) {
                    Ok(Some(bytes)) => match self.write_game(&path, &bytes).with_context(|| {
                        format!("{written} ({}) could not be put back", path.display())
                    }) {
                        Ok(()) => {
                            report.restored.push(written.clone());
                            journal.swaps[index].files[file].step = Step::Done;
                            self.save(journal)?;
                        }
                        Err(error) => failures.push(format!("{error:#}")),
                    },
                    Ok(None) => {
                        report.warnings.push(format!(
                            "{written}: the kept original {} is missing, so it could not be put back",
                            original.display()
                        ));
                        journal.swaps[index].files[file].step = Step::Done;
                        self.save(journal)?;
                    }
                    Err(error) => failures.push(format!(
                        "{:#}",
                        anyhow!(error).context(format!("reading {}", original.display()))
                    )),
                }
            }
        }
        if failures.is_empty() {
            let swap = journal.swaps.remove(index);
            self.save(journal)?;
            // Every file is back: its original becomes the backup of the PC's last own file.
            let mut all_moved = true;
            for file in &swap.files {
                let kept = originals.join(&file.key);
                if !kept.is_file() {
                    continue;
                }
                let backup = self.original(&app, &file.key);
                // The same file as the newest backup is not a new one: the older stay.
                let same = matches!(
                    (std::fs::read(&kept), std::fs::read(&backup)),
                    (Ok(a), Ok(b)) if a == b
                );
                let moved = if same {
                    std::fs::remove_file(&kept)
                } else {
                    backup
                        .parent()
                        .map_or(Ok(()), std::fs::create_dir_all)
                        .and_then(|()| rotate(&backup))
                        .and_then(|()| std::fs::rename(&kept, &backup))
                };
                if let Err(error) = moved {
                    all_moved = false;
                    report.warnings.push(format!(
                        "{}: the original stays in {}, as it could not become the backup {}: {error}",
                        file.written,
                        kept.display(),
                        backup.display()
                    ));
                }
            }
            if all_moved {
                let _ = std::fs::remove_dir_all(&originals);
            }
            return Ok(report);
        }
        let text = failures.join("; ");
        journal.swaps[index].error = Some(text.clone());
        self.save(journal)?;
        bail!("{who}: {text}")
    }

    /// Save `bytes` as the device's copy, its previous copy kept beside it.
    fn keep_copy(&self, app: &str, device: &str, key: &str, bytes: &[u8]) -> Result<()> {
        let copy = self.device_copy(app, device, key);
        if copy.is_file() {
            let mut previous = copy.clone().into_os_string();
            previous.push(".previous");
            std::fs::copy(&copy, PathBuf::from(previous))
                .with_context(|| format!("keeping the previous copy {}", copy.display()))?;
        }
        state::atomic_write(&copy, bytes)
    }

    /// Read a game's file as the user, with why it cannot be read.
    fn read_game(&self, written: &str, path: &Path) -> Result<Vec<u8>> {
        let read = game_files::reach(self.access, || read_file(path)).and_then(|read| read);
        read.map_err(|error| {
            let missing = error.kind() == std::io::ErrorKind::NotFound;
            let why = if missing {
                "does not exist. Start the game once on this PC so it writes its settings, or correct the path under Game settings files".to_owned()
            } else if matches!(error.raw_os_error(), Some(32 | 33)) {
                "is in use by another program (is the game still running?)".to_owned()
            } else {
                format!("could not be read as the Windows user: {error}")
            };
            let shown = path.display().to_string();
            let message = if shown == written {
                format!("{written} {why}")
            } else {
                format!("{written} ({shown}) {why}")
            };
            anyhow!(Unread { missing, message })
        })
    }

    /// Write a game's file as the user: replaced in one step when it exists, created when the
    /// game removed it meanwhile.
    fn write_game(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        game_files::reach(self.access, || {
            if std::fs::symlink_metadata(path).is_ok() {
                game_files::replace(path, bytes)
            } else {
                let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "the file has no folder",
                    ));
                };
                let temporary = parent.join(format!(
                    ".{}.{}.tmp",
                    name.to_string_lossy(),
                    uuid::Uuid::new_v4()
                ));
                let result = (|| {
                    let mut out = std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&temporary)?;
                    out.write_all(bytes)?;
                    out.sync_all()?;
                    drop(out);
                    std::fs::hard_link(&temporary, path)
                })();
                let _ = std::fs::remove_file(&temporary);
                result
            }
        })
        .and_then(|written| written)
        .map_err(|error| {
            if matches!(error.raw_os_error(), Some(32 | 33)) {
                anyhow!("it is in use by another program (is the game still running?)")
            } else {
                anyhow!(error)
            }
        })
    }
}

/// Make room for a new newest backup at `newest`: each older one moves down, the oldest goes.
fn rotate(newest: &Path) -> std::io::Result<()> {
    let numbered = |n: usize| {
        let mut name = newest.as_os_str().to_owned();
        name.push(format!(".{n}"));
        PathBuf::from(name)
    };
    for n in (1..BACKUPS).rev() {
        let from = if n == 1 {
            newest.to_owned()
        } else {
            numbered(n - 1)
        };
        if from.is_file() {
            std::fs::rename(&from, numbered(n))?;
        }
    }
    Ok(())
}

/// A settings file: a file of at most [`MAX_FILE_BYTES`].
fn read_file(path: &Path) -> std::io::Result<Vec<u8>> {
    let file = std::fs::File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other("it is not a file"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(std::io::Error::other(
            "it is larger than 16 MiB, more than a settings file",
        ));
    }
    Ok(bytes)
}

/// A file of the store, or None when there is none.
fn read_store(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests;
