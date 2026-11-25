//! Crash recovery, halfway swap, folder pinning, and multi-session restore tests for managed game files.
// Links, held files and half-done swaps are Windows behaviour; those tests run there only.
use super::super::{Change, Journal, MAX_TEXT, Report, Scope, Unusable, move_in, recover};
#[cfg(windows)]
use super::super::{Target, linked, pin, replace};
#[cfg(windows)]
use super::HALFWAY;
use super::{OPTISCALER, apply, current, folders, game, limit, no_user, restore, targets};
use std::{collections::BTreeSet, path::PathBuf};

#[test]
fn a_host_start_after_a_crash_puts_back_what_the_journal_holds() {
    let game = game();
    let folders = folders([game.folder.clone()]);
    let targets = targets(
        &[limit("{stream_fps-1}")],
        Some(120),
        Some(&game.folder),
        &folders,
        &mut Report::default(),
    );
    apply(&game.journal, "phone", &targets).unwrap();
    assert_eq!(current(&game.file), "119");
    // The host stops without a restore; the next start restores everything journaled.
    let report = restore(&game.journal, Scope::All).unwrap();
    assert_eq!(report.restored.len(), 1);
    assert_eq!(std::fs::read_to_string(&game.file).unwrap(), OPTISCALER);
    // A crash after the journal and before the write leaves the file as it is.
    let mut journal = Journal::default();
    journal.changes.push(Change {
        session: "phone".into(),
        path: game.resolved.clone(),
        folder: PathBuf::new(),
        section: "Framerate".into(),
        key: "FramerateLimit".into(),
        original: "30".into(),
        written: "120".into(),
    });
    journal.save(&game.journal).unwrap();
    let report = restore(&game.journal, Scope::All).unwrap();
    assert!(report.restored.is_empty() && report.kept.len() == 1);
    assert_eq!(std::fs::read_to_string(&game.file).unwrap(), OPTISCALER);
    // A file gone since drops its changes.
    journal.save(&game.journal).unwrap();
    std::fs::remove_file(&game.file).unwrap();
    let report = restore(&game.journal, Scope::All).unwrap();
    assert_eq!(report.skipped.len(), 1);
    assert!(Journal::load(&game.journal).unwrap().changes.is_empty());
    // So does a game folder deleted with it, on a drive that is still there.
    journal.changes[0].path = game
        .resolved
        .with_file_name("Deleted")
        .join("OptiScaler.ini");
    journal.changes[0].folder = game.resolved.with_file_name("Deleted");
    journal.save(&game.journal).unwrap();
    let report = restore(&game.journal, Scope::All).unwrap();
    assert_eq!(report.skipped.len(), 1, "{report:?}");
    assert!(Journal::load(&game.journal).unwrap().changes.is_empty());
    // And an exe folder deleted with the game around it (`Missing\Game\Binaries\Win64`).
    let win64 = game
        .resolved
        .with_file_name("Missing")
        .join("Game")
        .join("Binaries")
        .join("Win64");
    journal.changes[0].path = win64.join("OptiScaler.ini");
    journal.changes[0].folder = win64;
    journal.save(&game.journal).unwrap();
    let report = restore(&game.journal, Scope::All).unwrap();
    assert_eq!(report.skipped.len(), 1, "{report:?}");
    assert!(Journal::load(&game.journal).unwrap().changes.is_empty());
    // A drive that is not there (not mounted yet) keeps them for a later restore.
    #[cfg(windows)]
    if let Some(drive) = ('D'..='Z')
        .rev()
        .map(|letter| PathBuf::from(format!(r"\\?\{letter}:\")))
        .find(|drive| !drive.exists())
    {
        journal.changes[0].path = drive.join("Games").join("OptiScaler.ini");
        journal.changes[0].folder = drive.join("Games");
        journal.save(&game.journal).unwrap();
        let report = restore(&game.journal, Scope::All).unwrap();
        assert_eq!(report.failed.len(), 1, "{report:?}");
        assert_eq!(
            Journal::load(&game.journal).unwrap().changes,
            journal.changes
        );
    }
}

#[test]
fn a_file_a_failed_write_left_under_its_temporary_name_goes_back() {
    let game = game();
    let folders = folders([game.folder.clone()]);
    let targets = targets(
        &[limit("120")],
        None,
        Some(&game.folder),
        &folders,
        &mut Report::default(),
    );
    apply(&game.journal, "phone", &targets).unwrap();
    // The swap failed halfway and the move after it too: only the temporary file is left.
    let temporary = game
        .resolved
        .with_file_name(format!(".OptiScaler.ini.{}.tmp", uuid::Uuid::new_v4()));
    std::fs::rename(&game.resolved, &temporary).unwrap();
    let report = restore(&game.journal, Scope::All).unwrap();
    assert_eq!(report.restored.len(), 1, "{report:?}");
    assert_eq!(std::fs::read_to_string(&game.file).unwrap(), OPTISCALER);
    assert!(!temporary.exists());
    assert!(Journal::load(&game.journal).unwrap().changes.is_empty());
}

#[test]
fn a_swap_that_failed_halfway_moves_the_new_file_in_and_no_other_failure_makes_one() {
    let game = game();
    let temporary = game.folder.join(".OptiScaler.ini.halfway.tmp");
    // ERROR_UNABLE_TO_MOVE_REPLACEMENT: the file is gone, its new contents are beside it.
    std::fs::remove_file(&game.file).unwrap();
    std::fs::write(&temporary, "[Framerate]\r\nFramerateLimit=120\r\n").unwrap();
    let halfway = recover(
        &game.file,
        &temporary,
        std::io::Error::from_raw_os_error(1176),
    );
    if cfg!(windows) {
        halfway.unwrap();
        assert_eq!(current(&game.file), "120");
        assert!(!temporary.exists());
    } else {
        assert!(halfway.is_err());
    }
    // A file that turns up meanwhile (the game wrote one) is not overwritten.
    std::fs::write(&game.file, "[Framerate]\r\nFramerateLimit=120\r\n").unwrap();
    std::fs::write(&temporary, "new").unwrap();
    assert!(move_in(&temporary, &game.file).is_err());
    assert_eq!(current(&game.file), "120");
    assert!(
        recover(
            &game.file,
            &temporary,
            std::io::Error::from_raw_os_error(1176)
        )
        .is_err()
    );
    assert_eq!(current(&game.file), "120");
    assert!(!temporary.exists());
    // Any other failure leaves no new file, and no temporary one.
    std::fs::remove_file(&game.file).ok();
    std::fs::write(&temporary, "new").unwrap();
    assert!(recover(&game.file, &temporary, std::io::Error::from_raw_os_error(5)).is_err());
    assert!(!game.file.exists() && !temporary.exists());
    // A file the user deleted while it was written stays deleted.
    #[cfg(windows)]
    assert!(replace(&game.resolved, b"[Framerate]\r\nFramerateLimit=120\r\n").is_err());
    assert!(!game.file.exists());
    assert_eq!(std::fs::read_dir(&game.folder).unwrap().count(), 0);
}

#[test]
#[cfg(windows)]
fn a_write_whose_new_file_is_left_beside_it_stays_journaled_and_goes_back() {
    let game = game();
    let folders = folders([game.folder.clone()]);
    let targets = targets(
        &[limit("120")],
        None,
        Some(&game.folder),
        &folders,
        &mut Report::default(),
    );
    HALFWAY.set(true);
    let report = apply(&game.journal, "phone", &targets);
    HALFWAY.set(false);
    let report = report.unwrap();
    assert!(
        report.written.is_empty() && report.skipped.len() == 1,
        "{report:?}"
    );
    assert!(!game.file.exists());
    assert_eq!(Journal::load(&game.journal).unwrap().changes.len(), 1);
    let report = restore(&game.journal, Scope::All).unwrap();
    assert_eq!(report.restored.len(), 1, "{report:?}");
    assert_eq!(std::fs::read_to_string(&game.file).unwrap(), OPTISCALER);
    assert_eq!(std::fs::read_dir(&game.folder).unwrap().count(), 1);
    assert!(Journal::load(&game.journal).unwrap().changes.is_empty());
}

#[test]
fn a_restore_that_changes_nothing_leaves_the_journal_alone() {
    let game = game();
    let folders = folders([game.folder.clone()]);
    let targets = targets(
        &[limit("60")],
        None,
        Some(&game.folder),
        &folders,
        &mut Report::default(),
    );
    apply(&game.journal, "phone", &targets).unwrap();
    let mut permissions = std::fs::metadata(&game.journal).unwrap().permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&game.journal, permissions.clone()).unwrap();
    // At the sign-in screen nothing goes back, and the journal is not written again.
    let report = super::super::restore(&game.journal, Scope::All, no_user).unwrap();
    assert_eq!(report.failed.len(), 1, "{report:?}");
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    std::fs::set_permissions(&game.journal, permissions).unwrap();
    restore(&game.journal, Scope::All).unwrap();
    assert_eq!(std::fs::read_to_string(&game.file).unwrap(), OPTISCALER);
}

#[test]
#[cfg(windows)]
fn the_folders_down_to_a_file_are_held_while_it_is_written() {
    let game = game();
    let sub = game.folder.join("Binaries");
    std::fs::create_dir(&sub).unwrap();
    let file = std::fs::canonicalize(&sub).unwrap().join("OptiScaler.ini");
    std::fs::write(&file, OPTISCALER).unwrap();
    let folder = std::fs::canonicalize(&game.folder).unwrap();
    let pins = pin(&folder, &file).unwrap();
    assert_eq!(pins.len(), 2);
    // Neither the app's folder nor the one below it can move, so neither can be swapped.
    assert!(std::fs::rename(&sub, game.folder.join("Moved")).is_err());
    assert!(std::fs::rename(&game.folder, game.folder.with_file_name("Moved")).is_err());
    drop(pins);
    std::fs::rename(&sub, game.folder.join("Moved")).unwrap();
    // A folder swapped for a junction before the write is refused, and the write with it.
    let made = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&sub)
        .arg(game.folder.join("Moved"))
        .output()
        .unwrap();
    assert!(made.status.success(), "{made:?}");
    assert!(linked(&pin(&folder, &file).unwrap_err()));
    let target = Target {
        file: file.clone(),
        folder: folder.clone(),
        section: "Framerate".into(),
        key: "FramerateLimit".into(),
        value: "120".into(),
    };
    let report = apply(&game.journal, "phone", &[target]).unwrap();
    assert!(
        report.written.is_empty() && report.skipped.len() == 1,
        "{report:?}"
    );
    assert_eq!(
        current(&game.folder.join("Moved").join("OptiScaler.ini")),
        "59"
    );

    // A folder above the app's: the held ones keep it from moving, and one that is a
    // junction by the time they are held is refused, though the app's folder opens.
    let library = game.folder.with_file_name("Library");
    std::fs::create_dir_all(library.join("Game")).unwrap();
    std::fs::write(library.join("Game").join("OptiScaler.ini"), OPTISCALER).unwrap();
    let folder = std::fs::canonicalize(library.join("Game")).unwrap();
    let file = folder.join("OptiScaler.ini");
    let pins = pin(&folder, &file).unwrap();
    let elsewhere = game.folder.with_file_name("Elsewhere");
    assert!(std::fs::rename(&library, &elsewhere).is_err());
    drop(pins);
    std::fs::rename(&library, &elsewhere).unwrap();
    let made = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&library)
        .arg(&elsewhere)
        .output()
        .unwrap();
    assert!(made.status.success(), "{made:?}");
    assert!(linked(&pin(&folder, &file).unwrap_err()));
}

