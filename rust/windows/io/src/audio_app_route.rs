//! Game-only audio (a background stream's game, or `audio_game_only`) leaves
//! the PC's default playback device alone, so the game played on the PC's
//! speakers as well as on the client (#59). The game's own process is played
//! on a virtual sink nobody hears instead (Windows' per-app output device, as
//! in Settings > Sound > Volume mixer); process loopback captures it there.
//!
//! Windows keeps an app's output device for its program, after it exits, so
//! the choice is journalled first and put back once the program runs again
//! when it exited mid-stream or the host crashed: right away when a process
//! of it still runs, else as soon as it next starts (a watcher sees
//! processes start through WMI, `policy::Starts`; without it, it
//! looks every `watcher::WATCH`). A device the user picked for the game during the
//! stream stays.
//! Windows sets a device only for a running process, so a host that stops
//! the game itself puts it back first ([`put_back`]); an uninstall can't put
//! back a game that isn't running (Volume mixer's Reset does).
//! Windows has no device for a process that hasn't opened its sound yet: it
//! refuses both calls with E_INVALIDARG ([`NoSoundYet`], #59: every game
//! routed as it started). Once the process opened it, a change moves the
//! sound it plays at once, so a route is tried again until then.

use anyhow::{Context, Result};
use butterpollo_config::config::Config;
#[cfg(test)]
use std::time::{Duration, Instant};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
};
use windows::Win32::Media::Audio::*;

mod policy;
mod sessions;
mod watcher;

use policy::{App, Apps, Journal, Windows, load, same_program, save};
#[cfg(test)]
use policy::{FactoryVtbl, Starts, path, refusal};
pub use sessions::OnThePc;
use sessions::{MMDEVAPI, RENDER_INTERFACE, active_sessions, endpoint_id, plays_elsewhere};
use watcher::watch;
#[cfg(test)]
use watcher::{Reported, SILENT_FOR, SWEEP, WATCH, waits_still, watch_starts};

/// The roles a game's stream opens on; communications stays the user's.
const ROLES: [ERole; 2] = [eConsole, eMultimedia];

/// The process hasn't opened its sound yet, so Windows has no device for it:
/// try again once it has ([`no_sound_yet`]).
#[derive(Debug)]
pub struct NoSoundYet;
impl std::fmt::Display for NoSoundYet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the game has not opened its sound yet")
    }
}
impl std::error::Error for NoSoundYet {}
/// Whether `error` is [`NoSoundYet`]: the route is tried again later.
pub fn no_sound_yet(error: &anyhow::Error) -> bool {
    error.is::<NoSoundYet>()
}

#[derive(Default)]
struct State {
    /// Streams playing each program (by path) on the sink right now.
    active: BTreeMap<String, usize>,
    /// Programs the host is stopping, put back already ([`put_back`]): not
    /// routed again until they stopped, by how many stops.
    closing: BTreeMap<String, usize>,
    watching: bool,
}
static STATE: Mutex<State> = Mutex::new(State {
    active: BTreeMap::new(),
    closing: BTreeMap::new(),
    watching: false,
});

/// How [`route_with`] left the process.
#[derive(Debug, PartialEq)]
enum Routed {
    /// On the sink, read back from Windows.
    Sink,
    /// On a device the user chose for the game during this stream: kept.
    Users(String),
    /// Left alone: the host is stopping the program.
    Closing,
}

