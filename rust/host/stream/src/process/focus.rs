//! Foreground game detection, launch window bring-back focus, and process classification.

use super::{RunningApp, session_processes};
use anyhow::Result;
use butterpollo_session::state::App;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    time::{Duration, Instant},
};

impl RunningApp {
    /// The game's window processes that may be brought to the front after a
    /// launch (issue #131): never a background game's, never a Playnite launch's,
    /// never a program the desktop's explorer started (the user's own, not the
    /// app's), and none for an app with no game process to name.
    #[cfg(test)]
    pub fn focus_windows(&self) -> Result<BTreeSet<u32>> {
        if self.background || self.playnite.is_some() || !self.has_game_process() {
            return Ok(Default::default());
        }
        self.make_focus_snapshot().candidate_pids()
    }
    /// A lock-free snapshot of the game's tracked processes for [`FocusSnapshot::candidate_pids`],
    /// returning `None` when launch focus is not active for this app at `now`.
    pub fn focus_snapshot(&self, now: Instant) -> Option<FocusSnapshot> {
        self.bring_back_open(now)
            .then(|| self.make_focus_snapshot())
    }
    fn make_focus_snapshot(&self) -> FocusSnapshot {
        FocusSnapshot {
            generation: self.generation.clone(),
            steam_tracked: self.steam.as_ref().map(|t| t.tracked.clone()),
            child_pids: self
                .child
                .as_ref()
                .and_then(|c| c.process_ids().ok())
                .unwrap_or_default(),
            foreign: self.foreign.clone(),
        }
    }
    /// A fullscreen window on the stream's display belongs to process `pid`.
    /// A store client (Xbox, Epic, EA, Ubisoft, Battle.net) starts its games
    /// itself, outside the processes this app owns, so quitting never closed
    /// them. Such a game is remembered when the app launched a program, no
    /// Steam or Playnite tracking covers it, and its process started after the
    /// launch; quitting the plain desktop never closes what a user opened.
    ///
    /// Whether a program is such a game takes the PC's process list: that is
    /// left to the returned check, which runs off the stream's thread and
    /// without `current_app` held, one at a time.
    pub fn observe_foreground(&mut self, pid: u32, program: &str) -> Option<ForegroundCheck> {
        // The job's process list is read at most once, and not at all once the game was seen
        // and nothing else needs it: this runs on the stream's thread every second.
        let mut owned = None;
        let mut owns = |app: &Self| {
            *owned.get_or_insert_with(|| {
                app.child
                    .as_ref()
                    .and_then(|child| child.process_ids().ok())
                    .is_some_and(|ids| ids.contains(&pid))
            })
        };
        let is_launcher = butterpollo_session::launcher_prompt::launcher_of(program).is_some();
        if !self.game_seen && !is_launcher && owns(self) {
            self.game_seen = true;
        }
        if !self.launches
            || self.steam.is_some()
            || self.playnite.is_some()
            || self.foreign.contains_key(&pid)
            || self.foreign.len() >= 8
            || pid == std::process::id()
            || self.foreground_check.is_some()
            || self
                .not_foreign
                .get(&pid)
                .is_some_and(|seen| seen == program)
            || owns(self)
        {
            return None;
        }
        self.foreground_check = Some(pid);
        Some(ForegroundCheck {
            pid,
            program: program.to_owned(),
            launched_at: self.launched_at,
            generation: self.generation.clone(),
        })
    }
    /// Apply what `check` found, if this is still the app it was made for.
    pub fn finish_foreground(&mut self, check: ForegroundCheck, verdict: ForegroundVerdict) {
        if check.generation != self.generation {
            return;
        }
        self.foreground_check = None;
        let ForegroundCheck { pid, program, .. } = check;
        let created = match verdict {
            ForegroundVerdict::Foreign(created) => created,
            ForegroundVerdict::NotForeign => {
                self.not_foreign.insert(pid, program);
                return;
            }
            ForegroundVerdict::Retry => return,
        };
        if self.foreign.len() >= 8 || self.foreign.contains_key(&pid) {
            return;
        }
        tracing::info!(app = %self.name, program, pid, "game started by another program; quitting the app closes it");
        self.foreign.insert(pid, created);
        self.game_seen = true;
    }
    /// A check that could not be started: the program is looked at again.
    pub fn abandon_foreground(&mut self) {
        self.foreground_check = None;
    }
    /// Whether the game itself is up, and how long ago the app was launched, for an app that
    /// starts a program. A launcher window is a prompt only while the game is not up.
    ///
    /// None for an app that only opens a launcher, and once the watch ran out
    /// (`launcher_prompt::WATCH_LIMIT`): the desktop is then no longer read for it.
    pub fn launch_progress(&self) -> Option<(bool, Duration)> {
        let age = self.started.elapsed();
        if !self.launches
            || self.launcher_only
            || age > butterpollo_session::launcher_prompt::WATCH_LIMIT
        {
            return None;
        }
        // Playnite starts the game itself and says when it runs.
        let game_up = self.game_seen
            || self
                .playnite
                .as_ref()
                .is_some_and(|launch| launch.game_up());
        Some((game_up, age))
    }
    /// Whether bringing a launched game's window back may still happen at `now`
    /// (issue #131): worth a look at the PC's process list and the desktop. Before
    /// the game's window is seen that lasts as long as a launcher prompt is
    /// watched for ([`butterpollo_session::launcher_prompt::WATCH_LIMIT`]); after, a
    /// minute from its first sight. Playnite already manages its own launch focus.
    pub fn bring_back_open(&self, now: Instant) -> bool {
        if self.background || !self.launches || self.playnite.is_some() || !self.has_game_process()
        {
            return false;
        }
        match &self.focus {
            Some(focus) => {
                now.saturating_duration_since(focus.seen) < BRING_BACK_WINDOW
                    && focus.restores < BRING_BACK_LIMIT
            }
            None => self.launch_progress().is_some(),
        }
    }
    /// As [`Self::bring_back`], applied only if this app still has `generation`,
    /// returning the pid to bring back and the app's name.
    pub fn bring_back_for(
        &mut self,
        generation: &str,
        now: Instant,
        windows: Vec<(u32, bool)>,
        foreground: Option<u32>,
        store_client: impl Fn(u32) -> bool,
    ) -> Option<(u32, String)> {
        if self.generation != generation {
            return None;
        }
        self.bring_back(now, windows, foreground, store_client)
            .map(|pid| (pid, self.name.clone()))
    }
    /// Whether to bring a launched game's window back at `now`, `found` being
    /// what a look at the desktop saw of it (issue #131). An exclusive-fullscreen
    /// game that switches the streamed display's mode can be left minimised, or
    /// behind the Steam window that started with it; the user then clicks it in
    /// the taskbar. For a minute after the window was first seen the host does
    /// that itself: at most [`BRING_BACK_LIMIT`] times, [`BRING_BACK_SPACING`]
    /// apart, and never over a program the user is using. A background game's
    /// window and one of an app that starts no program are never brought back.
    ///
    /// `windows` are the game's windows on the desktop (`pid`, minimised),
    /// `foreground` the process of the window in front, and `store_client`
    /// whether a process's program is a store client's. The pid whose window to
    /// bring back, if any.
    pub fn bring_back(
        &mut self,
        now: Instant,
        windows: Vec<(u32, bool)>,
        foreground: Option<u32>,
        store_client: impl Fn(u32) -> bool,
    ) -> Option<u32> {
        if !self.bring_back_open(now) {
            return None;
        }
        let found = found_window(windows.iter().copied(), foreground, store_client);
        // Only a window that showed up counts: the minute starts at its first sight.
        if self.focus.is_none() && found == LaunchWindow::Missing {
            return None;
        }
        let focus = self.focus.get_or_insert(Focus {
            seen: now,
            restores: 0,
            next: now,
        });
        if now < focus.next
            || !matches!(found, LaunchWindow::Minimized | LaunchWindow::BehindClient)
        {
            return None;
        }
        focus.restores += 1;
        focus.next = now + BRING_BACK_SPACING;
        // The window that is down is the one to bring back, else the game's main one.
        windows
            .iter()
            .find(|(_, minimized)| *minimized)
            .or_else(|| windows.first())
            .map(|(pid, _)| *pid)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ExitAction {
    Running,
    Detached(Option<u32>),
    Finished,
}
pub(super) fn exit_action(
    auto_detach: bool,
    elapsed: Duration,
    code: Option<u32>,
    waiting: bool,
) -> ExitAction {
    let Some(code) = code.filter(|_| !waiting) else {
        return ExitAction::Running;
    };
    if auto_detach && code != 9009 && elapsed < Duration::from_secs(5) {
        ExitAction::Detached((code != 0).then_some(code))
    } else {
        ExitAction::Finished
    }
}
/// The PC's processes read for a Steam game's tracker, with the program paths
/// of the ones it has not looked at ([`RunningApp::unlooked`]), all read
/// without `current_app` held.
pub struct ProcessScan {
    pub processes: Vec<butterpollo_session::steam::Process>,
    pub paths: HashMap<u32, Option<String>>,
}
/// Whether a fullscreen program another program started is the app's game,
/// from [`RunningApp::observe_foreground`]: run off the stream's thread
/// without `current_app` held, then given to [`RunningApp::finish_foreground`].
pub struct ForegroundCheck {
    pub pid: u32,
    program: String,
    launched_at: u64,
    generation: String,
}
#[derive(Debug, PartialEq, Eq)]
pub enum ForegroundVerdict {
    /// The process (pid → creation time) is the app's game.
    Foreign(u64),
    /// It is not: never closed, not looked at again.
    NotForeign,
    /// The program is gone, or its id now names another process: nothing is
    /// recorded, and the next look sees whatever holds the id then.
    Retry,
}
impl ForegroundCheck {
    pub fn run(&self) -> ForegroundVerdict {
        self.run_with(
            butterpollo_win_io::process::creation_time,
            butterpollo_win_io::process::processes,
        )
    }
    /// The process is looked at only as the program the window showed: by the
    /// time this runs, its id can name an unrelated process, which quitting
    /// must never close.
    pub(super) fn run_with(
        &self,
        creation_time: impl FnOnce(u32) -> u64,
        processes: impl FnOnce() -> Result<Vec<butterpollo_session::steam::Process>>,
    ) -> ForegroundVerdict {
        let created = creation_time(self.pid);
        if created == 0 {
            return ForegroundVerdict::Retry;
        }
        let Ok(processes) = processes() else {
            return ForegroundVerdict::Retry;
        };
        let name = self
            .program
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(&self.program);
        if !processes.iter().any(|process| {
            process.pid == self.pid
                && process.started == created
                && process.name.eq_ignore_ascii_case(name)
        }) {
            return ForegroundVerdict::Retry;
        }
        // A program the user opened on the streamed display (its parent is
        // explorer.exe) is theirs, not the app's.
        if foreign_game(&self.program, created, self.launched_at)
            && !started_from_desktop(self.pid, &processes)
        {
            ForegroundVerdict::Foreign(created)
        } else {
            ForegroundVerdict::NotForeign
        }
    }
}
/// How long after a launched game's window was first seen it may be brought
/// back (issue #131): the display mode switch that leaves it minimised happens
/// in its first seconds.
const BRING_BACK_WINDOW: Duration = Duration::from_secs(60);
/// The most brings-back, and the least time between two: the user's one click
/// in the taskbar, not a fight with the game.
const BRING_BACK_LIMIT: u8 = 3;
const BRING_BACK_SPACING: Duration = Duration::from_secs(3);
/// What a look at the desktop found of a launched game's window (issue #131).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LaunchWindow {
    /// No window of the game is on the desktop.
    Missing,
    /// The game's window is in front.
    Front,
    /// The game's window is minimised, as an exclusive-fullscreen mode switch
    /// can leave it.
    Minimized,
    /// A store client's window (Steam's) is in front of the game's.
    BehindClient,
    /// Another program is in front: the user's, never taken from them.
    BehindOther,
}
/// Read what a look saw: the game's own windows (`pid`, minimised) and the
/// process of the window in front, with whether a process is a store client's.
fn found_window(
    windows: impl IntoIterator<Item = (u32, bool)>,
    foreground: Option<u32>,
    store_client: impl Fn(u32) -> bool,
) -> LaunchWindow {
    let (mut present, mut minimized, mut front) = (false, false, false);
    for (pid, down) in windows {
        present = true;
        minimized |= down;
        front |= foreground == Some(pid);
    }
    if !present {
        LaunchWindow::Missing
    } else if front {
        LaunchWindow::Front
    } else if foreground.is_some_and(|pid| !store_client(pid)) {
        LaunchWindow::BehindOther
    } else if minimized {
        LaunchWindow::Minimized
    } else if foreground.is_some_and(store_client) {
        LaunchWindow::BehindClient
    } else {
        LaunchWindow::BehindOther
    }
}
/// The game's window first seen, and the brings-back since (issue #131).
#[derive(Debug, PartialEq)]
pub(super) struct Focus {
    seen: Instant,
    restores: u8,
    next: Instant,
}
/// Lock-free inputs for finding the game's candidate window processes off `current_app`.
pub struct FocusSnapshot {
    pub(crate) generation: String,
    steam_tracked: Option<BTreeMap<u32, u64>>,
    child_pids: Vec<u32>,
    foreign: BTreeMap<u32, u64>,
}
impl FocusSnapshot {
    /// Resolve the game's candidate window pids against the current session process list.
    pub fn candidate_pids(&self) -> Result<BTreeSet<u32>> {
        let processes = session_processes()?;
        Ok(self.candidates_in(&processes))
    }
    pub(crate) fn candidates_in(
        &self,
        processes: &[butterpollo_session::steam::Process],
    ) -> BTreeSet<u32> {
        let owned = if let Some(tracked) = &self.steam_tracked {
            tracked.clone()
        } else {
            let mut map: BTreeMap<u32, u64> = processes
                .iter()
                .filter(|p| self.child_pids.contains(&p.pid))
                .map(|p| (p.pid, p.started))
                .collect();
            map.extend(self.foreign.iter().map(|(&pid, &started)| (pid, started)));
            map
        };
        focus_candidates(&owned, processes)
    }
}
/// The `owned` game processes' windows, minus what the desktop's explorer
/// started (the user's own program) and minus store clients, launchers, or shells
/// (`FocusSnapshot::candidate_pids`).
pub(super) fn focus_candidates(
    owned: &BTreeMap<u32, u64>,
    processes: &[butterpollo_session::steam::Process],
) -> BTreeSet<u32> {
    butterpollo_session::background::window_owners(owned, processes)
        .into_iter()
        .filter(|pid| {
            !started_from_desktop(*pid, processes)
                && processes
                    .iter()
                    .find(|p| p.pid == *pid)
                    .is_none_or(|p| watches_program(&p.name))
        })
        .collect()
}
/// Whether a fullscreen `program`, created at `created`, is a game a store
/// client started for an app launched at `launched_at`. The store clients
/// themselves never are: quitting a Big Picture app that started Steam must
/// not close Steam.
/// Whether explorer.exe, the desktop, started `pid`: a program the user
/// opened themselves. A parent created after the child is an unrelated
/// process that reused the parent's id.
/// The `owned` game processes (pid and creation time) and every process
/// each started, once each.
pub(super) fn game_trees(
    owned: &BTreeMap<u32, u64>,
    processes: &[butterpollo_session::steam::Process],
) -> Vec<u32> {
    let mut pids: Vec<u32> = owned
        .iter()
        .flat_map(|(&pid, &started)| {
            butterpollo_session::audio_scope::tree(
                butterpollo_session::audio_scope::Identity { pid, started },
                processes,
            )
        })
        .map(|identity| identity.pid)
        .collect();
    pids.sort_unstable();
    pids.dedup();
    pids
}

pub(super) fn started_from_desktop(
    pid: u32,
    processes: &[butterpollo_session::steam::Process],
) -> bool {
    let Some(child) = processes.iter().find(|p| p.pid == pid) else {
        return false;
    };
    processes.iter().any(|p| {
        p.pid == child.parent
            && p.started <= child.started
            && p.name.eq_ignore_ascii_case("explorer.exe")
    })
}
/// Whether everything the app starts only opens a launcher's own window.
pub(super) fn only_opens_launchers(app: &App) -> bool {
    let detached = app
        .extra
        .get("detached")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str);
    let commands: Vec<&str> = std::iter::once(app.cmd.as_str())
        .chain(detached)
        .filter(|command| !command.trim().is_empty())
        .collect();
    !commands.is_empty()
        && commands
            .iter()
            .all(|command| butterpollo_session::launcher_prompt::opens_launcher_itself(command))
}
/// Whether `app` starts its game where the host may not see it (detached commands, a
/// launcher only), so its swapped settings files cannot wait for the game to close.
pub(super) fn running_settings_unwatched(app: &App) -> bool {
    only_opens_launchers(app)
        || app
            .extra
            .get("detached")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|commands| {
                commands
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .any(|command| !command.trim().is_empty())
            })
}
/// How often a swapped game's processes are recorded while it runs.
pub(super) const SETTINGS_WATCH: Duration = Duration::from_secs(5);
/// Whether a game's settings files wait for `program`: not for a store client, a launcher or
/// a shell, which stay open after the game.
pub(super) fn watches_program(program: &str) -> bool {
    const SHELLS: [&str; 6] = [
        "cmd.exe",
        "conhost.exe",
        "powershell.exe",
        "pwsh.exe",
        "explorer.exe",
        "rundll32.exe",
    ];
    let name = program.rsplit(['\\', '/']).next().unwrap_or(program);
    !store_client(name)
        && !SHELLS.iter().any(|shell| name.eq_ignore_ascii_case(shell))
        && butterpollo_session::launcher_prompt::launcher_of(program).is_none()
}
pub(super) fn foreign_game(program: &str, created: u64, launched_at: u64) -> bool {
    let name = program.rsplit(['/', '\\']).next().unwrap_or(program);
    created != 0 && created >= launched_at && !store_client(name)
}
/// Whether the program named `name` (its file name) is a store client, Playnite or the host.
pub(crate) fn store_client(name: &str) -> bool {
    const CLIENTS: [&str; 14] = [
        "steam.exe",
        "steamwebhelper.exe",
        "epicgameslauncher.exe",
        "eadesktop.exe",
        "eabackgroundservice.exe",
        "upc.exe",
        "ubisoftconnect.exe",
        "battle.net.exe",
        "xboxpcapp.exe",
        "gamingservices.exe",
        "galaxyclient.exe",
        "playnite.fullscreenapp.exe",
        "playnite.desktopapp.exe",
        "butterpollo.exe",
    ];
    CLIENTS
        .iter()
        .any(|client| name.eq_ignore_ascii_case(client))
}
