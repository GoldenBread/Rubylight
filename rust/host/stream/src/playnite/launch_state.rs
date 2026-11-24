//! State machine and game-process watch for a Playnite game or fullscreen launch.

use butterpollo_session::game_settings::Watched;
use butterpollo_session::playnite::{self, Message};
use std::{
    collections::BTreeSet,
    path::Path,
    time::{Duration, Instant},
};

/// How a game started through Playnite is going.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) enum Phase {
    #[default]
    Starting,
    Running,
    Exited,
    Stopping,
    /// Started without the plugin: the stream stays until it is ended.
    Untracked,
}
pub(super) const START_TIMEOUT: Duration = Duration::from_secs(120);
/// How long a game must stay closed before its stream ends, unless
/// `playnite_exit_wait_secs` says otherwise.
pub(super) const EXIT_GRACE: Duration = playnite::EXIT_WAIT;
/// The wait of one launch (`playnite_exit_wait_secs`).
pub(super) struct ExitGrace(pub(super) Duration);
impl Default for ExitGrace {
    fn default() -> Self {
        Self(EXIT_GRACE)
    }
}
/// How often a running game's processes are looked for, for its swapped
/// settings files: every second while none was found in the game's first
/// minute, so a host crash soon after the launch still waits for the game,
/// and every five seconds otherwise (a launcher hands over to the game, a
/// game starts helpers).
pub(super) const WATCH_SOON: Duration = Duration::from_secs(1);
pub(super) const WATCH_LATER: Duration = Duration::from_secs(5);
pub(super) const WATCH_START: Duration = Duration::from_secs(60);
/// Finds the processes of the game Playnite started, for its swapped settings
/// files (`RunningApp::watch_game`, issue #82): those started since the launch
/// whose program is in the game's folder, the ones [`Launch::stop`] ends. A
/// process's program is read once.
pub(super) struct GameWatch {
    /// The processes already running when the launch began (id, creation time).
    baseline: BTreeSet<(u32, u64)>,
    /// The processes already looked at for `game`.
    seen: BTreeSet<(u32, u64)>,
    /// The game's folder and program, as Playnite reported them.
    game: (String, String),
    /// When `game` was first looked for, and whether a process of it was found.
    since: Instant,
    found: bool,
    next: Instant,
}
impl GameWatch {
    pub(super) fn new(baseline: &[butterpollo_session::steam::Process], now: Instant) -> Self {
        Self {
            baseline: baseline.iter().map(|p| (p.pid, p.started)).collect(),
            seen: BTreeSet::new(),
            game: Default::default(),
            since: now,
            found: false,
            next: now,
        }
    }
    pub(super) fn due(&self, now: Instant) -> bool {
        now >= self.next
    }
    /// The process list could not be read: look again later.
    pub(super) fn failed(&mut self, now: Instant) {
        self.next = now + WATCH_LATER;
    }
    /// The processes of the game in `folder`, or of the program `exe`, among
    /// `processes` that no earlier call returned. `program` reads a process's
    /// program; one it cannot read yet is looked at again.
    pub(super) fn scan(
        &mut self,
        now: Instant,
        (folder, exe): (&str, &str),
        processes: &[butterpollo_session::steam::Process],
        program: impl Fn(u32) -> Option<String>,
    ) -> Vec<Watched> {
        if self.game.0 != folder || self.game.1 != exe {
            // Another game from the fullscreen menu, or Playnite named the program later.
            self.game = (folder.to_owned(), exe.to_owned());
            self.seen.clear();
            self.since = now;
            self.found = false;
        }
        let running: BTreeSet<(u32, u64)> = processes.iter().map(|p| (p.pid, p.started)).collect();
        self.seen.retain(|process| running.contains(process));
        let mut found = Vec::new();
        for process in processes {
            let key = (process.pid, process.started);
            if process.started == 0 || self.baseline.contains(&key) || self.seen.contains(&key) {
                continue;
            }
            let Some(program) = program(process.pid) else {
                continue;
            };
            self.seen.insert(key);
            if playnite::game_process(&program, folder, exe) {
                found.push(Watched {
                    pid: process.pid,
                    started: process.started,
                    program,
                });
            }
        }
        self.found |= !found.is_empty();
        let soon = !self.found && now.duration_since(self.since) < WATCH_START;
        self.next = now + if soon { WATCH_SOON } else { WATCH_LATER };
        found
    }
}
#[derive(Default)]
pub(super) struct LaunchState {
    pub(super) phase: Phase,
    pub(super) install_dir: String,
    pub(super) exe: String,
    pub(super) stopped: bool,
    pub(super) pipe_closed: bool,
    pub(super) saw_process: bool,
    pub(super) missing_since: Option<Instant>,
    pub(super) fullscreen: bool,
    pub(super) fullscreen_seen: bool,
    pub(super) game_id: String,
    /// A game started, or the fullscreen menu appeared or came back: bring
    /// it to the front (`playnite_focus_*`).
    pub(super) focus_game: bool,
    pub(super) focus_menu: bool,
    /// The phase is Exited because the game's processes ended.
    pub(super) game_ended: bool,
    /// Playnite said it is closing with no game of its own running
    /// (`playniteExiting`): the user quit the fullscreen menu, so neither a
    /// menu still shutting down nor the game's end brings it back.
    pub(super) playnite_exiting: bool,
    pub(super) exit_grace: ExitGrace,
    /// Looks for the game's processes when its settings files were swapped;
    /// the worker takes it, to read the process list without this state held.
    pub(super) watch: Option<GameWatch>,
    /// The game's processes found since [`Launch::take_game_processes`].
    pub(super) game_processes: Vec<Watched>,
}
impl LaunchState {
    pub(super) fn status(&mut self, id: &str, message: Message) {
        let Message::Status {
            name,
            id: game,
            install_dir,
            exe,
        } = message
        else {
            return;
        };
        let game = game.trim().trim_matches(['{', '}']);
        let ours = !game.is_empty()
            && game.eq_ignore_ascii_case(if self.fullscreen {
                &self.game_id
            } else {
                id.trim().trim_matches(['{', '}'])
            });
        match name.as_str() {
            "gameStarted" if ours || (self.fullscreen && !game.is_empty()) => {
                if !game.eq_ignore_ascii_case(&self.game_id) {
                    self.saw_process = false;
                    self.install_dir.clear();
                    self.exe.clear();
                }
                self.game_id = game.to_owned();
                self.phase = Phase::Running;
                if !install_dir.is_empty() {
                    self.install_dir = install_dir;
                }
                if !exe.is_empty() {
                    self.exe = exe;
                }
                self.stopped = false;
                self.missing_since = None;
                self.focus_game = true;
                // A Playnite started again (its pipe reconnected) runs a game: its exit counts anew.
                self.playnite_exiting = false;
                tracing::info!(id, folder = %self.install_dir, exe = %self.exe, "Playnite started the game");
            }
            "gameStopped" if ours => {
                if self.phase == Phase::Running {
                    self.stopped = true;
                    tracing::info!(
                        id,
                        wait_secs = self.exit_grace.0.as_secs(),
                        "Playnite reports the game stopped; verifying game processes"
                    );
                } else {
                    tracing::warn!(
                        id,
                        "Playnite reported gameStopped before gameStarted; still waiting for startup"
                    );
                }
            }
            "stopRequested" if ours || game.is_empty() => {
                self.phase = Phase::Stopping;
                tracing::info!(id, "Playnite requested game cleanup and stream shutdown");
            }
            "playniteExiting" => {
                if self.fullscreen && self.phase != Phase::Running {
                    self.phase = Phase::Exited;
                    tracing::info!(
                        "Playnite fullscreen is closing with no active game; ending the stream"
                    );
                    return;
                }
                if self.fullscreen {
                    // The plugin sends this only when Playnite runs no game,
                    // and it never reports the stop of a game started from the
                    // fullscreen menu (only of one the host asked for). So the
                    // game is over as far as Playnite knows: its processes
                    // still decide, but the menu is not started again (#127).
                    self.stopped = true;
                    self.playnite_exiting = true;
                }
                tracing::info!(id, "Playnite is closing; checking the game independently");
                self.disconnected();
            }
            _ => {}
        }
    }
    pub(super) fn disconnected(&mut self) {
        self.pipe_closed = true;
        if self.phase == Phase::Starting {
            self.phase = Phase::Untracked;
        }
    }
    pub(super) fn start_timeout(&mut self, elapsed: Duration) -> bool {
        if self.phase == Phase::Starting
            && !(self.fullscreen && self.fullscreen_seen)
            && elapsed >= START_TIMEOUT
        {
            self.phase = Phase::Untracked;
            true
        } else {
            false
        }
    }
    pub(super) fn verifying(&self) -> bool {
        self.phase == Phase::Running && (self.stopped || self.pipe_closed)
    }
    pub(super) fn has_process_hint(&self) -> bool {
        !self.install_dir.trim().is_empty() || Path::new(&self.exe).is_absolute()
    }
    pub(super) fn poll(&mut self, at: Instant, alive: Option<bool>) {
        if !self.verifying() {
            return;
        }
        if alive == Some(true) {
            self.saw_process = true;
            self.missing_since = None;
            return;
        }
        if self.has_process_hint() && alive.is_none() {
            self.missing_since = None;
            return;
        }
        if !self.has_process_hint() && !self.stopped {
            self.phase = Phase::Untracked;
            return;
        }
        let missing = at.duration_since(*self.missing_since.get_or_insert(at));
        if !self.stopped && !self.saw_process {
            // A lost connector is not proof that a slow game ever started.
            if missing >= START_TIMEOUT {
                self.phase = Phase::Untracked;
            }
        } else if missing >= self.exit_grace.0 {
            self.phase = Phase::Exited;
            self.game_ended = true;
        }
    }
    pub(super) fn poll_fullscreen(&mut self, running: bool) {
        if !self.fullscreen || !running {
            return;
        }
        if !self.fullscreen_seen {
            self.focus_menu = true;
        }
        self.fullscreen_seen = true;
        // A menu still shutting down after `playniteExiting` is not kept.
        if self.phase == Phase::Exited && !self.playnite_exiting {
            tracing::info!(id = %self.game_id, "Playnite game ended; keeping the fullscreen menu open");
            self.back_to_menu();
            self.focus_menu = true;
        }
    }
    /// In fullscreen mode, a game that ended while Playnite was closed (set
    /// to quit when a game starts, or closed during the game) goes back to
    /// the menu, as in Vibepollo: true when fullscreen mode has to start
    /// again. Closing Playnite with no game running still ends the stream.
    pub(super) fn relaunch_menu(&mut self, running: bool) -> bool {
        if !self.fullscreen
            || running
            || self.playnite_exiting
            || self.phase != Phase::Exited
            || !self.game_ended
        {
            return false;
        }
        tracing::info!(id = %self.game_id, "Playnite game ended with Playnite closed; starting fullscreen mode again");
        self.back_to_menu();
        // Focus once the menu appears again.
        self.fullscreen_seen = false;
        true
    }
    fn back_to_menu(&mut self) {
        self.phase = Phase::Starting;
        self.game_id.clear();
        self.install_dir.clear();
        self.exe.clear();
        self.stopped = false;
        self.saw_process = false;
        self.missing_since = None;
        self.game_ended = false;
    }
}
