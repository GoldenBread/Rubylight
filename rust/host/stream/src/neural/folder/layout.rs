//! Install layout selection (full OptiScaler vs user add-on), leftover folder
//! adoption, and legacy AMDNR removal simulation for Neural Rendering folders.
use super::{ENTRIES, Failure, LINK, Outcome, PARTIAL, Writer, guard, manifest, state};
use crate::os_ext::MetadataExt;
use butterpollo_session::neural::{
    self, LEGACY_MANIFEST, Manifest, Mode, Now, Occupant, SETTINGS, in_the_way,
};
use std::path::Path;

/// What has `name` beside the game's .exe.
fn occupant(folder: &Path, name: &str, manifest: Option<&Manifest>) -> Occupant {
    let path = folder.join(name);
    if !path.exists() {
        return Occupant::Free;
    }
    if let Some(manifest) = manifest
        && manifest.mode == Mode::Full
        && manifest.proxy == name
        && manifest.files.contains_key(name)
        && state(folder, manifest, name, false) == Now::Same
    {
        return Occupant::Ours;
    }
    match std::fs::read(&path) {
        Ok(bytes) if neural::is_optiscaler_binary(&bytes) => Occupant::OptiScaler,
        _ => Occupant::Foreign,
    }
}

/// Where and how the files go: the full package under a free proxy name,
/// or the add-on into the OptiScaler the user installed.
pub(super) struct Layout {
    pub mode: Mode,
    /// OptiScaler's name: the one the host gives it, or the user's own.
    pub proxy: String,
    /// OptiScaler's folder (with `/`, empty for the game folder).
    pub opti_dir: String,
    /// Folders an earlier install left behind ([`leftover`]), parents
    /// first: the new manifest takes them over as its own.
    pub adopt: Vec<String>,
}

/// The folder `root` in the game's folder as an earlier install left it
/// once its manifest went: `Ok` with its folders, parents first, when it
/// holds nothing but folders, what the provider writes in its own folder
/// ([`neural::provider_files`]) and the provider's settings, which the
/// install keeps as they are ([`super::place`]); otherwise `Err` with what else is
/// in it. A link in it is never Rubylight's.
fn leftover(
    folder: &Path,
    root: &str,
    writer: Writer,
    gone: &Gone,
) -> Outcome<Result<Vec<String>, Vec<String>>> {
    guard(folder, root, writer)?;
    let mut folders = vec![root.to_owned()];
    let mut foreign = Vec::new();
    let mut seen = 0;
    let mut next = 0;
    while let Some(relative) = folders.get(next).cloned() {
        next += 1;
        guard(folder, &relative, writer)?;
        let entries = std::fs::read_dir(folder.join(&relative)).map_err(|e| {
            Failure::Error(format!(
                "cannot read {}: {e}",
                folder.join(&relative).display()
            ))
        })?;
        for entry in entries.flatten() {
            seen += 1;
            if seen > ENTRIES {
                return Ok(Err(vec![root.to_owned()]));
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = format!("{relative}/{name}");
            if gone.has(&path) {
                continue;
            }
            let link = entry
                .metadata()
                .is_ok_and(|meta| meta.file_attributes() & LINK != 0);
            // A folder the manifest could not name is not taken over either.
            let nameable = neural::entry_path(&path).as_deref() == Some(path.as_str());
            match entry.file_type() {
                Ok(kind) if kind.is_dir() && !link && nameable => folders.push(path),
                Ok(kind)
                    if kind.is_file()
                        && !link
                        && ((neural::provider_dlssg(&relative)
                            && neural::provider_files(&name))
                            || neural::settings(&path)) => {}
                _ => foreign.push(path),
            }
        }
    }
    foreign.sort();
    Ok(if foreign.is_empty() {
        Ok(folders)
    } else {
        Err(foreign)
    })
}

/// What a removal of Rubylight 3.0's AMDNR would take out of the game's
/// folder, for [`super::Request::Check`] to ask about the folder as it will be:
/// the files its manifest names that are still as placed and its settings
/// ([`neural::removal`]), OptiScaler's logs, and its folders once nothing
/// else is in them. Empty for an install, which sees the folder as it is.
#[derive(Default)]
pub struct Gone {
    /// AMDNR's manifest is in the folder, and goes too.
    pub legacy: bool,
    /// In ASCII lower case, with `/`.
    pub paths: Vec<String>,
}
impl Gone {
    pub(super) fn has(&self, relative: &str) -> bool {
        let relative = relative.to_ascii_lowercase();
        self.paths.contains(&relative)
    }
    /// AMDNR in `folder` as its removal would leave it, and the files that
    /// removal would keep there, sorted.
    pub(super) fn of_legacy(folder: &Path, writer: Writer) -> Outcome<(Self, Vec<String>)> {
        guard(folder, LEGACY_MANIFEST, writer)?;
        let Some(legacy) = manifest(folder, LEGACY_MANIFEST).map_err(Failure::Error)? else {
            return Ok((Self::default(), Vec::new()));
        };
        for path in legacy.files.keys().chain(&legacy.folders) {
            guard(folder, path, writer)?;
        }
        // By size for a big file, as before a launch: the removal itself
        // hashes, and keeps (and so stops at) what changed.
        let removal = neural::removal(&legacy, |path| state(folder, &legacy, path, true));
        let mut paths: Vec<String> = removal
            .delete
            .iter()
            .map(|path| path.to_ascii_lowercase())
            .collect();
        paths.extend(
            std::fs::read_dir(folder)
                .into_iter()
                .flatten()
                .flatten()
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| neural::logs(name))
                .map(|name| name.to_ascii_lowercase()),
        );
        // What the removal deletes beside them (`remove_by`): partial copies.
        let partials: Vec<String> = legacy
            .files
            .keys()
            .map(|path| format!("{path}{PARTIAL}").to_ascii_lowercase())
            .collect();
        // What the removal keeps stops the install after it (`kept_text`):
        // a file changed since (by size here, so a same-size change still
        // stops it then), the settings it took over, and whatever else is in
        // its folders. Asked now, so the game keeps the earlier upscaling.
        let mut kept = removal.keep;
        // Deepest first: a folder goes once all that is in it goes.
        for relative in legacy.folders.iter().rev() {
            let Ok(entries) = std::fs::read_dir(folder.join(relative)) else {
                continue;
            };
            let mut empty = true;
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let path = format!("{relative}/{name}");
                let lower = path.to_ascii_lowercase();
                let kind = entry.file_type().ok();
                if paths.contains(&lower)
                    || (kind.is_some_and(|kind| kind.is_file())
                        && (partials.contains(&lower)
                            || (neural::provider_dlssg(relative) && neural::provider_files(&name))))
                {
                    continue;
                }
                empty = false;
                // A folder of its own that stays is named by what is in it.
                let own = kind.is_some_and(|kind| kind.is_dir() && !kind.is_symlink())
                    && legacy.folders.iter().any(|f| f.eq_ignore_ascii_case(&path));
                if !own {
                    kept.push(path);
                }
            }
            if empty {
                paths.push(relative.to_ascii_lowercase());
            }
        }
        kept.sort();
        kept.dedup();
        Ok((
            Self {
                legacy: true,
                paths,
            },
            kept,
        ))
    }
}