/// Play process `pid` (program `exe`, lower case) on `sink`.
fn route_with(
    apps: &dyn Apps,
    state: &mut State,
    directory: &Path,
    pid: u32,
    exe: &str,
    sink: &str,
) -> Result<Routed> {
    if state
        .closing
        .keys()
        .any(|closing| same_program(closing, exe))
    {
        return Ok(Routed::Closing);
    }
    let mut journal = load(directory)?;
    let mut current: [String; 2] = Default::default();
    for (slot, role) in current.iter_mut().zip(ROLES) {
        *slot = apps.get(pid, role)?;
    }
    let is_sink = |device: &str| device.eq_ignore_ascii_case(sink);
    match journal
        .apps
        .iter_mut()
        .find(|app| same_program(&app.exe, exe))
    {
        Some(app) => {
            // The program waits for its restore, or another stream plays it:
            // its journalled device stays the user's. One the user chose for
            // it meanwhile (neither the new sink, the previously applied sink,
            // the saved device, nor Windows Default) is kept.
            if let Some(chosen) = current
                .iter()
                .zip(&app.before)
                .find(|(now, before)| {
                    !now.is_empty()
                        && !is_sink(now)
                        && !now.eq_ignore_ascii_case(&app.applied)
                        && !now.eq_ignore_ascii_case(before)
                })
                .map(|(now, _)| now.clone())
            {
                return Ok(Routed::Users(chosen));
            }
            // If the user reset a role to Windows Default ("") in Volume Mixer
            // while a prior non-empty device was saved, record that reset so
            // teardown restores Default instead of the old device.
            for (now, before) in current.iter().zip(&mut app.before) {
                if now.is_empty() && !before.is_empty() {
                    before.clear();
                }
            }
            app.applied = sink.into();
        }
        None => journal.apps.push(App {
            exe: exe.into(),
            // Already on the sink with no record of it (the record was
            // deleted): the game goes back to the default, never stays here.
            before: current.map(|device| {
                if is_sink(&device) {
                    String::new()
                } else {
                    device
                }
            }),
            applied: sink.into(),
        }),
    }
    save(directory, &journal)?;
    *state.active.entry(exe.into()).or_default() += 1;
    let applied = ROLES.into_iter().try_for_each(|role| {
        apps.set(pid, role, sink)?;
        let read = apps.get(pid, role)?;
        anyhow::ensure!(
            is_sink(&read),
            "Windows kept {} for the game, not the virtual sink",
            if read.is_empty() {
                "the default device"
            } else {
                read.as_str()
            }
        );
        Ok(())
    });
    if let Err(error) = applied {
        // Whatever this set goes back now, or when the program next runs.
        if let Err(restore) = release_with(apps, state, directory, exe) {
            tracing::warn!(exe, error = %format!("{restore:#}"), "game audio device could not be restored after a failed route");
        }
        // Not tried again as if nothing changed: it was journalled.
        if no_sound_yet(&error) {
            return Err(anyhow::anyhow!(
                "Windows lost the game's sound while moving it"
            ));
        }
        return Err(error);
    }
    Ok(Routed::Sink)
}

/// One stream stopped playing `exe` on the sink; the last puts it back.
/// Returns whether a restore still waits for the program to run.
fn release_with(apps: &dyn Apps, state: &mut State, directory: &Path, exe: &str) -> Result<bool> {
    if !release(state, exe) {
        return Ok(false);
    }
    restore_with(apps, state, directory)
}
/// One stream stopped playing `exe`: whether it was the last.
fn release(state: &mut State, exe: &str) -> bool {
    if let Some(users) = state.active.get_mut(exe) {
        *users -= 1;
        if *users > 0 {
            return false;
        }
        state.active.remove(exe);
    }
    true
}

/// Programs `exes` are about to be stopped with every stream that plays
/// them: put back now, while they run, whatever the streams' count.
/// Returns whether a restore still waits for a program to run.
#[cfg(test)]
fn put_back_with(
    apps: &dyn Apps,
    state: &mut State,
    directory: &Path,
    exes: &[String],
) -> Result<bool> {
    forget(state, exes);
    restore_with(apps, state, directory)
}
/// No stream plays programs `exes` any more.
fn forget(state: &mut State, exes: &[String]) {
    state
        .active
        .retain(|active, _| !exes.iter().any(|exe| same_program(active, exe)));
}

/// Put back every journalled program no stream plays that runs now.
/// Returns whether one still waits to run (a stream's own program is put
/// back when it ends).
fn restore_with(apps: &dyn Apps, state: &State, directory: &Path) -> Result<bool> {
    let waiting = waiting(state, &load(directory)?);
    if waiting.is_empty() {
        return Ok(false);
    }
    let running = apps.running(&waiting.iter().map(String::as_str).collect::<Vec<_>>())?;
    restore_running(apps, state, directory, &running)
}

