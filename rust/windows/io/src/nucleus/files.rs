//! The files a host session reads and writes in a Nucleus Co-op folder: the
//! handlers and the user's added games, `Settings.ini`, and the game profile
//! titled "Rubylight" that starts the session's players.
use super::layers::{LOCK_WAIT, Lock};
use anyhow::{Context, Result, bail, ensure};
use butterpollo_session::nucleus::generic::{self, Inject, Setup};
use butterpollo_session::nucleus::handler::{self, Catalog, Handler};
use butterpollo_session::nucleus::profile::{self, Player};
use butterpollo_session::nucleus::window::Rect;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

mod settings_profiles;

#[cfg(test)]
use settings_profiles::settings_journal;
pub use settings_profiles::{
    SettingsGuard, index_pads, meta_info, restore_settings_left, write_copy_settings,
    write_profile, write_settings,
};

/// The handlers and the games added in the Nucleus Co-op at `folder`, from
/// its game list [`list`], the one Nucleus Co-op 2.4.2 reads (#59). A
/// handler that cannot be read is left out with a log line; an unreadable
/// game list is an error. With `signed_in`, [`Catalog::user`] is the
/// signed-in user, whose sweep may remove the handlers written for them.
/// Every folder the host runs is that release's, the chosen one as well as
/// an earlier install, so its own list is the only one.
pub fn catalog(folder: &Path, signed_in: bool) -> Result<Catalog> {
    let mut catalog = Catalog {
        user: signed_in.then(crate::process::user_sid).flatten(),
        ..Catalog::default()
    };
    catalog.handlers = handlers(folder)?;
    let path = list(folder);
    if let Some(text) = read_list(&path)? {
        let games = handler::user_games(&text)
            .map_err(anyhow::Error::msg)
            .with_context(|| path.display().to_string())?;
        for game in games {
            if !catalog.games.contains(&game) {
                catalog.games.push(game);
            }
        }
    }
    Ok(catalog)
}

/// The handlers in the Nucleus Co-op at `folder`; one that cannot be read is
/// left out with a log line.
fn handlers(folder: &Path) -> Result<Vec<Handler>> {
    let mut found = Vec::new();
    let handlers = folder.join("handlers");
    let entries = match std::fs::read_dir(&handlers) {
        Ok(entries) => entries.collect::<std::io::Result<Vec<_>>>()?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error).with_context(|| format!("reading {}", handlers.display())),
    };
    let mut files: Vec<PathBuf> = entries
        .into_iter()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("js"))
                && path.is_file()
        })
        .collect();
    files.sort();
    for path in files {
        let file = path.file_name().unwrap_or_default().to_string_lossy();
        let text = match read_handler_text(&path) {
            Ok(text) => text,
            Err(error) => {
                tracing::warn!(%file, error = %format!("{error:#}"), "Nucleus Co-op handler could not be read; skipped");
                continue;
            }
        };
        match handler::parse(&file, &text) {
            Ok(handler) => found.push(handler),
            Err(error) => tracing::info!(%file, %error, "Nucleus Co-op handler skipped"),
        }
    }
    Ok(found)
}

/// The games Nucleus Co-op 2.4.2 shows from the Nucleus Co-op at `folder`,
/// by their GUIDs, as its GameManager loads them: from [`list`] only, each
/// whose GUID a handler has and whose ExePath is a file. A game it does not
/// show can't be selected, and no profile of it appears.
pub fn listed_by_nucleus(folder: &Path) -> Result<Vec<String>> {
    let handlers = handlers(folder)?;
    let path = list(folder);
    let Some(text) = read_list(&path)? else {
        return Ok(Vec::new());
    };
    let games = handler::user_games(&text)
        .map_err(anyhow::Error::msg)
        .with_context(|| path.display().to_string())?;
    let mut listed: Vec<String> = games
        .into_iter()
        .filter(|game| handlers.iter().any(|h| h.guid == game.guid))
        .filter(|game| Path::new(&game.exe_path).is_file())
        .map(|game| game.guid)
        .collect();
    listed.sort();
    listed.dedup();
    Ok(listed)
}

