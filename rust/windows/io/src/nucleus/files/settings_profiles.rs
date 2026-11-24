//! `Settings.ini` completion/journal restoration, `metaInfo.json`, and Rubylight game profile files.

use super::*;

/// The file in Nucleus' folder that keeps the values a session forced in
/// `Settings.ini` until they are put back.
pub(super) fn settings_journal(folder: &Path) -> PathBuf {
    folder.join("Rubylight-settings.json")
}

/// Write `path` below `root` whole: a temporary file, on the disk before it
/// is renamed over it, so a crash or power loss never leaves half a file.
/// Never through a link below `root` ([`super::super::unfollowed`]).
fn write_whole(root: &Path, path: &Path, bytes: &[u8]) -> Result<()> {
    super::super::unfollowed::write(root, path, bytes)
}

/// `Settings.ini` refused when it is UTF-16 text, which the host does not
/// edit ([`profile::utf16_ini`]).
fn ansi_settings(path: &Path, text: &[u8]) -> Result<()> {
    ensure!(
        !profile::utf16_ini(text),
        "{} is saved as UTF-16 (Unicode) text, which Rubylight does not edit: save it as ANSI, or delete it and Rubylight writes Nucleus Co-op's defaults",
        path.display()
    );
    Ok(())
}

/// Put back the user's values of `forced` in `folder`'s `Settings.ini`
/// where the session's still are ([`profile::put_back_settings`]). A file
/// that is gone has nothing to put back.
fn put_back_settings(folder: &Path, forced: &[profile::Forced]) -> Result<usize> {
    let path = folder.join("Settings.ini");
    let text = match std::fs::read(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            tracing::info!(
                file = %path.display(),
                "Nucleus Co-op: Settings.ini is gone, nothing to put back"
            );
            return Ok(0);
        }
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    ansi_settings(&path, &text)?;
    let (text, changed) = profile::put_back_settings(&text, forced);
    if changed > 0 {
        write_whole(folder, &path, &text)?;
    }
    Ok(changed)
}

/// Remove a journal put back from; one already gone is fine.
fn remove_settings_journal(journal: &Path) -> Result<()> {
    let folder = journal.parent().context("the journal has no folder")?;
    super::super::unfollowed::remove(folder, journal).map(drop)
}

/// Put back what a session the host never ended forced in the
/// `Settings.ini` of the Nucleus at `folder`, and remove its journal, as the
/// service starts or sees a sign-in. Returns how many values it put back,
/// or None while a session or another restore holds the journal's lock: a
/// session's values are never put back under it.
pub fn restore_settings_left(folder: &Path) -> Result<Option<usize>> {
    let journal = settings_journal(folder);
    if !journal.exists() {
        return Ok(Some(0));
    }
    let Some(_lock) = Lock::try_take(&journal)? else {
        return Ok(None);
    };
    restore_left_locked(folder).map(Some)
}

/// [`restore_settings_left`] with the journal's lock held.
fn restore_left_locked(folder: &Path) -> Result<usize> {
    let journal = settings_journal(folder);
    let bytes = match std::fs::read(&journal) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "reading {}, which keeps the Nucleus Co-op settings an earlier session changed",
                    journal.display()
                )
            });
        }
    };
    // Created but never written to the disk, as power loss can leave it.
    if bytes.iter().all(u8::is_ascii_whitespace) {
        tracing::warn!(
            file = %journal.display(),
            "Nucleus Co-op: the settings file of an earlier session is empty; removing it"
        );
        remove_settings_journal(&journal)?;
        return Ok(0);
    }
    let forced: Vec<profile::Forced> = serde_json::from_slice(&bytes).map_err(|error| {
        anyhow::anyhow!(
            "{} can't be read ({error}): delete it, then start the stream again. The Nucleus Co-op settings it kept stay as the earlier session set them",
            journal.display()
        )
    })?;
    let changed = put_back_settings(folder, &forced).with_context(|| {
        format!(
            "the Nucleus Co-op settings an earlier session changed were not put back from {}",
            journal.display()
        )
    })?;
    tracing::info!(
        changed,
        "Nucleus Co-op: an earlier session's changes to Settings.ini are put back"
    );
    remove_settings_journal(&journal)?;
    Ok(changed)
}

