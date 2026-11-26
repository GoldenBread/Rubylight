//! A Nucleus session on the PC: Nucleus Co-op started for one game with the
//! host's profile, each player's instance found and parked beside the
//! desktop, each player's pad and audio, and all of it stopped again. A
//! failure ends the session with the reason the stream and the console
//! show; nothing falls back to another mode.
use super::desktop::Win32;
#[cfg(test)]
use super::instances::MessageBox;
use super::instances::{Desktop, Keeper};
use super::layers::{self, Guard, Layers};
use super::pads::{self, Dropped, Pads, XInput};
use super::{files, user_copy};
use crate::input::PadReport;
use anyhow::{Context, Result, bail, ensure};
use butterpollo_input_policy::input::Input;
use butterpollo_session::audio_scope::Identity;
use butterpollo_session::nucleus::coop;
#[cfg(test)]
use butterpollo_session::nucleus::coop::{End, Phase};
use butterpollo_session::nucleus::elevation::{Account, Refused, Run};
#[cfg(test)]
use butterpollo_session::nucleus::generic;
use butterpollo_session::nucleus::generic::Inject;
use butterpollo_session::nucleus::handler::Handler;
use butterpollo_session::nucleus::install::EXECUTABLE;
use butterpollo_session::nucleus::profile::{Player, TITLE};
use butterpollo_session::nucleus::window;
use std::collections::{BTreeMap, BTreeSet};
use std::os::windows::io::OwnedHandle;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

mod diagnostics;
mod launch;
mod runner;

pub use launch::{
    Elevated, Opened, Unelevated, already_running, needs_administrator, open, running,
    steam_environment,
};

use diagnostics::Crashlog;
use launch::*;
use runner::{Runner, Shared, run};

/// How long Nucleus may take to show the instances, counted again from
/// each step it makes (the profile clicked, an instance, a window).
pub const START_TIMEOUT: Duration = Duration::from_secs(60);
/// How often the session looks at the desktop.
const PERIOD: Duration = Duration::from_millis(250);
/// How long after a click on the profile it is clicked again while Nucleus
/// still lists it: a loaded profile with every player's controller plays at
/// once and hides the list within about a second, and a click before its
/// setup screen is made fails.
const RECLICK: Duration = Duration::from_secs(5);
/// How long Nucleus lists the profile before it is clicked the first time.
/// Nucleus Co-op 2.4.2 finds the PC's screens for its setup screen on a 3 s
/// timer (`BoundsFunctions.ScreensWatcher_Tick`); a profile clicked before
/// that fails to load, and its untitled box says so (#56).
const SETTLE: Duration = Duration::from_millis(3500);
/// Nucleus Co-op's Play button, as its program calls it (`MainForm.Designer`).
const PLAY: &str = "btn_Play";
/// Passes in a row that may fail (the desktop switching for a UAC prompt)
/// before the session ends with the error.
const FAILED_PASSES: u32 = 8;
/// How long an instance of Rubylight's own handler's game that exited
/// before the session runs waits for Nucleus' box saying the game refused
/// its hooks: one Nucleus fails to inject into as it starts it is ended
/// before the box shows (#59).
const REFUSAL_GRACE: Duration = Duration::from_secs(2);
/// What a game says in its own message box when Windows forces it to load
/// its images at random addresses (Exploit protection's "Force
/// randomization for images"), and waits there, its window up and black.
/// Assassin's Creed IV Black Flag Resynced shows only "Disable Mandatory
/// ASLR" (#59); none of Nucleus Co-op, ProtoInput and EasyHook has that
/// text.
const MANDATORY_ASLR: &str = "Mandatory ASLR";
/// What the player can do about it. An exemption is a security setting, so
/// it is theirs to make.
const EXEMPT_ASLR: &str = "turn off \"Force randomization for images (Mandatory ASLR)\" for {program} in Windows Security, App & browser control, Exploit protection, Program settings";
/// How long Nucleus and the instances get to close before they are ended.
const STOP_TIMEOUT: Duration = Duration::from_secs(10);
/// Process loopback capture of one process tree came with this build.
pub const PROCESS_AUDIO_BUILD: u32 = 20348;

