//! Installer download, range resumption, staging folder security, and cleanup.

use super::{Installer, busy, client_with, held_for_others, now, supported};
use crate::state::Shared;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use tokio::io::AsyncWriteExt;

/// Publish the result of the download for the current `download_id`.
pub(super) fn finish_download(state: &mut Value, result: Result<PathBuf>) {
    match result {
        Ok(path) => {
            state["installer_path"] = json!(path);
            state["phase"] = json!("ready");
        }
        // What arrived is kept (`partial`): the download goes on from there.
        Err(error) if error.is::<DownloadPaused>() => {
            state["phase"] = json!("waiting");
            state["idle_since"] = json!(0);
        }
        Err(error) => {
            // A transfer that broke off (network, disk) is tried again at the
            // next scheduled check; an installer that does not match its
            // release is not downloaded again automatically.
            if !error.is::<InstallerRejected>() {
                state["attempted_version"] = Value::Null;
            }
            failed(state, error)
        }
    }
}

pub(super) fn failed(state: &mut Value, error: anyhow::Error) {
    tracing::warn!(%error, "update failed");
    state["phase"] = json!("failed");
    state["error"] = json!(format!("{error:#}"));
}

/// A body that ends early without a transport error (a proxy or server closing cleanly) is a
/// broken-off transfer to try again, not an installer to refuse.
pub(super) fn arrived(size: u64, candidate: &Installer) -> Result<()> {
    if size < candidate.size {
        bail!("The download ended before the whole installer arrived");
    }
    Ok(())
}

pub(super) fn verify(size: u64, digest: &str, candidate: &Installer) -> Result<()> {
    if size != candidate.size || digest != candidate.digest {
        return Err(
            InstallerRejected("Installer verification failed; no files were installed").into(),
        );
    }
    Ok(())
}

pub(super) async fn download(h: &Shared, id: &str, candidate: &Installer) -> Result<PathBuf> {
    check_recovery(&h.directory)?;
    // No record names a backup still to be restored.
    remove_legacy_downloads(&h.directory.join("updates"));
    // A transfer of the same installer that paused or broke off goes on where it stopped.
    let kept = h.updates.lock().unwrap()["partial"].take();
    let path = match resumable(&kept, candidate) {
        Some(path) => path,
        None => prepare_download(&install_folder()?, id)?.join("setup.exe"),
    };
    let directory = path
        .parent()
        .context("download folder missing")?
        .to_path_buf();
    let result = transfer(h, id, candidate, &path).await;
    if let Err(error) = &result {
        let mut state = h.updates.lock().unwrap();
        // Not a cancelled download, nor an installer refused for its release.
        if state["download_id"] == id && !error.is::<InstallerRejected>() {
            state["partial"] = json!({
                "path": path,
                "url": candidate.url,
                "size": candidate.size,
                "digest": candidate.digest,
            });
        } else {
            drop(state);
            remove_download(&directory);
        }
    }
    result
}
/// The file an earlier transfer left (`partial`) when it was for this same installer (address,
/// size and digest) and holds part of it.
pub(super) fn resumable(kept: &Value, candidate: &Installer) -> Option<PathBuf> {
    if kept["url"] != candidate.url
        || kept["size"] != candidate.size
        || kept["digest"] != candidate.digest
    {
        return None;
    }
    let path = PathBuf::from(kept["path"].as_str()?);
    let metadata = std::fs::symlink_metadata(&path).ok()?;
    (metadata.is_file() && metadata.len() > 0 && metadata.len() < candidate.size).then_some(path)
}

/// The folder this host runs from, which setup installed and secured.
fn install_folder() -> Result<PathBuf> {
    Ok(std::env::current_exe()?
        .parent()
        .context("installation folder missing")?
        .to_path_buf())
}
/// Setup's staging folder in the installation folder (setup's
/// `install::staging_root`). The installer is downloaded there, never to
/// ProgramData: Defender's cloud model scores an unsigned setup that runs
/// elevated from ProgramData as malware.
fn staging_folder(install: &Path) -> PathBuf {
    install.join(".staging")
}
const DOWNLOAD_PREFIX: &str = "download-";
/// A new, empty folder for download `id` in `install`'s staging folder,
/// limited to SYSTEM and Administrators before anything lands in it. A link
/// at or in the staging folder is refused, never followed. Earlier
/// downloads go first; one a cancelled transfer still holds open stays and
/// cleans itself up.
pub(super) fn prepare_download(install: &Path, id: &str) -> Result<PathBuf> {
    let staging = staging_folder(install);
    match std::fs::symlink_metadata(&staging) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            butterpollo_win_io::process::create_folder_administrators_only(&staging)?
        }
        Err(error) => return Err(error.into()),
        Ok(_) => {}
    }
    butterpollo_win_io::process::restrict_to_administrators(&staging)
        .with_context(|| format!("securing {}", staging.display()))?;
    remove_downloads(&staging);
    // A UUID folder per download avoids partial downloads and overlapping
    // cancellation races; a new one, so nothing made before is inside it.
    let directory = staging.join(format!("{DOWNLOAD_PREFIX}{id}"));
    butterpollo_win_io::process::create_folder_administrators_only(&directory)?;
    Ok(directory)
}
/// Remove the download folders in `staging`. Returns how many are left: one a setup still runs from, or a cancelled transfer
/// still writes to.
pub(super) fn remove_downloads(staging: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(staging) else {
        return 0;
    };
    let mut left = 0;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name
            .strip_prefix(DOWNLOAD_PREFIX)
            .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
        {
            continue;
        }
        if let Err(error) = remove_tree(&entry.path()) {
            tracing::debug!(%error, folder = %entry.path().display(), "download folder not removed yet");
            left += 1;
        }
    }
    left
}
/// One download folder, and its staging folder once nothing else is in it.
pub(super) fn remove_download(directory: &Path) {
    if let Err(error) = remove_tree(directory)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(%error, folder = %directory.display(), "the update download could not be removed");
    }
    if let Some(staging) = directory.parent() {
        let _ = std::fs::remove_dir(staging);
    }
}
/// `path` and everything in it; a link is removed itself, never followed.
fn remove_tree(path: &Path) -> std::io::Result<()> {
    use std::os::windows::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_attributes() & 0x400 != 0 {
        std::fs::remove_dir(path).or_else(|_| std::fs::remove_file(path))
    } else if metadata.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}
