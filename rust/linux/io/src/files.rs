//! Mounted file systems on this PC, for the console's file picker, and deleting a set of files all or none.

use std::{
    ffi::OsString,
    io,
    os::unix::ffi::OsStringExt,
    path::{Path, PathBuf},
};

/// One line of `/proc/self/mounts`.
#[derive(Debug, PartialEq, Eq)]
struct Mount {
    point: PathBuf,
    kind: String,
}

/// File systems that hold no files a person browses: the kernel's own and per-program ones.
const VIRTUAL: &[&str] = &[
    "autofs",
    "bpf",
    "cgroup",
    "cgroup2",
    "configfs",
    "debugfs",
    "devpts",
    "devtmpfs",
    "efivarfs",
    "fusectl",
    "hugetlbfs",
    "mqueue",
    "nsfs",
    "overlay",
    "proc",
    "pstore",
    "ramfs",
    "securityfs",
    "squashfs",
    "sysfs",
    "tmpfs",
    "tracefs",
    "binfmt_misc",
    "fuse.portal",
    "fuse.gvfsd-fuse",
];

/// File systems another machine serves.
const REMOTE: &[&str] = &[
    "nfs",
    "nfs4",
    "cifs",
    "smb3",
    "smbfs",
    "9p",
    "ceph",
    "afs",
    "fuse.sshfs",
    "davfs",
];

/// A path as `/proc/self/mounts` writes it: a space, tab, newline or backslash is `\040`,
/// `\011`, `\012` or `\134`.
fn unescape(field: &str) -> PathBuf {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'\\'
            && at + 3 < bytes.len()
            && let Some(code) = field
                .get(at + 1..at + 4)
                .and_then(|digits| u8::from_str_radix(digits, 8).ok())
        {
            out.push(code);
            at += 4;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    PathBuf::from(OsString::from_vec(out))
}

fn parse_mounts(text: &str) -> Vec<Mount> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let _device = fields.next()?;
            let point = unescape(fields.next()?);
            let kind = fields.next()?.to_owned();
            Some(Mount { point, kind })
        })
        .collect()
}

fn mounts() -> Vec<Mount> {
    std::fs::read(Path::new("/proc/self/mounts"))
        .map(|bytes| parse_mounts(&String::from_utf8_lossy(&bytes)))
        .unwrap_or_default()
}

fn browsable(mounts: Vec<Mount>) -> Vec<String> {
    let mut points = vec!["/".to_owned()];
    for mount in mounts {
        if VIRTUAL.contains(&mount.kind.as_str()) {
            continue;
        }
        let Some(point) = mount.point.to_str() else {
            continue;
        };
        if point.starts_with("/boot") || point.starts_with("/var/lib") || point.starts_with("/snap")
        {
            continue;
        }
        if !points.iter().any(|known| known == point) {
            points.push(point.to_owned());
        }
    }
    points
}

/// The places the file picker starts from: the root folder and every mounted disk or share.
pub fn drives() -> Vec<String> {
    browsable(mounts())
}

fn local_in(mounts: &[Mount], path: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    let holder = mounts
        .iter()
        .filter(|mount| path.starts_with(&mount.point))
        .max_by_key(|mount| mount.point.as_os_str().len());
    holder.is_none_or(|mount| !REMOTE.contains(&mount.kind.as_str()))
}

/// Whether `path` is on a disk of this PC, so that a file missing there is missing for the
/// signed-in user too, and not on a share another machine serves.
pub fn local_volume(path: &Path) -> bool {
    local_in(&mounts(), path)
}

