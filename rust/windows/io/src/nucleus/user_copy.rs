//! The signed-in user's own copy of Nucleus Co-op, which Rubylight's own
//! one-player handler runs with the user's standard rights.
//!
//! It is never a folder an elevated Nucleus runs from, and never shares one
//! with the host's copy: `<data>\nucleus-user\<SID>\<key>`, outside
//! `C:\Users` (Nucleus warns against running from there) and removed with
//! the host's data. It is copied from the host's Nucleus folder (the pinned
//! release's folder the user chose, which only SYSTEM and Administrators
//! may change, or an earlier Rubylight's install), program files only, and
//! `<key>` changes with them.
//! SYSTEM makes `nucleus-user` (only SYSTEM and Administrators may open
//! it), `<SID>` (its user may list it, nothing more) and `<key>` (its user
//! may add to it and change what is in it, never its permissions, nor
//! rename or delete it), each with its permissions from the start, and
//! checks them when they exist, never following a link: one owned or
//! permitted otherwise is refused, never rebuilt. Each is held open shared
//! for reading only while SYSTEM works below it, so the user can't make it a
//! link meanwhile. SYSTEM also removes an emptied folder from `<SID>`, after
//! checking `<SID>` is no link, renames a copy with a file the user can't
//! change any more aside within `<SID>` (a link in its place moves itself,
//! #56), and opens the copy to start Nucleus from it
//! ([`hold`]): `<key>` and its `NucleusCoop.exe`, checked by handle, the
//! program by the path Windows opened it at, and both held so neither can
//! be swapped for a link until it runs.
//! Everything else below `<SID>` is done as that user: writing the copy,
//! checking it, emptying an older one, the handlers, its game list, the
//! profile and settings. What the user can change there runs with their
//! rights only.
use super::{files, install};
use anyhow::{Context, Result, bail, ensure};
use butterpollo_session::nucleus::generic::{self, Setup};
use butterpollo_session::nucleus::handler::Handler;
use butterpollo_session::nucleus::install as pinned;
use butterpollo_session::nucleus::profile;
use std::os::windows::fs::MetadataExt;
use std::os::windows::io::OwnedHandle;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

mod security;
mod sync;

pub use security::{Held, hold};
#[cfg(test)]
use security::{hold_with, in_folder, open_held, open_program, verdict};
use security::{made, secured};
#[cfg(test)]
use sync::{COPY_CHUNK, copy_through, names_of, remove_entry};
use sync::{Differs, Listing, differs, empty, make, remove_others};

/// The folder holding every user's copy: SYSTEM and Administrators own and
/// change it, and nobody else may open it. A user reaches their own folder
/// below it with the bypass traverse checking every account has, never
/// learning whose others are there.
const ROOT: &str = "O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)";

/// The file a whole copy holds: the listing of the program it was copied
/// from ([`Listing::text`]), written last. Its name is the host's, so it is
/// no part of the listing itself.
const MARKER: &str = "Rubylight-copy.txt";

const REPARSE_POINT: u32 = 0x400;
const DIRECTORY: u32 = 0x10;

/// A user's folder (`<SID>`): SYSTEM and Administrators own and change it;
/// the user may list it and pass through it, nothing more. Adding to a
/// folder is writing its data, which is enough to make an empty one a link.
fn sid_sddl(sid: &str) -> String {
    format!("O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;;0x1200a9;;;{sid})")
}

/// A user's copy (`<key>`): SYSTEM and Administrators own and change it;
/// the user may list it and add to it, and change and delete what is in it,
/// never its permissions, its owner or the folder itself (deleting or
/// renaming it). Emptied, they could make it a link: [`hold`] keeps them
/// from that while Nucleus starts.
fn user_sddl(sid: &str) -> String {
    format!(
        "O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;;0x1200af;;;{sid})(A;OICIIO;0x1301bf;;;{sid})"
    )
}

/// The signed-in user's token, which their copy is written and run as. An
/// error without one: the host never writes their copy as itself.
pub fn user() -> Result<OwnedHandle> {
    match crate::user_folder::user_token() {
        Ok(Some(token)) => Ok(token),
        Ok(None) => bail!(
            "Nucleus Co-op's copy for the signed-in user needs the host to run as the Rubylight Windows service"
        ),
        Err(error) => Err(anyhow::anyhow!("{error}"))
            .context("finding the signed-in user, whose own Nucleus Co-op copy runs"),
    }
}

