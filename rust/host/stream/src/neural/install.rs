//! Per-app Neural Rendering enable/disable jobs, removal slots, and install
//! completion.
use super::{
    Busy, DOWNLOAD_FAILED, Job, KEY, LEGACY_KEY, Pending, Refusal, Removed, State, StreamRunning,
    StreamStarted,
    cache::fetch_using,
    folder::{self, Request, Response},
    game_check::{
        app, find_game, game_command, game_folder, has_files, install, legacy_exe, placed_exe,
        placed_exes, set_key,
    },
    pending::{clear_pending, record_pending, take_back},
    probe::capability,
    process::run_folder,
    record::accepted,
};
use crate::state::Shared;
use anyhow::{Context, Result, anyhow, bail};
use butterpollo_session::{neural, state::App};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub fn running(state: &State, uuid: &str) -> bool {
    state.jobs.get(uuid).is_some_and(Job::is_running)
}

pub fn start_job(
    h: &Shared,
    uuid: &str,
    action: &'static str,
    phase: &'static str,
) -> Result<(), Refusal> {
    let mut state = h.neural.lock();
    if running(&state, uuid) {
        return Err(Refusal::Busy);
    }
    forget_game_check(&mut state, uuid);
    state.jobs.insert(uuid.into(), Job::new(action, phase));
    Ok(())
}

pub fn update_job(h: &Shared, uuid: &str, change: impl FnOnce(&mut Job)) {
    let mut state = h.neural.lock();
    forget_game_check(&mut state, uuid);
    if let Some(job) = state.jobs.get_mut(uuid) {
        change(job);
    }
}

/// A job changes the game's folder: its own check is asked again, and one
/// that runs now is not kept ([`super::game_check::game_check`]).
fn forget_game_check(state: &mut State, uuid: &str) {
    state.games.remove(uuid);
    *state.generation.entry(uuid.to_owned()).or_default() += 1;
}

pub fn fail_job(h: &Shared, uuid: &str, error: String, choose_exe: bool) {
    update_job(h, uuid, |job| {
        job.phase = "failed";
        job.error = Some(neural::shown(&error));
        job.choose_exe = choose_exe;
    });
}

