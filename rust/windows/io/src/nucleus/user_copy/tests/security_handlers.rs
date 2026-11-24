//! Tests for user-copy SDDL permissions, holding folders/programs open, handler
//! mirroring, and chunked copying.
use super::*;

#[test]
fn the_folders_permissions_keep_the_user_to_what_is_inside() {
    // Owned by Administrators: the user can't change the permissions.
    assert!(ROOT.starts_with("O:BAD:P("));
    assert!(user_sddl(SID).starts_with("O:BAD:P("));
    // Only SYSTEM and Administrators may open the root: a user reaches
    // their own folder by traverse, and never lists whose are there.
    assert_eq!(ROOT, "O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)");
    // The user's folder itself: list, add files and folders, read; what
    // is inside: modify (no permissions, owner or delete-child).
    let sddl = user_sddl(SID);
    assert!(sddl.contains(&format!("(A;;0x1200af;;;{SID})")), "{sddl}");
    assert!(
        sddl.contains(&format!("(A;OICIIO;0x1301bf;;;{SID})")),
        "{sddl}"
    );
    const WRITE_DAC: u32 = 0x4_0000;
    const WRITE_OWNER: u32 = 0x8_0000;
    const DELETE: u32 = 0x1_0000;
    const DELETE_CHILD: u32 = 0x40;
    assert_eq!(
        0x1200af & (WRITE_DAC | WRITE_OWNER | DELETE | DELETE_CHILD),
        0
    );
    assert_eq!(0x1301bf & (WRITE_DAC | WRITE_OWNER | DELETE_CHILD), 0);
    // Their own folder above the copy: list and pass through only. Adding
    // a file is writing its data, which could make it a link.
    const ADD: u32 = 0x2 | 0x4;
    const WRITE_EA: u32 = 0x10;
    const WRITE_ATTRIBUTES: u32 = 0x100;
    let theirs = sid_sddl(SID);
    assert_eq!(
        theirs,
        format!("O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;;0x1200a9;;;{SID})")
    );
    assert_eq!(
        0x1200a9
            & (ADD | WRITE_EA | WRITE_ATTRIBUTES | WRITE_DAC | WRITE_OWNER | DELETE | DELETE_CHILD),
        0
    );
    // Each makes a valid descriptor.
    for sddl in [ROOT.to_string(), sddl, theirs] {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("x");
        match crate::process::create_folder(&path, &sddl) {
            Ok(()) => {
                let file = crate::process::open_unfollowed(&path, true).unwrap();
                assert_eq!(
                    crate::process::open_file_owner(&file).unwrap(),
                    "S-1-5-32-544"
                );
            }
            // Not elevated: Administrators can't be made the owner.
            Err(error) => assert_eq!(error.raw_os_error(), Some(1307), "{error}"),
        }
    }
}