/// Deletes every file in `paths`, or none of them. A link is deleted itself, never what it
/// points to. Each is first moved to a hidden name beside it, which can be undone; only when
/// every move worked do the files go. One that cannot be moved or removed (it is missing,
/// its folder is not writable) puts every other back, and the error names that file.
pub fn delete_all(paths: &[PathBuf]) -> Result<(), (PathBuf, io::Error)> {
    let mut unique: Vec<&PathBuf> = Vec::with_capacity(paths.len());
    for path in paths {
        if !unique.contains(&path) {
            unique.push(path);
        }
    }
    for path in &unique {
        let metadata = std::fs::symlink_metadata(path).map_err(|error| ((*path).clone(), error))?;
        if metadata.is_dir() {
            return Err((
                (*path).clone(),
                io::Error::other("it is a folder, not a file"),
            ));
        }
    }
    let mut moved: Vec<(&PathBuf, PathBuf)> = Vec::with_capacity(unique.len());
    for (index, path) in unique.iter().enumerate() {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let held = path.with_file_name(format!(
            ".{name}.rubylight-delete-{}-{index}",
            std::process::id()
        ));
        if let Err(error) = std::fs::rename(path, &held) {
            put_back(&moved);
            return Err(((*path).clone(), error));
        }
        moved.push((path, held));
    }
    for at in 0..moved.len() {
        if let Err(error) = std::fs::remove_file(&moved[at].1) {
            let failed = moved[at].0.clone();
            put_back(&moved[at..]);
            return Err((failed, error));
        }
    }
    Ok(())
}

fn put_back(moved: &[(&PathBuf, PathBuf)]) {
    for (path, held) in moved {
        if let Err(error) = std::fs::rename(held, path) {
            tracing::warn!(
                file = %path.display(),
                held = %held.display(),
                %error,
                "could not put a file back after a failed delete"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("rubylight-files-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn mounts_are_read_unescaped_and_filtered() {
        let text = "/dev/sda2 / ext4 rw 0 0\nproc /proc proc rw 0 0\n/dev/sdb1 /run/media/me/My\\040Disk vfat rw 0 0\ntmpfs /tmp tmpfs rw 0 0\nserver:/x /mnt/x nfs4 rw 0 0\n/dev/sda1 /boot/efi vfat rw 0 0\n";
        let mounts = parse_mounts(text);
        assert_eq!(mounts.len(), 6);
        assert_eq!(mounts[2].point, PathBuf::from("/run/media/me/My Disk"));
        assert_eq!(
            browsable(parse_mounts(text)),
            ["/", "/run/media/me/My Disk", "/mnt/x"]
        );
        assert!(local_in(&mounts, Path::new("/home/me/game.sh")));
        assert!(local_in(&mounts, Path::new("/run/media/me/My Disk/g")));
        assert!(!local_in(&mounts, Path::new("/mnt/x/game")));
        assert!(!local_in(&mounts, Path::new("relative")));
    }

    #[test]
    fn this_pc_lists_its_root() {
        assert_eq!(drives().first().map(String::as_str), Some("/"));
        assert!(local_volume(Path::new("/")));
    }

    #[test]
    fn delete_all_removes_every_file() {
        let dir = scratch("all");
        let files: Vec<PathBuf> = ["a", "b", "c"].iter().map(|n| dir.join(n)).collect();
        for file in &files {
            std::fs::write(file, b"x").unwrap();
        }
        delete_all(&files).unwrap();
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn delete_all_with_a_missing_file_keeps_the_rest() {
        let dir = scratch("missing");
        let (a, gone) = (dir.join("a"), dir.join("gone"));
        std::fs::write(&a, b"x").unwrap();
        let (file, error) = delete_all(&[a.clone(), gone.clone()]).unwrap_err();
        assert_eq!(file, gone);
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(a.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn delete_all_puts_back_what_it_moved_when_a_later_file_cannot_move() {
        let dir = scratch("rollback");
        let locked = dir.join("locked");
        std::fs::create_dir(&locked).unwrap();
        let (a, b) = (dir.join("a"), locked.join("b"));
        std::fs::write(&a, b"x").unwrap();
        std::fs::write(&b, b"y").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();
        let outcome = delete_all(&[a.clone(), b.clone()]);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
        // Root can write a folder whatever its mode, and then both files go.
        if let Err((file, _)) = outcome {
            assert_eq!(file, b);
            assert!(a.exists() && b.exists());
        } else {
            assert!(!a.exists() && !b.exists());
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
