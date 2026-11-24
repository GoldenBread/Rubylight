//! Administrator ACL checks, archive extraction, digest verification, and cache trust/locking tests for Neural Rendering.
use super::common::*;
use super::common::{install, remove};
use butterpollo_session::neural::{self, Download, Mode, Release};
use butterpollo_stream::neural::folder::{Request, Response, Writer, handle};
use butterpollo_stream::neural::*;
use butterpollo_win_io::process::Hold;
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

/// Everyone denied adding to `folder` until dropped, as Program Files is to users.
struct Unwritable<'a>(&'a Path);
impl<'a> Unwritable<'a> {
    fn new(folder: &'a Path) -> Self {
        let denied = std::process::Command::new("icacls")
            .arg(folder)
            .args(["/deny", "*S-1-1-0:(WD,AD)"])
            .output()
            .unwrap();
        assert!(denied.status.success(), "{denied:?}");
        Self(folder)
    }
}
impl Drop for Unwritable<'_> {
    fn drop(&mut self) {
        let _ = std::process::Command::new("icacls")
            .arg(self.0)
            .args(["/remove:d", "*S-1-1-0"])
            .output();
    }
}

/// A folder the user may not write is the helper's answer, not an error: a
/// SYSTEM host writes it only when nobody but administrators may change it
/// or a folder above it, and a host that is not SYSTEM refuses.
#[test]
fn a_folder_only_administrators_may_write_is_written_only_by_system_and_only_then() {
    let temp = tempdir();
    let cache = release(temp.path(), "Alpha0.3.5.8", b"OptiScaler proxy");
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    let before = names(folder);
    let denied = Response::NotWritable {
        folder: folder.to_path_buf(),
    };
    {
        let _unwritable = Unwritable::new(folder);
        assert_eq!(install(Some(&exe), None, &cache), denied);
        assert_eq!(names(folder), before);
    }
    assert!(matches!(
        install(Some(&exe), None, &cache),
        Response::Placed { .. }
    ));
    {
        let _unwritable = Unwritable::new(folder);
        assert_eq!(remove(&exe), denied);
    }
    assert_eq!(remove(&exe), Response::Removed { kept: vec![] });
    assert_eq!(names(folder), before);

    // This folder's user may change it: SYSTEM does not write it.
    let response = handle(
        Request::Install {
            exe: Some(exe.clone()),
            install: None,
            package: cached_at(&cache),
        },
        Writer::System,
        &|_, _| {},
    );
    let Response::Refused { reason } = response else {
        panic!("{response:?}")
    };
    assert!(reason.contains("nobody but administrators"), "{reason}");
    assert_eq!(names(folder), before);

    // What the host does with the answer.
    let placed = || Response::Placed {
        exe: exe.clone(),
        release: "Alpha0.3.5.8".into(),
        proxy: "dxgi.dll".into(),
        updated: false,
        mode: Mode::Full,
    };
    assert_eq!(as_system(denied.clone(), true, placed), placed());
    let Response::Refused { reason } = as_system(denied, false, || unreachable!()) else {
        panic!()
    };
    assert!(reason.contains(&folder.display().to_string()), "{reason}");
    assert_eq!(as_system(placed(), true, || unreachable!()), placed());

    // Who may change a folder: Program Files' owners only.
    let trusted = "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464";
    let security =
        |link, owner: &str, writers: &[&str]| butterpollo_stream::neural::folder::Security {
            link,
            owner: owner.into(),
            writers: writers.iter().map(|sid| sid.to_string()).collect(),
        };
    let path = Path::new(r"C:\Program Files\Game");
    let admins = ["S-1-5-18", "S-1-5-32-544", trusted];
    assert_eq!(
        butterpollo_stream::neural::folder::verdict(path, &security(false, trusted, &admins)),
        Ok(())
    );
    assert_eq!(
        butterpollo_stream::neural::folder::verdict(path, &security(false, "S-1-5-32-544", &[])),
        Ok(())
    );
    // Accounts by name in this PC's language, never a raw SID or entry.
    let users = users();
    for (security, problem) in [
        (security(true, trusted, &admins), "link".to_string()),
        (
            security(false, "S-1-5-21-1-2-3-1001", &admins),
            "owned by another account".into(),
        ),
        (
            security(false, trusted, &["S-1-5-18", "S-1-5-32-545"]),
            format!("changed by {users}"),
        ),
        (
            security(false, trusted, &["S-1-5-18", "ACE type 9"]),
            "changed by another account".into(),
        ),
    ] {
        let error = butterpollo_stream::neural::folder::verdict(path, &security).unwrap_err();
        assert!(error.contains(&problem), "{error}");
        assert!(
            !error.contains("S-1-5-21") && !error.contains("ACE"),
            "{error}"
        );
    }
}