fn read_text(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    String::from_utf8(bytes).with_context(|| format!("{} is not UTF-8 text", path.display()))
}

/// Decode a Nucleus handler file's bytes lossily: UTF-16 LE/BE when a BOM is
/// present, otherwise UTF-8 with replacement characters for Latin-1 /
/// Windows-1252 bytes so one non-UTF-8 handler never aborts the catalog.
fn decode_handler_text(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        let units: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&pair| u16::from_le_bytes(pair))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let units: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&pair| u16::from_be_bytes(pair))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    String::from_utf8_lossy(bytes).into_owned()
}

fn read_handler_text(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(decode_handler_text(&bytes))
}

/// The most of a game list the host reads; Nucleus' own are kilobytes.
const LIST_CAP: u64 = 16 * 1024 * 1024;

/// The game list at `path`, or None when there is none. It is the signed-in
/// user's, read with the host's rights: a link is opened itself and refused,
/// never followed, and only a file on a disk of at most [`LIST_CAP`] bytes
/// is read.
fn read_list(path: &Path) -> Result<Option<String>> {
    use std::io::Read;
    use std::os::windows::fs::MetadataExt;
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Storage::FileSystem::{FILE_TYPE_DISK, GetFileType};
    const REPARSE_POINT: u32 = 0x400;
    let refused = || {
        anyhow::anyhow!(
            "{} is a folder, a link or a device, not Nucleus Co-op's game list, so Rubylight does not read it: move it away, and Nucleus Co-op makes a new one",
            path.display()
        )
    };
    let file = match crate::process::open_unfollowed(path, false) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        // A folder or a link to one can't be opened as a file.
        Err(_)
            if std::fs::symlink_metadata(path).is_ok_and(|metadata| {
                metadata.is_dir() || metadata.file_attributes() & REPARSE_POINT != 0
            }) =>
        {
            return Err(refused());
        }
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    let metadata = file
        .metadata()
        .with_context(|| format!("reading {}", path.display()))?;
    // SAFETY: the handle is open for the call.
    let kind = unsafe { GetFileType(windows::Win32::Foundation::HANDLE(file.as_raw_handle())) };
    if !metadata.is_file()
        || metadata.file_attributes() & REPARSE_POINT != 0
        || kind != FILE_TYPE_DISK
    {
        return Err(refused());
    }
    let mut bytes = Vec::new();
    file.take(LIST_CAP + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading {}", path.display()))?;
    ensure!(
        bytes.len() as u64 <= LIST_CAP,
        "{} is larger than {} MB, far more than Nucleus Co-op's game list holds, so Rubylight does not read it: move it away, and Nucleus Co-op makes a new one",
        path.display(),
        LIST_CAP / (1024 * 1024)
    );
    String::from_utf8(bytes)
        .map(Some)
        .with_context(|| format!("{} is not UTF-8 text", path.display()))
}

/// The file's text, or None when there is no file.
fn read_existing(path: &Path) -> Result<Option<String>> {
    match std::fs::metadata(path) {
        Ok(_) => read_text(path).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

/// Refuses `setup`'s handler file in the Nucleus Co-op at `folder` when one
/// Rubylight did not write is there: it is never overwritten.
pub fn check_own(folder: &Path, setup: &Setup) -> Result<()> {
    let path = folder.join("handlers").join(setup.file());
    match read_existing(&path)? {
        Some(old) if !generic::owns(&old) => bail!(
            "{} is not Rubylight's handler, so Rubylight leaves it as it is: move it out of Nucleus Co-op's handlers folder",
            path.display()
        ),
        _ => Ok(()),
    }
}

/// Write Rubylight's one-player handler for `setup` into the Nucleus Co-op
/// at `folder` and add its game to its game list [`list`], with the host's
/// rights: the folder is Administrators'. Call it only while Nucleus Co-op
/// is not running: it would not read them, and its own save would drop the
/// game. A handler file Rubylight did not write is never overwritten; one it
/// did is rewritten when the template changed. Returns the handler as
/// Nucleus reads it.
pub fn write_own(folder: &Path, setup: &Setup) -> Result<Handler> {
    let file = setup.file();
    // Only its account's sweep may find it unused: the list it is in is theirs.
    let owner = crate::process::user_sid()
        .context("finding the signed-in user, whose game list the handler is for")?;
    let setup = &Setup {
        inject: inject_for(folder, &setup.exe_path)?,
        arguments: start_arguments_under(setup, &epic_program_data(), &EPIC_MANIFEST_COMPONENTS),
        ..setup.clone()
    };
    match setup.inject {
        Inject::Runtime => {}
        Inject::None => tracing::warn!(
            exe = setup.exe_path,
            "Nucleus Co-op: the game refused ProtoInput's input hooks every way, so Rubylight's handler injects none: a keyboard, mouse or pad on the PC can reach it too"
        ),
        way => tracing::info!(
            exe = setup.exe_path,
            way = way.name(),
            "Nucleus Co-op: the game refused ProtoInput's input hooks before, so Rubylight's handler injects them another way"
        ),
    }
    let text = setup.handler_of(Some(&owner));
    let handler = handler::parse(&file, &text).map_err(anyhow::Error::msg)?;
    let handlers = folder.join("handlers");
    super::unfollowed::create_folders(folder, &handlers)?;
    let path = handlers.join(&file);
    check_own(folder, setup)?;
    match read_existing(&path)? {
        Some(old) if old == text => {}
        _ => {
            super::unfollowed::write(folder, &path, text.as_bytes())?;
            tracing::info!(
                file,
                exe = setup.exe_path,
                "Nucleus Co-op: Rubylight's one-player handler written"
            );
        }
    }
    add_game(folder, setup)?;
    Ok(handler)
}

/// Relative path components of the Epic Games Launcher manifest folder below `%PROGRAMDATA%`.
const EPIC_MANIFEST_COMPONENTS: [&str; 4] = ["Epic", "EpicGamesLauncher", "Data", "Manifests"];

/// `%PROGRAMDATA%`, the root from which the Epic manifest path is walked.
fn epic_program_data() -> PathBuf {
    PathBuf::from(std::env::var_os("PROGRAMDATA").unwrap_or_else(|| r"C:\ProgramData".into()))
}

/// Larger than any manifest the launcher writes.
const EPIC_MANIFEST_MAX: u64 = 1 << 20;

/// Run `work` while impersonating the signed-in user when running as `SYSTEM`.
/// Fails closed if `SYSTEM` has no signed-in user token to impersonate.
fn as_signed_in_user_if_system<T>(work: impl FnOnce() -> T) -> std::result::Result<T, String> {
    if !crate::process::is_system() {
        return Ok(work());
    }
    let token = crate::user_folder::user_token().map_err(|error| error.to_string())?;
    let Some(token) = token else {
        return Err("no signed-in user token available".into());
    };
    let _impersonation = crate::user_folder::Impersonation::begin(Some(&token))
        .map_err(|error| error.to_string())?;
    Ok(work())
}

/// Open `base` and every relative component in `components` without following
/// reparse points, holding each directory handle open without delete sharing
/// so no component can be swapped for a junction while manifests are read.
fn open_unfollowed_epic_dir(
    base: &Path,
    components: &[&str],
) -> std::result::Result<Option<(Vec<std::fs::File>, PathBuf)>, String> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use windows::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_READ,
        FILE_SHARE_WRITE, SECURITY_ANONYMOUS, SECURITY_SQOS_PRESENT,
    };
    let open_dir = |path: &Path| {
        std::fs::OpenOptions::new()
            .access_mode(FILE_LIST_DIRECTORY.0 | FILE_READ_ATTRIBUTES.0)
            .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE).0)
            .custom_flags(
                (FILE_FLAG_BACKUP_SEMANTICS
                    | FILE_FLAG_OPEN_REPARSE_POINT
                    | SECURITY_SQOS_PRESENT
                    | SECURITY_ANONYMOUS)
                    .0,
            )
            .open(path)
    };
    let root_handle = match open_dir(base) {
        Ok(handle) => handle,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let root_attrs = root_handle
        .metadata()
        .map_err(|error| error.to_string())?
        .file_attributes();
    if root_attrs & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
        return Err(format!("{} is a link", base.display()));
    }
    if root_attrs & FILE_ATTRIBUTE_DIRECTORY.0 == 0 {
        return Err(format!("{} is not a directory", base.display()));
    }
    let mut resolved = PathBuf::from(
        crate::user_folder::final_path(&root_handle).map_err(|error| error.to_string())?,
    );
    let mut pinned = vec![root_handle];
    for part in components {
        let next = resolved.join(part);
        let handle = match open_dir(&next) {
            Ok(handle) => handle,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.to_string()),
        };
        let attrs = handle
            .metadata()
            .map_err(|error| error.to_string())?
            .file_attributes();
        if attrs & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
            return Err(format!("{} is a link", next.display()));
        }
        if attrs & FILE_ATTRIBUTE_DIRECTORY.0 == 0 {
            return Err(format!("{} is not a directory", next.display()));
        }
        let actual = crate::user_folder::final_path(&handle).map_err(|error| error.to_string())?;
        if !crate::user_folder::same_name(&actual, next.as_os_str()) {
            return Err(format!("{} leads elsewhere through a link", next.display()));
        }
        resolved = next;
        pinned.push(handle);
    }
    Ok(Some((pinned, resolved)))
}

