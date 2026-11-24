//! Hardware-case cleanup, process inspection, log capture, and Settings.ini helpers for Nucleus Co-op tests.
use super::setup_helper::app;
use anyhow::{Context, Result, ensure};
use butterpollo_session::nucleus::elevation::Run;
use butterpollo_session::state::App;
use butterpollo_stream::nucleus::*;
use butterpollo_win_io::nucleus::user_copy;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

/// The app a hardware case streams in the background: the game's .exe as
/// its command, or a Steam game's link.
pub(crate) fn case_app(game: Option<&str>, steam_app: Option<u32>) -> Result<App> {
    match (game, steam_app) {
        (Some(game), None) => {
            let name = Path::new(game)
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .context("the game's .exe has no name")?;
            Ok(app(&name, &format!("\"{game}\"")))
        }
        (None, Some(id)) => Ok(app(
            &format!("Steam app {id}"),
            &format!("steam://rungameid/{id}"),
        )),
        _ => anyhow::bail!("set RUBYLIGHT_NUCLEUS_GAME or RUBYLIGHT_NUCLEUS_STEAM_APP"),
    }
}

/// The share of a BGRA frame's pixels that are not near black.
pub(crate) fn lit(image: &butterpollo_win_graphics::capture::Image) -> Result<f64> {
    ensure!(
        image.pixel == butterpollo_win_graphics::capture::Pixel::Bgra8,
        "a {:?} frame, not BGRA",
        image.pixel
    );
    let (width, height) = (image.width as usize, image.height as usize);
    ensure!(
        width > 0 && height > 0 && image.bytes.len() >= image.stride * (height - 1) + width * 4,
        "a {width}x{height} frame of {} bytes",
        image.bytes.len()
    );
    let (mut seen, mut bright) = (0usize, 0usize);
    for y in (0..height).step_by(4) {
        let row = &image.bytes[y * image.stride..][..width * 4];
        for pixel in row.as_chunks::<4>().0.iter().step_by(4) {
            seen += 1;
            bright += usize::from(pixel[..3].iter().any(|&c| c > 24));
        }
    }
    Ok(bright as f64 / seen as f64)
}

/// Where Nucleus sets the DPI mode for the game at `game`: that exact
/// path, and the instances of its `executable` below the `content` folder
/// of each Nucleus copy it may run from (the host's, the user's).
fn nucleus_sets(name: &str, game: &str, contents: &[PathBuf], executable: &str) -> bool {
    let plain = |path: &str| path.trim().replace('/', "\\").to_lowercase();
    let name = plain(name);
    let below = |content: &PathBuf| {
        name.strip_prefix(plain(&content.to_string_lossy()).trim_end_matches('\\'))
            .is_some_and(|rest| rest.starts_with('\\'))
    };
    name == plain(game)
        || (contents.iter().any(below) && file_name(&name).eq_ignore_ascii_case(executable))
}

/// The compatibility settings the case changes back to `before`: each
/// value Nucleus sets for the game at `game` ([`nucleus_sets`]) that is
/// not as it was, with the data it gets, None to remove it. Another
/// program's, even one of the same file name, is never touched.
pub(crate) fn layer_changes(
    before: &BTreeMap<String, butterpollo_win_io::nucleus::layers::Value>,
    after: &BTreeMap<String, butterpollo_win_io::nucleus::layers::Value>,
    game: &str,
    contents: &[PathBuf],
    executable: &str,
) -> Vec<(String, Option<butterpollo_win_io::nucleus::layers::Value>)> {
    let named = |name: &&String| nucleus_sets(name, game, contents, executable);
    let mut changes: Vec<_> = after
        .iter()
        .filter(|(name, _)| named(name))
        .filter(|(name, value)| before.get(*name) != Some(*value))
        .map(|(name, _)| (name.clone(), before.get(name).cloned()))
        .collect();
    changes.extend(
        before
            .iter()
            .filter(|(name, _)| named(name) && !after.contains_key(*name))
            .map(|(name, value)| (name.clone(), Some(value.clone()))),
    );
    changes
}

