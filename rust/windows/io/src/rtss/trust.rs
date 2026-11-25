//! Handle-pinned trust, reparse-point, and safe library/profile I/O helpers for RTSS.
//!
//! RTSS runs elevated and the Windows service loads `RTSSHooks64.dll` as SYSTEM and writes
//! `Profiles\Global` as SYSTEM. Every directory from the drive root down to the RTSS folder,
//! its executables/DLLs, and its `Profiles\Global` file are opened without following reparse
//! points, verified by handle (`GetFinalPathNameByHandleW`, owner, and DACL), and held open
//! while used.

use anyhow::{Context, Result, bail, ensure};
use butterpollo_session::neural::{Rights, Security, security_verdict};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::windows::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};
use windows::Win32::{
    Storage::FileSystem::{
        DELETE, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_WRITE, FILE_READ_ATTRIBUTES, FILE_RENAME_INFO,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FileRenameInfo, READ_CONTROL,
        SetFileInformationByHandle,
    },
    System::LibraryLoader::{LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32},
};

/// Maximum size read from `Profiles\Global`.
const MAX_PROFILE_BYTES: u64 = 4 * 1024 * 1024;

/// How strictly an RTSS path is validated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TrustMode {
    /// SYSTEM/Administrators/TrustedInstaller only (owner + DACL) and no reparse points.
    AdministratorsOnly,
    /// Reject reparse points and verify final handle paths (non-SYSTEM profile read/write).
    ReparseOnly,
}

/// Validated `root` and every ancestor directory up to the drive root, held open without
/// delete sharing so none can be renamed or swapped for a junction while in use.
pub(super) struct PinnedRoot {
    _handles: Vec<File>,
    resolved: PathBuf,
}

impl PinnedRoot {
    /// The resolved `\\?\...` path of `root` as returned by `GetFinalPathNameByHandleW`.
    pub(super) fn resolved(&self) -> &Path {
        &self.resolved
    }

    /// A standard Win32 path (`C:\...`) for APIs such as `CreateProcess` and `LoadLibraryExW`.
    pub(super) fn win32_path(&self) -> PathBuf {
        strip_verbatim(&self.resolved)
    }
}

/// A validated binary (`RTSS.exe` or `RTSSHooks64.dll`) pinned open alongside its root chain.
pub(super) struct PinnedBinary {
    _root: PinnedRoot,
    _file: File,
    path: PathBuf,
}

