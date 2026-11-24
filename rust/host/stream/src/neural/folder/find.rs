//! Game executable discovery, launcher/helper filtering, install walking, and
//! anti-cheat / DLSS frame generation checks for Neural Rendering folders.
use super::{ANTI_CHEAT, DEPTH, ENTRIES, Failure, NO_DLSS, Outcome, folder_of, real};
use butterpollo_session::neural;
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
};

/// The chosen .exe, or the game's .exe in its install: the one Unreal Engine
/// shipping build, else the one program in the install's top folder that is
/// no crash reporter, launcher or installer.
pub(super) fn find_exe(exe: Option<PathBuf>, install: Option<&Path>) -> Outcome<PathBuf> {
    if let Some(exe) = exe {
        let program = exe
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("exe"));
        return if let Some(what) = not_the_game(&exe) {
            Err(Failure::ChooseExe(format!(
                "{} is {what}, not the game: choose the game's own program",
                exe.display()
            )))
        } else if let Some(install) = install.filter(|install| {
            // A program found in a linked game folder is where the link
            // leads ([`super::Request::Find`]).
            !inside(&exe, install) && real(install).is_none_or(|real| !inside(&exe, &real))
        }) {
            Err(Failure::ChooseExe(format!(
                "{} is not in the game's folder {}: choose the game's own program",
                exe.display(),
                install.display()
            )))
        } else if program && exe.is_absolute() && exe.is_file() {
            Ok(exe)
        } else {
            Err(Failure::Error(format!(
                "{} is not a program on this PC",
                exe.display()
            )))
        };
    }
    let Some(install) = install else {
        return Err(Failure::ChooseExe(
            "Rubylight doesn't know where this game is installed".into(),
        ));
    };
    if !install.is_dir() {
        return Err(Failure::ChooseExe(format!(
            "{} is not a folder on this PC",
            install.display()
        )));
    }
    let mut shipping = Vec::new();
    let mut top = Vec::new();
    let walked = walk(install, |path, depth, folder| {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_lowercase();
        if folder || !name.ends_with(".exe") {
            return None::<()>;
        }
        if name.ends_with("-shipping.exe") {
            shipping.push(path.to_path_buf());
        } else if depth == 1 && !neural::is_helper_program(&name) && not_the_game(path).is_none() {
            top.push(path.to_path_buf());
        }
        None
    });
    if let Err(TooBig) = walked {
        return Err(Failure::ChooseExe(format!(
            "{} has too many files for Rubylight to tell which program is the game",
            install.display()
        )));
    }
    match (shipping.len(), top.len()) {
        (1, _) => Ok(shipping.remove(0)),
        (0, 1) => Ok(top.remove(0)),
        _ => Err(Failure::ChooseExe(format!(
            "Rubylight can't tell which program in {} is the game",
            install.display()
        ))),
    }
}

/// What `exe` is when it is not a game's own program: a store's or
/// library's launcher, a script host or interpreter, or a program of
/// Windows. The mod beside it would load into that program and what it
/// starts, never into the game, and in Program Files it would be there for
/// every account.
pub(in crate::neural) fn not_the_game(exe: &Path) -> Option<&'static str> {
    let name = exe.file_name().unwrap_or_default().to_string_lossy();
    if let Some(kind) = neural::classify_non_game_exe(&name) {
        return Some(kind);
    }
    let windows = ["SystemRoot", "windir"]
        .into_iter()
        .filter_map(std::env::var_os)
        .filter(|root| !root.is_empty())
        .any(|root| inside(exe, Path::new(&root)));
    windows.then_some("part of Windows")
}

/// [`walk`] saw more than [`ENTRIES`] and stopped.
struct TooBig;

/// Walk `root` breadth first, `depth` 1 for its own entries, without following
/// links, until `visit` returns something; [`TooBig`] when the walk is too big.
fn walk<T>(
    root: &Path,
    mut visit: impl FnMut(&Path, usize, bool) -> Option<T>,
) -> Result<Option<T>, TooBig> {
    let mut queue = VecDeque::from([(root.to_path_buf(), 1)]);
    let mut seen = 0;
    while let Some((folder, depth)) = queue.pop_front() {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            seen += 1;
            if seen > ENTRIES {
                return Err(TooBig);
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if let Some(found) = visit(&path, depth, kind.is_dir()) {
                return Ok(Some(found));
            }
            if kind.is_dir() && !kind.is_symlink() && depth < DEPTH {
                queue.push_back((path, depth + 1));
            }
        }
    }
    Ok(None)
}

/// Whether `path` is `root` or inside it, comparing names as Windows does,
/// whatever their case.
pub(in crate::neural) fn inside(path: &Path, root: &Path) -> bool {
    let mut parts = path.components();
    root.components().all(|part| {
        parts.next().is_some_and(|other| {
            part.as_os_str().to_string_lossy().to_lowercase()
                == other.as_os_str().to_string_lossy().to_lowercase()
        })
    })
}

pub fn same(a: &Path, b: &Path) -> bool {
    a.components().count() == b.components().count() && inside(a, b)
}