/// A manifest's bytes. The folder is writable by every user: never through a
/// link, never a non-disk device/pipe, with anonymous SQOS, and never more
/// than a manifest's size.
#[cfg(test)]
fn read_epic_manifest(path: &Path) -> Result<Vec<u8>, String> {
    read_epic_manifest_checked(path, None)
}

fn read_epic_manifest_checked(path: &Path, expected: Option<&Path>) -> Result<Vec<u8>, String> {
    use std::io::Read;
    use std::os::windows::{
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawHandle,
    };
    use windows::Win32::{
        Foundation::HANDLE,
        Storage::FileSystem::{
            FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT, FILE_TYPE_DISK,
            GetFileType, SECURITY_ANONYMOUS, SECURITY_SQOS_PRESENT,
        },
    };
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags((FILE_FLAG_OPEN_REPARSE_POINT | SECURITY_SQOS_PRESENT | SECURITY_ANONYMOUS).0)
        .open(path)
        .map_err(|error| error.to_string())?;
    // SAFETY: `file` is an open handle for the duration of this call.
    if unsafe { GetFileType(HANDLE(file.as_raw_handle())) } != FILE_TYPE_DISK {
        return Err("it is not a disk file".into());
    }
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
        return Err("it is a link".into());
    }
    if !metadata.is_file() {
        return Err("it is not a file".into());
    }
    if let Some(expected) = expected {
        let actual = crate::user_folder::final_path(&file).map_err(|error| error.to_string())?;
        if !crate::user_folder::same_name(&actual, expected.as_os_str()) {
            return Err("it leads elsewhere through a link".into());
        }
    }
    let mut bytes = Vec::new();
    file.take(EPIC_MANIFEST_MAX + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > EPIC_MANIFEST_MAX {
        return Err(format!("it is over {EPIC_MANIFEST_MAX} bytes"));
    }
    Ok(bytes)
}

