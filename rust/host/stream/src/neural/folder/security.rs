//! Reparse-point guards, Windows ACL inspection, and administrator-only write
//! checks for Neural Rendering game and cache folders.
use super::{Failure, LINK, Outcome, PROBE, Rights, Security, Writer, is_clean_path};
use butterpollo_session::neural;
use std::{os::windows::fs::MetadataExt, path::Path};

/// Refuse a link on the way from the game's folder to `relative` in it, the
/// two included: a junction or symbolic link planted there would send a
/// copy or a delete elsewhere. A SYSTEM host also refuses a folder on the
/// way that anyone but administrators may change.
pub(super) fn guard(folder: &Path, relative: &str, writer: Writer) -> Outcome<()> {
    if !is_clean_path(folder) {
        return Err(Failure::Refused(format!(
            "{} is not a plain path on this PC",
            folder.display()
        )));
    }
    let parts: Vec<&str> = relative
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    if parts
        .iter()
        .any(|part| *part == "." || *part == ".." || part.contains('\\'))
    {
        return Err(Failure::Refused(format!(
            "{relative} is not a path inside {}",
            folder.display()
        )));
    }
    let mut path = folder.to_path_buf();
    for depth in 0..=parts.len() {
        if depth > 0 {
            path.push(parts[depth - 1]);
        }
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => {
                return Err(Failure::Error(format!(
                    "cannot read {}: {e}",
                    path.display()
                )));
            }
        };
        if meta.file_attributes() & LINK != 0 {
            return Err(Failure::Refused(format!(
                "{} is a link to elsewhere, and Rubylight changes nothing through one",
                path.display()
            )));
        }
        if writer == Writer::System && depth > 0 && meta.is_dir() {
            security(&path, Rights::Change)
                .map_err(|e| {
                    Failure::Error(format!(
                        "cannot check who may change {}: {e:#}",
                        path.display()
                    ))
                })
                .and_then(|security| verdict(&path, &security).map_err(Failure::Refused))?;
        }
    }
    Ok(())
}

pub fn security(path: &Path, rights: Rights) -> anyhow::Result<Security> {
    use butterpollo_win_io::process::{
        open_file_child_movers, open_file_movers, open_file_owner, open_file_writers,
        open_unfollowed,
    };
    let file = open_unfollowed(path, true)?;
    Ok(Security {
        link: file.metadata()?.file_attributes() & LINK != 0,
        owner: open_file_owner(&file)?,
        writers: match rights {
            Rights::Change => open_file_writers(&file),
            Rights::MoveContents => open_file_child_movers(&file),
            Rights::Move => open_file_movers(&file),
        }?,
    })
}

/// An account as the console names it: `DOMAIN\name`, or "another account"
/// for one Windows doesn't know or a permission entry naming none.
pub fn account(sid: &str) -> String {
    butterpollo_win_io::process::account_name(sid).unwrap_or_else(|| "another account".into())
}

/// Whether only administrators may change `path`.
pub fn verdict(path: &Path, security: &Security) -> Result<(), String> {
    neural::security_verdict(&path.display().to_string(), security, account)
}

/// Whether administrators own `path` and alone may change it, or for a
/// folder above the cache (not `whole`) move or delete it or what it holds
/// ([`Rights::Move`], [`verdict`]); the cache's check ([`crate::neural::trusted_root`]).
pub(crate) fn administrators_hold(path: &Path, whole: bool) -> Result<(), String> {
    hold(path, whole, security)
}

/// [`administrators_hold`], with `read` in place of [`security`].
pub fn hold(
    path: &Path,
    whole: bool,
    read: impl FnOnce(&Path, Rights) -> anyhow::Result<Security>,
) -> Result<(), String> {
    let rights = if whole { Rights::Change } else { Rights::Move };
    let security = read(path, rights)
        .map_err(|e| format!("cannot check who may change {}: {e:#}", path.display()))?;
    verdict(path, &security)
}

/// A SYSTEM host's check before it writes `folder` for the user: it and
/// every folder above it, the drive's root included, are no links and
/// nobody but administrators may change them. Otherwise an ordinary account
/// could swap one for a link and send SYSTEM's copies anywhere.
pub fn administrators_only(folder: &Path) -> Result<(), String> {
    if !is_clean_path(folder) || !folder.is_absolute() {
        return Err(format!("{} is not a plain folder path", folder.display()));
    }
    for path in folder.ancestors().filter(|p| !p.as_os_str().is_empty()) {
        let rights = if path.parent().is_none() {
            Rights::MoveContents
        } else {
            Rights::Change
        };
        let security = security(path, rights)
            .map_err(|e| format!("cannot check who may change {}: {e:#}", path.display()))?;
        verdict(path, &security)?;
    }
    Ok(())
}

/// Whether this process may change the game's folder. The user's helper
/// tries, and answers [`Failure::NotWritable`] for a folder only
/// administrators may write; a SYSTEM host writes only such a folder, and
/// only when nobody else may change it ([`administrators_only`]).
pub fn writable(folder: &Path, writer: Writer) -> Outcome<()> {
    match writer {
        Writer::User => {
            guard(folder, PROBE, writer)?;
            let probe = folder.join(PROBE);
            let tried = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&probe)
                .and_then(|file| {
                    drop(file);
                    std::fs::remove_file(&probe)
                });
            match tried {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    Err(Failure::NotWritable(folder.to_path_buf()))
                }
                Err(e) => Err(Failure::Error(format!(
                    "cannot write {}: {e}",
                    folder.display()
                ))),
            }
        }
        Writer::System => administrators_only(folder).map_err(|problem| {
            Failure::Refused(format!(
                "you may not change the game's folder {}, and Rubylight changes it for you only when nobody but administrators may: {problem}",
                folder.display()
            ))
        }),
    }
}
