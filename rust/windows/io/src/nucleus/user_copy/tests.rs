//! Tests for the signed-in user's Nucleus Co-op copy syncing, recovery, and ownership checks.
use super::*;

mod security_handlers;

const SID: &str = "S-1-5-21-1-2-3-1001";

fn create(path: &Path, _: &str) -> std::io::Result<()> {
    std::fs::create_dir(path)
}
/// [`secured`]'s open without its checks: a test process can't make
/// Administrators the owner.
fn open(path: &Path, _: &[&str]) -> Result<std::fs::File> {
    Ok(open_held(path)?)
}
const TEST: Steps = Steps {
    create,
    check: open,
};

/// The host's Nucleus settings, with its user's own values.
const HOST_SETTINGS: &str = "[Misc]\r\nNucleusAccountPassword=hunter2\r\n[ControllerMapping]\r\nPlayer_1=Alice\r\n[SteamIDs]\r\nPlayer_1=76561190000000001\r\n";

/// A host's Nucleus folder after it ran: its program and settings, and
/// what it and the host wrote as they ran.
fn host_folder() -> tempfile::TempDir {
    let folder = tempfile::tempdir().unwrap();
    for (name, data) in [
        ("NucleusCoop.exe", &b"MZ nucleus"[..]),
        ("Settings.ini", HOST_SETTINGS.as_bytes()),
        (r"gui\theme\default\theme.ini", b"theme"),
        (r"webview\darkreader\background.js", b"dark"),
        // Not the release's: never copied.
        ("planted.dll", b"MZ planted"),
        (r"handlers\Puzzle.js", b"Game.GUID = 'Puzzle';"),
        (r"content\Puzzle\Instance0\game.exe", b"MZ game"),
        (r"content\userprofile.json", b"{\"Games\":[\"host\"]}"),
        (r"game profiles\Puzzle\Profile[1].json", b"{}"),
        (r"webview\cache\data_0", b"cache"),
        ("debug-log.txt", b"log"),
        ("Rubylight-layers-S-1-5-21-1-2-3-1001.json", b"{}"),
        // Settings.ini as the host wrote it, left by a crash.
        ("Settings.rubylight.tmp", HOST_SETTINGS.as_bytes()),
    ] {
        let path = folder.path().join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, data).unwrap();
    }
    std::fs::create_dir_all(folder.path().join(r"gui\covers")).unwrap();
    folder
}

fn names(folder: &Path) -> Vec<String> {
    let mut names: Vec<_> = std::fs::read_dir(folder)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

fn junction(target: &Path, link: &Path) {
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "mklink /J failed");
}