/// The Users group as Windows names it here (`BUILTIN\Users` in English).
fn users() -> String {
    let name = butterpollo_win_io::process::account_name("S-1-5-32-545").unwrap();
    assert!(name.contains('\\'), "{name}");
    name
}

/// This PC's Program Files, as an ordinary user: where a SYSTEM host's check
/// finds that nobody but administrators may change a program's folder or a
/// folder above it, the user's helper may not write there and writes
/// nothing. The helper runs only on those folders, as it writes wherever the
/// user may:
/// `cargo test -p butterpollo neural::tests::program_files -- --ignored --nocapture`.
#[test]
#[ignore = "reads this PC's Program Files"]
fn program_files_folders_are_left_to_system() {
    assert!(
        !butterpollo_win_io::process::elevated().unwrap(),
        "run it as an ordinary user"
    );
    let temp = tempdir();
    let cache = release(temp.path(), "Alpha0.3.5.8", b"OptiScaler proxy");
    let programs = PathBuf::from(std::env::var_os("ProgramFiles").unwrap());
    // Everything the helper writes starts at the folder's root; some
    // programs' subfolders are closed to the user.
    let names = |folder: &Path| {
        let mut names: Vec<_> = std::fs::read_dir(folder)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name())
            .collect();
        names.sort();
        names
    };
    let mut closed = Vec::new();
    for entry in std::fs::read_dir(&programs).unwrap().flatten() {
        let folder = entry.path();
        let Some(exe) = std::fs::read_dir(&folder)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .find(|path| path.extension().is_some_and(|e| e == "exe"))
        else {
            continue;
        };
        let system = butterpollo_stream::neural::folder::administrators_only(&folder);
        if let Err(error) = system {
            println!("{}: SYSTEM's check {error}", folder.display());
            continue;
        }
        let before = names(&folder);
        let user = install(Some(&exe), None, &cache);
        println!(
            "{}: SYSTEM's check Ok; the user's helper {user:?}",
            folder.display()
        );
        assert_eq!(
            names(&folder),
            before,
            "the helper wrote {}",
            folder.display()
        );
        assert!(
            matches!(
                user,
                Response::NotWritable { .. } | Response::Refused { .. }
            ),
            "{user:?}"
        );
        closed.push((folder, user));
    }
    let (folder, user) = closed
        .iter()
        .find(|(folder, _)| folder.ends_with("Rubylight"))
        .expect("Rubylight installed in Program Files");
    assert_eq!(
        *user,
        Response::NotWritable {
            folder: folder.clone()
        }
    );
}

/// The installed host's data folder, read only: a SYSTEM host keeps its
/// download in `neural` there (made writable by administrators only), and
/// the folder passes its check, which ends there:
/// `cargo test -p butterpollo neural::tests::the_installed -- --ignored`.
#[test]
#[ignore = "reads this PC's installed host"]
fn the_installed_hosts_data_folder_holds_a_system_cache() {
    let data = butterpollo_session::paths::data_root();
    assert!(data.is_dir(), "{} is not installed", data.display());
    butterpollo_stream::neural::trusted_root(&data, &data, true).unwrap();
}