#[test]
fn a_user_reaches_their_folder_through_a_root_they_cannot_open() {
    let sid = crate::process::own_sid().unwrap();
    // Standing in for the user: no rights by this account's name, and
    // only the privileges every account has (bypass traverse checking).
    let user = crate::user_folder::tests::without_own_sid();
    let data = tempfile::tempdir().unwrap();
    let root = data.path().join("nucleus-user");
    // As it is made, with this account in place of Administrators.
    crate::process::create_folder(&root, &ROOT.replace("BA", &sid)).unwrap();
    let theirs = root.join(SID);
    crate::process::create_folder(&theirs, "D:P(A;OICI;FA;;;WD)").unwrap();
    std::fs::write(theirs.join("NucleusCoop.exe"), b"MZ").unwrap();
    let (listed, reached) = as_user(Some(&user), || {
        (
            std::fs::read_dir(&root).map(|_| ()),
            std::fs::read(theirs.join("NucleusCoop.exe")),
        )
    })
    .unwrap();
    assert_eq!(
        listed.unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    assert_eq!(reached.unwrap(), b"MZ");
}

thread_local! {
    /// The folders [`RECORDED`] made, with their permissions.
    static MADE: std::cell::RefCell<Vec<(PathBuf, String)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}
fn create_recorded(path: &Path, sddl: &str) -> std::io::Result<()> {
    std::fs::create_dir(path)?;
    MADE.with_borrow_mut(|made| made.push((path.to_path_buf(), sddl.to_string())));
    Ok(())
}
/// As [`secured`] does, refuses a folder the host did not make.
fn check_recorded(path: &Path, user: &[&str]) -> Result<std::fs::File> {
    ensure!(
        MADE.with_borrow(|made| made.iter().any(|(made, _)| made == path)),
        "{} is owned by {SID}",
        path.display()
    );
    open(path, user)
}
const RECORDED: Steps = Steps {
    create: create_recorded,
    check: check_recorded,
};

#[test]
fn the_copys_folder_is_the_hosts_and_any_other_is_refused() {
    let data = tempfile::tempdir().unwrap();
    let host = host_folder();
    let copy = ensure_with(data.path(), host.path(), SID, None, &RECORDED).unwrap();
    // Made by the host: the user's folder they can only list, the copy
    // they fill and can't move.
    let folder = pinned::user_folder(data.path(), SID).unwrap();
    let made = MADE.with_borrow(|made| made.clone());
    assert!(made.contains(&(folder.clone(), sid_sddl(SID))), "{made:?}");
    assert!(made.contains(&(copy.clone(), user_sddl(SID))), "{made:?}");
    MADE.with_borrow_mut(|made| made.retain(|(path, _)| *path != copy));

    // One the host did not make is refused, and left as it is.
    std::fs::remove_dir_all(&copy).unwrap();
    std::fs::create_dir(&copy).unwrap();
    std::fs::write(copy.join("old.txt"), b"x").unwrap();
    let error = ensure_with(data.path(), host.path(), SID, None, &RECORDED).unwrap_err();
    assert!(format!("{error:#}").contains("is owned by"), "{error:#}");
    assert_eq!(names(&copy), ["old.txt"]);

    // So is a link in its place, and nothing is written through it.
    let elsewhere = tempfile::tempdir().unwrap();
    std::fs::remove_dir_all(&copy).unwrap();
    junction(elsewhere.path(), &copy);
    assert!(ensure_with(data.path(), host.path(), SID, None, &RECORDED).is_err());
    assert!(names(elsewhere.path()).is_empty());
}

/// Open `path` for writing its data, as a user setting a link on it
/// does.
fn open_to_write(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_WRITE_DATA,
    };
    std::fs::OpenOptions::new()
        .access_mode(FILE_WRITE_DATA.0)
        .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0)
        .custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0)
        .open(path)
}

