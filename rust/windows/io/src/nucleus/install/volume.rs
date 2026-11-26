//! Drive placement, volume identity, file stamping, and Mark of the Web checks
//! for Nucleus Co-op install folders.
use super::{Checks, PROGRAM_FILES, refuse};
use anyhow::{Context, Result, bail};
use butterpollo_session::nucleus::install::{Manifest, Problem, Shipped};
use std::collections::HashMap;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// GetDriveTypeW's answers.
pub(super) const DRIVE_NO_ROOT_DIR: u32 = 1;
pub(super) const DRIVE_REMOVABLE: u32 = 2;
pub(super) const DRIVE_FIXED: u32 = 3;
pub(super) const DRIVE_REMOTE: u32 = 4;
pub(super) const DRIVE_CDROM: u32 = 5;
pub(super) const DRIVE_RAMDISK: u32 = 6;

/// Where `folder` is, unless that alone refuses it: no device or network
/// path, no relative one, none through `.` or `..`, and on a fixed drive
/// inside this PC. Its drive's root (`C:\`) and the drive.
pub(super) fn placed(folder: &Path, checks: &Checks) -> Result<(String, Volume)> {
    let shown = folder.display();
    let text = folder.to_string_lossy();
    let device = [r"\\?\", r"\\.\", "//?/", "//./"]
        .iter()
        .any(|prefix| text.starts_with(prefix));
    let unc = ["\\\\?\\UNC\\", "\\\\.\\UNC\\", "//?/UNC/", "//./UNC/"]
        .iter()
        .any(|prefix| {
            text.get(..prefix.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
        });
    if device && !unc {
        return Err(refuse(
            Problem::Untrusted,
            format!(
                "{shown} is a device path: choose the folder by its own full path, such as a folder under C:\\Program Files"
            ),
        ));
    }
    if text.starts_with(r"\\") || text.starts_with("//") {
        return Err(refuse(
            Problem::Untrusted,
            format!(
                "{shown} is a network path, and the co-op helper runs elevated, so Rubylight runs it only from a drive of this PC. {PROGRAM_FILES}"
            ),
        ));
    }
    if !folder.is_absolute() {
        return Err(refuse(
            Problem::Missing,
            format!("{shown} is no full folder path: choose the co-op helper's folder"),
        ));
    }
    if text
        .split(['\\', '/'])
        .any(|part| part == "." || part == "..")
    {
        return Err(refuse(
            Problem::Untrusted,
            format!("{shown} goes through . or ..: choose the folder by its own full path"),
        ));
    }
    let root = format!("{}\\", &text[..2]);
    if let Some(why) = drive_refusal(&shown.to_string(), &root, drive_kind(&root)) {
        return Err(why);
    }
    let volume = (checks.volume)(&root).with_context(|| format!("reading the drive {root}"))?;
    if volume.usb {
        return Err(refuse(
            Problem::Untrusted,
            format!(
                "{shown} is on a USB drive, and the co-op helper runs elevated, so Rubylight runs it only from a drive inside this PC. {PROGRAM_FILES}"
            ),
        ));
    }
    Ok((root, volume))
}

/// An error when `folder` is on another drive under `root` than when it was
/// first checked: a disk swapped in under the same letter could hold
/// anything its maker wanted, owners included. The first check remembers
/// the drive until the host restarts.
pub(super) fn same_drive(
    folder: &Path,
    root: &str,
    volume: &Volume,
    checks: &Checks,
) -> Result<()> {
    let key = folder.to_string_lossy().to_lowercase();
    let mut volumes = checks
        .volumes
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match volumes.get(&key) {
        Some(first) if first != volume => Err(refuse(
            Problem::Untrusted,
            format!(
                "The drive {root} that holds {} isn't the one Rubylight first checked it on ({}, serial number {:08X}; now {}, {:08X}). If you replaced that drive yourself, restart the PC",
                folder.display(),
                first.name,
                first.serial,
                volume.name,
                volume.serial
            ),
        )),
        Some(_) => Ok(()),
        None => {
            volumes.insert(key, volume.clone());
            Ok(())
        }
    }
}

/// A drive as Windows knows it: its volume's name (`\\?\Volume{...}\`,
/// which Windows gives each volume as it first sees it), the serial number
/// its format gave it, and whether it is attached by USB.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Volume {
    pub(super) name: String,
    pub(super) serial: u32,
    pub(super) usb: bool,
}

/// The drive at `root` (`C:\`).
pub(super) fn volume_of(root: &str) -> Result<Volume> {
    use windows::Win32::Storage::FileSystem::{
        GetVolumeInformationW, GetVolumeNameForVolumeMountPointW,
    };
    let wide: Vec<u16> = root.encode_utf16().chain(Some(0)).collect();
    let mut name = [0u16; 64];
    let mut serial = 0u32;
    // SAFETY: `wide` is NUL-terminated, and `name` and `serial` are
    // writable, all outliving the calls.
    unsafe {
        GetVolumeNameForVolumeMountPointW(windows::core::PCWSTR(wide.as_ptr()), &mut name)
            .context("reading the volume's name")?;
        GetVolumeInformationW(
            windows::core::PCWSTR(wide.as_ptr()),
            None,
            Some(&raw mut serial),
            None,
            None,
            None,
        )
        .context("reading the volume's serial number")?;
    }
    let len = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    Ok(Volume {
        name: String::from_utf16_lossy(&name[..len]),
        serial,
        usb: on_usb(root).context("reading the drive's bus")?,
    })
}

/// Whether the drive at `root` (`C:\`) is attached by USB, by the bus its
/// storage device names.
fn on_usb(root: &str) -> Result<bool> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Storage::FileSystem::{
        BusTypeUsb, CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
        OPEN_EXISTING,
    };
    use windows::Win32::System::IO::DeviceIoControl;
    use windows::Win32::System::Ioctl::{
        IOCTL_STORAGE_QUERY_PROPERTY, PropertyStandardQuery, STORAGE_DEVICE_DESCRIPTOR,
        STORAGE_PROPERTY_QUERY, StorageDeviceProperty,
    };
    let device: Vec<u16> = format!(r"\\.\{}", root.trim_end_matches('\\'))
        .encode_utf16()
        .chain(Some(0))
        .collect();
    // SAFETY: `device` is NUL-terminated and outlives the call. No access is
    // asked: a storage query needs none.
    let handle = unsafe {
        CreateFileW(
            windows::core::PCWSTR(device.as_ptr()),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
    }?;
    let query = STORAGE_PROPERTY_QUERY {
        PropertyId: StorageDeviceProperty,
        QueryType: PropertyStandardQuery,
        ..Default::default()
    };
    let mut descriptor = STORAGE_DEVICE_DESCRIPTOR::default();
    let mut returned = 0u32;
    // SAFETY: the handle is open until it is closed below, and each buffer
    // is the size passed and outlives the call. The descriptor's fixed part
    // fits; the strings after it are not asked for.
    let asked = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_STORAGE_QUERY_PROPERTY,
            Some((&raw const query).cast()),
            size_of::<STORAGE_PROPERTY_QUERY>() as u32,
            Some((&raw mut descriptor).cast()),
            size_of::<STORAGE_DEVICE_DESCRIPTOR>() as u32,
            Some(&raw mut returned),
            None,
        )
    };
    // SAFETY: opened above, closed once.
    let _ = unsafe { CloseHandle(handle) };
    asked?;
    Ok(descriptor.BusType == BusTypeUsb)
}