/// The SID of the user of `token`.
pub fn sid(token: &OwnedHandle) -> Result<String> {
    crate::process::token_user_sid(token)
}

/// Runs `work` as the user of `token` (None: as the host, in tests).
pub fn as_user<T>(token: Option<&OwnedHandle>, work: impl FnOnce() -> T) -> Result<T> {
    let _user =
        crate::user_folder::Impersonation::begin(token).context("acting as the signed-in user")?;
    Ok(work())
}

/// What differs in tests: a test process can't give a folder to the
/// Administrators group.
struct Steps {
    create: fn(&Path, &str) -> std::io::Result<()>,
    check: fn(&Path, &[&str]) -> Result<std::fs::File>,
}
impl Steps {
    const PINNED: Self = Self {
        create: crate::process::create_folder,
        check: secured,
    };
}

/// The user `sid`'s own copy of the host's Nucleus folder `source` (read as
/// the host: only SYSTEM and Administrators may change it), made again when
/// it is missing or differs from `source`'s program files by name, size or
/// time, or holds a file Rubylight changes that the user can't ([`stuck`]),
/// with every other copy and leftover in their folder removed.
/// `token` is theirs.
pub fn ensure(data: &Path, source: &Path, sid: &str, token: &OwnedHandle) -> Result<PathBuf> {
    ensure_with(data, source, sid, Some(token), &Steps::PINNED)
}

fn ensure_with(
    data: &Path,
    source: &Path,
    sid: &str,
    token: Option<&OwnedHandle>,
    steps: &Steps,
) -> Result<PathBuf> {
    static MAKING: Mutex<()> = Mutex::new(());
    let _one = MAKING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let folder = pinned::user_folder(data, sid)
        .with_context(|| format!("{sid:?} is not a Windows account's SID"))?;
    let listing = Listing::of(source).with_context(|| {
        format!(
            "reading the Nucleus Co-op folder {} for the signed-in user's own copy",
            source.display()
        )
    })?;
    ensure!(
        listing.files.iter().any(|(name, _)| name
            .to_str()
            .is_some_and(|name| name.eq_ignore_ascii_case(pinned::EXECUTABLE))),
        "{} holds no {}",
        source.display(),
        pinned::EXECUTABLE
    );
    let root = pinned::user_root(data);
    // Each held open ([`security::open_held`]): none can be moved, replaced or made a
    // link while the host works below it.
    let _root = made(&root, ROOT, &[], steps)?;
    let mut held_folder = made(&folder, &sid_sddl(sid), &[], steps)?;
    let key = pinned::copy_key(&listing.text);
    let copy = folder.join(&key);
    let mut held = made(&copy, &user_sddl(sid), &[sid], steps)?;
    // One file the user can't change any more would refuse every launch
    // (#56): a copy that holds one, or can't be emptied to be made again,
    // goes aside, and a new one is made.
    let differs = as_user(token, || differs(&copy, &listing.text))?;
    match &differs {
        None => {}
        Some(Differs::Missing) => {
            tracing::info!(copy = %copy.display(), source = %source.display(), "Nucleus Co-op: copying it for the signed-in user");
        }
        Some(Differs::Changed(why)) => {
            tracing::warn!(copy = %copy.display(), source = %source.display(), why, "Nucleus Co-op: the signed-in user's copy is not the host's; copying it again");
        }
    }
    let stuck = match &differs {
        None => as_user(token, || stuck(&copy))?,
        Some(_) => as_user(token, || empty(&copy))?
            .err()
            .map(|why| format!("{why:#}")),
    };
    if let Some(why) = &stuck {
        tracing::warn!(copy = %copy.display(), why, "Nucleus Co-op: the signed-in user can't change a file in their copy any more; it is set aside and copied again");
        // A rename writes the folder it is in: both are let go for it, and
        // checked again. The user can't add to `<SID>`.
        drop((held, held_folder));
        set_aside(&folder, &key)?;
        held_folder = made(&folder, &sid_sddl(sid), &[], steps)?;
        held = made(&copy, &user_sddl(sid), &[sid], steps)?;
    }
    if stuck.is_some() || differs.is_some() {
        make(source, &listing, &copy, token)?;
    }
    as_user(token, || seed_settings(&copy))??;
    remove_others(&folder, &key, token)?;
    drop((held, held_folder));
    Ok(copy)
}

