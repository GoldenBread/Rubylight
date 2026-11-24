//! What is left of the removed background child session, on Linux: nothing. Linux has no Windows registry, child-session switch or firewall to restore and no profiles a saved child-session password can be in, so every query finds nothing and every change succeeds without doing anything. Only wiping a file and removing an empty folder are real.

pub mod lock;

use anyhow::Result;
use butterpollo_session::child_session::{Hive, Registry, SystemSettings};
use std::{
    fs::File,
    io::{self, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

/// Whether child sessions are on. They do not exist on Linux.
pub fn child_sessions_enabled() -> Result<bool> {
    Ok(false)
}

/// The child session of this process's session, None when there is none: always.
pub fn child_session_id() -> Option<u32> {
    None
}

/// The settings restore puts back. There are none on Linux: reads find no value and changes
/// are accepted without effect.
pub struct NativeSettings;
impl NativeSettings {
    pub fn new() -> Result<Self> {
        Ok(Self)
    }
}

/// The registry alone, which Linux does not have.
pub struct NativeRegistry;
impl Registry for NativeRegistry {
    fn read_dword(&self, hive: &Hive, path: &str, name: &str) -> Result<Option<u32>> {
        let _ = (hive, path, name);
        Ok(None)
    }
    fn write_dword(&mut self, hive: &Hive, path: &str, name: &str, value: u32) -> Result<()> {
        let _ = (hive, path, name, value);
        Ok(())
    }
}
impl Registry for NativeSettings {
    fn read_dword(&self, hive: &Hive, path: &str, name: &str) -> Result<Option<u32>> {
        let _ = (hive, path, name);
        Ok(None)
    }
    fn write_dword(&mut self, hive: &Hive, path: &str, name: &str, value: u32) -> Result<()> {
        let _ = (hive, path, name, value);
        Ok(())
    }
}
impl SystemSettings for NativeSettings {
    fn delete_value(&mut self, hive: &Hive, path: &str, name: &str) -> Result<()> {
        let _ = (hive, path, name);
        Ok(())
    }
    fn child_sessions_enabled(&self) -> Result<bool> {
        Ok(false)
    }
    fn set_child_sessions(&mut self, enabled: bool) -> Result<()> {
        let _ = enabled;
        Ok(())
    }
    fn block_rule_present(&self, name: &str) -> Result<bool> {
        let _ = name;
        Ok(false)
    }
    fn enable_rule(&mut self, group: &str, id: &str) -> Result<()> {
        let _ = (group, id);
        Ok(())
    }
    fn remove_rule(&mut self, name: &str) -> Result<()> {
        let _ = name;
        Ok(())
    }
    fn load_user_hive(&mut self, sid: &str) -> Result<bool> {
        let _ = sid;
        Ok(false)
    }
    fn unload_user_hive(&mut self, sid: &str) -> Result<()> {
        let _ = sid;
        Ok(())
    }
}

/// The profile folder of every account a child-session password can be saved in: none.
pub fn profile_folders() -> Result<Vec<PathBuf>> {
    Ok(Vec::new())
}

/// The `AppData\Local` of the account signed in at this PC's screen: none on Linux.
pub fn console_local_app_data() -> Result<Option<PathBuf>> {
    Ok(None)
}

/// Overwrite the file at `path` with zeros, flush it to disk and delete it, all through one
/// handle opened without following a link; false when there is none. A link or a folder is
/// refused with nothing written.
pub fn wipe_file(path: &Path) -> io::Result<bool> {
    let mut file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) if error.raw_os_error() == Some(libc::ELOOP) => {
            return Err(io::Error::other("it is a link; nothing was changed"));
        }
        Err(error) => return Err(error),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::other("it is not a file; nothing was changed"));
    }
    overwrite_with_zeros(&mut file, metadata.len())?;
    std::fs::remove_file(path)?;
    Ok(true)
}

fn overwrite_with_zeros(file: &mut File, length: u64) -> io::Result<()> {
    let zeros = [0u8; 64 * 1024];
    let mut left = length;
    while left > 0 {
        let chunk = left.min(zeros.len() as u64) as usize;
        file.write_all(&zeros[..chunk])?;
        left -= chunk as u64;
    }
    file.sync_all()
}

/// Remove the folder at `path` when it is empty; a folder that holds anything, or none, is
/// left, and a link is refused.
pub fn remove_empty_folder(path: &Path) -> io::Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if !metadata.is_dir() {
        return Err(io::Error::other("it is not a folder; nothing was changed"));
    }
    match std::fs::remove_dir(path) {
        Err(error) if error.kind() == io::ErrorKind::DirectoryNotEmpty => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        removed => removed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rubylight-cs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn wipe_removes_a_file_and_leaves_links_and_folders() {
        let dir = scratch("wipe");
        let file = dir.join("secret");
        std::fs::write(&file, b"password").unwrap();
        assert!(wipe_file(&file).unwrap());
        assert!(!file.exists());
        assert!(!wipe_file(&file).unwrap());
        let target = dir.join("target");
        std::fs::write(&target, b"keep").unwrap();
        let link = dir.join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(wipe_file(&link).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"keep");
        assert!(wipe_file(&dir).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn empty_folders_go_and_full_ones_stay() {
        let dir = scratch("empty");
        let inner = dir.join("inner");
        std::fs::create_dir(&inner).unwrap();
        std::fs::write(inner.join("file"), b"x").unwrap();
        remove_empty_folder(&inner).unwrap();
        assert!(inner.exists());
        std::fs::remove_file(inner.join("file")).unwrap();
        remove_empty_folder(&inner).unwrap();
        assert!(!inner.exists());
        remove_empty_folder(&inner).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
}