#[test]
fn the_user_never_makes_their_folder_or_a_held_copy_a_link() {
    use crate::user_folder::tests::mount;
    let sid = crate::process::own_sid().unwrap();
    // Standing in for the user: no rights by this account's name.
    let user = crate::user_folder::tests::without_own_sid();
    let root = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    // As the host makes them, with this account in place of
    // Administrators and Everyone in place of the user.
    let ours = |sddl: String| sddl.replace("BA", &sid);
    let theirs = root.path().join(SID);
    crate::process::create_folder(&theirs, &ours(sid_sddl("WD"))).unwrap();
    // An empty folder the user may add to, they can make a link: why
    // their own folder doesn't let them add to it.
    let open = root.path().join("open");
    crate::process::create_folder(&open, &ours(user_sddl("WD"))).unwrap();
    let (writable, linked) = as_user(Some(&user), || {
        (
            open_to_write(&open).map(drop),
            mount(&open, elsewhere.path()),
        )
    })
    .unwrap();
    writable.unwrap();
    linked.unwrap();
    assert!(
        std::fs::symlink_metadata(&open)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    // Their own folder: neither, nor adding to it.
    let (writable, linked, added) = as_user(Some(&user), || {
        (
            open_to_write(&theirs).map(drop),
            mount(&theirs, elsewhere.path()),
            std::fs::write(theirs.join("x"), b"x"),
        )
    })
    .unwrap();
    for refused in [writable, linked, added] {
        assert_eq!(
            refused.unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
    }
    assert!(std::fs::symlink_metadata(&theirs).unwrap().is_dir());
    // An empty copy, held as the host holds it: not while it is held.
    let copy = theirs.join("0123456789abcdef");
    crate::process::create_folder(&copy, &ours(user_sddl("WD"))).unwrap();
    let held = open_held(&copy).unwrap();
    let (writable, linked) = as_user(Some(&user), || {
        (
            open_to_write(&copy).map(drop),
            mount(&copy, elsewhere.path()),
        )
    })
    .unwrap();
    for refused in [writable, linked] {
        // A sharing violation.
        assert_eq!(refused.unwrap_err().raw_os_error(), Some(32));
    }
    // Files can still be added and removed meanwhile.
    let filled = as_user(Some(&user), || {
        std::fs::write(copy.join("Settings.ini"), b"x")
            .and_then(|()| std::fs::remove_file(copy.join("Settings.ini")))
    })
    .unwrap();
    filled.unwrap();
    drop(held);
    let linked = as_user(Some(&user), || mount(&copy, elsewhere.path())).unwrap();
    linked.unwrap();
    assert!(names(elsewhere.path()).is_empty());
}

#[test]
fn a_file_windows_refuses_to_remove_says_why_and_a_folder_link_goes() {
    let root = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    std::fs::write(elsewhere.path().join("kept.txt"), b"x").unwrap();
    let file = root.path().join("checkDotNet472Ver.bat");
    std::fs::write(&file, b"x").unwrap();
    let link = root.path().join("linked");
    junction(elsewhere.path(), &link);
    icacls(&file, &["/deny", "*S-1-1-0:(D)"]);
    icacls(root.path(), &["/deny", "*S-1-1-0:(DC)"]);
    let refused = remove_entry(&file);
    icacls(root.path(), &["/reset", "/T", "/C", "/Q"]);
    // Windows' own refusal, not a second try at a folder of that name
    // ("The directory name is invalid", #56).
    let error = refused.unwrap_err();
    assert_eq!(
        error.downcast_ref::<std::io::Error>().map(|e| e.kind()),
        Some(std::io::ErrorKind::PermissionDenied),
        "{error:#}"
    );
    assert!(format!("{error:#}").starts_with("removing "), "{error:#}");
    remove_entry(&file).unwrap();
    // A junction goes itself; what it led to stays.
    remove_entry(&link).unwrap();
    assert!(!link.exists() && !file.exists());
    assert_eq!(names(elsewhere.path()), ["kept.txt"]);
}

#[test]
fn the_user_changes_what_is_in_their_copy_but_never_moves_it() {
    let sid = crate::process::own_sid().unwrap();
    // Standing in for the user: no rights by this account's name.
    let user = crate::user_folder::tests::without_own_sid();
    let root = tempfile::tempdir().unwrap();
    // As the host makes both, with this account in place of
    // Administrators and Everyone in place of the user.
    let theirs = root.path().join(SID);
    crate::process::create_folder(&theirs, &sid_sddl("WD").replace("BA", &sid)).unwrap();
    let copy = theirs.join("0123456789abcdef");
    crate::process::create_folder(&copy, &user_sddl("WD").replace("BA", &sid)).unwrap();
    let (filled, emptied, renamed, removed) = as_user(Some(&user), || {
        let filled = (|| -> std::io::Result<()> {
            std::fs::write(copy.join("NucleusCoop.exe"), b"MZ")?;
            std::fs::create_dir(copy.join("gui"))?;
            std::fs::write(copy.join("gui").join("theme.ini"), b"x")
        })();
        (
            filled,
            empty(&copy),
            std::fs::rename(&copy, theirs.join("moved")),
            std::fs::remove_dir(&copy),
        )
    })
    .unwrap();
    filled.unwrap();
    emptied.unwrap();
    assert!(names(&copy).is_empty());
    for refused in [renamed, removed] {
        assert_eq!(
            refused.unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
    }
    // The host removes an emptied one.
    std::fs::remove_dir(&copy).unwrap();
}

/// A folder `key` in `root` holding a program as NucleusCoop.exe.
fn program_copy(root: &Path) -> (PathBuf, PathBuf) {
    let key = root.join("0123456789abcdef");
    std::fs::create_dir(&key).unwrap();
    let program = key.join(pinned::EXECUTABLE);
    let system = std::env::var("SystemRoot").unwrap();
    std::fs::copy(Path::new(&system).join(r"System32\where.exe"), &program).unwrap();
    (key, program)
}

#[test]
fn the_copy_and_its_program_are_held_while_nucleus_starts() {
    let root = tempfile::tempdir().unwrap();
    let (key, program) = program_copy(root.path());
    let moved = root.path().join("moved");
    let held = hold_with(&key, SID, open).unwrap();
    // Nobody can open the folder to write (to make it a link).
    assert_eq!(open_to_write(&key).unwrap_err().raw_os_error(), Some(32));
    // Nobody can move or delete either, nor write the program.
    assert!(std::fs::rename(&key, &moved).is_err());
    assert!(std::fs::remove_file(&program).is_err());
    assert!(std::fs::rename(&program, key.join("other.exe")).is_err());
    assert!(
        std::fs::OpenOptions::new()
            .write(true)
            .open(&program)
            .is_err()
    );
    // Windows starts it all the same, in that folder.
    let status = std::process::Command::new(&program)
        .arg("/?")
        .current_dir(&key)
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "{status}");
    drop(held);
    std::fs::rename(&key, &moved).unwrap();
}

#[test]
fn a_program_given_another_name_is_copied_again_and_then_starts() {
    let data = tempfile::tempdir().unwrap();
    let host = host_folder();
    let copy = ensure_with(data.path(), host.path(), SID, None, &TEST).unwrap();
    // A hard link the user (or a dedupe tool) made: refused to start.
    let elsewhere = tempfile::tempdir().unwrap();
    let other = elsewhere.path().join("other.exe");
    std::fs::hard_link(copy.join(pinned::EXECUTABLE), &other).unwrap();
    let error = format!("{:#}", hold_with(&copy, SID, open).err().unwrap());
    assert!(error.contains("has 2 names"), "{error}");
    // The next stream copies it again, with one name.
    assert_eq!(
        ensure_with(data.path(), host.path(), SID, None, &TEST).unwrap(),
        copy
    );
    assert_eq!(names_of(&copy.join(pinned::EXECUTABLE)).unwrap(), 1);
    hold_with(&copy, SID, open).unwrap();
    // The other name is left as it was.
    assert_eq!(std::fs::read(&other).unwrap(), b"MZ nucleus");
}

#[test]
fn a_link_in_place_of_the_copy_or_its_program_is_never_started() {
    let root = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let (real, program) = program_copy(elsewhere.path());
    // The copy's folder a junction to another.
    let key = root.path().join("0123456789abcdef");
    junction(&real, &key);
    let error = format!("{:#}", hold_with(&key, SID, secured).err().unwrap());
    assert!(error.contains("is a link to elsewhere"), "{error}");
    std::fs::remove_dir(&key).unwrap();
    // Its program a link (a file link needs a privilege or developer
    // mode), a junction, a folder or a hard link.
    std::fs::create_dir(&key).unwrap();
    let link = key.join(pinned::EXECUTABLE);
    if std::os::windows::fs::symlink_file(&program, &link).is_ok() {
        let error = format!("{:#}", hold_with(&key, SID, open).err().unwrap());
        assert!(error.contains("is a link or a folder"), "{error}");
        std::fs::remove_file(&link).unwrap();
    }
    junction(&real, &link);
    let error = format!("{:#}", hold_with(&key, SID, open).err().unwrap());
    assert!(error.contains("is not a program file"), "{error}");
    std::fs::remove_dir(&link).unwrap();
    std::fs::create_dir(&link).unwrap();
    assert!(hold_with(&key, SID, open).is_err());
    std::fs::remove_dir(&link).unwrap();
    std::fs::hard_link(&program, &link).unwrap();
    let error = format!("{:#}", hold_with(&key, SID, open).err().unwrap());
    assert!(error.contains("has 2 names"), "{error}");
    std::fs::remove_file(&link).unwrap();
    std::fs::copy(&program, &link).unwrap();
    hold_with(&key, SID, open).unwrap();
    // A program that opened elsewhere than in the folder held.
    let held = open_held(&key).unwrap();
    let error = in_folder(&held, &open_program(&program).unwrap()).unwrap_err();
    assert!(format!("{error:#}").contains("not in"), "{error:#}");
    in_folder(&held, &open_program(&link).unwrap()).unwrap();
}

#[test]
fn the_copys_handlers_are_the_hosts() {
    let host = tempfile::tempdir().unwrap();
    let copy = tempfile::tempdir().unwrap();
    let handlers = host.path().join("handlers");
    std::fs::create_dir_all(handlers.join("Puzzle")).unwrap();
    std::fs::write(handlers.join("Puzzle.js"), b"Game.GUID = 'Puzzle';").unwrap();
    // Rubylight's own: the launched game's, and another account's game's.
    let ours = format!("{}\n// Written for S-1-5-21-1-2-3-1001\n", generic::MARKER);
    let theirs_too = format!("{}\n// Written for S-1-5-21-1-2-3-1002\n", generic::MARKER);
    std::fs::write(handlers.join("Rubylight-game.js"), &ours).unwrap();
    std::fs::write(handlers.join("Rubylight-other.js"), &theirs_too).unwrap();
    // A hub handler named like ours is still a hub handler.
    std::fs::write(handlers.join("Rubylight-hub.js"), b"Game.GUID = 'Hub';").unwrap();
    std::fs::write(handlers.join("Puzzle").join("x.dll"), b"x").unwrap();
    std::fs::write(handlers.join("notes.txt"), b"x").unwrap();
    let theirs = copy.path().join("handlers");
    std::fs::create_dir_all(&theirs).unwrap();
    // Gone from the host's: removed. Changed: replaced.
    std::fs::write(theirs.join("Removed.JS"), b"old").unwrap();
    std::fs::write(theirs.join("Puzzle.js"), b"old").unwrap();
    std::fs::write(theirs.join("readme.txt"), b"kept").unwrap();
    // An earlier launch's game's handler of ours: gone on this one.
    std::fs::write(theirs.join("Rubylight-other.js"), &theirs_too).unwrap();
    let mirrored = mirror_handlers(host.path(), copy.path(), "rubylight-GAME.js", None).unwrap();
    assert_eq!(mirrored, 3);
    let mut names: Vec<_> = std::fs::read_dir(&theirs)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "Puzzle.js",
            "Rubylight-game.js",
            "Rubylight-hub.js",
            "readme.txt"
        ]
    );
    assert_eq!(
        std::fs::read(theirs.join("Puzzle.js")).unwrap(),
        b"Game.GUID = 'Puzzle';"
    );
    assert_eq!(
        std::fs::read_to_string(theirs.join("Rubylight-game.js")).unwrap(),
        ours
    );
    // A host's copy without handlers leaves the copy none.
    std::fs::remove_dir_all(&handlers).unwrap();
    assert_eq!(
        mirror_handlers(host.path(), copy.path(), "Rubylight-game.js", None).unwrap(),
        0
    );
    assert!(!theirs.join("Puzzle.js").exists());
}