/// Files a hardware case may change, as they were before it: a game
/// list, and each handler of Rubylight's (its marker) by name, in each
/// handlers folder. Nothing is put back through a link: the user may
/// change their own copy (whose files the case takes and puts back as
/// them).
#[derive(Debug, Default)]
pub(crate) struct KeptFiles {
    /// Each path's bytes; None when it was not there.
    files: BTreeMap<PathBuf, Option<Vec<u8>>>,
    handlers: Vec<PathBuf>,
}
impl KeptFiles {
    pub(crate) fn own_handlers(handlers: &Path) -> Result<Vec<PathBuf>> {
        let mut own = Vec::new();
        let entries = match std::fs::read_dir(handlers) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(own),
            Err(error) => return Err(error).context(handlers.display().to_string()),
        };
        for entry in entries {
            let path = entry?.path();
            if path.is_file()
                && std::fs::read(&path).is_ok_and(|text| {
                    butterpollo_session::nucleus::generic::owns(&String::from_utf8_lossy(&text))
                })
            {
                own.push(path);
            }
        }
        Ok(own)
    }

    pub(crate) fn take(handlers: &[&Path], list: &Path) -> Result<Self> {
        let mut files = BTreeMap::new();
        let mut own = Vec::new();
        for folder in handlers {
            own.extend(Self::own_handlers(folder)?);
        }
        for path in own.into_iter().chain([list.to_path_buf()]) {
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => Some(bytes),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error).context(path.display().to_string()),
            };
            files.insert(path, bytes);
        }
        Ok(Self {
            files,
            handlers: handlers.iter().map(|folder| folder.to_path_buf()).collect(),
        })
    }

    /// An error when `path` or its folder is a link.
    fn unlinked(path: &Path) -> Result<()> {
        use std::os::windows::fs::MetadataExt;
        for at in [Some(path), path.parent()].into_iter().flatten() {
            match std::fs::symlink_metadata(at) {
                Ok(metadata) => ensure!(
                    metadata.file_attributes() & 0x400 == 0,
                    "{} is a link, so the case puts nothing back through it",
                    at.display()
                ),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error).context(at.display().to_string()),
            }
        }
        Ok(())
    }

    /// Each file as it was, and a handler of ours the case wrote gone.
    /// Returns what could not be put back.
    pub(crate) fn put_back(&self) -> Vec<String> {
        let mut failures = Vec::new();
        for folder in &self.handlers {
            if let Err(error) = Self::unlinked(folder) {
                failures.push(format!("{error:#}"));
                continue;
            }
            match Self::own_handlers(folder) {
                Ok(now) => {
                    for path in now.iter().filter(|path| !self.files.contains_key(*path)) {
                        match std::fs::remove_file(path) {
                            Ok(()) => println!("cleanup: removed {}", path.display()),
                            Err(error) => failures.push(format!("{}: {error}", path.display())),
                        }
                    }
                }
                Err(error) => failures.push(format!("{error:#}")),
            }
        }
        for (path, bytes) in &self.files {
            if let Err(error) = Self::unlinked(path) {
                failures.push(format!("{error:#}"));
                continue;
            }
            let now = std::fs::read(path).ok();
            if now == *bytes {
                continue;
            }
            let done = match bytes {
                Some(bytes) => std::fs::write(path, bytes),
                None => std::fs::remove_file(path),
            };
            match done {
                Ok(()) => println!("cleanup: {} as before", path.display()),
                Err(error) => failures.push(format!("{}: {error}", path.display())),
            }
        }
        failures
    }
}

