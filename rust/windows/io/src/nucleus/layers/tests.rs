//! Unit tests and mock [`Layers`] harness for compatibility-layer snapshotting and restoration.

use super::*;
use std::sync::{Arc, Mutex};
use std::time::Instant;

mod locking;

/// Layers in a map, shared with the test; `broken` fails every call.
#[derive(Clone, Default)]
pub struct Map {
    pub values: Arc<Mutex<BTreeMap<String, Value>>>,
    pub broken: Arc<Mutex<bool>>,
}
impl Map {
    fn check(&self) -> Result<()> {
        anyhow::ensure!(!*self.broken.lock().unwrap(), "access denied");
        Ok(())
    }
}
impl Layers for Map {
    fn user(&self) -> &str {
        "S-1-5-21-7"
    }
    fn values(&mut self) -> Result<BTreeMap<String, Value>> {
        self.check()?;
        Ok(self.values.lock().unwrap().clone())
    }
    fn set(&mut self, name: &str, value: &Value) -> Result<()> {
        self.check()?;
        self.values
            .lock()
            .unwrap()
            .insert(name.into(), value.clone());
        Ok(())
    }
    fn delete(&mut self, name: &str) -> Result<()> {
        self.check()?;
        self.values.lock().unwrap().remove(name);
        Ok(())
    }
}

/// REG_SZ `text`, as the registry keeps it: UTF-16 and terminated.
pub fn text(text: &str) -> Value {
    Value {
        kind: 1,
        data: to_wide(text)
            .into_iter()
            .flat_map(u16::to_le_bytes)
            .collect(),
    }
}

const GAME: &str = r"D:\Games\Puzzle\Puzzle.exe";

/// The locks of this process' tests: a folder of their own, never
/// Nucleus' or the installed host's. Without administrator rights it
/// can't be given to SYSTEM and Administrators, so it stays the user's.
pub(super) fn locks() -> Locks {
    static FOLDER: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    let folder = FOLDER
        .get_or_init(|| tempfile::tempdir().unwrap())
        .path()
        .join(LOCK_FOLDER);
    let mut locks = Locks::administrators(folder);
    if !crate::process::elevated().unwrap() {
        // Only an elevated process can make Administrators the owner.
        locks.owners.extend(crate::process::user_sid());
        locks.create = |folder| std::fs::create_dir(folder);
        locks.restrict = |_| Ok(());
    }
    locks
}

/// What Nucleus does as it starts the instances: set each one's DPI mode.
fn nucleus_runs(map: &Map, paths: &[&str]) {
    for path in paths {
        map.values
            .lock()
            .unwrap()
            .insert(path.to_string(), text("~ HIGHDPIAWARE"));
    }
}

/// An instance's .exe below the content folder of the Nucleus at `folder`.
fn instance(folder: &Path) -> String {
    folder
        .join(r"content\Rubylight-puzzle\Instance0\Puzzle.exe")
        .to_string_lossy()
        .into_owned()
}

/// The host stops mid-session: Windows closes its lock, nothing more.
fn host_stops(mut guard: Guard) {
    drop(guard._lock.take());
    std::mem::forget(guard);
}

fn take(map: &Map, folder: &Path) -> Result<Guard> {
    Guard::take(Box::new(map.clone()), folder, folder, "puzzle.EXE", GAME)
}

#[test]
fn the_users_settings_come_back_exactly_and_nucleus_values_go() {
    let map = Map::default();
    let other = r"C:\Tools\Other.exe";
    let before = BTreeMap::from([
        (GAME.to_string(), text("~ RUNASADMIN WIN7RTM")),
        (other.to_string(), text("~ DPIUNAWARE")),
    ]);
    *map.values.lock().unwrap() = before.clone();
    let folder = tempfile::tempdir().unwrap();
    let mut guard = take(&map, folder.path()).unwrap();
    let journal = journal(folder.path(), "S-1-5-21-7");
    assert!(journal.is_file());
    assert!(!journal.with_extension("json.tmp").exists());
    // Nucleus replaces the game's flags and sets its link's; the user
    // changes another program meanwhile.
    nucleus_runs(&map, &[GAME, &instance(folder.path())]);
    map.values
        .lock()
        .unwrap()
        .insert(other.into(), text("~ HIGHDPIAWARE"));
    guard.restore();
    let mut expected = before;
    expected.insert(other.into(), text("~ HIGHDPIAWARE"));
    assert_eq!(*map.values.lock().unwrap(), expected);
    assert!(!journal.exists());
    // Once only: a later change is the user's.
    nucleus_runs(&map, &[GAME]);
    drop(guard);
    assert_eq!(map.values.lock().unwrap()[GAME], text("~ HIGHDPIAWARE"));
}

