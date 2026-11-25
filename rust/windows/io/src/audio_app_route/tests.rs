//! Per-app audio routing and journal recovery tests.
use super::*;
use std::cell::RefCell;

mod watch;

const SINK: &str = r"\\?\SWD#MMDEVAPI#{steam}#{e6327cad-dcec-4949-ae8a-991e976a79d2}";
const HEADSET: &str = r"\\?\SWD#MMDEVAPI#{headset}#{e6327cad-dcec-4949-ae8a-991e976a79d2}";
const GAME: &str = r"e:\games\lego batman\legobatmanlotdk-win64-shipping.exe";

/// Windows as the routing sees it: each program's own device per role, and
/// the processes running now.
#[derive(Default)]
struct Fake {
    devices: RefCell<BTreeMap<(String, i32), String>>,
    running: RefCell<Vec<(u32, String)>>,
    /// A device Windows refuses (unplugged).
    gone: RefCell<Option<String>>,
    /// Windows answers S_OK and keeps nothing.
    ignores: std::cell::Cell<bool>,
    /// For each process listing, whether [`LISTED`] was free.
    listed: RefCell<Vec<bool>>,
    /// Processes that haven't opened their sound: Windows refuses both calls.
    silent: RefCell<std::collections::BTreeSet<u32>>,
}
/// The routes' lock a restore is checked against.
static LISTED: Mutex<State> = Mutex::new(State {
    active: BTreeMap::new(),
    closing: BTreeMap::new(),
    watching: false,
});
impl Fake {
    fn exe(&self, pid: u32) -> Result<String> {
        self.running
            .borrow()
            .iter()
            .find(|(p, _)| *p == pid)
            .map(|(_, exe)| exe.clone())
            .context("no such process")
    }
    fn device(&self, exe: &str, role: ERole) -> String {
        self.devices
            .borrow()
            .get(&(exe.into(), role.0))
            .cloned()
            .unwrap_or_default()
    }
}
impl Apps for Fake {
    fn get(&self, pid: u32, role: ERole) -> Result<String> {
        let exe = self.exe(pid)?;
        if self.silent.borrow().contains(&pid) {
            return Err(NoSoundYet.into());
        }
        Ok(self.device(&exe, role))
    }
    fn set(&self, pid: u32, role: ERole, device: &str) -> Result<()> {
        let exe = self.exe(pid)?;
        if self.silent.borrow().contains(&pid) {
            return Err(NoSoundYet.into());
        }
        anyhow::ensure!(
            self.gone.borrow().as_deref() != Some(device),
            "element not found"
        );
        if self.ignores.get() {
            return Ok(());
        }
        self.devices
            .borrow_mut()
            .insert((exe, role.0), device.into());
        Ok(())
    }
    fn running(&self, exes: &[&str]) -> Result<Vec<(u32, String)>> {
        self.listed.borrow_mut().push(LISTED.try_lock().is_ok());
        Ok(self
            .running
            .borrow()
            .iter()
            .filter(|(_, exe)| exes.iter().any(|e| same_program(e, exe)))
            .cloned()
            .collect())
    }
}

fn started(fake: &Fake, pid: u32) {
    fake.running.borrow_mut().push((pid, GAME.into()));
}
fn exited(fake: &Fake) {
    fake.running.borrow_mut().clear();
}

#[test]
fn the_game_plays_on_the_sink_and_goes_back_to_the_default_when_the_stream_ends() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (fake, mut state) = (Fake::default(), State::default());
    started(&fake, 7);
    let Routed::Sink(id) = route_with(&fake, &mut state, directory.path(), 7, GAME, SINK)? else {
        anyhow::bail!("expected Routed::Sink");
    };
    for role in ROLES {
        assert_eq!(fake.device(GAME, role), SINK);
    }
    assert!(!release_with(
        &fake,
        &mut state,
        directory.path(),
        GAME,
        id
    )?);
    for role in ROLES {
        assert_eq!(
            fake.device(GAME, role),
            "",
            "the game follows the default again"
        );
    }
    assert_eq!(load(directory.path())?, Journal::default());
    Ok(())
}