/// What a hardware case leaves: the session, the game list and
/// Rubylight's handlers ([`KeptFiles`]), and the game's compatibility
/// settings at its exact paths, put back as they were once, also when
/// the case fails.
pub(crate) struct CaseCleanup {
    pub(crate) registry: Arc<Registry>,
    /// The host's files; the user's copy's, put back as `user`.
    pub(crate) kept: KeptFiles,
    pub(crate) kept_copy: KeptFiles,
    pub(crate) user: Option<std::os::windows::io::OwnedHandle>,
    pub(crate) layers_before: BTreeMap<String, butterpollo_win_io::nucleus::layers::Value>,
    /// The `content` folder of each Nucleus copy the session may run
    /// from: the host's and the user's own.
    pub(crate) contents: Vec<PathBuf>,
    /// The game's path and its handler's .exe, once admitted.
    pub(crate) game: Option<(String, String)>,
    pub(crate) done: bool,
}
impl CaseCleanup {
    pub(crate) fn run(&mut self) -> Result<()> {
        if std::mem::replace(&mut self.done, true) {
            return Ok(());
        }
        for listed in self.registry.sessions() {
            self.registry.end(listed.session.id);
        }
        let mut failures = self.kept.put_back();
        match user_copy::as_user(self.user.as_ref(), || self.kept_copy.put_back()) {
            Ok(more) => failures.extend(more),
            Err(error) => failures.push(format!("the user's copy: {error:#}")),
        }
        if let Some((game, executable)) = &self.game {
            let changed = (|| -> Result<usize> {
                use butterpollo_win_io::nucleus::layers::Layers;
                let mut hive = butterpollo_win_io::nucleus::layers::UserHive::signed_in()?;
                let changes = layer_changes(
                    &self.layers_before,
                    &hive.values()?,
                    game,
                    &self.contents,
                    executable,
                );
                for (name, value) in &changes {
                    match value {
                        Some(value) => hive.set(name, value)?,
                        None => hive.delete(name)?,
                    }
                }
                Ok(changes.len())
            })();
            match changed {
                Ok(0) => {}
                Ok(changed) => println!("cleanup: {changed} compatibility settings put back"),
                Err(error) => failures.push(format!("compatibility settings: {error:#}")),
            }
        }
        ensure!(
            failures.is_empty(),
            "cleanup failed: {}",
            failures.join("; ")
        );
        Ok(())
    }
}
impl Drop for CaseCleanup {
    fn drop(&mut self) {
        if let Err(error) = self.run() {
            eprintln!("{error:#}");
        }
    }
}

/// How a hardware case expects the session to run, from
/// `RUBYLIGHT_NUCLEUS_RUN`: "user" or "elevated". Unset or empty, it
/// takes whichever the admission chooses.
pub(crate) fn expected_run(value: Option<&str>) -> Result<Option<Run>> {
    match value.map(str::trim) {
        None | Some("") => Ok(None),
        Some("user") => Ok(Some(Run::User)),
        Some("elevated") => Ok(Some(Run::Elevated)),
        Some(other) => {
            anyhow::bail!("RUBYLIGHT_NUCLEUS_RUN is {other:?}, not user or elevated")
        }
    }
}

/// How a session ran, from its NucleusCoop.exe's program path `image`:
/// as the user from their own `copy`, elevated from the host's `folder`.
pub(crate) fn ran_from(image: &str, folder: &Path, copy: &Path) -> Result<Run> {
    let plain = |path: &Path| {
        let path = path.to_string_lossy().replace('/', "\\").to_lowercase();
        let path = path.strip_prefix(r"\\?\").unwrap_or(&path);
        path.trim_end_matches('\\').to_string()
    };
    let image = Path::new(image);
    ensure!(
        image
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("NucleusCoop.exe")),
        "{} is not NucleusCoop.exe",
        image.display()
    );
    match image.parent().map(plain) {
        Some(parent) if parent == plain(copy) => Ok(Run::User),
        Some(parent) if parent == plain(folder) => Ok(Run::Elevated),
        _ => anyhow::bail!(
            "NucleusCoop.exe ran from {}, neither the user's own copy {} nor the host's {}",
            image.display(),
            copy.display(),
            folder.display()
        ),
    }
}

/// SECURITY_MANDATORY_MEDIUM_RID: the integrity level of a program
/// running with a user's standard rights.
pub(crate) const MEDIUM_INTEGRITY: u32 = 0x2000;

/// That `name` (process `pid`), whose token is `elevated` at `integrity`,
/// runs with the user's standard rights: not elevated, medium integrity.
pub(crate) fn standard_rights(name: &str, pid: u32, elevated: bool, integrity: u32) -> Result<()> {
    ensure!(
        !elevated && integrity == MEDIUM_INTEGRITY,
        "{name} {pid} runs {} at integrity 0x{integrity:x}, not unelevated at medium (0x2000)",
        if elevated { "elevated" } else { "unelevated" }
    );
    Ok(())
}