#[test]
fn the_copy_is_the_hosts_program_without_what_it_wrote() {
    let data = tempfile::tempdir().unwrap();
    let host = host_folder();
    let source = host.path();
    let copy = ensure_with(data.path(), source, SID, None, &TEST).unwrap();
    let folder = pinned::user_folder(data.path(), SID).unwrap();
    let key = copy.file_name().unwrap().to_str().unwrap().to_string();
    assert_eq!(copy, pinned::user_copy(data.path(), SID, &key).unwrap());
    assert_eq!(key.len(), 16);
    // Not the host's copy, nor below it.
    assert!(!copy.starts_with(data.path().join("nucleus")));
    for name in [
        "NucleusCoop.exe",
        r"gui\theme\default\theme.ini",
        r"webview\darkreader\background.js",
    ] {
        assert_eq!(
            std::fs::read(copy.join(name)).unwrap(),
            std::fs::read(source.join(name)).unwrap(),
            "{name}"
        );
    }
    // A file that is not the pinned release's stays in the host's.
    assert!(!copy.join("planted.dll").exists());
    // With the host's times, which the next check compares.
    let modified = |path: &Path| std::fs::metadata(path).unwrap().modified().unwrap();
    assert_eq!(
        modified(&copy.join("NucleusCoop.exe")),
        modified(&source.join("NucleusCoop.exe"))
    );
    // What Nucleus and the host wrote as they ran is not copied; the
    // folders it is in are there, empty.
    for name in [
        "debug-log.txt",
        "Rubylight-layers-S-1-5-21-1-2-3-1001.json",
        "Settings.rubylight.tmp",
    ] {
        assert!(!copy.join(name).exists(), "{name}");
    }
    for name in [
        "content",
        "handlers",
        "game profiles",
        r"gui\covers",
        r"webview\cache",
    ] {
        assert!(names(&copy.join(name)).is_empty(), "{name}");
    }
    assert!(copy.join(MARKER).is_file());
    assert_eq!(names(&folder), [key.as_str()]);
    // Nucleus' own settings, with every key it ships and ours, and none
    // of the host's user's.
    let settings = std::fs::read_to_string(copy.join("Settings.ini")).unwrap();
    assert_eq!(settings.as_bytes(), profile::default_settings_ini());
    for line in profile::DEFAULT_SETTINGS.lines() {
        let key = line.split('=').next().unwrap();
        assert!(settings.contains(key), "{key}");
    }
    for private in ["hunter2", "Alice", "76561190000000001"] {
        assert!(!settings.contains(private), "{private}");
    }

    // What the host's Nucleus or the user's writes as they run is no
    // change: the same copy is kept, with what the user's wrote.
    std::fs::write(copy.join("content").join("Puzzle"), b"theirs").unwrap();
    // Its game list, where Nucleus 2.4.2 reads it, and the host's.
    let list = copy.join("content").join("userprofile.json");
    std::fs::write(&list, b"{\"Games\":[\"theirs\"]}").unwrap();
    std::fs::write(
        source.join(r"content\userprofile.json"),
        b"{\"Games\":[\"host\",\"hub\"]}",
    )
    .unwrap();
    std::fs::write(copy.join("Settings.ini"), b"[Dev]\r\ntheirs").unwrap();
    std::fs::write(source.join("debug-log.txt"), b"more").unwrap();
    std::fs::write(source.join("crashlog.txt"), b"crash").unwrap();
    std::fs::write(source.join("Settings.ini"), b"[Dev]\r\nhost").unwrap();
    std::fs::write(source.join(r"handlers\Rubylight-x.js"), b"//").unwrap();
    std::fs::create_dir_all(source.join(r"Temp\download")).unwrap();
    assert_eq!(
        ensure_with(data.path(), source, SID, None, &TEST).unwrap(),
        copy
    );
    assert!(copy.join("content").join("Puzzle").is_file());
    assert_eq!(std::fs::read(&list).unwrap(), b"{\"Games\":[\"theirs\"]}");
    assert!(
        !std::fs::read_to_string(copy.join(MARKER))
            .unwrap()
            .contains("userprofile")
    );
    assert_eq!(
        std::fs::read(copy.join("Settings.ini")).unwrap(),
        b"[Dev]\r\ntheirs"
    );
    // Gone, Nucleus' own are back.
    std::fs::remove_file(copy.join("Settings.ini")).unwrap();
    ensure_with(data.path(), source, SID, None, &TEST).unwrap();
    assert_eq!(
        std::fs::read(copy.join("Settings.ini")).unwrap(),
        profile::default_settings_ini()
    );
}

