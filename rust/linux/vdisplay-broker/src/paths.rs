//! Where the broker reads and writes. Production defaults, each movable for tests.
//!
//! Protocol and lease rules follow libvirtualdisplay's `vibeshine-vkms` broker (Nonary/
//! libvirtualdisplay, MIT, Copyright (c) 2026 Chase Payne); the pool uses the kernel's VKMS.
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// The VKMS device the pool creates, under configfs and under `/sys/devices/faux`.
pub const DEVICE: &str = "rubylight";

/// The path roots of one run.
#[derive(Clone, Debug)]
pub struct Paths {
    /// Configfs mount, `/sys/kernel/config`.
    pub configfs: PathBuf,
    /// Faux bus devices, `/sys/devices/faux`.
    pub faux: PathBuf,
    /// The broker's state: `pool.json` and `leases/`.
    pub run: PathBuf,
    /// systemd-logind's session state files.
    pub sessions: PathBuf,
}

impl Default for Paths {
    fn default() -> Self {
        Self {
            configfs: "/sys/kernel/config".into(),
            faux: "/sys/devices/faux".into(),
            run: "/run/rubylight-vdisplay".into(),
            sessions: "/run/systemd/sessions".into(),
        }
    }
}

impl Paths {
    /// Remove `--configfs`, `--faux`, `--run-dir` and `--sessions-dir` (each with its value)
    /// from `args` and apply them over the defaults.
    pub fn take_from(args: &mut Vec<String>) -> Result<Self> {
        let mut paths = Self::default();
        for flag in ["--configfs", "--faux", "--run-dir", "--sessions-dir"] {
            if let Some(at) = args.iter().position(|a| a == flag) {
                let value = args
                    .get(at + 1)
                    .with_context(|| format!("{flag} needs a directory"))?
                    .into();
                args.drain(at..at + 2);
                match flag {
                    "--configfs" => paths.configfs = value,
                    "--faux" => paths.faux = value,
                    "--run-dir" => paths.run = value,
                    _ => paths.sessions = value,
                }
            }
        }
        Ok(paths)
    }

    /// The device's directory in configfs.
    pub fn device(&self) -> PathBuf {
        self.configfs.join("vkms").join(DEVICE)
    }

    /// The device's DRM directory (holds `cardN`).
    pub fn drm(&self) -> PathBuf {
        self.faux.join(DEVICE).join("drm")
    }

    /// The slot-to-DRM-name map the pool writes.
    pub fn pool_json(&self) -> PathBuf {
        self.run.join("pool.json")
    }

    /// The lease directory.
    pub fn leases(&self) -> PathBuf {
        self.run.join("leases")
    }
}

/// Whether `name` is `Virtual-` and a number from 1.
pub fn public_name(name: &str) -> bool {
    name.strip_prefix("Virtual-").is_some_and(|n| {
        !n.is_empty()
            && n.len() <= 9
            && !n.starts_with('0')
            && n.bytes().all(|b| b.is_ascii_digit())
    })
}

/// The `status` file of the DRM connector `name` (`Virtual-N`), if the device has it.
pub fn drm_connector_dir(paths: &Paths, name: &str) -> Option<PathBuf> {
    let suffix = format!("-{name}");
    for card in std::fs::read_dir(paths.drm()).ok()?.flatten() {
        let card_name = card.file_name().to_string_lossy().into_owned();
        if !card_name.starts_with("card") || card_name.contains('-') {
            continue;
        }
        let dir: &Path = &card.path();
        let candidate = dir.join(format!("{card_name}{suffix}"));
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    None
}