/// The values a session forced in `Settings.ini`, put back once when it is
/// dropped or [`restore`](SettingsGuard::restore) is called, after Nucleus
/// closed. Should that fail, its journal keeps them for the next session.
/// It holds the journal's lock until it drops, so the service never puts
/// the user's values back under a starting or running session.
pub struct SettingsGuard {
    folder: PathBuf,
    forced: Vec<profile::Forced>,
    restored: bool,
    /// Tests let go of it to stand in for a host that stopped.
    pub(in crate::nucleus) _lock: Option<Lock>,
}

impl SettingsGuard {
    pub fn restore(&mut self) {
        if std::mem::replace(&mut self.restored, true) || self.forced.is_empty() {
            return;
        }
        let journal = settings_journal(&self.folder);
        match put_back_settings(&self.folder, &self.forced) {
            Ok(changed) => {
                tracing::info!(
                    changed,
                    "Nucleus Co-op: Settings.ini is back as the user set it"
                );
                if let Err(error) = remove_settings_journal(&journal) {
                    tracing::warn!(
                        error = format!("{error:#}"),
                        "Nucleus Co-op: the put-back settings' file was not removed"
                    );
                }
            }
            Err(error) => tracing::error!(
                error = format!("{error:#}"),
                file = %journal.display(),
                "Nucleus Co-op: Settings.ini was not put back; the next session tries again"
            ),
        }
    }
}

impl Drop for SettingsGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Set what a session needs in Nucleus' `Settings.ini` (read from Nucleus'
/// working folder), first putting back what an earlier session left
/// ([`restore_settings_left`]). Nucleus stops with a message box on a
/// missing key, so a file missing keys (or missing) is completed from its
/// own defaults ([`profile::settings_ini`]). The user's choices the session
/// changes come back through the returned guard; every other byte stays.
pub fn write_settings(folder: &Path) -> Result<SettingsGuard> {
    let journal = settings_journal(folder);
    let lock = Lock::take(&journal, LOCK_WAIT)?;
    restore_left_locked(folder)?;
    let path = folder.join("Settings.ini");
    let (text, settings) = session_settings(&path)?;
    // The journal first: a host that stops after the next write finds it.
    if !settings.forced.is_empty() {
        write_whole(
            folder,
            &journal,
            &serde_json::to_vec_pretty(&settings.forced)?,
        )?;
    }
    if settings.text != text {
        write_whole(folder, &path, &settings.text)?;
    }
    Ok(SettingsGuard {
        folder: folder.to_path_buf(),
        forced: settings.forced,
        restored: false,
        _lock: Some(lock),
    })
}

/// The `Settings.ini` at `path` (empty when there is none) and what a
/// session makes of it ([`profile::settings_ini`]); refused when it is
/// UTF-16.
fn session_settings(path: &Path) -> Result<(Vec<u8>, profile::Settings)> {
    let text = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    ansi_settings(path, &text)?;
    let settings = profile::settings_ini(&text);
    if !settings.added.is_empty() {
        tracing::info!(
            file = %path.display(),
            keys = settings.added.join(", "),
            "Nucleus Co-op: Settings.ini completed from Nucleus Co-op {}'s defaults",
            butterpollo_session::nucleus::install::VERSION
        );
    }
    Ok((text, settings))
}

/// Set what a session needs in the `Settings.ini` of the user's own copy at
/// `copy` ([`super::super::user_copy`]), called as that user. The copy is
/// Rubylight's and only its sessions run it, so the values stay: no
/// journal, and nothing in the copy is ever put back as the host.
pub fn write_copy_settings(copy: &Path) -> Result<()> {
    let path = copy.join("Settings.ini");
    let (text, settings) = session_settings(&path)?;
    if settings.text != text {
        write_whole(copy, &path, &settings.text)?;
    }
    Ok(())
}

/// Nucleus' per-game settings in the user's profile folder `profile`
/// ([`profile::meta_info_by_index`]).
pub fn meta_info(profile: &Path) -> PathBuf {
    profile.join("NucleusCoop").join("metaInfo.json")
}