/// A folder above a SYSTEM host's cache passes when users may only add to
/// it, as to C:\ProgramData, and not when they may move or delete what it
/// holds: they could move the cache away and put their own in its place.
/// The folder's own permissions, its owner taken as Administrators, which
/// an unelevated test cannot give it to.
#[test]
fn a_folder_above_the_cache_lets_nobody_but_administrators_move_what_it_holds() {
    let temp = tempdir();
    let above = temp.path().join("above");
    std::fs::create_dir(&above).unwrap();
    let icacls = |args: &[&str]| {
        let changed = std::process::Command::new("icacls")
            .arg(&above)
            .args(args)
            .output()
            .unwrap();
        assert!(changed.status.success(), "{changed:?}");
    };
    // The whole DACL is replaced: on CI the temp folder's entry for the
    // runner's account reaches a new folder without being marked inherited,
    // so icacls' /inheritance:r left it in place.
    let sid = butterpollo_win_io::process::own_sid().unwrap();
    butterpollo_win_io::process::restrict(
        &above,
        &format!("O:{sid}D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;;0x1200a9;;;BU)"),
    )
    .unwrap();
    icacls(&["/grant", "*S-1-5-32-545:(AD,WD)"]);
    let held = |whole| {
        butterpollo_stream::neural::folder::hold(&above, whole, |path, rights| {
            butterpollo_stream::neural::folder::security(path, rights).map(|security| {
                butterpollo_stream::neural::folder::Security {
                    owner: "S-1-5-32-544".into(),
                    ..security
                }
            })
        })
    };
    let users = format!("changed by {}", users());
    assert_eq!(held(false), Ok(()));
    // The cache itself is held only when users may not add to it either.
    let error = held(true).unwrap_err();
    assert!(error.contains(&users), "{error}");
    // Users who may delete what it holds could swap the cache's folder.
    icacls(&["/grant", "*S-1-5-32-545:(DC)"]);
    let error = held(false).unwrap_err();
    assert!(error.contains(&users), "{error}");
    // So could users who may delete or rename the folder itself.
    icacls(&["/remove:g", "*S-1-5-32-545"]);
    icacls(&["/grant", "*S-1-5-32-545:(RX)"]);
    icacls(&["/grant", "*S-1-5-32-545:(AD,WD)"]);
    assert_eq!(held(false), Ok(()));
    icacls(&["/grant", "*S-1-5-32-545:(D)"]);
    let error = held(false).unwrap_err();
    assert!(error.contains(&users), "{error}");
}

fn file(asset: String) -> Download {
    Download {
        asset,
        url: String::new(),
        digest: String::new(),
        size: 0,
    }
}

/// The release `tag` with its two archives and checksum list.
fn listed(tag: &str) -> Release {
    Release {
        tag: tag.into(),
        full: file(format!("anr-fg-{tag}-full.zip")),
        addon: file(format!("anr-fg-{tag}-addon.zip")),
        sums: file("SHA256SUMS.txt".into()),
    }
}