/// Earlier hosts downloaded into the profile's `updates` folder, where
/// earlier setups also kept their transactions. Its downloads go (a link
/// is removed itself), its transactions but the newest two, and the folder
/// once empty. Only after `check_recovery`: no record may name a backup
/// still to be restored.
pub(super) fn remove_legacy_downloads(updates: &Path) {
    let Ok(entries) = std::fs::read_dir(updates) else {
        return;
    };
    let mut transactions = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if uuid::Uuid::parse_str(&name).is_ok() {
            if let Err(error) = remove_tree(&entry.path()) {
                tracing::warn!(%error, folder = %entry.path().display(), "an earlier update download could not be removed");
            }
        } else if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            transactions.push(name);
        }
    }
    for name in stale_transactions(&transactions) {
        let _ = remove_tree(&updates.join(name));
    }
    let _ = std::fs::remove_dir(updates);
}
/// When the next `tidy_downloads` runs, in seconds since the epoch.
pub(super) static NEXT_TIDY: AtomicU64 = AtomicU64::new(0);
/// A download was just given up: the tidy looks again at the next maintenance pass. It stops
/// looking once it finds no folder left, which without this would leave one given up later
/// (a cancelled download, automatic updates turned off) until the host restarts.
pub(super) fn tidy_soon() {
    NEXT_TIDY.store(0, Ordering::Relaxed);
}
/// A setup that updated this host ran from its download folder until after
/// it started this host, so only this host can remove that folder once
/// setup has exited: tried every minute until none is left.
pub(super) fn tidy_downloads(h: &Shared) {
    let now = now();
    if NEXT_TIDY.load(Ordering::Relaxed) > now {
        return;
    }
    NEXT_TIDY.store(now + 60, Ordering::Relaxed);
    let (phase, partial) = {
        let state = h.updates.lock().unwrap();
        (state["phase"].clone(), !state["partial"].is_null())
    };
    // A paused or broken-off download's folder stays for its next try.
    if matches!(phase.as_str(), Some("downloading" | "ready" | "installing"))
        || partial
        || !supported(h)
    {
        return;
    }
    let Ok(install) = install_folder() else {
        return;
    };
    let staging = staging_folder(&install);
    if remove_downloads(&staging) == 0 {
        // Once nothing else is in it (setup keeps a failed update's backup
        // there).
        let _ = std::fs::remove_dir(&staging);
        NEXT_TIDY.store(u64::MAX, Ordering::Relaxed);
    }
}

pub(super) fn check_recovery(profile: &std::path::Path) -> Result<Value> {
    let record =
        butterpollo_session::state::load_json(&profile.join("update-result.json"), Value::Null)
            .context("The last update's record is unreadable. Run the Rubylight installer to repair this installation")?;
    if record["phase"] == "recovery_failed"
        || (record["phase"] == "installing" && record["backup"].is_string())
    {
        bail!(
            "An earlier update could not be rolled back. Run the Rubylight installer to repair this installation; keep the backup the update record names until it has finished."
        );
    }
    Ok(record)
}

/// Setup's update folders (transaction-<process id>-<nanoseconds>, kept
/// with their backup when an update fails) older than the newest two.
/// Other names are left alone.
pub(super) fn stale_transactions(names: &[String]) -> Vec<&str> {
    let mut dated: Vec<(u128, &str)> = names
        .iter()
        .filter_map(|name| {
            let (process, time) = name.strip_prefix("transaction-")?.split_once('-')?;
            process.parse::<u32>().ok()?;
            Some((time.parse().ok()?, name.as_str()))
        })
        .collect();
    dated.sort_unstable_by(|a, b| b.cmp(a));
    dated.into_iter().skip(2).map(|(_, name)| name).collect()
}

