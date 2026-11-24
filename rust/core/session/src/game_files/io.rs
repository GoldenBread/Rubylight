//! Low-level local file access, atomic swap/recovery, and Windows folder pinning for managed game files.
use std::{
    fmt,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

use super::MAX_FILE_BYTES;

/// Whether `path` is on a local drive.
pub(super) fn local(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::path::Prefix;
        matches!(
            path.components().next(),
            Some(Component::Prefix(prefix))
                if matches!(prefix.kind(), Prefix::VerbatimDisk(_) | Prefix::Disk(_))
        )
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        true
    }
}

/// How the host reaches game files: runs the file work it is given with the rights Windows
/// checks that work against, or fails without running it. The host acts as the signed-in
/// Windows user: a game's folder is usually the user's to change, so a link or junction put in
/// it between a check and a write could otherwise lead a host running as SYSTEM to any file.
/// As the user, it leads nowhere the user could not write anyway. The journal is the host's
/// own and is not reached this way.
pub type Access = fn(&mut dyn FnMut()) -> std::io::Result<()>;

/// [`Access`] as the calling process, for tests and tools.
pub fn as_host(work: &mut dyn FnMut()) -> std::io::Result<()> {
    work();
    Ok(())
}

/// Run `work` through `access`.
pub(crate) fn reach<T>(access: Access, work: impl FnOnce() -> T) -> std::io::Result<T> {
    let mut work = Some(work);
    let mut done = None;
    access(&mut || done = work.take().map(|work| work()))?;
    done.ok_or_else(|| std::io::Error::other("the game file work did not run"))
}

/// Read a file the host may edit: at most [`MAX_FILE_BYTES`].
pub(super) fn read(file: &Path) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(file)?
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(std::io::Error::other("the file is larger than 1 MiB"));
    }
    Ok(bytes)
}

/// Write `bytes` over `file` in one step: a temporary file beside it, then a swap that keeps
/// the file's permissions and attributes. A read-only file is not overwritten.
pub(crate) fn replace(file: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let (Some(parent), Some(name)) = (file.parent(), file.file_name()) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the file has no folder",
        ));
    };
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        name.to_string_lossy(),
        uuid::Uuid::new_v4()
    ));
    let result = (|| {
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        out.write_all(bytes)?;
        out.sync_all()?;
        drop(out);
        #[cfg(test)]
        if super::tests::HALFWAY.get() {
            std::fs::remove_file(file)?;
            return Err(std::io::Error::from_raw_os_error(1176));
        }
        swap(file, &temporary)
    })();
    match result {
        Ok(()) => Ok(()),
        Err(error) => recover(file, &temporary, error),
    }
}

/// After a failed [`replace`]: a swap that failed halfway (ERROR_UNABLE_TO_MOVE_REPLACEMENT,
/// when the new file is held open) has taken the file away and left its new contents under
/// the temporary name, which is moved in, tried for a second. The temporary file is the only
/// copy then and is never deleted; when this fails too, the error is [`LeftBehind`] and a
/// restore finds it ([`left_behind`]). A file that turns up meanwhile (the game wrote one) is
/// not overwritten. After any other failure the file is as it was, or gone because it was
/// deleted meanwhile and stays gone, and the temporary file goes.
pub(super) fn recover(file: &Path, temporary: &Path, error: std::io::Error) -> std::io::Result<()> {
    // ERROR_UNABLE_TO_MOVE_REPLACEMENT and ERROR_UNABLE_TO_MOVE_REPLACEMENT_2.
    let halfway = cfg!(windows) && matches!(error.raw_os_error(), Some(1176 | 1177));
    if !halfway || std::fs::symlink_metadata(file).is_ok() {
        let _ = std::fs::remove_file(temporary);
        return Err(error);
    }
    let mut tries = 0;
    loop {
        match move_in(temporary, file) {
            Ok(()) => return Ok(()),
            Err(_) if std::fs::symlink_metadata(file).is_ok() => {
                let _ = std::fs::remove_file(temporary);
                return Err(error);
            }
            Err(_) if tries < 10 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(again) => {
                return Err(std::io::Error::other(LeftBehind(format!(
                    "{error}; the file is gone and its new contents are in {}: {again}",
                    temporary.display()
                ))));
            }
        }
    }
}

