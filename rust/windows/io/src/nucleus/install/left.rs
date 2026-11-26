//! Validating and holding Nucleus Co-op folders from earlier sessions before
//! restoring leftover settings.
use super::{
    Checks, DIRECTORY, REPARSE_POINT, above, attributes_of, normal, open_checked, placed, verdict,
};
use anyhow::{Context, Result, bail};
use butterpollo_session::nucleus::install as pinned;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// The Nucleus folders an earlier session may have left settings in to put
/// back: the one `configured` names (`nucleus_path`) and the copy an earlier
/// Rubylight installed below `data`, each that is there, by the path Windows
/// resolved. Unlike [`super::installed`], neither need hold the pinned release:
/// putting back runs nothing, so the user's settings come back from a folder
/// Rubylight no longer runs Nucleus from. Each is otherwise checked as
/// [`super::chosen`] checks one, since SYSTEM writes there and applies what it
/// holds to the user's hive: on a fixed drive inside this PC, named by its
/// own full path, no link in it or on its way, below folders only SYSTEM,
/// Administrators and TrustedInstaller may move, and owned and changed only
/// by them, so nobody else could have put a file there or swap the folder.
/// Each stays open ([`Left`]) until its restore is done. An error per
/// folder refused.
pub fn left_in(data: &Path, configured: &str) -> Vec<Result<Left>> {
    left_in_checked(data, configured, &Checks::pc())
}

/// A Nucleus folder [`left_in`] accepted, held open: it can't be renamed,
/// deleted or replaced while this lives. Restore from [`Left::folder`].
pub struct Left {
    folder: PathBuf,
    _handle: std::fs::File,
}
impl Left {
    /// The folder by the path Windows resolved it to.
    pub fn folder(&self) -> &Path {
        &self.folder
    }
}

pub(super) fn left_in_checked(data: &Path, configured: &str, checks: &Checks) -> Vec<Result<Left>> {
    let configured = configured.trim();
    let mut candidates = Vec::new();
    if !configured.is_empty() {
        candidates.push(PathBuf::from(configured));
    }
    candidates.push(pinned::folder(data));
    let mut seen: Vec<String> = Vec::new();
    let mut folders = Vec::new();
    for folder in candidates {
        let checked = (|| -> Result<Option<Left>> {
            // Before anything touches it: SYSTEM never reaches out to a
            // share, or a drive that could be swapped.
            placed(&folder, checks)?;
            let handle = match crate::process::open_unfollowed(&folder, true) {
                Ok(handle) => handle,
                // Nothing there: nothing to put back. Any other error is one.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => {
                    return Err(error).with_context(|| format!("opening {}", folder.display()));
                }
            };
            let attributes = attributes_of(&handle, &folder)?;
            if attributes & REPARSE_POINT != 0 {
                bail!("{} is a link to elsewhere", folder.display());
            }
            if attributes & DIRECTORY == 0 {
                return Ok(None);
            }
            let resolved = normal(
                &crate::user_folder::final_path(&handle).context("reading the folder's path")?,
            );
            if !crate::user_folder::same_name(
                std::ffi::OsStr::new(&resolved),
                std::ffi::OsStr::new(&normal(folder.as_os_str())),
            ) {
                bail!(
                    "{} leads to {resolved} through a link or a short name",
                    folder.display()
                );
            }
            let resolved = PathBuf::from(resolved);
            above(&resolved, checks)?;
            let owner = crate::process::open_file_owner(&handle)
                .with_context(|| format!("reading the owner of {}", resolved.display()))?;
            let writers = crate::process::open_file_writers(&handle)
                .with_context(|| format!("reading the permissions of {}", resolved.display()))?;
            verdict(&resolved, &owner, &writers, |sid| {
                checks.trusts(&resolved, sid)
            })?;
            kept_trusted(&resolved, checks)?;
            Ok(Some(Left {
                folder: resolved,
                _handle: handle,
            }))
        })()
        .with_context(|| {
            format!(
                "Rubylight doesn't put back the settings a co-op session left in {}",
                folder.display()
            )
        });
        match checked {
            Ok(None) => {}
            Ok(Some(left)) => {
                let key = left.folder.to_string_lossy().to_lowercase();
                if !seen.contains(&key) {
                    seen.push(key);
                    folders.push(Ok(left));
                }
            }
            Err(error) => folders.push(Err(error)),
        }
    }
    folders
}

/// Whether `name` is one a restore from a [`Left`] folder reads or writes:
/// the journals of [`crate::nucleus::files`] and [`crate::nucleus::layers`], and the
/// `Settings.ini` it puts back with its temporary file.
fn kept(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    matches!(
        name.as_str(),
        "rubylight-settings.json" | "settings.ini" | "settings.ini.rubylight"
    ) || (name.starts_with("rubylight-layers-") && name.ends_with(".json"))
}

/// Each file a restore reads or writes in `folder` ([`kept`]), checked as
/// [`super::walk`] checks the folder Nucleus runs from, never following a link: no
/// link, owned and changed only by those `checks` trusts. The folder is
/// held open and only they may change it, so none of these is replaced or
/// changed by anyone else before the restore is done.
fn kept_trusted(folder: &Path, checks: &Checks) -> Result<()> {
    let entries =
        std::fs::read_dir(folder).with_context(|| format!("listing {}", folder.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("listing {}", folder.display()))?;
        if !kept(&entry.file_name().to_string_lossy()) {
            continue;
        }
        let path = folder.join(entry.file_name());
        let listed = entry
            .metadata()
            .with_context(|| format!("reading {}", path.display()))?
            .file_attributes();
        let file = match open_checked(&path, listed & DIRECTORY != 0) {
            Ok(file) => file,
            // Gone since it was listed: a session's restore removed it.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("opening {}", path.display()));
            }
        };
        if (listed | attributes_of(&file, &path)?) & REPARSE_POINT != 0 {
            bail!("{} is a link to elsewhere", path.display());
        }
        let owner = crate::process::open_file_owner(&file)
            .with_context(|| format!("reading the owner of {}", path.display()))?;
        let writers = crate::process::open_file_writers(&file)
            .with_context(|| format!("reading the permissions of {}", path.display()))?;
        verdict(&path, &owner, &writers, |sid| checks.trusts(&path, sid))?;
    }
    Ok(())
}
