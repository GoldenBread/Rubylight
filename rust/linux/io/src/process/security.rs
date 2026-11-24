//! Owners and permissions of files and folders, in Unix terms: an account is named by its user ID in decimal (`1000`, root is `0`), a group by `gid:<id>`, and everyone by `everyone`. Access control lists are not read, an open folder is not kept from being renamed, and Windows permission descriptors have no meaning here.

use anyhow::{Context, Result};
use std::{
    ffi::CStr,
    fs::File,
    io,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::Path,
};

/// Everyone on the PC, in a list of who may change a file.
const EVERYONE: &str = "everyone";

/// The owner of the file or folder `file` has open, as its user ID in decimal: of the
/// object then read through it, which the path could have been swapped for since.
pub fn open_file_owner(file: &File) -> Result<String> {
    Ok(file
        .metadata()
        .context("reading the owner")?
        .uid()
        .to_string())
}

/// `path` opened to check and read through one descriptor (`O_NOFOLLOW`): a link at
/// `path` is refused, never followed, and with `folder` anything but a folder is. Linux
/// lets an open file be written and an open folder be renamed or deleted; what stays
/// fixed is the object the descriptor names.
pub fn open_unfollowed(path: &Path, folder: bool) -> io::Result<File> {
    let mut flags = libc::O_NOFOLLOW | libc::O_CLOEXEC;
    if folder {
        flags |= libc::O_DIRECTORY;
    }
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(flags)
        .open(path)
        .map_err(|error| {
            // The kernel says ELOOP for a link, or ENOTDIR when a folder was asked for.
            let link = matches!(
                error.raw_os_error(),
                Some(libc::ELOOP) | Some(libc::ENOTDIR)
            ) && std::fs::symlink_metadata(path).is_ok_and(|found| found.is_symlink());
            if link {
                io::Error::new(
                    error.kind(),
                    format!(
                        "{} is a link to elsewhere, which is not followed",
                        path.display()
                    ),
                )
            } else {
                error
            }
        })
}

/// Folders held open. Their descriptors keep the folders themselves alive while the hold
/// lives; unlike on Windows, an open folder can still be renamed or deleted, so a hold
/// does not fix its path.
#[derive(Clone, Debug, Default)]
pub struct Hold {
    /// Held for as long as this lives, never read.
    _folders: std::sync::Arc<Vec<File>>,
}
impl Hold {
    /// `folders`, each held open. Never follows a link.
    pub fn new<'a>(folders: impl IntoIterator<Item = &'a Path>) -> io::Result<Self> {
        let held = folders
            .into_iter()
            .map(|folder| open_unfollowed(folder, true))
            .collect::<io::Result<Vec<_>>>()?;
        Ok(Self {
            _folders: std::sync::Arc::new(held),
        })
    }
}

/// The name of the account `sid` names (a user ID in decimal, `gid:<id>` or
/// `everyone`), from the system's user and group databases; None for one they don't know.
pub fn account_name(sid: &str) -> Option<String> {
    if sid == EVERYONE {
        return Some(EVERYONE.into());
    }
    if let Some(group) = sid.strip_prefix("gid:") {
        return group_name(group.parse().ok()?);
    }
    user_name(sid.parse().ok()?)
}

fn user_name(uid: u32) -> Option<String> {
    let mut buffer = vec![0u8; 4096];
    // SAFETY: an all-zero passwd is a valid value of that plain C structure.
    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut found = std::ptr::null_mut();
    // SAFETY: `entry`, `buffer` (with its length) and `found` are live locals the call
    // writes; the strings it leaves in `entry` point into `buffer`.
    let status = unsafe {
        libc::getpwuid_r(
            uid,
            &mut entry,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut found,
        )
    };
    if status != 0 || found.is_null() || entry.pw_name.is_null() {
        return None;
    }
    // SAFETY: on success `pw_name` is a NUL-terminated string inside `buffer`, which is
    // still alive.
    let name = unsafe { CStr::from_ptr(entry.pw_name) };
    Some(name.to_string_lossy().into_owned())
}

