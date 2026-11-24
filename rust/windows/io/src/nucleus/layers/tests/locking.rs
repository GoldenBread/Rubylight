//! Unit tests for compatibility-layer lock files, race/recovery edge cases, and registry hive access.

use super::*;

#[test]
fn a_running_session_keeps_its_values_from_the_service_and_a_check_waits() {
    let map = Map::default();
    map.values
        .lock()
        .unwrap()
        .insert(GAME.into(), text("~ WIN8RTM"));
    let folder = tempfile::tempdir().unwrap();
    let guard = take(&map, folder.path()).unwrap();
    nucleus_runs(&map, &[GAME]);
    let journal = journal(folder.path(), "S-1-5-21-7");
    let kept = std::fs::read(&journal).unwrap();
    // The user signs in again while the session runs.
    assert_eq!(at_console(&map, folder.path(), 1).unwrap(), AtConsole::Busy);
    assert_eq!(std::fs::read(&journal).unwrap(), kept);
    assert_eq!(map.values.lock().unwrap()[GAME], text("~ HIGHDPIAWARE"));
    drop(guard);
    assert_eq!(map.values.lock().unwrap()[GAME], text("~ WIN8RTM"));
    assert!(!journal.exists());

    // A check before the next session waits for the service's restore.
    host_stops(take(&map, folder.path()).unwrap());
    nucleus_runs(&map, &[GAME]);
    let held = Lock::try_take(&journal).unwrap().unwrap();
    assert!(Lock::try_take(&journal).unwrap().is_none());
    let started = Instant::now();
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        drop(held);
    });
    restore_left_in(&mut map.clone(), folder.path()).unwrap();
    assert!(started.elapsed() >= Duration::from_millis(250));
    release.join().unwrap();
    assert_eq!(map.values.lock().unwrap()[GAME], text("~ WIN8RTM"));
    assert!(!journal.exists());

    // One held past the wait is an error naming the lock.
    let _held = Lock::try_take(&journal).unwrap().unwrap();
    let error = Lock::take(&journal, Duration::from_millis(100))
        .err()
        .unwrap()
        .to_string();
    assert!(
        error.contains("Rubylight-layers-S-1-5-21-7.json"),
        "{error}"
    );
}

#[test]
fn the_lock_is_a_file_only_administrators_write_and_a_closed_holder_lets_go() {
    use std::os::windows::io::IntoRawHandle;
    let map = Map::default();
    let folder = tempfile::tempdir().unwrap();
    let journal = journal(folder.path(), "S-1-5-21-7");
    let locks = locks();
    let elevated = crate::process::elevated().unwrap();
    // One file per journal in the locks' folder, the same in every
    // process; nothing of it beside Nucleus.
    let file = locks.file(&journal);
    assert_eq!(file.parent(), Some(locks.folder.as_path()));
    assert_eq!(file.extension().unwrap(), "lock");
    let upper = PathBuf::from(journal.to_string_lossy().to_uppercase());
    assert_eq!(locks.file(&upper), file);
    assert_ne!(locks.file(&journal.with_extension("other")), file);
    let guard = take(&map, folder.path()).unwrap();
    assert!(file.is_file());
    let names = |path: &Path| {
        let mut names: Vec<_> = std::fs::read_dir(path)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    };
    assert_eq!(names(folder.path()), ["Rubylight-layers-S-1-5-21-7.json"]);
    // A second taker, on another thread as in another process, waits.
    let other = journal.clone();
    let busy = std::thread::spawn(move || Lock::try_take(&other).unwrap().is_none());
    assert!(busy.join().unwrap());
    drop(guard);
    assert!(names(folder.path()).is_empty());
    // Only SYSTEM and Administrators may write the folder and the file.
    if elevated {
        for (path, is_folder) in [(&locks.folder, true), (&file, false)] {
            let open = crate::process::open_unfollowed(path, is_folder).unwrap();
            let mut writers = crate::process::open_file_writers(&open).unwrap();
            writers.sort();
            writers.dedup();
            assert_eq!(writers, ["S-1-5-18", "S-1-5-32-544"], "{}", path.display());
        }
    } else {
        eprintln!(
            "not elevated: the lock folder's permissions are checked only in an elevated run, which can give it to SYSTEM and Administrators"
        );
    }
    // A drop lets go, and so does the holder's handle closing on another
    // thread, as Windows closes the handles of a host that stopped.
    let lock = Lock::try_take(&journal).unwrap().unwrap();
    assert!(Lock::try_take(&journal).unwrap().is_none());
    drop(lock);
    let lock = Lock::try_take(&journal).unwrap().unwrap();
    let handle = lock._file.into_raw_handle() as usize;
    std::thread::spawn(move || {
        // SAFETY: the handle is open and closed once, here.
        unsafe {
            windows::Win32::Foundation::CloseHandle(windows::Win32::Foundation::HANDLE(
                handle as *mut _,
            ))
        }
        .unwrap();
    })
    .join()
    .unwrap();
    drop(Lock::try_take(&journal).unwrap().unwrap());
    drop(take(&map, folder.path()).unwrap());

    // A folder another account owns is refused: its owner could change
    // its permissions and hold or replace the locks.
    let foreign = tempfile::tempdir().unwrap();
    let theirs = Locks::administrators(foreign.path().join(LOCK_FOLDER));
    std::fs::create_dir(&theirs.folder).unwrap();
    let owner = if elevated {
        // An administrator's folder is theirs; give it to the user.
        let user = crate::process::user_sid().unwrap();
        let status = std::process::Command::new("icacls")
            .arg(&theirs.folder)
            .args(["/setowner", &format!("*{user}"), "/Q"])
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "icacls /setowner failed");
        user
    } else {
        crate::process::user_sid().unwrap()
    };
    let error = format!(
        "{:#}",
        Lock::acquire(&theirs, &journal, Duration::ZERO)
            .err()
            .unwrap()
    );
    assert!(error.contains(&format!("is owned by {owner}")), "{error}");
    assert!(
        error.contains(&theirs.folder.display().to_string()),
        "{error}"
    );
    assert!(names(&theirs.folder).is_empty());
    // So is a link where the folder should be, never followed.
    let target = foreign.path().join("elsewhere");
    std::fs::create_dir(&target).unwrap();
    let linked = Locks {
        folder: foreign.path().join("linked"),
        ..locks
    };
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&linked.folder)
        .arg(&target)
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "mklink /J failed");
    let error = format!(
        "{:#}",
        Lock::acquire(&linked, &journal, Duration::ZERO)
            .err()
            .unwrap()
    );
    assert!(error.contains("is a link to elsewhere"), "{error}");
    assert!(names(&target).is_empty());
}