/// What the Epic Games Launcher in `base`/`components` starts the game at `exe`
/// with ([`generic::epic_arguments`]); None for any other game. Walks every
/// directory component rejecting reparse points, and runs under signed-in user
/// impersonation when the host is `SYSTEM`.
fn epic_arguments_under(base: &Path, components: &[&str], exe: &str) -> Option<String> {
    match as_signed_in_user_if_system(|| {
        let (_pinned, resolved) = match open_unfollowed_epic_dir(base, components) {
            Ok(Some(found)) => found,
            Ok(None) => return None,
            Err(error) => {
                tracing::warn!(folder = %base.display(), error, "Nucleus Co-op: the Epic Games Launcher's manifests can't be read");
                return None;
            }
        };
        let files = match std::fs::read_dir(&resolved) {
            Ok(files) => files,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
            Err(error) => {
                tracing::warn!(folder = %resolved.display(), %error, "Nucleus Co-op: the Epic Games Launcher's manifests can't be read");
                return None;
            }
        };
        files
            .flatten()
            .map(|file| resolved.join(file.file_name()))
            .filter(|path| path.extension().is_some_and(|ext| ext == "item"))
            .find_map(|path| {
                match read_epic_manifest_checked(&path, Some(&path))
                    .and_then(|bytes| generic::epic_arguments(&bytes, exe))
                {
                    Ok(arguments) => arguments,
                    Err(error) => {
                        tracing::warn!(manifest = %path.display(), error, "Nucleus Co-op: an Epic Games Launcher manifest can't be used");
                        None
                    }
                }
            })
    }) {
        Ok(arguments) => arguments,
        Err(error) => {
            tracing::warn!(folder = %base.display(), error, "Nucleus Co-op: could not read Epic Games Launcher manifests as the signed-in user");
            None
        }
    }
}

