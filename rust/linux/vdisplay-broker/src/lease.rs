//! Connector leases: `leases/<slot>.owner` holds the uid that plugged a monitor in.
//!
//! The rules follow libvirtualdisplay's `vibeshine-vkms` broker (Nonary/libvirtualdisplay, MIT,
//! Copyright (c) 2026 Chase Payne): the directory belongs to the broker's user with mode 0700,
//! a lease is a regular file of that user (never a symlink), and a per-slot lock serializes
//! requests.
use anyhow::{Result, bail};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The lease directory of a run.
pub struct Leases {
    dir: PathBuf,
}

/// The effective uid: the owner every lease file and the directory must have.
fn me() -> u32 {
    // SAFETY: geteuid takes no arguments and cannot fail.
    unsafe { libc::geteuid() }
}

/// A slot name is a key in file names: letters, digits and dashes only.
fn check_slot(slot: &str) -> Result<()> {
    if slot.is_empty() || !slot.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        bail!("unsafe connector slot name {slot:?}");
    }
    Ok(())
}

impl Leases {
    /// Open `dir`, making it (0700) if missing; refuse a symlink, a non-directory or one that
    /// another user owns.
    pub fn open(dir: &Path) -> Result<Self> {
        match fs::symlink_metadata(dir) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                fs::DirBuilder::new().mode(0o700).create(dir)?;
            }
            Err(e) => return Err(e.into()),
        }
        let meta = fs::symlink_metadata(dir)?;
        if !meta.is_dir() {
            bail!("the lease path {} is not a plain directory", dir.display());
        }
        if meta.uid() != me() {
            bail!("the lease directory {} has another owner", dir.display());
        }
        if meta.mode() & 0o077 != 0 {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self { dir: dir.into() })
    }

    fn lease_path(&self, slot: &str) -> PathBuf {
        self.dir.join(format!("{slot}.owner"))
    }

    /// Lock `slot` for the life of the returned file; waits up to one second.
    pub fn lock(&self, slot: &str) -> Result<File> {
        check_slot(slot)?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(self.dir.join(format!("{slot}.lock")))?;
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            // SAFETY: the descriptor belongs to `file`, which outlives the call.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                return Ok(file);
            }
            if Instant::now() >= deadline {
                bail!("could not lock the connector lease for {slot}");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// The owner of `slot`, or None without a lease. An unsafe or unreadable lease is an error.
    pub fn read(&self, slot: &str) -> Result<Option<u32>> {
        check_slot(slot)?;
        let path = self.lease_path(slot);
        let meta = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        if !meta.file_type().is_file() || meta.uid() != me() {
            bail!(
                "the lease {} is not a plain file of the broker",
                path.display()
            );
        }
        let text = fs::read_to_string(&path)?;
        match text.trim().parse::<u32>() {
            Ok(uid) if text.trim().bytes().all(|b| b.is_ascii_digit()) => Ok(Some(uid)),
            _ => bail!("the lease {} holds no uid", path.display()),
        }
    }

    /// Record `uid` as the owner of `slot`, replacing any lease.
    pub fn write(&self, slot: &str, uid: u32) -> Result<()> {
        check_slot(slot)?;
        let path = self.lease_path(slot);
        if let Ok(meta) = fs::symlink_metadata(&path)
            && !meta.file_type().is_file()
        {
            bail!("refusing the unsafe lease path {}", path.display());
        }
        let temp = self
            .dir
            .join(format!(".{slot}.owner.{}", std::process::id()));
        let _ = fs::remove_file(&temp);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)?;
        writeln!(file, "{uid}")?;
        drop(file);
        if let Err(e) = fs::rename(&temp, &path) {
            let _ = fs::remove_file(&temp);
            return Err(e.into());
        }
        Ok(())
    }

    /// Drop the lease of `slot` (none is fine); refuse one that is not a plain file.
    pub fn release(&self, slot: &str) -> Result<()> {
        check_slot(slot)?;
        let path = self.lease_path(slot);
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_file() => Ok(fs::remove_file(&path)?),
            Ok(_) => bail!(
                "refusing to remove the unsafe lease path {}",
                path.display()
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}