/// The cache takes from the full package what goes into a game (`game/`
/// and OptiScaler's licence) and from the add-on only the provider, never
/// their scripts or readmes; a release without the provider or with a
/// path outside its folder is not cached.
#[test]
fn the_cache_takes_from_each_archive_only_what_goes_into_a_game() {
    let temp = tempdir();
    let provider = "game/OptiScaler/custom/Apex Neural Rendering FG/dlssg/nvngx_dlssg.dll";
    let full = temp.path().join("full.zip");
    archive(
        &full,
        &[
            ("game/dxgi.dll", b"OptiScaler"),
            ("game/OptiScaler.ini", b"[FrameGen]\n"),
            (provider, b"provider"),
            ("LICENSES/OptiScaler-GPL-3.0.txt", b"GPL"),
            ("install-fg.ps1", b"a script"),
            ("README.md", b"the readme"),
        ],
    );
    let addon = temp.path().join("addon.zip");
    archive(
        &addon,
        &[
            (
                "Apex Neural Rendering FG/dlssg/nvngx_dlssg.dll",
                b"addon provider",
            ),
            ("Apex Neural Rendering FG/dlssg/nr_fg.ini", b"path = auto\n"),
            ("README.md", b"the readme"),
        ],
    );
    let root = temp.path().join("cache");
    std::fs::create_dir(&root).unwrap();
    let (folder, package) =
        extract(&[full.clone(), addon.clone()], &root, &listed("V.B16")).unwrap();
    assert_eq!(folder, root.join("V.B16"));
    let in_full = format!("full/{provider}");
    assert_eq!(
        package.files.keys().map(String::as_str).collect::<Vec<_>>(),
        [
            "addon/Apex Neural Rendering FG/dlssg/nr_fg.ini",
            "addon/Apex Neural Rendering FG/dlssg/nvngx_dlssg.dll",
            "full/LICENSES/OptiScaler-GPL-3.0.txt",
            "full/game/OptiScaler.ini",
            in_full.as_str(),
            "full/game/dxgi.dll",
        ]
    );
    assert_eq!(
        package
            .archives
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["anr-fg-V.B16-addon.zip", "anr-fg-V.B16-full.zip"]
    );
    assert_eq!(
        package.files["full/game/dxgi.dll"].sha256,
        sha(b"OptiScaler")
    );
    assert_eq!(
        std::fs::read(folder.join("files").join(&in_full)).unwrap(),
        b"provider"
    );
    assert_eq!(cached(&root).unwrap().1, package);

    // The add-on without the provider: not the whole release.
    let empty = temp.path().join("empty.zip");
    archive(&empty, &[("README.md", b"the readme")]);
    let error = extract(&[full.clone(), empty], &root, &listed("V.B17"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("the release has no addon/"), "{error}");
    // A path outside the folder, in a part the cache doesn't take.
    let outside = temp.path().join("outside.zip");
    archive(&outside, &[("../evil.dll", b"evil")]);
    assert!(extract(&[full, outside], &root, &listed("V.B18")).is_err());
    assert!(!temp.path().join("evil.dll").exists());
    // The good release is still the one cached.
    assert_eq!(cached(&root).unwrap().1.tag, "V.B16");
}

#[test]
fn a_download_must_match_its_published_digest_and_size() {
    let release = Download {
        asset: "anr-fg-V.B16-full.zip".into(),
        url: String::new(),
        digest: sha(b"archive"),
        size: 7,
    };
    assert!(verified(7, &sha(b"archive"), &release).is_ok());
    let error = verified(7, &sha(b"archivf"), &release)
        .unwrap_err()
        .to_string();
    assert!(error.contains("SHA-256"), "{error}");
    assert!(verified(8, &sha(b"archive"), &release).is_err());
}

/// A SYSTEM host asks of the cache itself who may change it, and of each
/// folder up to the host's data root who may move it or what it holds;
/// above the data root only the link check applies.
#[test]
fn only_the_cache_itself_is_held_whole() {
    let temp = tempdir();
    let data_root = temp.path().join("data");
    let root = data_root.join("neural");
    std::fs::create_dir_all(&root).unwrap();
    let asked = Mutex::new(Vec::new());
    butterpollo_stream::neural::trusted_root_with(&root, &data_root, true, |path, whole| {
        asked.lock().unwrap().push((path.to_path_buf(), whole));
        Ok(())
    })
    .unwrap();
    assert_eq!(
        asked.into_inner().unwrap(),
        [(root.clone(), true), (data_root.clone(), false)]
    );
    // An ordinary host asks nothing of the kind.
    butterpollo_stream::neural::trusted_root_with(&root, &data_root, false, |_, _| panic!("asked"))
        .unwrap();
}

/// A record that can't be read is never taken for empty and written over:
/// it holds the acceptance and the pending removals.
#[test]
fn a_damaged_record_is_never_written_over() {
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let h = &f.host;
    let path = h.directory.join(SETTINGS_FILE);
    assert!(!path.exists());
    assert!(!accepted(h));
    accept(h).unwrap();
    assert!(accepted(h));
    for damaged in [&b"{\"accepted_at\": 1, \"pending_rem"[..], b"[]"] {
        std::fs::write(&path, damaged).unwrap();
        let error = format!("{:#}", accept(h).unwrap_err());
        assert!(error.contains("damaged"), "{error}");
        assert!(record_pending(h, &[]).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), damaged);
    }
    // A pending removals list that doesn't parse fails each change, and
    // nothing writes over the removals it holds.
    let damaged = br#"{"accepted_at": 1, "pending_removals": [{"name": "Game", "exe": 7}]}"#;
    std::fs::write(&path, damaged).unwrap();
    let away = Pending {
        name: "Away".into(),
        exe: unconnected_drive().join(r"Games\Away\Away.exe"),
        uuid: None,
    };
    let error = format!(
        "{:#}",
        record_pending(h, std::slice::from_ref(&away)).unwrap_err()
    );
    assert!(error.contains("pending removals are damaged"), "{error}");
    clear_pending(h, &away.exe);
    try_pending(h, std::slice::from_ref(&away));
    remove_pending(h);
    assert!(pending(h).is_empty());
    assert_eq!(std::fs::read(&path).unwrap(), damaged);
}

