//! Shared folder wire types and pure path/name helpers.

use super::Error;
use serde::Deserialize;
/// The suffix of a transfer in progress, on the host and here.
pub const PART_SUFFIX: &str = ".part";
/// The most names a folder path has (the host's limit).
pub const MAX_FOLDER_DEPTH: usize = 64;

/// What a grant allows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FolderAccess {
    Read,
    /// Read and write.
    Write,
    /// A value from a newer host: treat it as `Read`.
    #[serde(other)]
    Unknown,
}

/// A folder the PC shares with this device.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct SharedFolder {
    /// The id the routes take; stable across renames.
    pub id: String,
    /// What the console calls the folder.
    pub name: String,
    pub access: FolderAccess,
}

impl SharedFolder {
    /// Whether this device may upload into the folder.
    pub fn writable(&self) -> bool {
        self.access == FolderAccess::Write
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    File,
    Dir,
    /// A kind from a newer host: neither opened nor downloaded.
    #[serde(other)]
    Unknown,
}

/// One name in a listed directory.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct FolderEntry {
    pub name: String,
    pub kind: EntryKind,
    /// Bytes; 0 for a directory.
    pub size: u64,
    pub modified_unix_ms: u64,
}

/// A directory of a shared folder: folders first, then files, each by name.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct FolderEntries {
    /// The listed directory below the folder, `/`-separated; empty for the folder itself.
    pub path: String,
    pub entries: Vec<FolderEntry>,
}

/// `name` below the directory `dir` (empty for the folder itself).
pub fn child_path(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

/// The directory holding `path`; empty for a name in the folder itself.
pub fn parent_path(path: &str) -> &str {
    path.rfind('/').map_or("", |slash| &path[..slash])
}

/// The last name of `path`.
pub fn file_name(path: &str) -> &str {
    path.rfind('/').map_or(path, |slash| &path[slash + 1..])
}

/// Whether `name` ends in [`PART_SUFFIX`], in any case, as the host checks it.
pub fn has_part_suffix(name: &str) -> bool {
    name.get(name.len().saturating_sub(PART_SUFFIX.len())..)
        .is_some_and(|end| end.eq_ignore_ascii_case(PART_SUFFIX))
}

/// The name a copy takes on its `n`th collision, as the host and Explorer number them:
/// `name (n).ext`; `n == 0` is the name itself. A leading dot starts a name, not an extension.
pub fn collision_name(name: &str, n: u32) -> String {
    if n == 0 {
        return name.to_owned();
    }
    match name.rfind('.') {
        Some(dot) if dot > 0 => format!("{} ({n}){}", &name[..dot], &name[dot..]),
        _ => format!("{name} ({n})"),
    }
}

/// A folder id as the host's setting allows it: 1 to 64 ASCII letters, digits, `-` or `_`. It
/// is a segment of the URL path, so nothing else is sent.
pub fn check_id(id: &str) -> Result<(), Error> {
    let valid = (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if valid {
        Ok(())
    } else {
        Err(Error::InvalidArgument(format!(
            "not a shared folder id: {id:?}"
        )))
    }
}

/// A path the host could read: at most [`MAX_FOLDER_DEPTH`] names, none of them empty.
pub fn check_path(path: &str) -> Result<(), Error> {
    if path.is_empty() {
        return Ok(());
    }
    let names = path.split('/');
    if names.clone().count() > MAX_FOLDER_DEPTH || names.clone().any(str::is_empty) {
        return Err(Error::InvalidArgument(format!(
            "not a shared folder path: {path:?}"
        )));
    }
    Ok(())
}

