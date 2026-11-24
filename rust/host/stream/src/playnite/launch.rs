//! Playnite game and fullscreen launcher worker, pipe monitoring, and window focus.

use super::launch_state::{ExitGrace, GameWatch, LaunchState, Phase, START_TIMEOUT};
use super::plugin::update_plugin;
use crate::state::Shared;
use anyhow::{Context, Result, ensure};
use butterpollo_session::game_settings::Watched;
use butterpollo_session::playnite::{self, Settings};
use butterpollo_win_io::playnite::Pipe;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// A Playnite game or fullscreen menu started for a stream.
pub struct Launch {
    state: Arc<Mutex<LaunchState>>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
    baseline: Vec<butterpollo_session::steam::Process>,
}
impl Launch {
    /// Start a game, or the fullscreen menu without an id, with the
    /// stream's environment. A background stream's game is never brought to
    /// the front. `watch`: the game's settings files were swapped, so its
    /// processes are found for them ([`Self::take_game_processes`]).
    pub fn start(
        h: &Shared,
        id: Option<&str>,
        environment: &BTreeMap<String, String>,
        background: bool,
        watch: bool,
    ) -> Result<Self> {
        let fullscreen = id.is_none();
        let id = id
            .map(uuid::Uuid::parse_str)
            .transpose()
            .context("Playnite launch failed: invalid game ID")?
            .map(|id| id.to_string())
            .unwrap_or_default();
        let program = butterpollo_win_io::playnite::install_dir()
            .and_then(|dir| butterpollo_win_io::playnite::executable(&dir))
            .context("Playnite launch failed: no Desktop or Fullscreen executable found; open Playnite once or repair its installation")?;
        tracing::info!(id, fullscreen, executable = %program.display(), running = butterpollo_win_io::playnite::running().is_some(), "Playnite launch prepared");
        let plugin_ready = update_plugin(h);
        let mut settings = Settings::from_config(&h.config.read().unwrap());
        if background {
            settings.focus_attempts = 0;
        }
        let baseline = butterpollo_win_io::playnite::session_processes().unwrap_or_default();
        let state = Arc::new(Mutex::new(LaunchState {
            fullscreen,
            exit_grace: ExitGrace(settings.exit_wait),
            watch: watch.then(|| GameWatch::new(&baseline, Instant::now())),
            ..Default::default()
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let worker = {
            let (state, stop) = (state.clone(), stop.clone());
            let (id, environment) = (id.to_owned(), environment.clone());
            std::thread::Builder::new()
                .name("playnite-launch".into())
                .spawn(move || {
                    run(
                        &program,
                        &id,
                        &environment,
                        &state,
                        &stop,
                        plugin_ready,
                        &settings,
                    )
                })
                .context("starting the Playnite launch worker")?
        };
        Ok(Self {
            state,
            stop,
            worker: Some(worker),
            baseline,
        })
    }
    /// Whether Playnite reported the game running, or already over: a launcher window is no
    /// longer standing in for it (`launcher_watch`).
    pub fn game_up(&self) -> bool {
        matches!(
            self.state.lock().unwrap().phase,
            Phase::Running | Phase::Exited | Phase::Stopping
        )
    }
    /// Whether Playnite reported the game stopped.
    pub fn finished(&self) -> bool {
        matches!(
            self.state.lock().unwrap().phase,
            Phase::Exited | Phase::Stopping
        )
    }
    /// Game audio uses the connector's confirmed game path and excludes every
    /// process already present when this launch began. Menu/launcher processes
    /// are excluded by the same path policy used to follow game lifetime.
    pub fn audio_processes(
        &self,
        processes: &[butterpollo_session::steam::Process],
    ) -> Result<BTreeMap<u32, u64>> {
        let (folder, exe) = {
            let state = self.state.lock().unwrap();
            ensure!(
                state.phase == Phase::Running && state.has_process_hint(),
                "Playnite has not identified the running game's executable yet"
            );
            (state.install_dir.clone(), state.exe.clone())
        };
        Ok(processes
            .iter()
            .filter(|p| {
                p.started != 0
                    && !self
                        .baseline
                        .iter()
                        .any(|old| old.pid == p.pid && old.started == p.started)
            })
            .filter(|p| {
                butterpollo_win_io::process::image_path(p.pid)
                    .is_some_and(|path| playnite::game_process(&path, &folder, &exe))
            })
            .map(|p| (p.pid, p.started))
            .collect())
    }
    /// The game's processes found since the last call, for its swapped
    /// settings files. The launch's worker looks for them, so that the caller
    /// (the stream's thread, with the app held) reads no process list. None
    /// are found for a game started without the plugin, or one whose folder
    /// Playnite does not know.
    pub fn take_game_processes(&self) -> Vec<Watched> {
        std::mem::take(&mut self.state.lock().unwrap().game_processes)
    }
    /// A launch whose worker found `processes`.
    #[cfg(test)]
    pub fn with_game_processes(processes: Vec<Watched>) -> Self {
        Self {
            state: Arc::new(Mutex::new(LaunchState {
                phase: Phase::Running,
                game_processes: processes,
                ..Default::default()
            })),
            stop: Arc::new(AtomicBool::new(false)),
            worker: None,
            baseline: Vec::new(),
        }
    }
    /// The game's folder, once Playnite has reported it.
    pub fn folder(&self) -> impl Fn() -> Option<String> + Send + 'static {
        let state = self.state.clone();
        move || Some(state.lock().unwrap().install_dir.clone()).filter(|f| !f.is_empty())
    }
    /// End the game: close the windows of the processes started in its
    /// folder since the launch, then end them.
    pub fn stop(mut self, timeout: Duration) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        let state = self.state.lock().unwrap();
        if state.install_dir.trim().is_empty() || state.phase == Phase::Exited {
            return;
        }
        let mut tracker = butterpollo_session::steam::Tracker::new(
            &self.baseline,
            &state.install_dir,
            Duration::ZERO,
        );
        if let Ok(now) = butterpollo_win_io::playnite::session_processes() {
            tracker.update(&now, butterpollo_win_io::process::image_path);
            butterpollo_win_io::process::stop_processes(&tracker.tracked, timeout);
        }
    }
}
fn run(
    program: &Path,
    id: &str,
    environment: &BTreeMap<String, String>,
    state: &Mutex<LaunchState>,
    stop: &AtomicBool,
    plugin_ready: bool,
    settings: &Settings,
) {
    let fullscreen = id.is_empty();
    let set = |phase: Phase| state.lock().unwrap().phase = phase;
    if !plugin_ready {
        fallback(program, id, environment, state);
        return;
    }
    if (fullscreen || butterpollo_win_io::playnite::running().is_none())
        && let Err(error) = butterpollo_win_io::playnite::launch(
            program,
            if fullscreen {
                &["--startfullscreen"]
            } else {
                &[]
            },
            environment,
        )
    {
        tracing::error!(id, error = %format!("{error:#}"), "Playnite startup failed");
        set(Phase::Exited);
        return;
    }
    let opened = Instant::now();
    let deadline = opened + START_TIMEOUT;
    let mut attempts = 0;
    let hello = json!({"type":"hello","role":"launcher","pid":std::process::id(),"mode":if fullscreen { "fullscreen" } else { "standard" },"gameId":id});
    let pipe = loop {
        if stop.load(Ordering::Acquire) {
            return;
        }
        attempts += 1;
        match Pipe::connect(&hello) {
            Ok(pipe) => {
                tracing::debug!(
                    id,
                    attempts,
                    elapsed_ms = opened.elapsed().as_millis(),
                    "Playnite launch pipe connected"
                );
                break pipe;
            }
            Err(error) if Instant::now() >= deadline => {
                tracing::warn!(id, attempts, elapsed_ms = opened.elapsed().as_millis(), error = %format!("{error:#}"), "the Playnite plugin is not available; starting the game without tracking it");
                fallback(program, id, environment, state);
                return;
            }
            Err(error) => {
                if attempts == 1 {
                    tracing::info!(id, error = %format!("{error:#}"), "waiting up to 120 seconds for the Playnite launch pipe");
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    };
    let command = launch_command(id, environment);
    if let Err(error) = pipe.send(&command) {
        tracing::warn!(id, error = %format!("{error:#}"), "the Playnite launch request failed; trying the CLI fallback");
        fallback(program, id, environment, state);
        return;
    }
    if fullscreen {
        tracing::info!(
            "Playnite fullscreen launch requested; stream environment sent to the plugin"
        );
    } else {
        tracing::info!(id, "asked Playnite to start the game");
    }
    let requested = Instant::now();
    let mut checked = requested;
    let mut reconnect_at = requested;
    let mut pipe = Some(pipe);
    let shared = state;
    // What to bring to the front: the game (false) or the fullscreen menu.
    let mut focus: Option<(bool, playnite::Focus)> = None;
    let mut watch = state.lock().unwrap().watch.take();
    // The connection stays open for the stream: Playnite keeps the
    // stream's environment for the game until it closes.
    while !stop.load(Ordering::Acquire) {
        let message = match &pipe {
            Some(pipe) => pipe.lines.recv_timeout(Duration::from_millis(200)),
            None => {
                std::thread::sleep(Duration::from_millis(200));
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            }
        };
        match message {
            Ok(line) => {
                let mut state = state.lock().unwrap();
                state.status(id, playnite::parse(&line));
                if matches!(state.phase, Phase::Exited | Phase::Stopping) {
                    return;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                pipe = None;
                state.lock().unwrap().disconnected();
                tracing::warn!(
                    id,
                    "the Playnite plugin closed the connection; game exit is unconfirmed"
                );
            }
        }
        let mut state = state.lock().unwrap();
        if state.start_timeout(requested.elapsed()) {
            tracing::warn!(
                id,
                fullscreen,
                "Playnite did not confirm startup within 120 seconds; leaving the stream open for a slow launch or a Playnite dialog"
            );
        }
        let mut reconnect = false;
        let mut relaunch = false;
        if (state.verifying() || fullscreen) && checked.elapsed() >= Duration::from_secs(1) {
            checked = Instant::now();
            let previous = state.phase.clone();
            match butterpollo_win_io::playnite::session_processes() {
                Ok(processes) => {
                    let alive = (state.verifying() && state.has_process_hint()).then(|| {
                        processes.iter().any(|p| {
                            butterpollo_win_io::process::image_path(p.pid).is_some_and(|path| {
                                playnite::game_process(&path, &state.install_dir, &state.exe)
                            })
                        })
                    });
                    state.poll(checked, alive);
                    let fullscreen_running = processes
                        .iter()
                        .any(|p| p.name.eq_ignore_ascii_case("Playnite.FullscreenApp.exe"));
                    state.poll_fullscreen(fullscreen_running);
                    relaunch = state.relaunch_menu(fullscreen_running);
                    reconnect = fullscreen
                        && fullscreen_running
                        && pipe.is_none()
                        && checked >= reconnect_at;
                }
                Err(error) => {
                    tracing::warn!(id, error = %format!("{error:#}"), "Playnite game process check failed; keeping the stream open");
                    state.poll(checked, None);
                }
            }
            match state.phase {
                Phase::Exited => {
                    tracing::info!(id, "Playnite game exit confirmed; ending the stream");
                    return;
                }
                Phase::Untracked if previous != Phase::Untracked => tracing::warn!(
                    id,
                    "Playnite game could not be tracked; the stream stays until ended manually"
                ),
                _ => {}
            }
        }
        if !fullscreen && pipe.is_none() && state.phase == Phase::Untracked {
            return;
        }
        // A game that started wins over the menu it started from.
        let game = std::mem::take(&mut state.focus_game);
        if std::mem::take(&mut state.focus_menu) || game {
            focus = playnite::Focus::arm(settings, Instant::now()).map(|f| (!game, f));
        }
        let (install_dir, exe) = (state.install_dir.clone(), state.exe.clone());
        let running = state.phase == Phase::Running && state.has_process_hint();
        drop(state);
        // The game's settings files wait for its processes. They are looked
        // for without the state held: the stream's thread reads it.
        if running
            && let Some(watch) = &mut watch
            && watch.due(Instant::now())
        {
            match butterpollo_win_io::playnite::session_processes() {
                Ok(processes) => {
                    let found = watch.scan(
                        Instant::now(),
                        (&install_dir, &exe),
                        &processes,
                        butterpollo_win_io::process::image_path,
                    );
                    if !found.is_empty() {
                        tracing::info!(id, processes = ?found, "Playnite game processes recorded for its settings files");
                        shared.lock().unwrap().game_processes.extend(found);
                    }
                }
                Err(error) => {
                    tracing::debug!(id, error = %format!("{error:#}"), "the Playnite game's processes could not be looked for");
                    watch.failed(Instant::now());
                }
            }
        }
        if relaunch
            && let Err(error) =
                butterpollo_win_io::playnite::launch(program, &["--startfullscreen"], environment)
        {
            tracing::error!(error = %format!("{error:#}"), "Playnite fullscreen mode could not start again; ending the stream");
            shared.lock().unwrap().phase = Phase::Exited;
            return;
        }
        if let Some((menu, budget)) = &mut focus
            && budget.due(Instant::now())
        {
            let focused = bring_forward(*menu, &install_dir, &exe);
            budget.checked(Instant::now(), focused);
            if budget.finished(Instant::now()) {
                tracing::debug!(id, menu = *menu, focused, "Playnite window focus finished");
                focus = None;
            }
        }
        if reconnect {
            reconnect_at = Instant::now() + Duration::from_secs(5);
            if let Ok(connected) = Pipe::connect(&hello)
                && connected.send(&command).is_ok()
            {
                // Only restore the environment; replaying a launch could start a game twice.
                shared.lock().unwrap().pipe_closed = false;
                pipe = Some(connected);
                tracing::info!("Playnite fullscreen pipe reconnected; stream environment restored");
            }
        }
    }
}
/// Bring the game's window, or Playnite's fullscreen menu, to the front;
/// true when it is in front.
fn bring_forward(menu: bool, install_dir: &str, exe: &str) -> bool {
    let Ok(processes) = butterpollo_win_io::playnite::session_processes() else {
        return false;
    };
    let candidates: Vec<u32> = processes
        .iter()
        .filter(|p| {
            if menu {
                p.name.eq_ignore_ascii_case("Playnite.FullscreenApp.exe")
            } else {
                butterpollo_win_io::process::image_path(p.pid)
                    .is_some_and(|path| playnite::game_process(&path, install_dir, exe))
            }
        })
        .map(|p| p.pid)
        .collect();
    if butterpollo_win_graphics::foreground::process().is_some_and(|p| candidates.contains(&p)) {
        return true;
    }
    candidates
        .into_iter()
        .any(butterpollo_win_io::lossless::focus)
}
pub(super) fn launch_command(id: &str, environment: &BTreeMap<String, String>) -> Value {
    if id.is_empty() {
        json!({"type":"command","command":"set-environment","env":environment})
    } else {
        json!({"type":"command","command":"launch","id":id,"env":environment})
    }
}
fn fallback(
    program: &Path,
    id: &str,
    environment: &BTreeMap<String, String>,
    state: &Mutex<LaunchState>,
) {
    let game_args = ["--start", id];
    match butterpollo_win_io::playnite::launch(
        program,
        if id.is_empty() {
            &["--startfullscreen"]
        } else {
            &game_args
        },
        environment,
    ) {
        Ok(()) => {
            tracing::warn!(
                id,
                "Playnite CLI fallback requested; start is unconfirmed and the stream stays until ended manually"
            );
            state.lock().unwrap().phase = Phase::Untracked;
        }
        Err(error) => {
            tracing::error!(id, error = %format!("{error:#}"), "Playnite fallback launch failed");
            state.lock().unwrap().phase = Phase::Exited;
        }
    }
}
/// Close Playnite's fullscreen mode after its stream.
pub fn close_fullscreen(timeout: Duration) {
    let Ok(processes) = butterpollo_win_io::playnite::session_processes() else {
        return;
    };
    let fullscreen: BTreeMap<u32, u64> = processes
        .iter()
        .filter(|p| p.name.eq_ignore_ascii_case("Playnite.FullscreenApp.exe"))
        .map(|p| (p.pid, p.started))
        .collect();
    butterpollo_win_io::process::stop_processes(&fullscreen, timeout);
}