/// `path` as Windows names it to a person: without the `\\?\` that
/// GetFinalPathNameByHandleW puts first, `\` between names, and no `\` at
/// the end but a drive's root's.
pub(super) fn normal(path: &std::ffi::OsStr) -> String {
    let text = path.to_string_lossy().replace('/', "\\");
    let text = if let Some(share) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{share}")
    } else {
        text.strip_prefix(r"\\?\").unwrap_or(&text).to_string()
    };
    let trimmed = text.trim_end_matches('\\');
    if trimmed.len() == 2 && trimmed.ends_with(':') {
        format!("{trimmed}\\")
    } else {
        trimmed.to_string()
    }
}

/// GetDriveTypeW's answer for the drive `root` (`C:\`).
pub(super) fn drive_kind(root: &str) -> u32 {
    let wide: Vec<u16> = root.encode_utf16().chain(Some(0)).collect();
    // SAFETY: `wide` is NUL-terminated and outlives the call.
    unsafe {
        windows::Win32::Storage::FileSystem::GetDriveTypeW(windows::core::PCWSTR(wide.as_ptr()))
    }
}

/// Why Rubylight doesn't run Nucleus Co-op from `shown` on the drive `root`
/// of `kind` ([`drive_kind`]): only a fixed drive of this PC. A network
/// drive mapped for its user is no drive of the service's.
pub(super) fn drive_refusal(shown: &str, root: &str, kind: u32) -> Option<anyhow::Error> {
    let what = match kind {
        DRIVE_FIXED => return None,
        DRIVE_NO_ROOT_DIR => {
            return Some(refuse(
                Problem::Missing,
                format!(
                    "{shown} is on {root}, which is no drive of this PC to Rubylight (a network drive mapped for its user is none). {PROGRAM_FILES}"
                ),
            ));
        }
        DRIVE_REMOVABLE => "a removable drive",
        DRIVE_REMOTE => "a network drive",
        DRIVE_CDROM => "a CD or DVD drive",
        DRIVE_RAMDISK => "a RAM disk",
        _ => "a drive Windows doesn't know",
    };
    Some(refuse(
        Problem::Untrusted,
        format!(
            "{shown} is on {what}, and the co-op helper runs elevated, so Rubylight runs it only from a fixed drive of this PC. {PROGRAM_FILES}"
        ),
    ))
}