#[test]
fn a_users_own_copy_keeps_the_file_in_the_hosts_and_its_instances_count() {
    let map = Map::default();
    *map.values.lock().unwrap() =
        BTreeMap::from([(GAME.to_string(), text("~ RUNASADMIN WIN7RTM"))]);
    let host = tempfile::tempdir().unwrap();
    let copy = tempfile::tempdir().unwrap();
    let mut guard = Guard::take(
        Box::new(map.clone()),
        host.path(),
        copy.path(),
        "puzzle.EXE",
        GAME,
    )
    .unwrap();
    // Where the console's restore and the next session look, never in
    // the folder the user can write.
    assert!(journal(host.path(), "S-1-5-21-7").is_file());
    assert!(std::fs::read_dir(copy.path()).unwrap().next().is_none());
    // The copy's instance is the session's, the host's copy's is not.
    let other = instance(host.path());
    nucleus_runs(&map, &[GAME, &instance(copy.path()), &other]);
    guard.restore();
    assert_eq!(
        *map.values.lock().unwrap(),
        BTreeMap::from([
            (GAME.to_string(), text("~ RUNASADMIN WIN7RTM")),
            (other, text("~ HIGHDPIAWARE")),
        ])
    );
}

#[test]
fn a_programs_layers_are_found_whatever_the_case() {
    let values = BTreeMap::from([
        (
            r"D:\Games\Puzzle\PUZZLE.exe".to_string(),
            text("~ RUNASADMIN"),
        ),
        (
            r"D:\Games\Other.exe".to_string(),
            Value {
                kind: 4,
                data: vec![1, 0, 0, 0],
            },
        ),
    ]);
    assert_eq!(
        program_layers(&values, "d:/games/puzzle/puzzle.exe").as_deref(),
        Some("~ RUNASADMIN")
    );
    // Not text, or not there.
    assert_eq!(program_layers(&values, r"D:\Games\Other.exe"), None);
    assert_eq!(program_layers(&values, r"D:\Games\Puzzle.exe"), None);
}

#[test]
fn another_game_with_the_same_exe_name_keeps_what_the_user_set() {
    // Every RPG Maker game is Game.exe: the user's settings for another
    // one, made while the session ran, are theirs, even Nucleus' mode.
    let map = Map::default();
    let (old, new) = (r"E:\Old\Puzzle.exe", r"F:\New\Puzzle.exe");
    map.values
        .lock()
        .unwrap()
        .insert(old.into(), text("~ WIN7RTM"));
    let folder = tempfile::tempdir().unwrap();
    let guard = take(&map, folder.path()).unwrap();
    nucleus_runs(&map, &[GAME]);
    {
        let mut values = map.values.lock().unwrap();
        values.insert(old.into(), text("~ HIGHDPIAWARE"));
        values.insert(new.into(), text("~ RUNASADMIN"));
        // Outside the content folder, though it names it.
        values.insert(r"D:\content\Puzzle.exe".into(), text("~ HIGHDPIAWARE"));
    }
    drop(guard);
    let values = map.values.lock().unwrap();
    assert!(!values.contains_key(GAME));
    assert_eq!(values[old], text("~ HIGHDPIAWARE"));
    assert_eq!(values[new], text("~ RUNASADMIN"));
    assert_eq!(values[r"D:\content\Puzzle.exe"], text("~ HIGHDPIAWARE"));
}

#[test]
fn a_value_the_user_changed_or_removed_since_stays_as_they_left_it() {
    let map = Map::default();
    let link = r"C:\Other\Puzzle.exe";
    map.values
        .lock()
        .unwrap()
        .insert(GAME.into(), text("~ WIN7RTM"));
    let folder = tempfile::tempdir().unwrap();
    let instance = instance(folder.path());
    map.values
        .lock()
        .unwrap()
        .insert(instance.clone(), text("~ WIN8RTM"));
    let guard = take(&map, folder.path()).unwrap();
    nucleus_runs(&map, &[GAME, &instance]);
    {
        let mut values = map.values.lock().unwrap();
        values.insert(GAME.into(), text("~ RUNASADMIN"));
        values.remove(&instance);
        values.insert(link.into(), text("~ HIGHDPIAWARE"));
    }
    drop(guard);
    let values = map.values.lock().unwrap();
    assert_eq!(values[GAME], text("~ RUNASADMIN"));
    assert!(!values.contains_key(&instance));
    assert_eq!(values[link], text("~ HIGHDPIAWARE"));
}