/// Turn the switch on: download the release unless it is cached, then place
/// it beside the game's .exe (`exe`, the user's pick, or found). A game with
/// Rubylight 3.0's AMDNR has it taken out first, once the game passed every
/// check and the download is in. Runs on.
pub fn enable(h: &Shared, uuid: &str, exe: Option<PathBuf>) -> Result<(), Refusal> {
    if !accepted(h) {
        return Err(Refusal::NotAccepted);
    }
    let app = app(h, uuid);
    // The runtime beside the program the user picked, else the one the app names.
    let folder = exe
        .as_deref()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .or_else(|| app.as_ref().and_then(game_folder));
    let capability_ok = capability(h, folder.as_deref()).map(|_| ());
    let already_placed = app.as_ref().is_some_and(|a| placed_exe(a).is_some());
    neural::enable_preconditions(
        true,
        capability_ok,
        app.is_some(),
        already_placed,
        crate::updater::streaming(h),
    )?;
    let app = app.ok_or(Refusal::NotFound)?;
    start_job(h, uuid, "install", "download")?;
    let (h, uuid) = (h.clone(), uuid.to_owned());
    tokio::spawn(async move {
        // The earlier upscaling's folder is the game's.
        let exe = exe
            .or_else(|| legacy_exe(&app))
            .or_else(|| game_command(&app));
        let install = install(&app);
        // The game's program, its DLSS frame generation and Valve Anti-Cheat
        // before the download: a game that is refused never waits for it.
        // Asked again after it, as the game may have changed meanwhile.
        let checked = {
            let (h, app, exe, install) = (h.clone(), app.clone(), exe.clone(), install.clone());
            tokio::task::spawn_blocking(move || find_game(&h, &app, exe, install)).await
        };
        match checked {
            Ok(Ok(Response::Found { .. })) => {}
            Ok(response) => return finish_install(&h, &uuid, &app, None, response),
            Err(error) => {
                return fail_job(&h, &uuid, format!("the install stopped: {error}"), false);
            }
        }
        let progress = |done, total| {
            update_job(&h, &uuid, |job| {
                job.done = done;
                job.total = total;
            })
        };
        let (using, package) = match fetch_using(&h, &progress).await {
            Ok(cached) => cached,
            Err(error) => {
                // The details name the release's files; the console says what happened.
                tracing::warn!(app = %app.name, error = %format!("{error:#}"), "Neural Rendering: the download failed");
                let shown = if error.is::<StreamStarted>() {
                    StreamStarted.to_string()
                } else {
                    DOWNLOAD_FAILED.into()
                };
                return fail_job(&h, &uuid, shown, false);
            }
        };
        update_job(&h, &uuid, |job| {
            (job.phase, job.done, job.total) = ("place", 0, 0)
        });
        let package = package.with(using.folder.clone());
        let worker = h.clone();
        let task_uuid = uuid.clone();
        let finished = tokio::task::spawn_blocking(move || {
            let progress = |done, total| {
                update_job(&worker, &task_uuid, |job| {
                    (job.done, job.total) = (done, total)
                })
            };
            let mut recorded = None;
            let response = (|| {
                let _reading = worker.neural.reading();
                let _using = using;
                let exe = match find_game(&worker, &app, exe, install.clone())? {
                    Response::Found { exe } => exe,
                    other => return Ok(other),
                };
                // Rubylight 3.0's AMDNR goes first: the two can't share a
                // game. Every refusal the install gives is asked before, with
                // AMDNR taken as gone, so a game refused keeps it. Once it is
                // out, the app no longer names it, whatever the install does
                // next; a file of it that stayed in the folder the install
                // goes to stops the install. AMDNR beside another program
                // (the launcher, with the game's own .exe chosen now) is
                // judged in its own folder, and what stays there is named.
                if let Some(legacy) = legacy_exe(&app) {
                    let check = Request::Check {
                        exe: Some(exe.clone()),
                        install: install.clone(),
                        package: package.clone(),
                        legacy: Some(legacy.clone()),
                    };
                    match run_folder(check, &|_, _| {})? {
                        Response::Found { .. } => {}
                        other => return Ok(other),
                    }
                    update_job(&worker, &task_uuid, |job| job.phase = "remove");
                    match remove_beside(&app.name, &legacy, Some(&exe), false)? {
                        // Left in another folder: its record stays, so the
                        // console still offers its removal, which names what
                        // stays then.
                        (Removed::Done(_), true) => {}
                        (Removed::Done(kept), false) if !kept.is_empty() => {
                            set_key(&worker, &task_uuid, LEGACY_KEY, None)?;
                            update_job(&worker, &task_uuid, |job| {
                                job.kept = kept.iter().map(|path| neural::shown(path)).collect();
                            });
                            return Ok(Response::Error {
                                message: folder::kept_text(&kept, true),
                            });
                        }
                        (Removed::Done(_), false) => set_key(&worker, &task_uuid, LEGACY_KEY, None)?,
                        (Removed::Unreachable(folder), _) => {
                            return Ok(Response::Error {
                                message: format!(
                                    "{folder} can't be reached, so the earlier upscaling could not be removed; connect its drive and try again"
                                ),
                            });
                        }
                    }
                    update_job(&worker, &task_uuid, |job| job.phase = "place");
                }
                // The game's program first: its removal is recorded before
                // anything is written, so an install cut short (a timeout,
                // a host restart) is taken out by the pending removals.
                let pending = Pending {
                    name: app.name.clone(),
                    exe: exe.clone(),
                    uuid: Some(task_uuid.clone()),
                };
                record_pending(&worker, std::slice::from_ref(&pending))?;
                recorded = Some(exe.clone());
                let request = Request::Install {
                    exe: Some(exe),
                    install,
                    package,
                };
                run_folder(request, &progress)
            })();
            // Saving the app and any taking back block too.
            finish_install(&worker, &task_uuid, &app, recorded.as_deref(), response);
        })
        .await;
        if let Err(error) = finished {
            fail_job(&h, &uuid, format!("the install stopped: {error}"), false);
        }
    });
    Ok(())
}

