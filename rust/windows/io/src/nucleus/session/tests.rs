//! Tests for Nucleus Co-op session launch configuration, runner flow, hook
//! refusals, crashlog/box diagnostics, and full session lifecycle.
use super::super::instances::tests::{Fake, ON_SCREEN, fake, process, window};
use super::super::pads::tests::{Fake as FakeXInput, Recorder};
use super::*;
use butterpollo_session::nucleus::window::{Rect, Window};
use butterpollo_session::steam::Process;

mod boxes_crashlog;
mod hooks_aslr;
mod lifecycle;
mod runner_flow;

#[test]
fn webview2_keeps_its_data_in_the_users_own_folder() {
    // WebView2's browser runs without administrator rights and could not
    // write Nucleus' Administrators-only webview\cache (#59).
    let local = Path::new(r"C:\Users\Player\AppData\Local");
    let environment = webview_environment(local, &BTreeMap::new());
    assert_eq!(environment.len(), 1);
    assert_eq!(
        environment["WEBVIEW2_USER_DATA_FOLDER"],
        r"C:\Users\Player\AppData\Local\Butterpollo\nucleus-webview"
    );
}

#[test]
fn a_steam_games_app_id_joins_webview2s_folder() {
    // Nucleus needs both: WebView2's folder for its window, and the app
    // id so a Steam game it starts by its .exe doesn't restart via Steam.
    let local = Path::new(r"C:\Users\Player\AppData\Local");
    let environment = webview_environment(local, &steam_environment(Some(400)));
    assert_eq!(
        environment.keys().collect::<Vec<_>>(),
        ["SteamAppId", "SteamGameId", "WEBVIEW2_USER_DATA_FOLDER"]
    );
    assert_eq!(environment["SteamAppId"], "400");
}

#[test]
fn the_log_says_what_rights_nucleus_really_has() {
    assert_eq!(rights_note(false, &Ok(false)), None);
    assert_eq!(rights_note(true, &Ok(true)), None);
    // An administrator with User Account Control off: a "user" run
    // gets their full token.
    let uac_off = rights_note(false, &Ok(true)).unwrap();
    assert!(
        uac_off.contains("runs with administrator rights all the same"),
        "{uac_off}"
    );
    assert!(
        rights_note(true, &Ok(false))
            .unwrap()
            .contains("without administrator rights")
    );
    let unread = rights_note(false, &Err(anyhow::anyhow!("access denied"))).unwrap();
    assert!(unread.ends_with(": access denied"), "{unread}");
}

#[test]
fn the_users_own_copy_starts_as_them_with_the_elevated_ones_environment() {
    let local = Path::new(r"C:\Users\Player\AppData\Local");
    let environment = webview_environment(local, &steam_environment(Some(400)));
    let copy = Path::new(r"C:\ProgramData\Butterpollo\nucleus-user\S-1-5-21-7\0123456789abcdef");
    let user = launch_of(copy, "Rubylight-game", &environment, Run::User);
    assert_eq!(
        user,
        Launch {
            program: copy.join("NucleusCoop.exe"),
            folder: copy.to_path_buf(),
            arguments: vec!["Rubylight-game".into()],
            elevated: false,
            environment: BTreeMap::from([
                ("SteamAppId".to_string(), "400".to_string()),
                ("SteamGameId".to_string(), "400".to_string()),
                (
                    "WEBVIEW2_USER_DATA_FOLDER".to_string(),
                    r"C:\Users\Player\AppData\Local\Butterpollo\nucleus-webview".to_string()
                ),
            ]),
        }
    );
    // Nothing lowers the game's rights behind its back (__COMPAT_LAYER):
    // the same environment as elevated, only the token differs.
    let host = Path::new(r"C:\ProgramData\Butterpollo\nucleus\2.4.2");
    let elevated = launch_of(host, "Rubylight-game", &environment, Run::Elevated);
    assert!(elevated.elevated);
    assert_eq!(elevated.environment, user.environment);
}