/// A session's end stops its game after every stream that plays it: the
/// game is put back while it runs, not left on the sink until it next
/// starts, and the streams letting go later change nothing.
#[test]
fn a_game_the_host_stops_is_put_back_before_it_stops() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (fake, mut state) = (Fake::default(), State::default());
    started(&fake, 7);
    fake.set(7, eConsole, HEADSET)?;
    // The session's route and its stream's.
    let Routed::Sink(first) = route_with(&fake, &mut state, directory.path(), 7, GAME, SINK)?
    else {
        anyhow::bail!("expected Routed::Sink");
    };
    let Routed::Sink(second) = route_with(&fake, &mut state, directory.path(), 7, GAME, SINK)?
    else {
        anyhow::bail!("expected Routed::Sink");
    };
    assert!(!put_back_with(
        &fake,
        &mut state,
        directory.path(),
        &[GAME.into()]
    )?);
    assert_eq!(fake.device(GAME, eConsole), HEADSET);
    assert_eq!(fake.device(GAME, eMultimedia), "");
    assert_eq!(load(directory.path())?, Journal::default());
    exited(&fake);
    started(&fake, 9);
    fake.set(9, eMultimedia, HEADSET)?;
    for id in [first, second] {
        assert!(!release_with(
            &fake,
            &mut state,
            directory.path(),
            GAME,
            id
        )?);
    }
    assert_eq!(fake.device(GAME, eMultimedia), HEADSET);
    Ok(())
}

/// A journal that can't be read is moved aside, its games kept for a
/// reset by hand, and never stops the next stream's route.
#[test]
fn an_unreadable_record_is_moved_aside() -> Result<()> {
    let directory = tempfile::tempdir()?;
    std::fs::write(
        path(directory.path()),
        b"{\"apps\": [{\"exe\": \"e:\\\\game.exe\"",
    )?;
    let (fake, mut state) = (Fake::default(), State::default());
    started(&fake, 7);
    route_with(&fake, &mut state, directory.path(), 7, GAME, SINK)?;
    assert_eq!(fake.device(GAME, eConsole), SINK);
    assert!(
        std::fs::read_to_string(directory.path().join("audio-app-recovery.unreadable.json"))?
            .contains("game.exe")
    );
    assert_eq!(load(directory.path())?.apps.len(), 1);
    Ok(())
}

/// A restore lists the processes, its slow part, without the routes' lock:
/// a stream's route never waits behind it, and a program a stream routed
/// meanwhile stays that stream's.
#[test]
fn a_restore_lists_the_processes_without_the_lock() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let fake = Fake::default();
    started(&fake, 7);
    let Routed::Sink(id) = route_with(
        &fake,
        &mut LISTED.lock().unwrap(),
        directory.path(),
        7,
        GAME,
        SINK,
    )?
    else {
        anyhow::bail!("expected Routed::Sink");
    };
    assert!(release(&mut LISTED.lock().unwrap(), GAME, id));
    let (state, restored) = restore_unlocked(&fake, &LISTED, directory.path());
    assert!(!restored?);
    assert_eq!(*fake.listed.borrow(), [true]);
    assert_eq!(fake.device(GAME, eConsole), "");
    drop(state);
    let running = fake.running(&[GAME])?;
    // Routed again between the listing and the restore: left alone.
    route_with(
        &fake,
        &mut LISTED.lock().unwrap(),
        directory.path(),
        7,
        GAME,
        SINK,
    )?;
    let state = LISTED.lock().unwrap();
    assert!(!restore_running(&fake, &state, directory.path(), &running)?);
    assert_eq!(fake.device(GAME, eConsole), SINK);
    Ok(())
}

/// Between the put-back and the game's stop no stream routes it again (a
/// child it starts as it quits); once stopped, it is routed as before.
#[test]
fn a_game_the_host_stops_is_not_routed_again_until_it_stopped() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (fake, mut state) = (Fake::default(), State::default());
    started(&fake, 7);
    let closing = Closing::mark(&mut state, vec![GAME.into()]);
    assert_eq!(
        route_with(&fake, &mut state, directory.path(), 7, GAME, SINK)?,
        Routed::Closing
    );
    assert_eq!(fake.device(GAME, eConsole), "");
    assert_eq!(load(directory.path())?, Journal::default());
    closing.unmark(&mut state);
    std::mem::forget(closing);
    assert!(matches!(
        route_with(&fake, &mut state, directory.path(), 7, GAME, SINK)?,
        Routed::Sink(_)
    ));
    Ok(())
}

/// A second unreadable record never replaces the first one's list.
#[test]
fn unreadable_records_are_all_kept() -> Result<()> {
    let directory = tempfile::tempdir()?;
    for _ in 0..2 {
        std::fs::write(path(directory.path()), b"{")?;
        load(directory.path())?;
    }
    assert!(
        directory
            .path()
            .join("audio-app-recovery.unreadable.json")
            .exists()
    );
    assert!(
        directory
            .path()
            .join("audio-app-recovery.unreadable-2.json")
            .exists()
    );
    Ok(())
}