#[test]
fn a_copy_that_differs_from_the_hosts_is_made_again() {
    let data = tempfile::tempdir().unwrap();
    let host = host_folder();
    let copy = ensure_with(data.path(), host.path(), SID, None, &TEST).unwrap();
    type Change = (&'static str, fn(&Path));
    let changes: [Change; 8] = [
        ("a file removed", |copy| {
            std::fs::remove_file(copy.join(r"gui\theme\default\theme.ini")).unwrap()
        }),
        ("a folder removed", |copy| {
            std::fs::remove_dir_all(copy.join("webview")).unwrap()
        }),
        // Its size is the same, its time not.
        ("a file changed", |copy| {
            std::fs::write(copy.join("NucleusCoop.exe"), b"MZ NUCLEUS").unwrap()
        }),
        ("a file shortened", |copy| {
            std::fs::write(copy.join("NucleusCoop.exe"), b"MZ").unwrap()
        }),
        // One of the release's that the host's doesn't have.
        ("a file added", |copy| {
            std::fs::write(copy.join("SDL2.dll"), b"MZ").unwrap()
        }),
        ("its marker removed", |copy| {
            std::fs::remove_file(copy.join(MARKER)).unwrap()
        }),
        ("its marker changed", |copy| {
            std::fs::write(copy.join(MARKER), b"NucleusCoop.exe\t2\t1\n").unwrap()
        }),
        // Read no further than the listing it should be.
        ("its marker grown", |copy| {
            let mut marker = std::fs::read(copy.join(MARKER)).unwrap();
            marker.extend(vec![b'x'; 1 << 20]);
            std::fs::write(copy.join(MARKER), marker).unwrap()
        }),
    ];
    for (what, change) in changes {
        change(&copy);
        assert_eq!(
            ensure_with(data.path(), host.path(), SID, None, &TEST).unwrap(),
            copy,
            "{what}"
        );
        assert_eq!(
            std::fs::read(copy.join("NucleusCoop.exe")).unwrap(),
            b"MZ nucleus",
            "{what}"
        );
        assert!(
            copy.join(r"gui\theme\default\theme.ini").is_file(),
            "{what}"
        );
        assert!(
            copy.join(r"webview\darkreader\background.js").is_file(),
            "{what}"
        );
        assert!(!copy.join("SDL2.dll").exists(), "{what}");
        assert_eq!(
            differs(&copy, &Listing::of(host.path()).unwrap().text).map(|_| ()),
            None,
            "{what}"
        );
    }

    // A link the user put in their copy goes with it, and what it
    // points to stays.
    let elsewhere = tempfile::tempdir().unwrap();
    std::fs::write(elsewhere.path().join("kept.txt"), b"x").unwrap();
    junction(elsewhere.path(), &copy.join(r"gui\linked"));
    ensure_with(data.path(), host.path(), SID, None, &TEST).unwrap();
    assert!(!copy.join(r"gui\linked").exists());
    assert_eq!(names(elsewhere.path()), ["kept.txt"]);
}

/// `icacls` on `path` with `args`.
fn icacls(path: &Path, args: &[&str]) {
    let status = std::process::Command::new("icacls")
        .arg(path)
        .args(args)
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "icacls {args:?} failed");
}

