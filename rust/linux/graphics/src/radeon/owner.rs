//! Who owns the driver settings journal. On Linux an exclusive lock on a file next to the journal serializes every load-modify-save of it, and the owner's process id and start say whether the host that wrote it is still alive. A second host must not put back a live host's settings.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions, TryLockError},
    path::Path,
    time::{Duration, Instant},
};

/// The process that last wrote the journal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Owner {
    pub pid: u32,
    /// When the process started (see [`identity`]), so a reused pid is not mistaken for the
    /// owner.
    pub started: u64,
}
impl Owner {
    /// This process.
    pub fn current() -> Result<Self> {
        let pid = std::process::id();
        let started = identity(pid)?.ok_or_else(|| anyhow::anyhow!("host identity unavailable"))?;
        Ok(Self { pid, started })
    }
    /// Another process that is still running. When the process cannot be read it counts as
    /// alive: a second host must not restore a live host's settings.
    pub fn is_other_and_alive(&self) -> bool {
        self.pid != std::process::id() && self.still_running(identity(self.pid))
    }
    fn still_running(&self, identity: Result<Option<u64>>) -> bool {
        match identity {
            // A process that is there with the recorded start is the owner.
            Ok(started) => started == Some(self.started),
            Err(e) => {
                tracing::warn!(pid = self.pid, error = %format!("{e:#}"), "cannot tell whether the Radeon journal's owner is alive; assuming it is");
                true
            }
        }
    }
}

/// The user this process runs as, the way journal entries name a user: the effective user id
/// as decimal text.
pub(super) fn own_user() -> String {
    // SAFETY: geteuid takes no arguments, cannot fail and touches no memory.
    let uid = unsafe { libc::geteuid() };
    uid.to_string()
}

/// How long [`lock`] waits for another holder before it gives up.
const LOCK_WAIT: Duration = Duration::from_secs(30);

/// The journal's lock, held until dropped. A process that dies releases it.
pub struct JournalLock {
    _file: File,
}

/// Take the journal's lock; wait for another thread or process that holds it. The lock is an
/// exclusive advisory lock on `<journal>.lock` in the journal's directory, so every host that
/// shares the directory shares the lock. An account that cannot open that file gets an error,
/// which is loud, not a silent lock-less run.
pub fn lock(path: &Path) -> Result<JournalLock> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let mut name = path
        .file_name()
        .context("journal path missing file name")?
        .to_os_string();
    name.push(".lock");
    let lock_path = parent.join(name);
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("opening {}", lock_path.display()))?;
    let deadline = Instant::now() + LOCK_WAIT;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(JournalLock { _file: file }),
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(TryLockError::WouldBlock) => bail!("the Radeon journal is busy"),
            Err(TryLockError::Error(e)) => {
                return Err(e).with_context(|| format!("locking {}", lock_path.display()));
            }
        }
    }
}

/// The start of a running process, `None` when it has exited or never existed. The value is
/// opaque: the process's start tick since boot mixed with the id of that boot, so the same pid
/// started at the same tick of another boot is a different process.
pub fn identity(pid: u32) -> Result<Option<u64>> {
    if pid == 0 {
        return Ok(None);
    }
    let path = format!("/proc/{pid}/stat");
    let stat = match std::fs::read_to_string(&path) {
        Ok(stat) => stat,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        // The process went away between the open and the read.
        Err(e) if e.raw_os_error() == Some(libc::ESRCH) => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading {path}")),
    };
    let (state, ticks) =
        parse_stat(&stat).with_context(|| format!("{path} has an unknown format"))?;
    // A zombie has exited; only its parent has not collected it yet.
    if matches!(state, 'Z' | 'X' | 'x') {
        return Ok(None);
    }
    Ok(Some(stamp(&boot_id(), ticks)))
}

/// The state letter and the start tick from a `/proc/<pid>/stat` line. The command name sits in
/// parentheses and may itself hold spaces and parentheses, so the fields are counted from the
/// last `)`: the state is the first after it and the start time the twentieth.
fn parse_stat(stat: &str) -> Option<(char, u64)> {
    let rest = &stat[stat.rfind(')')? + 1..];
    let mut fields = rest.split_ascii_whitespace();
    let state = fields.next()?.chars().next()?;
    let ticks = fields.nth(18)?.parse().ok()?;
    Some((state, ticks))
}

/// The kernel's id of this boot, empty when it cannot be read.
fn boot_id() -> String {
    std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map(|id| id.trim().to_owned())
        .unwrap_or_default()
}

fn stamp(boot: &str, ticks: u64) -> u64 {
    let hash = butterpollo_crypto::crypto::hash(format!("{boot}:{ticks}").as_bytes());
    let mut first = [0u8; 8];
    first.copy_from_slice(&hash[..8]);
    u64::from_le_bytes(first)
}
