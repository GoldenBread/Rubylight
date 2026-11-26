//! Listing, diffing, copying, and cleaning the signed-in user's Nucleus Co-op
//! copy against the host's program folder.
use super::{DIRECTORY, MARKER, REPARSE_POINT, as_user};
use anyhow::{Context, Result, bail, ensure};
use butterpollo_session::nucleus::install::{self as pinned, Part};
use std::collections::BTreeSet;
use std::os::windows::fs::MetadataExt;
use std::os::windows::io::OwnedHandle;
use std::path::{Path, PathBuf};

/// What of a Nucleus Co-op folder its user's copy holds ([`pinned::part`]).
#[derive(Debug, Default)]
pub(super) struct Listing {
    /// Program folders, below the folder.
    pub(super) folders: Vec<PathBuf>,
    /// Program files and their (size, last write time).
    pub(super) files: Vec<(PathBuf, (u64, u64))>,
    /// Files copied into a new copy only.
    pub(super) seeded: Vec<PathBuf>,
    /// State folders, made empty in a new copy.
    pub(super) state: Vec<PathBuf>,
    /// One sorted line per program folder (`name\`) and file
    /// (`name<TAB>size<TAB>time`): the copy's key and marker.
    pub(super) text: String,
}

impl Listing {
    /// `folder`'s listing, read as whoever runs it. A link anywhere it
    /// looks is an error: no link is copied or followed. State folders are
    /// not looked into (Nucleus links game files in `content`).
    pub(super) fn of(folder: &Path) -> Result<Self> {
        let mut listing = Self::default();
        let mut lines = Vec::new();
        let mut pending = vec![PathBuf::new()];
        while let Some(below) = pending.pop() {
            let path = folder.join(&below);
            let entries =
                std::fs::read_dir(&path).with_context(|| format!("listing {}", path.display()))?;
            for entry in entries {
                let entry = entry.with_context(|| format!("listing {}", path.display()))?;
                // From the listing itself: never follows a link.
                let metadata = entry
                    .metadata()
                    .with_context(|| format!("reading {}", entry.path().display()))?;
                let attributes = metadata.file_attributes();
                if attributes & REPARSE_POINT != 0 {
                    bail!(
                        "{} is a link, which Rubylight does not copy into a user's own co-op helper: remove it",
                        entry.path().display()
                    );
                }
                let name = below.join(entry.file_name());
                let Some(text) = name.to_str() else {
                    bail!("{} has a name that is not Unicode", entry.path().display());
                };
                let is_folder = attributes & DIRECTORY != 0;
                match (pinned::part(text, is_folder), is_folder) {
                    (Part::Program, true) => {
                        lines.push(format!("{text}\\"));
                        listing.folders.push(name.clone());
                        pending.push(name);
                    }
                    (Part::Program, false) => {
                        let stamp = (metadata.file_size(), metadata.last_write_time());
                        lines.push(format!("{text}\t{}\t{}", stamp.0, stamp.1));
                        listing.files.push((name, stamp));
                    }
                    (Part::Seeded, _) => listing.seeded.push(name),
                    (Part::State, true) => listing.state.push(name),
                    (Part::State, false) => {}
                }
            }
        }
        lines.sort();
        listing.text = lines.iter().map(|line| format!("{line}\n")).collect();
        Ok(listing)
    }
}

/// Why the copy at `copy` is not a whole copy of the program `text` lists.
pub(super) enum Differs {
    Missing,
    Changed(String),
}

/// None when the copy at `copy` is a whole copy of the program `text`
/// lists: its marker says so and its program files are those, with the
/// same sizes and times, and its `NucleusCoop.exe` has no other name.
pub(super) fn differs(copy: &Path, text: &str) -> Option<Differs> {
    // No more than it should hold: the copy is the user's to fill.
    let read = std::fs::File::open(copy.join(MARKER)).and_then(|file| {
        let mut marker = String::new();
        std::io::Read::read_to_string(
            &mut std::io::Read::take(file, text.len() as u64 + 1),
            &mut marker,
        )?;
        Ok(marker)
    });
    let marker = match read {
        Ok(marker) => marker,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Some(Differs::Missing);
        }
        Err(error) => return Some(Differs::Changed(format!("reading its {MARKER}: {error}"))),
    };
    if marker != text {
        return Some(Differs::Changed(format!(
            "its {MARKER} lists another program"
        )));
    }
    match Listing::of(copy) {
        Ok(found) if found.text == text => {}
        Ok(found) => return Some(Differs::Changed(difference(text, &found.text))),
        Err(error) => return Some(Differs::Changed(format!("{error:#}"))),
    }
    // [`super::hold`] refuses a program with another name: copied again, it has one.
    match names_of(&copy.join(pinned::EXECUTABLE)) {
        Ok(1) => None,
        Ok(names) => Some(Differs::Changed(format!("its program has {names} names"))),
        Err(error) => Some(Differs::Changed(format!("reading its program: {error}"))),
    }
}

