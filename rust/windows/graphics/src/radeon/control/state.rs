//! Atomic journal JSON helpers local to `radeon::control`.
use anyhow::{Context, Result};
use serde::Serialize;
use std::{fs::OpenOptions, io::Write, path::Path};

pub(super) fn json_bytes(bytes: &[u8]) -> Option<&[u8]> {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    bytes
        .iter()
        .any(|b| !b" \t\r\n\x0b\x0c".contains(b))
        .then_some(bytes)
}

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
        let wide_tmp = crate::text::to_wide(&tmp.to_string_lossy());
        let wide_dst = crate::text::to_wide(&path.to_string_lossy());
        // SAFETY: Both paths are NUL-terminated UTF-16 buffers alive for the call.
        unsafe {
            windows::Win32::Storage::FileSystem::MoveFileExW(
                windows::core::PCWSTR(wide_tmp.as_ptr()),
                windows::core::PCWSTR(wide_dst.as_ptr()),
                windows::Win32::Storage::FileSystem::MOVEFILE_REPLACE_EXISTING
                    | windows::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH,
            )?;
        }
        Ok(())
    })();
    if write_result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    write_result
}