#[test]
fn ours_is_in_the_hosts_nucleus_and_leaves_the_copy_after_the_hosts_sweep() {
    let host = tempfile::tempdir().unwrap();
    let copy = tempfile::tempdir().unwrap();
    let puzzle = Setup::new(r"D:\Puzzle\Puzzle.exe", "Puzzle", None);
    let ice = Setup::new(r"D:\Ice\Ice.exe", "Ice", None);
    let first = write_own(host.path(), copy.path(), &puzzle, None).unwrap();
    // The host's Nucleus has it too, so its sweep knows the game.
    let hosts = files::catalog(host.path(), true).unwrap();
    assert!(hosts.handlers.iter().any(|h| h.own && h.guid == first.guid));
    assert!(hosts.games.iter().any(|game| game.guid == first.guid));
    let theirs = copy.path().join("handlers");
    assert_eq!(names(&theirs), [puzzle.file()]);
    // Another game's launch takes only that game's into the copy: the
    // host's Nucleus keeps both.
    write_own(host.path(), copy.path(), &ice, None).unwrap();
    assert_eq!(names(&theirs), [ice.file()]);
    let mut both = [puzzle.file(), ice.file()];
    both.sort();
    assert_eq!(names(&host.path().join("handlers")), both);
    // The host's sweep removes it and its game; the copy keeps the
    // launched game's.
    files::remove_own(host.path(), &first).unwrap();
    remove_game(copy.path(), &first.guid, None).unwrap();
    write_own(host.path(), copy.path(), &ice, None).unwrap();
    assert_eq!(names(&theirs), [ice.file()]);
    let copys = files::catalog(copy.path(), true).unwrap();
    assert!(copys.games.iter().all(|game| game.guid != first.guid));
}