/// #56: one file in the user's copy that they can't change any more
/// refused every launch. A read-only one is made plain again; for one
/// Windows refuses them, the copy is set aside and made again, also when
/// it differs from the host's anyway, and all of the old one but that
/// file goes. A copy whose files they can change is kept as it is.
#[test]
fn a_copy_with_a_file_the_user_cant_change_is_set_aside_and_made_again() {
    let data = tempfile::tempdir().unwrap();
    let host = host_folder();
    let copy = ensure_with(data.path(), host.path(), SID, None, &TEST).unwrap();
    let folder = pinned::user_folder(data.path(), SID).unwrap();
    let key = copy.file_name().unwrap().to_str().unwrap().to_string();
    let handler = copy.join(r"handlers\Rubylight-game-1a9b8f6f.js");
    let profiles = copy.join(r"game profiles\Rubylight-game-1a9b8f6f");
    let profile = profiles.join("Profile[1].json");
    let theirs = copy.join(r"game profiles\Puzzle\Profile[1].json");
    std::fs::create_dir_all(&profiles).unwrap();
    std::fs::create_dir_all(theirs.parent().unwrap()).unwrap();
    std::fs::write(&handler, b"// Rubylight one-player handler").unwrap();
    std::fs::write(&profile, b"{}").unwrap();
    std::fs::write(&theirs, b"{}").unwrap();
    // A profile of a hub handler's game that its user made read-only is
    // theirs: Rubylight never writes it.
    let mut permissions = std::fs::metadata(&theirs).unwrap().permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&theirs, permissions).unwrap();
    assert_eq!(stuck(&copy), None);
    assert_eq!(
        ensure_with(data.path(), host.path(), SID, None, &TEST).unwrap(),
        copy
    );
    assert!(handler.is_file() && profile.is_file());
    #[allow(clippy::permissions_set_readonly_false)]
    {
        let mut permissions = std::fs::metadata(&theirs).unwrap().permissions();
        permissions.set_readonly(false);
        std::fs::set_permissions(&theirs, permissions).unwrap();
    }

    // Read-only, as the reporter's handler may be: the user makes it
    // plain again, and the copy stays.
    let mut permissions = std::fs::metadata(&handler).unwrap().permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&handler, permissions).unwrap();
    assert_eq!(stuck(&copy), None);
    assert!(
        !std::fs::metadata(&handler)
            .unwrap()
            .permissions()
            .readonly()
    );
    assert_eq!(
        std::fs::read(&handler).unwrap(),
        b"// Rubylight one-player handler"
    );
    assert_eq!(
        ensure_with(data.path(), host.path(), SID, None, &TEST).unwrap(),
        copy
    );
    assert!(handler.is_file());
    assert_eq!(names(&folder), [key.as_str()]);

    // Windows refuses the user the profile, and to remove it from its
    // folder: the copy goes aside, and all of it but that file goes.
    icacls(&profile, &["/deny", "*S-1-1-0:(W,D)"]);
    icacls(&profiles, &["/deny", "*S-1-1-0:(DC)"]);
    let why = stuck(&copy).unwrap();
    assert!(why.contains("Profile[1].json"), "{why}");
    let made = ensure_with(data.path(), host.path(), SID, None, &TEST);
    let aside = folder.join(format!("{key}.aside-1"));
    let left = names(&aside);
    icacls(data.path(), &["/reset", "/T", "/C", "/Q"]);
    assert_eq!(made.unwrap(), copy);
    assert_eq!(left, ["game profiles"]);
    assert!(!profile.exists() && !handler.exists());
    assert_eq!(stuck(&copy), None);
    assert_eq!(
        differs(&copy, &Listing::of(host.path()).unwrap().text).map(|_| ()),
        None
    );
    // Removed at the next start, once it can be.
    ensure_with(data.path(), host.path(), SID, None, &TEST).unwrap();
    assert_eq!(names(&folder), [key.as_str()]);

    // A copy that is no longer the host's and can't be emptied to be
    // made again goes aside too.
    let stuck_file = copy.join(r"content\stuck.txt");
    std::fs::write(&stuck_file, b"x").unwrap();
    std::fs::write(copy.join(r"gui\theme\default\theme.ini"), b"changed").unwrap();
    icacls(&stuck_file, &["/deny", "*S-1-1-0:(D)"]);
    icacls(&copy.join("content"), &["/deny", "*S-1-1-0:(DC)"]);
    let made = ensure_with(data.path(), host.path(), SID, None, &TEST);
    let left = names(&folder.join(format!("{key}.aside-1")));
    icacls(data.path(), &["/reset", "/T", "/C", "/Q"]);
    assert_eq!(made.unwrap(), copy);
    assert_eq!(left, ["content"]);
    assert!(!stuck_file.exists());
    assert_eq!(
        std::fs::read(copy.join(r"gui\theme\default\theme.ini")).unwrap(),
        b"theme"
    );
}

#[test]
fn a_changed_program_makes_a_new_copy_and_drops_the_old() {
    let data = tempfile::tempdir().unwrap();
    let host = host_folder();
    let folder = pinned::user_folder(data.path(), SID).unwrap();
    // A copy named by its version (an earlier build's), a partial one.
    std::fs::create_dir_all(folder.join("2.4.2").join("handlers")).unwrap();
    std::fs::create_dir_all(folder.join("0123456789abcdef.partial")).unwrap();
    let first = ensure_with(data.path(), host.path(), SID, None, &TEST).unwrap();
    let name = |copy: &Path| copy.file_name().unwrap().to_str().unwrap().to_string();
    assert_eq!(names(&folder), [name(&first)]);
    // Nucleus updated (or nucleus_path pointed elsewhere).
    std::fs::write(host.path().join("NucleusCoop.exe"), b"MZ nucleus 2.5").unwrap();
    let second = ensure_with(data.path(), host.path(), SID, None, &TEST).unwrap();
    assert_ne!(first, second);
    assert_eq!(
        std::fs::read(second.join("NucleusCoop.exe")).unwrap(),
        b"MZ nucleus 2.5"
    );
    assert_eq!(names(&folder), [name(&second)]);
    // Nothing is removed through a link in the user's folder's place.
    let elsewhere = tempfile::tempdir().unwrap();
    std::fs::create_dir(elsewhere.path().join("other")).unwrap();
    let linked = data.path().join("linked");
    junction(elsewhere.path(), &linked);
    let error = remove_others(&linked, &name(&second), None).unwrap_err();
    assert!(
        format!("{error:#}").contains("is a link to elsewhere"),
        "{error:#}"
    );
    assert_eq!(names(elsewhere.path()), ["other"]);
}