/// The layout for the game's folder, or why it is refused: Rubylight 3.0's
/// AMDNR still there, an OptiScaler of the user's that can't load the
/// provider, another mod, or every name taken. An earlier install keeps its
/// mode. What `gone` names counts as not there.
pub(super) fn layout(
    folder: &Path,
    exe: &Path,
    old: Option<&Manifest>,
    writer: Writer,
    gone: &Gone,
) -> Outcome<Layout> {
    if !gone.legacy && folder.join(LEGACY_MANIFEST).exists() {
        return Err(Failure::Refused(
            "the earlier Neural Rendering upscaling is in the game's folder: remove it on the game's page first"
                .into(),
        ));
    }
    let users: Vec<&str> = neural::PROXIES
        .iter()
        .copied()
        .filter(|name| {
            let ours = old.is_some_and(|m| m.mode == Mode::Full && m.proxy == *name);
            !ours && !gone.has(name) && occupant(folder, name, old) == Occupant::OptiScaler
        })
        .collect();
    let addon = old.map_or(!users.is_empty(), |m| m.mode == Mode::Addon);
    if addon {
        let proxy = match (old, users.as_slice()) {
            (Some(old), _) => old.proxy.clone(),
            (None, [one]) => (*one).to_owned(),
            (None, more) => {
                return Err(Failure::Refused(format!(
                    "the game's folder has more than one OptiScaler ({}): keep one",
                    more.join(", ")
                )));
            }
        };
        guard(folder, &proxy, writer)?;
        // Another mod beside the user's OptiScaler is refused as in full mode;
        // OptiScaler's own files and its fakenvapi add-on are what it needs.
        for entry in std::fs::read_dir(folder)
            .map_err(|e| Failure::Error(format!("cannot read {}: {e}", folder.display())))?
            .flatten()
        {
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(other) = neural::other_mod(&name)
                && !gone.has(&name)
                && other != "OptiScaler"
                && !other.starts_with("fakenvapi")
            {
                return Err(Failure::Refused(format!(
                    "{other} is installed in the game's folder ({name}): remove it first"
                )));
            }
        }
        let dll = std::fs::read(folder.join(&proxy))
            .map_err(|e| Failure::Error(format!("cannot read {proxy}: {e}")))?;
        if !neural::custom_providers(&dll) {
            return Err(Failure::Refused(format!(
                "the OptiScaler in the game's folder ({proxy}) can't load another frame generation: update it to a release that can, or remove it"
            )));
        }
        let ini = folder.join(SETTINGS);
        let bytes = (!gone.has(SETTINGS))
            .then(|| std::fs::read(&ini).ok())
            .flatten()
            .ok_or_else(|| {
                Failure::Refused(format!(
                    "the OptiScaler in the game's folder ({proxy}) has no {SETTINGS} beside it"
                ))
            })?;
        let opti_dir = neural::opti_dir(
            &bytes,
            folder.join("OptiScaler").is_dir() && !gone.has("OptiScaler"),
        )
        .map_err(Failure::Refused)?;
        if !opti_dir.is_empty() && (!folder.join(&opti_dir).is_dir() || gone.has(&opti_dir)) {
            return Err(Failure::Refused(format!(
                "OptiScaler's folder {opti_dir} is not in the game's folder"
            )));
        }
        let provider = if opti_dir.is_empty() {
            format!("custom/{}", neural::PROVIDER)
        } else {
            format!("{opti_dir}/custom/{}", neural::PROVIDER)
        };
        // The provider's folder an earlier add-on left is taken over when
        // only what the provider wrote is in it.
        let mut adopt = Vec::new();
        if old.is_none()
            && !gone.has(&provider)
            && std::fs::symlink_metadata(folder.join(&provider)).is_ok()
        {
            if !std::fs::symlink_metadata(folder.join(&provider))
                .is_ok_and(|meta| meta.is_dir() && meta.file_attributes() & LINK == 0)
            {
                return Err(Failure::Refused(format!(
                    "{provider} is already in the game's folder and is not Rubylight's"
                )));
            }
            adopt = leftover(folder, &provider, writer, gone)?
                .map_err(|foreign| Failure::Refused(in_the_way(&foreign)))?;
        }
        return Ok(Layout {
            mode: Mode::Addon,
            proxy,
            opti_dir,
            adopt,
        });
    }
    let ours = |name: &str| {
        old.is_some_and(|m| {
            m.files.contains_key(name) || m.folders.iter().any(|f| f == name) || (m.proxy == name)
        })
    };
    // The OptiScaler folder an earlier install left once its manifest went
    // (the provider writes there while the game runs) is asked about after
    // every other mod, which refuses first.
    let mut left = None;
    for entry in std::fs::read_dir(folder)
        .map_err(|e| Failure::Error(format!("cannot read {}: {e}", folder.display())))?
        .flatten()
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        if ours(&name) || gone.has(&name) {
            continue;
        }
        let Some(other) = neural::other_mod(&name) else {
            continue;
        };
        let folder_of_ours = old.is_none()
            && name.eq_ignore_ascii_case("OptiScaler")
            && entry
                .metadata()
                .is_ok_and(|meta| meta.is_dir() && meta.file_attributes() & LINK == 0);
        if folder_of_ours {
            left = Some(name);
            continue;
        }
        return Err(Failure::Refused(format!(
            "{other} is installed in the game's folder ({name}): remove it first"
        )));
    }
    // Taken over only when it holds the provider's folder and nothing else
    // but what the provider wrote; a folder of that name without it may be
    // the game's own.
    let adopt = match left {
        Some(name) => match leftover(folder, &name, writer, gone)? {
            Ok(folders)
                if folders.iter().any(|relative| {
                    relative.eq_ignore_ascii_case(&format!("{name}/custom/{}", neural::PROVIDER))
                }) =>
            {
                folders
            }
            Ok(_) => {
                return Err(Failure::Refused(format!(
                    "OptiScaler is installed in the game's folder ({name}): remove it first"
                )));
            }
            Err(foreign) => return Err(Failure::Refused(in_the_way(&foreign))),
        },
        None => Vec::new(),
    };
    let vulkan = std::fs::read(exe).is_ok_and(|program| neural::vulkan_only(&program));
    let proxy = neural::proxy(vulkan, |name| {
        if gone.has(name) {
            Occupant::Free
        } else {
            occupant(folder, name, old)
        }
    })
    .map_err(Failure::Refused)?;
    Ok(Layout {
        mode: Mode::Full,
        proxy: proxy.into(),
        opti_dir: String::new(),
        adopt,
    })
}
