//! Manifest-driven removal, OptiScaler.ini restoration, and empty-folder
//! cleanup for Neural Rendering game folders.
use super::{
    Failure, LINK, Outcome, PARTIAL, PROBE, Response, Writer, cleanup::left_in, guard, manifest,
    state, writable, write_named, write_whole,
};
use butterpollo_session::neural::{self, LEGACY_MANIFEST, MANIFEST};
use std::{
    os::windows::fs::MetadataExt,
    path::{Path, PathBuf},
};

/// Delete what the host placed in the folder, by each of its manifests:
/// this release's and Rubylight 3.0's AMDNR, whose record stays for what
/// it leaves when the install goes `elsewhere` ([`Response::Left`]); with
/// `earlier`, only AMDNR.
pub fn remove(folder: &Path, writer: Writer, elsewhere: bool, earlier: bool) -> Outcome<Response> {
    // A folder that is gone with its game holds nothing; one on a drive that
    // is not connected may, and is not called removed. So may a game folder
    // moved and linked back whose link leads nowhere now ([`super::followed`]).
    let dangling = std::fs::symlink_metadata(folder)
        .is_ok_and(|meta| meta.file_attributes() & LINK != 0)
        && std::fs::canonicalize(folder).is_err();
    if dangling || (!folder.is_dir() && folder.ancestors().last().is_none_or(|root| !root.is_dir()))
    {
        return Ok(Response::Unreachable {
            folder: folder.display().to_string(),
        });
    }
    let mut kept = if earlier {
        Vec::new()
    } else {
        remove_by(folder, MANIFEST, writer, false)?
    };
    let left = remove_by(folder, LEGACY_MANIFEST, writer, elsewhere)?;
    let record = elsewhere && !left.is_empty();
    kept.extend(left);
    Ok(if record {
        Response::Left { kept }
    } else {
        Response::Removed { kept }
    })
}

/// Delete what the manifest `name` names and is still as placed, the DLLs
/// first ([`neural::Removal::delete`]), so a game that holds them stops the
/// removal before anything changed; then put the
/// user's OptiScaler.ini lines back, delete OptiScaler's logs, what the
/// provider wrote in its folder ([`neural::provider_files`]) and the folders
/// it created once empty. A file someone changed stays, as do the settings
/// it took over and anything else in those folders ([`left_in`]); all are
/// returned. With `keep_record`, a manifest whose removal leaves something
/// stays, naming only what is left.
fn remove_by(folder: &Path, name: &str, writer: Writer, keep_record: bool) -> Outcome<Vec<String>> {
    guard(folder, name, writer)?;
    let Some(manifest) = manifest(folder, name).map_err(Failure::Error)? else {
        return Ok(Vec::new());
    };
    writable(folder, writer)?;
    // A link anywhere refuses the removal before anything goes.
    for path in manifest
        .files
        .keys()
        .chain(&manifest.folders)
        .chain(&manifest.taken_over)
        .chain(manifest.ini.iter().map(|ini| &ini.path))
    {
        guard(folder, path, writer)?;
    }
    // The user's ini as it goes back, before anything changes: one that
    // can't be put back stops the removal with nothing gone.
    let ini = match &manifest.ini {
        Some(record) => match std::fs::read(folder.join(&record.path)) {
            Ok(bytes) => {
                let restored = neural::ini_restore(&bytes, record).map_err(Failure::Error)?;
                (restored != bytes).then_some((&record.path, restored))
            }
            // The user removed OptiScaler meanwhile: nothing to put back.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                return Err(Failure::Error(format!(
                    "cannot read {}: {e}",
                    folder.join(&record.path).display()
                )));
            }
        },
        None => None,
    };
    let removal = neural::removal(&manifest, |path| state(folder, &manifest, path, false));
    let (dlls, others): (Vec<&String>, Vec<&String>) = removal
        .delete
        .iter()
        .partition(|path| path.to_ascii_lowercase().ends_with(".dll"));
    let delete = |path: &str| {
        guard(folder, path, writer)?;
        let file = folder.join(path);
        std::fs::remove_file(&file).map_err(|e| {
            // A loaded DLL refuses its delete (access denied), a file held
            // open without delete sharing too (a sharing violation).
            let held = matches!(e.raw_os_error(), Some(5 | 32));
            Failure::Error(if held {
                format!(
                    "cannot remove {}: a program holds it, most likely the game: close the game and try again",
                    file.display()
                )
            } else {
                format!("cannot remove {}: {e}", file.display())
            })
        })
    };
    // The DLLs all go or none does: a game that holds one (in add-on mode the
    // provider's, with no proxy of ours to stop at first) leaves the folder
    // and the user's ini as they were.
    for path in &dlls {
        guard(folder, path, writer)?;
    }
    let dlls: Vec<PathBuf> = dlls.iter().map(|path| folder.join(path)).collect();
    butterpollo_win_io::files::delete_all(&dlls).map_err(|(file, e)| {
        let held = matches!(e.raw_os_error(), Some(5 | 32));
        Failure::Error(if held {
            format!(
                "cannot remove {}: a program holds it, most likely the game: close the game and try again",
                file.display()
            )
        } else {
            format!("cannot remove {}: {e}", file.display())
        })
    })?;
    // Then the ini: OptiScaler, gone or no longer loaded, looks for the
    // provider no more.
    if let Some((path, restored)) = &ini {
        write_whole(folder, path, restored, writer)?;
    }
    for path in others {
        delete(path)?;
    }
    let mut leftovers: Vec<String> = manifest
        .files
        .keys()
        .map(|path| format!("{path}{PARTIAL}"))
        .collect();
    leftovers.extend(
        std::fs::read_dir(folder)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| neural::logs(name)),
    );
    // What the provider wrote in its dlssg folder, when the host created
    // that folder: its logs and its copy of the Vulkan loader.
    for dlssg in manifest
        .folders
        .iter()
        .filter(|relative| neural::provider_dlssg(relative))
    {
        leftovers.extend(
            std::fs::read_dir(folder.join(dlssg))
                .into_iter()
                .flatten()
                .flatten()
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| neural::provider_files(name))
                .map(|name| format!("{dlssg}/{name}")),
        );
    }
    leftovers.extend([format!("{name}{PARTIAL}"), PROBE.into()]);
    if let Some(record) = &manifest.ini {
        leftovers.push(format!("{}{PARTIAL}", record.path));
    }
    for path in leftovers {
        guard(folder, &path, writer)?;
        let _ = std::fs::remove_file(folder.join(path));
    }
    // Deepest first; a folder that still holds someone's files stays.
    for relative in manifest.folders.iter().rev() {
        guard(folder, relative, writer)?;
        let _ = std::fs::remove_dir(folder.join(relative));
    }
    let mut kept = removal.keep;
    kept.extend(left_in(folder, &manifest.folders));
    kept.sort();
    kept.dedup();
    if keep_record && !kept.is_empty() {
        // What a later removal needs: the files and folders left, with
        // nothing more to put back.
        let left = neural::retained_manifest(manifest, &kept, |relative| {
            folder.join(relative).is_dir()
        });
        return write_named(folder, name, &left, writer).map(|()| kept);
    }
    guard(folder, name, writer)?;
    let path = folder.join(name);
    std::fs::remove_file(&path)
        .map_err(|e| Failure::Error(format!("cannot remove {}: {e}", path.display())))?;
    Ok(kept)
}