/// The install's end: the app saved with the files, or the job failed. The
/// removal `recorded` before the install goes once the files are in and
/// saved, or nothing was written; otherwise it takes out what is left. Blocks.
pub fn finish_install(
    h: &Shared,
    uuid: &str,
    app: &App,
    recorded: Option<&Path>,
    response: Result<Response>,
) {
    let clear = || {
        if let Some(exe) = recorded {
            clear_pending(h, exe);
        }
    };
    match response {
        Ok(Response::Placed {
            exe,
            release,
            proxy,
            mode,
            ..
        }) => {
            if let Err(error) = set_key(
                h,
                uuid,
                KEY,
                Some(neural::app_key_value(&exe, &release, &proxy, mode)),
            ) {
                // The app went while the job ran (a library sync dropped it)
                // or could not be saved: nothing would remove the files later.
                tracing::warn!(app = %app.name, error = %format!("{error:#}"), "Neural Rendering: the app could not be saved with frame generation; taking it out again");
                let after = match take_back(h, uuid, &app.name, &exe) {
                    Ok(true) => {
                        clear();
                        "frame generation was taken out again".to_owned()
                    }
                    Ok(false) => "frame generation's removal is pending until the game's folder can be changed".to_owned(),
                    Err(error) => format!("{error:#}"),
                };
                return fail_job(
                    h,
                    uuid,
                    format!("the app could not be saved ({error:#}); {after}"),
                    false,
                );
            }
            clear();
            tracing::info!(app = %app.name, folder = %exe.parent().unwrap_or(&exe).display(), %release, %proxy, ?mode, "Neural Rendering: frame generation installed");
            update_job(h, uuid, |job| job.phase = "done");
        }
        // Refused before anything was written.
        Ok(Response::ChooseExe { reason }) => {
            clear();
            tracing::info!(app = %app.name, %reason, "Neural Rendering needs the game's program chosen");
            fail_job(h, uuid, reason, true);
        }
        Ok(Response::Refused { reason }) => {
            clear();
            tracing::warn!(app = %app.name, %reason, "Neural Rendering refused the game");
            fail_job(h, uuid, reason, false);
        }
        Ok(Response::Error { message }) => {
            tracing::warn!(app = %app.name, error = %message, "Neural Rendering could not be installed");
            fail_job(h, uuid, message, false);
        }
        Ok(
            Response::Removed { .. }
            | Response::Left { .. }
            | Response::Unreachable { .. }
            | Response::NotWritable { .. }
            | Response::Found { .. },
        ) => fail_job(h, uuid, "unexpected helper answer".into(), false),
        Err(error) => {
            tracing::warn!(app = %app.name, error = %format!("{error:#}"), "Neural Rendering could not be installed");
            fail_job(h, uuid, format!("{error:#}"), false);
        }
    }
}

/// Turn the switch off: remove what was placed, frame generation or
/// Rubylight 3.0's upscaling. Runs on.
pub fn disable(h: &Shared, uuid: &str) -> Result<(), Refusal> {
    removal(h, uuid, false)
}

/// Remove only Rubylight 3.0's upscaling, which frame generation installed
/// beside another program left a record of: frame generation stays. Runs on.
pub fn remove_earlier(h: &Shared, uuid: &str) -> Result<(), Refusal> {
    removal(h, uuid, true)
}