/// A write whose file is gone, its new contents left under the temporary name ([`recover`]):
/// its journal entries stay, so the restore moves them back and puts the originals in.
#[derive(Debug)]
struct LeftBehind(String);
impl fmt::Display for LeftBehind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for LeftBehind {}

pub(super) fn left_behind_error(error: &std::io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(|inner| inner.is::<LeftBehind>())
}

/// Move `from` to `to`, failing when `to` exists rather than replacing it.
#[cfg(windows)]
pub(super) fn move_in(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[cfg(test)]
    if super::tests::HALFWAY.get() {
        return Err(std::io::Error::other("the new file is held open"));
    }
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    let wide = |path: &Path| -> Vec<u16> {
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    };
    let (from, to) = (wide(from), wide(to));
    if from[..from.len() - 1].contains(&0) || to[..to.len() - 1].contains(&0) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "file path contains a NUL",
        ));
    }
    // SAFETY: both paths are NUL-terminated UTF-16 buffers that outlive the call.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), MOVEFILE_WRITE_THROUGH) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(windows))]
pub(super) fn move_in(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::hard_link(from, to)?;
    std::fs::remove_file(from)
}

/// The new contents of `file` a [`replace`] that failed twice left under a temporary name (the
/// newest, when there are several), for a restore that finds the file gone.
fn left_behind(file: &Path) -> std::io::Result<Option<PathBuf>> {
    let (Some(parent), Some(name)) = (file.parent(), file.file_name()) else {
        return Ok(None);
    };
    let prefix = format!(".{}.", name.to_string_lossy());
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(parent)? {
        let entry = entry?;
        let named = entry.file_name();
        let ours = named
            .to_str()
            .and_then(|named| named.strip_prefix(&prefix))
            .and_then(|rest| rest.strip_suffix(".tmp"))
            .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok());
        // From the listing: a link is not taken.
        let metadata = entry.metadata()?;
        if !ours || !metadata.is_file() {
            continue;
        }
        let modified = metadata.modified()?;
        if newest.as_ref().is_none_or(|(time, _)| modified > *time) {
            newest = Some((modified, entry.path()));
        }
    }
    Ok(newest.map(|(_, path)| path))
}

/// A folder on the way to a game file is a link now ([`pin`]).
#[derive(Debug)]
#[cfg_attr(not(windows), allow(dead_code))]
struct Linked(PathBuf);
impl fmt::Display for Linked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} was replaced by a link", self.0.display())
    }
}
impl std::error::Error for Linked {}

pub(super) fn linked(error: &std::io::Error) -> bool {
    error.get_ref().is_some_and(|inner| inner.is::<Linked>())
}

/// Hold the folders from `folder` (the app's, [`Target::folder`]) down to `file`'s own open
/// without delete sharing, so none can be moved, or swapped for a link, until the handles drop.
/// One that is a link already fails with [`Linked`], and so does a folder above the app's that
/// is one now (the held folders keep those from moving too). Without `folder`, only the file's
/// own folder is held. Opened through the caller's access, so as the user.
#[cfg(windows)]
pub(super) fn pin(folder: &Path, file: &Path) -> std::io::Result<Vec<std::fs::File>> {
    use std::os::windows::fs::OpenOptionsExt;
    // Traverse counts as reading, so the handle holds its sharing; attributes to check it.
    const FILE_TRAVERSE: u32 = 0x20;
    const FILE_READ_ATTRIBUTES: u32 = 0x80;
    const SYNCHRONIZE: u32 = 0x0010_0000;
    const FILE_SHARE_READ: u32 = 0x1;
    const FILE_SHARE_WRITE: u32 = 0x2;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    let Some(parent) = file.parent() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the file has no folder",
        ));
    };
    let top = if folder.as_os_str().is_empty() {
        parent
    } else {
        folder
    };
    let Ok(below) = parent.strip_prefix(top) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the file is not in the app's folder",
        ));
    };
    let mut dir = top.to_path_buf();
    let mut pins = Vec::new();
    for name in std::iter::once(None).chain(below.components().map(Some)) {
        if let Some(name) = name {
            dir.push(name);
        }
        // The last name is not followed; those before it are held already.
        let pinned = std::fs::OpenOptions::new()
            .access_mode(FILE_TRAVERSE | FILE_READ_ATTRIBUTES | SYNCHRONIZE)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
            .open(&dir)?;
        let metadata = pinned.metadata()?;
        if metadata.is_symlink() || !metadata.is_dir() {
            return Err(std::io::Error::other(Linked(dir)));
        }
        pins.push(pinned);
    }
    // The first folder was opened through those above it, which could be links by now.
    if std::fs::canonicalize(parent)? != parent {
        return Err(std::io::Error::other(Linked(parent.to_path_buf())));
    }
    Ok(pins)
}