/// Nucleus Co-op as a session runs it: the elevated process on the PC, a
/// fake in tests.
pub trait Nucleus: Send {
    /// When it started (FILETIME): an older process is not its instance.
    fn since(&self) -> u64;
    /// Its exit code, once it exited.
    fn exit_code(&mut self) -> Result<Option<u32>>;
    /// The processes in its job: Nucleus and what it started.
    fn job(&mut self) -> Result<BTreeSet<u32>>;
    /// Close Nucleus, then the `instances` (pid → start time) it started;
    /// what has not closed in time is ended.
    fn stop(&mut self, instances: &BTreeMap<u32, u64>);
}

/// Starts Nucleus Co-op: elevated or as the user on the PC, a fake in tests.
pub trait Launcher {
    /// Start it for game `guid` with `environment` added to the user's: the
    /// game it starts gets the same.
    fn launch(
        &mut self,
        folder: &Path,
        guid: &str,
        environment: &BTreeMap<String, String>,
    ) -> Result<Box<dyn Nucleus>>;
    /// Run `work`, which writes Nucleus' settings and profile, as the
    /// account whose copy it writes to: the host for its own copy, the user
    /// for theirs.
    fn write(&self, work: &mut dyn FnMut() -> Result<()>) -> Result<()> {
        work()
    }
    /// Run `work` on the profile folder of the user Nucleus runs as, as
    /// that user: Nucleus keeps its per-game settings there
    /// ([`files::index_pads`]). Nothing runs for a launcher that has none.
    fn in_profile(&self, _work: &mut dyn FnMut(&Path) -> Result<()>) -> Result<()> {
        Ok(())
    }
    /// The account the folder it runs from is read as while the session
    /// runs: None for the host's own, the user for their copy.
    fn reader(&self) -> Result<Option<OwnedHandle>> {
        Ok(None)
    }
}

/// Why per-player audio cannot be captured on this Windows build. An
/// unknown build is taken as new enough, as `os_gate` does.
pub fn audio_limit(build: Option<u32>) -> Option<String> {
    let build = build?;
    (build < PROCESS_AUDIO_BUILD).then(|| {
        format!(
            "Per-player audio needs Windows build {PROCESS_AUDIO_BUILD} or newer and this PC runs build {build}: update Windows to hear each player's own game"
        )
    })
}

/// What a session tells about its instances as it runs.
pub trait Told: Send + Sync {
    /// Each slot's instance processes (its root and every process it
    /// started) after every pass, from the pass that finds the instance on:
    /// before its window shows, and often before the game opens its sound.
    fn found(&self, trees: &[BTreeSet<Identity>]);
    /// The instances' processes `pids`, still running, are about to be
    /// stopped with the session.
    fn stopping(&self, pids: &[u32]);
    /// ...and were stopped.
    fn stopped(&self);
}
pub type Instances = Arc<dyn Told>;

/// What starts a session.
pub struct Start<'a> {
    pub id: u32,
    pub kind: coop::Kind,
    /// The host's Nucleus Co-op folder, which keeps the compatibility
    /// settings' file.
    pub folder: &'a Path,
    /// The signed-in user's own copy ([`super::user_copy`]) when the
    /// session runs it as them without administrator rights; None runs
    /// `folder` elevated.
    pub user_copy: Option<&'a Path>,
    pub handler: &'a Handler,
    /// The game's .exe as Nucleus starts it: the user's added game's, or the
    /// one Rubylight's handler is for.
    pub exe_path: &'a str,
    /// The Steam app the game was found from, for Rubylight's own handler
    /// ([`environment`]).
    pub steam_app: Option<u32>,
    /// The game as the stream names it.
    pub title: &'a str,
    pub players: usize,
    /// The first player's device id and name.
    pub owner: (&'a str, &'a str),
    /// The first player's stream size.
    pub size: (u32, u32),
    /// Told the instances as the session finds them.
    pub instances: Option<Instances>,
}

/// A session that runs until [`stop`](Running::stop) or drop. Its calls
/// never wait for a stop, which can take seconds.
pub struct Running {
    shared: Arc<Shared>,
    pads: Arc<dyn Pads>,
    stop: Mutex<Option<mpsc::Sender<()>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
    players: usize,
    /// The crashlog.txt a start that times out quotes.
    #[cfg(test)]
    crashlog: PathBuf,
}

/// The stream's warning for a game Rubylight's handler starts without
/// ProtoInput's hooks ([`unhooked`]).
pub const UNHOOKED: &str = "nucleus_unhooked";