/// A cache is used only as this host keeps it: no link on the way to it,
/// for SYSTEM nobody but administrators may change it, its files checked
/// once per host start, and its archives the pinned ones.
#[test]
fn a_cache_is_used_only_as_this_host_keeps_it() {
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let h = &f.host;
    let temp = tempdir();
    let root = temp.path().join("neural");
    std::fs::create_dir(&root).unwrap();
    trusted_root(&root, temp.path(), false).unwrap();
    // This test's folders are its user's to change.
    let error = format!("{:#}", trusted_root(&root, temp.path(), true).unwrap_err());
    assert!(error.contains("nobody but administrators"), "{error}");
    let elsewhere = temp.path().join("elsewhere");
    std::fs::create_dir_all(elsewhere.join("neural")).unwrap();
    let linked = temp.path().join("linked");
    junction(&linked, &elsewhere);
    for root in [linked.clone(), linked.join("neural")] {
        let error = format!("{:#}", trusted_root(&root, temp.path(), false).unwrap_err());
        assert!(error.contains("is a link"), "{error}");
    }
    // A link above the data root is still no way to the cache.
    std::fs::create_dir_all(elsewhere.join("data/neural")).unwrap();
    let above = temp.path().join("above");
    junction(&above, &elsewhere);
    let error = format!(
        "{:#}",
        trusted_root(&above.join("data/neural"), &above.join("data"), false).unwrap_err()
    );
    assert!(error.contains("is a link"), "{error}");
    std::fs::remove_dir(&above).unwrap();
    std::fs::remove_dir(&linked).unwrap();

    // A launch takes the cached release without hashing it: the copy checks
    // each file as it goes, so a changed one never reaches the game.
    assert_eq!(launch_cache(h).unwrap().map(|(cached, _held)| cached), None);
    let folder = release(&cache_root(h), "Alpha0.3.5.8", b"OptiScaler proxy");
    let pak = folder.join(format!("files/full/game/{PAK}"));
    std::fs::write(&pak, "tampered").unwrap();
    // A release Rubylight does not pin is never applied.
    assert_eq!(launch_cache(h).unwrap().map(|(cached, _held)| cached), None);
    pin_to(h, &folder);
    assert_eq!(
        launch_cache(h).unwrap().map(|(cached, _held)| cached),
        Some(cached_at(&folder))
    );
    assert_eq!(h.neural.lock().verified, None);
    let exe = game(temp.path());
    let before = names(exe.parent().unwrap());
    let Response::Error { message } = install(Some(&exe), None, &folder) else {
        panic!("a changed cache was copied")
    };
    assert!(message.contains("does not match its release"), "{message}");
    assert_eq!(names(exe.parent().unwrap()), before);
    // A fetch's cache hit hashes it once per host start.
    let package = butterpollo_stream::neural::folder::read_package(&folder).unwrap();
    let error = format!("{:#}", verify_cache(h, &folder, &package).unwrap_err());
    assert!(error.contains("pack.nrx in"), "{error}");
    std::fs::write(&pak, "model").unwrap();
    verify_cache(h, &folder, &package).unwrap();
    std::fs::write(&pak, "tampered").unwrap();
    verify_cache(h, &folder, &package).unwrap();

    // The same tag with other archives is another release; the checksum
    // list is not cached and doesn't count.
    let mut package = butterpollo_stream::neural::folder::read_package(&folder).unwrap();
    let (full, addon) = (
        "anr-fg-Alpha0.3.5.8-full.zip".to_owned(),
        "anr-fg-Alpha0.3.5.8-addon.zip".to_owned(),
    );
    let (one, two, three) = ("1".repeat(64), "2".repeat(64), "3".repeat(64));
    package.archives = [(full.clone(), one.clone()), (addon.clone(), two.clone())].into();
    let mut pin = neural::Pin {
        tag: "Alpha0.3.5.8",
        full: neural::PinnedFile {
            asset: &full,
            sha256: &one,
        },
        addon: neural::PinnedFile {
            asset: &addon,
            sha256: &two,
        },
        sums: neural::PinnedFile {
            asset: "SHA256SUMS.txt",
            sha256: &three,
        },
    };
    assert!(serves(&package, &pin));
    pin.sums.sha256 = &one;
    assert!(serves(&package, &pin));
    pin.addon.sha256 = &three;
    assert!(!serves(&package, &pin));
    pin.addon.sha256 = &two;
    pin.tag = "FG-V.B17";
    assert!(!serves(&package, &pin));
    pin.tag = "Alpha0.3.5.8";
    package.archives.remove(&addon);
    assert!(!serves(&package, &pin));
}