/// Why the user can't change what in their copy at `copy` Rubylight
/// changes or removes as them before Nucleus Co-op starts: the copy and its
/// `handlers`, `game profiles` and `content` folders, which it lists and
/// adds to, and the handlers, the profiles of its own handlers' games, the
/// game list and the settings, each with the file it is written through.
/// A read-only, hidden or system attribute on one of those files, which
/// Rubylight's writes and removals refuse, is cleared: the user may. Done
/// as the user, never through a link. None when they can change them all.
fn stuck(copy: &Path) -> Option<String> {
    let list = files::list(copy);
    // The folders Rubylight adds to, then the files it changes.
    let mut folders = vec![
        copy.to_path_buf(),
        copy.join("handlers"),
        copy.join("game profiles"),
    ];
    folders.extend(list.parent().map(Path::to_path_buf));
    let settings = copy.join("Settings.ini");
    let mut files = vec![
        list.with_extension("json.rubylight"),
        list,
        settings.with_extension("rubylight.tmp"),
        settings,
    ];
    let listed = |folder: &Path| -> Vec<(PathBuf, std::fs::FileType)> {
        std::fs::read_dir(folder)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| Some((entry.path(), entry.file_type().ok()?)))
            .collect()
    };
    for (path, kind) in listed(&copy.join("handlers")) {
        if kind.is_file()
            && path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("js"))
        {
            files.push(path);
        }
    }
    for (game, kind) in listed(&copy.join("game profiles")) {
        let ours = game
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.get(..generic::PREFIX.len()))
            .is_some_and(|start| start.eq_ignore_ascii_case(generic::PREFIX));
        if kind.is_dir() && ours {
            files.extend(
                listed(&game)
                    .into_iter()
                    .filter(|(_, kind)| kind.is_file())
                    .map(|(path, _)| path),
            );
            folders.push(game);
        }
    }
    let folders = folders.iter().map(|path| (path, true));
    let files = files.iter().map(|path| (path, false));
    folders
        .chain(files)
        .find_map(|(path, folder)| unchangeable(path, folder))
}

/// The read-only, hidden and system attributes: Windows refuses to write a
/// read-only file or delete it, and to replace a hidden or system one
/// (`CREATE_ALWAYS`).
const KEPT: u32 = 0x1 | 0x2 | 0x4;

/// Why the user can't list and add to the `folder` at `path`, or write and
/// delete the file there, if it is there. A file's [`KEPT`] attributes are
/// cleared first, through a handle that never follows a link.
fn unchangeable(path: &Path, folder: bool) -> Option<String> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows::Win32::Storage::FileSystem::{
        DELETE, FILE_APPEND_DATA, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA,
    };
    let open = |access: u32, flags: u32| {
        std::fs::OpenOptions::new()
            .access_mode(access)
            .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0)
            .custom_flags(flags | FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(path)
    };
    let refused = |error: std::io::Error| match error.kind() {
        std::io::ErrorKind::PermissionDenied => Some(format!("{}: {error}", path.display())),
        // Gone, or in use for now: neither stays.
        _ => None,
    };
    if folder {
        // A folder's FILE_ADD_FILE and FILE_ADD_SUBDIRECTORY are a file's
        // FILE_WRITE_DATA and FILE_APPEND_DATA.
        let access = FILE_LIST_DIRECTORY.0 | FILE_WRITE_DATA.0 | FILE_APPEND_DATA.0;
        return open(access, FILE_FLAG_BACKUP_SEMANTICS.0)
            .err()
            .and_then(refused);
    }
    let attributes = match open(FILE_READ_ATTRIBUTES.0 | FILE_WRITE_ATTRIBUTES.0, 0) {
        Ok(file) => file.metadata().map(|metadata| metadata.file_attributes()),
        Err(error) => return refused(error),
    };
    match attributes {
        Ok(attributes) if attributes & KEPT != 0 => {
            if let Err(error) = clear_kept(path, attributes) {
                return Some(format!(
                    "{} is read-only, hidden or a system file (attributes 0x{attributes:x}), which the user could not change: {error}",
                    path.display()
                ));
            }
            tracing::info!(file = %path.display(), attributes = format!("0x{attributes:x}"), "Nucleus Co-op: a read-only, hidden or system file in the signed-in user's copy made plain");
        }
        Ok(_) => {}
        Err(error) => return refused(error),
    }
    open(FILE_WRITE_DATA.0 | DELETE.0, 0)
        .err()
        .and_then(refused)
}

