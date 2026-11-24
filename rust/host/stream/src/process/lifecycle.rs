//! Running application exit polling, game-settings process watching, menu undo, and teardown.

use super::RunningApp;
use super::commands::{MenuRun, directory, expand};
use super::focus::{ExitAction, ProcessScan, SETTINGS_WATCH, exit_action, watches_program};
use anyhow::Result;
use butterpollo_session::state::App;
use butterpollo_win_io::process::Process;
use std::{collections::HashMap, time::Duration};

impl RunningApp {
    /// Record the programs of the game as the host sees it (the app's processes, the Steam
    /// game, the game Playnite started, a store game it noticed) for its settings files: they
    /// stay while one of these still runs, also after a host crash. Launchers and store
    /// clients are left out; they stay open after a game.
    pub(super) fn watch_game(&mut self) {
        if self.settings_swap.is_none() {
            return;
        }
        self.settings_watch = std::time::Instant::now();
        let mut pids: std::collections::BTreeSet<u32> = self
            .child
            .as_ref()
            .and_then(|child| child.process_ids().ok())
            .unwrap_or_default()
            .into_iter()
            .collect();
        if let Some(tracker) = &self.steam {
            pids.extend(tracker.tracked.keys());
        }
        pids.extend(self.foreign.keys());
        pids.remove(&std::process::id());
        // By id and creation time: a shared helper or the same game started later at the desk
        // is a process of its own, and never holds the files.
        let mut processes: Vec<butterpollo_session::game_settings::Watched> = pids
            .into_iter()
            .filter_map(|pid| {
                let program = butterpollo_win_io::process::image_path(pid)?;
                let started = butterpollo_win_io::process::creation_time(pid);
                (started != 0 && watches_program(&program)).then_some(
                    butterpollo_session::game_settings::Watched {
                        pid,
                        started,
                        program,
                    },
                )
            })
            .collect();
        // A game started through Playnite is no process of this app's: its launch finds
        // them, in the game's folder (issue #82).
        if let Some(launch) = &self.playnite {
            processes.extend(
                launch
                    .take_game_processes()
                    .into_iter()
                    .filter(|process| watches_program(&process.program)),
            );
        }
        if let Some(swap) = &mut self.settings_swap {
            swap.watch(processes);
        }
    }
    /// Tell `stopped` once this app's processes have stopped, before its undo
    /// commands run. Used at shutdown to release the game's audio route right
    /// after it closes without waiting for slow undos.
    pub fn notify_when_processes_stop(&mut self, stopped: std::sync::mpsc::Sender<()>) {
        self.processes_stopped = Some(stopped);
    }
    pub fn stop(&mut self) {
        // What runs now is what the files wait for, should it outlive the stop below.
        self.watch_game();
        self.state_events.take();
        if let Some(lossless) = self.lossless.take() {
            lossless.stop();
        }
        if let Some(tracker) = self.steam.take() {
            butterpollo_win_io::process::stop_processes(&tracker.tracked, self.exit_timeout);
        }
        let foreign = std::mem::take(&mut self.foreign);
        if !foreign.is_empty() {
            butterpollo_win_io::process::stop_processes(&foreign, self.exit_timeout);
        }
        if let Some(launch) = self.playnite.take() {
            launch.stop(self.exit_timeout);
        }
        if std::mem::take(&mut self.playnite_fullscreen) {
            crate::playnite::close_fullscreen(self.exit_timeout);
        }
        if let Some(child) = self.child.take()
            && let Err(error) = child.stop_graceful(self.exit_timeout)
        {
            tracing::warn!(%error, "application could not exit gracefully");
            let _ = child.stop();
        }
        #[cfg(test)]
        if let Some(delay) = self.stop_delay.take() {
            std::thread::sleep(delay);
        }
        if let Some(stopped) = self.processes_stopped.take() {
            let _ = stopped.send(());
        }
        // Menu commands ran after the preparation, so they are undone before it.
        for index in self.menu_undo.take() {
            let Some(command) = self.menu_commands.get(index) else {
                continue;
            };
            match expand(&command.undo, &self.environment)
                .and_then(|undo| {
                    Process::shell_leaving_children(
                        &undo,
                        directory(&self.working),
                        command.elevated,
                        &self.environment,
                    )
                })
                .and_then(|p| p.wait_leaving_children(Duration::from_secs(20)))
            {
                Ok(0) => {}
                Ok(code) => {
                    tracing::warn!(code, command = %command.name, "menu command undo failed")
                }
                Err(e) => {
                    tracing::warn!(error = %e, command = %command.name, "menu command undo failed")
                }
            }
        }
        for prep in self.undo.drain(..).rev() {
            if !prep.undo.is_empty() {
                match Process::shell_leaving_children(
                    &prep.undo,
                    directory(&self.working),
                    prep.elevated,
                    &self.environment,
                )
                .and_then(|p| p.wait_leaving_children(Duration::from_secs(20)))
                {
                    Ok(0) => {}
                    Ok(code) => tracing::warn!(code, "undo command failed"),
                    Err(e) => tracing::warn!(error=%e,"undo command failed"),
                }
            }
        }
        // The game is closed: its settings files become the device's copies and the PC's own
        // go back.
        drop(self.settings_swap.take());
    }
    /// App menu command `index`, ready to start with the app's environment, and
    /// recorded so its undo runs when the app stops. None for an index the app
    /// does not have, an empty command, or one whose variables cannot be expanded.
    pub fn menu_command(&mut self, index: usize) -> Option<MenuRun> {
        let command = self.menu_commands.get(index)?;
        if command.cmd.trim().is_empty() {
            return None;
        }
        let cmd = match expand(&command.cmd, &self.environment) {
            Ok(cmd) => cmd,
            Err(error) => {
                tracing::warn!(%error, command = %command.name, "menu command not run");
                return None;
            }
        };
        let run = MenuRun {
            cmd,
            elevated: command.elevated,
            environment: self.environment.clone(),
            working: self.working.clone(),
        };
        self.menu_undo.record(index, command);
        Some(run)
    }
    /// The deferred app, once a user has signed in (checked once a second).
    pub fn take_ready_deferred(&mut self) -> Option<(App, HashMap<String, String>)> {
        if self.deferred.is_none() || self.deferred_check.elapsed() < Duration::from_secs(1) {
            return None;
        }
        self.deferred_check = std::time::Instant::now();
        butterpollo_win_io::process::user_signed_in()
            .then(|| self.deferred.take())
            .flatten()
    }
    /// Whether the next [`Self::exited`] wants the PC's process list: a Steam
    /// game is looked for in it once a second. The caller reads the list
    /// without `current_app` held, since reading it takes a creation-time query
    /// per process (8 ms on an idle PC, more on one loading a game) and the
    /// stream and input threads look at the app too.
    pub fn wants_processes(&self) -> bool {
        self.playnite.is_none()
            && self.steam.is_some()
            && self.steam_check.elapsed() >= Duration::from_secs(1)
    }
    /// The processes in `processes` whose program path the Steam game's
    /// tracker has not read yet, for [`ProcessScan::paths`].
    pub fn unlooked(&self, processes: &[butterpollo_session::steam::Process]) -> Vec<u32> {
        self.steam
            .as_ref()
            .map_or_else(Vec::new, |tracker| tracker.unchecked(processes))
    }
    /// A process list asked for by [`Self::wants_processes`] could not be
    /// read: the next try is a second away, as after a look.
    pub fn scan_failed(&mut self) {
        self.steam_check = std::time::Instant::now();
    }
    /// `scan` is read for a [`Self::wants_processes`] that said yes; without
    /// one, a Steam game is looked for next time.
    pub fn exited(&mut self, scan: Option<&ProcessScan>) -> Result<bool> {
        if self.settings_swap.is_some() && self.settings_watch.elapsed() >= SETTINGS_WATCH {
            self.watch_game();
        }
        if let Some(launch) = &self.playnite {
            return Ok(launch.finished());
        }
        if let Some(tracker) = &mut self.steam {
            use butterpollo_session::steam::Tracked;
            let Some(scan) = scan.filter(|_| self.steam_check.elapsed() >= Duration::from_secs(1))
            else {
                return Ok(false);
            };
            self.steam_check = std::time::Instant::now();
            // Every path the tracker asks for was read with the scan.
            let path = |pid| scan.paths.get(&pid).cloned().flatten();
            match tracker.update(&scan.processes, path) {
                Tracked::Waiting => return Ok(false),
                Tracked::Running => {
                    self.game_seen = true;
                    return Ok(false);
                }
                Tracked::Exited => {
                    tracing::info!(app = %self.name, "the game exited");
                    return Ok(true);
                }
                Tracked::Unknown => {
                    if !self.detached {
                        tracing::info!(app = %self.name, "waiting for the game to appear; tracking continues while the stream stays open");
                        self.detached = true;
                    }
                    return Ok(false);
                }
            }
        }
        if self.detached {
            return Ok(false);
        }
        let Some(child) = self.child.as_ref() else {
            return Ok(false);
        };
        let code = child.exit_code()?;
        let waiting = self.wait_all && child.active_processes()? != 0;
        match exit_action(self.auto_detach, self.started.elapsed(), code, waiting) {
            ExitAction::Running => Ok(false),
            ExitAction::Detached(error) => {
                self.detached = true;
                tracing::info!(app=%self.name,"application launcher exited; retaining its streaming session");
                if let Some(code) = error {
                    butterpollo_win_io::tray::launch_error(&self.name, code);
                }
                Ok(false)
            }
            ExitAction::Finished => {
                if code == Some(9009) {
                    butterpollo_win_io::tray::launch_error(&self.name, 9009);
                }
                Ok(true)
            }
        }
    }
    /// Return true when the last game transport leaves an app that closes on pause.
    pub fn connection_state(&mut self, connected: bool) -> bool {
        if self.connected == connected {
            return false;
        }
        self.connected = connected;
        if !connected && self.terminate_on_pause {
            return true;
        }
        if let Some(events) = &self.state_events {
            events.send(connected);
        }
        false
    }
}

impl Drop for RunningApp {
    fn drop(&mut self) {
        self.stop();
    }
}