/// The cache is held where it was checked: a launch's [`launch_cache`] and an
/// install's [`Using`] keep it from being renamed away until their copy from
/// it is done, and let go when they end.
#[test]
fn the_cache_cannot_be_moved_while_a_launch_or_an_install_holds_it() {
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let h = &f.host;
    let root = cache_root(h);
    std::fs::create_dir_all(&root).unwrap();
    let folder = release(&root, "Alpha0.3.5.8", b"OptiScaler proxy");
    pin_to(h, &folder);
    let moved = root.with_file_name("neural-anr-moved");
    // A launch finishes an update cut short from this cache: the hold it gets
    // back keeps the root the trust check approved.
    let (cached, held) = launch_cache(h).unwrap().expect("the pinned release");
    assert_eq!(cached.folder, folder);
    assert!(
        std::fs::rename(&root, &moved).is_err(),
        "renamed while a launch holds it"
    );
    assert!(root.is_dir(), "the held cache stays where it is");
    drop(held);
    std::fs::rename(&root, &moved).unwrap();
    std::fs::rename(&moved, &root).unwrap();
    // An install keeps the same hold in its Using until the last file is placed.
    let held = Hold::new([root.as_path(), data_root(h).as_path()]).unwrap();
    let using = Using::for_test(h.clone(), folder.clone(), Some(held));
    assert!(
        std::fs::rename(&root, &moved).is_err(),
        "renamed while an install copies it"
    );
    drop(using);
    std::fs::rename(&root, &moved).unwrap();
}