/// A folder that holds other programs or their data: a drive's root, a
/// library's `common` folder, or one of the `shared` folders
/// ([`shared_folders`]).
fn holds_others(folder: &Path, shared: &[PathBuf]) -> bool {
    let Some(name) = folder.file_name() else {
        return true;
    };
    let name = name.to_string_lossy();
    neural::is_shared_folder_name(&name) || shared.iter().any(|root| same(folder, root))
}

/// The folders known by place to hold other programs or their data: the
/// Program Files folders and the user's profile and `AppData` folders
/// (`%LOCALAPPDATA%\BattlEye` is no game's own).
pub fn shared_folders() -> Vec<PathBuf> {
    [
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramW6432",
        "USERPROFILE",
        "LOCALAPPDATA",
        "APPDATA",
    ]
    .into_iter()
    .filter_map(std::env::var_os)
    .filter(|root| !root.is_empty())
    .map(PathBuf::from)
    .collect()
}

/// [`refuse_anti_cheat_in`] with this account's [`shared_folders`].
pub(super) fn refuse_anti_cheat(exe: &Path, install: Option<&Path>) -> Outcome<()> {
    refuse_anti_cheat_in(exe, install, &shared_folders())
}

/// [`refuse_anti_cheat_in`] for a test's own shared folders, and why it refused.
#[cfg(any(test, feature = "test-support"))]
pub fn anti_cheat_refusal(
    exe: &Path,
    install: Option<&Path>,
    shared: &[PathBuf],
) -> Result<(), String> {
    refuse_anti_cheat_in(exe, install, shared).map_err(|failure| failure.message())
}

/// Refuse a game with anti-cheat anywhere in its install or below its .exe,
/// or beside the three folders above the .exe (an Unreal game's
/// `EasyAntiCheat` sits three up): only each folder's own entries, and the
/// search stays in the install and stops at a folder that holds other
/// programs ([`holds_others`]). An install too big to search through is
/// refused too.
fn refuse_anti_cheat_in(exe: &Path, install: Option<&Path>, shared: &[PathBuf]) -> Outcome<()> {
    let found = |path: &Path| {
        let name = path.file_name()?.to_string_lossy().into_owned();
        neural::anti_cheat(&name).map(|which| (which, path.to_path_buf()))
    };
    let own = folder_of(exe);
    let mut roots = Vec::from_iter(install.filter(|install| !same(install, own)));
    roots.push(own);
    let mut hit = None;
    for root in roots {
        hit = walk(root, |path, _, _| found(path)).map_err(|TooBig| {
            Failure::Refused(format!(
                "{} has too many files for Rubylight to check it for anti-cheat",
                root.display()
            ))
        })?;
        if hit.is_some() {
            break;
        }
    }
    let within = install.filter(|install| inside(own, install));
    for above in own.ancestors().skip(1).take(3) {
        if hit.is_some()
            || holds_others(above, shared)
            || within.is_some_and(|install| !inside(above, install))
        {
            break;
        }
        hit = std::fs::read_dir(above)
            .into_iter()
            .flatten()
            .flatten()
            .find_map(|entry| found(&entry.path()));
    }
    match hit {
        Some((which, path)) => Err(Failure::Refused(format!(
            "the game uses {which} ({}), and {ANTI_CHEAT}",
            path.display()
        ))),
        None => Ok(()),
    }
}

/// Refuse a game with no DLSS frame generation for the provider to take
/// the place of: no NVIDIA DLSS frame generation DLL or Streamline plugin
/// for it in its install or below its .exe (searched as anti-cheat is,
/// case-blind). OptiScaler's own folder and a provider's `dlssg` folder
/// don't count: they hold the provider itself.
pub(super) fn refuse_without_frame_generation(exe: &Path, install: Option<&Path>) -> Outcome<()> {
    let own = folder_of(exe);
    let mut roots = Vec::from_iter(install.filter(|install| !same(install, own)));
    roots.push(own);
    let provider = |path: &Path| {
        path.parent()
            .and_then(Path::file_name)
            .is_some_and(|parent| parent.eq_ignore_ascii_case("dlssg"))
            || path.components().any(|part| {
                part.as_os_str().eq_ignore_ascii_case("OptiScaler")
                    || part.as_os_str().eq_ignore_ascii_case(neural::PROVIDER)
            })
    };
    for root in roots {
        let found = walk(root, |path, _, folder| {
            let name = path.file_name()?.to_string_lossy();
            (!folder && neural::dlss_frame_generation(&name) && !provider(path)).then_some(())
        })
        .map_err(|TooBig| {
            Failure::Refused(format!(
                "{} has too many files for Rubylight to look for the game's DLSS frame generation",
                root.display()
            ))
        })?;
        if found.is_some() {
            return Ok(());
        }
    }
    Err(Failure::Refused(format!(
        "{NO_DLSS} (nvngx_dlssg.dll or sl.dlss_g.dll in its folder) for Neural Rendering to take the place of; turn it on only for a game that offers DLSS frame generation"
    )))
}