impl PinnedBinary {
    pub(super) fn root_path(&self) -> PathBuf {
        self._root.win32_path()
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

/// Strip the `\\?\` prefix from local drive paths (`\\?\C:\...` -> `C:\...`).
pub(super) fn strip_verbatim(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\")
        && !rest.starts_with(r"UNC\")
    {
        return PathBuf::from(rest);
    }
    path.to_path_buf()
}

/// Resolve an account SID to `DOMAIN\name`, or `"another account"` if unknown.
fn account_label(sid: &str) -> String {
    crate::process::account_name(sid).unwrap_or_else(|| "another account".into())
}

/// Pure verdict matching `neural::security_verdict` for RTSS trust checks.
pub(super) fn check_security(path: &Path, security: &Security, mode: TrustMode) -> Result<()> {
    if security.link {
        bail!("{} is a link to elsewhere", path.display());
    }
    if mode == TrustMode::AdministratorsOnly {
        security_verdict(&path.display().to_string(), security, account_label)
            .map_err(|problem| anyhow::anyhow!("untrusted RTSS path: {problem}"))?;
    }
    Ok(())
}

/// Inspect an already-open handle for reparse points and (under `AdministratorsOnly`) owner/DACL.
fn verify_handle(file: &File, path: &Path, rights: Rights, mode: TrustMode) -> Result<()> {
    let attrs = file
        .metadata()
        .with_context(|| format!("reading metadata of {}", path.display()))?
        .file_attributes();
    let link = attrs & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0;
    if mode == TrustMode::ReparseOnly {
        if link {
            bail!("{} is a link to elsewhere", path.display());
        }
        return Ok(());
    }
    let owner = crate::process::open_file_owner(file)
        .with_context(|| format!("reading owner of {}", path.display()))?;
    let writers = match rights {
        Rights::Change => crate::process::open_file_writers(file),
        Rights::MoveContents => crate::process::open_file_child_movers(file),
        Rights::Move => crate::process::open_file_movers(file),
    }
    .with_context(|| format!("reading permissions of {}", path.display()))?;
    check_security(
        path,
        &Security {
            link,
            owner,
            writers,
        },
        mode,
    )
}

/// Verify an open handle's resolved path matches `expected` (case-insensitive NTFS ordinal).
fn verify_final_path(file: &File, expected: &Path, shown: &Path) -> Result<()> {
    let opened = crate::user_folder::final_path(file)
        .with_context(|| format!("reading resolved path of {}", shown.display()))?;
    ensure!(
        crate::user_folder::same_name(&opened, expected.as_os_str()),
        "{} opens as {}, through a link on the way",
        shown.display(),
        Path::new(&opened).display()
    );
    Ok(())
}

/// Whether `path` explicitly starts with a non-local-disk Windows prefix (UNC share,
/// verbatim UNC, device namespace `\\.\...`, or verbatim non-disk prefix). Checked
/// before any filesystem syscall so UNC/device paths never trigger outbound SMB or device opens.
fn has_non_disk_prefix(path: &Path) -> bool {
    use std::path::{Component, Prefix};
    matches!(
        path.components().next(),
        Some(Component::Prefix(prefix))
            if !matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_))
    )
}

/// Whether `path` uses a local drive prefix (`C:\...` or `\\?\C:\...`).
fn is_local_disk_path(path: &Path) -> bool {
    use std::path::{Component, Prefix};
    matches!(
        path.components().next(),
        Some(Component::Prefix(prefix))
            if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_))
    )
}

/// Validate and pin `root` and every ancestor directory up to the drive root.
pub(super) fn pin_root(root: &Path, mode: TrustMode) -> Result<PinnedRoot> {
    ensure!(
        !has_non_disk_prefix(root),
        "RTSS folder {} must be on a local drive",
        root.display()
    );
    let absolute = std::path::absolute(root)
        .with_context(|| format!("resolving RTSS folder {}", root.display()))?;
    ensure!(
        is_local_disk_path(&absolute),
        "RTSS folder {} must be on a local drive",
        root.display()
    );
    let mut ancestors: Vec<&Path> = absolute
        .ancestors()
        .filter(|p| !p.as_os_str().is_empty())
        .collect();
    ensure!(
        !ancestors.is_empty(),
        "RTSS folder {} is invalid",
        root.display()
    );
    ancestors.reverse();
    let mut handles = Vec::with_capacity(ancestors.len());
    for dir in &ancestors {
        let handle = crate::process::open_unfollowed(dir, true)
            .with_context(|| format!("opening RTSS path {}", dir.display()))?;
        let attrs = handle
            .metadata()
            .with_context(|| format!("reading {}", dir.display()))?
            .file_attributes();
        ensure!(
            attrs & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0,
            "{} is a link to elsewhere",
            dir.display()
        );
        ensure!(
            attrs & FILE_ATTRIBUTE_DIRECTORY.0 != 0,
            "{} is not a directory",
            dir.display()
        );
        let rights = if dir.parent().is_none() {
            Rights::MoveContents
        } else {
            Rights::Change
        };
        verify_handle(&handle, dir, rights, mode)?;
        handles.push(handle);
    }
    let root_handle = handles.last().expect("ancestors is non-empty");
    let resolved = PathBuf::from(
        crate::user_folder::final_path(root_handle)
            .with_context(|| format!("reading resolved path of {}", root.display()))?,
    );
    if mode == TrustMode::AdministratorsOnly {
        let text = resolved.to_string_lossy();
        ensure!(
            !text.starts_with(r"\\?\UNC\"),
            "RTSS folder {} must be on a local drive",
            root.display()
        );
    }
    Ok(PinnedRoot {
        _handles: handles,
        resolved,
    })
}

/// Validate and pin a binary (`RTSS.exe` or `RTSSHooks64.dll`) inside `root`.
pub(super) fn pin_binary(
    root: &Path,
    candidates: &[&str],
    missing_message: &str,
    mode: TrustMode,
) -> Result<PinnedBinary> {
    let pinned_root = pin_root(root, mode)?;
    for name in candidates {
        let candidate = pinned_root.resolved().join(name);
        let shown = root.join(name);
        let file = match OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ.0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(&candidate)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("opening {}", shown.display()));
            }
        };
        let attrs = file
            .metadata()
            .with_context(|| format!("reading {}", shown.display()))?
            .file_attributes();
        ensure!(
            attrs & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0,
            "{} is a link to elsewhere",
            shown.display()
        );
        ensure!(
            attrs & FILE_ATTRIBUTE_DIRECTORY.0 == 0,
            "{} is a directory, not a file",
            shown.display()
        );
        verify_final_path(&file, &candidate, &shown)?;
        verify_handle(&file, &shown, Rights::Change, mode)?;
        let path = pinned_root.win32_path().join(name);
        return Ok(PinnedBinary {
            _root: pinned_root,
            _file: file,
            path,
        });
    }
    bail!("{missing_message}")
}

