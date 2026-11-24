//! Checked file staging, atomic placement, pre-install checking, and pre-launch
//! verification for Neural Rendering game folders.
use super::{
    Cached, Failure, Outcome, PARTIAL, Response, Writer,
    find::refuse_anti_cheat,
    folder_of, guard, kept_text,
    layout::{Gone, layout},
    manifest, matched, partial, remove, same_folder, state, writable, write_manifest, write_whole,
};
use butterpollo_session::neural::{
    self, LEGACY_MANIFEST, MANIFEST, Manifest, Mode, Now, PLACED_BY, Placed, SETTINGS,
};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::Path,
};

/// Copy `source` to `target`'s partial file, checking it against the package
/// as it goes; a tampered cache never reaches the game.
fn copy_checked(
    source: &Path,
    target: &Path,
    placed: &Placed,
    done: &mut u64,
    progress: &dyn Fn(u64),
) -> Result<(), String> {
    let partial = partial(target);
    let _ = std::fs::remove_file(&partial);
    let result = (|| {
        let mut input = std::fs::File::open(source)?;
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&partial)?;
        let mut hash = Sha256::new();
        let mut buffer = vec![0; 1 << 20];
        let mut size = 0;
        loop {
            let read = input.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            size += read as u64;
            if size > placed.size {
                break;
            }
            hash.update(&buffer[..read]);
            output.write_all(&buffer[..read])?;
            *done += read as u64;
            progress(*done);
        }
        output.sync_all()?;
        drop(output);
        if size != placed.size || hex::encode(hash.finalize()) != placed.sha256 {
            return Err(std::io::Error::other(format!(
                "the cached {} does not match its release",
                source.display()
            )));
        }
        Ok(())
    })();
    result.map_err(|e| {
        let _ = std::fs::remove_file(&partial);
        format!("cannot place {}: {e}", target.display())
    })
}

pub fn place(
    exe: &Path,
    cached: &Cached,
    writer: Writer,
    progress: &dyn Fn(u64, u64),
) -> Outcome<Response> {
    place_or_check(exe, cached, writer, progress, None)
}

/// [`super::Request::Check`]: what [`place`] refuses before it writes anything,
/// with Rubylight 3.0's AMDNR beside `legacy` (else `exe`) taken as
/// removed. A file its removal would keep stops the install only in the
/// folder the install goes to; AMDNR in another folder leaves this one as
/// it is.
pub fn check(
    exe: &Path,
    legacy: Option<&Path>,
    cached: &Cached,
    writer: Writer,
) -> Outcome<Response> {
    let folder = legacy.map_or(folder_of(exe), folder_of);
    let (gone, kept) = Gone::of_legacy(folder, writer)?;
    if !same_folder(folder, folder_of(exe)) {
        return place_or_check(exe, cached, writer, &|_, _| {}, Some(&Gone::default()));
    }
    if !kept.is_empty() {
        return Err(Failure::Refused(kept_text(&kept, false)));
    }
    place_or_check(exe, cached, writer, &|_, _| {}, Some(&gone))
}