#[test]
fn only_nucleus_dpi_mode_counts_as_nucleus() {
    assert!(nucleus_wrote(&text("~ HIGHDPIAWARE")));
    // Without the terminating zero, as some programs write it.
    let mut bare = text("~ HIGHDPIAWARE");
    bare.data.truncate(bare.data.len() - 2);
    assert!(nucleus_wrote(&bare));
    assert!(!nucleus_wrote(&text("~ HIGHDPIAWARE RUNASADMIN")));
    assert!(!nucleus_wrote(&text("~ highdpiaware")));
    assert!(!nucleus_wrote(&Value {
        kind: 2,
        data: text("~ HIGHDPIAWARE").data
    }));
    assert!(!nucleus_wrote(&Value {
        kind: 1,
        data: vec![1, 2, 3]
    }));
}

#[test]
fn a_file_an_earlier_build_wrote_restores_nucleus_values_by_file_name() {
    let map = Map::default();
    let folder = tempfile::tempdir().unwrap();
    let journal = journal(folder.path(), "S-1-5-21-7");
    std::fs::write(
            &journal,
            r#"{"user":"S-1-5-21-7","executable":"Puzzle.exe","values":{"E:\\Old\\Puzzle.exe":{"kind":1,"data":[126,0,32,0,87,0,73,0,78,0,56,0,82,0,84,0,77,0,0,0]}}}"#,
        )
        .unwrap();
    {
        let mut values = map.values.lock().unwrap();
        values.insert(r"E:\Old\Puzzle.exe".into(), text("~ HIGHDPIAWARE"));
        values.insert(r"E:\Mine\Puzzle.exe".into(), text("~ RUNASADMIN"));
    }
    restore_left_in(&mut map.clone(), folder.path()).unwrap();
    let values = map.values.lock().unwrap();
    assert_eq!(values[r"E:\Old\Puzzle.exe"], text("~ WIN8RTM"));
    assert_eq!(values[r"E:\Mine\Puzzle.exe"], text("~ RUNASADMIN"));
    assert!(!journal.exists());
}

#[test]
fn an_unreadable_file_names_itself_to_delete_and_then_sessions_start() {
    let map = Map::default();
    let folder = tempfile::tempdir().unwrap();
    let journal = journal(folder.path(), "S-1-5-21-7");
    std::fs::write(&journal, "{\"user\":").unwrap();
    let error = format!("{:#}", take(&map, folder.path()).err().unwrap());
    assert!(error.contains(&journal.display().to_string()), "{error}");
    assert!(error.contains("delete it"), "{error}");
    assert!(journal.is_file());
    std::fs::remove_file(&journal).unwrap();
    drop(take(&map, folder.path()).unwrap());
}

#[test]
fn a_game_without_settings_has_none_again_and_a_dropped_guard_restores() {
    let map = Map::default();
    let folder = tempfile::tempdir().unwrap();
    let guard = take(&map, folder.path()).unwrap();
    nucleus_runs(&map, &[GAME]);
    // A session that failed to start, or a thread that never ran.
    drop(guard);
    assert!(map.values.lock().unwrap().is_empty());
    assert!(!journal(folder.path(), "S-1-5-21-7").exists());
}

#[test]
fn values_left_by_a_host_that_stopped_come_back_before_the_next_session() {
    let map = Map::default();
    map.values
        .lock()
        .unwrap()
        .insert(GAME.into(), text("~ WIN8RTM"));
    let folder = tempfile::tempdir().unwrap();
    let guard = take(&map, folder.path()).unwrap();
    nucleus_runs(&map, &[GAME]);
    // The host stops before the session ends.
    host_stops(guard);
    let journal = journal(folder.path(), "S-1-5-21-7");
    assert!(journal.is_file());
    // The next session, of another game, restores first, then keeps its own.
    let next = Guard::take(
        Box::new(map.clone()),
        folder.path(),
        folder.path(),
        "Other.exe",
        r"C:\Other\Other.exe",
    )
    .unwrap();
    assert_eq!(map.values.lock().unwrap()[GAME], text("~ WIN8RTM"));
    let kept: Snapshot = serde_json::from_slice(&std::fs::read(&journal).unwrap()).unwrap();
    assert_eq!(kept.executable, "Other.exe");
    assert_eq!(kept.game.as_deref(), Some(r"C:\Other\Other.exe"));
    drop(next);
    assert!(!journal.exists());
}

#[test]
fn values_left_by_a_host_that_stopped_come_back_when_the_user_signs_in() {
    let map = Map::default();
    map.values
        .lock()
        .unwrap()
        .insert(GAME.into(), text("~ WIN8RTM"));
    let folder = tempfile::tempdir().unwrap();
    host_stops(take(&map, folder.path()).unwrap());
    nucleus_runs(&map, &[GAME]);
    // What the service runs as the user signs in, before any session.
    restore_left_in(&mut map.clone(), folder.path()).unwrap();
    assert_eq!(map.values.lock().unwrap()[GAME], text("~ WIN8RTM"));
    assert!(!journal(folder.path(), "S-1-5-21-7").exists());
    // Nothing left: nothing to do.
    restore_left_in(&mut map.clone(), folder.path()).unwrap();
}