/// Clear the [`KEPT`] attributes of the file at `path`, which has
/// `attributes`, by a handle that never follows a link.
fn clear_kept(path: &Path, attributes: u32) -> std::io::Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_NORMAL, FILE_BASIC_INFO, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_WRITE_ATTRIBUTES, FileBasicInfo,
        SetFileInformationByHandle,
    };
    let file = std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES.0 | FILE_WRITE_ATTRIBUTES.0)
        .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)?;
    let left = attributes & !KEPT;
    // Zero times leave them as they are; no attributes at all is NORMAL.
    let info = FILE_BASIC_INFO {
        FileAttributes: if left == 0 {
            FILE_ATTRIBUTE_NORMAL.0
        } else {
            left
        },
        ..Default::default()
    };
    // SAFETY: the handle is open for as long as `file` is, and `info` is a
    // FILE_BASIC_INFO of its size.
    unsafe {
        SetFileInformationByHandle(
            HANDLE(file.as_raw_handle()),
            FileBasicInfo,
            std::ptr::from_ref(&info).cast(),
            std::mem::size_of::<FILE_BASIC_INFO>() as u32,
        )
    }
    .map_err(crate::user_folder::os_error)
}

/// Move the copy `key` in the user's folder `folder` aside, as the host:
/// the user may not rename it themselves. A link in its place moves
/// itself; `folder` was checked by the caller, and the user can neither add
/// to it nor move it, so no name in it leads elsewhere. [`remove_others`]
/// removes the copy like any other, and warns while a file in it can't be
/// removed.
fn set_aside(folder: &Path, key: &str) -> Result<()> {
    let from = folder.join(key);
    let to = (1..=64)
        .map(|n| folder.join(format!("{key}.aside-{n}")))
        .find(|to| std::fs::symlink_metadata(to).is_err())
        .with_context(|| {
            format!(
                "{} holds too many copies set aside: an administrator of this PC can delete them",
                folder.display()
            )
        })?;
    std::fs::rename(&from, &to).with_context(|| {
        format!(
            "setting {} aside, as the signed-in user can't change a file in it: an administrator of this PC can delete it, then start the stream again",
            from.display()
        )
    })?;
    tracing::info!(from = %from.display(), to = %to.display(), "Nucleus Co-op: the signed-in user's copy set aside");
    Ok(())
}

/// Give the copy at `copy` Nucleus Co-op's own settings when it has none
/// ([`profile::default_settings_ini`]): never the host's, which hold its
/// user's nicknames, Steam IDs and helper accounts' password. What the
/// user's Nucleus changed in them stays.
fn seed_settings(copy: &Path) -> Result<()> {
    use std::io::Write;
    let path = copy.join("Settings.ini");
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(()),
        Err(error) => return Err(error).with_context(|| format!("creating {}", path.display())),
    };
    let written = file.write_all(&profile::default_settings_ini());
    if let Err(error) = written {
        drop(file);
        // Not left part-written: Nucleus refuses to start on a missing key.
        let _ = std::fs::remove_file(&path);
        return Err(error).with_context(|| format!("writing {}", path.display()));
    }
    Ok(())
}