pub fn removal(h: &Shared, uuid: &str, earlier: bool) -> Result<(), Refusal> {
    let app = app(h, uuid).ok_or(Refusal::NotFound)?;
    // The streamed game may be the one: it holds the DLLs loaded, so the
    // removal would stop at the first and ask to close the game mid-stream.
    if crate::updater::streaming(h) {
        return Err(Refusal::Streaming);
    }
    start_job(h, uuid, "remove", "remove")?;
    let (h, uuid) = (h.clone(), uuid.to_owned());
    let removed = move |app: &App| match legacy_exe(app).filter(|_| earlier) {
        Some(legacy) => remove_beside(&app.name, &legacy, None, true).map(|(removed, _)| removed),
        None if earlier => Ok(Removed::Done(Vec::new())),
        None => remove(app),
    };
    tokio::task::spawn_blocking(move || match removed(&app) {
        Ok(Removed::Done(kept)) => {
            let off = if earlier {
                Ok(())
            } else {
                set_key(&h, &uuid, KEY, None)
            };
            match off.and_then(|()| set_key(&h, &uuid, LEGACY_KEY, None)) {
                Ok(()) => update_job(&h, &uuid, |job| {
                    job.phase = "done";
                    job.kept = kept.iter().map(|path| neural::shown(path)).collect();
                }),
                Err(error) => fail_job(&h, &uuid, format!("{error:#}"), false),
            }
        }
        Ok(Removed::Unreachable(folder)) => {
            tracing::warn!(app = %app.name, %folder, "Neural Rendering could not be removed: the game's folder can't be reached");
            fail_job(
                &h,
                &uuid,
                if earlier {
                    format!(
                        "{folder} can't be reached; connect its drive and remove the earlier upscaling again"
                    )
                } else {
                    format!("{folder} can't be reached; connect its drive and turn it off again")
                },
                false,
            )
        }
        Err(error) => fail_job(&h, &uuid, format!("{error:#}"), false),
    });
    Ok(())
}

/// Remove what the host placed in the app's game folders. Blocks.
pub fn remove(app: &App) -> Result<Removed> {
    let mut kept = Vec::new();
    for exe in placed_exes(app) {
        match remove_at(&app.name, &exe)? {
            Removed::Done(more) => kept.extend(more),
            unreachable => return Ok(unreachable),
        }
    }
    Ok(Removed::Done(kept))
}

/// Remove whatever the host placed beside `exe`: frame generation, and
/// Rubylight 3.0's AMDNR, each by its manifest. Blocks.
pub(super) fn remove_at(name: &str, exe: &Path) -> Result<Removed> {
    remove_beside(name, exe, None, false).map(|(removed, _)| removed)
}

/// [`remove_at`], and whether AMDNR's record stays for what it left beside
/// another program than `beside`, the one an install goes beside
/// ([`Response::Left`]); with `earlier`, only AMDNR. Blocks.
fn remove_beside(
    name: &str,
    exe: &Path,
    beside: Option<&Path>,
    earlier: bool,
) -> Result<(Removed, bool)> {
    let request = Request::Remove {
        exe: exe.into(),
        beside: beside.map(Path::to_path_buf),
        earlier,
    };
    let (kept, record) = match run_folder(request, &|_, _| {})? {
        Response::Removed { kept } => (kept, false),
        Response::Left { kept } => (kept, true),
        Response::Unreachable { folder } => return Ok((Removed::Unreachable(folder), false)),
        Response::Error { message }
        | Response::Refused { reason: message }
        | Response::ChooseExe { reason: message } => {
            tracing::warn!(app = %name, error = %message, "Neural Rendering could not be removed");
            bail!("{message}")
        }
        Response::Placed { .. } | Response::NotWritable { .. } | Response::Found { .. } => {
            bail!("unexpected helper answer")
        }
    };
    let folder = exe.parent().unwrap_or(exe).display().to_string();
    if kept.is_empty() {
        tracing::info!(app = %name, %folder, "Neural Rendering removed");
    } else {
        tracing::warn!(app = %name, %folder, kept = ?kept, "Neural Rendering removed; files that changed since, are not Rubylight's or were held open were left");
    }
    Ok((Removed::Done(kept), record))
}

/// Before an app is deleted: the switch's slot for it, held until the caller
/// drops the [`Removing`], once the app is gone. A delete of several apps
/// takes every slot before any file is removed, so one busy app refuses the
/// delete with nothing changed. Then [`Removing::remove_mod`].
pub fn reserve(h: &Shared, app: &App) -> Result<Removing> {
    // As `disable`: the streamed game may hold the DLLs loaded, so the
    // removal would stop at the first and ask to close the game mid-stream.
    // Refused before any slot is taken or file touched; the app stays.
    if has_files(app) && crate::updater::streaming(h) {
        return Err(StreamRunning(app.name.clone()).into());
    }
    let uuid = app.extra.get("uuid").and_then(Value::as_str);
    // An install not finished has no key to remove by yet, and a launch's
    // update would race the removal.
    Ok(Removing::take(h, uuid).map_err(|_| Busy(app.name.clone()))?)
}

