//! Tests for drive placement, release integrity, fallback installs, and path normalization.
use super::*;

#[test]
fn dots_network_paths_and_drives_other_than_fixed_ones_are_refused() {
    let tree = tree();
    let dotted = format!(
        r"{}\..\Programs\Nucleus Co-op",
        tree.top.join("Programs").display()
    );
    let here = format!(r"{}\.\Nucleus Co-op", tree.top.join("Programs").display());
    for path in [dotted.as_str(), here.as_str()] {
        let (problem, error) = refused(&tree, Path::new(path));
        assert_eq!(problem, Some(Problem::Untrusted), "{error}");
        assert!(error.contains("goes through . or .."), "{error}");
    }
    for path in [
        r"\\server\share\Nucleus Co-op",
        r"\\?\UNC\server\share\Nucleus Co-op",
        r"\\localhost\C$\Program Files\Nucleus Co-op",
        "//server/share/Nucleus Co-op",
    ] {
        let (problem, error) = refused(&tree, Path::new(path));
        assert_eq!(problem, Some(Problem::Untrusted), "{error}");
        assert!(error.contains("is a network path"), "{error}");
    }
    let device = format!(r"\\?\{}", tree.folder.display());
    let (problem, error) = refused(&tree, Path::new(&device));
    assert_eq!(problem, Some(Problem::Untrusted), "{error}");
    assert!(error.contains("is a device path"), "{error}");
    // A mapped network drive, a stick, a disc: only a fixed drive.
    for (kind, says) in [
        (DRIVE_REMOTE, "a network drive"),
        (DRIVE_REMOVABLE, "a removable drive"),
        (DRIVE_CDROM, "a CD or DVD drive"),
        (DRIVE_RAMDISK, "a RAM disk"),
        (0, "a drive Windows doesn't know"),
    ] {
        let error = drive_refusal(r"Z:\Nucleus", r"Z:\", kind).unwrap();
        assert_eq!(problem_of(&error), Some(Problem::Untrusted));
        assert!(error.to_string().contains(says), "{error}");
    }
    assert!(drive_refusal(r"C:\Nucleus", r"C:\", DRIVE_FIXED).is_none());
    // A drive letter mapped for the user only is none of the service's.
    let error = drive_refusal(r"Z:\Nucleus", r"Z:\", DRIVE_NO_ROOT_DIR).unwrap();
    assert_eq!(problem_of(&error), Some(Problem::Missing));
    if let Some(free) = ('D'..='Z')
        .rev()
        .find(|letter| drive_kind(&format!("{letter}:\\")) == DRIVE_NO_ROOT_DIR)
    {
        let (problem, error) = refused(&tree, Path::new(&format!(r"{free}:\Nucleus Co-op")));
        assert_eq!(problem, Some(Problem::Missing), "{error}");
        assert!(error.contains("no drive of this PC"), "{error}");
    }
    assert_eq!(
        drive_kind(&format!("{}\\", &tree.top.to_string_lossy()[..2])),
        DRIVE_FIXED
    );
}

#[test]
fn a_short_name_is_refused_for_the_folders_own() {
    let tree = tree();
    let wide: Vec<u16> = tree
        .folder
        .as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut short = vec![0u16; 1024];
    // SAFETY: `wide` is NUL-terminated and `short` writable, both outliving the call.
    let len = unsafe {
        windows::Win32::Storage::FileSystem::GetShortPathNameW(
            windows::core::PCWSTR(wide.as_ptr()),
            Some(&mut short),
        )
    } as usize;
    let short = String::from_utf16_lossy(&short[..len]);
    if len == 0 || short.eq_ignore_ascii_case(&tree.folder.to_string_lossy()) {
        eprintln!("skipped: this volume makes no 8.3 short names");
        return;
    }
    let (problem, error) = refused(&tree, Path::new(&short));
    assert_eq!(problem, Some(Problem::Untrusted), "{error}");
    assert!(
        error.contains(&format!("choose {} itself", tree.folder.display())),
        "{error}"
    );
}

#[test]
fn a_swapped_program_file_is_found_though_its_size_and_time_stay() {
    let tree = tree();
    let dll = tree.folder.join("Nucleus.Gaming.dll");
    // Checked, so its answer is kept.
    check(&tree, &tree.folder).unwrap();
    let written = std::fs::metadata(&dll).unwrap().modified().unwrap();
    let same_time = |path: &Path| {
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(written)
            .unwrap();
    };
    // The change time ticks with the system clock (15.6 ms on some PCs): each
    // write waits for a new tick, as a real change would come later anyway.
    let tick = || std::thread::sleep(std::time::Duration::from_millis(50));
    // Written in place, its time put back: the same file id.
    tick();
    std::fs::write(&dll, b"MZ gamin!").unwrap();
    same_time(&dll);
    let (problem, error) = refused(&tree, &tree.folder);
    assert_eq!(problem, Some(Problem::Version), "{error}");
    assert!(
        error.contains(&format!("{} isn't Nucleus Co-op 2.4.2's", dll.display())),
        "{error}"
    );
    tick();
    std::fs::write(&dll, b"MZ gaming").unwrap();
    same_time(&dll);
    check(&tree, &tree.folder).unwrap();
    // Another file renamed over it with the same size and time.
    tick();
    let other = tree.folder.join("other.tmp");
    std::fs::write(&other, b"MZ gamin?").unwrap();
    same_time(&other);
    std::fs::rename(&other, &dll).unwrap();
    assert_eq!(
        std::fs::metadata(&dll).unwrap().modified().unwrap(),
        written
    );
    let (problem, error) = refused(&tree, &tree.folder);
    assert_eq!(problem, Some(Problem::Version), "{error}");
}

#[test]
fn a_program_file_held_open_to_write_is_refused_though_its_answer_is_kept() {
    let tree = tree();
    // Checked, so its answer is kept.
    check(&tree, &tree.folder).unwrap();
    let dll = tree.folder.join("Nucleus.Gaming.dll");
    // Its stamp stays, but whoever holds it may write it after the check.
    let writer = std::fs::OpenOptions::new().write(true).open(&dll).unwrap();
    let error = format!("{:#}", check(&tree, &tree.folder).unwrap_err());
    assert!(
        error.contains(&format!("opening {}", dll.display())),
        "{error}"
    );
    drop(writer);
    check(&tree, &tree.folder).unwrap();
}

#[test]
fn a_held_folder_and_its_program_cant_be_moved_replaced_or_written_until_dropped() {
    let tree = tree();
    let release = manifest();
    let checks = checks(&release, &tree.top);
    let held = hold_checked(&tree.folder, &checks).unwrap();
    assert_eq!(held.folder(), tree.folder);
    let exe = held.program();
    assert_eq!(exe, tree.folder.join(pinned::EXECUTABLE));
    let moved = tree.top.join("Moved");
    let other = tree.folder.join("other.exe");
    std::fs::write(&other, b"MZ 2.4.2").unwrap();
    assert!(std::fs::rename(&tree.folder, &moved).is_err());
    assert!(std::fs::rename(&exe, tree.folder.join("old.exe")).is_err());
    assert!(std::fs::rename(&other, &exe).is_err());
    assert!(std::fs::OpenOptions::new().write(true).open(&exe).is_err());
    assert!(std::fs::remove_file(&exe).is_err());
    drop(held);
    std::fs::rename(&other, &exe).unwrap();
    std::fs::rename(&tree.folder, &moved).unwrap();
}

#[test]
fn a_drive_swapped_under_the_same_letter_or_on_usb_is_refused() {
    let tree = tree();
    let release = manifest();
    let first = Volume {
        name: r"\\?\Volume{11111111-2222-3333-4444-555555555555}\".into(),
        serial: 0x1234_abcd,
        usb: false,
    };
    let now = std::sync::Arc::new(Mutex::new(first.clone()));
    let mut checks = checks(&release, &tree.top);
    let seen = now.clone();
    checks.volume = Box::new(move |_| Ok(seen.lock().unwrap().clone()));
    hold_checked(&tree.folder, &checks).unwrap();
    // Another disk with the same files and owners, or the same serial
    // number on a volume Windows names otherwise.
    for swapped in [
        Volume {
            serial: 0x0bad_f00d,
            ..first.clone()
        },
        Volume {
            name: r"\\?\Volume{99999999-2222-3333-4444-555555555555}\".into(),
            ..first.clone()
        },
    ] {
        *now.lock().unwrap() = swapped;
        let error = hold_checked(&tree.folder, &checks).err().unwrap();
        assert_eq!(problem_of(&error), Some(Problem::Untrusted), "{error:#}");
        assert!(
            error
                .to_string()
                .contains("isn't the one Rubylight first checked it on"),
            "{error}"
        );
        // The status the console shows says so too.
        assert!(chosen(&tree.folder, &checks).is_err());
    }
    *now.lock().unwrap() = first.clone();
    hold_checked(&tree.folder, &checks).unwrap();
    // On a USB drive, which Windows may count as fixed.
    *now.lock().unwrap() = Volume {
        usb: true,
        ..first.clone()
    };
    let (problem, error) = match chosen(&tree.folder, &checks) {
        Err(error) => (problem_of(&error), format!("{error:#}")),
        Ok(_) => panic!("a folder on a USB drive was accepted"),
    };
    assert_eq!(problem, Some(Problem::Untrusted), "{error}");
    assert!(error.contains("is on a USB drive"), "{error}");
    // This PC's own drive, read as the PC does.
    let real = volume_of(&format!("{}\\", &tree.top.to_string_lossy()[..2])).unwrap();
    assert!(real.name.starts_with(r"\\?\Volume{"), "{real:?}");
}

#[test]
fn a_program_file_marked_as_downloaded_is_refused_and_keeps_its_mark() {
    for name in ["NucleusCoop.exe", "Nucleus.Gaming.dll"] {
        let tree = tree();
        let path = tree.folder.join(name);
        let mut stream = path.as_os_str().to_owned();
        stream.push(":Zone.Identifier");
        std::fs::write(&stream, b"[ZoneTransfer]\r\nZoneId=3\r\n").unwrap();
        let (problem, error) = refused(&tree, &tree.folder);
        assert_eq!(problem, Some(Problem::Blocked), "{error}");
        assert!(
            error.contains("Unblock the zip before extracting"),
            "{error}"
        );
        assert!(error.contains(&path.display().to_string()), "{error}");
        // Unblocking is the user's: the mark stays.
        assert!(std::fs::metadata(&stream).is_ok());
    }
}

#[test]
fn a_missing_or_other_program_file_is_refused_with_the_release_it_needs() {
    let tree = tree();
    let hook = tree.folder.join(r"utils\Hooks\EasyHook64.dll");
    std::fs::remove_file(&hook).unwrap();
    let (problem, error) = refused(&tree, &tree.folder);
    assert_eq!(problem, Some(Problem::Version), "{error}");
    assert!(
        error.contains(&format!(
            "{} is missing from Nucleus Co-op 2.4.2",
            hook.display()
        )),
        "{error}"
    );
    let exe = tree.folder.join(pinned::EXECUTABLE);
    std::fs::write(&exe, b"MZ 2.3.1").unwrap();
    let (problem, error) = refused(&tree, &tree.folder);
    assert_eq!(problem, Some(Problem::Version), "{error}");
    assert_eq!(
        error,
        format!(
            "The NucleusCoop.exe in {} isn't Nucleus Co-op 2.4.2's: download 2.4.2 from its release page and choose its folder",
            tree.folder.display()
        )
    );
}

#[test]
fn a_folder_without_nucleus_is_refused_never_replaced_by_the_old_install() {
    let tree = tree();
    let empty = tree.top.join("Empty");
    std::fs::create_dir(&empty).unwrap();
    let file = tree.top.join("NucleusApp.zip");
    std::fs::write(&file, b"PK").unwrap();
    for (path, says) in [
        (empty.clone(), "holds no NucleusCoop.exe"),
        (tree.top.join("Gone"), "doesn't exist"),
        (file, "holds no NucleusCoop.exe"),
        (PathBuf::from("Nucleus"), "is no full folder path"),
    ] {
        let (problem, error) = refused(&tree, &path);
        assert_eq!(problem, Some(Problem::Missing), "{error}");
        assert!(error.contains(says), "{error}");
    }
}

#[test]
fn an_earlier_install_keeps_working_and_nothing_is_installed() {
    let tree = tree();
    let release = manifest();
    let checks = checks(&release, &tree.top);
    let data = tree.top.join("data");
    std::fs::create_dir(&data).unwrap();
    assert_eq!(found_checked(&data, "", &checks).unwrap(), None);
    // Nothing was made below the host's data folder.
    assert_eq!(std::fs::read_dir(&data).unwrap().count(), 0);
    let data = earlier(&tree);
    let found = found_checked(&data, "", &checks).unwrap().unwrap();
    assert_eq!(found.folder, pinned::folder(&data));
    assert_eq!(found.notice(), None);
    // The folder chosen runs over it.
    let found = found_checked(&data, &tree.folder.to_string_lossy(), &checks)
        .unwrap()
        .unwrap();
    assert_eq!(found.folder, tree.folder);
}

#[test]
fn another_release_chosen_falls_back_to_the_earlier_install_with_a_notice() {
    let tree = tree();
    let release = manifest();
    let checks = checks(&release, &tree.top);
    std::fs::write(tree.folder.join(pinned::EXECUTABLE), b"MZ 2.3.1").unwrap();
    let configured = tree.folder.to_string_lossy().into_owned();
    // No earlier install: refused, saying which release it needs.
    let data = tree.top.join("none");
    let error = found_checked(&data, &configured, &checks).unwrap_err();
    assert_eq!(problem_of(&error), Some(Problem::Version));
    let data = earlier(&tree);
    let found = found_checked(&data, &configured, &checks).unwrap().unwrap();
    assert_eq!(found.folder, pinned::folder(&data));
    assert_eq!(
        found.instead.as_ref().map(|why| why.problem),
        Some(Problem::Version)
    );
    let notice = found.notice().unwrap();
    assert!(notice.contains("isn't Nucleus Co-op 2.4.2's"), "{notice}");
    assert!(
        notice.contains(&format!(
            "Until then Rubylight runs the Nucleus Co-op 2.4.2 an earlier Rubylight installed, in {}",
            pinned::folder(&data).display()
        )),
        "{notice}"
    );
    // Only another release: a folder others can change is refused.
    std::fs::write(tree.folder.join(pinned::EXECUTABLE), b"MZ 2.4.2").unwrap();
    crate::process::restrict(&tree.folder.join("handlers"), &sddl("(A;;0x1301bf;;;AU)")).unwrap();
    let error = found_checked(&data, &configured, &checks).unwrap_err();
    assert_eq!(problem_of(&error), Some(Problem::Untrusted), "{error:#}");
}

#[test]
fn only_system_and_administrators_may_change_a_folder_nucleus_runs_from() {
    let folder = Path::new(r"C:\Program Files\Nucleus");
    let trusts = |sid: &str| TRUSTED.contains(&sid);
    let trusted = &["S-1-5-18".to_string(), "S-1-5-32-544".to_string()];
    assert!(verdict(folder, "S-1-5-32-544", trusted, trusts).is_ok());
    assert!(verdict(folder, TRUSTED[2], &[], trusts).is_ok());
    let user = verdict(folder, "S-1-5-21-1-2-3-1001", &[], trusts).unwrap_err();
    assert!(
        user.to_string().contains("is owned by S-1-5-21-1-2-3-1001"),
        "{user}"
    );
    let writer = verdict(folder, "S-1-5-18", &["S-1-5-11".into()], trusts).unwrap_err();
    assert!(
        writer.to_string().contains("can be changed by S-1-5-11"),
        "{writer}"
    );
    let entry = verdict(folder, "S-1-5-18", &["ACE type 5".into()], trusts).unwrap_err();
    assert!(entry.to_string().contains("ACE type 5"), "{entry}");
}

#[test]
fn a_path_is_compared_as_windows_names_it() {
    for (path, name) in [
        (
            r"\\?\C:\Program Files\Nucleus Co-op",
            r"C:\Program Files\Nucleus Co-op",
        ),
        (r"\\?\UNC\server\share\x", r"\\server\share\x"),
        (
            r"C:/Program Files/Nucleus Co-op/",
            r"C:\Program Files\Nucleus Co-op",
        ),
        (r"\\?\C:\", r"C:\"),
        (r"C:\", r"C:\"),
    ] {
        assert_eq!(normal(std::ffi::OsStr::new(path)), name, "{path}");
    }
}
