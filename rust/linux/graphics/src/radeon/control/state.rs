//! Atomic journal JSON helpers local to `radeon::control`: a journal is written to a temporary file, synced, and renamed over the old one.
use anyhow::{Context, Result};
use serde::Serialize;
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::Path,
};

/// The JSON in a journal file, without a byte order mark; `None` when the file is blank.
pub(super) fn json_bytes(bytes: &[u8]) -> Option<&[u8]> {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    bytes
        .iter()
        .any(|b| !b" \t\r\n\x0b\x0c".contains(b))
        .then_some(bytes)
}

/// Replaces `path` with `value` as JSON. The new content is on disk before the rename, and the
/// rename is on disk before this returns, so a crash leaves the old journal or the new one.
pub(super) fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .context("journal path missing file name")?
        .to_string_lossy();
    let tmp = parent.join(format!("{file_name}.{}.tmp", std::process::id()));
    let write_result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)?;
        // The rename itself reaches the disk with the directory.
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    write_result
}
