//! Folder ownership, permissions, and process-start holding for the signed-in
//! user's own copy of Nucleus Co-op.
use super::{DIRECTORY, REPARSE_POINT, Steps, install};
use anyhow::{Context, Result, ensure};
use butterpollo_session::nucleus::install as pinned;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// The user's copy at `copy`, open while Nucleus Co-op starts from it:
/// SYSTEM opens the program and the working folder by their paths, so
/// neither may be a link. The folders above `<key>` are SYSTEM's and
/// Administrators', and the user can't rename or delete `<key>` itself.
/// Held, `<key>` is shared for reading only, so nobody can open it to make
/// it a link, rename or delete it; `NucleusCoop.exe` can't be written,
/// renamed or deleted. Both until this drops.
pub struct Held {
    _folder: std::fs::File,
    _program: std::fs::File,
}

/// [`Held`] for the user `sid`'s copy at `copy`: an error when `<key>` is a
/// link or not the host's ([`secured`]), or its `NucleusCoop.exe` is a
/// link, a folder, a hard link to another file or not in the folder held.
pub fn hold(copy: &Path, sid: &str) -> Result<Held> {
    hold_with(copy, sid, secured)
}

pub(super) fn hold_with(
    copy: &Path,
    sid: &str,
    check: fn(&Path, &[&str]) -> Result<std::fs::File>,
) -> Result<Held> {
    let folder = check(copy, &[sid])?;
    let program = copy.join(pinned::EXECUTABLE);
    let file = open_program(&program)
        .and_then(|file| {
            in_folder(&folder, &file)?;
            Ok(file)
        })
        .with_context(|| {
            format!(
                "{} is not a program file Rubylight starts the co-op helper from: start the stream again, which copies it again",
                program.display()
            )
        })?;
    Ok(Held {
        _folder: folder,
        _program: file,
    })
}

/// An error unless the file `program` has open is `NucleusCoop.exe` (in any
/// case) right in the folder `folder` has open, by the paths Windows
/// resolved both to.
pub(super) fn in_folder(folder: &std::fs::File, program: &std::fs::File) -> Result<()> {
    let folder = crate::user_folder::final_path(folder).context("reading the copy's path")?;
    let found = crate::user_folder::final_path(program).context("reading the program's path")?;
    let found = PathBuf::from(found);
    let named = found.file_name().is_some_and(|name| {
        crate::user_folder::same_name(name, std::ffi::OsStr::new(pinned::EXECUTABLE))
    });
    ensure!(
        named && found.parent() == Some(Path::new(&folder)),
        "it opened as {}, not in {}",
        found.display(),
        Path::new(&folder).display()
    );
    Ok(())
}

/// The folder `path` opened to check and hold: a link is opened itself,
/// never followed, and it is shared for reading only. Nobody can open it to
/// write (which can make an empty folder a link), rename or delete it while
/// it is open; files and folders can still be added to it and removed.
pub(super) fn open_held(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY,
        FILE_READ_ATTRIBUTES, FILE_SHARE_READ, READ_CONTROL,
    };
    std::fs::OpenOptions::new()
        .access_mode(FILE_LIST_DIRECTORY.0 | READ_CONTROL.0 | FILE_READ_ATTRIBUTES.0)
        .share_mode(FILE_SHARE_READ.0)
        .custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0)
        .open(path)
}

/// The file `path`, never through a link, open for reading only and shared
/// only for reading: it can't be written, renamed or deleted while it is
/// open. Refused when it is a link, a folder or one of several names of a
/// file.
pub(super) fn open_program(path: &Path) -> Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_TYPE_DISK,
        GetFileInformationByHandle, GetFileType,
    };
    let file = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ.0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    let handle = HANDLE(file.as_raw_handle());
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: the handle is open for as long as `file` is, and `info` is
    // writable.
    unsafe { GetFileInformationByHandle(handle, &mut info) }
        .with_context(|| format!("reading {}", path.display()))?;
    // SAFETY: as above.
    let disk = unsafe { GetFileType(handle) } == FILE_TYPE_DISK;
    ensure!(
        info.dwFileAttributes & (REPARSE_POINT | DIRECTORY) == 0 && disk,
        "{} is a link or a folder",
        path.display()
    );
    ensure!(
        info.nNumberOfLinks == 1,
        "{} has {} names",
        path.display(),
        info.nNumberOfLinks
    );
    Ok(file)
}

/// The folder `path` with `sddl`'s permissions, made now or checked
/// ([`secured`]) when it is there, and open: it can't be renamed while the
/// returned handle is.
pub(super) fn made(path: &Path, sddl: &str, user: &[&str], steps: &Steps) -> Result<std::fs::File> {
    match (steps.create)(path, sddl) {
        Ok(()) => {
            tracing::info!(folder = %path.display(), "Co-op helper: folder for the users' own copies made")
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error).with_context(|| format!("creating {}", path.display())),
    }
    (steps.check)(path, user)
}

/// The folder at `path`, held open ([`open_held`]), when it is no link,
/// SYSTEM or Administrators own it, nobody but them may move, delete or
/// re-permission it, and nobody but them and `user` may change it.
/// Otherwise someone else made it: the host never writes through it.
pub(super) fn secured(path: &Path, user: &[&str]) -> Result<std::fs::File> {
    let file = open_held(path).with_context(|| format!("opening {}", path.display()))?;
    let attributes = file
        .metadata()
        .with_context(|| format!("reading {}", path.display()))?
        .file_attributes();
    let owner = crate::process::open_file_owner(&file)
        .with_context(|| format!("reading the owner of {}", path.display()))?;
    let writers = crate::process::open_file_writers(&file)
        .with_context(|| format!("reading the permissions of {}", path.display()))?;
    let movers = crate::process::open_file_movers(&file)
        .with_context(|| format!("reading the permissions of {}", path.display()))?;
    verdict(path, attributes, &owner, (&writers, &movers), user)?;
    Ok(file)
}

/// Whether the folder at `path` may be used, from its `attributes`, its
/// `owner`, and who may change it and who may move it (`writers`,
/// `movers`): see [`secured`].
pub(super) fn verdict(
    path: &Path,
    attributes: u32,
    owner: &str,
    (writers, movers): (&[String], &[String]),
    user: &[&str],
) -> Result<()> {
    let refused = |why: String| {
        anyhow::anyhow!(
            "{why}, so Rubylight doesn't use it for the co-op helper: an administrator of this PC can delete {}, then start the stream again",
            path.display()
        )
    };
    if attributes & REPARSE_POINT != 0 {
        return Err(refused(format!(
            "{} is a link to elsewhere",
            path.display()
        )));
    }
    if !install::TRUSTED.contains(&owner) {
        return Err(refused(format!(
            "{} is owned by {owner}, not by SYSTEM or Administrators",
            path.display()
        )));
    }
    if let Some(other) = writers
        .iter()
        .find(|sid| !install::TRUSTED.contains(&sid.as_str()) && !user.contains(&sid.as_str()))
    {
        return Err(refused(format!(
            "{} can be changed by {other}",
            path.display()
        )));
    }
    if let Some(other) = movers
        .iter()
        .find(|sid| !install::TRUSTED.contains(&sid.as_str()))
    {
        return Err(refused(format!(
            "{} can be moved or deleted by {other}",
            path.display()
        )));
    }
    Ok(())
}