#[test]
fn an_account_without_administrator_rights_is_refused_before_nucleus_starts() {
    // A standard user's Nucleus Co-op hung on its loading screen: it
    // could not write its WebView2 cache into the Administrators-only folder.
    for refused in [Refused::Open, Refused::Game] {
        assert!(require_administrator(Account::Elevates, refused).is_ok());
        let words = format!(
            "{:#}",
            require_administrator(Account::Standard, refused).unwrap_err()
        );
        assert_eq!(
            words,
            "Nucleus Co-op needs administrator rights, and Windows doesn't give this account any: sign in to Windows on this PC as an administrator"
        );
    }
    // An administrator under Windows' Administrator protection is told
    // what can't run and what still does: never to sign in as an
    // administrator, which doesn't help, nor to turn it off.
    let open = format!(
        "{:#}",
        require_administrator(Account::Protected, Refused::Open).unwrap_err()
    );
    assert_eq!(
        open,
        "Nucleus Co-op needs administrator rights, and Windows' Administrator protection keeps them from Rubylight, so Nucleus Co-op can't open on this PC; games Rubylight adds itself still stream if they need no administrator rights"
    );
    // The console's words before the button is pressed.
    assert_eq!(open, needs_administrator(Account::Protected));
    let game = format!(
        "{:#}",
        require_administrator(Account::Protected, Refused::Game).unwrap_err()
    );
    assert_eq!(
        game,
        "Nucleus Co-op needs administrator rights, and Windows' Administrator protection keeps them from Rubylight, so Nucleus Co-op can't run this game on this PC; games Rubylight adds itself still stream if they need no administrator rights"
    );
}

/// A fake desktop the test changes while the session runs.
#[derive(Clone)]
struct Screen(Arc<Mutex<Fake>>);
impl Desktop for Screen {
    fn processes(&mut self) -> Result<Vec<Process>> {
        self.0.lock().unwrap().processes()
    }
    fn image_path(&mut self, pid: u32) -> Option<String> {
        self.0.lock().unwrap().image_path(pid)
    }
    fn windows(&mut self) -> Result<Vec<Window>> {
        self.0.lock().unwrap().windows()
    }
    fn monitors(&mut self) -> Result<Vec<Rect>> {
        self.0.lock().unwrap().monitors()
    }
    fn virtual_screen(&mut self) -> Rect {
        self.0.lock().unwrap().virtual_screen()
    }
    fn place(&mut self, window: u64, to: Rect) -> Result<()> {
        self.0.lock().unwrap().place(window, to)
    }
    fn unframe(&mut self, window: u64) -> Result<()> {
        self.0.lock().unwrap().unframe(window)
    }
    fn unthrottle(&mut self, processes: &BTreeSet<u32>) {
        self.0.lock().unwrap().unthrottle(processes)
    }
    fn label(&mut self, processes: &BTreeSet<u32>, text: &str) -> Result<Option<u64>> {
        self.0.lock().unwrap().label(processes, text)
    }
    fn button(&mut self, processes: &BTreeSet<u32>, name: &str) -> Result<Option<u64>> {
        self.0.lock().unwrap().button(processes, name)
    }
    fn click(&mut self, label: u64) -> Result<()> {
        self.0.lock().unwrap().click(label)
    }
    fn to_front(&mut self, window: u64) -> bool {
        self.0.lock().unwrap().to_front(window)
    }
    fn pc_idle(&mut self) -> bool {
        self.0.lock().unwrap().pc_idle()
    }
    fn message_boxes(&mut self, processes: &BTreeSet<u32>) -> Result<Vec<MessageBox>> {
        self.0.lock().unwrap().message_boxes(processes)
    }
    fn close_forms(&mut self, processes: &BTreeSet<u32>) -> Result<Vec<MessageBox>> {
        self.0.lock().unwrap().close_forms(processes)
    }
    fn owner(&mut self, window: u64) -> Result<u32> {
        self.0.lock().unwrap().owner(window)
    }
    fn forced_aslr(&mut self, pid: u32) -> Result<bool> {
        self.0.lock().unwrap().forced_aslr(pid)
    }
}

const NUCLEUS: u32 = 10;
const SINCE: u64 = 100;