#[test]
fn a_link_in_the_hosts_folder_is_never_copied() {
    let data = tempfile::tempdir().unwrap();
    let host = host_folder();
    let elsewhere = tempfile::tempdir().unwrap();
    std::fs::write(elsewhere.path().join("secret.txt"), b"x").unwrap();
    // Nucleus links game files into its instances: never looked into.
    junction(
        elsewhere.path(),
        &host.path().join(r"content\Puzzle\Instance1"),
    );
    let copy = ensure_with(data.path(), host.path(), SID, None, &TEST).unwrap();
    assert!(names(&copy.join("content")).is_empty());
    // In its program: refused, and nothing is copied.
    std::fs::write(host.path().join("NucleusCoop.exe"), b"MZ nucleus 2.5").unwrap();
    junction(elsewhere.path(), &host.path().join(r"gui\linked"));
    let error = ensure_with(data.path(), host.path(), SID, None, &TEST).unwrap_err();
    let error = format!("{error:#}");
    assert!(error.contains(r"gui\linked is a link"), "{error}");
    let folder = pinned::user_folder(data.path(), SID).unwrap();
    assert_eq!(
        names(&folder),
        [copy.file_name().unwrap().to_str().unwrap()]
    );
    assert_eq!(names(elsewhere.path()), ["secret.txt"]);
}

#[test]
fn the_hosts_folder_is_read_as_the_host_and_the_copy_written_as_the_user() {
    let sid = crate::process::own_sid().unwrap();
    let user = crate::user_folder::tests::without_own_sid();
    let root = tempfile::tempdir().unwrap();
    // A Nucleus folder only this account by name may read: the token
    // standing in for the user is refused it, the host is not.
    let host = root.path().join("host");
    crate::process::create_folder(&host, &format!("D:P(A;OICI;FA;;;{sid})")).unwrap();
    std::fs::write(host.join("NucleusCoop.exe"), b"MZ nucleus").unwrap();
    let open = root.path().join("open");
    crate::process::create_folder(&open, "D:P(A;OICI;FA;;;WD)").unwrap();
    let copy = ensure_with(&open, &host, SID, Some(&user), &TEST).unwrap();
    assert_eq!(
        std::fs::read(copy.join("NucleusCoop.exe")).unwrap(),
        b"MZ nucleus"
    );
    // A data folder only this account by name may change: the user's
    // copy is not written there.
    let named = root.path().join("named");
    crate::process::create_folder(&named, &format!("D:P(A;OICI;FA;;;{sid})")).unwrap();
    let error = ensure_with(&named, &host, SID, Some(&user), &TEST).unwrap_err();
    let error = format!("{error:#}");
    assert!(error.contains("(os error 5)"), "{error}");
    // Only the folders the host makes are there.
    let folder = pinned::user_folder(&named, SID).unwrap();
    for copy in names(&folder) {
        assert!(names(&folder.join(copy)).is_empty());
    }
}

#[test]
fn a_folder_without_nucleus_or_a_bad_sid_makes_no_copy() {
    let data = tempfile::tempdir().unwrap();
    let host = host_folder();
    std::fs::remove_file(host.path().join("NucleusCoop.exe")).unwrap();
    let error = ensure_with(data.path(), host.path(), SID, None, &TEST).unwrap_err();
    assert!(
        error.to_string().ends_with("holds no NucleusCoop.exe"),
        "{error}"
    );
    assert!(!pinned::user_root(data.path()).exists());
    // Its name in another case is the same file to Windows.
    std::fs::write(host.path().join("nucleuscoop.EXE"), b"MZ nucleus").unwrap();
    let copy = ensure_with(data.path(), host.path(), SID, None, &TEST).unwrap();
    assert!(copy.join("nucleuscoop.EXE").is_file());

    let host = host_folder();
    let error =
        ensure_with(data.path(), host.path(), r"S-1-5-21-1\..\..", None, &TEST).unwrap_err();
    assert!(
        error.to_string().contains("is not a Windows account's SID"),
        "{error}"
    );
}

