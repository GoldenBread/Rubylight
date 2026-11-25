//! Path containment, traversal, verbatim/device prefix, anti-cheat, and link
//! security regression tests for Neural Rendering folders.
use super::{
    Failure, Request, Response, Writer,
    find::{anti_cheat_refusal, find_exe, inside, is_clean_path, is_valid_exe_path, not_the_game},
    followed, handle, real, same,
    security::administrators_only,
};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

struct Temp {
    path: PathBuf,
    junctions: Vec<PathBuf>,
}

impl Temp {
    fn new(tag: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "rubylight-neural-folder-test-{tag}-{}-{id}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self {
            path,
            junctions: Vec::new(),
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn junction(&mut self, link: PathBuf, target: &Path) -> PathBuf {
        if let Some(parent) = link.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let output = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(&link)
            .arg(target)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        self.junctions.push(link.clone());
        link
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        for link in self.junctions.iter().rev() {
            let _ = std::fs::remove_dir(link);
        }
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[test]
fn clean_path_rejects_traversal_and_verbatim_or_device_prefixes() {
    for good in [
        r"C:\Program Files\DLSSGame\Game.exe",
        r"D:/Games/Game.v1.2/Game..exe",
        r"\\server\share\Games\Game.exe",
    ] {
        let path = Path::new(good);
        assert!(is_clean_path(path), "expected clean: {good}");
        assert!(is_valid_exe_path(path), "expected valid exe: {good}");
    }
    for bad in [
        r"C:\Program Files\DLSSGame\..\OtherApp\target.exe",
        r"C:\Games\Game\..\..\Windows\System32\foo.exe",
        r"C:\Games\EACGame\Binaries\Win64\..\Win64\Game.exe",
        r"C:\Games\Game\.\Game.exe",
        r"\\?\C:\Windows\System32\foo.exe",
        r"\\?\UNC\server\share\Game.exe",
        r"\\?\GLOBALROOT\Device\HarddiskVolume1\Windows\System32\foo.exe",
        r"\\.\C:\Windows\System32\foo.exe",
        r"\\.\pipe\foo.exe",
        "//?/C:/Windows/System32/foo.exe",
        "//./C:/Windows/System32/foo.exe",
        r"\??\C:\Windows\System32\foo.exe",
    ] {
        let path = Path::new(bad);
        assert!(!is_clean_path(path), "expected unsafe: {bad}");
        assert!(!is_valid_exe_path(path), "expected invalid exe: {bad}");
    }
}

#[test]
fn traversal_cannot_escape_install_containment() {
    let mut temp = Temp::new("containment");
    let install = temp.path().join("DLSSGame");
    let other = temp.path().join("OtherApp");
    std::fs::create_dir_all(&install).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("target.exe"), b"MZ").unwrap();

    // Attack (a): C:\Program Files\DLSSGame\..\OtherApp\target.exe
    let synthetic_install = Path::new(r"C:\Program Files\DLSSGame");
    let synthetic_escape = Path::new(r"C:\Program Files\DLSSGame\..\OtherApp\target.exe");
    assert!(!inside(synthetic_escape, synthetic_install));
    assert!(!same(synthetic_escape, synthetic_escape));

    let real_escape = PathBuf::from(format!(r"{}\..\OtherApp\target.exe", install.display()));
    assert!(!inside(&real_escape, &install));
    assert!(matches!(
        find_exe(Some(real_escape.clone()), Some(&install)),
        Err(Failure::ChooseExe(_))
    ));
    assert!(matches!(
        handle(
            Request::Find {
                exe: Some(real_escape),
                install: Some(install.clone()),
            },
            Writer::User,
            &|_, _| {}
        ),
        Response::ChooseExe { .. }
    ));

    // A junction planted inside `install` pointing to `OtherApp` also cannot escape.
    let sublink = temp.junction(install.join("sub"), &other);
    let escaped_via_junction = sublink.join("target.exe");
    assert!(matches!(
        find_exe(Some(escaped_via_junction), Some(&install)),
        Err(Failure::ChooseExe(_))
    ));

    // Legitimate game-folder-moved-and-linked-back (`mklink /J`) still works.
    let moved = temp.path().join("MovedGame");
    std::fs::create_dir_all(&moved).unwrap();
    let moved_exe = moved.join("Game.exe");
    std::fs::write(&moved_exe, b"MZ").unwrap();
    let linked = temp.junction(temp.path().join(r"library\common\LinkedGame"), &moved);
    let through = linked.join("Game.exe");
    assert_eq!(
        find_exe(Some(through.clone()), Some(&linked)).ok(),
        Some(through.clone())
    );
    let resolved = real(&moved).unwrap().join("Game.exe");
    assert_eq!(
        find_exe(Some(resolved.clone()), Some(&linked)).ok(),
        Some(resolved)
    );
}

#[test]
fn verbatim_and_traversal_paths_cannot_bypass_windows_dir_block() {
    let mut temp = Temp::new("windir");
    let sysroot = PathBuf::from(std::env::var_os("SystemRoot").unwrap());
    let win_name = sysroot.file_name().unwrap().to_string_lossy();
    let drive = sysroot
        .components()
        .next()
        .unwrap()
        .as_os_str()
        .to_string_lossy();

    // Attack (b): \\?\C:\Windows\...\foo.exe, \\.\C:\Windows\...\foo.exe,
    // and C:\Games\Game\..\..\Windows\...\foo.exe.
    let verbatim = PathBuf::from(format!(r"\\?\{}\System32\foo.exe", sysroot.display()));
    let device = PathBuf::from(format!(r"\\.\{}\System32\foo.exe", sysroot.display()));
    let traversal = PathBuf::from(format!(
        r"{drive}\Games\Game\..\..\{win_name}\System32\foo.exe"
    ));
    for exe in [&verbatim, &device, &traversal] {
        assert_eq!(
            not_the_game(exe),
            Some("part of Windows"),
            "{}",
            exe.display()
        );
        assert!(
            matches!(
                find_exe(Some(exe.clone()), None),
                Err(Failure::ChooseExe(_))
            ),
            "{}",
            exe.display()
        );
    }

    // A junction pointing into SystemRoot is also blocked as part of Windows.
    let win_link = temp.junction(temp.path().join("WinLink"), &sysroot.join("System32"));
    let via_link = win_link.join("foo.exe");
    assert_eq!(not_the_game(&via_link), Some("part of Windows"));
    assert!(matches!(
        find_exe(Some(via_link), None),
        Err(Failure::ChooseExe(_))
    ));
}

#[test]
fn traversal_cannot_bypass_three_up_anti_cheat_scan() {
    let temp = Temp::new("eac");
    let game_root = temp.path().join("EACGame");
    let win64 = game_root.join(r"Binaries\Win64");
    std::fs::create_dir_all(&win64).unwrap();
    std::fs::create_dir_all(game_root.join("EasyAntiCheat")).unwrap();
    let clean_exe = win64.join("Game.exe");
    std::fs::write(&clean_exe, b"MZ").unwrap();
    std::fs::write(win64.join("nvngx_dlssg.dll"), b"DLSS").unwrap();

    // Baseline: clean path catches EasyAntiCheat two folders above Win64.
    let err = anti_cheat_refusal(&clean_exe, None, &[]).unwrap_err();
    assert!(err.contains("Easy Anti-Cheat"), "{err}");

    // Attack (c): ...\EACGame\Binaries\Win64\..\Win64\Game.exe
    let traversal_exe = PathBuf::from(format!(r"{}\..\Win64\Game.exe", win64.display()));
    assert!(matches!(
        find_exe(Some(traversal_exe.clone()), None),
        Err(Failure::ChooseExe(_))
    ));
    assert!(matches!(
        handle(
            Request::Find {
                exe: Some(traversal_exe.clone()),
                install: None,
            },
            Writer::User,
            &|_, _| {}
        ),
        Response::ChooseExe { .. }
    ));
    assert!(
        anti_cheat_refusal(&traversal_exe, None, &[]).is_err(),
        "anti_cheat_refusal must reject traversal path"
    );
    assert!(matches!(
        handle(
            Request::Prepare {
                exe: traversal_exe,
                install: None,
                package: None,
            },
            Writer::User,
            &|_, _| {}
        ),
        Response::Refused { .. }
    ));
}

#[test]
fn administrators_only_and_followed_reject_unsafe_paths() {
    for bad in [
        r"C:\Program Files\Game\..\Other",
        r"\\?\C:\Program Files\Game",
        r"\\.\C:\Program Files\Game",
    ] {
        let path = Path::new(bad);
        assert!(
            administrators_only(path).is_err(),
            "administrators_only must reject {bad}"
        );
        let exe = path.join("Game.exe");
        assert_eq!(followed(exe.clone(), Writer::User), exe);
    }
}