#[derive(Clone, Default)]
struct Probe {
    exit: Arc<Mutex<Option<u32>>>,
    stopped: Arc<Mutex<Option<BTreeMap<u32, u64>>>>,
    launched: Arc<Mutex<Vec<String>>>,
    environments: Arc<Mutex<Vec<BTreeMap<String, String>>>>,
    /// The user's compatibility settings.
    layers: super::super::layers::tests::Map,
    /// Why Nucleus does not start, when it does not.
    refused: Arc<Mutex<Option<String>>>,
    /// The folder each launch started Nucleus from.
    folders: Arc<Mutex<Vec<PathBuf>>>,
    /// What was written through the launcher's account, and when it ran.
    writes: Arc<Mutex<usize>>,
    /// The user's profile folder, where Nucleus keeps per-game settings;
    /// None leaves the launcher without one.
    profile: Arc<Mutex<Option<PathBuf>>>,
}
struct FakeNucleus {
    probe: Probe,
    screen: Screen,
}
impl Nucleus for FakeNucleus {
    fn since(&self) -> u64 {
        SINCE
    }
    fn exit_code(&mut self) -> Result<Option<u32>> {
        Ok(*self.probe.exit.lock().unwrap())
    }
    fn job(&mut self) -> Result<BTreeSet<u32>> {
        // Nucleus and every process it started, as its job would hold them.
        let desktop = self.screen.0.lock().unwrap();
        let exited = self.probe.exit.lock().unwrap().is_some();
        Ok(desktop
            .processes
            .iter()
            .filter(|p| (p.pid == NUCLEUS && !exited) || p.parent == NUCLEUS)
            .map(|p| p.pid)
            .collect())
    }
    fn stop(&mut self, instances: &BTreeMap<u32, u64>) {
        *self.probe.stopped.lock().unwrap() = Some(instances.clone());
    }
}
impl Launcher for (Probe, Screen) {
    fn launch(
        &mut self,
        folder: &Path,
        guid: &str,
        environment: &BTreeMap<String, String>,
    ) -> Result<Box<dyn Nucleus>> {
        if let Some(why) = self.0.refused.lock().unwrap().clone() {
            bail!(why);
        }
        self.0.folders.lock().unwrap().push(folder.to_path_buf());
        self.0.launched.lock().unwrap().push(guid.into());
        self.0
            .environments
            .lock()
            .unwrap()
            .push(environment.clone());
        self.1
            .0
            .lock()
            .unwrap()
            .processes
            .push(process(NUCLEUS, 1, SINCE, EXECUTABLE));
        Ok(Box::new(FakeNucleus {
            probe: self.0.clone(),
            screen: self.1.clone(),
        }))
    }
    fn in_profile(&self, work: &mut dyn FnMut(&Path) -> Result<()>) -> Result<()> {
        let profile = self.0.profile.lock().unwrap().clone();
        profile.map_or(Ok(()), |profile| work(&profile))
    }
}

fn handler() -> Handler {
    Handler {
        file: "Game.js".into(),
        guid: "Game".into(),
        name: "Game".into(),
        executable: "Game.exe".into(),
        max_players: 4,
        supports_keyboard: false,
        hides_taskbar: false,
        has_steps: false,
        own: false,
        owner: None,
        admin: Vec::new(),
    }
}

fn kind(players: usize) -> coop::Kind {
    if players == 1 {
        coop::Kind::Background
    } else {
        coop::Kind::Coop
    }
}

fn runner(players: usize, screen: &Screen, probe: &Probe, t0: Instant) -> Runner {
    let pads: Vec<u8> = (0..players as u8).collect();
    let session =
        coop::Session::new(1, kind(players), "Game", "Game", &pads, ("phone", "Phone")).unwrap();
    screen
        .0
        .lock()
        .unwrap()
        .processes
        .push(process(NUCLEUS, 1, SINCE, EXECUTABLE));
    Runner {
        shared: Arc::new(Shared {
            session: Mutex::new(session),
            sizes: Mutex::new(vec![(1280, 720); players]),
            roots: Mutex::new(vec![None; players]),
        }),
        desktop: Box::new(screen.clone()),
        nucleus: Box::new(FakeNucleus {
            probe: probe.clone(),
            screen: screen.clone(),
        }),
        keeper: Keeper::new(players, &["Game.exe".into()], r"C:\Nucleus\content", SINCE),
        title: "Game".into(),
        players,
        // Tests of the later steps click the profile at once.
        settle: Duration::ZERO,
        listed_at: None,
        clicked: false,
        clicked_at: None,
        hid: false,
        // Tests of the later steps have Nucleus play the profile.
        plays_itself: true,
        played: false,
        closed: BTreeSet::new(),
        boxed: false,
        last_form: None,
        unclicked: None,
        showing: Vec::new(),
        logged: Vec::new(),
        before: BTreeSet::new(),
        shown: false,
        unseen: None,
        crashlog: None,
        game: None,
        reader: None,
        progress: (false, 0, 0),
        windows: vec![None; players],
        deadline: t0 + START_TIMEOUT,
        exited: false,
        failed: 0,
        game_folder: None,
        own_game: None,
        inject_by: None,
        exited_at: None,
        settings: None,
        layers: None,
        instances: None,
        to_front: false,
        fronted: BTreeSet::new(),
    }
}