/// What the Epic Games Launcher in `manifests` starts the game at `exe` with.
#[cfg(test)]
fn epic_arguments(manifests: &Path, exe: &str) -> Option<String> {
    epic_arguments_under(manifests, &[], exe)
}

/// Whether the Epic Games Launcher installed the game at `exe`.
pub(super) fn epic_game(exe: &str) -> bool {
    epic_arguments_under(&epic_program_data(), &EPIC_MANIFEST_COMPONENTS, exe).is_some()
}

/// What Rubylight's handler starts `setup`'s game with.
fn start_arguments_under(setup: &Setup, base: &Path, components: &[&str]) -> String {
    let arguments = setup.windowed_arguments();
    match epic_arguments_under(base, components, &setup.exe_path) {
        Some(epic) if !arguments.to_ascii_lowercase().contains("-epicportal") => {
            tracing::info!(
                exe = setup.exe_path,
                "Nucleus Co-op: an Epic game, started with the arguments its launcher gives it"
            );
            format!("{arguments} {epic}").trim().to_string()
        }
        _ => arguments,
    }
}

#[cfg(test)]
fn start_arguments(setup: &Setup, manifests: &Path) -> String {
    start_arguments_under(setup, manifests, &[])
}

/// How Rubylight's handler injects ProtoInput's input hooks into the games
/// that refused a way in a session of the Nucleus Co-op at `folder` (the
/// host's, which only SYSTEM and Administrators change):
/// `{"Games": {<.exe path in lower case>: <Inject::name>}}`. Named anew when
/// Nucleus stopped starting games twice: what the files before it hold are
/// mostly those double starts, not refusals (#59), and a game kept there
/// would never get its hooks back.
fn injection(folder: &Path) -> PathBuf {
    folder.join("Rubylight-injection-2.json")
}

fn injections(folder: &Path) -> Result<BTreeMap<String, Inject>> {
    let mut games = BTreeMap::new();
    let path = injection(folder);
    let Some(text) = read_existing(&path)? else {
        return Ok(games);
    };
    let unreadable = || format!("{} can't be read: delete it", path.display());
    let json: serde_json::Value = serde_json::from_str(&text).with_context(unreadable)?;
    let Some(kept) = json.get("Games").and_then(serde_json::Value::as_object) else {
        bail!(unreadable());
    };
    for (game, way) in kept {
        let way = way.as_str().and_then(Inject::from_name);
        games.insert(game.clone(), way.with_context(unreadable)?);
    }
    Ok(games)
}

