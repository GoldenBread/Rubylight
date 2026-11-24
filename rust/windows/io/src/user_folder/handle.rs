//! Handle-based Windows file operations for transfer and shared folders.
use super::{Impersonation, PART_MARKER, UserFolderError, ZONE_IDENTIFIER, os_error};
use std::{
    ffi::{OsStr, OsString},
    fs::{File, Metadata, OpenOptions},
    io::{self, Write},
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        fs::MetadataExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::Path,
};
use windows::Win32::{
    Foundation::HANDLE,
    Globalization::{CSTR_EQUAL, CompareStringOrdinal},
    Storage::FileSystem::{
        FILE_ATTRIBUTE_NORMAL, FILE_BASIC_INFO, FILE_DISPOSITION_INFO, FILE_FLAGS_AND_ATTRIBUTES,
        FILE_GENERIC_READ, FILE_NAME_NORMALIZED, FILE_RENAME_INFO, FILE_SHARE_READ, FileBasicInfo,
        FileDispositionInfo, FileRenameInfo, GetFinalPathNameByHandleW, ReOpenFile,
        SetFileInformationByHandle,
    },
};

/// A plain file: not a folder or a link. Other reparse points, such as
/// compressed (WOF), deduplicated or cloud files, hold the file's own data.
pub(crate) fn plain(metadata: &Metadata) -> bool {
    metadata.is_file()
}

