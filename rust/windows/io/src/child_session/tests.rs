//! Unit tests for child-session registry reads, firewall rule lookup/removal, and file wiping.
use super::*;
use butterpollo_session::child_session::{
    Hive, REMOTE_DESKTOP_RULES, Registry, SystemSettings, TERMINAL_SERVER,
};

#[test]
fn reading_settings_changes_nothing() {
    // Read-only: this PC's registry, child-session and firewall settings stay as they are.
    let mut settings = NativeSettings::new().unwrap();
    let value = settings
        .read_dword(&Hive::LocalMachine, TERMINAL_SERVER, "fDenyTSConnections")
        .unwrap();
    assert!(matches!(value, None | Some(0) | Some(1)));
    assert_eq!(
        settings
            .read_dword(
                &Hive::LocalMachine,
                r"SOFTWARE\Rubylight\No\Such\Key",
                "Missing",
            )
            .unwrap(),
        None
    );
    settings.child_sessions_enabled().unwrap();
    assert!(
        !settings
            .block_rule_present("Rubylight test rule that does not exist")
            .unwrap()
    );
    let session = crate::process::current_session().unwrap();
    if let Ok(sid) = crate::process::session_user_sid(session) {
        assert!(!settings.load_user_hive(&sid).unwrap(), "{sid} is loaded");
    }
    assert!(profile_folder("S-1-5-21-0-0-0-1").is_err());
}

#[test]
fn a_rule_outside_its_group_is_never_turned_on() {
    let mut settings = NativeSettings::new().unwrap();
    let error = format!(
        "{:#}",
        settings
            .enable_rule(
                "Rubylight test group that does not exist",
                "RemoteDesktop-UserMode-In-TCP"
            )
            .unwrap_err()
    );
    assert!(error.contains("is not in the group"), "{error}");
    // Nor one the group doesn't have, and nothing is looked up for
    // neither a group nor a rule.
    for (group, id, expected) in [
        (
            REMOTE_DESKTOP_RULES,
            "RubylightNoSuchRule",
            "is not in the group",
        ),
        (
            "",
            "RemoteDesktop-UserMode-In-TCP",
            "no firewall group was given",
        ),
        (REMOTE_DESKTOP_RULES, "", "no firewall rule was given"),
    ] {
        let error = format!("{:#}", settings.enable_rule(group, id).unwrap_err());
        assert!(error.contains(expected), "{error}");
    }
}

#[test]
fn a_remote_desktop_rule_is_found_by_its_id_in_its_group() {
    // Read-only: the rule is looked up and read back, never turned on.
    let (id, group) = crate::wmi::on_com_thread(
        "reading a firewall rule",
        std::time::Duration::from_secs(60),
        || {
            let wmi = Wmi::connect(FIREWALL_NAMESPACE)?;
            let persistent = persistent_store()?;
            let path = rule_in_group(
                &wmi,
                &persistent,
                REMOTE_DESKTOP_RULES,
                "remotedesktop-usermode-in-tcp",
            )?;
            let rule = wmi.get(&path)?;
            Ok((
                crate::wmi::string(&rule, "InstanceID")?,
                crate::wmi::string(&rule, "RuleGroup")?,
            ))
        },
    )
    .unwrap();
    assert_eq!(id.as_deref(), Some("RemoteDesktop-UserMode-In-TCP"));
    assert_eq!(group.as_deref(), Some(REMOTE_DESKTOP_RULES));
}

#[test]
fn every_profile_folder_and_this_users_app_data_are_readable() {
    let profiles = profile_folders().unwrap();
    assert!(!profiles.is_empty());
    assert!(profiles.iter().all(|p| p.is_absolute()), "{profiles:?}");
    // Not SYSTEM here: this account's own.
    let local = console_local_app_data().unwrap().unwrap();
    assert!(local.is_absolute() && local.is_dir(), "{}", local.display());
}