pub(super) async fn transfer(
    h: &Shared,
    id: &str,
    candidate: &Installer,
    path: &std::path::Path,
) -> Result<PathBuf> {
    // A download for "Install now" carries on beside the stream, but not beside
    // another device's when a device asked.
    let paused = || {
        // Its own statement: the updates lock is free again before
        // `held_for_others` takes it.
        let now = h.updates.lock().unwrap()["now"] == true;
        (busy(h) && !now) || held_for_others(h).is_some()
    };
    if paused() {
        return Err(DownloadPaused.into());
    }
    // What a paused or broken-off transfer of this installer left (`download` checked it is
    // the same installer): asked for from there on.
    let kept = match tokio::fs::metadata(path).await {
        Ok(metadata) if metadata.len() < candidate.size => metadata.len(),
        _ => 0,
    };
    let client = client_with(None)?;
    let mut request = client.get(&candidate.url);
    if kept > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={kept}-"));
    }
    let mut response = idle(request.send()).await??;
    let resumed = kept > 0
        && resumes(
            response.status().as_u16(),
            response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|value| value.to_str().ok()),
            kept,
            candidate.size,
        );
    // A server that ignores the range sends the whole installer; any other answer is asked
    // again without one.
    if kept > 0 && !resumed {
        tracing::info!(kept, status = %response.status(), "the update download starts again: the server did not resume it");
        if response.status() != reqwest::StatusCode::OK {
            response = idle(client.get(&candidate.url).send()).await??;
        }
    }
    let mut response = response.error_for_status()?;
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true);
    if kept == 0 {
        options.create_new(true);
    } else if resumed {
        options.append(true);
    } else {
        options.truncate(true);
    }
    let mut file = options.open(&path).await?;
    let (mut size, mut hash) = if resumed {
        let prefix = path.to_path_buf();
        let hash = tokio::task::spawn_blocking(move || hash_prefix(&prefix, kept)).await??;
        (kept, hash)
    } else {
        (0, Sha256::new())
    };
    h.updates.lock().unwrap()["downloaded_bytes"] = json!(size);
    while let Some(chunk) = idle(response.chunk()).await?? {
        if paused() {
            return Err(DownloadPaused.into());
        }
        if h.updates.lock().unwrap()["download_id"] != id {
            bail!("Download cancelled");
        }
        size += chunk.len() as u64;
        if size > candidate.size {
            return Err(InstallerRejected("Installer exceeds its advertised size").into());
        }
        file.write_all(&chunk).await?;
        hash.update(&chunk);
        h.updates.lock().unwrap()["downloaded_bytes"] = json!(size);
    }
    file.sync_all().await?;
    drop(file);
    arrived(size, candidate)?;
    let digest = hex::encode(hash.finalize());
    if resumed && verify(size, &digest, candidate).is_err() {
        // The part kept from before did not belong with the rest: the next try starts over.
        let _ = tokio::fs::remove_file(path).await;
        bail!("The resumed download did not match its release; it starts again");
    }
    verify(size, &digest, candidate)?;
    if h.updates.lock().unwrap()["download_id"] != id {
        bail!("Download cancelled");
    }
    Ok(path.to_path_buf())
}

/// How long the installer download may receive nothing before it counts as broken off. A slow
/// link takes as long as it needs.
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
/// `step` of the download, failed when nothing arrives for [`IDLE_TIMEOUT`].
async fn idle<T>(step: impl std::future::Future<Output = T>) -> Result<T> {
    tokio::time::timeout(IDLE_TIMEOUT, step).await.map_err(|_| {
        anyhow::anyhow!(
            "The download received nothing for {} seconds",
            IDLE_TIMEOUT.as_secs()
        )
    })
}
/// Whether the answer to `Range: bytes={from}-` carries the rest of a `size`-byte installer.
pub(super) fn resumes(status: u16, content_range: Option<&str>, from: u64, size: u64) -> bool {
    status == 206
        && from < size
        && content_range == Some(format!("bytes {from}-{}/{size}", size - 1).as_str())
}
/// The hash of the first `length` bytes of `path`, which must hold exactly that many.
pub(super) fn hash_prefix(path: &Path, length: u64) -> Result<Sha256> {
    let file = std::fs::File::open(path)?;
    let mut hash = Sha256::new();
    let read = std::io::copy(&mut std::io::Read::take(file, length), &mut hash)?;
    anyhow::ensure!(
        read == length,
        "the kept part of the download is shorter than recorded"
    );
    Ok(hash)
}

#[derive(Debug)]
pub(super) struct DownloadPaused;
impl std::fmt::Display for DownloadPaused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("download paused while the host is busy")
    }
}
impl std::error::Error for DownloadPaused {}

/// The downloaded installer does not match its release's size or digest.
#[derive(Debug)]
pub(super) struct InstallerRejected(&'static str);
impl std::fmt::Display for InstallerRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for InstallerRejected {}