/// `path` made absolute with the `\\?\` prefix, so Win32 calls take it
/// beyond MAX_PATH (260 characters). Not NUL-terminated.
pub(crate) fn verbatim(path: &Path) -> io::Result<Vec<u16>> {
    let path: Vec<u16> = std::path::absolute(path)?
        .as_os_str()
        .encode_wide()
        .collect();
    let prefix = |prefix: &str| -> Vec<u16> { OsStr::new(prefix).encode_wide().collect() };
    let (keep, from) = if path.starts_with(&prefix(r"\\?\")) {
        return Ok(path);
    } else if path.starts_with(&prefix(r"\\.\")) {
        (prefix(r"\\?\"), 4)
    } else if path.starts_with(&prefix(r"\\")) {
        (prefix(r"\\?\UNC\"), 2)
    } else {
        (prefix(r"\\?\"), 0)
    };
    Ok([keep.as_slice(), &path[from..]].concat())
}

pub(crate) fn raw(file: &File) -> HANDLE {
    HANDLE(file.as_raw_handle())
}

/// Renames the open file to `target`, never over a file already there.
pub(super) fn rename_open(file: &File, target: &Path) -> io::Result<()> {
    let target = verbatim(target)?;
    let name_bytes = target.len() * size_of::<u16>();
    let bytes = std::mem::offset_of!(FILE_RENAME_INFO, FileName) + name_bytes + size_of::<u16>();
    // u64 keeps the structure's alignment; zeroed is ReplaceIfExists false,
    // no root folder and a NUL after the name.
    let mut buffer = vec![0u64; bytes.div_ceil(size_of::<u64>())];
    let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    // SAFETY: the buffer is aligned for FILE_RENAME_INFO and holds the
    // name it is followed by; Windows reads it during the call only.
    unsafe {
        (*info).FileNameLength = name_bytes as u32;
        std::ptr::copy_nonoverlapping(
            target.as_ptr(),
            (&raw mut (*info).FileName).cast::<u16>(),
            target.len(),
        );
        SetFileInformationByHandle(
            raw(file),
            FileRenameInfo,
            info.cast_const().cast(),
            (buffer.len() * size_of::<u64>()) as u32,
        )
    }
    .map_err(os_error)
}

/// Deletes the open file once its last handle closes.
pub(crate) fn delete_on_close(file: &File) -> io::Result<()> {
    let info = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: info is a live FILE_DISPOSITION_INFO of the size passed.
    unsafe {
        SetFileInformationByHandle(
            raw(file),
            FileDispositionInfo,
            (&raw const info).cast(),
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    }
    .map_err(os_error)
}

/// Clears [`PART_MARKER`] from the open file.
pub(super) fn unmark(file: &File) -> io::Result<()> {
    let attributes = file.metadata()?.file_attributes() & !PART_MARKER.0;
    let info = FILE_BASIC_INFO {
        // Times of zero stay as they are; no attributes at all is NORMAL.
        FileAttributes: if attributes == 0 {
            FILE_ATTRIBUTE_NORMAL.0
        } else {
            attributes
        },
        ..Default::default()
    };
    // SAFETY: info is a live FILE_BASIC_INFO of the size passed.
    unsafe {
        SetFileInformationByHandle(
            raw(file),
            FileBasicInfo,
            (&raw const info).cast(),
            size_of::<FILE_BASIC_INFO>() as u32,
        )
    }
    .map_err(os_error)
}

/// The open plain file opened again, as the user, without
/// FILE_FLAG_OPEN_REPARSE_POINT, so a compressed, deduplicated or cloud file
/// reads as its data; with its length.
pub(crate) fn reopen_for_data(
    token: Option<&OwnedHandle>,
    file: &File,
) -> Result<(File, u64), UserFolderError> {
    let reopened = {
        let _user = Impersonation::begin(token)?;
        // SAFETY: the handle is open for the call.
        unsafe {
            ReOpenFile(
                raw(file),
                FILE_GENERIC_READ.0,
                FILE_SHARE_READ,
                FILE_FLAGS_AND_ATTRIBUTES(0),
            )
        }
        .map_err(os_error)?
    };
    // SAFETY: ReOpenFile returned a new open handle only this File closes.
    let file = unsafe { File::from_raw_handle(reopened.0) };
    let len = file.metadata()?.len();
    Ok((file, len))
}

/// The name the open file has in its folder, as stored on disk: the long
/// name even when it was opened by its 8.3 short name.
pub(super) fn final_name(file: &File) -> io::Result<OsString> {
    let path = final_path(file)?;
    let path: Vec<u16> = path.encode_wide().collect();
    let start = path
        .iter()
        .rposition(|&unit| unit == u16::from(b'\\'))
        .map_or(0, |separator| separator + 1);
    Ok(OsString::from_wide(&path[start..]))
}

/// The whole path of the open file as Windows resolved it, with the `\\?\`
/// prefix: links followed and 8.3 short names replaced by the long ones.
pub(crate) fn final_path(file: &File) -> io::Result<OsString> {
    let mut path = vec![0u16; 512];
    loop {
        // SAFETY: the handle is open and path is a writable buffer.
        let len = unsafe { GetFinalPathNameByHandleW(raw(file), &mut path, FILE_NAME_NORMALIZED) }
            as usize;
        if len == 0 {
            return Err(io::Error::last_os_error());
        }
        if len < path.len() {
            path.truncate(len);
            break;
        }
        // Too small: len is the size needed, with the NUL.
        path.resize(len, 0);
    }
    Ok(OsString::from_wide(&path))
}

/// Whether `a` and `b` are one name or path to NTFS: equal but for case as
/// Windows maps it, code unit by code unit. Unlike Unicode lowercasing, the
/// Kelvin sign is not a `k` there, nor the Ohm sign an omega.
pub(crate) fn same_name(a: &OsStr, b: &OsStr) -> bool {
    let a: Vec<u16> = a.encode_wide().collect();
    let b: Vec<u16> = b.encode_wide().collect();
    // SAFETY: both are live slices, passed with their lengths.
    a == b || unsafe { CompareStringOrdinal(&a, &b, true) } == CSTR_EQUAL
}

/// Writes the Mark of the Web next to the data of the file at `path`.
pub(super) fn mark_of_the_web(path: &Path) -> io::Result<()> {
    let mut stream = path.as_os_str().to_owned();
    stream.push(":Zone.Identifier");
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(stream)?
        .write_all(ZONE_IDENTIFIER)
}
