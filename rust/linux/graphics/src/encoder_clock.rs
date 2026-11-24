//! The video encoder's clock on AMD GPUs while a stream runs.
//!
//! The GPU's firmware runs its video engines (VCN) only as fast as their load needs: a 60 fps
//! stream fits at the engine's low clock, where a 1080p HEVC frame takes about twice as long
//! as at the top one (docs/linux-host-spike.md, "The encoder's clock follows its load"). While
//! a stream runs, [`raise`] pins each video clock domain (`pp_dpm_vclk`, `pp_dpm_vclk1`) to its
//! top level through the amdgpu sysfs files, leaving the shader and memory clocks dynamic, and
//! the returned [`Raised`] puts the automatic mode back when the last stream ends.
//!
//! The files are root's. `rust/linux/packaging/udev/70-rubylight-encoder-clock.rules` lets
//! the `video` group write these three, and only these; without it [`raise`] fails with the
//! rule's name and the stream runs at the firmware's clock, as before.
//!
//! Every change is journaled before it is made, so a host that crashed mid-stream restores
//! the clock at its next start ([`recover`]). A GPU whose level someone else set (anything but
//! `auto`) is left alone.

use anyhow::{Context, Result, bail};
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

/// The level the firmware chooses by itself; the only one [`raise`] changes and restores.
const AUTO: &str = "auto";
/// The level that lets the clock files take a forced level.
const MANUAL: &str = "manual";
/// The video clock domains: one per VCN instance.
const DOMAINS: [&str; 2] = ["pp_dpm_vclk", "pp_dpm_vclk1"];
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

/// The sysfs device folder of a DRM render node or card (`/dev/dri/renderD128`).
pub fn device_of(node: &Path) -> Option<PathBuf> {
    let name = node.file_name()?;
    let device = Path::new("/sys/class/drm").join(name).join("device");
    device.join(LEVEL).exists().then_some(device)
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

/// The highest level index a clock file lists (`0: 24Mhz`, `1: 513Mhz *`, `2: 2934Mhz`).
fn top_level(listing: &str) -> Option<u32> {
    listing
        .lines()
        .filter_map(|line| line.split(':').next()?.trim().parse().ok())
        .max()
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

/// Pins the video clocks of the GPU at `device` (a sysfs device folder, [`device_of`]) to
/// their top level while the returned value lives. Several streams share one raise.
pub fn raise(device: &Path) -> Result<Raised> {
    raise_in(device, &journal_folder())
}

fn raise_in(device: &Path, journal_folder: &Path) -> Result<Raised> {
    let journal = journal_of(journal_folder, device);
    let mut holders = HOLDERS.lock().unwrap();
    if let Some((_, count)) = holders.iter_mut().find(|(held, _)| held == device) {
        *count += 1;
        return Ok(Raised {
            device: device.to_owned(),
            journal,
        });
    }
    let level = read(&device.join(LEVEL))?;
    if level != AUTO {
        bail!(
            "the GPU's performance level is {level}, set by someone else; its video clock is left as it is"
        );
    }
    let domains: Vec<(PathBuf, u32)> = DOMAINS
        .iter()
        .map(|name| device.join(name))
        .filter(|path| path.exists())
        .map(|path| {
            let top = top_level(&read(&path)?).context("the clock file lists no level")?;
            Ok((path, top))
        })
        .collect::<Result<_>>()?;
    if domains.is_empty() {
        bail!("this GPU lists no video clock to raise");
    }
    std::fs::create_dir_all(journal_folder)
        .with_context(|| format!("creating {}", journal_folder.display()))?;
    std::fs::write(&journal, device.to_string_lossy().as_bytes())
        .with_context(|| format!("writing {}", journal.display()))?;
    let forced = write(&device.join(LEVEL), MANUAL).and_then(|()| {
        domains
            .iter()
            .try_for_each(|(path, top)| write(path, &top.to_string()))
    });
    if let Err(error) = forced {
        restore(device, &journal);
        return Err(error);
    }
    holders.push((device.to_owned(), 1));
    tracing::info!(device = %device.display(), "video encoder clock raised for the stream");
    Ok(Raised {
        device: device.to_owned(),
        journal,
    })
}

/// Puts the automatic level back and removes the journal.
fn restore(device: &Path, journal: &Path) {
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
            tracing::info!(device = %self.device.display(), "video encoder clock back to automatic");
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

    /// A fake amdgpu device folder with the three files.
    fn device(root: &Path, level: &str) -> PathBuf {
        let device = root.join("device");
        std::fs::create_dir_all(&device).unwrap();
        std::fs::write(device.join(LEVEL), format!("{level}\n")).unwrap();
        std::fs::write(
            device.join("pp_dpm_vclk"),
            "0: 24Mhz *\n1: 513Mhz\n2: 2934Mhz\n",
        )
        .unwrap();
        std::fs::write(
            device.join("pp_dpm_vclk1"),
            "0: 24Mhz *\n1: 513Mhz\n2: 2200Mhz\n",
        )
        .unwrap();
        device
    }

    fn root(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("encoder-clock-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        root
    }

    #[test]
    fn the_top_level_is_the_highest_index_listed() {
        assert_eq!(top_level("0: 24Mhz *\n1: 513Mhz\n2: 2934Mhz\n"), Some(2));
        assert_eq!(top_level("S: 28Mhz *\n1: 513Mhz\n2: 2934Mhz\n"), Some(2));
        assert_eq!(top_level(""), None);
    }

    #[test]
    fn streams_share_a_raise_and_the_last_one_restores_it() {
        let root = root("share");
        let device = device(&root, AUTO);
        let journal = root.join("state");
        let first = raise_in(&device, &journal).unwrap();
        assert_eq!(read(&device.join(LEVEL)).unwrap(), MANUAL);
        assert_eq!(read(&device.join("pp_dpm_vclk")).unwrap(), "2");
        assert_eq!(read(&device.join("pp_dpm_vclk1")).unwrap(), "2");
        assert!(
            journal_of(&journal, &device).exists(),
            "journaled while raised"
        );
        let second = raise_in(&device, &journal).unwrap();
        drop(first);
        assert_eq!(
            read(&device.join(LEVEL)).unwrap(),
            MANUAL,
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
        let device = device(&root, "high");
        let error = raise_in(&device, &root.join("state")).unwrap_err();
        assert!(
            format!("{error:#}").contains("set by someone else"),
            "{error:#}"
        );
        assert_eq!(read(&device.join(LEVEL)).unwrap(), "high");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_raise_a_crash_left_is_put_back_at_the_next_start() {
        let root = root("crash");
        let device = device(&root, AUTO);
        let journal = root.join("state");
        std::mem::forget(raise_in(&device, &journal).unwrap());
        HOLDERS.lock().unwrap().retain(|(held, _)| *held != device);
        assert_eq!(read(&device.join(LEVEL)).unwrap(), MANUAL);
        recover_in(&journal);
        assert_eq!(read(&device.join(LEVEL)).unwrap(), AUTO);
        assert!(!journal_of(&journal, &device).exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_denied_write_names_the_rule() {
        let error = write(Path::new("/proc/version"), "x").unwrap_err();
        assert!(format!("{error:#}").contains("/proc/version"), "{error:#}");
    }
}
