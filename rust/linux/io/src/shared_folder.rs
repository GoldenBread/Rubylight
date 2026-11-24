//! A folder the PC shares with devices: listed, read and written as the signed-in user, which this host is, and never left. A device's path is a list of names opened below the root one folder at a time without following links, so a link on the way is refused rather than followed out of the folder. Links are never listed.
//!
//! Every call blocks: run them on a blocking thread, not an async worker.

use crate::user_folder::{
    PartFile, UserFolder, UserFolderError, fd_path, is_link_below, modified_ms, open_below,
    open_plain, plain,
};
use butterpollo_session::file_transfer;
use std::{
    fs::{self, File, OpenOptions},
    io,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

/// A file or folder in a shared folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderEntry {
    pub name: String,
    pub dir: bool,
    /// Bytes; 0 for a folder.
    pub size: u64,
    pub modified_unix_ms: u64,
}

/// A shared folder's root.
#[derive(Clone, Debug)]
pub struct SharedFolder {
    /// The root with every link on its way resolved.
    root: PathBuf,
}
impl SharedFolder {
    /// The shared folder at `root`, which must be a folder already: it is never created.
    pub fn open(root: &Path) -> Result<Self, UserFolderError> {
        Self::at(root)
    }

    /// The folder at `root`, for tests of the callers (the `test-folder` feature).
    #[cfg(feature = "test-folder")]
    pub fn unimpersonated(root: &Path) -> Result<Self, UserFolderError> {
        Self::at(root)
    }