/// Write Rubylight's handler for `setup` into the host's Nucleus at
/// `folder` with its game in that one's list ([`files::write_own`]), then
/// give the user's copy at `copy` the host's handlers and add the game to
/// the copy's own list ([`files::list`], the one its Nucleus reads), as the
/// user of `token`. A copy's list Rubylight could not add to refuses the
/// launch before anything is written. The host's sweep removes the
/// handler with its game ([`files::remove_own`], then [`remove_game`]),
/// and the next mirror the copy's handler.
pub fn write_own(
    folder: &Path,
    copy: &Path,
    setup: &Setup,
    token: Option<&OwnedHandle>,
) -> Result<Handler> {
    as_user(token, || files::check_list(copy, Some(setup), &[]))??;
    let handler = files::write_own(folder, setup)?;
    let handlers = mirror_handlers(folder, copy, &handler.file, token)?;
    tracing::debug!(handlers, copy = %copy.display(), "Nucleus Co-op: the user's own copy has the host's handlers");
    as_user(token, || files::add_game(copy, setup))??;
    Ok(handler)
}

/// Remove the game `guid` of a handler of Rubylight's the host's sweep
/// removed from the list of the user's copy at `copy`, as the user of
/// `token`.
pub fn remove_game(copy: &Path, guid: &str, token: Option<&OwnedHandle>) -> Result<()> {
    as_user(token, || files::remove_game(copy, guid))?
}

/// Make the handlers of the user's copy at `copy` the host's hub handlers
/// at `folder` and Rubylight's own handler `own` for the game launched:
/// Nucleus drops from its list every game whose handler it doesn't have,
/// and a hub handler added in the host's Nucleus works in the copy too.
/// Rubylight's handlers for other games (another account's among them, with
/// their games' paths) stay out. The host's are read as the host from its
/// own folder and written as the user of `token`; a handler only the copy
/// had goes (Rubylight's own once the host's sweep removed it, or another
/// game was launched), so the copy runs only handlers the host's has.
/// Returns how many there are.
fn mirror_handlers(
    folder: &Path,
    copy: &Path,
    own: &str,
    token: Option<&OwnedHandle>,
) -> Result<usize> {
    let source = folder.join("handlers");
    let mut handlers = Vec::new();
    let entries = match std::fs::read_dir(&source) {
        Ok(entries) => entries.collect::<std::io::Result<Vec<_>>>()?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error).with_context(|| format!("reading {}", source.display())),
    };
    for entry in entries {
        if !handler_file(&entry) {
            continue;
        }
        let bytes = std::fs::read(entry.path())
            .with_context(|| format!("reading {}", entry.path().display()))?;
        let launched = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.eq_ignore_ascii_case(own));
        if !launched && generic::owns(&String::from_utf8_lossy(&bytes)) {
            continue;
        }
        handlers.push((entry.file_name(), bytes));
    }
    let count = handlers.len();
    as_user(token, || write_handlers(&copy.join("handlers"), &handlers))??;
    Ok(count)
}

/// A handler file (`*.js`, not a link) in a handlers folder.
fn handler_file(entry: &std::fs::DirEntry) -> bool {
    entry.file_type().is_ok_and(|kind| kind.is_file())
        && Path::new(&entry.file_name())
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("js"))
}

fn write_handlers(target: &Path, handlers: &[(std::ffi::OsString, Vec<u8>)]) -> Result<()> {
    std::fs::create_dir_all(target).with_context(|| format!("creating {}", target.display()))?;
    for entry in
        std::fs::read_dir(target).with_context(|| format!("reading {}", target.display()))?
    {
        let entry = entry.with_context(|| format!("reading {}", target.display()))?;
        let name = entry.file_name();
        let js = Path::new(&name)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("js"));
        let kept = handlers
            .iter()
            .any(|(kept, _)| kept.eq_ignore_ascii_case(&name));
        if js && !kept && !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            std::fs::remove_file(entry.path())
                .with_context(|| format!("removing {}", entry.path().display()))?;
        }
    }
    for (name, bytes) in handlers {
        let path = target.join(name);
        // No more of the user's file than the host's handler holds.
        let mut old = Vec::new();
        let same = std::fs::File::open(&path)
            .and_then(|file| {
                std::io::Read::read_to_end(
                    &mut std::io::Read::take(file, bytes.len() as u64 + 1),
                    &mut old,
                )
            })
            .is_ok_and(|_| old == *bytes);
        if same {
            continue;
        }
        std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