/// The journalled programs no stream plays.
fn waiting(state: &State, journal: &Journal) -> Vec<String> {
    journal
        .apps
        .iter()
        .filter(|app| idle(state, app))
        .map(|app| app.exe.clone())
        .collect()
}
fn idle(state: &State, app: &App) -> bool {
    !state.active.keys().any(|exe| same_program(&app.exe, exe))
}

/// [`restore_with`] holding `state` only to change devices: the process
/// listing, its slow part, runs without it, so a stream's route never waits
/// behind it. A program a stream routed meanwhile is left to that stream.
/// The lock comes back for a decision that must follow the restore.
fn restore_unlocked<'a>(
    apps: &dyn Apps,
    state: &'a Mutex<State>,
    directory: &Path,
) -> (MutexGuard<'a, State>, Result<bool>) {
    let waiting = {
        let state = state.lock().unwrap();
        match load(directory) {
            Ok(journal) => waiting(&state, &journal),
            Err(error) => return (state, Err(error)),
        }
    };
    let running = if waiting.is_empty() {
        Ok(vec![])
    } else {
        apps.running(&waiting.iter().map(String::as_str).collect::<Vec<_>>())
    };
    let state = state.lock().unwrap();
    let result = running.and_then(|running| restore_running(apps, &state, directory, &running));
    (state, result)
}

/// [`restore_unlocked`] for this host's streams, on Windows.
fn restore_now(directory: &Path) -> (MutexGuard<'static, State>, Result<bool>) {
    match crate::os_helpers::ComGuard::new().and_then(|com| Ok((com, Windows::new()?))) {
        Ok((_com, apps)) => restore_unlocked(&apps, &STATE, directory),
        Err(error) => (STATE.lock().unwrap(), Err(error)),
    }
}

/// Put back the journalled programs no stream plays that are `running`.
fn restore_running(
    apps: &dyn Apps,
    state: &State,
    directory: &Path,
    running: &[(u32, String)],
) -> Result<bool> {
    let mut journal = load(directory)?;
    let mut failed = None;
    let mut done = vec![];
    for app in journal.apps.iter().filter(|app| idle(state, app)) {
        let restore = |pid: u32| {
            ROLES
                .into_iter()
                .zip(&app.before)
                .try_for_each(|(role, before)| -> Result<()> {
                    // A device the user chose for the game meanwhile stays.
                    if !apps.get(pid, role)?.eq_ignore_ascii_case(&app.applied) {
                        return Ok(());
                    }
                    if let Err(error) = apps.set(pid, role, before) {
                        // The device it had is gone (unplugged, uninstalled): it
                        // follows the default instead of waiting forever.
                        tracing::warn!(exe = app.exe, device = before, error = %format!("{error:#}"), "the game's own audio device is unavailable; it follows the default device again");
                        apps.set(pid, role, "")?;
                    }
                    Ok(())
                })
        };
        // Windows keeps one device per program: any of its processes that
        // opened its sound sets it, a helper that never does (a launcher
        // stub, a crash reporter) doesn't hold it up.
        let mut result = None;
        for (pid, _) in running
            .iter()
            .filter(|(_, exe)| same_program(&app.exe, exe))
        {
            let tried = restore(*pid);
            let more = matches!(&tried, Err(error) if no_sound_yet(error));
            result = Some(tried);
            if !more {
                break;
            }
        }
        let Some(result) = result else {
            continue;
        };
        match result {
            Ok(()) => {
                tracing::info!(exe = app.exe, "game audio device restored");
                done.push(app.exe.clone());
            }
            // Just started: put back once it opened its sound
            // (`watcher::watch_starts` looks again soon); Windows moves no sound a
            // program opened already, so that plays where it opened.
            Err(error) if no_sound_yet(&error) => {
                failed.get_or_insert(error);
            }
            Err(error) => {
                tracing::warn!(exe = app.exe, error = %format!("{error:#}"), "game audio device could not be restored");
                if failed.as_ref().is_none_or(no_sound_yet) {
                    failed = Some(error);
                }
            }
        }
    }
    journal.apps.retain(|app| !done.contains(&app.exe));
    if !done.is_empty() {
        save(directory, &journal)?;
    }
    if let Some(error) = failed {
        return Err(error);
    }
    Ok(journal.apps.iter().any(|app| idle(state, app)))
}

