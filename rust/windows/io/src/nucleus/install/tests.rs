//! Tests for Nucleus Co-op install verification, trust boundaries, and leftover restore folders.
use super::*;

mod drives_release;

/// The program files of a test's release, as it ships them.
const FILES: [(&str, &[u8]); 4] = [
    ("NucleusCoop.exe", b"MZ 2.4.2"),
    ("NucleusCoop.exe.config", b"<configuration/>"),
    ("Nucleus.Gaming.dll", b"MZ gaming"),
    (r"utils\Hooks\EasyHook64.dll", b"MZ hook"),
];

fn sha(bytes: &[u8]) -> String {
    pinned::sha256(&mut &bytes[..], u64::MAX).unwrap()
}

/// [`FILES`], the Settings.ini the release ships (Nucleus' to change) and an
/// empty folder.
fn manifest() -> Manifest {
    let mut text = String::new();
    for (path, bytes) in FILES {
        text += &format!("{} {} {path}\n", sha(bytes), bytes.len());
    }
    text += &format!("{} 3 Settings.ini\nutils\\User\\\n", sha(b"abc"));
    Manifest::parse(&text).unwrap()
}

/// The owner what this test makes gets: the Administrators group when it
/// runs elevated, else its account.
fn owner() -> String {
    crate::process::default_owner().unwrap()
}

/// Permissions for [`owner`], SYSTEM and Administrators only, and `extra`.
fn sddl(extra: &str) -> String {
    let owner = owner();
    format!("O:{owner}D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;{owner}){extra}")
}

/// [`TRUSTED`] and the test's [`owner`] trusted, the real drive, and a
/// memory of drives of the test's own.
fn checks<'a>(release: &'a Manifest, top: &'a Path) -> Checks<'a> {
    let me = owner();
    Checks {
        trusts: Box::new(move |_, sid| TRUSTED.contains(&sid) || sid == me),
        top: Some(top),
        release,
        volume: Box::new(volume_of),
        volumes: Box::leak(Box::default()),
    }
}

/// `path` by its long name, as Windows resolves it: a temporary folder may
/// be named by an 8.3 short name (`RUNNER~1`).
fn long(path: &Path) -> PathBuf {
    PathBuf::from(normal(std::fs::canonicalize(path).unwrap().as_os_str()))
}

