//! Session polling thread, desktop loop, and profile launch interactions for a
//! running Nucleus Co-op session.
use super::super::files;
use super::super::instances::{Desktop, Keeper, MessageBox};
use super::super::layers::Guard;
use super::super::pads::Pads;
use super::diagnostics::{Crashlog, said};
use super::{Instances, Nucleus, PERIOD, PLAY, RECLICK, REFUSAL_GRACE, START_TIMEOUT};
use butterpollo_session::audio_scope::Identity;
use butterpollo_session::nucleus::coop::{self, End, Phase};
use butterpollo_session::nucleus::profile::TITLE;
use std::collections::BTreeSet;
use std::os::windows::io::OwnedHandle;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

/// What the session thread and the host share.
pub(super) struct Shared {
    pub(super) session: Mutex<coop::Session>,
    /// Each slot's stream size: the first player's until the slot's own.
    pub(super) sizes: Mutex<Vec<(u32, u32)>>,
    /// Each slot's instance root, for its audio.
    pub(super) roots: Mutex<Vec<Option<Identity>>>,
}

/// The session thread's state: one [`step`](Runner::step) per period.
pub(super) struct Runner {
    pub(super) shared: Arc<Shared>,
    pub(super) desktop: Box<dyn Desktop>,
    pub(super) nucleus: Box<dyn Nucleus>,
    pub(super) keeper: Keeper,
    pub(super) title: String,
    pub(super) players: usize,
    /// How long Nucleus lists the profile before it is clicked
    /// ([`super::SETTLE`]), and since when it does.
    pub(super) settle: Duration,
    pub(super) listed_at: Option<Instant>,
    pub(super) clicked: bool,
    /// When the profile was last clicked.
    pub(super) clicked_at: Option<Instant>,
    /// Whether Nucleus hid its profile list after a click: it plays then,
    /// and the profile is never clicked again.
    pub(super) hid: bool,
    /// The profile plays by itself, for a handler with steps of its own
    /// (`Handler::has_steps`): Nucleus has no Play to press before them.
    pub(super) plays_itself: bool,
    /// Play was pressed. Never again: Nucleus turns it into Stop.
    pub(super) played: bool,
    /// The forms Nucleus showed over its window after a click, closed;
    /// whether one was closed since the last click; the last one.
    pub(super) closed: BTreeSet<u64>,
    pub(super) boxed: bool,
    pub(super) last_form: Option<MessageBox>,
    /// Why the last click on the profile was refused.
    pub(super) unclicked: Option<String>,
    /// While the profile is not shown: Nucleus' message boxes at the last
    /// look, every one logged so far, whether it showed any window, and why
    /// the last look failed. A box Windows showed for another program
    /// before the launch (`before`, its windows) is not Nucleus'.
    pub(super) showing: Vec<MessageBox>,
    pub(super) logged: Vec<MessageBox>,
    pub(super) before: BTreeSet<u64>,
    pub(super) shown: bool,
    pub(super) unseen: Option<String>,
    /// Nucleus' crashlog.txt as it was at the launch. None for a runner
    /// tested alone.
    pub(super) crashlog: Option<Crashlog>,
    /// The folder Nucleus Co-op runs from and the game's GUID, to tell
    /// whether Nucleus lists the game, read as `reader` (the user, for their
    /// own copy). None for a runner tested alone.
    pub(super) game: Option<(PathBuf, String)>,
    pub(super) reader: Option<OwnedHandle>,
    /// Clicked, instances found, windows found.
    pub(super) progress: (bool, usize, usize),
    pub(super) windows: Vec<Option<u64>>,
    pub(super) deadline: Instant,
    pub(super) exited: bool,
    pub(super) failed: u32,
    /// The game's folder: walked again after a failed start
    /// (`super::super::elevation::forget`).
    pub(super) game_folder: Option<PathBuf>,
    /// For Rubylight's own handler: the host's Nucleus folder and the
    /// game's .exe, kept there when the game refuses ProtoInput's hooks
    /// ([`files::refuse_inject`]).
    pub(super) own_game: Option<(PathBuf, String)>,
    /// Until when Nucleus may still show that box: it injects once, as it
    /// starts the game or right after. Set at the first window.
    pub(super) inject_by: Option<Instant>,
    /// An instance that exited while that box may still come, and when
    /// ([`REFUSAL_GRACE`]).
    pub(super) exited_at: Option<(usize, Instant)>,
    /// The user's Nucleus settings the session changed (None from the
    /// user's own copy, whose stay), then the game's compatibility settings
    /// from before Nucleus started; last, so they drop after Nucleus. None
    /// for a runner tested alone.
    pub(super) settings: Option<files::SettingsGuard>,
    pub(super) layers: Option<Guard>,
    /// Told each slot's instance after every pass ([`Instances`]).
    pub(super) instances: Option<Instances>,
    /// This session's virtual pads: unplugged in [`Runner::finish`] as soon as
    /// the session ends (even before [`super::Running::stop`] or drop).
    pub(super) pads: Option<Arc<dyn Pads>>,
    /// A one-player game without ProtoInput's hooks ([`super::unhooked`]): its
    /// window is brought to the front as it shows, since such a game takes
    /// its pad only there and nothing hides from it that it isn't (#59).
    /// The windows brought there, each once, and only while nobody uses
    /// the PC.
    pub(super) to_front: bool,
    pub(super) fronted: BTreeSet<u64>,
}

