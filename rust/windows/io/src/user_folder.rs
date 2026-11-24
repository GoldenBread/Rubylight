//! The folder file transfers land in, opened as the signed-in user: a host
//! running as SYSTEM must not create files the user can't touch, nor reach
//! files the user can't. Handles are opened while impersonating the user's
//! token, so Windows checks the user's rights, and then read and written
//! without it. An elevated host acts as the same user without administrator
//! rights; a host without them already is the user.
//!
//! Every call blocks and impersonates on the calling thread: run them on a
//! blocking thread, not an async worker.
use butterpollo_session::file_transfer::{self, NameError};
#[cfg(test)]
use std::os::windows::{
    ffi::OsStrExt,
    io::{AsRawHandle, FromRawHandle},
};
use std::{
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::windows::{
        fs::{MetadataExt, OpenOptionsExt},
        io::OwnedHandle,
    },
    path::{Path, PathBuf},
    sync::Arc,
    time::UNIX_EPOCH,
};
#[cfg(test)]
use windows::{
    Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_NO_TOKEN, HANDLE},
    core::HRESULT,
};
use windows::{
    Win32::{
        Foundation::ERROR_SHARING_VIOLATION,
        Storage::FileSystem::{
            DELETE, FILE_ATTRIBUTE_HIDDEN, FILE_FLAG_BACKUP_SEMANTICS,
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_FLAGS_AND_ATTRIBUTES, FILE_GENERIC_WRITE,
            FILE_SHARE_READ, GetDiskFreeSpaceExW,
        },
    },
    core::HSTRING,
};

mod handle;
mod impersonation;

pub(crate) use handle::{
    delete_on_close, final_path, plain, raw, reopen_for_data, same_name, verbatim,
};
use handle::{final_name, mark_of_the_web, rename_open, unmark};
use impersonation::downloads;
#[cfg(test)]
use impersonation::standard_user;
pub(crate) use impersonation::{
    Impersonation, as_token, known_folder, no_signed_in_user, os_error, user_token,
};
pub use impersonation::{as_session_user, as_user, roaming_app_data};

/// The subfolder of the user's Downloads folder transfers use by default.
pub const DEFAULT_SUBFOLDER: &str = "Rubylight";
/// How many numbered names an upload tries before it gives up.
const MAX_COLLISIONS: u32 = 9999;
/// Marks this host's `.part` files until they are renamed, so a later open
/// of the file transfer folder sweeps those a stopped host left behind and
/// nobody else's (shared folders are never swept). Unlike most
/// attributes, a new file doesn't inherit it from its folder.
const PART_MARKER: FILE_FLAGS_AND_ATTRIBUTES = FILE_ATTRIBUTE_HIDDEN;
/// The Mark of the Web: Windows treats an upload like a download from the
/// internet (SmartScreen, Office Protected View).
const ZONE_IDENTIFIER: &[u8] = b"[ZoneTransfer]\r\nZoneId=3\r\n";

#[derive(Debug)]
pub enum UserFolderError {
    /// Nobody is signed in to the host's Windows session, so there is no
    /// user to write as.
    NoConsoleUser,
    /// The host runs elevated and the Windows desktop shell isn't running,
    /// so there is no token of the user without administrator rights. The
    /// folder is refused rather than written with administrator rights.
    NoUserShell,
    InvalidName(NameError),
    /// The name is a folder or a link (a symbolic link or a junction).
    NotPlainFile,
    /// What Windows opened is not the path asked for below a shared folder's
    /// root: a link or mount point on the way leads elsewhere, or a name was
    /// another name (8.3) of the file.
    OutsideFolder,
    Io(io::Error),
}
impl std::fmt::Display for UserFolderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoConsoleUser => f.write_str("nobody is signed in to Windows"),
            Self::NoUserShell => f.write_str(
                "file transfer cannot act as the Windows user: the host runs with administrator rights and the Windows desktop shell is not running. Start Rubylight without administrator rights, or use the installed Windows service",
            ),
            Self::InvalidName(error) => error.fmt(f),
            Self::NotPlainFile => f.write_str("not a plain file"),
            Self::OutsideFolder => f.write_str(
                "the path leaves the shared folder through a link, or names a file by another name",
            ),
            Self::Io(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for UserFolderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidName(error) => Some(error),
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}
impl From<io::Error> for UserFolderError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
impl From<NameError> for UserFolderError {
    fn from(error: NameError) -> Self {
        Self::InvalidName(error)
    }
}