/// What the player should know when Rubylight's own handler starts the game
/// without ProtoInput's hooks, because it refused every way (#59).
fn unhooked(start: &Start) -> Result<Option<String>> {
    if !start.handler.own || files::inject_for(start.folder, start.exe_path)? != Inject::None {
        return Ok(None);
    }
    Ok(Some(format!(
        "{} refused every way the co-op helper injects its input hooks, so it runs without them: a keyboard, mouse or pad on the PC reaches it too, and it takes the controller only while its window is in front on the PC, where Rubylight puts it as it opens unless someone uses the PC. If the game pauses when it is not in front, the picture stops too",
        start.title
    )))
}

/// Whether the game's window is brought to the front as it shows: a
/// one-player game without ProtoInput's hooks ([`unhooked`]), which takes
/// its pad only there. A co-op session's windows share the screen, and a
/// hooked game takes its pad anywhere.
fn brought_to_front(start: &Start) -> Result<bool> {
    Ok(start.players == 1 && unhooked(start)?.is_some())
}

/// Start a session on the PC: virtual Xbox pads, XInput, the user's desktop
/// and compatibility settings, and Nucleus Co-op, elevated or from the
/// user's own copy.
pub fn start_on_pc(
    start: &Start,
    policy: butterpollo_input_policy::input_policy::Policy,
    warnings: Arc<butterpollo_session::session::Warnings>,
) -> Result<Running> {
    let mut launcher: Box<dyn Launcher> = match start.user_copy {
        Some(_) => Box::new(Unelevated::signed_in()?),
        None => Box::new(Elevated),
    };
    if let Some(why) = unhooked(start)? {
        warnings.set(UNHOOKED, why);
    }
    let pads = pads::virtual_pads(policy, warnings)?;
    let mut xinput = pads::System::open()?;
    self::start(
        start,
        Box::new(pads),
        &mut xinput,
        Box::new(Win32::default()),
        Box::new(layers::UserHive::signed_in()?),
        launcher.as_mut(),
    )
}

/// The folder of the game at `exe`, which [`super::elevation::folder_need`]
/// walks.
fn game_folder(exe: &str) -> Option<PathBuf> {
    Path::new(exe).parent().map(Path::to_path_buf)
}

/// What starting a session needs that doesn't need Nucleus Co-op closed:
/// administrator rights for the signed-in user when it runs elevated, their
/// AppData folder and their account, and the compatibility settings an
/// earlier session left in the host's `folder` put back. Checked before a
/// session closes the console's window, so a launch refused for one of
/// these leaves the window open.
pub fn ready(folder: &Path, run: Run) -> Result<()> {
    if run == Run::Elevated {
        require_administrator(crate::process::user_account()?, Refused::Game)?;
    }
    environment(&BTreeMap::new())?;
    layers::restore_left_in(&mut layers::UserHive::signed_in()?, folder)?;
    Ok(())
}

/// Start a session: refused while Nucleus Co-op already runs, then one pad
/// per player, the profile, the game's compatibility settings kept
/// ([`layers`]), and Nucleus. Should it fail, the game's folder is walked
/// again next time ([`super::elevation::forget`]).
pub fn start(
    start: &Start,
    pads: Box<dyn Pads>,
    xinput: &mut dyn XInput,
    desktop: Box<dyn Desktop>,
    layers: Box<dyn Layers>,
    launcher: &mut dyn Launcher,
) -> Result<Running> {
    begin(start, pads, xinput, desktop, layers, launcher).inspect_err(|_| {
        if let Some(game) = game_folder(start.exe_path) {
            super::elevation::forget(&game);
        }
    })
}