impl Runner {
    /// One look at the desktop. Some when the session ends, with why.
    pub(super) fn step(&mut self, now: Instant) -> Option<End> {
        {
            let mut session = self.shared.session.lock().unwrap();
            for event in session.tick(now) {
                if let coop::Event::Freed { slot, device } = event {
                    tracing::info!(slot, device, "Nucleus Co-op player did not return");
                }
            }
            if let Phase::Ended(why) = &session.phase {
                return Some(why.clone());
            }
        }
        let job = match self.nucleus.job() {
            Ok(job) => job,
            Err(error) => {
                return self.failed_pass(error.context("listing Nucleus Co-op's processes"));
            }
        };
        // Once an instance runs, Nucleus has loaded a profile: a second
        // click could start the game again.
        if !self.clicked
            && self.progress.1 == 0
            && let Some(end) = self.click(&job, now)
        {
            return Some(end);
        }
        let sizes = self.shared.sizes.lock().unwrap().clone();
        let pass = match self.keeper.pass(self.desktop.as_mut(), &job, &sizes) {
            Ok(pass) => pass,
            Err(error) => return self.failed_pass(error),
        };
        let windows = pass.windows.iter().flatten().count();
        if windows > 0 && self.inject_by.is_none() {
            self.inject_by = Some(now + START_TIMEOUT);
        }
        let injecting = self.inject_by.is_none_or(|by| now < by);
        // From the click: a game Nucleus injects into as it starts it is
        // refused before its process runs, and ended before the box shows.
        if self.clicked
            && injecting
            && let Some(end) = self.refused_hooks(&job, &pass.roots)
        {
            return Some(end);
        }
        if let Some(&slot) = pass.gone.first() {
            let running = self.shared.session.lock().unwrap().phase == Phase::Running;
            if self.own_game.is_some() && injecting && !running {
                self.exited_at.get_or_insert((slot, now));
            } else {
                return Some(self.exited(slot));
            }
        }
        if let Some((slot, at)) = self.exited_at
            && now >= at + REFUSAL_GRACE
        {
            return Some(self.exited(slot));
        }
        *self.shared.roots.lock().unwrap() = pass.roots.clone();
        if let Some(instances) = &self.instances {
            instances.found(&pass.trees);
        }
        let roots = pass.roots.iter().flatten().count();
        let progress = (self.clicked, roots, windows);
        if progress != self.progress {
            self.progress = progress;
            self.deadline = now + START_TIMEOUT;
        }
        self.windows = pass.windows.clone();
        if self.to_front
            && let Some(&Some(window)) = pass.windows.first()
            && !self.fronted.contains(&window)
            // Never from someone using the PC: it waits until they don't.
            && self.desktop.pc_idle()
        {
            // Once per window: the person at the PC may take the focus back.
            self.fronted.insert(window);
            if self.desktop.to_front(window) {
                tracing::info!(
                    title = self.title,
                    "Nucleus Co-op: the game runs without ProtoInput's hooks, so its window was brought to the front, where it takes the stream's controller"
                );
            } else {
                tracing::warn!(
                    title = self.title,
                    "Nucleus Co-op: the game runs without ProtoInput's hooks and Windows kept its window from the front: it takes the stream's controller only once it is clicked on the PC"
                );
            }
        }
        let running = {
            let mut session = self.shared.session.lock().unwrap();
            let running = session.phase == Phase::Running;
            // While it runs, a window that closed is gone from its slot at once.
            if running || windows == self.players {
                if !running {
                    tracing::info!(title = self.title, "Nucleus Co-op session running");
                }
                session.running(&pass.windows);
            }
            session.phase == Phase::Running
        };
        if !self.exited {
            let code = match self.nucleus.exit_code() {
                Ok(code) => code,
                Err(error) => {
                    return self.failed_pass(error.context("reading Nucleus Co-op's exit code"));
                }
            };
            if let Some(code) = code {
                self.exited = true;
                if !running && roots == 0 {
                    return Some(End::Failed(format!(
                        "Nucleus Co-op exited with code {code} before starting {}",
                        self.title
                    )));
                }
                tracing::warn!(
                    code,
                    "Nucleus Co-op exited; the session goes on with the instances it started"
                );
            }
        }
        if !running && roots == 0 && self.clicked {
            // A click that comes while Nucleus 2.4.2 still makes its setup
            // screen fails to load the profile, and its box saying so
            // disables the window, so no later click lands (#59). Nobody
            // answers it during a background stream: it is closed, and the
            // profile clicked again.
            match self.desktop.close_forms(&job) {
                Ok(closed) => {
                    for form in closed {
                        // One that takes a few passes to close is told once.
                        if self.closed.insert(form.window) {
                            tracing::warn!(
                                "Nucleus Co-op showed {} over its window after the profile {TITLE} was clicked; closed it to click the profile again",
                                said(&form)
                            );
                        }
                        self.last_form = Some(form);
                        self.boxed = true;
                    }
                }
                Err(error) => {
                    return self.failed_pass(error.context("closing Nucleus Co-op's windows"));
                }
            }
            // A box Nucleus shows after the click (its End-key reminder, a
            // check of the game) holds the start as one before it does.
            self.look(&job);
            // Nucleus shows Play once the profile is loaded and every player
            // has a controller.
            if !self.plays_itself && !self.played && self.showing.is_empty() {
                match self.desktop.button(&job, PLAY) {
                    Ok(Some(button)) => self.play(&job, button),
                    Ok(None) => {}
                    Err(error) => {
                        return self
                            .failed_pass(error.context("looking for Nucleus Co-op's Play button"));
                    }
                }
            }
            // Nucleus hides the list when it plays the profile: a list that
            // comes back means no second click, which could start the game
            // again.
            if !self.hid {
                match self.desktop.label(&job, TITLE) {
                    Ok(Some(label)) => {
                        // Not as the start times out: what the last click
                        // led to says why.
                        if !self.played
                            && self.showing.is_empty()
                            && self.clicked_at.is_some_and(|at| now >= at + RECLICK)
                            && now < self.deadline
                        {
                            // Still no instance: clicked again.
                            self.press(&job, label, now);
                        }
                    }
                    Ok(None) => self.hid = true,
                    Err(error) => {
                        return self.failed_pass(
                            error.context("looking for Nucleus Co-op's profile list"),
                        );
                    }
                }
            }
        }
        self.failed = 0;
        if !running && now >= self.deadline {
            return Some(End::Failed(self.timeout()));
        }
        None
    }