#[test]
fn a_game_that_exited_is_put_back_when_it_next_runs() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (fake, mut state) = (Fake::default(), State::default());
    started(&fake, 7);
    fake.set(7, eConsole, HEADSET)?;
    let Routed::Sink(id) = route_with(&fake, &mut state, directory.path(), 7, GAME, SINK)? else {
        anyhow::bail!("expected Routed::Sink");
    };
    exited(&fake);
    assert!(
        release_with(&fake, &mut state, directory.path(), GAME, id)?,
        "Windows keeps the sink for the program: the restore waits for it"
    );
    assert!(restore_with(&fake, &state, directory.path())?);
    started(&fake, 9);
    assert!(!restore_with(&fake, &state, directory.path())?);
    assert_eq!(fake.device(GAME, eConsole), HEADSET);
    assert_eq!(fake.device(GAME, eMultimedia), "");
    assert_eq!(load(directory.path())?, Journal::default());
    Ok(())
}

#[test]
fn after_a_crash_the_journal_alone_puts_the_game_back() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let fake = Fake::default();
    started(&fake, 7);
    fake.set(7, eMultimedia, HEADSET)?;
    route_with(
        &fake,
        &mut State::default(),
        directory.path(),
        7,
        GAME,
        SINK,
    )?;
    // The host died: a new one starts with no streams.
    assert!(!restore_with(&fake, &State::default(), directory.path())?);
    assert_eq!(fake.device(GAME, eMultimedia), HEADSET);
    assert_eq!(fake.device(GAME, eConsole), "");
    Ok(())
}

#[test]
fn a_new_stream_before_the_restore_keeps_the_users_device() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (fake, mut state) = (Fake::default(), State::default());
    started(&fake, 7);
    fake.set(7, eConsole, HEADSET)?;
    let Routed::Sink(first) = route_with(&fake, &mut state, directory.path(), 7, GAME, SINK)?
    else {
        anyhow::bail!("expected Routed::Sink");
    };
    exited(&fake);
    assert!(release_with(
        &fake,
        &mut state,
        directory.path(),
        GAME,
        first
    )?);
    // Started for the next stream: it reads the sink as the game's device.
    started(&fake, 9);
    let Routed::Sink(second) = route_with(&fake, &mut state, directory.path(), 9, GAME, SINK)?
    else {
        anyhow::bail!("expected Routed::Sink");
    };
    assert_eq!(fake.device(GAME, eConsole), SINK);
    assert!(!release_with(
        &fake,
        &mut state,
        directory.path(),
        GAME,
        second
    )?);
    assert_eq!(fake.device(GAME, eConsole), HEADSET);
    Ok(())
}

#[test]
fn a_device_the_user_chose_during_the_stream_stays() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (fake, mut state) = (Fake::default(), State::default());
    started(&fake, 7);
    let Routed::Sink(id) = route_with(&fake, &mut state, directory.path(), 7, GAME, SINK)? else {
        anyhow::bail!("expected Routed::Sink");
    };
    fake.set(7, eConsole, HEADSET)?;
    assert!(!release_with(
        &fake,
        &mut state,
        directory.path(),
        GAME,
        id
    )?);
    assert_eq!(fake.device(GAME, eConsole), HEADSET);
    assert_eq!(fake.device(GAME, eMultimedia), "");
    Ok(())
}

#[test]
fn the_last_of_two_streams_of_one_game_puts_it_back() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (fake, mut state) = (Fake::default(), State::default());
    started(&fake, 7);
    started(&fake, 8);
    let Routed::Sink(first) = route_with(&fake, &mut state, directory.path(), 7, GAME, SINK)?
    else {
        anyhow::bail!("expected Routed::Sink");
    };
    let Routed::Sink(second) = route_with(&fake, &mut state, directory.path(), 8, GAME, SINK)?
    else {
        anyhow::bail!("expected Routed::Sink");
    };
    assert!(!release_with(
        &fake,
        &mut state,
        directory.path(),
        GAME,
        first
    )?);
    assert_eq!(
        fake.device(GAME, eConsole),
        SINK,
        "the other stream still plays it"
    );
    assert!(
        !restore_with(&fake, &state, directory.path())?,
        "the watcher leaves a playing game alone"
    );
    assert_eq!(fake.device(GAME, eConsole), SINK);
    assert!(!release_with(
        &fake,
        &mut state,
        directory.path(),
        GAME,
        second
    )?);
    assert_eq!(fake.device(GAME, eConsole), "");
    Ok(())
}