/// Whether process `pid`'s token is elevated, and its integrity level
/// (the last RID of its mandatory label).
pub(crate) fn token_rights(pid: u32) -> Result<(bool, u32)> {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{
        GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TOKEN_ELEVATION,
        TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TokenElevation, TokenIntegrityLevel,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    struct Owned(HANDLE);
    impl Drop for Owned {
        fn drop(&mut self) {
            // SAFETY: the handle was opened for this value and is closed once.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
    // SAFETY: each handle is closed once by `Owned`. The buffers outlive
    // the calls that fill them, which get their sizes; the label's SID
    // points into `label`, read while it lives.
    unsafe {
        let process = Owned(
            OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
                .with_context(|| format!("opening process {pid}"))?,
        );
        let mut token = HANDLE::default();
        OpenProcessToken(process.0, TOKEN_QUERY, &mut token)
            .with_context(|| format!("opening process {pid}'s token"))?;
        let token = Owned(token);
        let mut elevation = TOKEN_ELEVATION::default();
        let mut bytes = 0;
        GetTokenInformation(
            token.0,
            TokenElevation,
            Some((&mut elevation as *mut TOKEN_ELEVATION).cast()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut bytes,
        )?;
        // TOKEN_MANDATORY_LABEL and the SID it points to, 8-byte aligned.
        let mut label = [0u64; 16];
        GetTokenInformation(
            token.0,
            TokenIntegrityLevel,
            Some(label.as_mut_ptr().cast()),
            size_of_val(&label) as u32,
            &mut bytes,
        )?;
        let sid = (*label.as_ptr().cast::<TOKEN_MANDATORY_LABEL>()).Label.Sid;
        let count = *GetSidSubAuthorityCount(sid);
        ensure!(count > 0, "process {pid}'s integrity label has no level");
        let integrity = *GetSidSubAuthority(sid, u32::from(count) - 1);
        Ok((elevation.TokenIsElevated != 0, integrity))
    }
}

/// The values of a user's Nucleus `Settings.ini` that stalled a
/// background stream at Nucleus' start (#59): its folder check on,
/// favourites only, and only games for a single keyboard listed.
pub(crate) const COLD: [(&str, &str, &str); 3] = [
    ("Dev", "DisablePathCheck", "False"),
    ("Dev", "ShowFavoriteOnly", "True"),
    ("Misc", "GameSortingOpt", "Sort_2,Show_Type2"),
];
/// What they are while a session from them runs.
pub(crate) const FORCED: [(&str, &str, &str); 6] = [
    ("Dev", "UseXinputIndex", "True"),
    ("Misc", "DisableGameProfiles", "False"),
    ("Dev", "DisablePathCheck", "True"),
    ("Dev", "ShowFavoriteOnly", "False"),
    ("Misc", "IgnoreInputLockReminder", "True"),
    ("Misc", "GameSortingOpt", "Sort_2"),
];

/// Nucleus' `Settings.ini` with [`COLD`]'s values.
pub(crate) fn cold_settings(text: &[u8]) -> Vec<u8> {
    butterpollo_session::nucleus::profile::ini_set(text, &COLD)
}

/// Fails unless `Settings.ini` has each of `values`, naming the first
/// that differs.
pub(crate) fn settings_are(text: &[u8], values: &[(&str, &str, &str)]) -> Result<()> {
    for &(section, key, value) in values {
        let found = butterpollo_session::nucleus::profile::ini_value(text, section, key)
            .map(|v| String::from_utf8_lossy(&v).into_owned());
        ensure!(
            found.as_deref() == Some(value),
            "Settings.ini has {key}={}, not {value}",
            found.as_deref().unwrap_or("nothing")
        );
    }
    Ok(())
}

/// How long after "Nucleus Co-op started" the session logged
/// "profile Rubylight clicked", from the log lines and when each came.
pub(crate) fn click_delay(lines: &[(Instant, String)]) -> Result<Duration> {
    let started = lines
        .iter()
        .find(|(_, line)| line.contains("Nucleus Co-op started"))
        .map(|(at, _)| *at)
        .context("no \"Nucleus Co-op started\" in the log")?;
    let clicked = lines
        .iter()
        .find(|(at, line)| *at >= started && line.contains("profile Rubylight clicked"))
        .map(|(at, _)| *at)
        .context("no \"profile Rubylight clicked\" after \"Nucleus Co-op started\"")?;
    Ok(clicked - started)
}

/// The host's log during a hardware case: printed, and each line kept
/// with when it came.
#[derive(Clone, Default)]
pub(crate) struct CaseLog(pub(crate) Arc<std::sync::Mutex<Vec<(Instant, String)>>>);
impl std::io::Write for CaseLog {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let text = String::from_utf8_lossy(buf);
        print!("{text}");
        let mut lines = self.0.lock().unwrap();
        lines.extend(text.lines().map(|line| (Instant::now(), line.to_string())));
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl CaseLog {
    pub(crate) fn install() -> Result<CaseLog> {
        let log = CaseLog::default();
        let writer = log.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::set_global_default(subscriber)
            .context("a log was set up before the case")?;
        Ok(log)
    }
}

/// A file's bytes, None when it is not there.
pub(crate) fn read_if_there(path: &Path) -> Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| path.display().to_string()),
    }
}

/// The windows of the message boxes Windows shows now, for any program:
/// none of them is the case's.
pub(crate) fn windows_boxes(
    desktop: &mut butterpollo_win_io::nucleus::desktop::Win32,
) -> Result<BTreeSet<u64>> {
    use butterpollo_win_io::nucleus::instances::Desktop;
    Ok(desktop
        .message_boxes(&BTreeSet::new())?
        .into_iter()
        .map(|shown| shown.window)
        .collect())
}

/// Fails while Nucleus Co-op or the game started since `since` shows a
/// message box, naming it; Windows' boxes up `before` the case aside.
pub(crate) fn no_message_box(
    desktop: &mut butterpollo_win_io::nucleus::desktop::Win32,
    before: &BTreeSet<u64>,
    since: u64,
    executable: &str,
) -> Result<()> {
    use butterpollo_win_io::nucleus::instances::Desktop;
    let processes: BTreeSet<u32> = butterpollo_win_io::process::processes()?
        .into_iter()
        .filter(|p| {
            p.started >= since
                && (p.name.eq_ignore_ascii_case("NucleusCoop.exe")
                    || p.name.eq_ignore_ascii_case(executable))
        })
        .map(|p| p.pid)
        .collect();
    let mut shown = desktop.message_boxes(&processes)?;
    shown.retain(|b| !before.contains(&b.window));
    match shown.first() {
        Some(shown) => anyhow::bail!(
            "Nucleus Co-op shows \"{}\" on the PC's screen",
            shown.summary()
        ),
        None => Ok(()),
    }
}

/// Nucleus' `Settings.ini` as it was before a case, put back when the
/// case ends, also on a failure. The session's journal in the host's
/// folder goes with it: it would put back the seeded values over the
/// user's later. A user's copy's file is put back as `user`.
pub(crate) struct KeptSettings {
    pub(crate) path: PathBuf,
    pub(crate) before: Vec<u8>,
    pub(crate) journal: Option<PathBuf>,
    pub(crate) user: Option<std::os::windows::io::OwnedHandle>,
}
impl Drop for KeptSettings {
    fn drop(&mut self) {
        let put_back = || {
            match std::fs::write(&self.path, &self.before) {
                Ok(()) => println!("Settings.ini as before: {}", self.path.display()),
                Err(error) => eprintln!("Settings.ini not put back: {error}"),
            }
            let Some(journal) = &self.journal else {
                return;
            };
            match std::fs::remove_file(journal) {
                Ok(()) => println!("cleanup: {} removed", journal.display()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => eprintln!("{} not removed: {error}", journal.display()),
            }
        };
        if let Err(error) = user_copy::as_user(self.user.as_ref(), put_back) {
            eprintln!("{} not put back: {error:#}", self.path.display());
        }
    }
}
