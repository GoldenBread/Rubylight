//! Unit tests for child-session setup record integrity and restoration.

mod retire;

use super::*;
use butterpollo_session::child_session::Registry;
use std::cell::Cell;

thread_local! {
    /// [`settings_wait`] on this test thread, when not the host's.
    pub(super) static SETTINGS_WAIT: std::cell::Cell<Option<std::time::Duration>> =
        const { std::cell::Cell::new(None) };
}

fn scratch() -> PathBuf {
    let directory =
        std::env::temp_dir().join(format!("butterpollo-child-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&directory).unwrap();
    // By its long name: the temp folder can be an 8.3 short one (CI's
    // C:\Users\RUNNER~1), which the wipe's link check refuses as leading
    // elsewhere. ProfileList and the known folders give long names.
    let long = std::fs::canonicalize(&directory).unwrap();
    let long = long.to_str().unwrap();
    PathBuf::from(long.strip_prefix(r"\\?\").unwrap_or(long))
}
/// The PC's record, in `directory`.
fn record_in(directory: &Path) -> Record {
    Record {
        path: directory.join(logic::SETUP_FOLDER).join(logic::SETUP_FILE),
        create_folder: |path| std::fs::create_dir(path),
        create_file: |path| {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
        },
        trusted: |_, _| Ok(()),
        trusted_root: |_, _| Ok(()),
        elevated: || Ok(true),
    }
}
fn write_record(record: &Record, text: &str) {
    std::fs::create_dir_all(record.folder()).unwrap();
    std::fs::write(&record.path, text).unwrap();
}
fn unopened() -> Result<native::NativeSettings> {
    panic!("Windows' settings were opened")
}

#[test]
fn restore_without_a_setup_record_changes_nothing() {
    let directory = scratch();
    let record = record_in(&directory);
    assert!(!restore_saved(&record, unopened).unwrap());
    write_record(&record, "not json");
    let error = restore_saved(&record, unopened).unwrap_err();
    assert!(
        format!("{error:#}").contains(logic::SETUP_FILE),
        "{error:#}"
    );
    assert!(record.path.is_file());
    std::fs::remove_dir_all(directory).unwrap();
}

/// The Windows settings setup changed, shared by every [`Pc`] handle as
/// one PC's are by every host on it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Machine {
    values: std::collections::BTreeMap<(String, String, String), u32>,
    child_sessions: bool,
    /// Rule ID -> enabled, all in the Remote Desktop group.
    rules: std::collections::BTreeMap<String, bool>,
    block_rules: std::collections::BTreeSet<String>,
    /// Turning child sessions on or off fails.
    child_sessions_fail: bool,
}
#[derive(Clone)]
struct Pc(std::rc::Rc<std::cell::RefCell<Machine>>);
impl Pc {
    fn new() -> Self {
        let mut machine = Machine::default();
        machine.values.insert(deny(), 1);
        machine
            .rules
            .insert("RemoteDesktop-UserMode-In-TCP".into(), true);
        Self(std::rc::Rc::new(std::cell::RefCell::new(machine)))
    }
    fn now(&self) -> Machine {
        self.0.borrow().clone()
    }
    fn open(&self) -> impl FnOnce() -> Result<Pc> + use<> {
        let pc = self.clone();
        move || Ok(pc)
    }
    fn rdp(&self) -> Option<u32> {
        self.0.borrow().values.get(&deny()).copied()
    }
}
fn deny() -> (String, String, String) {
    hive_key(
        &logic::Hive::LocalMachine,
        logic::TERMINAL_SERVER,
        "fDenyTSConnections",
    )
}
fn hive_key(hive: &logic::Hive, path: &str, name: &str) -> (String, String, String) {
    let hive = match hive {
        logic::Hive::LocalMachine => "LocalMachine".to_string(),
        logic::Hive::User(sid) => sid.clone(),
    };
    (hive, path.into(), name.into())
}
impl Registry for Pc {
    fn read_dword(&self, hive: &logic::Hive, path: &str, name: &str) -> Result<Option<u32>> {
        Ok(self
            .0
            .borrow()
            .values
            .get(&hive_key(hive, path, name))
            .copied())
    }
    fn write_dword(
        &mut self,
        hive: &logic::Hive,
        path: &str,
        name: &str,
        value: u32,
    ) -> Result<()> {
        self.0
            .borrow_mut()
            .values
            .insert(hive_key(hive, path, name), value);
        Ok(())
    }
}
impl SystemSettings for Pc {
    fn delete_value(&mut self, hive: &logic::Hive, path: &str, name: &str) -> Result<()> {
        self.0
            .borrow_mut()
            .values
            .remove(&hive_key(hive, path, name));
        Ok(())
    }
    fn child_sessions_enabled(&self) -> Result<bool> {
        Ok(self.0.borrow().child_sessions)
    }
    fn set_child_sessions(&mut self, enabled: bool) -> Result<()> {
        let mut machine = self.0.borrow_mut();
        anyhow::ensure!(!machine.child_sessions_fail, "the call failed");
        machine.child_sessions = enabled;
        Ok(())
    }
    fn block_rule_present(&self, name: &str) -> Result<bool> {
        Ok(self.0.borrow().block_rules.contains(name))
    }
    fn enable_rule(&mut self, _: &str, id: &str) -> Result<()> {
        *self
            .0
            .borrow_mut()
            .rules
            .get_mut(id)
            .context("no such rule")? = true;
        Ok(())
    }
    fn remove_rule(&mut self, name: &str) -> Result<()> {
        self.0.borrow_mut().block_rules.remove(name);
        Ok(())
    }
    fn load_user_hive(&mut self, _: &str) -> Result<bool> {
        Ok(false)
    }
    fn unload_user_hive(&mut self, _: &str) -> Result<()> {
        Ok(())
    }
}

/// What the removed setup recorded in `record` and changed on `pc`: RDP
/// on behind its block rule, the frame interval, child sessions on.
fn set_up(pc: &Pc, record: &Record) {
    let mut machine = pc.0.borrow_mut();
    let state = SetupState {
        remote_desktop_rule_ids: machine
            .rules
            .iter()
            .filter(|(_, on)| **on)
            .map(|(id, _)| id.clone())
            .collect(),
        ..recorded()
    };
    record.save(&state).unwrap();
    machine.values.insert(deny(), 0);
    machine.values.insert(
        hive_key(
            &logic::Hive::LocalMachine,
            logic::WINSTATIONS,
            "DWMFRAMEINTERVAL",
        ),
        8,
    );
    for on in machine.rules.values_mut() {
        *on = false;
    }
    machine.block_rules.insert(logic::BLOCK_RULE.into());
    machine.child_sessions = true;
}

#[test]
fn a_restore_that_stops_after_rdp_leaves_rdp_to_the_user() {
    let directory = scratch();
    let record = record_in(&directory);
    let pc = Pc::new();
    let before = pc.now();
    set_up(&pc, &record);
    // Child sessions, the last step, fail: RDP and the firewall are back, and the record
    // says so.
    pc.0.borrow_mut().child_sessions_fail = true;
    let error = format!("{:#}", restore_saved(&record, pc.open()).unwrap_err());
    assert!(error.contains("child sessions"), "{error}");
    let saved = record.load().unwrap().unwrap();
    assert!(saved.rdp_restored && saved.remote_desktop_rule_ids.is_empty());
    assert!(pc.now().block_rules.is_empty());
    // The user turns RDP on since. Restoring again finishes without writing RDP's value
    // again.
    pc.0.borrow_mut().values.insert(deny(), 0);
    pc.0.borrow_mut().child_sessions_fail = false;
    assert!(restore_saved(&record, pc.open()).unwrap());
    assert!(!record.path.exists());
    let mut expected = before;
    expected.values.insert(deny(), 0);
    assert_eq!(pc.now(), expected);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn a_record_another_account_owns_is_never_used() {
    let directory = scratch();
    let trusted = record_in(&directory);
    trusted.save(&recorded()).unwrap();
    /// [`administrators_own`]'s refusal of `path`.
    fn foreign(path: &Path) -> Result<()> {
        Err(Foreign {
            folder: path.extension().is_none(),
            problem: format!("{} is owned by S-1-5-21-9", path.display()),
        }
        .into())
    }
    let foreign_file = Record {
        trusted: |_, path| match path.extension() {
            None => Ok(()),
            Some(_) => foreign(path),
        },
        ..trusted.clone()
    };
    let foreign_folder = Record {
        trusted: |_, path| foreign(path),
        ..trusted.clone()
    };
    // A folder that can't be checked is no proof that setup never made it.
    let unchecked_folder = Record {
        trusted: |_, path| match path.extension() {
            None => bail!("reading the owner of {}: access denied", path.display()),
            Some(_) => Ok(()),
        },
        ..trusted.clone()
    };
    for record in [&foreign_file, &foreign_folder, &unchecked_folder] {
        let error = format!("{:#}", record.load().unwrap_err());
        assert!(
            error.contains("S-1-5-21-9") || error.contains("access denied"),
            "{error}"
        );
        // Restore refuses it.
        let pc = Pc::new();
        assert!(restore_saved(record, pc.open()).is_err());
        assert_eq!(pc.now(), Pc::new().now());
    }
    // A record another account wrote into setup's folder, or a folder that can't be
    // checked: RDP goes off, as after any restore that failed.
    for record in [&foreign_file, &unchecked_folder] {
        let fake = Fake::new(Pc::new());
        fake.pc.0.borrow_mut().values.insert(deny(), 0);
        let left = retire(record, &directory, &fake);
        assert_eq!((fake.rdp_offs.get(), fake.pc.rdp()), (1, Some(1)));
        assert!(
            left.problems[0].contains("Remote Desktop was turned off"),
            "{left:?}"
        );
    }
    // A folder setup never made holds no setup of Rubylight's: any user could make one in
    // a data root they can write, so RDP, the user's own, is left as it is. Still said.
    let fake = Fake::new(Pc::new());
    fake.pc.0.borrow_mut().values.insert(deny(), 0);
    let left = retire(&foreign_folder, &directory, &fake);
    assert_eq!((fake.rdp_offs.get(), fake.pc.rdp()), (0, Some(0)));
    assert!(
        left.problems[0].contains("is owned by S-1-5-21-9")
            && left.problems[0].contains("Remote Desktop is left as it is"),
        "{left:?}"
    );
    // A folder someone else made first is not saved into.
    std::fs::remove_dir_all(&directory).unwrap();
    let squatted = record_in(&directory);
    std::fs::create_dir_all(squatted.folder()).unwrap();
    let squatted = Record {
        trusted: foreign_folder.trusted,
        ..squatted
    };
    assert!(squatted.save(&recorded()).is_err());
    assert!(!squatted.path.exists());
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn this_pcs_check_tells_a_folder_setup_never_made_from_a_file() {
    // Under %TEMP%, which its user can write: never SYSTEM's and Administrators' alone,
    // whether this test runs elevated or not.
    let directory = scratch();
    let file = directory.join(logic::SETUP_FILE);
    std::fs::write(&file, "{}").unwrap();
    for (path, folder) in [(&directory, true), (&file, false)] {
        let open = butterpollo_win_io::process::open_unfollowed(path, folder).unwrap();
        match administrators_own(&open, path) {
            Ok(()) => panic!("{} passed as administrators' own", path.display()),
            Err(error) => {
                let foreign = error.downcast_ref::<Foreign>().unwrap();
                assert_eq!(foreign.folder, folder, "{error:#}");
                assert_eq!(foreign_folder(&error), folder);
            }
        }
    }
    // Checked through a context too.
    let error = administrators_own(
        &butterpollo_win_io::process::open_unfollowed(&directory, true).unwrap(),
        &directory,
    )
    .unwrap_err()
    .context("opening the record");
    assert!(foreign_folder(&error));
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn the_record_read_is_the_one_whose_owner_was_checked() {
    // Checked by its path, then read by its path, another file could take its place in
    // between. The check here swaps one in: what is read is still the file checked.
    let directory = scratch();
    let record = Record {
        trusted: |file, path| {
            if path.extension().is_none() {
                // The folder stays where it is while open.
                assert!(std::fs::rename(path, path.with_extension("moved")).is_err());
                return Ok(());
            }
            let swapped = path.with_extension("swap");
            let other = SetupState {
                rdp_port: 3391,
                ..recorded()
            };
            std::fs::write(&swapped, serde_json::to_vec(&other).unwrap()).unwrap();
            std::fs::rename(&swapped, path).unwrap();
            assert!(file.metadata().unwrap().len() > 0);
            Ok(())
        },
        ..record_in(&directory)
    };
    record.save(&recorded()).unwrap();
    assert_eq!(record.load().unwrap(), Some(recorded()));
    // The next read finds the file swapped in, through its own check.
    assert_eq!(record.load().unwrap().unwrap().rdp_port, 3391);
    std::fs::remove_dir_all(directory).unwrap();
}

thread_local! {
    static CREATED: std::cell::RefCell<Vec<PathBuf>> = const { std::cell::RefCell::new(Vec::new()) };
}
#[test]
fn the_record_is_made_for_administrators_only() {
    let directory = scratch();
    let record = Record {
        create_folder: |path| {
            CREATED.with_borrow_mut(|done| done.push(path.into()));
            (record_in(path).create_folder)(path)
        },
        create_file: |path| {
            CREATED.with_borrow_mut(|done| done.push(path.into()));
            (record_in(path).create_file)(path)
        },
        ..record_in(&directory)
    };
    record.save(&recorded()).unwrap();
    record.save(&recorded()).unwrap();
    // Made with their permissions: the data root and the folder each time (there the
    // second), every file before it takes the record's name.
    let temporary = record.path.with_extension("tmp");
    let folder = record.folder().to_path_buf();
    assert_eq!(
        CREATED.take(),
        [
            directory.clone(),
            folder.clone(),
            temporary.clone(),
            directory.clone(),
            folder,
            temporary.clone()
        ]
    );
    assert_eq!(record.load().unwrap(), Some(recorded()));
    assert!(!temporary.exists());
    // A data root that is not there is made with them too, never with its parents.
    let root = directory.join("data");
    let rooted = Record {
        create_folder: record.create_folder,
        create_file: record.create_file,
        ..record_in(&root)
    };
    rooted.save(&recorded()).unwrap();
    assert_eq!(CREATED.take()[..2], [root.clone(), rooted.folder().into()]);
    let orphan = record_in(&directory.join("gone").join("data"));
    assert!(orphan.save(&recorded()).is_err());
    assert!(!directory.join("gone").exists());
    // A folder that could not be made is not saved into.
    let unmade = Record {
        create_folder: |_| Err(ErrorKind::PermissionDenied.into()),
        ..record_in(&directory.join("unmade"))
    };
    assert!(unmade.save(&recorded()).is_err());
    assert!(!unmade.folder().exists());
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn a_data_root_administrators_do_not_own_is_never_saved_into() {
    // Its owner could change its permissions and swap the record's folder in it.
    let directory = scratch();
    let foreign = Record {
        trusted_root: |_, path| bail!("{} is owned by S-1-5-21-9", path.display()),
        ..record_in(&directory)
    };
    let error = format!("{:#}", foreign.save(&recorded()).unwrap_err());
    assert!(error.contains("S-1-5-21-9"), "{error}");
    assert!(!foreign.folder().exists());
    // A link is opened itself, never followed, and refused on this PC's check.
    let target = directory.join("elsewhere");
    std::fs::create_dir(&target).unwrap();
    let link = directory.join("linked");
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&link)
        .arg(&target)
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "mklink /J failed");
    let linked = Record {
        trusted_root: administrators_own_root,
        ..record_in(&link)
    };
    let error = format!("{:#}", linked.save(&recorded()).unwrap_err());
    assert!(error.contains("is a link to elsewhere"), "{error}");
    assert_eq!(std::fs::read_dir(&target).unwrap().count(), 0);
    // A folder: refused unless SYSTEM or Administrators own it, as this test's own does
    // only when it runs elevated, and alone may write it, which %TEMP%'s user may too.
    let own = Record {
        trusted_root: administrators_own_root,
        ..record_in(&target)
    };
    let open = butterpollo_win_io::process::open_unfollowed(&target, true).unwrap();
    let owner = butterpollo_win_io::process::open_file_owner(&open).unwrap();
    let writers = butterpollo_win_io::process::open_file_writers(&open).unwrap();
    drop(open);
    let administrators = |sid: &str| matches!(sid, "S-1-5-18" | "S-1-5-32-544");
    let error = format!("{:#}", own.save(&recorded()).unwrap_err());
    if administrators(&owner) {
        let other = writers
            .iter()
            .find(|sid| !administrators(sid.as_str()))
            .unwrap();
        assert!(
            error.contains(&format!("can be written by {other}")),
            "{error}"
        );
    } else {
        assert!(error.contains(&format!("is owned by {owner}")), "{error}");
    }
    assert!(!own.folder().exists());
    std::fs::remove_dir(&link).unwrap();
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn a_temporary_record_a_cut_short_setup_left_is_replaced() {
    // A setup cut short between making and renaming it left one: it is never written into
    // (it is replaced by one made new), and never holds off the next setup.
    let directory = scratch();
    let record = record_in(&directory);
    let temporary = record.path.with_extension("tmp");
    std::fs::create_dir_all(record.folder()).unwrap();
    std::fs::write(&temporary, "left by a setup cut short").unwrap();
    record.save(&recorded()).unwrap();
    assert!(!temporary.exists());
    assert_eq!(record.load().unwrap(), Some(recorded()));
    // Replaced once: one that can't be made new even then is a failure that says so.
    std::fs::write(&temporary, "left by a setup cut short").unwrap();
    let stuck = Record {
        create_file: |path| {
            std::fs::write(path, "made again")?;
            Err(std::io::ErrorKind::AlreadyExists.into())
        },
        ..record.clone()
    };
    let error = format!("{:#}", stuck.save(&recorded()).unwrap_err());
    assert!(
        error.contains("creating") && error.contains(".tmp"),
        "{error}"
    );
    assert_eq!(record.load().unwrap(), Some(recorded()));
    // One that can't be removed (held open, sharing nothing) is a failure that says so.
    let held = {
        use std::os::windows::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&temporary)
            .unwrap()
    };
    let error = format!("{:#}", record.save(&recorded()).unwrap_err());
    assert!(error.contains("which a setup cut short left"), "{error}");
    drop(held);
    std::fs::remove_file(&temporary).unwrap();
    record.remove().unwrap();
    // One this save made and could not write goes, so it holds off no later setup.
    let unwritable = Record {
        create_file: |path| {
            std::fs::write(path, "")?;
            File::open(path)
        },
        ..record.clone()
    };
    assert!(unwritable.save(&recorded()).is_err());
    assert!(!temporary.exists() && !record.path.exists());
    record.save(&recorded()).unwrap();
    assert_eq!(record.load().unwrap(), Some(recorded()));
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn restore_needs_administrator_rights() {
    let directory = scratch();
    let pc = Pc::new();
    let elevated = record_in(&directory);
    let before = pc.now();
    set_up(&pc, &elevated);
    let changed = pc.now();
    for unelevated in [
        Record {
            elevated: || Ok(false),
            ..elevated.clone()
        },
        Record {
            elevated: || anyhow::bail!("no token"),
            ..elevated.clone()
        },
    ] {
        let error = format!(
            "{:#}",
            restore_locked(&unelevated, |_| -> Result<bool> { panic!("restored") }).unwrap_err()
        );
        assert!(error.contains("administrator rights"), "{error}");
        assert_eq!(pc.now(), changed);
        assert!(elevated.path.is_file());
    }
    assert!(restore_locked(&elevated, |record| restore_saved(record, pc.open())).unwrap());
    assert_eq!(pc.now(), before);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn a_planted_profile_record_is_ignored() {
    // A profile folder is writable by its user; a record there would
    // make restore put back what that user wrote.
    let directory = scratch();
    let record = record_in(&directory);
    let profile = directory.join("config");
    std::fs::create_dir_all(&profile).unwrap();
    // It would turn RDP on at restore.
    let planted = SetupState {
        deny_ts_connections: Some(0),
        rdp_port: 3391,
        ..recorded()
    };
    std::fs::write(
        profile.join(logic::SETUP_FILE),
        serde_json::to_vec(&planted).unwrap(),
    )
    .unwrap();
    assert!(!record.may_exist());
    assert_eq!(record.load().unwrap(), None);
    assert!(!restore_saved(&record, unopened).unwrap());
    let fake = Fake::new(Pc::new());
    assert_eq!(retire(&record, &profile, &fake), Left::default());
    assert_eq!((fake.restores.get(), fake.pc.rdp()), (0, Some(1)));
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn the_record_is_the_pcs_not_the_profiles() {
    assert_eq!(Record::new().path, logic::setup_record());
}

fn set_up_record() -> (PathBuf, Record) {
    let directory = scratch();
    let record = record_in(&directory);
    record.save(&recorded()).unwrap();
    (directory, record)
}
fn recorded() -> SetupState {
    SetupState {
        version: 1,
        user_sid: "S-1-5-21-1".into(),
        deny_ts_connections: Some(1),
        dwm_frame_interval: None,
        suppress_when_minimized: None,
        child_sessions: false,
        remote_desktop_rule_ids: Vec::new(),
        rdp_port: 3390,
        rdp_restored: false,
    }
}

/// [`Leftovers`] over a [`Pc`], counting what [`retire`] calls; saved
/// passwords in `profiles`, wiped as on this PC.
struct Fake {
    pc: Pc,
    restores: Cell<u32>,
    /// The restore fails before it changes anything.
    restore_fails: bool,
    rdp_offs: Cell<u32>,
    unrecorded: Cell<u32>,
    profiles: Vec<PathBuf>,
    session: Option<u32>,
}
impl Fake {
    fn new(pc: Pc) -> Self {
        Self {
            pc,
            restores: Cell::new(0),
            restore_fails: false,
            rdp_offs: Cell::new(0),
            unrecorded: Cell::new(0),
            profiles: Vec::new(),
            session: None,
        }
    }
}
impl Leftovers for Fake {
    fn restore(&self, record: &Record) -> Result<bool> {
        self.restores.set(self.restores.get() + 1);
        if self.restore_fails {
            bail!("access denied");
        }
        restore_saved(record, self.pc.open())
    }
    fn rdp_off(&self) -> String {
        self.rdp_offs.set(self.rdp_offs.get() + 1);
        logic::turn_rdp_off(&mut self.pc.clone())
    }
    fn unrecorded(&self) -> Result<Unrecorded> {
        self.unrecorded.set(self.unrecorded.get() + 1);
        logic::enforce_unrecorded(&mut self.pc.clone())
    }
    fn password_folders(&self) -> (Vec<PathBuf>, Vec<String>) {
        (logic::password_folders(&self.profiles, &[]), Vec::new())
    }
    fn wipe(&self, path: &Path) -> std::io::Result<bool> {
        native::wipe_file(path)
    }
    fn remove_empty(&self, path: &Path) -> std::io::Result<()> {
        native::remove_empty_folder(path)
    }
    fn child_session(&self) -> Option<u32> {
        self.session
    }
}