#[test]
fn a_game_found_on_the_sink_without_a_record_goes_back_to_the_default() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (fake, mut state) = (Fake::default(), State::default());
    started(&fake, 7);
    // The record was lost while the game was routed.
    fake.set(7, eConsole, &SINK.to_lowercase())?;
    let Routed::Sink(id) = route_with(&fake, &mut state, directory.path(), 7, GAME, SINK)? else {
        anyhow::bail!("expected Routed::Sink");
    };
    assert!(!release_with(
        &fake,
        &mut state,
        directory.path(),
        GAME,
        id
    )?);
    assert_eq!(fake.device(GAME, eConsole), "");
    Ok(())
}

#[test]
fn a_device_that_is_gone_does_not_block_the_restore() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (fake, mut state) = (Fake::default(), State::default());
    started(&fake, 7);
    fake.set(7, eConsole, HEADSET)?;
    let Routed::Sink(id) = route_with(&fake, &mut state, directory.path(), 7, GAME, SINK)? else {
        anyhow::bail!("expected Routed::Sink");
    };
    *fake.gone.borrow_mut() = Some(HEADSET.into());
    assert!(!release_with(
        &fake,
        &mut state,
        directory.path(),
        GAME,
        id
    )?);
    assert_eq!(fake.device(GAME, eConsole), "", "it follows the default");
    assert_eq!(load(directory.path())?, Journal::default());
    Ok(())
}

#[test]
fn a_device_the_user_chose_is_kept_when_the_game_starts_again() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (fake, mut state) = (Fake::default(), State::default());
    started(&fake, 7);
    let Routed::Sink(id) = route_with(&fake, &mut state, directory.path(), 7, GAME, SINK)? else {
        anyhow::bail!("expected Routed::Sink");
    };
    fake.set(7, eConsole, HEADSET)?;
    exited(&fake);
    assert!(release_with(&fake, &mut state, directory.path(), GAME, id)?);
    started(&fake, 9);
    assert_eq!(
        route_with(&fake, &mut state, directory.path(), 9, GAME, SINK)?,
        Routed::Users(HEADSET.into())
    );
    assert_eq!(fake.device(GAME, eConsole), HEADSET);
    // The pending entry still puts the other role back.
    assert!(!restore_with(&fake, &state, directory.path())?);
    assert_eq!(fake.device(GAME, eMultimedia), "");
    assert_eq!(fake.device(GAME, eConsole), HEADSET);
    Ok(())
}

#[test]
fn a_route_windows_does_not_keep_fails_loudly_and_leaves_nothing() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (fake, mut state) = (Fake::default(), State::default());
    started(&fake, 7);
    fake.ignores.set(true);
    let error = route_with(&fake, &mut state, directory.path(), 7, GAME, SINK).unwrap_err();
    assert!(
        format!("{error:#}").contains("not the virtual sink"),
        "{error:#}"
    );
    assert!(state.active.is_empty());
    assert_eq!(load(directory.path())?, Journal::default());
    Ok(())
}

#[test]
fn a_store_game_is_the_same_program_after_an_update() {
    let old = r"c:\program files\windowsapps\studio.game_1.0.2.0_x64__abc123\game\game.exe";
    let new = r"c:\program files\windowsapps\studio.game_1.0.3.0_x64__abc123\game\game.exe";
    let other = r"c:\program files\windowsapps\studio.other_1.0.3.0_x64__abc123\game\game.exe";
    assert!(same_program(old, new));
    assert!(!same_program(old, other));
    assert!(!same_program(
        GAME,
        r"d:\games\lego batman\legobatmanlotdk-win64-shipping.exe"
    ));
}