/// An error when the file at `path` carries the Mark of the Web, which
/// Windows gives what is unpacked from a downloaded zip that was not
/// unblocked: .NET refuses to load such a DLL, and Windows asks before it
/// starts such a program. The mark is never removed: unblocking is the
/// user's.
pub(super) fn blocked(path: &Path) -> Result<()> {
    use windows::Win32::Storage::FileSystem::{
        FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    let mut stream = path.as_os_str().to_owned();
    stream.push(":Zone.Identifier");
    match std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES.0)
        .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0)
        .open(&stream)
    {
        Ok(_) => Err(refuse(
            Problem::Blocked,
            format!(
                "{} is marked as downloaded from the internet. Unblock the zip before extracting: right-click the downloaded zip, choose Properties, tick Unblock, then unpack it again into a new folder",
                path.display()
            ),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => {
            Err(error).with_context(|| format!("reading whether {} was downloaded", path.display()))
        }
    }
}

/// What tells a file's bytes from another's without reading them: its size,
/// its last write time, its id on its volume and the time it last changed
/// (any write, rename or change of its attributes moves that, a new file
/// under its name has another id).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp {
    size: u64,
    written: i64,
    changed: i64,
    volume: u64,
    id: [u8; 16],
}

fn stamp(file: &std::fs::File) -> std::io::Result<Stamp> {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        FILE_BASIC_INFO, FILE_ID_INFO, FILE_STANDARD_INFO, FileBasicInfo, FileIdInfo,
        FileStandardInfo, GetFileInformationByHandleEx,
    };
    let handle = HANDLE(file.as_raw_handle());
    let mut basic = FILE_BASIC_INFO::default();
    let mut standard = FILE_STANDARD_INFO::default();
    let mut id = FILE_ID_INFO::default();
    // SAFETY: the handle is open for as long as `file` is, and each buffer
    // is a writable structure of the size passed.
    unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileBasicInfo,
            (&raw mut basic).cast(),
            size_of::<FILE_BASIC_INFO>() as u32,
        )
        .and_then(|()| {
            GetFileInformationByHandleEx(
                handle,
                FileStandardInfo,
                (&raw mut standard).cast(),
                size_of::<FILE_STANDARD_INFO>() as u32,
            )
        })
        .and_then(|()| {
            GetFileInformationByHandleEx(
                handle,
                FileIdInfo,
                (&raw mut id).cast(),
                size_of::<FILE_ID_INFO>() as u32,
            )
        })
    }
    .map_err(crate::user_folder::os_error)?;
    Ok(Stamp {
        size: standard.EndOfFile as u64,
        written: basic.LastWriteTime,
        changed: basic.ChangeTime,
        volume: id.VolumeSerialNumber,
        id: id.FileId.Identifier,
    })
}

/// Whether the file at `path`, open as `file`, is `shipped` by size and
/// SHA-256. The answer stands while its [`Stamp`] does: the console asks
/// every few seconds, and the release is 127 MB.
pub(super) fn as_shipped(path: &Path, file: &std::fs::File, shipped: &Shipped) -> Result<bool> {
    static HASHED: Mutex<Option<HashMap<PathBuf, (Stamp, bool)>>> = Mutex::new(None);
    let stamp_of =
        |file: &std::fs::File| stamp(file).with_context(|| format!("reading {}", path.display()));
    let seen = stamp_of(file)?;
    if seen.size != shipped.size {
        return Ok(false);
    }
    // Shares no writing, so it fails while anyone holds the file open to
    // write it: opened even when its answer is kept, which only saves
    // reading it again.
    let mut reader = crate::process::open_unfollowed(path, false)
        .with_context(|| format!("opening {}", path.display()))?;
    if stamp_of(&reader)? != seen {
        bail!(
            "{} changed while Rubylight read it: choose the folder again",
            path.display()
        );
    }
    let kept = HASHED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .and_then(|hashed| hashed.get(path).copied());
    if let Some((_, matches)) = kept.filter(|(stamp, _)| *stamp == seen) {
        return Ok(matches);
    }
    let matches = Manifest::matches(shipped, seen.size, &mut reader)
        .with_context(|| format!("reading {}", path.display()))?;
    HASHED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get_or_insert_default()
        .insert(path.to_path_buf(), (seen, matches));
    Ok(matches)
}