#[test]
fn a_wiped_file_is_zeroed_flushed_and_gone_and_a_missing_one_is_no_error() {
    let folder = tempfile::tempdir().unwrap();
    // By its long name: the temp folder can be an 8.3 short one (CI's
    // C:\Users\RUNNER~1), which the wipe's link check refuses as leading
    // elsewhere. ProfileList and the known folders give long names.
    let long = std::fs::canonicalize(folder.path()).unwrap();
    let long = long.to_str().unwrap();
    let long = std::path::PathBuf::from(long.strip_prefix(r"\\?\").unwrap_or(long));
    let path = long.join("child-session-password.bin");
    std::fs::write(&path, vec![7u8; 100_000]).unwrap();
    assert!(wipe_file(&path).unwrap());
    assert!(!path.exists());
    assert!(!wipe_file(&path).unwrap());
    assert!(!wipe_file(&long.join("none").join("x.bin")).unwrap());
    // A folder of that name is refused, and stays.
    std::fs::create_dir(&path).unwrap();
    assert!(wipe_file(&path).is_err());
    assert!(path.is_dir());
    // A folder goes only when it is empty.
    std::fs::write(path.join("kept"), b"x").unwrap();
    remove_empty_folder(&path).unwrap();
    assert!(path.join("kept").is_file());
    std::fs::remove_file(path.join("kept")).unwrap();
    remove_empty_folder(&path).unwrap();
    assert!(!path.exists());
    remove_empty_folder(&path).unwrap();
}

#[test]
fn a_file_behind_a_junction_is_never_written() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("elsewhere");
    std::fs::create_dir(&target).unwrap();
    std::fs::write(target.join("child-session-password.bin"), b"not yours").unwrap();
    let link = root.path().join("Rubylight");
    let made = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&link)
        .arg(&target)
        .output()
        .unwrap();
    assert!(made.status.success(), "{made:?}");
    let error = wipe_file(&link.join("child-session-password.bin")).unwrap_err();
    assert!(error.to_string().contains("through a link"), "{error}");
    assert_eq!(
        std::fs::read(target.join("child-session-password.bin")).unwrap(),
        b"not yours"
    );
    // The junction itself is a link, never removed as a folder.
    assert!(remove_empty_folder(&link).is_err());
    assert!(target.is_dir());
}

#[test]
fn remove_named_propagates_com_errors_and_checks_after_cap() {
    use std::cell::Cell;
    use windows::Win32::Foundation::{E_ACCESSDENIED, ERROR_FILE_NOT_FOUND};
    use windows::core::{Error, HRESULT};
    let not_found = || {
        Err(Error::from_hresult(HRESULT::from_win32(
            ERROR_FILE_NOT_FOUND.0,
        )))
    };
    let denied = || Err(Error::from_hresult(E_ACCESSDENIED));
    let removed = Cell::new(0usize);
    remove_all_matching("Rule (TCP)", not_found, || {
        removed.set(removed.get() + 1);
        Ok(())
    })
    .unwrap();
    assert_eq!(removed.get(), 0);
    let err = remove_all_matching("Rule (TCP)", denied, || Ok(())).unwrap_err();
    assert_eq!(err.downcast_ref::<Error>().unwrap().code(), E_ACCESSDENIED);
    // Exactly 16 duplicates: removed cleanly and verified absent.
    let left = Cell::new(16usize);
    remove_all_matching(
        "Rule (TCP)",
        || if left.get() > 0 { Ok(()) } else { not_found() },
        || {
            left.set(left.get() - 1);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(left.get(), 0);
    // More than 16 duplicates: post-cap verification fails loudly.
    let left = Cell::new(17usize);
    let err = remove_all_matching(
        "Rule (TCP)",
        || if left.get() > 0 { Ok(()) } else { not_found() },
        || {
            left.set(left.get() - 1);
            Ok(())
        },
    )
    .unwrap_err();
    assert!(err.to_string().contains("still present"), "{err}");
    assert_eq!(left.get(), 1);
    // Post-cap verification COM error propagates.
    let probes = Cell::new(0usize);
    let err = remove_all_matching(
        "Rule (TCP)",
        || {
            let n = probes.get();
            probes.set(n + 1);
            if n < MAX_DUPLICATE_RULES {
                Ok(())
            } else {
                denied()
            }
        },
        || Ok(()),
    )
    .unwrap_err();
    assert_eq!(err.downcast_ref::<Error>().unwrap().code(), E_ACCESSDENIED);
}