/// [`place`], or with `check` only what it refuses before it writes,
/// counting what `check` names as not there.
fn place_or_check(
    exe: &Path,
    cached: &Cached,
    writer: Writer,
    progress: &dyn Fn(u64, u64),
    check: Option<&Gone>,
) -> Outcome<Response> {
    let folder = folder_of(exe);
    let package = &cached.package;
    let nothing = Gone::default();
    let gone = check.unwrap_or(&nothing);
    guard(folder, MANIFEST, writer)?;
    let old = manifest(folder, MANIFEST).map_err(Failure::Error)?;
    let layout = layout(folder, exe, old.as_ref(), writer, gone)?;
    writable(folder, writer)?;
    // The user's OptiScaler.ini with the three values set, its lines as
    // they were kept from the first install.
    let ini = if layout.mode == Mode::Addon {
        guard(folder, SETTINGS, writer)?;
        let bytes = std::fs::read(folder.join(SETTINGS))
            .map_err(|e| Failure::Error(format!("cannot read {SETTINGS}: {e}")))?;
        let (changed, record) =
            neural::ini_apply(&bytes, SETTINGS, old.as_ref().and_then(|m| m.ini.as_ref()))
                .map_err(Failure::Refused)?;
        Some((bytes != changed, changed, record))
    } else {
        None
    };
    // Each file of the package where it goes. A file already as this
    // release has it stays; one to copy keeps the form the release before
    // left, if any.
    let mut wanted = Vec::new();
    let mut taken_over = Vec::new();
    let mut plan = Vec::new();
    for (path, placed) in &package.files {
        let Some(target) = neural::target(layout.mode, path, &layout.proxy, &layout.opti_dir)
        else {
            continue;
        };
        guard(folder, &target, writer)?;
        let forms: Vec<&Placed> = old
            .iter()
            .flat_map(|m| [m.files.get(&target), m.previous.get(&target)])
            .flatten()
            .collect();
        // Settings found in a folder an earlier install left are the user's
        // (Rubylight's own removal takes the ones it placed): the install
        // uses them as they are, and the manifest names them apart from the
        // files it placed, so a removal leaves them, as does a first install
        // cut short. An update keeps them so.
        let in_taken_over = target.rsplit_once('/').is_some_and(|(parent, _)| {
            layout
                .adopt
                .iter()
                .any(|relative| relative.eq_ignore_ascii_case(parent))
        });
        if forms.is_empty()
            && (in_taken_over || old.as_ref().is_some_and(|m| m.taken_over.contains(&target)))
            && neural::settings(&target)
            && !gone.has(&target)
            && std::fs::symlink_metadata(folder.join(&target)).is_ok_and(|meta| meta.is_file())
        {
            taken_over.push(target);
            continue;
        }
        if forms.is_empty()
            && !gone.has(&target)
            && std::fs::symlink_metadata(folder.join(&target)).is_ok()
        {
            return Err(Failure::Refused(format!(
                "{target} is already in the game's folder and is not Rubylight's"
            )));
        }
        // What the check takes as gone is copied anew.
        let before = match matched(folder, &target, &forms, false) {
            _ if gone.has(&target) => None,
            Ok(form) if form == placed => {
                wanted.push((target, placed.clone()));
                continue;
            }
            Ok(form) => Some(form.clone()),
            Err(Now::Changed) if neural::settings(&target) => {
                // The user's settings stay through an update.
                continue;
            }
            Err(Now::Changed) => {
                return Err(Failure::Error(format!(
                    "{target} changed since Rubylight placed it in {}: turn frame generation off and on again",
                    folder.display()
                )));
            }
            Err(_) => None,
        };
        plan.push((path.clone(), target.clone(), placed.clone(), before));
        wanted.push((target, placed.clone()));
    }
    // Folders an earlier install left, then those the files need that are
    // not there yet, parents first.
    let mut folders = old.as_ref().map(|m| m.folders.clone()).unwrap_or_default();
    for relative in &layout.adopt {
        if !folders.contains(relative) {
            folders.push(relative.clone());
        }
    }
    let mut make = Vec::new();
    for (_, target, _, _) in &plan {
        let parts: Vec<&str> = target.split('/').collect();
        for end in 1..parts.len() {
            let relative = parts[..end].join("/");
            let path = folder.join(&relative);
            if make.contains(&relative) {
                continue;
            }
            if !path.exists() || gone.has(&relative) {
                if !folders.contains(&relative) {
                    folders.push(relative.clone());
                }
                make.push(relative);
            } else if !path.is_dir() {
                return Err(Failure::Refused(format!(
                    "{relative} in the game's folder is not a folder"
                )));
            }
        }
    }
    if check.is_some() {
        return Ok(Response::Found {
            exe: exe.to_path_buf(),
        });
    }
    let mut files = old.as_ref().map(|m| m.files.clone()).unwrap_or_default();
    let mut previous = old.as_ref().map(|m| m.previous.clone()).unwrap_or_default();
    for (target, placed) in &wanted {
        files.insert(target.clone(), placed.clone());
        previous.remove(target);
    }
    for (_, target, _, before) in &plan {
        if let Some(before) = before {
            previous.insert(target.clone(), before.clone());
        }
    }
    let updated = old.as_ref().is_some_and(|m| m.release != package.tag);
    let first = old.is_none();
    // The manifest names every file and folder before one is made, each
    // file it replaces in both forms, and the ini's lines before they
    // change; whatever a failure or a kill leaves is the host's, and a
    // removal finds it.
    let mut manifest = Manifest {
        by: PLACED_BY.into(),
        release: old.map_or_else(|| package.tag.clone(), |m| m.release),
        proxy: layout.proxy.clone(),
        mode: layout.mode,
        switches: vec![neural::Switch::FrameGeneration],
        files,
        previous,
        folders,
        taken_over,
        ini: ini.as_ref().map(|(_, _, record)| record.clone()),
    };
    write_manifest(folder, &manifest, writer)?;
    // What goes in last: the game loads frame generation only once it is
    // all there. OptiScaler for the full package; the provider, then the
    // ini that names it, for the add-on.
    let last = match layout.mode {
        Mode::Full => layout.proxy.clone(),
        Mode::Addon => neural::target(
            Mode::Addon,
            &format!("addon/{}/{}", neural::PROVIDER, neural::PROVIDER_DLL),
            "",
            &layout.opti_dir,
        )
        .unwrap_or_default(),
    };
    let placing = (|| {
        for relative in &make {
            guard(folder, relative, writer)?;
            let path = folder.join(relative);
            std::fs::create_dir(&path)
                .map_err(|e| Failure::Error(format!("cannot create {}: {e}", path.display())))?;
        }
        // Each file whole and checked beside its place first, then all in
        // place at once.
        let total: u64 = plan.iter().map(|(_, _, placed, _)| placed.size).sum();
        let mut done = 0;
        for (source, target, placed, _) in &plan {
            guard(folder, target, writer)?;
            guard(folder, &format!("{target}{PARTIAL}"), writer)?;
            let source = cached.folder.join("files").join(source);
            copy_checked(&source, &folder.join(target), placed, &mut done, &|done| {
                progress(done, total)
            })
            .map_err(Failure::Error)?;
        }
        let (dll, others): (Vec<_>, Vec<_>) =
            plan.iter().partition(|(_, target, _, _)| *target == last);
        for (_, target, _, _) in others.into_iter().chain(dll) {
            guard(folder, target, writer)?;
            let path = folder.join(target);
            std::fs::rename(partial(&path), &path)
                .map_err(|e| Failure::Error(format!("cannot place {}: {e}", path.display())))?;
        }
        if let Some((true, changed, _)) = &ini {
            write_whole(folder, SETTINGS, changed, writer)?;
        }
        // Files of the release before that this one no longer has.
        let stale = neural::stale_manifest_paths(&manifest, &wanted);
        for path in stale {
            let forms: Vec<&Placed> = manifest
                .files
                .get(&path)
                .into_iter()
                .chain(manifest.previous.get(&path))
                .collect();
            let form = match matched(folder, &path, &forms, false) {
                Ok(form) => form.clone(),
                Err(Now::Missing) => {
                    manifest.files.remove(&path);
                    continue;
                }
                // Someone's since: it stays, for a removal to keep too.
                Err(_) => continue,
            };
            guard(folder, &path, writer)?;
            match std::fs::remove_file(folder.join(&path)) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    // Still the host's, as it is now: a removal takes it.
                    manifest.files.insert(path, form);
                }
                _ => {
                    manifest.files.remove(&path);
                }
            }
        }
        manifest.release = package.tag.clone();
        manifest.previous.clear();
        write_manifest(folder, &manifest, writer)
    })();
    if let Err(failure) = placing {
        for (_, target, _, _) in &plan {
            if guard(folder, &format!("{target}{PARTIAL}"), writer).is_ok() {
                let _ = std::fs::remove_file(partial(&folder.join(target)));
            }
        }
        // A first install cut short goes again, so the switch stays off with
        // nothing left behind. An update cut short leaves each file it
        // replaces in one of its two forms: the next launch finishes it, and
        // turning the switch off removes either.
        if first && let Err(left) = remove(folder, writer, false, false) {
            return Err(Failure::Error(format!(
                "{}, and what was placed could not be removed again: {}",
                failure.message(),
                left.message()
            )));
        }
        return Err(failure);
    }
    Ok(Response::Placed {
        exe: exe.to_path_buf(),
        release: package.tag.clone(),
        proxy: layout.proxy,
        updated,
        mode: layout.mode,
    })
}

