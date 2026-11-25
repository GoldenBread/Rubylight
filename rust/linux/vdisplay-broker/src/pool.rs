//! The pool: N hot-pluggable monitors on one stock VKMS device, made once at boot.
//!
//! The pool uses the kernel's own VKMS configfs (`modprobe vkms create_default_dev=0`, done by
//! the unit); the map file and the connect/disconnect rules around it follow libvirtualdisplay's
//! `linux/packaging/vibeshine-vkms` broker (Nonary/libvirtualdisplay, MIT, Copyright (c) 2026
//! Chase Payne).
use crate::lease::Leases;
use crate::paths::Paths;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::Path;
use std::time::{Duration, Instant};

pub const DEFAULT_COUNT: usize = 4;
pub const MAX_COUNT: usize = 16;
const WAIT: Duration = Duration::from_secs(5);

/// One pooled monitor: its configfs name and the DRM name the host and the compositor see.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Entry {
    pub slot: String,
    pub drm: String,
}

/// `pool.json`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Map {
    pub connectors: Vec<Entry>,
}

impl Map {
    /// The configfs slot behind the public name.
    pub fn slot_of(&self, drm: &str) -> Option<&str> {
        self.connectors
            .iter()
            .find(|e| e.drm == drm)
            .map(|e| e.slot.as_str())
    }

    /// Read `pool.json` of a run.
    pub fn load(paths: &Paths) -> Result<Self> {
        let text = fs::read_to_string(paths.pool_json())
            .with_context(|| format!("no pool map at {}", paths.pool_json().display()))?;
        Ok(serde_json::from_str(&text)?)
    }
}

/// Make the connector status file say `connected` (1) or `disconnected` (2).
pub fn set_status(paths: &Paths, slot: &str, connected: bool) -> Result<()> {
    let file = paths.device().join("connectors").join(slot).join("status");
    fs::write(&file, if connected { "1\n" } else { "2\n" })
        .with_context(|| format!("could not write {}", file.display()))
}

fn mkdir(path: &Path) -> Result<()> {
    match fs::create_dir(path) {
        Ok(()) => Ok(()),
        // Configfs makes some subdirectories itself; a re-run finds the rest.
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e).with_context(|| format!("could not create {}", path.display())),
    }
}

fn link(target: &Path, dir: &Path) -> Result<()> {
    let name = target.file_name().context("link target without a name")?;
    mkdir(dir)?;
    match std::os::unix::fs::symlink(target, dir.join(name)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e).with_context(|| format!("could not link {}", target.display())),
    }
}

/// Build the chain plane, crtc, encoder, connector number `i` (from 1).
fn chain(device: &Path, i: usize) -> Result<()> {
    let plane = device.join("planes").join(format!("primary-{i}"));
    let crtc = device.join("crtcs").join(format!("crtc-{i}"));
    let encoder = device.join("encoders").join(format!("encoder-{i}"));
    let connector = device.join("connectors").join(format!("connector-{i}"));
    mkdir(&plane)?;
    fs::write(plane.join("type"), "1\n").context("could not set the plane type")?;
    mkdir(&crtc)?;
    link(&crtc, &plane.join("possible_crtcs"))?;
    mkdir(&encoder)?;
    link(&crtc, &encoder.join("possible_crtcs"))?;
    mkdir(&connector)?;
    link(&encoder, &connector.join("possible_encoders"))?;
    fs::write(connector.join("status"), "2\n").context("could not set the connector status")?;
    Ok(())
}

fn enabled(device: &Path) -> bool {
    fs::read_to_string(device.join("enabled")).is_ok_and(|s| s.trim() == "1")
}

/// The device's DRM connectors as (connector_id, `Virtual-K`), ascending by id.
fn drm_connectors(paths: &Paths) -> Vec<(u32, String)> {
    let mut found = Vec::new();
    let Ok(cards) = fs::read_dir(paths.drm()) else {
        return found;
    };
    for card in cards.flatten() {
        let card_name = card.file_name().to_string_lossy().into_owned();
        if !card_name.starts_with("card") || card_name.contains('-') {
            continue;
        }
        let Ok(entries) = fs::read_dir(card.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(drm) = name.strip_prefix(&format!("{card_name}-")) else {
                continue;
            };
            if !crate::paths::public_name(drm) {
                continue;
            }
            let id = fs::read_to_string(entry.path().join("connector_id"))
                .ok()
                .and_then(|s| s.trim().parse::<u32>().ok());
            if let Some(id) = id {
                found.push((id, drm.to_string()));
            }
        }
    }
    found.sort();
    found
}