    /// Load the host's profile: Nucleus shows Play then, or starts the game
    /// by itself for a profile that plays by itself.
    fn click(&mut self, job: &BTreeSet<u32>, now: Instant) -> Option<End> {
        let label = match self.desktop.label(job, TITLE) {
            Ok(Some(label)) => label,
            Ok(None) => {
                self.look(job);
                return None;
            }
            Err(error) => return Some(End::Failed(format!("{error:#}"))),
        };
        let listed = *self.listed_at.get_or_insert(now);
        if now < listed + self.settle {
            return None;
        }
        self.press(job, label, now);
        None
    }

    /// Click the profile's entry in Nucleus' list.
    fn press(&mut self, job: &BTreeSet<u32>, label: u64, now: Instant) {
        match self.desktop.click(label) {
            Ok(()) => {
                if self.clicked {
                    tracing::info!(
                        "Nucleus Co-op profile {TITLE} clicked again: it still lists it"
                    );
                } else {
                    tracing::info!("Nucleus Co-op profile {TITLE} clicked");
                }
                self.clicked = true;
                self.clicked_at = Some(now);
                self.boxed = false;
                self.unclicked = None;
            }
            Err(error) => {
                let why = format!("{error:#}");
                if self.unclicked.as_ref() != Some(&why) {
                    tracing::warn!(why, "Nucleus Co-op profile {TITLE} not clicked");
                }
                self.unclicked = Some(why);
                // What covers the list may be Nucleus' own message box.
                self.look(job);
            }
        }
    }