/// The game's process played on a virtual sink while this lives.
pub struct AppRoute {
    directory: PathBuf,
    exe: String,
    pid: u32,
    /// The sink's endpoint ID, and its name for messages.
    sink_id: String,
    pub sink: String,
    /// How [`route_with`] left the process: routed to the virtual sink, kept on
    /// a user-chosen device, or left alone while the host stops the game.
    routed: Routed,
}
impl AppRoute {
    /// Play process `pid` on the configured `virtual_sink`, else on Steam
    /// Streaming Speakers. Fails when there is neither, or when Windows
    /// does not keep the choice; with [`NoSoundYet`], changing nothing, when
    /// the process hasn't opened its sound yet.
    pub fn acquire(config: &Config, directory: &Path, pid: u32) -> Result<Self> {
        let _com = crate::os_helpers::ComGuard::new()?;
        let available = crate::audio_route::endpoints()?;
        let configured = config.get("virtual_sink", "");
        let sink = if configured.is_empty() {
            available.iter().find(|endpoint| endpoint.virtual_sink).context(
                "there is no virtual playback device: install Steam Streaming Speakers or set a virtual sink in Audio settings",
            )?
        } else {
            crate::audio_route::find(&available, configured)
                .context("the virtual sink set in Audio settings is unavailable")?
        };
        let exe = crate::process::image_path(pid)
            .context("the game's program can't be read")?
            .to_lowercase();
        let apps = Windows::new()?;
        let routed = route_with(
            &apps,
            &mut STATE.lock().unwrap(),
            directory,
            pid,
            &exe,
            &format!("{MMDEVAPI}{}{RENDER_INTERFACE}", sink.id),
        );
        let routed = match routed {
            Ok(Routed::Sink) => {
                tracing::info!(
                    exe,
                    pid,
                    sink = sink.name,
                    "game audio plays on the virtual sink"
                );
                Routed::Sink
            }
            Ok(Routed::Closing) => {
                tracing::info!(
                    exe,
                    pid,
                    "game audio not moved: the host is stopping the game"
                );
                Routed::Closing
            }
            Ok(Routed::Users(device)) => {
                tracing::info!(
                    exe,
                    pid,
                    device,
                    "game audio stays on the device the user chose for it"
                );
                Routed::Users(device)
            }
            // Nothing changed or journalled yet: the caller tries again.
            Err(error) if no_sound_yet(&error) => return Err(error),
            Err(error) => {
                watch(directory);
                return Err(error);
            }
        };
        Ok(Self {
            directory: directory.into(),
            exe,
            pid,
            sink_id: sink.id.clone(),
            sink: sink.name.clone(),
            routed,
        })
    }
    /// The routed program's path, in lower case.
    pub fn program(&self) -> &str {
        &self.exe
    }
    /// The device the user chose for the game, which it stays on.
    pub fn users_device(&self) -> Option<&str> {
        match &self.routed {
            Routed::Users(device) => Some(device.as_str()),
            Routed::Sink | Routed::Closing => None,
        }
    }
    /// Which of these routes' processes, and of the game's processes left
    /// `unrouted`, play where the PC plays the game, not on the routes' sink
    /// ([`plays_elsewhere`]).
    pub fn plays_elsewhere(routes: &[&AppRoute], unrouted: &[u32]) -> Result<OnThePc> {
        let Some(first) = routes
            .iter()
            .find(|route| matches!(route.routed, Routed::Sink))
        else {
            return Ok(OnThePc::default());
        };
        let pids: Vec<u32> = routes
            .iter()
            .filter(|r| matches!(r.routed, Routed::Sink))
            .map(|r| r.pid)
            .chain(unrouted.iter().copied())
            .collect();
        let journal = load(&first.directory)?;
        let own: Vec<&str> = journal
            .apps
            .iter()
            .filter(|app| routes.iter().any(|r| same_program(&app.exe, &r.exe)))
            .flat_map(|app| app.before.iter().map(|device| endpoint_id(device)))
            .collect();
        let defaults = {
            let _com = crate::os_helpers::ComGuard::new()?;
            crate::audio_route::defaults()?
        };
        Ok(plays_elsewhere(
            &pids,
            &active_sessions()?,
            &first.sink_id,
            &defaults,
            &own,
        ))
    }
}
impl Drop for AppRoute {
    fn drop(&mut self) {
        if !matches!(self.routed, Routed::Sink) {
            return;
        }
        if !release(&mut STATE.lock().unwrap(), &self.exe) {
            return;
        }
        let (state, result) = restore_now(&self.directory);
        drop(state);
        match result {
            Ok(false) => {}
            Ok(true) => watch(&self.directory),
            Err(error) if no_sound_yet(&error) => watch(&self.directory),
            Err(error) => {
                tracing::warn!(exe = self.exe, error = %format!("{error:#}"), "game audio device could not be restored; retrying while the host runs");
                watch(&self.directory);
            }
        }
    }
}

