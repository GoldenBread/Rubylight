//! The folder file transfers land in, and the helpers shared folders build on. A Linux host runs as the signed-in user, so the folder is the user's own `Downloads/Rubylight`, opened and written as this process. Links are never followed where the Windows code refuses them, uploads are written to a `.part` file that is renamed when complete, and no mark of the web is added.
//!
//! Every call blocks: run them on a blocking thread, not an async worker.

use butterpollo_session::file_transfer::{self, NameError};
use std::{
    ffi::CString,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt},
        },
    },
    path::{Path, PathBuf},
    sync::Arc,
    time::UNIX_EPOCH,
};

/// The subfolder of the user's Downloads folder transfers use by default.
pub const DEFAULT_SUBFOLDER: &str = "Rubylight";
/// How many numbered names an upload tries before it gives up.
const MAX_COLLISIONS: u32 = 9999;
/// Marks this host's `.part` files until they are renamed, so a later open of the transfer
/// folder sweeps those a stopped host left behind and nobody else's.
const PART_MARKER: &[u8] = b"user.rubylight.part\0";

#[derive(Debug)]
pub enum UserFolderError {
    /// Nobody is signed in, so there is no user to write as.
    NoConsoleUser,
    /// There is no token of the user without administrator rights. Never on Linux.
    NoUserShell,
    InvalidName(NameError),
    /// The name is a folder or a link.
    NotPlainFile,
    /// What was opened is not the path asked for below a shared folder's root: a link on the
    /// way leads elsewhere.
    OutsideFolder,
    Io(io::Error),
}
impl std::fmt::Display for UserFolderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoConsoleUser => f.write_str("nobody is signed in to this PC"),
            Self::NoUserShell => f.write_str(
                "file transfer cannot act as the signed-in user: the host has no way to find out who that is",
            ),
            Self::InvalidName(error) => error.fmt(f),
            Self::NotPlainFile => f.write_str("not a plain file"),
            Self::OutsideFolder => {
                f.write_str("the path leaves the shared folder through a link")
            }
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

/// Runs `work` as the signed-in user: this process already is that user.
pub fn as_user<T>(work: impl FnOnce() -> T) -> Result<T, UserFolderError> {
    Ok(work())
}

/// Runs `work` as the user signed in to the host's session, which this process is.
pub fn as_session_user<T>(work: impl FnOnce() -> T) -> io::Result<T> {
    Ok(work())
}

fn home() -> Result<PathBuf, UserFolderError> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|home| home.is_absolute())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "the signed-in user's home folder is unknown (HOME is not set)",
            )
            .into()
        })
}

fn config_home(home: &Path) -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| home.join(".config"))
}

/// The folder `XDG_DOWNLOAD_DIR` names in the text of `user-dirs.dirs`.
fn download_dir_in(text: &str, home: &Path) -> Option<PathBuf> {
    let line = text
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("XDG_DOWNLOAD_DIR="))?;
    let value = line.trim().trim_matches('"');
    let path = match value.strip_prefix("$HOME") {
        Some(rest) => home.join(rest.trim_start_matches('/')),
        None => PathBuf::from(value),
    };
    path.is_absolute().then_some(path)
}

/// The user's Downloads folder, wherever they moved it.
fn downloads() -> Result<PathBuf, UserFolderError> {
    let home = home()?;
    let configured = fs::read_to_string(config_home(&home).join("user-dirs.dirs"))
        .ok()
        .and_then(|text| download_dir_in(&text, &home));
    Ok(configured.unwrap_or_else(|| home.join("Downloads")))
}

/// The folder where per-user program settings live (`~/.config`), where Nucleus Co-op keeps
/// the user's game list on Windows.
pub fn roaming_app_data() -> Result<PathBuf, UserFolderError> {
    Ok(config_home(&home()?))
}

/// A plain file: not a folder or a link.
pub(crate) fn plain(metadata: &fs::Metadata) -> bool {
    metadata.is_file()
}

pub(crate) fn c_path(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "the path holds a NUL"))
}