    fn at(root: &Path) -> Result<Self, UserFolderError> {
        if !root.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("the shared folder {} is not absolute", root.display()),
            )
            .into());
        }
        // The root itself may be a link the administrator chose: the folder it leads to is
        // the root.
        let resolved = fs::canonicalize(root)?;
        if !fs::metadata(&resolved)?.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                format!("the shared folder {} is not a folder", root.display()),
            )
            .into());
        }
        local_root(root, &resolved)?;
        Ok(Self { root: resolved })
    }

    /// The root with its links resolved.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The root, open.
    fn open_root(&self) -> io::Result<File> {
        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC)
            .open(&self.root)
    }

    /// The folder `names` below the root, opened one name at a time without following a
    /// link, and kept open so it can't be swapped while the handle lives.
    fn dir(&self, names: &[&str]) -> Result<(File, PathBuf), UserFolderError> {
        let mut path = self.root.clone();
        let mut dir = self.open_root()?;
        for name in names {
            file_transfer::check_folder_name(name)?;
            path.push(name);
            dir = step(&dir, name, &path)?;
        }
        Ok((dir, path))
    }

    /// The folders and plain files in the folder `names` below the root, folders first, then
    /// by name. Links, uploads in progress (`.part`) and names a device couldn't ask for are
    /// left out.
    pub fn entries(&self, names: &[&str]) -> Result<Vec<FolderEntry>, UserFolderError> {
        let (dir, _) = self.dir(names)?;
        let mut entries = Vec::new();
        for entry in fs::read_dir(fd_path(&dir))? {
            let entry = entry?;
            // From the directory listing itself: links are not followed.
            let metadata = entry.metadata()?;
            let is_dir = metadata.is_dir();
            if !is_dir && !plain(&metadata) {
                continue;
            }
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if file_transfer::check_folder_name(&name).is_err() {
                continue;
            }
            entries.push(FolderEntry {
                name,
                dir: is_dir,
                size: if is_dir { 0 } else { metadata.len() },
                modified_unix_ms: modified_ms(&metadata)?,
            });
        }
        entries.sort_by(|a, b| {
            b.dir
                .cmp(&a.dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(entries)
    }

    /// Opens the plain file `names` below the root for reading, with its length.
    pub fn open_file(&self, names: &[&str]) -> Result<(File, u64), UserFolderError> {
        let Some((last, folders)) = names.split_last() else {
            return Err(UserFolderError::NotPlainFile);
        };
        file_transfer::check_folder_name(last)?;
        let (dir, _) = self.dir(folders)?;
        open_plain(&fd_path(&dir).join(last))
    }

    /// The folder `names` below the root as the folder an upload goes to. It stays held open
    /// while the returned folder, or an upload it started, lives.
    pub fn upload_folder(&self, names: &[&str]) -> Result<UserFolder, UserFolderError> {
        let (dir, path) = self.dir(names)?;
        Ok(UserFolder::pinned(path, dir))
    }

    /// Starts an upload of `name` into `folder`, from [`SharedFolder::upload_folder`], as
    /// [`UserFolder::create_part`] does, and checks that its `.part` file is where it was
    /// asked for.
    pub fn create_part(
        &self,
        folder: &UserFolder,
        name: &str,
    ) -> Result<PartFile, UserFolderError> {
        let part = folder.create_part(name)?;
        if let (Some(file), Some(path)) = (part.file(), part.part_path()) {
            let here = fs::symlink_metadata(path)
                .ok()
                .zip(file.metadata().ok())
                .is_some_and(|(named, open)| {
                    named.dev() == open.dev() && named.ino() == open.ino()
                });
            if !here {
                tracing::warn!(
                    "Shared folders refused {}: the folder was moved or swapped",
                    path.display()
                );
                return Err(UserFolderError::OutsideFolder);
            }
        }
        Ok(part)
    }
}

/// The folder `name` inside `dir`, which is at `path`; a link there leads out of the folder.
fn step(dir: &File, name: &str, path: &Path) -> Result<File, UserFolderError> {
    match open_below(dir, name, libc::O_RDONLY | libc::O_DIRECTORY) {
        Ok(next) => Ok(next),
        Err(_) if is_link_below(dir, name) => {
            tracing::warn!("Shared folders refused {}: it is a link", path.display());
            Err(UserFolderError::OutsideFolder)
        }
        Err(error) if error.raw_os_error() == Some(libc::ENOTDIR) => {
            Err(io::Error::new(io::ErrorKind::NotFound, "not a folder").into())
        }
        Err(error) => Err(error.into()),
    }
}

/// Fails unless `resolved`, where the shared folder `root` leads, is a UTF-8 path on a disk
/// of this PC rather than a share another machine serves.
fn local_root(root: &Path, resolved: &Path) -> io::Result<()> {
    if resolved.to_str().is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "the shared folder {} resolves to {}, which is not valid Unicode",
                root.display(),
                resolved.display()
            ),
        ));
    }
    if !crate::files::local_volume(resolved) {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "the shared folder {} leads to the network share {}",
                root.display(),
                resolved.display()
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rubylight-sf-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn links_never_lead_out_of_the_folder() {
        let dir = scratch("links");
        let (root, outside) = (dir.join("root"), dir.join("outside"));
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("secret"), b"s").unwrap();
        fs::write(root.join("sub/a.txt"), b"abc").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("out")).unwrap();
        std::os::unix::fs::symlink(outside.join("secret"), root.join("sub/leak")).unwrap();
        let shared = SharedFolder::open(&root).unwrap();
        let listed = shared.entries(&[]).unwrap();
        assert_eq!(
            listed.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            ["sub"]
        );
        assert!(matches!(
            shared.entries(&["out"]),
            Err(UserFolderError::OutsideFolder)
        ));
        assert!(matches!(
            shared.open_file(&["out", "secret"]),
            Err(UserFolderError::OutsideFolder)
        ));
        assert!(matches!(
            shared.open_file(&["sub", "leak"]),
            Err(UserFolderError::NotPlainFile)
        ));
        assert!(matches!(
            shared.open_file(&["sub"]),
            Err(UserFolderError::NotPlainFile)
        ));
        let (_, length) = shared.open_file(&["sub", "a.txt"]).unwrap();
        assert_eq!(length, 3);
        assert_eq!(shared.entries(&["sub"]).unwrap().len(), 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_upload_lands_below_the_root() {
        let dir = scratch("upload");
        fs::create_dir_all(dir.join("sub")).unwrap();
        let shared = SharedFolder::open(&dir).unwrap();
        let folder = shared.upload_folder(&["sub"]).unwrap();
        let mut part = shared.create_part(&folder, "n.bin").unwrap();
        part.write_all(b"12").unwrap();
        assert_eq!(part.finish().unwrap(), "n.bin");
        assert_eq!(fs::read(dir.join("sub/n.bin")).unwrap(), b"12");
        assert!(matches!(
            SharedFolder::open(Path::new("relative")),
            Err(UserFolderError::Io(_))
        ));
        fs::remove_dir_all(dir).unwrap();
    }
}