/// Load a validated, handle-pinned DLL using `LoadLibraryExW` with restricted search flags.
pub(super) fn load_library_pinned(binary: &PinnedBinary) -> Result<libloading::Library> {
    let flags = (LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32).0;
    // SAFETY: `binary` has already verified the DLL and every ancestor directory as trusted and
    // non-reparse, and holds them open without write/delete sharing for the duration of the load.
    let library =
        unsafe { libloading::os::windows::Library::load_with_flags(binary.path(), flags) }
            .with_context(|| format!("loading {}", binary.path().display()))?;
    Ok(library.into())
}

/// Pin `root\Profiles` without following links; returns `Ok(None)` when `Profiles` is absent
/// and `create` is false.
fn pin_profiles(
    pinned_root: &PinnedRoot,
    root: &Path,
    mode: TrustMode,
    create: bool,
) -> Result<Option<(File, PathBuf)>> {
    let at = pinned_root.resolved().join("Profiles");
    let shown = root.join("Profiles");
    let open = || crate::process::open_unfollowed(&at, true);
    let handle = match open() {
        Ok(handle) => handle,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && create => {
            match std::fs::create_dir(&at) {
                Err(err) if err.kind() != std::io::ErrorKind::AlreadyExists => {
                    return Err(err).with_context(|| format!("creating {}", shown.display()));
                }
                _ => {}
            }
            open().with_context(|| format!("opening {}", shown.display()))?
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("opening {}", shown.display())),
    };
    let attrs = handle
        .metadata()
        .with_context(|| format!("reading {}", shown.display()))?
        .file_attributes();
    ensure!(
        attrs & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0,
        "{} is a link to elsewhere",
        shown.display()
    );
    ensure!(
        attrs & FILE_ATTRIBUTE_DIRECTORY.0 != 0,
        "{} is not a directory",
        shown.display()
    );
    verify_final_path(&handle, &at, &shown)?;
    verify_handle(&handle, &shown, Rights::Change, mode)?;
    Ok(Some((handle, at)))
}

