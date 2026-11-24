//! Archive extraction and atomic placement helpers for downloaded NVIDIA component assets.

use super::{MAX_ASSET, Pinned, VERSION, pins};
use anyhow::{Context, Result, bail, ensure};
use std::path::{Path, PathBuf};

/// Copies only the component's own files from the downloaded archive.
pub(super) fn extract(archive: &Path, asset: &str, into: &Path) -> Result<Vec<String>> {
    use std::io::{Read, Write};
    let file = std::fs::File::open(archive)
        .with_context(|| format!("cannot read {}", archive.display()))?;
    let mut zip =
        zip::ZipArchive::new(file).with_context(|| format!("{asset} is not a zip archive"))?;
    let mut taken = Vec::new();
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        let name = entry.name().to_owned();
        if entry.is_dir() || name.contains(['/', '\\']) || !pins::wanted(&name) {
            continue;
        }
        ensure!(entry.size() <= MAX_ASSET, "{name} in {asset} is too large");
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(into.join(&name))
            .with_context(|| format!("{name} appears twice in {asset}"))?;
        let mut limited = (&mut entry).take(MAX_ASSET + 1);
        let copied = std::io::copy(&mut limited, &mut out)?;
        ensure!(copied <= MAX_ASSET, "{name} in {asset} is too large");
        out.flush()?;
        taken.push(name);
    }
    Ok(taken)
}

/// A folder name from this version and the adapter's SHA-256.
pub(super) fn folder_name(sha256: &str) -> String {
    let version: String = VERSION
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!("{version}-{}", &sha256[..16])
}

/// Moves the checked files in `staging` to their folder in `root`, named
/// after the adapter ([`folder_name`]). The same files already there are
/// kept; a folder an earlier install left, perhaps with a file still loaded,
/// goes or is passed over for the next free name. A link is never used.
pub(super) fn place(
    root: &Path,
    staging: &Path,
    sha256: &str,
    runtime: &[Pinned],
) -> Result<PathBuf> {
    use crate::os_ext::MetadataExt;
    let base = folder_name(sha256);
    for n in 0..10 {
        let target = match n {
            0 => root.join(&base),
            n => root.join(format!("{base}-{n}")),
        };
        match std::fs::symlink_metadata(&target) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("cannot read {}", target.display()));
            }
            Ok(meta) if meta.is_dir() && meta.file_attributes() & 0x400 == 0 => {
                if pins::verify_with(&target, sha256, runtime).is_ok() {
                    std::fs::remove_dir_all(staging)?;
                    return Ok(target);
                }
                if std::fs::remove_dir_all(&target).is_err() {
                    continue;
                }
            }
            Ok(_) => continue,
        }
        std::fs::rename(staging, &target).with_context(|| {
            format!("cannot move the NVIDIA components to {}", target.display())
        })?;
        return Ok(target);
    }
    bail!(
        "{} holds no free folder for the NVIDIA components; restart the host and download them again",
        root.display()
    )
}