#[test]
fn the_copys_nucleus_lists_our_game_from_its_own_list() {
    let host = tempfile::tempdir().unwrap();
    let copy = tempfile::tempdir().unwrap();
    // A game whose .exe is there, as Nucleus 2.4.2 lists only those.
    let game = tempfile::tempdir().unwrap();
    let exe = game.path().join("Ice.exe");
    std::fs::write(&exe, b"MZ").unwrap();
    let ice = Setup::new(exe.to_str().unwrap(), "Ice", None);
    let own = write_own(host.path(), copy.path(), &ice, None).unwrap();
    // The copy's Nucleus reads its own content\userprofile.json, never
    // the host's or %APPDATA%'s: the game is there with its handler.
    assert_eq!(
        files::listed_by_nucleus(copy.path()).unwrap(),
        std::slice::from_ref(&own.guid)
    );
    assert!(files::list(copy.path()).is_file());
    // A list the copy's Nucleus wrote in a way Rubylight can't edit
    // refuses the launch before anything is written.
    let other = Setup::new(r"D:\Puzzle\Puzzle.exe", "Puzzle", None);
    std::fs::write(files::list(copy.path()), b"[not a list").unwrap();
    assert!(write_own(host.path(), copy.path(), &other, None).is_err());
    assert!(!host.path().join("handlers").join(other.file()).exists());
}