/// A file in the transfer folder.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct FileEntry {
    pub name: String,
    pub size: u64,
    pub modified_unix_ms: u64,
}

/// The transfer folder and the user it belongs to.
#[derive(Clone, Debug)]
pub struct UserFolder {
    path: PathBuf,
    token: Option<Arc<OwnedHandle>>,
    /// A shared folder's directory, held open without delete sharing so it
    /// can't be moved or swapped for a link while an upload uses it.
    _pin: Option<Arc<File>>,
}
impl UserFolder {
    /// The transfer folder, created if missing: `custom` when the host's
    /// settings name one, otherwise the user's `Downloads\Rubylight`. Open
    /// it for each transfer, since the signed-in user can change.
    pub fn open(custom: Option<&Path>) -> Result<Self, UserFolderError> {
        Self::open_as(custom, user_token)
    }

    /// [`UserFolder::open`] with the user's token from `token`. The folder
    /// setting is checked first, so a bad one is named whoever runs the host.
    fn open_as(
        custom: Option<&Path>,
        token: impl FnOnce() -> Result<Option<OwnedHandle>, UserFolderError>,
    ) -> Result<Self, UserFolderError> {
        if let Some(path) = custom.filter(|path| !path.is_absolute()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "the file transfer folder {} is not absolute",
                    path.display()
                ),
            )
            .into());
        }
        let token = token()?.map(Arc::new);
        let path = match custom {
            Some(path) => path.to_path_buf(),
            None => downloads(token.as_deref())?.join(DEFAULT_SUBFOLDER),
        };
        Self::at(path, token)
    }

    /// The folder at `path`, opened as this process rather than as the
    /// signed-in user. Only for tests of the callers (the `test-folder`
    /// feature): a test host may run elevated without a desktop shell,
    /// where [`UserFolder::open`] refuses.
    #[cfg(feature = "test-folder")]
    pub fn unimpersonated(path: &Path) -> Result<Self, UserFolderError> {
        Self::at(path.to_path_buf(), None)
    }

    fn at(path: PathBuf, token: Option<Arc<OwnedHandle>>) -> Result<Self, UserFolderError> {
        let folder = Self {
            path,
            token,
            _pin: None,
        };
        folder.as_user(|| {
            fs::create_dir_all(&folder.path)?;
            folder.sweep()
        })?;
        Ok(folder)
    }

    /// A directory of a shared folder that `pin` holds open, as the folder
    /// uploads go to. Nothing is created and nothing swept: the folder is
    /// the user's, so a hidden `.part` file there can be anyone's. One a
    /// stopped host left behind stays, hidden and never listed.
    pub(crate) fn pinned(path: PathBuf, token: Option<Arc<OwnedHandle>>, pin: File) -> Self {
        Self {
            path,
            token,
            _pin: Some(Arc::new(pin)),
        }
    }

    /// Deletes the `.part` files of uploads a stopped host left behind.
    /// One still being written is open without delete sharing, so it fails
    /// with a sharing violation and stays.
    fn sweep(&self) -> io::Result<()> {
        for entry in fs::read_dir(&self.path)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            let name = entry.file_name();
            if !plain(&metadata)
                || metadata.file_attributes() & PART_MARKER.0 == 0
                || !name.to_str().is_some_and(file_transfer::has_part_suffix)
            {
                continue;
            }
            match fs::remove_file(entry.path()) {
                Ok(()) => tracing::info!(
                    "File transfer deleted the incomplete upload {}",
                    name.display()
                ),
                Err(error) if error.raw_os_error() == Some(ERROR_SHARING_VIOLATION.0 as i32) => {}
                Err(error) => tracing::warn!(
                    "File transfer could not delete the incomplete upload {}: {error}",
                    name.display()
                ),
            }
        }
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn as_user<T>(&self, work: impl FnOnce() -> io::Result<T>) -> Result<T, UserFolderError> {
        let _user = Impersonation::begin(self.token.as_deref())?;
        Ok(work()?)
    }

    /// Starts an upload of `name`: a new `.part` file under the first
    /// numbered name whose `.part` is free, so neither an upload in flight
    /// nor a leftover `.part` is overwritten.
    pub fn create_part(&self, name: &str) -> Result<PartFile, UserFolderError> {
        file_transfer::check_name(name)?;
        let (file, part) = self.as_user(|| {
            for n in 0..=MAX_COLLISIONS {
                let part =
                    self.path
                        .join(file_transfer::part_name(&file_transfer::collision_name(
                            name, n,
                        )));
                // CREATE_NEW never follows a link at that name: it fails.
                // Delete access renames or deletes the part through this
                // handle, without delete sharing nobody else can.
                match OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .access_mode(FILE_GENERIC_WRITE.0 | DELETE.0)
                    .share_mode(FILE_SHARE_READ.0)
                    .attributes(PART_MARKER.0)
                    .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
                    .open(&part)
                {
                    Ok(file) => return Ok((file, part)),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(error),
                }
            }
            Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("every numbered name of {name} is taken"),
            ))
        })?;
        Ok(PartFile {
            file: Some(file),
            part: Some(part),
            name: name.to_owned(),
            folder: self.clone(),
        })
    }

    /// Opens a plain file in the folder for reading, with its length. Only
    /// by its own name: not by its 8.3 short name, which could reach an
    /// upload in progress.
    pub fn open_file(&self, name: &str) -> Result<(File, u64), UserFolderError> {
        file_transfer::check_stored_name(name)?;
        let path = self.path.join(name);
        // Backup semantics let a folder open too, so it is refused below
        // like a link rather than failing with access denied.
        let file = self.as_user(|| {
            OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ.0)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0 | FILE_FLAG_BACKUP_SEMANTICS.0)
                .open(&path)
        })?;
        if !plain(&file.metadata()?) {
            return Err(UserFolderError::NotPlainFile);
        }
        let actual = final_name(&file)?;
        if !same_name(&actual, OsStr::new(name)) {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{name} is another name of {}", actual.display()),
            )
            .into());
        }
        reopen_for_data(self.token.as_deref(), &file)
    }

    /// The plain files in the folder, by name. Subfolders, links, uploads
    /// in progress (`.part`) and names a client couldn't ask for are left
    /// out.
    pub fn list(&self) -> Result<Vec<FileEntry>, UserFolderError> {
        self.as_user(|| {
            let mut files = Vec::new();
            for entry in fs::read_dir(&self.path)? {
                let entry = entry?;
                // From the directory listing itself: links are not followed.
                let metadata = entry.metadata()?;
                if !plain(&metadata) {
                    continue;
                }
                let Ok(name) = entry.file_name().into_string() else {
                    continue;
                };
                // Refuses `.part` names too.
                if file_transfer::check_stored_name(&name).is_err() {
                    continue;
                }
                let modified_unix_ms = metadata
                    .modified()?
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |since| {
                        u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
                    });
                files.push(FileEntry {
                    name,
                    size: metadata.len(),
                    modified_unix_ms,
                });
            }
            files.sort_by(|a, b| a.name.cmp(&b.name));
            Ok(files)
        })
    }

    /// The bytes the user may still write to the folder's volume, after
    /// any disk quota.
    pub fn free_bytes(&self) -> Result<u64, UserFolderError> {
        let mut path = verbatim(&self.path)?;
        if path.last() != Some(&u16::from(b'\\')) {
            path.push(u16::from(b'\\'));
        }
        let path = HSTRING::from_wide(&path);
        self.as_user(|| {
            let mut free = 0u64;
            // SAFETY: path is NUL-terminated and outlives the call; free is a
            // writable u64.
            unsafe { GetDiskFreeSpaceExW(&path, Some(&mut free), None, None) }.map_err(os_error)?;
            Ok(free)
        })
    }

    /// The bytes the user may still write to the folder's volume and the
    /// volume's size as the user sees it, both after any disk quota.
    pub fn volume_bytes(&self) -> Result<(u64, u64), UserFolderError> {
        let mut path = verbatim(&self.path)?;
        if path.last() != Some(&u16::from(b'\\')) {
            path.push(u16::from(b'\\'));
        }
        let path = HSTRING::from_wide(&path);
        self.as_user(|| {
            let (mut free, mut total) = (0u64, 0u64);
            // SAFETY: path is NUL-terminated and outlives the call; free and
            // total are writable u64s.
            unsafe { GetDiskFreeSpaceExW(&path, Some(&mut free), Some(&mut total), None) }
                .map_err(os_error)?;
            Ok((free, total))
        })
    }
}

