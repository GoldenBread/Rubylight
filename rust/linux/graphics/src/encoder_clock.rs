//! The video encoder's clock on AMD GPUs while a stream runs.
//!
//! The GPU's firmware runs its clocks only as fast as their load needs: a 60 fps stream fits at
//! low clocks, where a 1080p HEVC frame takes about four times as long as at the top ones
//! (docs/linux-host-spike.md, "The encoder's clock follows its load"). While a stream runs,
//! [`raise`] sets the GPU's performance level to `high` through the amdgpu sysfs file, and the
//! returned [`Raised`] puts `auto` back when the last stream ends. Forcing only the video
//! clocks (`manual` with `pp_dpm_vclk`) did not hold them up on an RX 7800 XT and gained 5%.
//!
//! The `vaapi_latency_over_power` setting (on by default) turns it off. The file is root's. `rust/linux/packaging/udev/70-rubylight-encoder-clock.rules` lets the
//! `video` group write it; without it [`raise`] fails with the rule's name and the stream runs
//! at the firmware's clocks, as before.
//!
//! Every change is journaled before it is made, so a host that crashed mid-stream restores
//! the clock at its next start ([`recover`]). A GPU whose level someone else set (anything but
//! `auto`) is left alone.

use anyhow::{Context, Result};
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

/// The level the firmware chooses by itself; the only one [`raise`] changes and restores.
const AUTO: &str = "auto";
/// The level a stream holds: every clock at its top.
const HIGH: &str = "high";
const LEVEL: &str = "power_dpm_force_performance_level";
/// The udev rule that grants the `video` group these files.
pub const RULE: &str = "rust/linux/packaging/udev/70-rubylight-encoder-clock.rules";

/// The streams holding each device's clock up, by sysfs device folder.
static HOLDERS: Mutex<Vec<(PathBuf, usize)>> = Mutex::new(Vec::new());

/// The video clocks of one GPU, held at their top level until dropped.
#[derive(Debug)]
pub struct Raised {
    device: PathBuf,
    journal: PathBuf,
}

/// Where the journal lives: `$XDG_STATE_HOME/rubylight`, else `~/.local/state/rubylight`.
fn journal_folder() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".local/state")))
        .unwrap_or_else(std::env::temp_dir)
        .join("rubylight")
}

/// The journal file of `device`: holds the device folder while its clocks are forced.
fn journal_of(folder: &Path, device: &Path) -> PathBuf {
    let tag: String = device
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    folder.join(format!("encoder-clock-{tag}"))
}

fn read(path: &Path) -> Result<String> {
    std::fs::read_to_string(path)
        .map(|text| text.trim().to_owned())
        .with_context(|| format!("reading {}", path.display()))
}

fn write(path: &Path, value: &str) -> Result<()> {
    std::fs::write(path, value).map_err(|error| {
        let hint = if error.kind() == std::io::ErrorKind::PermissionDenied {
            format!("; install {RULE} to let the host raise it")
        } else {
            String::new()
        };
        anyhow::anyhow!("writing {value} to {}: {error}{hint}", path.display())
    })
}

/// Holds the clocks of the GPU at `device` (its sysfs device folder) at their top level while
/// the returned value lives; `None` when the GPU has no such control, or when someone
/// else set its level (anything but `auto`), which is then theirs. Several streams share one
/// raise.
pub fn raise(device: &Path) -> Result<Option<Raised>> {
    raise_in(device, &journal_folder())
}

fn raise_in(device: &Path, journal_folder: &Path) -> Result<Option<Raised>> {
    if !device.join(LEVEL).exists() {
        return Ok(None);
    }
    let journal = journal_of(journal_folder, device);
    let mut holders = HOLDERS.lock().unwrap();
    if let Some((_, count)) = holders.iter_mut().find(|(held, _)| held == device) {
        *count += 1;
        return Ok(Some(Raised {
            device: device.to_owned(),
            journal,
        }));
    }
    let level = read(&device.join(LEVEL))?;
    if level != AUTO {
        tracing::info!(%level, "the GPU's performance level was set by someone else; it is left as it is");
        return Ok(None);
    }
    std::fs::create_dir_all(journal_folder)
        .with_context(|| format!("creating {}", journal_folder.display()))?;
    std::fs::write(&journal, device.to_string_lossy().as_bytes())
        .with_context(|| format!("writing {}", journal.display()))?;
    if let Err(error) = write(&device.join(LEVEL), HIGH) {
        restore(device, &journal);
        return Err(error);
    }
    holders.push((device.to_owned(), 1));
    tracing::info!(device = %device.display(), "GPU clocks raised for the stream");
    Ok(Some(Raised {
        device: device.to_owned(),
        journal,
    }))
}