#[test]
fn a_link_in_place_of_a_folder_is_refused() {
    let data = tempfile::tempdir().unwrap();
    let host = host_folder();
    let elsewhere = tempfile::tempdir().unwrap();
    junction(elsewhere.path(), &pinned::user_root(data.path()));
    let steps = Steps {
        create,
        check: secured,
    };
    let error = ensure_with(data.path(), host.path(), SID, None, &steps).unwrap_err();
    assert!(
        format!("{error:#}").contains("is a link to elsewhere"),
        "{error:#}"
    );
    assert!(
        std::fs::read_dir(elsewhere.path())
            .unwrap()
            .next()
            .is_none()
    );

    // The user's folder too: a junction there is never written through.
    std::fs::remove_dir(pinned::user_root(data.path())).unwrap();
    std::fs::create_dir(pinned::user_root(data.path())).unwrap();
    junction(
        elsewhere.path(),
        &pinned::user_folder(data.path(), SID).unwrap(),
    );
    let mut steps_root_ok = Steps {
        create,
        check: secured,
    };
    fn root_open_user_checked(path: &Path, user: &[&str]) -> Result<std::fs::File> {
        if path.ends_with(SID) {
            secured(path, user)
        } else {
            open(path, user)
        }
    }
    steps_root_ok.check = root_open_user_checked;
    let error = ensure_with(data.path(), host.path(), SID, None, &steps_root_ok).unwrap_err();
    assert!(
        format!("{error:#}").contains("is a link to elsewhere"),
        "{error:#}"
    );
    assert!(
        std::fs::read_dir(elsewhere.path())
            .unwrap()
            .next()
            .is_none()
    );
}

#[test]
fn only_the_hosts_and_the_users_own_folders_are_used() {
    let path = Path::new(r"C:\ProgramData\Butterpollo\nucleus-user");
    let trusted = ["S-1-5-18".to_string(), "S-1-5-32-544".to_string()];
    let none: [String; 0] = [];
    assert!(verdict(path, 0x10, "S-1-5-32-544", (&trusted, &trusted), &[]).is_ok());
    let user = [trusted[0].clone(), SID.to_string()];
    assert!(verdict(path, 0x10, "S-1-5-18", (&user, &trusted), &[SID]).is_ok());
    // Nor this user, who may change it, may move or delete it.
    let moved = verdict(path, 0x10, "S-1-5-18", (&user, &user), &[SID]).unwrap_err();
    assert!(
        moved
            .to_string()
            .contains(&format!("can be moved or deleted by {SID}")),
        "{moved}"
    );
    // Another user may not change it, nor this one where only the host may.
    let another = ["S-1-5-21-9".to_string()];
    let other = verdict(path, 0x10, "S-1-5-18", (&another, &none), &[SID]).unwrap_err();
    assert!(
        other.to_string().contains("can be changed by S-1-5-21-9"),
        "{other}"
    );
    assert!(
        other.to_string().ends_with(
            r"an administrator of this PC can delete C:\ProgramData\Butterpollo\nucleus-user, then start the stream again"
        ),
        "{other}"
    );
    let root = verdict(path, 0x10, "S-1-5-18", (&user, &none), &[]).unwrap_err();
    assert!(
        root.to_string()
            .contains(&format!("can be changed by {SID}")),
        "{root}"
    );
    // The user made it, or it's a link.
    let owned = verdict(path, 0x10, SID, (&none, &none), &[SID]).unwrap_err();
    assert!(
        owned.to_string().contains(&format!("is owned by {SID}")),
        "{owned}"
    );
    let link = verdict(path, 0x410, "S-1-5-18", (&none, &none), &[]).unwrap_err();
    assert!(
        link.to_string().contains("is a link to elsewhere"),
        "{link}"
    );
    let typed = ["ACE type 5".to_string()];
    let entry = verdict(path, 0x10, "S-1-5-18", (&typed, &none), &[SID]).unwrap_err();
    assert!(entry.to_string().contains("ACE type 5"), "{entry}");
}
