//! Tests for uninstall service handling, child-session restoration and driver notes.
mod files;

use super::*;
#[test]
fn only_rubylights_own_service_is_stopped_or_deleted() {
    let install = Path::new(r"C:\Program Files\Rubylight");
    let ours = install.join("butterpollo-service.exe");
    assert_eq!(service_steps(Some(&ours), install), (true, true));
    // The same folder as the service or the Apps entry spells it.
    let spelled = Path::new(r"c:\PROGRAM FILES\rubylight\butterpollo-service.exe");
    assert_eq!(service_steps(Some(spelled), install), (true, true));
    assert_eq!(
        service_steps(Some(&ours), Path::new(r"C:\Program Files\Rubylight\")),
        (true, true)
    );
    let beside = Path::new(r"C:\Program Files\Rubylight 2\butterpollo-service.exe");
    assert_eq!(service_steps(Some(beside), install), (true, false));
    // Rubylight's program in another folder is stopped, as before, and
    // stays registered.
    let elsewhere = Path::new(r"D:\Other\butterpollo-service.exe");
    assert_eq!(service_steps(Some(elsewhere), install), (true, false));
    // Another host's ApolloService is neither, wherever it is.
    let theirs = Path::new(r"C:\Program Files\Vibepollo\tools\sunshinesvc.exe");
    assert_eq!(service_steps(Some(theirs), install), (false, false));
    let inside = install.join("tools").join("sunshinesvc.exe");
    assert_eq!(service_steps(Some(&inside), install), (false, false));
    assert_eq!(service_steps(None, install), (false, false));
}
#[test]
fn side_by_side_removing_drivers_keeps_what_another_host_may_use() {
    assert!(SHARED_DRIVERS_KEPT.contains("gamepad"));
    assert!(SHARED_DRIVERS_KEPT.contains("libvirtualdisplay"));
    // A Windows checkout may have CRLF line ends.
    let code = include_str!("../uninstall.rs").replace("\r\n", "\n");
    assert!(code.contains(
        "if options.remove_drivers && side_by_side {\n        line(format!(\"note: {SHARED_DRIVERS_KEPT}\"));"
    ));
    assert!(code.contains(
        "} else if options.remove_drivers {\n        progress.set(\"Removing the virtual drivers…\");"
    ));
    // The record goes with the rest of the registration, or a later
    // normal installation would look side by side.
    assert!(code.contains("detect::forget_service();"));
}
fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}
#[test]
fn child_session_settings_are_restored_only_when_setup_recorded_them() {
    let install = tempfile::tempdir().unwrap();
    write(&install.path().join("butterpollo.exe"), "");
    let root = tempfile::tempdir().unwrap();
    let profile = root.path().join("config");
    assert_eq!(
        child_session_restore_pending(&profile, install.path()),
        None
    );
    // One in the profile, which its user can write, is ignored.
    write(&profile.join(CHILD_SESSION_SETUP_FILE), "{}");
    assert_eq!(
        child_session_restore_pending(&profile, install.path()),
        None
    );
    // Only the PC's record.
    let machine = child_session_record(&profile);
    assert_eq!(
        machine,
        root.path()
            .join("windows-settings")
            .join(CHILD_SESSION_SETUP_FILE)
    );
    write(&machine, "{}");
    assert_eq!(
        child_session_restore_pending(&profile, install.path()),
        Some(install.path().join("butterpollo.exe"))
    );
    std::fs::remove_file(&machine).unwrap();
    assert_eq!(
        child_session_restore_pending(&profile, install.path()),
        None
    );
    write(&machine, "{}");
    std::fs::remove_file(install.path().join("butterpollo.exe")).unwrap();
    assert_eq!(
        child_session_restore_pending(&profile, install.path()),
        None
    );
}
/// HKLM as the uninstaller sees it: RDP on, as child-session setup left it.
#[derive(Default)]
struct FakeRegistry {
    values: std::collections::BTreeMap<(String, String), u32>,
    /// Writes fail (access denied).
    denied: bool,
    /// Writes succeed without changing the value.
    stuck: bool,
    writes: usize,
}
impl FakeRegistry {
    fn rdp_on() -> Self {
        let mut registry = Self::default();
        registry.set(TERMINAL_SERVER, 0);
        registry
    }
    fn set(&mut self, path: &str, value: u32) {
        self.values
            .insert((path.into(), DENY_TS_CONNECTIONS.into()), value);
    }
    fn rdp(&self) -> Option<u32> {
        self.read_dword(TERMINAL_SERVER, DENY_TS_CONNECTIONS)
            .unwrap()
    }
}
impl MachineRegistry for FakeRegistry {
    fn read_dword(&self, path: &str, name: &str) -> Result<Option<u32>> {
        Ok(self.values.get(&(path.into(), name.into())).copied())
    }
    fn write_dword(&mut self, path: &str, name: &str, value: u32) -> Result<()> {
        self.writes += 1;
        if self.denied {
            bail!("access denied");
        }
        if !self.stuck {
            self.values.insert((path.into(), name.into()), value);
        }
        Ok(())
    }
}
/// A recorded setup with the host installed (`exe`) or not.
fn recorded(exe: bool) -> (tempfile::TempDir, tempfile::TempDir, PathBuf) {
    let install = tempfile::tempdir().unwrap();
    if exe {
        write(&install.path().join("butterpollo.exe"), "");
    }
    let root = tempfile::tempdir().unwrap();
    let profile = root.path().join("config");
    write(&child_session_record(&profile), "{}");
    (install, root, profile)
}
#[test]
fn a_restore_that_cannot_run_turns_remote_desktop_off_itself() {
    // The host is gone: nothing ran, so the uninstaller turns RDP off and reads it back.
    let (install, _root, profile) = recorded(false);
    let mut registry = FakeRegistry::rdp_on();
    let note = restore_child_session_with(
        &profile,
        install.path(),
        &mut registry,
        || panic!("a record is there"),
        |_| panic!("there is no host to run"),
    )
    .unwrap();
    assert_eq!(registry.rdp(), Some(1));
    assert!(
        note.contains("butterpollo.exe is missing")
            && note.contains("Remote Desktop was turned off (fDenyTSConnections = 1)"),
        "{note}"
    );
    // The host does not start, times out (killed) or crashes: the same.
    let (install, _root, profile) = recorded(true);
    type Outcome = fn() -> Result<(i32, String)>;
    let outcomes: [Outcome; 4] = [
        || bail!("starting butterpollo.exe: access denied"),
        || bail!("butterpollo.exe did not finish within 180 s"),
        || Ok((-1073741819, String::new())),
        // A restore that failed: the host turned RDP off already; again does no harm.
        || Ok((1, "were not restored: access denied.".into())),
    ];
    for outcome in outcomes {
        let mut registry = FakeRegistry::rdp_on();
        let note = restore_child_session_with(
            &profile,
            install.path(),
            &mut registry,
            || panic!("a record is there"),
            |program| {
                assert_eq!(program, install.path().join("butterpollo.exe"));
                outcome()
            },
        )
        .unwrap();
        assert_eq!(registry.rdp(), Some(1), "{note}");
        assert!(note.contains("Remote Desktop was turned off"), "{note}");
        assert!(!note.contains(".."), "{note}");
    }
    // Restored, or nothing to restore: RDP is left as the restore put it.
    let mut registry = FakeRegistry::rdp_on();
    assert_eq!(
        restore_child_session_with(
            &profile,
            install.path(),
            &mut registry,
            || panic!("a record is there"),
            |_| Ok((0, String::new()))
        ),
        None
    );
    let root = tempfile::tempdir().unwrap();
    assert_eq!(
        restore_child_session_with(
            &root.path().join("config"),
            install.path(),
            &mut registry,
            || Ok(false),
            |_| panic!("nothing to restore")
        ),
        None
    );
    assert_eq!((registry.rdp(), registry.writes), (Some(0), 0));
}
#[test]
fn a_partly_restored_remote_desktop_is_left_as_the_user_had_it() {
    // The host put RDP back to the user's value (on), then failed on the rest: turned
    // off now, it would stay off, as a later restore skips RDP.
    let (install, _root, profile) = recorded(true);
    let mut registry = FakeRegistry::rdp_on();
    let note = restore_child_session_with(
        &profile,
        install.path(),
        &mut registry,
        || panic!("a record is there"),
        |_| {
            Ok((
                CHILD_SESSION_RESTORE_PARTLY_EXIT,
                "were partly restored: access denied.".into(),
            ))
        },
    )
    .unwrap();
    assert_eq!((registry.rdp(), registry.writes), (Some(0), 0), "{note}");
    assert!(
        note.contains("partly restored: access denied")
            && note.contains("Remote Desktop was left as it is")
            && !note.contains("turned off"),
        "{note}"
    );
    assert!(!note.contains(".."), "{note}");
}
#[test]
fn remote_desktop_setups_block_rule_outlived_its_record_for_is_turned_off() {
    // No record, but setup's block rule is still there: RDP on is setup's, and nothing
    // restores it, so it goes off as the host's enforce_unrecorded turns it off.
    let install = tempfile::tempdir().unwrap();
    write(&install.path().join("butterpollo.exe"), "");
    let root = tempfile::tempdir().unwrap();
    let profile = root.path().join("config");
    let unrecorded = |registry: &mut FakeRegistry, block_rule: fn() -> Result<bool>| {
        restore_child_session_with(&profile, install.path(), registry, block_rule, |_| {
            panic!("there is no record to restore")
        })
    };
    let mut registry = FakeRegistry::rdp_on();
    let note = unrecorded(&mut registry, || Ok(true)).unwrap();
    assert_eq!(registry.rdp(), Some(1));
    assert!(
        note.contains("their record is gone")
            && note.contains(CHILD_SESSION_BLOCK_RULE)
            && note.contains("Remote Desktop was turned off (fDenyTSConnections = 1)"),
        "{note}"
    );
    // RDP is missing (on by default): the same.
    let mut registry = FakeRegistry::default();
    assert!(unrecorded(&mut registry, || Ok(true)).is_some());
    assert_eq!(registry.rdp(), Some(1));
    // Without the rule, RDP on is the user's own; with RDP off, nothing is open and the
    // rule is not even looked for. A rule that can't be read changes nothing.
    type BlockRule = fn() -> Result<bool>;
    let cases: [(FakeRegistry, BlockRule); 3] = [
        (FakeRegistry::rdp_on(), || Ok(false)),
        (FakeRegistry::rdp_on(), || {
            bail!("Windows Firewall is not running")
        }),
        (
            {
                let mut off = FakeRegistry::default();
                off.set(TERMINAL_SERVER, 1);
                off
            },
            || panic!("RDP is off"),
        ),
    ];
    for (mut registry, block_rule) in cases {
        let before = registry.rdp();
        assert_eq!(unrecorded(&mut registry, block_rule), None);
        assert_eq!((registry.rdp(), registry.writes), (before, 0));
    }
}
#[test]
fn a_firewall_rule_that_is_not_there_reads_as_missing() {
    // Read-only on this PC: no rule has this name.
    assert!(!system::firewall_rule_present("Rubylight test: no rule has this name (TCP)").unwrap());
}
#[test]
fn remote_desktop_that_stays_on_is_reported_loudly() {
    let (install, _root, profile) = recorded(false);
    let note = |registry: &mut FakeRegistry| {
        restore_child_session_with(
            &profile,
            install.path(),
            registry,
            || unreachable!(),
            |_| unreachable!(),
        )
        .unwrap()
    };
    let mut denied = FakeRegistry {
        denied: true,
        ..FakeRegistry::rdp_on()
    };
    let found = note(&mut denied);
    assert!(
        found.contains("Turning Remote Desktop off failed")
            && found.contains("access denied")
            && found.contains("turn it off in Windows' settings"),
        "{found}"
    );
    // Written but read back on.
    let mut stuck = FakeRegistry {
        stuck: true,
        ..FakeRegistry::rdp_on()
    };
    let found = note(&mut stuck);
    assert!(
        found.contains("Turning Remote Desktop off failed")
            && found.contains("it reads Some(0) after writing 1"),
        "{found}"
    );
    // Group policy wins over the value written.
    let mut policy = FakeRegistry::rdp_on();
    policy.set(TERMINAL_SERVICES_POLICY, 0);
    let found = note(&mut policy);
    assert_eq!(policy.rdp(), Some(1));
    assert!(
        found.contains("Turning Remote Desktop off failed") && found.contains("group policy"),
        "{found}"
    );
    // A policy that turns it off is no problem.
    let mut off = FakeRegistry::rdp_on();
    off.set(TERMINAL_SERVICES_POLICY, 1);
    assert!(note(&mut off).contains("Remote Desktop was turned off"));
}
#[test]
fn the_uninstaller_switches_remote_desktop_where_the_host_does() {
    use butterpollo_session::child_session as host;
    assert_eq!(TERMINAL_SERVER, host::TERMINAL_SERVER);
    assert_eq!(TERMINAL_SERVICES_POLICY, host::TERMINAL_SERVICES_POLICY);
    // Read-only on this PC: the value is a DWORD or absent, and a missing one reads as None.
    let value = NativeRegistry
        .read_dword(TERMINAL_SERVER, DENY_TS_CONNECTIONS)
        .unwrap();
    assert!(matches!(value, None | Some(0) | Some(1)), "{value:?}");
    assert_eq!(
        NativeRegistry
            .read_dword(r"SOFTWARE\Rubylight\No\Such\Key", "Missing")
            .unwrap(),
        None
    );
    // A value that is not a DWORD is an error, not a missing one.
    assert!(
        NativeRegistry
            .read_dword(
                r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
                "ProductName"
            )
            .is_err()
    );
}
#[test]
fn a_factory_reset_keeps_the_child_session_record_a_failed_restore_needs() {
    let root = tempfile::tempdir().unwrap();
    let profile = root.path().join("config");
    let records = vec![child_session_record(&profile)];
    let fill = || {
        for record in &records {
            write(record, "{}");
        }
        write(&profile.join("sunshine.conf"), "x");
        write(&profile.join("credentials").join("cacert.pem"), "x");
        write(&profile.join("logs").join("butterpollo.log"), "x");
        write(&root.path().join("state.json"), "x");
        write(&records[0].with_file_name("other.json"), "x");
        // Not a record: it goes with the profile.
        write(&profile.join(CHILD_SESSION_SETUP_FILE), "{}");
    };
    fill();
    assert!(remove_profile(root.path(), &records).is_empty());
    let mut kept = records.clone();
    kept.sort();
    let mut left = files_in(root.path()).unwrap();
    left.sort();
    assert_eq!(
        left, kept,
        "the records stay for a retry, everything else goes"
    );
    assert!(!profile.exists());

    // Restored, or nothing to restore: the whole profile goes.
    fill();
    assert!(remove_profile(root.path(), &[]).is_empty());
    assert!(!root.path().exists());
    assert_eq!(profile_note(root.path(), &[]), None);
}
#[test]
fn a_factory_reset_that_leaves_files_says_which_and_that_pairings_may_remain() {
    use std::os::windows::fs::OpenOptionsExt;
    let root = tempfile::tempdir().unwrap();
    let profile = root.path().join("config");
    let records = [child_session_record(&profile)];
    let key = profile.join("credentials").join("cakey.pem");
    write(&key, "x");
    write(&profile.join("sunshine.conf"), "x");
    write(&records[0], "{}");
    // An indexer or antivirus holds the key open.
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&key)
        .unwrap();
    for keep in [&records[..], &[]] {
        let left = remove_profile(root.path(), keep);
        assert_eq!(left, std::slice::from_ref(&key));
        assert!(!profile.join("sunshine.conf").exists());
        let note = profile_note(root.path(), &left).unwrap();
        assert!(note.contains("credentials, paired devices"), "{note}");
        assert!(note.contains(&key.display().to_string()), "{note}");
        assert!(note.contains(&root.path().display().to_string()), "{note}");
    }
    drop(held);
    // Long lists are cut short.
    let many: Vec<PathBuf> = (0..7).map(|n| root.path().join(n.to_string())).collect();
    let note = profile_note(root.path(), &many).unwrap();
    assert!(note.contains("2 more"), "{note}");
    assert!(!note.contains(&many[5].display().to_string()), "{note}");
}
#[test]
fn failed_driver_removals_become_notes() {
    let warning = "WARNING: [RubylightVirtualDisplay] DRIVER_WARNING: Rubylight display driver setup did not complete.\nWARNING: [RubylightVirtualDisplay] Installer best-effort driver action failed: pnputil /delete-driver oem9.inf /uninstall /force failed with exit code 5\n";
    let left = "Its device (Root\\RubylightVDisplay) or driver package may still be installed";
    assert_eq!(
        removal_note("Rubylight display", left, Ok((0, warning.into()))).unwrap(),
        "The Rubylight display driver could not be removed: pnputil /delete-driver oem9.inf /uninstall /force failed with exit code 5. Its device (Root\\RubylightVDisplay) or driver package may still be installed: remove it in Device Manager; the setup log has the details."
    );
    let note = removal_note(
        "virtual gamepad",
        "It may still be installed",
        Ok((1, String::new())),
    )
    .unwrap();
    assert!(note.contains("exited with code 1"), "{note}");
    // The native libvirtualdisplay removal's error.
    let note = removal_note(
        crate::install::LIBVIRTUALDISPLAY,
        "It may still be installed",
        Err(anyhow::anyhow!(
            "pnputil /remove-device ROOT\\DISPLAY\\0001 failed with exit code 5"
        )),
    )
    .unwrap();
    assert_eq!(
        note,
        "The old libvirtualdisplay driver could not be removed: pnputil /remove-device ROOT\\DISPLAY\\0001 failed with exit code 5. It may still be installed: remove it in Device Manager; the setup log has the details."
    );
    assert_eq!(
        removal_note(
            "virtual gamepad",
            "It may still be installed",
            Ok((0, "done\n".into()))
        ),
        None
    );
}
#[test]
fn a_removal_whose_restore_failed_has_its_own_exit_code() {
    let removed = |unrestored| Removed {
        notes: Vec::new(),
        unrestored,
    };
    assert_eq!(removed(false).exit_code(), 0);
    let code = removed(true).exit_code();
    assert_eq!(code, UNRESTORED_EXIT);
    // Not done, failed, cancelled or done-but-restart.
    assert!(![0, 1, 1223, 3010].contains(&code));
}
#[test]
fn child_session_restore_uses_the_hosts_record_and_flag() {
    assert_eq!(
        CHILD_SESSION_SETUP_FILE,
        butterpollo_session::child_session::SETUP_FILE
    );
    assert_eq!(
        CHILD_SESSION_SETUP_FOLDER,
        butterpollo_session::child_session::SETUP_FOLDER
    );
    // Beside the installed profile: the PC's record is the host's.
    assert_eq!(
        child_session_record(&profile()),
        butterpollo_session::child_session::setup_record()
    );
    assert_eq!(
        CHILD_SESSION_RESTORE_FLAG,
        butterpollo_session::child_session::RESTORE_FLAG
    );
    assert_eq!(
        CHILD_SESSION_BLOCK_RULE,
        butterpollo_session::child_session::BLOCK_RULE
    );
    assert_eq!(
        CHILD_SESSION_RESTORE_PARTLY_EXIT,
        butterpollo_session::child_session::RESTORE_PARTLY_EXIT
    );
}