/// A temporary folder (`top`) holding `Programs\Nucleus Co-op`, the test
/// release unpacked as an administrator would: with a handler, a changed
/// Settings.ini, a game instance and only SYSTEM, Administrators and the
/// test's [`owner`] able to change any of it.
struct Tree {
    _dir: tempfile::TempDir,
    top: PathBuf,
    folder: PathBuf,
}
fn tree() -> Tree {
    let dir = tempfile::tempdir().unwrap();
    let top = long(dir.path());
    let folder = top.join("Programs").join("Nucleus Co-op");
    for (path, bytes) in FILES {
        let path = folder.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    std::fs::write(folder.join("Settings.ini"), b"[Misc]\nchanged=1\n").unwrap();
    std::fs::create_dir_all(folder.join("handlers")).unwrap();
    std::fs::write(folder.join(r"handlers\Game.js"), b"var Game = {};").unwrap();
    std::fs::create_dir_all(folder.join(r"content\Game\Instance0")).unwrap();
    crate::process::restrict(&top, &sddl("")).unwrap();
    Tree {
        _dir: dir,
        top,
        folder,
    }
}

/// `tree`'s release copied to an earlier Rubylight's install below `data`.
fn earlier(tree: &Tree) -> PathBuf {
    let data = tree.top.join("data");
    let folder = pinned::folder(&data);
    for (path, bytes) in FILES {
        let path = folder.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    crate::process::restrict(&data, &sddl("")).unwrap();
    data
}

fn check(tree: &Tree, folder: &Path) -> Result<PathBuf> {
    let release = manifest();
    chosen(folder, &checks(&release, &tree.top)).map(|checked| checked.folder)
}

fn problem_of(error: &anyhow::Error) -> Option<Problem> {
    error
        .downcast_ref::<Unusable>()
        .map(|unusable| unusable.problem)
}

fn refused(tree: &Tree, folder: &Path) -> (Option<Problem>, String) {
    let error = check(tree, folder).unwrap_err();
    (problem_of(&error), format!("{error:#}"))
}

fn junction(link: &Path, target: &Path) {
    let made = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .unwrap();
    assert!(made.status.success(), "{made:?}");
}

#[test]
fn the_chosen_release_is_checked_whole_and_named_as_windows_resolved_it() {
    let tree = tree();
    assert_eq!(check(&tree, &tree.folder).unwrap(), tree.folder);
    // A pasted path's separators and its trailing one don't matter.
    let pasted = format!("{}\\", tree.folder.display()).replace('\\', "/");
    assert_eq!(check(&tree, Path::new(&pasted)).unwrap(), tree.folder);
    let upper = tree.folder.to_string_lossy().to_uppercase();
    assert_eq!(check(&tree, Path::new(&upper)).unwrap(), tree.folder);
}

#[test]
fn a_game_instance_folder_others_can_change_is_refused_and_links_inside_it_are_not() {
    // Nucleus' instances link to the game's own files: not walked.
    let tree = tree();
    let instance = tree.folder.join(r"content\Game");
    junction(&instance.join("Instance1"), &tree.top);
    assert_eq!(check(&tree, &tree.folder).unwrap(), tree.folder);
    // A folder its user may change: they could plant a junction or a DLL
    // in it, which the elevated Nucleus follows or runs.
    let tree = self::tree();
    let instance = tree.folder.join(r"content\Game");
    crate::process::restrict(&instance, &sddl("(A;OICI;0x1301bf;;;AU)")).unwrap();
    let (problem, error) = refused(&tree, &tree.folder);
    assert_eq!(problem, Some(Problem::Untrusted), "{error}");
    assert!(
        error.contains(&format!(
            "{} can be changed by S-1-5-11",
            instance.display()
        )),
        "{error}"
    );
    // A link in its place.
    let tree = self::tree();
    let instance = tree.folder.join(r"content\Game");
    std::fs::remove_dir_all(&instance).unwrap();
    junction(&instance, &tree.top);
    let (problem, error) = refused(&tree, &tree.folder);
    assert_eq!(problem, Some(Problem::Untrusted), "{error}");
    assert!(
        error.contains(&format!("{} is a link to elsewhere", instance.display())),
        "{error}"
    );
}

#[test]
fn a_dll_config_or_handler_others_can_change_is_refused_by_name() {
    for name in [
        "Nucleus.Gaming.dll",
        "NucleusCoop.exe.config",
        r"handlers\Game.js",
        r"utils\Hooks",
    ] {
        let tree = tree();
        let path = tree.folder.join(name);
        crate::process::restrict(&path, &sddl("(A;;0x1301bf;;;AU)")).unwrap();
        let (problem, error) = refused(&tree, &tree.folder);
        assert_eq!(problem, Some(Problem::Untrusted), "{error}");
        assert!(
            error.contains(&format!("{} can be changed by S-1-5-11", path.display())),
            "{error}"
        );
        assert!(
            error.contains("Unpack the release into a new folder under C:\\Program Files"),
            "{error}"
        );
    }
}

#[test]
fn a_file_or_folder_an_untrusted_account_owns_is_refused_in_an_otherwise_good_folder() {
    // As a file unpacked into Downloads and moved keeps its user as its
    // owner: here the test's own account, trusted everywhere but there, so
    // this runs unelevated and shows each check passes the owner it read.
    let me = owner();
    for name in [
        "",
        "NucleusCoop.exe",
        "Nucleus.Gaming.dll",
        "NucleusCoop.exe.config",
        "handlers",
        r"handlers\Game.js",
        r"content\Game",
    ] {
        let tree = tree();
        let release = manifest();
        let planted = if name.is_empty() {
            tree.folder.clone()
        } else {
            tree.folder.join(name)
        };
        let mut checks = checks(&release, &tree.top);
        let (mine, at) = (me.clone(), planted.clone());
        checks.trusts = Box::new(move |path, sid| {
            (TRUSTED.contains(&sid) || sid == mine) && !(sid == mine && path == at)
        });
        let error = chosen(&tree.folder, &checks).err().unwrap();
        assert_eq!(
            problem_of(&error),
            Some(Problem::Untrusted),
            "{name}: {error:#}"
        );
        let error = format!("{error:#}");
        assert!(
            error.contains(&format!("{} is owned by {me}", planted.display())),
            "{name}: {error}"
        );
    }
}

#[test]
fn a_link_in_the_folder_or_on_its_way_is_refused_never_followed() {
    // A user's folder swapped for a junction would point the host's writes
    // elsewhere.
    let tree = tree();
    let handlers = tree.folder.join("handlers");
    std::fs::remove_dir_all(&handlers).unwrap();
    junction(&handlers, &tree.top);
    let (problem, error) = refused(&tree, &tree.folder);
    assert_eq!(problem, Some(Problem::Untrusted), "{error}");
    assert!(
        error.contains(&format!("{} is a link to elsewhere", handlers.display())),
        "{error}"
    );
    // A junction on the way: checked through it, it could lead elsewhere by
    // the time Nucleus runs.
    let tree = self::tree();
    let link = tree.top.join("Link");
    junction(&link, &tree.top.join("Programs"));
    let through = link.join("Nucleus Co-op");
    let (problem, error) = refused(&tree, &through);
    assert_eq!(problem, Some(Problem::Untrusted), "{error}");
    assert!(
        error.contains(&format!(
            "leads to {} through a link or a short name",
            tree.folder.display()
        )),
        "{error}"
    );
    // The folder itself.
    let (problem, error) = refused(&tree, &tree.top.join("Link"));
    assert_eq!(problem, Some(Problem::Untrusted), "{error}");
    assert!(
        error.contains("is a link to elsewhere: choose the folder itself"),
        "{error}"
    );
}

#[test]
fn a_folder_whose_parent_or_grandparent_others_can_move_is_refused() {
    // Delete children on the grandparent, delete on the parent: either
    // lets its user move the checked folder aside and put another there.
    for (above, ace) in [("", "(A;;0x40;;;AU)"), ("Programs", "(A;;0x10000;;;AU)")] {
        let tree = tree();
        let movable = tree.top.join(above);
        crate::process::restrict(&movable, &sddl(ace)).unwrap();
        let below = if above.is_empty() {
            tree.top.join("Programs")
        } else {
            tree.folder.clone()
        };
        crate::process::restrict(&below, &sddl("")).unwrap();
        let (problem, error) = refused(&tree, &tree.folder);
        assert_eq!(problem, Some(Problem::Untrusted), "{error}");
        assert!(
            error.contains(&format!(
                "{} can be moved or renamed by S-1-5-11",
                normal(movable.as_os_str())
            )),
            "{error}"
        );
    }
}

/// The capability Windows 11 gives an entry on `C:\` (RL56's PC and ours).
const C_CAPABILITY: &str = "S-1-15-3-65536-1888954469-739942743-1668119174-2468466756-4239452838-1296943325-355587736-700089176";

#[test]
fn an_app_package_or_capability_entry_above_or_in_the_folder_is_no_refusal() {
    // Such an entry grants nothing its user's own SIDs aren't allowed too,
    // here even delete children on the folder above and writing a program.
    let tree = tree();
    crate::process::restrict(
        &tree.top,
        &sddl(&format!("(A;;0x1f01ff;;;{C_CAPABILITY})(A;;0x40;;;AC)")),
    )
    .unwrap();
    crate::process::restrict(&tree.top.join("Programs"), &sddl("")).unwrap();
    crate::process::restrict(
        &tree.folder.join("Nucleus.Gaming.dll"),
        &sddl(&format!("(A;;0x1301bf;;;{C_CAPABILITY})")),
    )
    .unwrap();
    assert_eq!(check(&tree, &tree.folder).unwrap(), tree.folder);
    // A user's entry beside them still is.
    crate::process::restrict(
        &tree.top,
        &sddl(&format!("(A;;0x1f01ff;;;{C_CAPABILITY})(A;;0x40;;;AU)")),
    )
    .unwrap();
    let (problem, error) = refused(&tree, &tree.folder);
    assert_eq!(problem, Some(Problem::Untrusted), "{error}");
    assert!(
        error.contains("can be moved or renamed by S-1-5-11"),
        "{error}"
    );
}

/// What [`left_in_checked`] accepts, by path, or the first refusal.
fn left(data: &Path, configured: &str, checks: &Checks) -> Vec<Result<PathBuf, String>> {
    left_in_checked(data, configured, checks)
        .into_iter()
        .map(|left| {
            left.map(|left| left.folder().to_path_buf())
                .map_err(|error| format!("{error:#}"))
        })
        .collect()
}

#[test]
fn settings_are_put_back_from_a_folder_nucleus_no_longer_runs_from() {
    let tree = tree();
    let release = manifest();
    let checks = checks(&release, &tree.top);
    let data = earlier(&tree);
    // Another release, with a handler others can change: no longer run from.
    std::fs::write(tree.folder.join(pinned::EXECUTABLE), b"MZ 2.3.1").unwrap();
    crate::process::restrict(&tree.folder.join("handlers"), &sddl("(A;;0x1301bf;;;AU)")).unwrap();
    assert!(check(&tree, &tree.folder).is_err());
    let configured = tree.folder.to_string_lossy().into_owned();
    assert_eq!(
        left(&data, &configured, &checks),
        [Ok(tree.folder.clone()), Ok(pinned::folder(&data))]
    );
    // Named twice, listed once; one not there is skipped.
    let earlier_folder = pinned::folder(&data).to_string_lossy().to_uppercase();
    assert_eq!(left(&data, &earlier_folder, &checks).len(), 1);
    let gone = tree.top.join("Gone");
    assert!(left(&gone, "", &checks).is_empty());
    // Held open until the restore is done: it can't be moved aside meanwhile.
    let held = left_in_checked(&data, &configured, &checks);
    let aside = tree.top.join("Programs").join("Aside");
    assert!(std::fs::rename(&tree.folder, &aside).is_err());
    drop(held);
    std::fs::rename(&tree.folder, &aside).unwrap();
    std::fs::rename(&aside, &tree.folder).unwrap();
    // A folder others can change could hold a file nobody trusted wrote.
    crate::process::restrict(&tree.folder, &sddl("(A;;0x1301bf;;;AU)")).unwrap();
    let folders = left(&data, &configured, &checks);
    let error = folders[0].as_ref().unwrap_err();
    assert!(
        error.contains("doesn't put back the settings a Nucleus Co-op session left in"),
        "{error}"
    );
    assert!(error.contains("can be changed by S-1-5-11"), "{error}");
    assert_eq!(folders[1], Ok(pinned::folder(&data)));
}

#[test]
fn settings_are_never_put_back_from_a_file_others_can_change() {
    let tree = tree();
    let release = manifest();
    let checks = checks(&release, &tree.top);
    let data = tree.top.join("none");
    let configured = tree.folder.to_string_lossy().into_owned();
    // A journal others could have written: its values would go to the hive.
    for name in [
        "Rubylight-settings.json",
        "Rubylight-layers-S-1-5-21-7.json",
        "Settings.ini",
        "Settings.ini.rubylight",
    ] {
        let file = tree.folder.join(name);
        std::fs::write(&file, b"{}").unwrap();
        assert_eq!(left(&data, &configured, &checks), [Ok(tree.folder.clone())]);
        crate::process::restrict(&file, &sddl("(A;;0x1301bf;;;AU)")).unwrap();
        let error = left(&data, &configured, &checks).remove(0).unwrap_err();
        assert!(
            error.contains(name) && error.contains("can be changed by S-1-5-11"),
            "{error}"
        );
        std::fs::remove_file(&file).unwrap();
    }
    // Nucleus' own files there are no matter: a restore never reads them.
    let other = tree.folder.join("Rubylight-notes.txt");
    std::fs::write(&other, b"").unwrap();
    crate::process::restrict(&other, &sddl("(A;;0x1301bf;;;AU)")).unwrap();
    assert_eq!(left(&data, &configured, &checks), [Ok(tree.folder.clone())]);
}

#[test]
fn a_network_folder_is_refused_before_it_is_reached() {
    let tree = tree();
    let release = manifest();
    let checks = checks(&release, &tree.top);
    let data = tree.top.join("none");
    // A network path is refused before anything reaches out to it.
    let error = left(&data, r"\\nowhere.invalid\share\Nucleus", &checks)
        .remove(0)
        .unwrap_err();
    assert!(error.contains("doesn't put back"), "{error}");
    // Not there: skipped quietly.
    let gone = tree.top.join("Programs").join("Gone");
    assert!(left(&data, &gone.to_string_lossy(), &checks).is_empty());
}

#[test]
fn settings_are_never_put_back_from_a_folder_that_can_be_swapped() {
    let tree = tree();
    let release = manifest();
    let checks = checks(&release, &tree.top);
    let data = tree.top.join("none");
    let configured = tree.folder.to_string_lossy().into_owned();
    // Its parent others can move: the folder could be moved aside and another
    // put in its place after the check.
    crate::process::restrict(&tree.top.join("Programs"), &sddl("(A;;0x40;;;AU)")).unwrap();
    crate::process::restrict(&tree.folder, &sddl("")).unwrap();
    let folders = left(&data, &configured, &checks);
    let error = folders[0].as_ref().unwrap_err();
    assert!(
        error.contains("can be moved or renamed by S-1-5-11"),
        "{error}"
    );
    crate::process::restrict(&tree.top.join("Programs"), &sddl("")).unwrap();
    assert_eq!(left(&data, &configured, &checks), [Ok(tree.folder.clone())]);
    // A link, to a folder that would pass, or on the way to it.
    let link = tree.top.join("Link");
    junction(&link, &tree.folder);
    let error = left(&data, &link.to_string_lossy(), &checks)
        .remove(0)
        .unwrap_err();
    assert!(error.contains("is a link to elsewhere"), "{error}");
    let through = tree.top.join("Through");
    junction(&through, &tree.top.join("Programs"));
    let below = through.join("Nucleus Co-op");
    let error = left(&data, &below.to_string_lossy(), &checks)
        .remove(0)
        .unwrap_err();
    assert!(error.contains("through a link or a short name"), "{error}");
    // A device path, or one through `..`.
    for path in [
        format!(r"\\?\{}", tree.folder.display()),
        format!(r"{}\..\Nucleus Co-op", tree.folder.display()),
    ] {
        assert!(std::fs::symlink_metadata(&path).is_ok(), "{path}");
        assert!(left(&data, &path, &checks)[0].is_err(), "{path}");
    }
}