#[test]
fn a_crash_while_another_devices_key_goes_back_leaves_the_pcs_own_value() {
    let game = game();
    let change = |session: &str, written: &str| Change {
        session: session.into(),
        path: game.resolved.clone(),
        folder: PathBuf::new(),
        section: "Framerate".into(),
        key: "FramerateLimit".into(),
        original: "59".into(),
        written: written.into(),
    };
    // The TV wrote 60 over the PC's 59. The handheld's write put the 59 back and journaled
    // its 120, keeping the TV's entry until the file changed. In either order, a restore
    // after a crash before the swap (the file at 60) or after it (at 120) gives the 59.
    for order in [["tv", "handheld"], ["handheld", "tv"]] {
        for on_disk in ["60", "120"] {
            let journal = Journal {
                version: 1,
                changes: order
                    .iter()
                    .map(|session| change(session, if *session == "tv" { "60" } else { "120" }))
                    .collect(),
            };
            journal.save(&game.journal).unwrap();
            std::fs::write(
                &game.file,
                OPTISCALER.replace("= 59", &format!("= {on_disk}")),
            )
            .unwrap();
            restore(&game.journal, Scope::All).unwrap();
            assert_eq!(current(&game.file), "59", "{order:?} {on_disk}");
        }
    }
}

#[test]
fn an_original_goes_back_whatever_its_length() {
    let game = game();
    let long = "x".repeat(MAX_TEXT + 10);
    std::fs::write(
        &game.file,
        format!("[Framerate]\r\nFramerateLimit={long}\r\n"),
    )
    .unwrap();
    let folders = folders([game.folder.clone()]);
    let targets = targets(
        &[limit("60")],
        None,
        Some(&game.folder),
        &folders,
        &mut Report::default(),
    );
    apply(&game.journal, "phone", &targets).unwrap();
    assert_eq!(current(&game.file), "60");
    restore(&game.journal, Scope::All).unwrap();
    assert_eq!(current(&game.file), long);
}