fn group_name(gid: u32) -> Option<String> {
    let mut buffer = vec![0u8; 4096];
    // SAFETY: an all-zero group is a valid value of that plain C structure.
    let mut entry: libc::group = unsafe { std::mem::zeroed() };
    let mut found = std::ptr::null_mut();
    // SAFETY: `entry`, `buffer` (with its length) and `found` are live locals the call
    // writes; the strings it leaves in `entry` point into `buffer`.
    let status = unsafe {
        libc::getgrgid_r(
            gid,
            &mut entry,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut found,
        )
    };
    if status != 0 || found.is_null() || entry.gr_name.is_null() {
        return None;
    }
    // SAFETY: on success `gr_name` is a NUL-terminated string inside `buffer`, which is
    // still alive.
    let name = unsafe { CStr::from_ptr(entry.gr_name) };
    Some(name.to_string_lossy().into_owned())
}

/// Give only the account this host runs as access to `folder` and the files directly in
/// it (modes 0700 and 0600): what stands for Windows' "SYSTEM and Administrators only" on
/// Linux, where the host's own user keeps its keys.
pub fn restrict_to_administrators(folder: &Path) -> Result<()> {
    let private = |path: &Path, mode: u32| {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
            .with_context(|| format!("restricting {}", path.display()))
    };
    private(folder, 0o700)?;
    for entry in
        std::fs::read_dir(folder).with_context(|| format!("listing {}", folder.display()))?
    {
        let entry = entry?;
        // From the listing itself: a link is left alone, as its target is not in the folder.
        let kind = entry.file_type()?;
        if kind.is_file() {
            private(&entry.path(), 0o600)?;
        } else if kind.is_dir() {
            private(&entry.path(), 0o700)?;
        }
    }
    Ok(())
}

/// The permissions a Windows security descriptor (`sddl`) describes on `folder`. Linux
/// has no such descriptors, so this fails and changes nothing.
pub fn restrict(_folder: &Path, _sddl: &str) -> Result<()> {
    Err(crate::unsupported("Applying a Windows security descriptor"))
}

/// Create the folder `path` for this host's account to write and everyone to read (mode
/// 0755), with that mode from the start. AlreadyExists when anything has its name, which
/// is left as it is.
pub fn create_folder_administrators_readable(path: &Path) -> io::Result<()> {
    create_folder(path, 0o755)
}

/// Create the folder `path` as [`create_folder_administrators_readable`] does, for this
/// host's account alone (mode 0700): nobody else can open it, not even between its
/// creation and its permissions.
pub fn create_folder_administrators_only(path: &Path) -> io::Result<()> {
    create_folder(path, 0o700)
}

/// Create the folder `path` with `mode`. The process's umask can only take rights away
/// while it is made; the mode asked for is set right after.
fn create_folder(path: &Path, mode: u32) -> io::Result<()> {
    std::fs::DirBuilder::new().mode(mode).create(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

/// Create the new file `path` for this host's account to write and everyone to read
/// (mode 0644), open for writing. AlreadyExists when anything has its name, a link too,
/// which is never followed. Linux does not keep others from opening it meanwhile.
pub fn create_file_administrators_readable(path: &Path) -> io::Result<File> {
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    file.set_permissions(std::fs::Permissions::from_mode(0o644))?;
    Ok(file)
}

/// Who may write the file or folder `file` has open or change its permissions, by its
/// owner and mode: its owner always (who can change the mode), its group (`gid:<id>`) and
/// `everyone` when the mode lets them write. Access control lists are not read.
pub fn open_file_writers(file: &File) -> Result<Vec<String>> {
    let metadata = file.metadata().context("reading the permissions")?;
    let mut writers = vec![metadata.uid().to_string()];
    if metadata.mode() & 0o020 != 0 {
        writers.push(format!("gid:{}", metadata.gid()));
    }
    if metadata.mode() & 0o002 != 0 {
        writers.push(EVERYONE.into());
    }
    Ok(writers)
}

/// Who may move or delete what the folder `file` has open holds, or change the folder's
/// permissions: on Linux those who may write the folder ([`open_file_writers`]). A sticky
/// folder (`/tmp`) lets each of them move only their own files, which this does not tell.
pub fn open_file_child_movers(file: &File) -> Result<Vec<String>> {
    open_file_writers(file)
}

/// Who may change the file or folder `file` has open, or what a folder holds
/// ([`open_file_child_movers`]). Who may move or delete the object itself is decided by
/// the folder it is in, which an open descriptor does not name: check that folder too.
pub fn open_file_movers(file: &File) -> Result<Vec<String>> {
    open_file_writers(file)
}