fn game_key(exe: &str) -> String {
    exe.trim().replace('/', "\\").to_lowercase()
}

/// How Rubylight's handler for the game at `exe` injects ProtoInput's
/// input hooks ([`Setup::inject`]): the first way, or the next after one it
/// refused in a session of the Nucleus Co-op at `folder` ([`refuse_inject`]).
pub fn inject_for(folder: &Path, exe: &str) -> Result<Inject> {
    Ok(injections(folder)?
        .get(&game_key(exe))
        .copied()
        .unwrap_or_default())
}

/// Remember that the game at `exe` refused the way ProtoInput's input hooks
/// were injected in a session of the Nucleus Co-op at `folder`, which
/// ended it (#59): its next start gets the next way. Returns that way.
pub fn refuse_inject(folder: &Path, exe: &str) -> Result<Inject> {
    let mut games = injections(folder)?;
    let key = game_key(exe);
    let refused = games.get(&key).copied().unwrap_or_default();
    let Some(next) = refused.next() else {
        return Ok(refused);
    };
    games.insert(key, next);
    let games: serde_json::Map<_, _> = games
        .into_iter()
        .map(|(game, way)| (game, way.name().into()))
        .collect();
    let text = serde_json::to_string_pretty(&serde_json::json!({ "Games": games }))?;
    replace(folder, &injection(folder), &text)?;
    tracing::warn!(
        exe,
        refused = refused.name(),
        next = next.name(),
        "Nucleus Co-op: the game refused ProtoInput's input hooks; its next start gets the next way"
    );
    Ok(next)
}

/// Add `setup`'s game to the game list of the Nucleus Co-op at `folder`
/// ([`list`]), replacing the file whole so Nucleus never reads half of it.
pub(super) fn add_game(folder: &Path, setup: &Setup) -> Result<()> {
    let path = &list(folder);
    let old = read_list(path)?;
    let Some(new) = generic::add_game(old.as_deref(), &setup.guid(), &setup.exe_path)
        .map_err(anyhow::Error::msg)
        .with_context(|| path.display().to_string())?
    else {
        return Ok(());
    };
    let parent = path.parent().context("the game list has no folder")?;
    super::unfollowed::create_folders(folder, parent)?;
    replace(folder, path, &new)?;
    tracing::info!(
        guid = setup.guid(),
        "Nucleus Co-op: game added to its game list"
    );
    Ok(())
}

/// Replace the game list at `path` below `root` whole, so Nucleus never
/// reads half of it, through no link at its name or on its way.
fn replace(root: &Path, path: &Path, text: &str) -> Result<()> {
    super::unfollowed::write(root, path, text.as_bytes())
}

/// The game list of the Nucleus Co-op at `folder`, the one 2.4.2 reads
/// (its GameManager's ALPHA branch is the released build). Nucleus Co-op
/// never reads `%APPDATA%\Nucleus Coop\userprofile.json`, where earlier
/// Rubylight builds added their games (#59).
pub fn list(folder: &Path) -> PathBuf {
    folder.join("content").join("userprofile.json")
}

/// The game list in the roaming folder `appdata`, which Nucleus Co-op 2.4.2
/// never reads.
fn appdata_list(appdata: &Path) -> PathBuf {
    appdata.join("Nucleus Coop").join("userprofile.json")
}

/// Refuses the game list of the Nucleus Co-op at `folder` when Rubylight
/// could not add `setup`'s game to it or remove the games of `remove` from
/// it. Checked before a session closes the console's window, so a list
/// Nucleus Co-op wrote in a way the host can't edit refuses the launch with
/// the window still open.
pub fn check_list(folder: &Path, setup: Option<&Setup>, remove: &[&str]) -> Result<()> {
    let path = list(folder);
    let Some(text) = read_list(&path)? else {
        return Ok(());
    };
    let edits = || -> Result<(), String> {
        if let Some(setup) = setup {
            generic::add_game(Some(&text), &setup.guid(), &setup.exe_path)?;
        }
        for guid in remove {
            generic::remove_game(&text, guid)?;
        }
        Ok(())
    };
    edits()
        .map_err(anyhow::Error::msg)
        .with_context(|| path.display().to_string())
}