/// The console's session (1) and who is signed in to it, as Windows
/// tells the service in session 0.
fn at_console(map: &Map, folder: &Path, console: u32) -> Result<AtConsole> {
    restore_console_with(
        folder,
        console,
        |session| {
            anyhow::ensure!(session == 1, "no token");
            Ok("S-1-5-21-7".into())
        },
        |sid| {
            assert_eq!(sid, "S-1-5-21-7");
            Box::new(map.clone())
        },
    )
}

#[test]
fn the_service_puts_back_what_the_user_at_the_console_left() {
    let map = Map::default();
    map.values
        .lock()
        .unwrap()
        .insert(GAME.into(), text("~ WIN8RTM"));
    let folder = tempfile::tempdir().unwrap();
    host_stops(take(&map, folder.path()).unwrap());
    nucleus_runs(&map, &[GAME]);
    let journal = journal(folder.path(), "S-1-5-21-7");
    // Nobody at the console, or a session nobody signed in to: the file
    // waits, and the service says so.
    for console in [u32::MAX, 2] {
        match at_console(&map, folder.path(), console).unwrap() {
            AtConsole::NoUser { why, waiting } => {
                assert!(!why.is_empty());
                assert_eq!(waiting, std::slice::from_ref(&journal));
            }
            other => panic!("{other:?}"),
        }
        assert!(journal.is_file());
        assert_eq!(map.values.lock().unwrap()[GAME], text("~ HIGHDPIAWARE"));
    }
    // The user signs in at the console.
    assert_eq!(at_console(&map, folder.path(), 1).unwrap(), AtConsole::Done);
    assert_eq!(map.values.lock().unwrap()[GAME], text("~ WIN8RTM"));
    assert!(!journal.exists());
    assert_eq!(at_console(&map, folder.path(), 1).unwrap(), AtConsole::Done);
    assert_eq!(
        at_console(&map, folder.path(), u32::MAX).unwrap(),
        AtConsole::NoUser {
            why: "no session is attached to the console".into(),
            waiting: Vec::new()
        }
    );
}

#[test]
fn leaving_a_nucleus_puts_back_what_its_sessions_left_or_says_why_not() {
    let map = Map::default();
    map.values
        .lock()
        .unwrap()
        .insert(GAME.into(), text("~ WIN8RTM"));
    let folder = tempfile::tempdir().unwrap();
    let settings = folder.path().join("Settings.ini");
    std::fs::write(&settings, b"[Dev]\r\nShowFavoriteOnly=True\r\n").unwrap();
    let leave = |console: u32| {
        leave_with(
            folder.path(),
            console,
            |session| {
                anyhow::ensure!(session == 1, "no token");
                Ok("S-1-5-21-7".into())
            },
            |_| Box::new(map.clone()),
        )
    };
    let favourites = || {
        let text = std::fs::read(&settings).unwrap();
        butterpollo_session::nucleus::profile::ini_value(&text, "Dev", "ShowFavoriteOnly").unwrap()
    };
    // A session runs from it: nothing is put back under it.
    let mut forced = crate::nucleus::files::write_settings(folder.path()).unwrap();
    let guard = take(&map, folder.path()).unwrap();
    nucleus_runs(&map, &[GAME]);
    let error = format!("{:#}", leave(1).unwrap_err());
    assert!(
        error.contains("A Nucleus Co-op session runs from") && error.contains("end it"),
        "{error}"
    );
    assert_eq!(favourites(), b"False");
    // The host stopped with it running, and nobody is at the console now:
    // Settings.ini is put back, the user's file waits and is named.
    forced._lock = None;
    std::mem::forget(forced);
    host_stops(guard);
    let journal = journal(folder.path(), "S-1-5-21-7");
    let error = format!("{:#}", leave(u32::MAX).unwrap_err());
    assert!(
        error.contains(&format!(
            "{} keeps the compatibility settings",
            journal.display()
        )) && error.contains("sign in to this PC as that account"),
        "{error}"
    );
    assert_eq!(favourites(), b"True");
    assert!(!folder.path().join("Rubylight-settings.json").exists());
    assert_eq!(map.values.lock().unwrap()[GAME], text("~ HIGHDPIAWARE"));
    // Signed in at the console: theirs come back too, and nothing is left.
    leave(1).unwrap();
    assert_eq!(map.values.lock().unwrap()[GAME], text("~ WIN8RTM"));
    assert!(!journal.exists());
    leave(u32::MAX).unwrap();
}