/// How many names (hard links) the file at `path` has, never following a
/// link.
pub(super) fn names_of(path: &Path) -> std::io::Result<u32> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileInformationByHandle,
    };
    let file = std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES.0)
        .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: the handle is open for as long as `file` is, and `info` is
    // writable.
    unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }
        .map_err(crate::user_folder::os_error)?;
    Ok(info.nNumberOfLinks)
}

/// The first line of two listings that differs, for the log.
fn difference(expected: &str, found: &str) -> String {
    let expected: BTreeSet<&str> = expected.lines().collect();
    let found: BTreeSet<&str> = found.lines().collect();
    let name = |line: &str| line.split('\t').next().unwrap_or_default().to_string();
    if let Some(line) = expected.difference(&found).next() {
        format!("{} is missing or changed", name(line))
    } else if let Some(line) = found.difference(&expected).next() {
        format!("{} is not the host's", name(line))
    } else {
        "its listing is in another order".into()
    }
}

/// Copy `source`'s `listing` into the copy's folder `copy`, emptied first:
/// written as the user of `token`, each file read as the host. Its marker
/// is written last, so a copy is whole or [`differs`] says so.
pub(super) fn make(
    source: &Path,
    listing: &Listing,
    copy: &Path,
    token: Option<&OwnedHandle>,
) -> Result<()> {
    // What is there differs: it is not used.
    as_user(token, || empty(copy))??;
    let files = match fill(source, listing, copy, token) {
        Ok(files) => files,
        Err(error) => {
            if let Err(left) = as_user(token, || empty(copy)).and_then(|emptied| emptied) {
                tracing::warn!(copy = %copy.display(), error = format!("{left:#}"), "Co-op helper: a partial copy is left; the next start empties it");
            }
            return Err(error.context("copying the co-op helper for the signed-in user"));
        }
    };
    tracing::info!(copy = %copy.display(), files, "Co-op helper: the signed-in user's own copy is made");
    Ok(())
}

/// Fill the empty folder `staging` with `listing`; the number of files.
fn fill(
    source: &Path,
    listing: &Listing,
    staging: &Path,
    token: Option<&OwnedHandle>,
) -> Result<usize> {
    as_user(token, || -> Result<()> {
        for folder in listing.folders.iter().chain(&listing.state) {
            let path = staging.join(folder);
            std::fs::create_dir_all(&path)
                .with_context(|| format!("creating {}", path.display()))?;
        }
        Ok(())
    })??;
    let program = listing
        .files
        .iter()
        .map(|(name, stamp)| (name, Some(*stamp)));
    let seeded = listing.seeded.iter().map(|name| (name, None));
    let mut files = 0;
    let mut buffer = vec![0; COPY_CHUNK];
    for (name, stamp) in program.chain(seeded) {
        copy_file(
            &source.join(name),
            &staging.join(name),
            stamp,
            token,
            &mut buffer,
        )?;
        files += 1;
    }
    as_user(token, || {
        std::fs::write(staging.join(MARKER), &listing.text)
    })?
    .with_context(|| format!("writing {}", staging.join(MARKER).display()))?;
    Ok(files)
}

/// How much of a file [`copy_file`] reads and writes at a time:
/// `std::io::copy` moves 8 KiB per call, which took about twice as long for
/// a folder of 127 MB in 464 files.
pub(super) const COPY_CHUNK: usize = 1 << 20;

/// Copy the file `from`, opened as the host without following a link, to
/// the new file `to`, written as the user of `token` through `buffer`, with
/// `from`'s last write time. A program file must still have its listed
/// `stamp`.
fn copy_file(
    from: &Path,
    to: &Path,
    stamp: Option<(u64, u64)>,
    token: Option<&OwnedHandle>,
    buffer: &mut [u8],
) -> Result<()> {
    // Shares no writing while it is open.
    let mut source = crate::process::open_unfollowed(from, false)
        .with_context(|| format!("opening {}", from.display()))?;
    let metadata = source
        .metadata()
        .with_context(|| format!("reading {}", from.display()))?;
    ensure!(
        metadata.file_attributes() & (REPARSE_POINT | DIRECTORY) == 0,
        "{} is a link or a folder, not a file, so Rubylight does not copy it",
        from.display()
    );
    if let Some(stamp) = stamp {
        ensure!(
            (metadata.file_size(), metadata.last_write_time()) == stamp,
            "{} changed while Rubylight copied it: start the stream again",
            from.display()
        );
    }
    let modified = metadata
        .modified()
        .with_context(|| format!("reading {}", from.display()))?;
    as_user(token, || -> Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(to)
            .with_context(|| format!("creating {}", to.display()))?;
        copy_through(&mut source, &mut file, buffer)
            .with_context(|| format!("copying {}", from.display()))?;
        // Last: a write after it would move it again.
        file.set_modified(modified)
            .with_context(|| format!("writing {}", to.display()))
    })?
}