#[test]
fn a_linked_handler_is_not_read() {
    let host = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(elsewhere.path().join("x.js")).unwrap();
    let handlers = host.path().join("handlers");
    std::fs::create_dir_all(&handlers).unwrap();
    junction(&elsewhere.path().join("x.js"), &handlers.join("linked.js"));
    let copy = tempfile::tempdir().unwrap();
    assert_eq!(
        mirror_handlers(host.path(), copy.path(), "linked.js", None).unwrap(),
        0
    );
}

#[test]
fn a_file_larger_than_the_copy_chunk_is_copied_whole_with_its_time() {
    let data = tempfile::tempdir().unwrap();
    let host = host_folder();
    // Not a multiple of the chunk: the last read is short.
    let big: Vec<u8> = (0..COPY_CHUNK * 2 + 12_345)
        .map(|i| (i % 251) as u8)
        .collect();
    std::fs::write(host.path().join("NucleusCoop.exe"), &big).unwrap();
    let copy = ensure_with(data.path(), host.path(), SID, None, &TEST).unwrap();
    let copied = copy.join("NucleusCoop.exe");
    assert_eq!(std::fs::read(&copied).unwrap(), big);
    assert_eq!(
        std::fs::metadata(&copied).unwrap().modified().unwrap(),
        std::fs::metadata(host.path().join("NucleusCoop.exe"))
            .unwrap()
            .modified()
            .unwrap()
    );
    assert!(differs(&copy, &Listing::of(host.path()).unwrap().text).is_none());
}

/// A reader that hands out at most 3 bytes a call and is interrupted
/// before each.
struct Choppy<'a>(&'a [u8], bool);
impl std::io::Read for Choppy<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.1 = !self.1;
        if self.1 {
            return Err(std::io::ErrorKind::Interrupted.into());
        }
        let n = buffer.len().min(3).min(self.0.len());
        buffer[..n].copy_from_slice(&self.0[..n]);
        self.0 = &self.0[n..];
        Ok(n)
    }
}

#[test]
fn a_copy_goes_on_after_short_and_interrupted_reads() {
    let mut to = Vec::new();
    copy_through(&mut Choppy(b"0123456789", false), &mut to, &mut [0; 4]).unwrap();
    assert_eq!(to, b"0123456789");
}