/// Restore what an interrupted stream left (host start), and keep looking
/// for programs that are not running yet.
/// An error leaves the watcher retrying.
pub fn recover(directory: &Path) -> Result<()> {
    match load(directory) {
        Ok(journal) if journal.apps.is_empty() => return Ok(()),
        Ok(_) => {}
        Err(error) => {
            watch(directory);
            return Err(error);
        }
    }
    let (state, result) = restore_now(directory);
    drop(state);
    if !matches!(result, Ok(false)) {
        watch(directory);
    }
    match result {
        // The watcher puts it back once it opened its sound.
        Err(error) if !no_sound_yet(&error) => Err(error),
        _ => Ok(()),
    }
}

/// The host is about to stop processes `pids` (a game, a session's
/// instances) and every stream playing them: their programs go back to
/// their own devices now, while Windows can still set them. A stream's
/// route that lets go later finds nothing left to put back. `pids` is
/// asked for the programs. Until the guard drops, once they stopped, no
/// stream routes them again.
pub fn put_back(directory: &Path, pids: impl FnOnce() -> Result<Vec<u32>>) -> Result<Closing> {
    let exes: Vec<String> = pids()?
        .iter()
        .filter_map(|&pid| crate::process::image_path(pid))
        .map(|exe| exe.to_lowercase())
        .collect();
    let closing = Closing::mark(&mut STATE.lock().unwrap(), exes);
    if load(directory)?.apps.is_empty() {
        return Ok(closing);
    }
    forget(&mut STATE.lock().unwrap(), &closing.exes);
    let (state, result) = restore_now(directory);
    drop(state);
    if !matches!(result, Ok(false)) {
        watch(directory);
    }
    match result {
        Err(error) if !no_sound_yet(&error) => Err(error),
        _ => Ok(closing),
    }
}

/// Programs the host stops: routed again once this drops.
pub struct Closing {
    exes: Vec<String>,
}
impl Closing {
    fn mark(state: &mut State, exes: Vec<String>) -> Self {
        for exe in &exes {
            *state.closing.entry(exe.clone()).or_default() += 1;
        }
        Self { exes }
    }
    fn unmark(&self, state: &mut State) {
        for exe in &self.exes {
            if let Some(count) = state.closing.get_mut(exe) {
                *count -= 1;
                if *count == 0 {
                    state.closing.remove(exe);
                }
            }
        }
    }
}
impl Drop for Closing {
    fn drop(&mut self) {
        self.unmark(&mut STATE.lock().unwrap());
    }
}

#[cfg(test)]
mod tests;