fn begin(
    start: &Start,
    pads: Box<dyn Pads>,
    xinput: &mut dyn XInput,
    mut desktop: Box<dyn Desktop>,
    layers: Box<dyn Layers>,
    launcher: &mut dyn Launcher,
) -> Result<Running> {
    // Its profile list would show the user's state, and its instances could
    // not be told from the session's.
    if let Some(process) = desktop
        .processes()?
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(EXECUTABLE))
    {
        bail!(already_running(process.pid));
    }
    let free = pads::free(xinput)?;
    ensure!(
        start.players <= free,
        "{} players need {} free XInput controller slots and the PC's own controllers leave {free}: disconnect one of the PC's controllers or use fewer players",
        start.players,
        start.players
    );
    let pads: Arc<dyn Pads> = Arc::from(pads);
    let indices = pads::plug(pads.as_ref(), xinput, start.players, pads::PLUG_TIMEOUT)?;
    let session = coop::Session::new(
        start.id,
        start.kind,
        &start.handler.guid,
        start.title,
        &indices,
        start.owner,
    )
    .map_err(anyhow::Error::msg)?;
    let screen = desktop.virtual_screen();
    let screens = desktop.monitors()?;
    let players = indices
        .iter()
        .enumerate()
        .map(|(slot, &xinput)| {
            Ok(Player {
                xinput,
                bounds: window::parking(screen, slot, start.size).map_err(anyhow::Error::msg)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    // The copy Nucleus runs from, written as the account it runs as. The
    // host's own settings are put back when the session ends, or here should
    // Nucleus not start; the user's copy keeps the session's, and nothing in
    // it is written as the host.
    let folder = start.user_copy.unwrap_or(start.folder);
    let settings = match start.user_copy {
        None => Some(files::write_settings(start.folder)?),
        Some(_) => None,
    };
    let mut number = 0;
    launcher.write(&mut || {
        if start.user_copy.is_some() {
            files::write_copy_settings(folder)?;
        }
        number = files::write_profile(
            folder,
            &start.handler.guid,
            &players,
            &screens,
            start.handler.has_steps,
        )?;
        Ok(())
    })?;
    // Nucleus takes the pads' assignment from there, not from the profile:
    // by button press, it would wait for a press no one gives (#56).
    launcher
        .in_profile(&mut |profile| files::index_pads(profile, &start.handler.guid))
        .context(
            "giving the game its pads by XInput index in the co-op helper's per-game settings",
        )?;
    tracing::info!(
        number,
        game = start.handler.guid,
        players = start.players,
        folder = %folder.display(),
        "Co-op profile {TITLE} written"
    );
    // Restored when the session ends, or here should Nucleus not start.
    let layers = Guard::take(
        layers,
        start.folder,
        folder,
        &start.handler.executable,
        start.exe_path,
    )
    .context("keeping the game's compatibility settings, which the co-op helper changes")?;
    // In the folder Nucleus runs from, read as the account that reads it.
    let reader = launcher.reader()?;
    let crashlog = Crashlog::at_launch(folder, reader.as_ref());
    #[cfg(test)]
    let crashlog_path = crashlog.path.clone();
    // Windows' boxes up now are another program's: Nucleus isn't running.
    let before = desktop
        .message_boxes(&BTreeSet::new())
        .context("listing the message boxes on the PC's screen before the co-op helper starts")?
        .into_iter()
        .map(|shown| shown.window)
        .collect();
    let nucleus = launcher.launch(
        folder,
        &start.handler.guid,
        &steam_environment(start.steam_app),
    )?;
    let content = folder.join("content");
    let keeper = Keeper::new(
        start.players,
        std::slice::from_ref(&start.handler.executable),
        &content.to_string_lossy(),
        nucleus.since(),
    );
    let shared = Arc::new(Shared {
        session: Mutex::new(session),
        sizes: Mutex::new(vec![start.size; start.players]),
        roots: Mutex::new(vec![None; start.players]),
    });
    let runner = Runner {
        shared: shared.clone(),
        desktop,
        nucleus,
        keeper,
        title: start.title.to_string(),
        players: start.players,
        settle: SETTLE,
        listed_at: None,
        clicked: false,
        clicked_at: None,
        hid: false,
        plays_itself: start.handler.has_steps,
        played: false,
        closed: BTreeSet::new(),
        boxed: false,
        last_form: None,
        unclicked: None,
        showing: Vec::new(),
        logged: Vec::new(),
        before,
        shown: false,
        unseen: None,
        crashlog: Some(crashlog),
        game: Some((folder.to_path_buf(), start.handler.guid.clone())),
        reader,
        progress: (false, 0, 0),
        windows: vec![None; start.players],
        deadline: Instant::now() + START_TIMEOUT,
        exited: false,
        failed: 0,
        game_folder: game_folder(start.exe_path),
        own_game: start
            .handler
            .own
            .then(|| (start.folder.to_path_buf(), start.exe_path.to_string())),
        inject_by: None,
        exited_at: None,
        settings,
        layers: Some(layers),
        instances: start.instances.clone(),
        pads: Some(pads.clone()),
        to_front: brought_to_front(start)?,
        fronted: BTreeSet::new(),
    };
    let (stop, stopped) = mpsc::channel();
    // Should the thread not start, dropping the runner ends Nucleus' job.
    let thread = std::thread::Builder::new()
        .name("nucleus".into())
        .spawn(move || run(runner, stopped))
        .context("starting the co-op session thread")?;
    Ok(Running {
        shared,
        pads,
        stop: Mutex::new(Some(stop)),
        thread: Mutex::new(Some(thread)),
        players: start.players,
        #[cfg(test)]
        crashlog: crashlog_path,
    })
}

impl Running {
    /// Read or change the session: players joining, leaving, kicked, ended.
    pub fn with_session<R>(&self, f: impl FnOnce(&mut coop::Session) -> R) -> R {
        f(&mut self.shared.session.lock().unwrap())
    }

    /// Each slot's game window while the session runs.
    pub fn windows(&self) -> Vec<Option<u64>> {
        let session = self.shared.session.lock().unwrap();
        session.slots.iter().map(|s| s.window).collect()
    }

    /// Slot `slot`'s capture target: its game window, captured on the GPU
    /// of `display`. A window Nucleus replaced gives a new target, which the
    /// stream reopens on; an error while the slot has no window.
    pub fn capture_target(&self, slot: usize, display: &str) -> Result<String> {
        let window = self
            .windows()
            .get(slot)
            .copied()
            .with_context(|| format!("the session has {} players, not {}", self.players, slot + 1))?
            .with_context(|| format!("Player {}'s game window is not open", slot + 1))?;
        Ok(butterpollo_media_policy::app_window::WindowTarget::new(window, display).to_string())
    }

    /// Park slot `slot`'s window at its player's stream size from the next
    /// pass on.
    pub fn resize(&self, slot: usize, size: (u32, u32)) -> Result<()> {
        let mut sizes = self.shared.sizes.lock().unwrap();
        let players = sizes.len();
        let entry = sizes
            .get_mut(slot)
            .with_context(|| format!("the session has {players} players, not {}", slot + 1))?;
        *entry = size;
        Ok(())
    }

    /// A device's input: its controller drives its slot's pad while it
    /// streams; anything else is dropped, with the limit to show.
    pub fn input(&self, device: &str, event: Input) -> Result<(), Dropped> {
        let slot = {
            let session = self.shared.session.lock().unwrap();
            match session.slot_of(device) {
                Some(slot) if session.route(device).is_some() => slot,
                _ => return Err(Dropped::Ignored),
            }
        };
        self.pads.send(pads::route(event, slot, self.players)?);
        Ok(())
    }

    /// Rumble and other feedback since the last call, per slot, for its
    /// player's first controller.
    pub fn feedback(&self) -> Vec<(usize, PadReport)> {
        self.pads
            .reports()
            .into_iter()
            .filter_map(|report| pads::feedback(report, self.players))
            .collect()
    }

    /// Slot `slot`'s instance, for its audio. An error on a Windows build
    /// without process loopback capture and while the instance is not there.
    pub fn audio(&self, slot: usize) -> Result<crate::process::AudioProcess> {
        self.audio_on(crate::os_helpers::windows_build(), slot)
    }

    fn audio_on(&self, build: Option<u32>, slot: usize) -> Result<crate::process::AudioProcess> {
        if let Some(limit) = audio_limit(build) {
            bail!(limit);
        }
        let root = self
            .shared
            .roots
            .lock()
            .unwrap()
            .get(slot)
            .copied()
            .flatten()
            .with_context(|| format!("Player {}'s game instance is not running", slot + 1))?;
        crate::process::AudioProcess::open(root)
            .with_context(|| format!("Player {}'s game instance", slot + 1))
    }

    /// End the session and wait until Nucleus and the instances closed; a
    /// second call waits for the first. Unplugs virtual pads immediately so
    /// an ended session never holds XInput slots until `Running` is dropped.
    pub fn stop(&self) {
        drop(self.stop.lock().unwrap().take());
        let mut thread = self.thread.lock().unwrap();
        if let Some(thread) = thread.take()
            && thread.join().is_err()
        {
            tracing::error!("the co-op session thread panicked");
        }
        self.pads.unplug();
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests;