/// Puts the automatic level back and removes the journal. A level already automatic needs no
/// write: a raise refused before it changed anything leaves no journal behind.
fn restore(device: &Path, journal: &Path) {
    if read(&device.join(LEVEL)).is_ok_and(|level| level == AUTO) {
        let _ = std::fs::remove_file(journal);
        return;
    }
    match write(&device.join(LEVEL), AUTO) {
        Ok(()) => {
            let _ = std::fs::remove_file(journal);
        }
        Err(error) => tracing::warn!(%error, "the video encoder clock could not be put back"),
    }
}

impl Drop for Raised {
    fn drop(&mut self) {
        let mut holders = HOLDERS.lock().unwrap();
        let Some(index) = holders.iter().position(|(held, _)| *held == self.device) else {
            return;
        };
        holders[index].1 -= 1;
        if holders[index].1 == 0 {
            holders.remove(index);
            restore(&self.device, &self.journal);
            tracing::info!(device = %self.device.display(), "GPU clocks back to automatic");
        }
    }
}

/// Restores the clocks a host left raised when it ended without dropping its [`Raised`].
/// Called once at the host's start.
pub fn recover() {
    recover_in(&journal_folder());
}

fn recover_in(folder: &Path) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let journal = entry.path();
        let is_journal = journal
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("encoder-clock-"));
        if !is_journal {
            continue;
        }
        match std::fs::read_to_string(&journal) {
            Ok(device) => {
                tracing::warn!(%device, "restoring a video encoder clock an earlier run left raised");
                restore(Path::new(device.trim()), &journal);
            }
            Err(error) => {
                tracing::warn!(%error, journal = %journal.display(), "unreadable clock journal")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake amdgpu device folder with its level file.
    fn device(root: &Path, level: &str) -> PathBuf {
        let device = root.join("device");
        std::fs::create_dir_all(&device).unwrap();
        std::fs::write(device.join(LEVEL), format!("{level}\n")).unwrap();
        device
    }

    fn root(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("encoder-clock-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        root
    }

    #[test]
    fn streams_share_a_raise_and_the_last_one_restores_it() {
        let root = root("share");
        let device = device(&root, AUTO);
        let journal = root.join("state");
        let first = raise_in(&device, &journal).unwrap().unwrap();
        assert_eq!(read(&device.join(LEVEL)).unwrap(), HIGH);
        assert!(
            journal_of(&journal, &device).exists(),
            "journaled while raised"
        );
        let second = raise_in(&device, &journal).unwrap().unwrap();
        drop(first);
        assert_eq!(
            read(&device.join(LEVEL)).unwrap(),
            HIGH,
            "one stream still runs"
        );
        drop(second);
        assert_eq!(read(&device.join(LEVEL)).unwrap(), AUTO);
        assert!(!journal_of(&journal, &device).exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_level_someone_else_set_is_left_alone() {
        let root = root("other");
        let device = device(&root, "profile_peak");
        assert!(raise_in(&device, &root.join("state")).unwrap().is_none());
        assert!(
            raise_in(&root.join("no-gpu"), &root.join("state"))
                .unwrap()
                .is_none()
        );
        assert_eq!(read(&device.join(LEVEL)).unwrap(), "profile_peak");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_raise_a_crash_left_is_put_back_at_the_next_start() {
        let root = root("crash");
        let device = device(&root, AUTO);
        let journal = root.join("state");
        std::mem::forget(raise_in(&device, &journal).unwrap());
        HOLDERS.lock().unwrap().retain(|(held, _)| *held != device);
        assert_eq!(read(&device.join(LEVEL)).unwrap(), HIGH);
        recover_in(&journal);
        assert_eq!(read(&device.join(LEVEL)).unwrap(), AUTO);
        assert!(!journal_of(&journal, &device).exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_raise_the_host_may_not_make_leaves_nothing_behind() {
        use std::os::unix::fs::PermissionsExt;
        let root = root("denied");
        let device = device(&root, AUTO);
        let journal = root.join("state");
        let level = device.join(LEVEL);
        std::fs::set_permissions(&level, std::fs::Permissions::from_mode(0o444)).unwrap();
        let error = raise_in(&device, &journal).unwrap_err();
        assert!(format!("{error:#}").contains(RULE), "{error:#}");
        assert!(
            !journal_of(&journal, &device).exists(),
            "nothing changed, nothing to restore"
        );
        assert_eq!(read(&level).unwrap(), AUTO);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_denied_write_names_the_rule() {
        let error = write(Path::new("/proc/version"), "x").unwrap_err();
        assert!(format!("{error:#}").contains("/proc/version"), "{error:#}");
    }
}