/// The configfs connector slots, `connector-1` upward.
fn slots(device: &Path) -> Result<Vec<String>> {
    let mut found: Vec<(usize, String)> = Vec::new();
    for entry in fs::read_dir(device.join("connectors"))?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(n) = name.strip_prefix("connector-").and_then(|n| n.parse().ok()) {
            found.push((n, name));
        }
    }
    found.sort();
    Ok(found.into_iter().map(|(_, name)| name).collect())
}

/// Map slots to DRM names: VKMS creates connectors in configfs creation order, so the n-th
/// slot is the connector with the n-th smallest id.
fn build_map(paths: &Paths, wait: Duration) -> Result<Map> {
    let slots = slots(&paths.device())?;
    let deadline = Instant::now() + wait;
    let drm = loop {
        let drm = drm_connectors(paths);
        if drm.len() >= slots.len() {
            break drm;
        }
        if Instant::now() >= deadline {
            bail!(
                "VKMS made {} of {} connectors under {} within {} s",
                drm.len(),
                slots.len(),
                paths.drm().display(),
                wait.as_secs()
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    if drm.len() != slots.len() {
        bail!(
            "{} has {} DRM connectors but the pool has {} slots",
            paths.drm().display(),
            drm.len(),
            slots.len()
        );
    }
    Ok(Map {
        connectors: slots
            .into_iter()
            .zip(drm)
            .map(|(slot, (_, drm))| Entry { slot, drm })
            .collect(),
    })
}

fn write_map(paths: &Paths, map: &Map) -> Result<()> {
    let target = paths.pool_json();
    let temp = paths.run.join(".pool.json.new");
    fs::write(&temp, serde_json::to_vec(map)?)?;
    fs::set_permissions(&temp, fs::Permissions::from_mode(0o644))?;
    fs::rename(&temp, &target).context("could not write pool.json")
}

/// Create the pool (or find it enabled), then write the map.
pub fn start_with(paths: &Paths, count: usize, wait: Duration) -> Result<()> {
    if count == 0 || count > MAX_COUNT {
        bail!("--count must be 1 to {MAX_COUNT}");
    }
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o755)
        .create(&paths.run)
        .with_context(|| format!("could not create {}", paths.run.display()))?;
    fs::set_permissions(&paths.run, fs::Permissions::from_mode(0o755))?;
    Leases::open(&paths.leases())?;
    let device = paths.device();
    if !enabled(&device) {
        if !paths.configfs.join("vkms").is_dir() {
            bail!(
                "{} is missing: load vkms with create_default_dev=0 first",
                paths.configfs.join("vkms").display()
            );
        }
        mkdir(&device)?;
        for sub in ["planes", "crtcs", "encoders", "connectors"] {
            mkdir(&device.join(sub))?;
        }
        for i in 1..=count {
            chain(&device, i).with_context(|| format!("could not build connector {i}"))?;
        }
        fs::write(device.join("enabled"), "1\n").context("could not enable the VKMS device")?;
    }
    let map = build_map(paths, wait)?;
    write_map(paths, &map)?;
    eprintln!("pool: {} connectors", map.connectors.len());
    Ok(())
}

/// `pool start`.
pub fn start(paths: &Paths, count: usize) -> Result<()> {
    start_with(paths, count, WAIT)
}

/// `pool stop`: every connector disconnected, leases removed; the device stays.
pub fn stop(paths: &Paths) -> Result<()> {
    let device = paths.device();
    if !device.is_dir() {
        return Ok(());
    }
    let leases = Leases::open(&paths.leases())?;
    let mut failed = 0;
    for slot in slots(&device)? {
        if let Err(e) = set_status(paths, &slot, false) {
            eprintln!("pool stop: {e:#}");
            failed += 1;
        }
        if let Err(e) = leases.release(&slot) {
            eprintln!("pool stop: {e:#}");
            failed += 1;
        }
    }
    if failed > 0 {
        bail!("{failed} steps failed");
    }
    Ok(())
}