/// Give game `guid` its pads by XInput index in the `metaInfo.json` of the
/// profile folder `profile` ([`meta_info`], [`profile::meta_info_by_index`]),
/// called as the user it belongs to. Nucleus' other games and values stay,
/// and so does this one's: like pads by index in `Settings.ini`, it is never
/// put back.
pub fn index_pads(profile: &Path, guid: &str) -> Result<()> {
    let path = &meta_info(profile);
    let text = match std::fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    let Some(changed) = profile::meta_info_by_index(text.as_deref(), guid)
        .map_err(|why| anyhow::anyhow!("{}: {why}; fix or delete it", path.display()))?
    else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        super::super::unfollowed::create_folders(profile, parent)?;
    }
    write_whole(profile, path, &changed)?;
    if text.is_some() {
        tracing::info!(
            file = %path.display(),
            game = guid,
            "Nucleus Co-op: the game's pads go by XInput index again in its per-game settings"
        );
    } else {
        tracing::info!(
            file = %path.display(),
            "Nucleus Co-op: per-game settings started empty, so each game takes pads by index from Settings.ini"
        );
    }
    Ok(())
}

/// A handler's GUID names its profile folder; one that would name another
/// folder is refused.
fn profile_folder(folder: &Path, guid: &str) -> Result<PathBuf> {
    ensure!(
        !guid.trim().is_empty()
            && guid != "."
            && guid != ".."
            && !guid.contains(['\\', '/', ':', '*', '?', '"', '<', '>', '|'])
            && !guid.chars().any(char::is_control),
        "the Nucleus Co-op handler's Game.GUID {guid:?} cannot name a profile folder"
    );
    Ok(folder.join("game profiles").join(guid))
}

/// The files of a game's profile folder in the order Nucleus lists them
/// (`GameProfile.ListGameProfiles`: every file, shortest path first, ties in
/// the folder's name order), with their profile number and title.
fn listed(profiles: &Path) -> Result<Vec<(String, u32, Option<String>)>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(profiles)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let number = profile::file_number(&name).unwrap_or(0);
        // A profile the host cannot read keeps its place; it is just not ours.
        let title = std::fs::read(entry.path())
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .and_then(|json| json.get("Title")?.as_str().map(str::to_string));
        files.push((name, number, title));
    }
    files.sort_by_cached_key(|(name, _, _)| (name.chars().count(), name.to_uppercase()));
    Ok(files)
}

/// Write the session's profile for the game `guid`: the host's own
/// "Rubylight" profile if the game has one, else a new one after the
/// user's. Returns its number. Nucleus' n-th listed entry loads
/// `Profile[n].json`, so a number that is not the file's place in the list
/// would load another profile: that is refused, as is overwriting a profile
/// that is not the host's.
pub fn write_profile(
    folder: &Path,
    guid: &str,
    players: &[Player],
    screens: &[Rect],
    plays_itself: bool,
) -> Result<u32> {
    ensure!(
        !players.is_empty(),
        "a Nucleus Co-op profile needs a player"
    );
    let profiles = profile_folder(folder, guid)?;
    super::super::unfollowed::create_folders(folder, &profiles)?;
    let before = listed(&profiles)?;
    let titles: Vec<(u32, Option<String>)> = before
        .iter()
        .map(|(_, number, title)| (*number, title.clone()))
        .collect();
    let number = profile::number(&titles);
    let name = format!("Profile[{number}].json");
    let ours = match before
        .iter()
        .find(|(file, _, _)| file.eq_ignore_ascii_case(&name))
    {
        Some((_, _, title)) if title.as_deref() != Some(profile::TITLE) => bail!(
            "Nucleus Co-op's profile {name} for {guid} is not Rubylight's: delete or renumber the game's profiles in Nucleus Co-op"
        ),
        Some(_) => true,
        None => false,
    };
    let path = profiles.join(&name);
    let text = serde_json::to_string_pretty(&profile::build(players, screens, plays_itself))?;
    super::super::unfollowed::write(folder, &path, text.as_bytes())?;
    let after = listed(&profiles)?;
    let place = after.iter().position(|(file, _, _)| *file == name);
    // A `Profile[0].json` titled as ours has no place Nucleus Co-op loads it from.
    if place.is_none() || place != (number as usize).checked_sub(1) {
        if !ours {
            // The refusal says what to do; a profile left behind is listed.
            let _ = super::super::unfollowed::remove(folder, &path);
        }
        let files: Vec<&str> = after.iter().map(|(file, _, _)| file.as_str()).collect();
        bail!(
            "Nucleus Co-op would not load Rubylight's profile {name} for {guid} (its folder lists {}): delete or renumber the game's profiles in Nucleus Co-op",
            files.join(", ")
        );
    }
    Ok(number)
}
