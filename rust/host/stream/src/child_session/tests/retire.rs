//! Unit tests for child-session settings locks, fail-closed restore, and retirement.

use super::super::*;
use super::*;

/// The settings locks as this process holds them, or only the
/// cross-process one, as the uninstaller or another host holds it.
type Hold = fn() -> Box<dyn std::any::Any>;
const HOLDERS: [Hold; 2] = [
    || Box::new(settings_lock().unwrap()),
    || Box::new(native::lock::setup_lock(settings_wait()).unwrap()),
];
/// `run`, on another thread, signals only after this test lets go of what
/// `hold` took.
fn waits_for_the_settings_lock<T: Send + 'static>(
    hold: Hold,
    run: impl FnOnce(std::sync::mpsc::Sender<()>) -> T + Send + 'static,
) -> T {
    let settings = hold();
    let (reached, reach) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || run(reached));
    assert!(
        reach
            .recv_timeout(std::time::Duration::from_millis(200))
            .is_err(),
        "ran while the settings lock was held"
    );
    drop(settings);
    reach
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    thread.join().unwrap()
}

#[test]
fn restore_and_the_unrecorded_check_wait_for_the_settings_lock() {
    let directory = scratch();
    let record = record_in(&directory);
    for hold in HOLDERS {
        // The uninstaller's restore.
        let config = record.clone();
        record.save(&recorded()).unwrap();
        let code = waits_for_the_settings_lock(hold, move |reached| {
            restore_exit_code_with(
                &config,
                |_| {
                    reached.send(()).unwrap();
                    Ok(true)
                },
                || panic!("restored"),
            )
        });
        assert_eq!(code, 0);
        // The host's restore.
        let config = record.clone();
        let restored = waits_for_the_settings_lock(hold, move |reached| {
            restore_locked(&config, |_| {
                reached.send(()).unwrap();
                Ok(false)
            })
        });
        assert!(!restored.unwrap());
        // The check without a record.
        record.remove().unwrap();
        let config = record.clone();
        let found = waits_for_the_settings_lock(hold, move |reached| {
            secure_unrecorded(&config, || {
                reached.send(()).unwrap();
                Ok(Unrecorded::default())
            })
        });
        assert_eq!(found.unwrap(), Unrecorded::default());
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn the_uninstallers_restore_fails_closed() {
    use std::sync::atomic::{AtomicU32, Ordering::SeqCst};
    let off = AtomicU32::new(0);
    let rdp_off = || {
        off.fetch_add(1, SeqCst);
        "Remote Desktop was turned off (fDenyTSConnections = 1), so it is not open to the network"
            .to_string()
    };
    // Setup never ran: nothing to restore, RDP left as the user has it.
    let empty = scratch();
    let never = |_: &Record| -> Result<bool> { panic!("nothing to restore") };
    assert_eq!(
        restore_exit_code_with(&record_in(&empty), never, rdp_off),
        0
    );
    assert_eq!(off.load(SeqCst), 0);
    std::fs::remove_dir_all(empty).unwrap();
    // A restore that fails turns RDP off.
    let (directory, record) = set_up_record();
    let failed = |_: &Record| -> Result<bool> { bail!("access denied") };
    assert_eq!(restore_exit_code_with(&record, failed, rdp_off), 1);
    assert_eq!(off.load(SeqCst), 1);
    // So does a settings lock another process keeps.
    let lock = native::lock::setup_lock(settings_wait()).unwrap();
    let code = std::thread::scope(|scope| {
        scope
            .spawn(|| {
                SETTINGS_WAIT.set(Some(std::time::Duration::from_millis(200)));
                restore_exit_code_with(&record, |_| panic!("restored without the lock"), rdp_off)
            })
            .join()
            .unwrap()
    });
    drop(lock);
    assert_eq!(code, 1);
    assert_eq!(off.load(SeqCst), 2);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn a_partly_restored_remote_desktop_is_left_as_the_user_had_it() {
    use std::sync::atomic::{AtomicU32, Ordering::SeqCst};
    let off = AtomicU32::new(0);
    let rdp_off = || {
        off.fetch_add(1, SeqCst);
        "Remote Desktop was turned off (fDenyTSConnections = 1), so it is not open to the network"
            .to_string()
    };
    // This restore puts RDP back, then fails on the rest: RDP is the user's own again.
    let (directory, record) = set_up_record();
    let partly = |record: &Record| -> Result<bool> {
        let mut state = record.load()?.unwrap();
        state.rdp_restored = true;
        record.save(&state)?;
        bail!("child sessions could not be turned off: access denied")
    };
    assert_eq!(
        restore_exit_code_with(&record, partly, rdp_off),
        logic::RESTORE_PARTLY_EXIT
    );
    assert_eq!(off.load(SeqCst), 0);
    // An earlier restore put it back; this one fails before the lock or the record.
    let failed = |_: &Record| -> Result<bool> { bail!("access denied") };
    assert_eq!(
        restore_exit_code_with(&record, failed, rdp_off),
        logic::RESTORE_PARTLY_EXIT
    );
    assert_eq!(off.load(SeqCst), 0);
    // Without that mark it still fails closed.
    let mut state = record.load().unwrap().unwrap();
    state.rdp_restored = false;
    record.save(&state).unwrap();
    assert_eq!(restore_exit_code_with(&record, failed, rdp_off), 1);
    assert_eq!(off.load(SeqCst), 1);
    assert_ne!(logic::RESTORE_PARTLY_EXIT, 1);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn the_host_restores_a_set_up_pc_at_its_start_once() {
    let directory = scratch();
    let record = record_in(&directory);
    let fake = Fake::new(Pc::new());
    let before = fake.pc.now();
    set_up(&fake.pc, &record);
    let left = retire(&record, &directory, &fake);
    assert_eq!(left, Left::default());
    assert_eq!(fake.pc.now(), before);
    assert!(!record.path.exists());
    assert_eq!(
        status_with(&record, &left),
        json!({"pending": false, "problem": null})
    );
    // The next start finds nothing to do.
    let fake = Fake {
        unrecorded: Cell::new(0),
        ..fake
    };
    assert_eq!(retire(&record, &directory, &fake), Left::default());
    assert_eq!((fake.restores.get(), fake.unrecorded.get()), (1, 1));
    assert_eq!(fake.pc.now(), before);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn a_restore_that_fails_at_the_start_turns_rdp_off_and_keeps_the_record() {
    let (directory, record) = set_up_record();
    let fake = Fake {
        restore_fails: true,
        ..Fake::new(Pc::new())
    };
    fake.pc.0.borrow_mut().values.insert(deny(), 0);
    let left = retire(&record, &directory, &fake);
    assert_eq!((fake.restores.get(), fake.rdp_offs.get()), (1, 1));
    assert_eq!(fake.pc.rdp(), Some(1));
    assert!(record.path.is_file());
    let problem = &left.problems[0];
    assert!(
        problem.starts_with("Child sessions were removed from Rubylight, and the Windows settings their setup changed were not restored: access denied. Remote Desktop was turned off")
            && problem.ends_with("Restore again under Maintenance, or run butterpollo.exe --child-session-restore as administrator."),
        "{problem}"
    );
    let status = status_with(&record, &left);
    assert_eq!(
        (&status["pending"], &status["problem"]),
        (&json!(true), &json!(problem))
    );
    // RDP back as before setup already: it stays the user's own.
    let mut state = record.load().unwrap().unwrap();
    state.rdp_restored = true;
    record.save(&state).unwrap();
    let fake = Fake {
        restore_fails: true,
        ..Fake::new(Pc::new())
    };
    fake.pc.0.borrow_mut().values.insert(deny(), 0);
    let left = retire(&record, &directory, &fake);
    assert_eq!((fake.rdp_offs.get(), fake.pc.rdp()), (0, Some(0)));
    assert!(
        left.problems[0].contains("Remote Desktop was left as it is"),
        "{left:?}"
    );
    assert_eq!(
        restore_exit_code_with(
            &record,
            |_| -> Result<bool> { bail!("access denied") },
            || panic!("RDP off")
        ),
        logic::RESTORE_PARTLY_EXIT
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn a_host_without_administrator_rights_changes_nothing_and_says_so_once() {
    let (directory, record) = set_up_record();
    let unelevated = Record {
        elevated: || Ok(false),
        ..record.clone()
    };
    let profile = directory.join(CHILD_PROFILE);
    std::fs::create_dir_all(&profile).unwrap();
    std::fs::write(profile.join("sunshine.conf"), "child_session_host = true\n").unwrap();
    let fake = Fake::new(Pc::new());
    fake.pc.0.borrow_mut().values.insert(deny(), 0);
    let before = fake.pc.now();
    let left = retire(&unelevated, &directory, &fake);
    assert_eq!(left.problems, [NOT_ELEVATED]);
    // Maintenance shows it under the title "Windows settings ... are
    // still changed": the text doesn't say that again.
    assert!(!NOT_ELEVATED.contains("still changed"), "{NOT_ELEVATED}");
    assert_eq!(
        (
            fake.restores.get(),
            fake.rdp_offs.get(),
            fake.unrecorded.get()
        ),
        (0, 0, 0)
    );
    assert_eq!(fake.pc.now(), before);
    assert!(record.path.is_file() && profile.is_dir());
    // Never set up: nothing to say.
    record.remove().unwrap();
    assert_eq!(retire(&unelevated, &directory, &fake), Left::default());
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn without_a_record_setups_traces_are_checked_once() {
    let directory = scratch();
    let record = record_in(&directory);
    // Set up, then the record went: RDP goes off, then the block rule.
    let fake = Fake::new(Pc::new());
    set_up(&fake.pc, &record);
    record.remove().unwrap();
    let left = retire(&record, &directory, &fake);
    assert_eq!(left, Left::default());
    assert_eq!((fake.unrecorded.get(), fake.restores.get()), (1, 0));
    assert_eq!(fake.pc.rdp(), Some(1));
    assert!(fake.pc.now().block_rules.is_empty());
    // A block rule that stays (RDP can't be turned off) is pending, and said.
    let fake = Fake::new(Pc::new());
    set_up(&fake.pc, &record);
    record.remove().unwrap();
    fake.pc.0.borrow_mut().values.insert(
        hive_key(
            &logic::Hive::LocalMachine,
            logic::TERMINAL_SERVICES_POLICY,
            "fDenyTSConnections",
        ),
        0,
    );
    let left = retire(&record, &directory, &fake);
    assert!(left.block_rule, "{left:?}");
    assert!(left.problems[0].contains("is still on this PC"), "{left:?}");
    assert_eq!(status_with(&record, &left)["pending"], json!(true));
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn the_saved_passwords_go_from_every_profile() {
    let directory = scratch();
    let profiles = [directory.join("ramaz"), directory.join("guest")];
    let folder = |profile: &Path| {
        profile
            .join("AppData")
            .join("Local")
            .join(logic::PASSWORD_FOLDER)
    };
    for profile in &profiles {
        std::fs::create_dir_all(folder(profile)).unwrap();
        for name in logic::PASSWORD_FILES {
            std::fs::write(folder(profile).join(name), "DPAPI blob").unwrap();
        }
    }
    // The guest's folder holds something else too, and stays.
    std::fs::write(folder(&profiles[1]).join("other.txt"), "kept").unwrap();
    // A profile with no saved password is no failure; a relative one, and one through `..`,
    // are skipped.
    let empty = directory.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let kept = directory.join("kept");
    std::fs::create_dir_all(folder(&kept)).unwrap();
    std::fs::write(folder(&kept).join(logic::PASSWORD_FILES[0]), "DPAPI blob").unwrap();
    let fake = Fake {
        profiles: vec![
            profiles[0].clone(),
            profiles[1].clone(),
            empty,
            PathBuf::from("relative"),
            directory.join("x").join("..").join("kept"),
        ],
        ..Fake::new(Pc::new())
    };
    assert!(delete_saved_passwords(&fake).is_empty());
    assert!(!folder(&profiles[0]).exists());
    assert_eq!(
        std::fs::read_dir(folder(&profiles[1])).unwrap().count(),
        1,
        "only other.txt stays"
    );
    assert!(folder(&kept).join(logic::PASSWORD_FILES[0]).is_file());
    // A file that can't be deleted (held open) is said with its path; the others go.
    for profile in &profiles {
        std::fs::create_dir_all(folder(profile)).unwrap();
        for name in logic::PASSWORD_FILES {
            std::fs::write(folder(profile).join(name), "DPAPI blob").unwrap();
        }
    }
    let held_path = folder(&profiles[0]).join(logic::PASSWORD_FILES[0]);
    let held = {
        use std::os::windows::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&held_path)
            .unwrap()
    };
    let problems = delete_saved_passwords(&fake);
    drop(held);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].contains(&held_path.display().to_string())
            && problems[0].contains("delete it by hand"),
        "{problems:?}"
    );
    assert!(!folder(&profiles[0]).join(logic::PASSWORD_FILES[1]).exists());
    for name in logic::PASSWORD_FILES {
        assert!(!folder(&profiles[1]).join(name).exists());
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn the_child_hosts_profile_goes_only_when_it_is_that_hosts() {
    let directory = scratch();
    let profile = directory.join(CHILD_PROFILE);
    assert_eq!(remove_child_profile(&directory), Ok(false));
    std::fs::create_dir_all(&profile).unwrap();
    std::fs::write(profile.join("sunshine.conf"), "port = 47989\n").unwrap();
    assert_eq!(remove_child_profile(&directory), Ok(false));
    assert!(profile.is_dir());
    std::fs::write(
        profile.join("sunshine.conf"),
        "port = 47999\n child_session_host = True \n",
    )
    .unwrap();
    std::fs::write(
        profile.join("sunshine_state.json"),
        r#"{"username":"admin"}"#,
    )
    .unwrap();
    assert_eq!(remove_child_profile(&directory), Ok(true));
    assert!(!profile.exists());
    std::fs::remove_dir_all(directory).unwrap();
}