#[test]
fn the_lock_folder_is_made_with_its_permissions_never_given_them_after() {
    let temporary = tempfile::tempdir().unwrap();
    let journal = temporary.path().join("Rubylight-layers-S-1-5-21-7.json");
    // The installed lock makes its folder for SYSTEM and Administrators
    // alone in one step, which only an elevated process can: a plain
    // create, whose folder would inherit the read access of the folder
    // around it until restricted, would succeed here unelevated too.
    let installed = Locks::administrators(temporary.path().join(LOCK_FOLDER));
    match Lock::acquire(&installed, &journal, Duration::ZERO) {
        Ok(lock) => {
            assert!(crate::process::elevated().unwrap());
            assert!(lock.is_some());
            let open = crate::process::open_unfollowed(&installed.folder, true).unwrap();
            let mut writers = crate::process::open_file_writers(&open).unwrap();
            writers.sort();
            writers.dedup();
            assert_eq!(writers, ["S-1-5-18", "S-1-5-32-544"]);
        }
        Err(error) => {
            assert!(!crate::process::elevated().unwrap(), "{error:#}");
            let error = format!("{error:#}");
            assert!(error.contains("creating"), "{error}");
            assert!(!installed.folder.exists());
        }
    }
    // One there already is taken as it is, and checked.
    let mut locks = Locks {
        restrict: |_| Ok(()),
        ..installed
    };
    locks.owners.extend(crate::process::user_sid());
    if !locks.folder.exists() {
        std::fs::create_dir(&locks.folder).unwrap();
    }
    drop(
        Lock::acquire(&locks, &journal, Duration::ZERO)
            .unwrap()
            .unwrap(),
    );
}

/// Layers whose first change removes the file, as a restore elsewhere
/// would have.
struct Raced(Map, PathBuf);
impl Layers for Raced {
    fn user(&self) -> &str {
        self.0.user()
    }
    fn values(&mut self) -> Result<BTreeMap<String, Value>> {
        self.0.values()
    }
    fn set(&mut self, name: &str, value: &Value) -> Result<()> {
        let _ = std::fs::remove_file(&self.1);
        self.0.set(name, value)
    }
    fn delete(&mut self, name: &str) -> Result<()> {
        let _ = std::fs::remove_file(&self.1);
        self.0.delete(name)
    }
}

