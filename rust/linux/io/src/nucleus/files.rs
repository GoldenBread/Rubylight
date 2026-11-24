//! The files of a Nucleus Co-op folder, on Linux: its handlers and its game list are read
//! as on Windows, and the checks that only read them work. Nothing is written: Rubylight's
//! own handler, the game list, `Settings.ini` and the game profile belong to a session, and
//! none starts here.
use anyhow::{Context, Result, bail, ensure};
use butterpollo_session::nucleus::generic::{self, Setup};
use butterpollo_session::nucleus::handler::{self, Catalog, Handler};
use std::path::{Path, PathBuf};

/// The handlers and the games added in the Nucleus Co-op at `folder`, from its game list
/// [`list`]. A handler that cannot be read is left out with a log line; an unreadable game
/// list is an error. Linux has no signed-in Windows account, so [`Catalog::user`] is none
/// whatever `signed_in` says.
pub fn catalog(folder: &Path, _signed_in: bool) -> Result<Catalog> {
    let mut catalog = Catalog {
        handlers: handlers(folder)?,
        ..Catalog::default()
    };
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

/// The handlers in the Nucleus Co-op at `folder`; one that cannot be read is left out with
/// a log line.
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
        let text = read_text(&path)?;
        match handler::parse(&file, &text) {
            Ok(handler) => found.push(handler),
            Err(error) => tracing::info!(%file, %error, "Nucleus Co-op handler skipped"),
        }
    }
    Ok(found)
}

/// The games Nucleus Co-op shows from the Nucleus Co-op at `folder`, by their GUIDs: from
/// [`list`] only, each whose GUID a handler has and whose ExePath is a file.
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

/// The most of a game list the host reads; Nucleus' own are kilobytes.
const LIST_CAP: u64 = 16 * 1024 * 1024;

/// The game list at `path`, or None when there is none. A link is refused, never followed,
/// and only a regular file of at most [`LIST_CAP`] bytes is read.
fn read_list(path: &Path) -> Result<Option<String>> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let refused = || {
        anyhow::anyhow!(
            "{} is a folder, a link or a device, not Nucleus Co-op's game list, so Rubylight does not read it: move it away, and Nucleus Co-op makes a new one",
            path.display()
        )
    };
    let file = match std::fs::OpenOptions::new()
        .read(true)
        // No link is followed, and a pipe in the list's place never holds the open up.
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        // What opening a link without following it fails with.
        Err(error) if error.raw_os_error() == Some(libc::ELOOP) => return Err(refused()),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    let metadata = file
        .metadata()
        .with_context(|| format!("reading {}", path.display()))?;
    if !metadata.is_file() {
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

/// Refuses `setup`'s handler file in the Nucleus Co-op at `folder` when one Rubylight did
/// not write is there: it is never overwritten.
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

/// Write Rubylight's one-player handler for `setup` into the Nucleus Co-op at `folder` and
/// add its game to the game list. Always an error on Linux: nothing is written.
pub fn write_own(_folder: &Path, _setup: &Setup) -> Result<Handler> {
    Err(crate::unsupported(
        "Writing Rubylight's handler for Nucleus Co-op",
    ))
}

/// The game list of the Nucleus Co-op at `folder`, the one Nucleus Co-op 2.4.2 reads.
pub fn list(folder: &Path) -> PathBuf {
    folder.join("content").join("userprofile.json")
}

/// Refuses the game list of the Nucleus Co-op at `folder` when Rubylight could not add
/// `setup`'s game to it or remove the games of `remove` from it. Only reads the list.
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

/// Remove Rubylight's own handler `own` and its game from the game list of the Nucleus
/// Co-op at `folder`. Always an error on Linux: nothing is removed.
pub fn remove_own(_folder: &Path, _own: &Handler) -> Result<()> {
    Err(crate::unsupported(
        "Removing Rubylight's handler for Nucleus Co-op",
    ))
}

/// Remove the games of Rubylight's handlers from the list earlier Windows builds wrote in
/// the user's roaming folder `appdata`; how many went. Linux never had that list, so none
/// is there to remove and nothing is read.
pub fn forget_appdata_games(_appdata: &Path) -> Result<usize> {
    Ok(0)
}