    /// Press Nucleus' Play, which starts the game; pressed once, it stops it.
    fn play(&mut self, job: &BTreeSet<u32>, button: u64) {
        match self.desktop.click(button) {
            Ok(()) => {
                tracing::info!("Nucleus Co-op Play pressed");
                self.played = true;
                self.unclicked = None;
            }
            Err(error) => {
                let why = format!("{error:#}");
                if self.unclicked.as_ref() != Some(&why) {
                    tracing::warn!(why, "Nucleus Co-op Play not pressed");
                }
                self.unclicked = Some(why);
                self.look(job);
            }
        }
    }

    /// End the session with `end` (unless it ended already), and stop
    /// Nucleus and the instances.
    pub(super) fn finish(mut self, end: End) {
        let phase = {
            let mut session = self.shared.session.lock().unwrap();
            session.end(end);
            session.phase.clone()
        };
        match phase {
            Phase::Ended(End::Ended) => {
                tracing::info!(title = self.title, "Nucleus Co-op session ended")
            }
            Phase::Ended(why) => {
                tracing::warn!(title = self.title, "Nucleus Co-op session ended: {why}");
                if let (End::Failed(_), Some(game)) = (why, &self.game_folder) {
                    super::super::elevation::forget(game);
                }
            }
            _ => {}
        }
        let instances = self.keeper.processes();
        // While they run: Windows sets a program's devices only then.
        if let Some(told) = &self.instances {
            told.stopping(&instances.keys().copied().collect::<Vec<_>>());
        }
        self.nucleus.stop(&instances);
        if let Some(told) = &self.instances {
            told.stopped();
        }
        if let Some(pads) = &self.pads {
            pads.unplug();
        }
        // Nucleus sets them as it starts each instance: back once all stopped.
        if let Some(layers) = &mut self.layers {
            layers.restore();
        }
        if let Some(settings) = &mut self.settings {
            settings.restore();
        }
    }
}

pub(super) fn run(mut runner: Runner, stop: mpsc::Receiver<()>) {
    let end = loop {
        if let Some(end) = runner.step(Instant::now()) {
            break end;
        }
        match stop.recv_timeout(PERIOD) {
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break End::Ended,
        }
    };
    runner.finish(end);
}