/// Read `Profiles\Global` from an already-pinned `Profiles` directory without following links.
fn read_global_in(profiles_resolved: &Path, root: &Path, mode: TrustMode) -> Result<String> {
    let at = profiles_resolved.join("Global");
    let shown = root.join("Profiles/Global");
    let file = match OpenOptions::new()
        .read(true)
        .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(&at)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(_) => bail!("RTSS Global profile is unreadable or exceeds its size limit"),
    };
    let meta = file
        .metadata()
        .with_context(|| format!("reading {}", shown.display()))?;
    ensure!(
        meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0,
        "{} is a link to elsewhere",
        shown.display()
    );
    ensure!(
        meta.is_file(),
        "RTSS Global profile is unreadable or exceeds its size limit"
    );
    verify_final_path(&file, &at, &shown)?;
    verify_handle(&file, &shown, Rights::Change, mode)?;
    ensure!(
        meta.len() <= MAX_PROFILE_BYTES,
        "RTSS Global profile is unreadable or exceeds its size limit"
    );
    let mut bytes = Vec::new();
    file.take(MAX_PROFILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading {}", shown.display()))?;
    ensure!(
        bytes.len() as u64 <= MAX_PROFILE_BYTES,
        "RTSS Global profile is unreadable or exceeds its size limit"
    );
    String::from_utf8(bytes).with_context(|| format!("{} is not valid UTF-8", shown.display()))
}

/// Read `<root>\Profiles\Global` after handle-pinned validation.
pub(super) fn read_profile_checked(root: &Path, mode: TrustMode) -> Result<String> {
    let pinned_root = pin_root(root, mode)?;
    let Some((_profiles_handle, profiles_resolved)) =
        pin_profiles(&pinned_root, root, mode, false)?
    else {
        return Ok(String::new());
    };
    read_global_in(&profiles_resolved, root, mode)
}

/// Verify `<root>\Profiles\Global` (when present) is neither a reparse point nor untrusted.
fn verify_existing_global(profiles_resolved: &Path, root: &Path, mode: TrustMode) -> Result<()> {
    let at = profiles_resolved.join("Global");
    let shown = root.join("Profiles/Global");
    let file = match OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES.0 | READ_CONTROL.0)
        .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(&at)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).with_context(|| format!("opening {}", shown.display())),
    };
    let meta = file
        .metadata()
        .with_context(|| format!("reading {}", shown.display()))?;
    ensure!(
        meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0,
        "{} is a link to elsewhere",
        shown.display()
    );
    ensure!(meta.is_file(), "{} is not a file", shown.display());
    verify_final_path(&file, &at, &shown)?;
    verify_handle(&file, &shown, Rights::Change, mode)
}