pub fn prepare(
    exe: &Path,
    install: Option<&Path>,
    package: Option<Cached>,
    writer: Writer,
    progress: &dyn Fn(u64, u64),
) -> Outcome<Response> {
    let folder = folder_of(exe);
    refuse_anti_cheat(exe, install)?;
    // This release's files, else Rubylight 3.0's AMDNR, which nothing updates.
    let (manifest, legacy) = match manifest(folder, MANIFEST).map_err(Failure::Error)? {
        Some(manifest) => (manifest, false),
        None => match manifest(folder, LEGACY_MANIFEST).map_err(Failure::Error)? {
            Some(manifest) => (manifest, true),
            None => {
                return Err(Failure::Error(format!(
                    "Neural Rendering's files are gone from {}: turn it off and on again",
                    folder.display()
                )));
            }
        },
    };
    // The rest of an update cut short; another release is never applied
    // on its own.
    if let Some(cached) = package.filter(|_| !legacy && !manifest.previous.is_empty()) {
        cached
            .package
            .validate_paths(&cached.folder.display().to_string())
            .map_err(Failure::Error)?;
        return place(exe, &cached, writer, progress);
    }
    if !manifest.previous.is_empty() {
        return Err(Failure::Error(format!(
            "an update of Neural Rendering in {} was cut short: turn it off and on again",
            folder.display()
        )));
    }
    let folder_display = folder.display().to_string();
    for path in manifest.files.keys() {
        let now = state(folder, &manifest, path, true);
        neural::prepare_file_check(path, now, &folder_display).map_err(Failure::Error)?;
    }
    // The user's settings it found are used as they are, but are needed:
    // turning it off and on again places the release's.
    for path in &manifest.taken_over {
        if std::fs::symlink_metadata(folder.join(path)).is_err() {
            return Err(Failure::Error(format!(
                "Neural Rendering's {path} is missing in {folder_display}: turn it off and on again"
            )));
        }
    }
    Ok(Response::Placed {
        exe: exe.to_path_buf(),
        release: manifest.release,
        proxy: manifest.proxy,
        updated: false,
        mode: manifest.mode,
    })
}