#[cfg(not(windows))]
pub(super) fn pin(_folder: &Path, _file: &Path) -> std::io::Result<Vec<std::fs::File>> {
    Ok(Vec::new())
}

/// The drive `path` is on: its prefix and root.
fn volume(path: &Path) -> PathBuf {
    path.components()
        .take_while(|part| matches!(part, Component::Prefix(_) | Component::RootDir))
        .collect()
}

#[cfg(windows)]
fn swap(file: &Path, replacement: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    unsafe extern "system" {
        fn ReplaceFileW(
            replaced: *const u16,
            replacement: *const u16,
            backup: *const u16,
            flags: u32,
            exclude: *mut std::ffi::c_void,
            reserved: *mut std::ffi::c_void,
        ) -> i32;
    }
    let wide = |path: &Path| -> std::io::Result<Vec<u16>> {
        let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        if wide.contains(&0) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "file path contains a NUL",
            ));
        }
        wide.push(0);
        Ok(wide)
    };
    let (file, replacement) = (wide(file)?, wide(replacement)?);
    // SAFETY: both paths are NUL-terminated UTF-16 buffers that outlive the call; no backup
    // name, no flags and the two reserved pointers null, as the API requires.
    let replaced = unsafe {
        ReplaceFileW(
            file.as_ptr(),
            replacement.as_ptr(),
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if replaced == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(windows))]
fn swap(file: &Path, replacement: &Path) -> std::io::Result<()> {
    std::fs::rename(replacement, file)
}

/// What a restore finds where a journaled file was.
pub(super) enum Found {
    File(Vec<u8>),
    /// A link or junction now: it is not followed.
    Link,
    /// The file is gone, or the app's folder, from a folder that is still there.
    Gone,
}

/// Look at `file`, in the app's `folder`, for a restore. A file that a [`replace`] which failed
/// twice left under its temporary name is moved back first. When the folder the app's folder
/// is in is missing too (a drive not mounted yet, or another drive under its letter), it is an
/// error, so the changes stay journaled.
pub(super) fn look_up(folder: &Path, file: &Path) -> std::io::Result<Found> {
    let found = pin(folder, file).and_then(|_pins| match std::fs::canonicalize(file) {
        Ok(resolved) if resolved == file => read(file).map(Found::File),
        Ok(_) => Ok(Found::Link),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => match left_behind(file)? {
            Some(temporary) => {
                move_in(&temporary, file)?;
                tracing::warn!(file = %file.display(), from = %temporary.display(), "a managed game file a failed write had left under a temporary name is back in place");
                read(file).map(Found::File)
            }
            None => Ok(Found::Gone),
        },
        Err(error) => Err(error),
    });
    match found {
        Err(error) if linked(&error) => Ok(Found::Link),
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound && on_its_drive(folder, file) =>
        {
            Ok(Found::Gone)
        }
        found => found,
    }
}

/// Whether the drive the app's `folder` was on is still there: some folder above it, below the
/// drive's root, exists. An uninstall can take several levels with it (the app's folder is often
/// `Game\Binaries\Win64`); a drive that is not mounted, or another drive under its letter, has
/// only its root.
fn on_its_drive(folder: &Path, file: &Path) -> bool {
    let root = volume(file);
    if folder.as_os_str().is_empty() {
        return root.is_dir();
    }
    folder
        .ancestors()
        .skip(1)
        .take_while(|above| above.starts_with(&root) && *above != root)
        .any(Path::is_dir)
}