/// One app's [`reserve`] and [`Removing::remove_mod`].
#[cfg(any(test, feature = "test-support"))]
pub fn before_delete(h: &Shared, app: &App) -> Result<Removing> {
    let slot = reserve(h, app)?;
    slot.remove_mod(app)?;
    Ok(slot)
}

/// The switch's slot taken for an app's removal, so no install or launch
/// update writes its folder meanwhile; done when dropped. A failure the
/// slot held before (an install that failed, its removal pending) shows
/// again after, not a removal the user never asked for.
pub struct Removing {
    h: Shared,
    uuid: Option<String>,
    previous: Option<Job>,
    restored: bool,
}
impl Removing {
    pub(super) fn take(h: &Shared, uuid: Option<&str>) -> Result<Self, Refusal> {
        let previous = if let Some(uuid) = uuid {
            let mut state = h.neural.lock();
            if running(&state, uuid) {
                return Err(Refusal::Busy);
            }
            let previous = state.jobs.get(uuid).cloned();
            forget_game_check(&mut state, uuid);
            state.jobs.insert(uuid.into(), Job::new("remove", "remove"));
            previous
        } else {
            None
        };
        Ok(Self {
            h: h.clone(),
            uuid: uuid.map(str::to_owned),
            previous,
            restored: false,
        })
    }

    /// Release the slot and restore the job that was there before without
    /// running a removal (used when a pending entry turned out to be stale or
    /// already cleared after the slot opened).
    pub(super) fn restore(mut self) {
        self.restored = true;
        if let Some(uuid) = &self.uuid {
            let mut state = self.h.neural.lock();
            if state
                .jobs
                .get(uuid)
                .is_some_and(|job| job.phase == "remove")
            {
                match self.previous.take() {
                    Some(previous) => {
                        state.jobs.insert(uuid.clone(), previous);
                    }
                    None => {
                        state.jobs.remove(uuid);
                    }
                }
            }
        }
    }

    /// For an app with files placed: take them out of its folder, or keep
    /// the app (an error) so nothing is left behind unnoticed. A folder on a
    /// drive that is not connected becomes a pending removal instead, so a
    /// game on a drive that is gone for good can still be deleted. Blocks.
    pub fn remove_mod(&self, app: &App) -> Result<()> {
        if !has_files(app) {
            return Ok(());
        }
        let kept =
            "Neural Rendering could not be removed from the game's folder, so the app was kept";
        let removed = (|| {
            if let Removed::Unreachable(folder) = remove(app).context(kept)? {
                record_pending(&self.h, &Pending::of(app)).context(kept)?;
                tracing::warn!(app = %app.name, %folder, "Neural Rendering: the game's folder can't be reached; its removal is pending");
            }
            anyhow::Ok(())
        })()
        .map_err(|error| {
            // The app delete's answer names no provider path; the log has it.
            let full = format!("{error:#}");
            tracing::warn!(app = %app.name, error = %full, "Neural Rendering could not be removed for the app's delete");
            anyhow!(neural::shown(&full))
        });
        if let (Err(error), Some(uuid)) = (&removed, &self.uuid) {
            fail_job(&self.h, uuid, error.to_string(), false);
        }
        removed
    }
}
impl Drop for Removing {
    fn drop(&mut self) {
        if self.restored {
            return;
        }
        if let Some(uuid) = &self.uuid {
            let failed = self.previous.take().filter(|job| job.phase == "failed");
            update_job(&self.h, uuid, |job| {
                if job.phase == "remove" {
                    match failed {
                        Some(failed) => *job = failed,
                        None => job.phase = "done",
                    }
                }
            });
        }
    }
}
