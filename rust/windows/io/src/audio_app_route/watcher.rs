//! Background watcher restoring per-app audio routes when games restart.
use super::{
    STATE, State, idle, no_sound_yet,
    policy::{Apps, Starts, Windows, WindowsStarts, load, same_program},
    restore_now, restore_running, restore_unlocked,
};
use anyhow::{Context, Result};
use std::{
    path::Path,
    sync::Mutex,
    time::{Duration, Instant},
};

/// How often a pending restore looks for the program it waits for when
/// process starts can't be watched.
pub(super) const WATCH: Duration = Duration::from_secs(2);
/// How often a watcher that sees process starts still looks for every
/// program: for a restore that failed, or one a stream did meanwhile.
pub(super) const SWEEP: Duration = Duration::from_secs(30);
/// How long a running game may go without opening its sound before the
/// watcher says its device waits for it.
pub(super) const SILENT_FOR: Duration = Duration::from_secs(30);

/// One watcher per host: it ends once nothing waits. It puts a program back
/// as it starts ([`watch_starts`]); when Windows can't tell it starts, it
/// looks every [`WATCH`], which can be after the game opened its sound.
pub(super) fn watch(directory: &Path) {
    {
        let mut state = STATE.lock().unwrap();
        if state.watching {
            return;
        }
        state.watching = true;
    }
    let directory = directory.to_path_buf();
    let spawned = std::thread::Builder::new()
        .name("game audio restore".into())
        .spawn(move || {
            let watched = crate::os_helpers::ComGuard::new().and_then(|com| {
                let apps = Windows::new()?;
                let mut starts = WindowsStarts::subscribe()
                    .context("watching programs start (WMI's Win32_ProcessStartTrace)")?;
                let watched = watch_starts(&apps, &STATE, &directory, &mut starts);
                drop((starts, apps, com));
                watched
            });
            if let Err(error) = watched {
                tracing::error!(
                    error = %format!("{error:#}"),
                    "game audio restore can't see games start: a game that quit by itself goes back to its own device up to 2 s after it next starts, and may play that run on the virtual playback device"
                );
                poll(&directory);
            }
        });
    if let Err(error) = spawned {
        STATE.lock().unwrap().watching = false;
        tracing::warn!(error = %error, "game audio devices are restored at the next host start");
    }
}

/// Watches `starts` until nothing waits: a waiting program is put back as
/// it starts, and every program every [`SWEEP`]. Returns the error that
/// ended the watch of starts, with the watcher still on; a failed restore is
/// told and tried again.
pub(super) fn watch_starts(
    apps: &dyn Apps,
    state: &Mutex<State>,
    directory: &Path,
    starts: &mut dyn Starts,
) -> Result<()> {
    let mut reported = Reported::default();
    // Starts are watched already: this first look finds a program that
    // started since the restore that left it waiting, the watch any later.
    let mut sweep = true;
    // A program that started but hasn't opened its sound is looked at
    // again [`WATCH`] after, whatever else starts meanwhile.
    let mut soon: Option<Instant> = None;
    loop {
        let (mut guard, result) = if std::mem::take(&mut sweep) {
            restore_unlocked(apps, state, directory)
        } else {
            let timeout = soon.map_or(SWEEP, |at| at.saturating_duration_since(Instant::now()));
            match starts.next(timeout)? {
                None => restore_unlocked(apps, state, directory),
                Some((pid, exe)) => match load(directory) {
                    Err(error) => (state.lock().unwrap(), Err(error)),
                    Ok(journal) => {
                        let guard = state.lock().unwrap();
                        // Any other program starting costs no device call.
                        if !journal
                            .apps
                            .iter()
                            .any(|app| idle(&guard, app) && same_program(&app.exe, &exe))
                        {
                            continue;
                        }
                        let result = restore_running(apps, &guard, directory, &[(pid, exe)]);
                        (guard, result)
                    }
                },
            }
        };
        soon = matches!(&result, Err(error) if no_sound_yet(error)).then(|| Instant::now() + WATCH);
        // One lock from the restore's device changes to the decision to
        // stop: a stream ending meanwhile finds the watcher either running
        // or gone.
        if !waits_still(&mut guard, result, &mut reported) {
            return Ok(());
        }
    }
}

/// [`watch`] without process starts: a look every [`WATCH`].
fn poll(directory: &Path) {
    let mut reported = Reported::default();
    loop {
        std::thread::sleep(WATCH);
        let (mut state, result) = restore_now(directory);
        if !waits_still(&mut state, result, &mut reported) {
            return;
        }
    }
}

/// What a watcher told: an error once until it changes, and a game that
/// runs without opening its sound once it did so for [`SILENT_FOR`].
#[derive(Default)]
pub(super) struct Reported {
    error: Option<String>,
    pub(super) silent_since: Option<Instant>,
    pub(super) said_silent: bool,
}

/// After a watcher's restore: whether it watches on. It ends when nothing
/// waits; an error is told once until it changes.
pub(super) fn waits_still(
    state: &mut State,
    result: Result<bool>,
    reported: &mut Reported,
) -> bool {
    let silent = matches!(&result, Err(error) if no_sound_yet(error));
    if !silent {
        reported.silent_since = None;
        reported.said_silent = false;
    }
    match result {
        Ok(true) => true,
        Ok(false) => {
            state.watching = false;
            false
        }
        // Not a failure: it waits for the game's sound.
        Err(_) if silent => {
            let since = *reported.silent_since.get_or_insert_with(Instant::now);
            if !reported.said_silent && since.elapsed() >= SILENT_FOR {
                reported.said_silent = true;
                tracing::warn!(
                    "a game runs without opening its sound for {} s: its own audio device is put back once it does",
                    SILENT_FOR.as_secs()
                );
            }
            true
        }
        Err(error) => {
            let detail = format!("{error:#}");
            let reported = &mut reported.error;
            if reported.as_ref() != Some(&detail) {
                tracing::warn!(
                    error = detail,
                    "game audio devices could not be restored; retrying"
                );
                *reported = Some(detail);
            }
            true
        }
    }
}