#[test]
fn without_the_windows_user_nothing_is_written_and_restores_wait() {
    let game = game();
    assert!(super::super::folders([game.folder.clone()], no_user).is_empty());
    let folders = folders([game.folder.clone()]);
    assert!(matches!(
        super::super::resolve("OptiScaler.ini", Some(&game.folder), &folders, no_user),
        Err(Unusable::Missing(_))
    ));
    let targets = targets(
        &[limit("60")],
        None,
        Some(&game.folder),
        &folders,
        &mut Report::default(),
    );
    let report = super::super::apply(&game.journal, "phone", &targets, no_user).unwrap();
    assert!(
        report.written.is_empty() && report.skipped.len() == 1,
        "{report:?}"
    );
    assert_eq!(std::fs::read_to_string(&game.file).unwrap(), OPTISCALER);
    assert!(Journal::load(&game.journal).unwrap().changes.is_empty());
    apply(&game.journal, "phone", &targets).unwrap();
    let report = super::super::restore(&game.journal, Scope::All, no_user).unwrap();
    assert_eq!(report.failed.len(), 1, "{report:?}");
    assert_eq!(current(&game.file), "60");
    restore(&game.journal, Scope::All).unwrap();
    assert_eq!(std::fs::read_to_string(&game.file).unwrap(), OPTISCALER);
}

#[test]
fn a_restore_can_spare_the_sessions_still_running() {
    let game = game();
    let folders = folders([game.folder.clone()]);
    let mut upscaler = limit("xess");
    upscaler.section = "Upscalers".into();
    upscaler.key = "Dx12Upscaler".into();
    for (session, entry) in [("tv", limit("60")), ("phone", upscaler)] {
        let targets = targets(
            &[entry],
            None,
            Some(&game.folder),
            &folders,
            &mut Report::default(),
        );
        apply(&game.journal, session, &targets).unwrap();
    }
    let running = BTreeSet::from(["tv".to_owned()]);
    restore(&game.journal, Scope::Except(&running)).unwrap();
    assert_eq!(
        std::fs::read_to_string(&game.file).unwrap(),
        OPTISCALER.replace("= 59", "= 60")
    );
    restore(&game.journal, Scope::Except(&BTreeSet::new())).unwrap();
    assert_eq!(std::fs::read_to_string(&game.file).unwrap(), OPTISCALER);
}