/// Copy what is left of `from` to `to`, `buffer` at a time.
pub(super) fn copy_through(
    from: &mut impl std::io::Read,
    to: &mut impl std::io::Write,
    buffer: &mut [u8],
) -> std::io::Result<()> {
    loop {
        match from.read(buffer) {
            Ok(0) => return Ok(()),
            Ok(read) => to.write_all(&buffer[..read])?,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
}

/// Remove what else is in the user's folder: a copy of another program
/// (`key` is this one's). What is in it goes as the user of `token`; the
/// folder itself then as the host, which may remove one it made (the user
/// may not). The user's folder is held by the caller and checked again to
/// be no link here; removing a link in it removes the link. One still in
/// use stays until the next time, with a warning.
pub(super) fn remove_others(folder: &Path, key: &str, token: Option<&OwnedHandle>) -> Result<()> {
    let attributes = std::fs::symlink_metadata(folder)
        .with_context(|| format!("reading {}", folder.display()))?
        .file_attributes();
    ensure!(
        attributes & REPARSE_POINT == 0 && attributes & DIRECTORY != 0,
        "{} is a link to elsewhere, so Rubylight removes nothing in it: an administrator of this PC can delete it, then start the stream again",
        folder.display()
    );
    let entries = as_user(token, || -> Result<Vec<(PathBuf, bool)>> {
        let mut entries = Vec::new();
        for entry in
            std::fs::read_dir(folder).with_context(|| format!("listing {}", folder.display()))?
        {
            let entry = entry.with_context(|| format!("listing {}", folder.display()))?;
            if entry.file_name() != key {
                // A link is no folder here: it goes itself.
                entries.push((
                    entry.path(),
                    entry.file_type().is_ok_and(|kind| kind.is_dir()),
                ));
            }
        }
        Ok(entries)
    })??;
    for (path, is_folder) in entries {
        let removed = if is_folder {
            as_user(token, || empty(&path))
                .and_then(|emptied| emptied)
                .and_then(|()| {
                    std::fs::remove_dir(&path)
                        .with_context(|| format!("removing {}", path.display()))
                })
        } else {
            as_user(token, || remove_entry(&path)).and_then(|removed| removed)
        };
        match removed {
            Ok(()) => {
                tracing::info!(path = %path.display(), "Co-op helper: another copy of the signed-in user's removed")
            }
            Err(error) => {
                tracing::warn!(path = %path.display(), error = format!("{error:#}"), "Co-op helper: another copy of the signed-in user's was not fully removed; it is tried again next time, and an administrator of this PC can delete it")
            }
        }
    }
    Ok(())
}

/// Remove everything in `folder`, never following a link: a link itself
/// goes. What can't be removed stays, and the first such is the error:
/// the rest goes all the same (a copy set aside for one stuck file, #56).
pub(super) fn empty(folder: &Path) -> Result<()> {
    let mut first = None;
    for entry in
        std::fs::read_dir(folder).with_context(|| format!("listing {}", folder.display()))?
    {
        let entry = entry.with_context(|| format!("listing {}", folder.display()))?;
        if let Err(error) = remove_entry(&entry.path()) {
            first.get_or_insert(error);
        }
    }
    first.map_or(Ok(()), Err)
}

/// Remove the file, folder or link at `path`, if there: a link itself, never
/// what it leads to.
pub(super) fn remove_entry(path: &Path) -> Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    let removed = if metadata.is_dir() {
        std::fs::remove_dir_all(path)
    } else if metadata.file_attributes() & DIRECTORY != 0 {
        // A link to a folder goes as a folder; a file's refusal is its own
        // error, not "The directory name is invalid" (#56).
        std::fs::remove_dir(path)
    } else {
        std::fs::remove_file(path)
    };
    match removed {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            Err(error).with_context(|| format!("removing {}", path.display()))
        }
        _ => Ok(()),
    }
}