/// Remove Rubylight's own handler `own`, one [`Catalog::stale`] names, and
/// its game from the game list of the Nucleus Co-op at `folder`. Like
/// [`write_own`], only while Nucleus Co-op is not running. Only a handler
/// Rubylight wrote, and only its game, ever go.
pub fn remove_own(folder: &Path, own: &Handler) -> Result<()> {
    ensure!(
        own.own && own.guid.starts_with(generic::PREFIX),
        "{} is not Rubylight's handler",
        own.file
    );
    // The handler first: Nucleus Co-op drops a game whose handler is gone
    // as it loads its list, and a handler that stays is found again.
    let path = folder.join("handlers").join(&own.file);
    if let Some(text) = read_existing(&path)? {
        ensure!(
            generic::owns(&text),
            "{} is not Rubylight's handler, so Rubylight leaves it as it is",
            path.display()
        );
        super::unfollowed::remove(folder, &path)?;
    }
    let list = list(folder);
    remove_game(folder, &own.guid).with_context(|| {
        format!(
            "{} is removed, but {} still lists its game. Nucleus Co-op leaves a game whose handler is gone out of the list as it loads it, and Rubylight's next handler for the game takes the entry over",
            path.display(),
            list.display()
        )
    })?;
    tracing::info!(
        file = own.file,
        "Nucleus Co-op: Rubylight's handler removed, a hub handler replaces it or no game uses it"
    );
    Ok(())
}

/// Remove the games of Rubylight's handlers, and nothing else, from
/// `%APPDATA%\Nucleus Coop\userprofile.json` in the signed-in user's roaming
/// folder `appdata`, as the user: earlier builds added them there, and
/// Nucleus Co-op 2.4.2 never reads that list. Returns how many went; none
/// once they are gone.
pub fn forget_appdata_games(appdata: &Path) -> Result<usize> {
    crate::user_folder::as_session_user(|| forget_games(appdata))
        .context("acting as the signed-in user")?
}

fn forget_games(appdata: &Path) -> Result<usize> {
    let path = &appdata_list(appdata);
    let Some(mut text) = read_list(path)? else {
        return Ok(0);
    };
    let games = handler::user_games(&text)
        .map_err(anyhow::Error::msg)
        .with_context(|| path.display().to_string())?;
    let mut guids: Vec<String> = games
        .into_iter()
        .map(|game| game.guid)
        .filter(|guid| guid.starts_with(generic::PREFIX))
        .collect();
    guids.dedup();
    let mut forgotten = 0;
    for guid in &guids {
        if let Some(new) = generic::remove_game(&text, guid)
            .map_err(anyhow::Error::msg)
            .with_context(|| path.display().to_string())?
        {
            text = new;
            forgotten += 1;
        }
    }
    if forgotten > 0 {
        replace(appdata, path, &text)?;
        tracing::info!(
            forgotten,
            list = %path.display(),
            "Nucleus Co-op: Rubylight's games removed from a list Nucleus Co-op does not read"
        );
    }
    Ok(forgotten)
}

/// Remove the game `guid` from the game list of the Nucleus Co-op at
/// `folder` ([`list`]), if it is there.
pub(super) fn remove_game(folder: &Path, guid: &str) -> Result<()> {
    let path = &list(folder);
    let Some(old) = read_list(path)? else {
        return Ok(());
    };
    match generic::remove_game(&old, guid)
        .map_err(anyhow::Error::msg)
        .with_context(|| path.display().to_string())?
    {
        Some(new) => replace(folder, path, &new),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests;