/// An upload in progress, written to its `.part` file through a handle
/// opened as the user, then renamed and deleted through that handle, so a
/// link put at its path later can't redirect either. Dropping it before
/// [`PartFile::finish`] deletes the `.part` file; [`PartFile::discard`]
/// does the same and reports a failure. A `.part` file a failed deletion
/// leaves behind is swept by the next [`UserFolder::open`].
///
/// Dropping it, like every call here, does blocking I/O: drop it on a
/// blocking thread.
#[derive(Debug)]
pub struct PartFile {
    file: Option<File>,
    part: Option<PathBuf>,
    name: String,
    folder: UserFolder,
}
impl PartFile {
    pub fn part_path(&self) -> Option<&Path> {
        self.part.as_deref()
    }
    /// The open `.part` file, until it is finished or discarded.
    pub(crate) fn file(&self) -> Option<&File> {
        self.file.as_ref()
    }

    /// Closes the upload and gives it the first free numbered name, which
    /// is returned. If the rename fails the `.part` file is deleted.
    pub fn finish(mut self) -> Result<String, UserFolderError> {
        let (Some(file), Some(part)) = (self.file.as_mut(), self.part.as_deref()) else {
            return Err(io::Error::other("the upload is already closed").into());
        };
        file.flush()?;
        // Not synced to disk before the rename: that would hold up the
        // reply for seconds on a large upload, and a power cut right after
        // could leave the named file short, as with a browser download.
        if let Err(error) = self.folder.as_user(|| mark_of_the_web(part)) {
            // As on FAT or exFAT, which can't hold the mark.
            tracing::warn!(
                "File transfer could not mark {} as downloaded from the internet: {error}",
                self.name
            );
        }
        unmark(file)?;
        let file = &*file;
        let name = self.folder.as_user(|| {
            for n in 0..=MAX_COLLISIONS {
                let candidate = file_transfer::collision_name(&self.name, n);
                match rename_open(file, &self.folder.path.join(&candidate)) {
                    Ok(()) => return Ok(candidate),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(error),
                }
            }
            Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("every numbered name of {} is taken", self.name),
            ))
        })?;
        self.part = None;
        drop(self.file.take());
        Ok(name)
    }

    /// Ends an incomplete upload by deleting its `.part` file.
    pub fn discard(mut self) -> Result<(), UserFolderError> {
        self.remove()
    }

    fn remove(&mut self) -> Result<(), UserFolderError> {
        self.part = None;
        let Some(file) = self.file.take() else {
            return Ok(());
        };
        // Through the handle, so only this upload's file goes; deleted
        // when the handle closes right after.
        delete_on_close(&file)?;
        Ok(())
    }
}
impl Write for PartFile {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match &mut self.file {
            Some(file) => file.write(bytes),
            None => Err(io::Error::other("the upload is closed")),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match &mut self.file {
            Some(file) => file.flush(),
            None => Ok(()),
        }
    }
}
impl Drop for PartFile {
    fn drop(&mut self) {
        if let Err(error) = self.remove() {
            tracing::warn!(
                "File transfer could not delete the incomplete upload of {}: {error}",
                self.name
            );
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