#[test]
fn a_file_removed_already_is_restored_all_the_same() {
    let map = Map::default();
    map.values
        .lock()
        .unwrap()
        .insert(GAME.into(), text("~ WIN8RTM"));
    let folder = tempfile::tempdir().unwrap();
    host_stops(take(&map, folder.path()).unwrap());
    nucleus_runs(&map, &[GAME]);
    let journal = journal(folder.path(), "S-1-5-21-7");
    restore_left_in(&mut Raced(map.clone(), journal.clone()), folder.path()).unwrap();
    assert_eq!(map.values.lock().unwrap()[GAME], text("~ WIN8RTM"));
    assert!(!journal.exists());
}

#[test]
fn an_empty_file_power_loss_left_is_nothing_to_put_back() {
    let map = Map::default();
    let folder = tempfile::tempdir().unwrap();
    let journal = journal(folder.path(), "S-1-5-21-7");
    std::fs::write(&journal, "").unwrap();
    let guard = take(&map, folder.path()).unwrap();
    let kept: Snapshot = serde_json::from_slice(&std::fs::read(&journal).unwrap()).unwrap();
    assert_eq!(kept.game.as_deref(), Some(GAME));
    drop(guard);
    std::fs::write(&journal, " \r\n").unwrap();
    restore_left_in(&mut map.clone(), folder.path()).unwrap();
    assert!(!journal.exists());
}

#[test]
fn a_hive_that_is_not_loaded_keeps_the_file() {
    // No such user is signed in; after sign-out their hive is unloaded
    // just the same.
    let sid = "S-1-5-21-1-2-3-987654";
    let mut hive = UserHive::of(sid.into());
    let error = format!("{:#}", hive.values().unwrap_err());
    assert!(error.contains("is not loaded"), "{error}");
    assert!(hive.delete(GAME).is_err());
    let folder = tempfile::tempdir().unwrap();
    let journal = journal(folder.path(), sid);
    let left = Snapshot {
        user: sid.into(),
        executable: "Puzzle.exe".into(),
        game: Some(GAME.into()),
        content: None,
        values: BTreeMap::new(),
    };
    write_journal(&journal, &left).unwrap();
    let error = format!(
        "{:#}",
        restore_left_in(&mut hive, folder.path()).unwrap_err()
    );
    assert!(error.contains("is not loaded"), "{error}");
    assert!(journal.is_file());
}

#[test]
fn values_that_cannot_be_restored_are_kept_and_block_the_next_session() {
    let map = Map::default();
    map.values
        .lock()
        .unwrap()
        .insert(GAME.into(), text("~ WIN8RTM"));
    let folder = tempfile::tempdir().unwrap();
    let guard = take(&map, folder.path()).unwrap();
    nucleus_runs(&map, &[GAME]);
    *map.broken.lock().unwrap() = true;
    drop(guard);
    let journal = journal(folder.path(), "S-1-5-21-7");
    assert!(journal.is_file());
    // No session may snapshot Nucleus' value as the user's.
    let error = take(&map, folder.path()).err().unwrap();
    let error = format!("{error:#}");
    assert!(error.contains("were not put back from"), "{error}");
    assert!(error.contains("deleting it lets sessions start"), "{error}");
    assert!(journal.is_file());
    *map.broken.lock().unwrap() = false;
    drop(take(&map, folder.path()).unwrap());
    assert_eq!(map.values.lock().unwrap()[GAME], text("~ WIN8RTM"));
    assert!(!journal.exists());
}

#[test]
fn only_the_games_exe_counts() {
    assert!(of(r"D:\A\Game.exe", "game.exe"));
    assert!(of(r"D:/A/Game.exe", r"bin\Game.exe"));
    assert!(!of(r"D:\A\MyGame.exe", "Game.exe"));
    assert!(!of(r"D:\Game.exe\Other.exe", "Game.exe"));
}

/// The signed-in user's real layers, read and a value set and removed
/// again.
#[test]
#[ignore = "writes to the signed-in user's registry"]
fn the_users_hive_reads_sets_and_removes() -> Result<()> {
    let mut hive = UserHive::signed_in()?;
    let name = r"C:\Rubylight\layers-test\Game.exe";
    let before = hive.values()?;
    hive.set(name, &text("~ HIGHDPIAWARE"))?;
    assert_eq!(hive.values()?[name], text("~ HIGHDPIAWARE"));
    hive.delete(name)?;
    hive.delete(name)?;
    assert_eq!(hive.values()?, before);
    Ok(())
}