#[test]
fn changing_virtual_sink_does_not_mistake_previous_sink_for_a_user_device() -> Result<()> {
    const SINK_2: &str = r"\\?\SWD#MMDEVAPI#{vbcable}#{e6327cad-dcec-4949-ae8a-991e976a79d2}";
    let directory = tempfile::tempdir()?;
    let (fake, mut state) = (Fake::default(), State::default());
    started(&fake, 7);
    fake.set(7, eConsole, HEADSET)?;
    // Routed to SINK_1, then game exits while restore waits.
    let Routed::Sink(first) = route_with(&fake, &mut state, directory.path(), 7, GAME, SINK)?
    else {
        anyhow::bail!("expected Routed::Sink");
    };
    exited(&fake);
    assert!(release_with(
        &fake,
        &mut state,
        directory.path(),
        GAME,
        first
    )?);
    // User switches virtual_sink setting from SINK to SINK_2 before next stream.
    started(&fake, 9);
    let Routed::Sink(second) = route_with(&fake, &mut state, directory.path(), 9, GAME, SINK_2)?
    else {
        anyhow::bail!("expected Routed::Sink");
    };
    for role in ROLES {
        assert_eq!(fake.device(GAME, role), SINK_2);
    }
    // Teardown restores the original HEADSET, not SINK_1.
    assert!(!release_with(
        &fake,
        &mut state,
        directory.path(),
        GAME,
        second
    )?);
    assert_eq!(fake.device(GAME, eConsole), HEADSET);
    assert_eq!(fake.device(GAME, eMultimedia), "");
    Ok(())
}

#[test]
fn volume_mixer_reset_to_default_updates_before_and_routes_to_virtual_sink() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (fake, mut state) = (Fake::default(), State::default());
    started(&fake, 7);
    fake.set(7, eConsole, HEADSET)?;
    let Routed::Sink(first) = route_with(&fake, &mut state, directory.path(), 7, GAME, SINK)?
    else {
        anyhow::bail!("expected Routed::Sink");
    };
    exited(&fake);
    assert!(release_with(
        &fake,
        &mut state,
        directory.path(),
        GAME,
        first
    )?);
    // User resets game to Windows Default ("") in Volume Mixer before next stream.
    started(&fake, 9);
    fake.set(9, eConsole, "")?;
    fake.set(9, eMultimedia, "")?;
    let Routed::Sink(second) = route_with(&fake, &mut state, directory.path(), 9, GAME, SINK)?
    else {
        anyhow::bail!("expected Routed::Sink");
    };
    assert_eq!(fake.device(GAME, eConsole), SINK);
    assert_eq!(fake.device(GAME, eMultimedia), SINK);
    // Teardown restores Default ("") rather than the old HEADSET.
    assert!(!release_with(
        &fake,
        &mut state,
        directory.path(),
        GAME,
        second
    )?);
    assert_eq!(fake.device(GAME, eConsole), "");
    assert_eq!(fake.device(GAME, eMultimedia), "");
    Ok(())
}

#[test]
fn app_route_users_device_distinguishes_sink_closing_and_users() {
    let make = |routed| AppRoute {
        directory: PathBuf::new(),
        exe: GAME.into(),
        pid: 1,
        sink_id: "{steam}".into(),
        sink: "Steam".into(),
        routed,
    };
    let sink = make(Routed::Sink(1));
    let closing = make(Routed::Closing);
    let users = make(Routed::Users(HEADSET.into()));
    assert_eq!(sink.users_device(), None);
    assert_eq!(closing.users_device(), None);
    assert_eq!(users.users_device(), Some(HEADSET));
    std::mem::forget((sink, closing, users));
}

#[test]
fn stale_route_dropped_after_put_back_and_new_stream_never_restores_games_speakers() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (fake, mut state) = (Fake::default(), State::default());
    started(&fake, 7);
    fake.set(7, eConsole, HEADSET)?;
    let Routed::Sink(old_id) = route_with(&fake, &mut state, directory.path(), 7, GAME, SINK)?
    else {
        panic!("expected Routed::Sink");
    };
    assert_eq!(fake.device(GAME, eConsole), SINK);
    // Stream 1 ends / game restarts: put_back forgets old active tokens, then Stream 2 routes it.
    forget(&mut state, &[GAME.into()]);
    started(&fake, 9);
    let Routed::Sink(new_id) = route_with(&fake, &mut state, directory.path(), 9, GAME, SINK)?
    else {
        panic!("expected Routed::Sink");
    };
    assert_ne!(old_id, new_id);
    // Old AppRoute dropping after Stream 2 started must NOT decrement Stream 2 or restore speakers.
    assert!(!release_with(
        &fake,
        &mut state,
        directory.path(),
        GAME,
        old_id
    )?);
    assert_eq!(fake.device(GAME, eConsole), SINK);
    // Stream 2's own drop restores when the game next starts (or immediately if still running).
    exited(&fake);
    assert!(release_with(
        &fake,
        &mut state,
        directory.path(),
        GAME,
        new_id
    )?);
    assert_eq!(fake.device(GAME, eConsole), SINK);
    started(&fake, 11);
    assert!(!restore_with(&fake, &state, directory.path())?);
    assert_eq!(fake.device(GAME, eConsole), HEADSET);
    Ok(())
}