fn screen() -> Screen {
    Screen(Arc::new(Mutex::new(Fake {
        obey: true,
        ..fake(vec![], vec![])
    })))
}

/// Nucleus starts player `slot`'s instance with a window on the monitor.
fn instance(screen: &Screen, slot: u32) {
    let mut desktop = screen.0.lock().unwrap();
    let pid = 20 + slot;
    desktop.processes.push(process(
        pid,
        NUCLEUS,
        SINCE + 1 + u64::from(slot),
        "Game.exe",
    ));
    desktop
        .windows
        .push(window(u64::from(pid) * 10, pid, ON_SCREEN));
}

fn label(screen: &Screen) {
    screen
        .0
        .lock()
        .unwrap()
        .labels
        .push((NUCLEUS, TITLE.into(), 7));
}

const HINT: &str = "open Nucleus Co-op and load the profile Rubylight once";

/// What a session told about its instances; whether Nucleus had stopped
/// them when it told they stop.
#[derive(Default)]
struct Listener {
    found: Mutex<Vec<Vec<BTreeSet<Identity>>>>,
    stopping: Mutex<Option<(Vec<u32>, bool)>>,
    stopped: Mutex<bool>,
    probe: Probe,
}
impl Told for Listener {
    fn found(&self, trees: &[BTreeSet<Identity>]) {
        self.found.lock().unwrap().push(trees.to_vec());
    }
    fn stopping(&self, pids: &[u32]) {
        let stopped = self.probe.stopped.lock().unwrap().is_some();
        *self.stopping.lock().unwrap() = Some((pids.to_vec(), stopped));
    }
    fn stopped(&self) {
        assert!(self.probe.stopped.lock().unwrap().is_some());
        *self.stopped.lock().unwrap() = true;
    }
}

/// The session's pads: a recorder the test keeps reading.
struct Recorded(Arc<Recorder>);
impl Pads for Recorded {
    fn send(&self, event: Input) {
        self.0.send(event)
    }
    fn wait_ready(&self, mask: u16, timeout: Duration) -> bool {
        self.0.wait_ready(mask, timeout)
    }
    fn reports(&self) -> Vec<PadReport> {
        self.0.reports()
    }
}

/// The game the sessions start.
const GAME: &str = r"D:\Games\Game\Game.exe";

fn start_with(
    folder: &Path,
    players: usize,
    taken: [bool; 4],
    screen: &Screen,
    probe: &Probe,
) -> (Result<Running>, Arc<Recorder>) {
    start_steam(folder, players, taken, screen, probe, None, GAME)
}

impl Launcher for (Probe, Screen, ()) {
    fn launch(
        &mut self,
        folder: &Path,
        guid: &str,
        environment: &BTreeMap<String, String>,
    ) -> Result<Box<dyn Nucleus>> {
        (self.0.clone(), self.1.clone()).launch(folder, guid, environment)
    }
    fn write(&self, work: &mut dyn FnMut() -> Result<()>) -> Result<()> {
        *self.0.writes.lock().unwrap() += 1;
        work()
    }
    fn in_profile(&self, work: &mut dyn FnMut(&Path) -> Result<()>) -> Result<()> {
        (self.0.clone(), self.1.clone()).in_profile(work)
    }
}

fn start_steam(
    folder: &Path,
    players: usize,
    taken: [bool; 4],
    screen: &Screen,
    probe: &Probe,
    steam_app: Option<u32>,
    exe_path: &str,
) -> (Result<Running>, Arc<Recorder>) {
    let recorder = Arc::new(Recorder::default());
    let handler = handler();
    let mut xinput = FakeXInput {
        pads: &recorder,
        taken,
        hidden: false,
        intruder: false,
    };
    let request = Start {
        id: 1,
        kind: kind(players),
        folder,
        user_copy: None,
        handler: &handler,
        exe_path,
        steam_app,
        title: "Game",
        players,
        owner: ("phone", "Phone"),
        size: (1280, 720),
        instances: None,
    };
    let running = start(
        &request,
        Box::new(Recorded(recorder.clone())),
        &mut xinput,
        Box::new(screen.clone()),
        Box::new(probe.layers.clone()),
        &mut (probe.clone(), screen.clone()),
    );
    (running, recorder)
}

/// Wait until the session thread got `running` to `phase`.
fn wait_for(running: &Running, phase: impl Fn(&Phase) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !running.with_session(|s| phase(&s.phase)) {
        assert!(
            Instant::now() < deadline,
            "{:?}",
            running.with_session(|s| s.phase.clone())
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