/// Opens `name` inside the folder `dir` has open, never following a link at `name`.
pub(crate) fn open_below(dir: &File, name: &str, flags: i32) -> io::Result<File> {
    let name = CString::new(name)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "the name holds a NUL"))?;
    // SAFETY: `name` is NUL-terminated and outlives the call; `dir` is an open descriptor.
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` is a new descriptor nothing else owns.
    Ok(File::from(unsafe { OwnedFd::from_raw_fd(fd) }))
}

/// Whether `name` in the folder `dir` has open is a link.
pub(crate) fn is_link_below(dir: &File, name: &str) -> bool {
    let Ok(name) = CString::new(name) else {
        return false;
    };
    // SAFETY: an all-zero `stat` is a valid value the call overwrites.
    let mut found: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: `name` is NUL-terminated, `found` is a live local and `dir` is open.
    let status = unsafe {
        libc::fstatat(
            dir.as_raw_fd(),
            name.as_ptr(),
            &mut found,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    status == 0 && found.st_mode & libc::S_IFMT == libc::S_IFLNK
}

/// The path through which the folder `dir` has open is reached, wherever it has been moved.
pub(crate) fn fd_path(dir: &File) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{}", dir.as_raw_fd()))
}

pub(crate) fn modified_ms(metadata: &fs::Metadata) -> io::Result<u64> {
    Ok(metadata
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        }))
}

fn lock(file: &File) -> io::Result<()> {
    flock(file.as_raw_fd())
}

fn flock(fd: RawFd) -> io::Result<()> {
    // SAFETY: `fd` is an open descriptor for the call.
    if unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn mark(file: &File) {
    // SAFETY: the name is NUL-terminated; the value is one byte that outlives the call.
    unsafe {
        libc::fsetxattr(
            file.as_raw_fd(),
            PART_MARKER.as_ptr().cast(),
            b"1".as_ptr().cast(),
            1,
            0,
        );
    }
}

fn unmark(file: &File) {
    // SAFETY: the name is NUL-terminated and the descriptor is open.
    unsafe {
        libc::fremovexattr(file.as_raw_fd(), PART_MARKER.as_ptr().cast());
    }
}

fn marked(path: &Path) -> bool {
    let Ok(path) = c_path(path) else {
        return false;
    };
    // SAFETY: both strings are NUL-terminated; a null buffer of length 0 only asks the size.
    let size = unsafe {
        libc::lgetxattr(
            path.as_ptr(),
            PART_MARKER.as_ptr().cast(),
            std::ptr::null_mut(),
            0,
        )
    };
    size >= 0
}

/// Whether the file at `path` is the open `file`.
fn names(file: &File, path: &Path) -> bool {
    match (file.metadata(), fs::symlink_metadata(path)) {
        (Ok(open), Ok(named)) => open.dev() == named.dev() && open.ino() == named.ino(),
        _ => false,
    }
}

/// Moves `from` to `to` unless something has that name: AlreadyExists then.
fn rename_new(from: &Path, to: &Path) -> io::Result<()> {
    let (from_c, to_c) = (c_path(from)?, c_path(to)?);
    // SAFETY: both paths are NUL-terminated and outlive the call; 1 is RENAME_NOREPLACE.
    let status = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            from_c.as_ptr(),
            libc::AT_FDCWD,
            to_c.as_ptr(),
            1u32,
        )
    };
    if status == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if !matches!(error.raw_os_error(), Some(libc::EINVAL | libc::ENOSYS)) {
        return Err(error);
    }
    // A file system without it: a hard link fails the same way when the name is taken.
    fs::hard_link(from, to)?;
    fs::remove_file(from)
}

/// A file in the transfer folder.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct FileEntry {
    pub name: String,
    pub size: u64,
    pub modified_unix_ms: u64,
}

/// The transfer folder.
#[derive(Clone, Debug)]
pub struct UserFolder {
    path: PathBuf,
    /// A shared folder's directory, held open so it can't be swapped while an upload uses it.
    pin: Option<Arc<File>>,
}
impl UserFolder {
    /// The transfer folder, created if missing: `custom` when the host's settings name one,
    /// otherwise the user's `Downloads/Rubylight`.
    pub fn open(custom: Option<&Path>) -> Result<Self, UserFolderError> {
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
        let path = match custom {
            Some(path) => path.to_path_buf(),
            None => downloads()?.join(DEFAULT_SUBFOLDER),
        };
        Self::at(path)
    }

    /// The folder at `path`, for tests of the callers (the `test-folder` feature).
    #[cfg(feature = "test-folder")]
    pub fn unimpersonated(path: &Path) -> Result<Self, UserFolderError> {
        Self::at(path.to_path_buf())
    }

    fn at(path: PathBuf) -> Result<Self, UserFolderError> {
        let folder = Self { path, pin: None };
        fs::create_dir_all(&folder.path)?;
        folder.sweep()?;
        Ok(folder)
    }

    /// A directory of a shared folder that `pin` holds open, as the folder uploads go to.
    /// Nothing is created and nothing swept: the folder is the user's, so a `.part` file
    /// there can be anyone's.
    pub(crate) fn pinned(path: PathBuf, pin: File) -> Self {
        Self {
            path,
            pin: Some(Arc::new(pin)),
        }
    }

    /// Where the folder's files are reached: through the pinned directory when there is one.
    fn base(&self) -> PathBuf {
        self.pin
            .as_deref()
            .map_or_else(|| self.path.clone(), fd_path)
    }

    /// Deletes the `.part` files of uploads a stopped host left behind. One still being
    /// written is locked, so it stays.
    fn sweep(&self) -> io::Result<()> {
        for entry in fs::read_dir(&self.path)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            let name = entry.file_name();
            let path = entry.path();
            if !plain(&metadata)
                || !name.to_str().is_some_and(file_transfer::has_part_suffix)
                || !marked(&path)
            {
                continue;
            }
            let free = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(&path)
                .and_then(|file| lock(&file));
            if free.is_err() {
                continue;
            }
            match fs::remove_file(&path) {
                Ok(()) => tracing::info!(
                    "File transfer deleted the incomplete upload {}",
                    name.display()
                ),
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

    /// Starts an upload of `name`: a new `.part` file under the first numbered name whose
    /// `.part` is free, so neither an upload in flight nor a leftover `.part` is overwritten.
    pub fn create_part(&self, name: &str) -> Result<PartFile, UserFolderError> {
        file_transfer::check_name(name)?;
        let base = self.base();
        for n in 0..=MAX_COLLISIONS {
            let part = file_transfer::part_name(&file_transfer::collision_name(name, n));
            // O_EXCL never follows a link at that name: it fails.
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(base.join(&part))
            {
                Ok(file) => {
                    // A lock held for as long as the upload lives, which a sweep respects.
                    lock(&file)?;
                    mark(&file);
                    return Ok(PartFile {
                        file: Some(file),
                        part: Some(self.path.join(&part)),
                        held: Some(base.join(&part)),
                        name: name.to_owned(),
                        folder: self.clone(),
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("every numbered name of {name} is taken"),
        )
        .into())
    }

    /// Opens a plain file in the folder for reading, with its length.
    pub fn open_file(&self, name: &str) -> Result<(File, u64), UserFolderError> {
        file_transfer::check_stored_name(name)?;
        open_plain(&self.base().join(name))
    }

    /// The plain files in the folder, by name. Subfolders, links, uploads in progress
    /// (`.part`) and names a client couldn't ask for are left out.
    pub fn list(&self) -> Result<Vec<FileEntry>, UserFolderError> {
        let mut files = Vec::new();
        for entry in fs::read_dir(self.base())? {
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
            files.push(FileEntry {
                name,
                size: metadata.len(),
                modified_unix_ms: modified_ms(&metadata)?,
            });
        }
        files.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(files)
    }

    /// The bytes the user may still write to the folder's volume.
    pub fn free_bytes(&self) -> Result<u64, UserFolderError> {
        Ok(self.volume_bytes()?.0)
    }

    /// The bytes the user may still write to the folder's volume and the volume's size.
    #[allow(
        clippy::useless_conversion,
        reason = "narrower C types on 32-bit Linux"
    )]
    pub fn volume_bytes(&self) -> Result<(u64, u64), UserFolderError> {
        let path = c_path(&self.base())?;
        // SAFETY: an all-zero `statvfs` is a valid value the call overwrites.
        let mut found: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: `path` is NUL-terminated and `found` is a live local.
        if unsafe { libc::statvfs(path.as_ptr(), &mut found) } != 0 {
            return Err(io::Error::last_os_error().into());
        }
        let unit = u64::from(found.f_frsize);
        Ok((
            u64::from(found.f_bavail).saturating_mul(unit),
            u64::from(found.f_blocks).saturating_mul(unit),
        ))
    }
}

/// Opens the plain file at `path` for reading, with its length; a link or folder there is
/// [`UserFolderError::NotPlainFile`].
pub(crate) fn open_plain(path: &Path) -> Result<(File, u64), UserFolderError> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.raw_os_error() == Some(libc::ELOOP) => {
            return Err(UserFolderError::NotPlainFile);
        }
        Err(error) => return Err(error.into()),
    };
    let metadata = file.metadata()?;
    if !plain(&metadata) {
        return Err(UserFolderError::NotPlainFile);
    }
    Ok((file, metadata.len()))
}

/// An upload in progress, written to its `.part` file, then renamed. Dropping it before
/// [`PartFile::finish`] deletes the `.part` file; [`PartFile::discard`] does the same and
/// reports a failure. A `.part` file a failed deletion leaves behind is swept by the next
/// [`UserFolder::open`].
///
/// Dropping it does blocking I/O: drop it on a blocking thread.
#[derive(Debug)]
pub struct PartFile {
    file: Option<File>,
    /// Where the `.part` file is, as a person sees it.
    part: Option<PathBuf>,
    /// Where it is reached: through the pinned directory when the folder has one.
    held: Option<PathBuf>,
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

    /// Closes the upload and gives it the first free numbered name, which is returned. If
    /// the rename fails the `.part` file is deleted.
    pub fn finish(mut self) -> Result<String, UserFolderError> {
        let (Some(file), Some(held)) = (self.file.as_mut(), self.held.as_deref()) else {
            return Err(io::Error::other("the upload is already closed").into());
        };
        file.flush()?;
        if !names(file, held) {
            return Err(io::Error::other("the upload's file was replaced").into());
        }
        unmark(file);
        let base = self.folder.base();
        for n in 0..=MAX_COLLISIONS {
            let candidate = file_transfer::collision_name(&self.name, n);
            match rename_new(held, &base.join(&candidate)) {
                Ok(()) => {
                    self.part = None;
                    self.held = None;
                    drop(self.file.take());
                    return Ok(candidate);
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("every numbered name of {} is taken", self.name),
        )
        .into())
    }

    /// Ends an incomplete upload by deleting its `.part` file.
    pub fn discard(mut self) -> Result<(), UserFolderError> {
        self.remove()
    }

    fn remove(&mut self) -> Result<(), UserFolderError> {
        self.part = None;
        let (Some(file), Some(held)) = (self.file.take(), self.held.take()) else {
            return Ok(());
        };
        // Only this upload's file goes, not a link or another file put at its name since.
        if names(&file, &held) {
            fs::remove_file(&held)?;
        }
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
mod tests {
    use super::*;

    pub(crate) fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rubylight-uf-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_downloads_folder_is_read_from_user_dirs() {
        let home = Path::new("/home/me");
        let text = "# comment\nXDG_DESKTOP_DIR=\"$HOME/Desktop\"\nXDG_DOWNLOAD_DIR=\"$HOME/Telechargements\"\n";
        assert_eq!(
            download_dir_in(text, home),
            Some(PathBuf::from("/home/me/Telechargements"))
        );
        assert_eq!(
            download_dir_in("XDG_DOWNLOAD_DIR=\"/data/dl\"\n", home),
            Some(PathBuf::from("/data/dl"))
        );
        assert_eq!(download_dir_in("XDG_DOWNLOAD_DIR=\"dl\"\n", home), None);
        assert_eq!(download_dir_in("", home), None);
    }

    #[test]
    fn an_upload_is_named_after_its_part_and_never_overwrites() {
        let dir = scratch("upload");
        let folder = UserFolder::open(Some(&dir)).unwrap();
        fs::write(dir.join("a.txt"), b"old").unwrap();
        let mut part = folder.create_part("a.txt").unwrap();
        assert_eq!(part.part_path(), Some(dir.join("a.txt.part").as_path()));
        part.write_all(b"new").unwrap();
        // A second upload of the same name takes the next part name.
        let second = folder.create_part("a.txt").unwrap();
        assert_eq!(
            second.part_path(),
            Some(dir.join("a (1).txt.part").as_path())
        );
        assert_eq!(part.finish().unwrap(), "a (1).txt");
        assert_eq!(fs::read(dir.join("a.txt")).unwrap(), b"old");
        assert_eq!(fs::read(dir.join("a (1).txt")).unwrap(), b"new");
        second.discard().unwrap();
        let names: Vec<_> = folder.list().unwrap().into_iter().map(|f| f.name).collect();
        assert_eq!(names, ["a (1).txt", "a.txt"]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_dropped_upload_leaves_nothing_and_links_are_not_files() {
        let dir = scratch("drop");
        let folder = UserFolder::open(Some(&dir)).unwrap();
        drop(folder.create_part("x.bin").unwrap());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
        fs::write(dir.join("real"), b"1234").unwrap();
        std::os::unix::fs::symlink(dir.join("real"), dir.join("link")).unwrap();
        let (_, length) = folder.open_file("real").unwrap();
        assert_eq!(length, 4);
        assert!(matches!(
            folder.open_file("link"),
            Err(UserFolderError::NotPlainFile)
        ));
        assert_eq!(folder.list().unwrap().len(), 1);
        let (free, total) = folder.volume_bytes().unwrap();
        assert!(total > 0 && free <= total);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_relative_folder_is_refused() {
        assert!(matches!(
            UserFolder::open(Some(Path::new("relative"))),
            Err(UserFolderError::Io(_))
        ));
    }
}