/// Atomically replace `Global` inside a pinned `Profiles` directory via a temp file + handle rename.
fn atomic_replace_global(
    profiles_resolved: &Path,
    root: &Path,
    content: &[u8],
    mode: TrustMode,
) -> std::io::Result<()> {
    static NEXT_TMP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let seq = NEXT_TMP.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp_name = format!("Global.rubylight.{}.{seq}.tmp", std::process::id());
    let tmp_path = profiles_resolved.join(&tmp_name);
    let target_path = profiles_resolved.join("Global");
    let shown = root.join("Profiles/Global");
    verify_existing_global(profiles_resolved, root, mode).map_err(std::io::Error::other)?;
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .access_mode(FILE_GENERIC_WRITE.0 | FILE_READ_ATTRIBUTES.0 | DELETE.0)
        .share_mode(0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(&tmp_path)?;
    let replaced = (|| -> std::io::Result<()> {
        verify_final_path(&file, &tmp_path, &shown).map_err(std::io::Error::other)?;
        let mut writer = &file;
        writer.write_all(content)?;
        file.sync_all()?;
        rename_replace(&file, &target_path)?;
        verify_final_path(&file, &target_path, &shown).map_err(std::io::Error::other)
    })();
    if replaced.is_err() {
        let _ = crate::user_folder::delete_on_close(&file);
    }
    replaced
}

/// Rename an open file handle over `target` (`ReplaceIfExists = true`).
fn rename_replace(file: &File, target: &Path) -> std::io::Result<()> {
    let wide: Vec<u16> = target.as_os_str().encode_wide().collect();
    let name_bytes = wide.len() * size_of::<u16>();
    let bytes = std::mem::offset_of!(FILE_RENAME_INFO, FileName) + name_bytes + size_of::<u16>();
    let mut buffer = vec![0u64; bytes.div_ceil(size_of::<u64>())];
    let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    // SAFETY: `buffer` is u64-aligned and sized for `FILE_RENAME_INFO` plus the NUL-terminated name.
    unsafe {
        (*info).Anonymous.ReplaceIfExists = true;
        (*info).FileNameLength = name_bytes as u32;
        std::ptr::copy_nonoverlapping(
            wide.as_ptr(),
            (&raw mut (*info).FileName).cast::<u16>(),
            wide.len(),
        );
        SetFileInformationByHandle(
            crate::user_folder::raw(file),
            FileRenameInfo,
            info.cast_const().cast(),
            (buffer.len() * size_of::<u64>()) as u32,
        )
    }
    .map_err(crate::user_folder::os_error)
}

/// Write `values` to `<root>\Profiles\Global` using handle-pinned validation and non-reparse I/O.
pub(super) fn write_profile_checked(
    root: &Path,
    values: &std::collections::BTreeMap<String, Option<u32>>,
    mode: TrustMode,
) -> Result<()> {
    ensure!(
        values.keys().all(|key| super::KEYS.contains(&key.as_str())),
        "unknown RTSS property"
    );
    let pinned_root = pin_root(root, mode)?;
    let Some((_profiles_handle, profiles_resolved)) = pin_profiles(&pinned_root, root, mode, true)?
    else {
        bail!("RTSS Profiles directory is missing");
    };
    let original = read_global_in(&profiles_resolved, root, mode)?;
    let current = super::properties(&original)?;
    // A failed application may leave only already-original properties to
    // restore. Do not require write/delete access or claim pending recovery
    // for a profile that never changed.
    if values
        .iter()
        .all(|(key, value)| current.get(key) == Some(value))
    {
        return Ok(());
    }
    let content = super::replace(&original, values)?;
    if let Err(error) = atomic_replace_global(&profiles_resolved, root, content.as_bytes(), mode) {
        // RTSS's UI opens the selected profile without delete sharing. It
        // still permits writing the file; open without following reparse
        // points and verify the handle's final path and ACL before writing.
        let locked = matches!(error.raw_os_error(), Some(5 | 32));
        if !locked {
            return Err(error)
                .with_context(|| format!("writing {}", root.join("Profiles/Global").display()));
        }
        let target = profiles_resolved.join("Global");
        let shown = root.join("Profiles/Global");
        let mut file = OpenOptions::new()
            .access_mode(FILE_GENERIC_WRITE.0 | FILE_READ_ATTRIBUTES.0 | READ_CONTROL.0)
            .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE).0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(&target)
            .context("open RTSS Global profile for update")?;
        let meta = file
            .metadata()
            .with_context(|| format!("reading {}", shown.display()))?;
        ensure!(
            meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0,
            "{} is a link to elsewhere",
            shown.display()
        );
        ensure!(meta.is_file(), "{} is not a file", shown.display());
        verify_final_path(&file, &target, &shown)?;
        verify_handle(&file, &shown, Rights::Change, mode)?;
        file.write_all(content.as_bytes())?;
        file.set_len(content.len() as u64)?;
        file.sync_all()?;
    }
    Ok(())
}

/// Whether `path` is a plain file (not a reparse point or directory).
pub(super) fn is_plain_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| {
        meta.is_file() && meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0
    })
}

/// Check that `path` matches one of `names` without following links or touching non-local paths.
pub(super) fn find_plain(root: &Path, names: &[&str]) -> Option<PathBuf> {
    if has_non_disk_prefix(root) {
        return None;
    }
    let absolute = std::path::absolute(root).ok()?;
    if !is_local_disk_path(&absolute) {
        return None;
    }
    if std::fs::symlink_metadata(root)
        .is_ok_and(|m| m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 || !m.is_dir())
    {
        return None;
    }
    names
        .iter()
        .map(|s| root.join(s))
        .find(|p| is_plain_file(p))
}
