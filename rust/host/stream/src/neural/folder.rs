//! Neural Rendering in one game's folder: find the game's .exe, refuse
//! anti-cheat, other mods and games without DLSS frame generation, place a
//! cached release (OptiScaler under a free proxy name, or the provider in
//! the user's own OptiScaler), check it before a launch, and remove exactly
//! what was placed, Rubylight 3.0's AMDNR included.
//!
//! Game folders are the user's to write, so a SYSTEM host runs
//! `butterpollo.exe --neural-folder` with the signed-in user's token
//! ([`super::run_folder`]). It writes a folder itself only when the user may
//! not and nobody but administrators may change it ([`Writer::System`]).
//! Neither changes a folder through a link inside the game's folder: one
//! planted in a library that other accounts share would send a copy or a
//! delete elsewhere. Only the user's helper follows the game's folder
//! itself when it is a link, as a game moved and linked back leaves it
//! ([`followed`]); a SYSTEM host follows none.

mod cleanup;
mod find;
mod layout;
mod place;
mod remove;
mod security;

pub use butterpollo_session::neural::{
    ANTI_CHEAT, Cached, NO_DLSS, PACKAGE, Package, Rights, Security, kept_text,
};
#[cfg(any(test, feature = "test-support"))]
pub use find::anti_cheat_refusal;
pub use find::{same, shared_folders};
pub(super) use find::{inside, not_the_game};
pub use layout::Gone;
pub use place::{check, place, prepare};
pub use remove::remove;
pub(crate) use security::administrators_hold;
pub use security::{account, administrators_only, hold, security, verdict, writable};

use butterpollo_session::neural::{MANIFEST, Manifest, Mode, Now, Placed};
use find::{find_exe, refuse_anti_cheat, refuse_without_frame_generation};
use security::guard;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::{BufRead, Read, Write},
    os::windows::fs::MetadataExt,
    path::{Path, PathBuf},
};

/// The hidden flag that makes the host exe do one folder request.
pub const FLAG: &str = "--neural-folder";
/// Marks the helper's answer and progress lines on its output.
pub const ANSWER: &str = "NEURAL-FOLDER-RESPONSE ";
pub const PROGRESS: &str = "NEURAL-FOLDER-PROGRESS ";
/// What a copy is called until it is complete.
const PARTIAL: &str = ".rubylight-partial";
/// What the user's helper creates and deletes to learn whether it may write
/// the game's folder.
const PROBE: &str = "rubylight-neural.probe";
/// Files at least this big are checked by size before a launch; hashing the
/// 760 MB model at every launch would delay the game.
const HASHED_AT_LAUNCH: u64 = 64 << 20;
/// How far the anti-cheat and .exe searches look into an install, and how
/// much they see before they give up (less in tests, which fill a folder).
const DEPTH: usize = 6;
const ENTRIES: usize = if cfg!(any(test, feature = "test-support")) {
    1_000
} else {
    50_000
};
/// A junction, symbolic link or other reparse point.
const LINK: u32 = 0x400;

/// Who writes the game's folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Writer {
    /// The signed-in user's helper, or a host that is not SYSTEM.
    User,
    /// A SYSTEM host, in a folder the user may not write: only when it and
    /// every folder above it are no links and nobody but administrators may
    /// change them ([`administrators_only`]).
    System,
}

/// Read and validate `package.json` from a cached release folder.
pub fn read_package(folder: &Path) -> Result<Package, String> {
    let label = folder.display().to_string();
    let text = std::fs::read_to_string(folder.join(PACKAGE))
        .map_err(|e| format!("the cached release in {label} is unreadable: {e}"))?;
    Package::parse(&text, &label)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// The program [`Request::Install`] would place beside, writing nothing:
    /// `exe` checked, or the game's .exe found in `install`.
    Find {
        exe: Option<PathBuf>,
        install: Option<PathBuf>,
    },
    /// Place `package` beside `exe`, or beside the game's .exe found in `install`.
    Install {
        exe: Option<PathBuf>,
        install: Option<PathBuf>,
        package: Cached,
    },
    /// Every refusal [`Request::Install`] of `package` would give, with
    /// Rubylight 3.0's AMDNR in the game's folder taken as removed already,
    /// writing nothing ([`Response::Found`]): asked before that install is
    /// taken out, so a game that is refused keeps it. `legacy` is the .exe
    /// AMDNR was placed beside, which need not be the program chosen now.
    Check {
        exe: Option<PathBuf>,
        install: Option<PathBuf>,
        package: Cached,
        #[serde(default)]
        legacy: Option<PathBuf>,
    },
    /// Before a launch: refuse anti-cheat, finish an update cut short with
    /// `package`, and check that the placed files are still as placed.
    Prepare {
        exe: PathBuf,
        install: Option<PathBuf>,
        package: Option<Cached>,
    },
    Remove {
        exe: PathBuf,
        /// The program an install goes beside: Rubylight 3.0's AMDNR in
        /// another folder keeps its record of what its removal leaves
        /// ([`Response::Left`]).
        #[serde(default)]
        beside: Option<PathBuf>,
        /// Only Rubylight 3.0's AMDNR: frame generation's files stay.
        #[serde(default)]
        earlier: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Response {
    /// The game's program ([`Request::Find`]).
    Found { exe: PathBuf },
    Placed {
        exe: PathBuf,
        release: String,
        proxy: String,
        updated: bool,
        #[serde(default)]
        mode: Mode,
    },
    Removed {
        /// What stayed: placed files that changed since, and what someone
        /// else put in the folders the host created.
        kept: Vec<String>,
    },
    /// As [`Response::Removed`], for Rubylight 3.0's AMDNR in a folder other
    /// than the one the install goes to (the paths followed): its manifest
    /// stays, naming only what is left, so a later removal finishes it.
    Left { kept: Vec<String> },
    /// The folder's drive or share is not there now; the mod may still be in it.
    Unreachable { folder: String },
    /// No single .exe in the install is the game's: the user chooses it.
    ChooseExe { reason: String },
    /// The game cannot have it (anti-cheat, another mod, no DLSS frame
    /// generation, a name taken).
    Refused { reason: String },
    Error { message: String },
    /// The signed-in user may not write the game's folder (a game in
    /// Program Files): a SYSTEM host may write it itself ([`Writer::System`]).
    NotWritable { folder: PathBuf },
}

pub fn handle(request: Request, writer: Writer, progress: &dyn Fn(u64, u64)) -> Response {
    let result = match request {
        // As the install places it: where a linked game folder leads, so the
        // removal recorded first names the folder the mod goes to.
        // Every refusal the install gives, before an install takes Rubylight
        // 3.0's AMDNR out: a game that gained anti-cheat keeps it and is refused.
        Request::Find { exe, install } => find_exe(exe, install.as_deref()).and_then(|exe| {
            let exe = followed(exe, writer);
            refuse_anti_cheat(&exe, install.as_deref())?;
            refuse_without_frame_generation(&exe, install.as_deref())?;
            Ok(Response::Found { exe })
        }),
        Request::Install {
            exe,
            install,
            package,
        } => find_exe(exe, install.as_deref()).and_then(|exe| {
            let exe = followed(exe, writer);
            package
                .package
                .validate_paths(&package.folder.display().to_string())
                .map_err(Failure::Error)?;
            refuse_anti_cheat(&exe, install.as_deref())?;
            refuse_without_frame_generation(&exe, install.as_deref())?;
            place(&exe, &package, writer, progress)
        }),
        Request::Check {
            exe,
            install,
            package,
            legacy,
        } => find_exe(exe, install.as_deref()).and_then(|exe| {
            let exe = followed(exe, writer);
            package
                .package
                .validate_paths(&package.folder.display().to_string())
                .map_err(Failure::Error)?;
            refuse_anti_cheat(&exe, install.as_deref())?;
            refuse_without_frame_generation(&exe, install.as_deref())?;
            let legacy = legacy.map(|legacy| followed(legacy, writer));
            check(&exe, legacy.as_deref(), &package, writer)
        }),
        Request::Prepare {
            exe,
            install,
            package,
        } => prepare(
            &followed(exe, writer),
            install.as_deref(),
            package,
            writer,
            progress,
        ),
        Request::Remove {
            exe,
            beside,
            earlier,
        } => {
            let exe = followed(exe, writer);
            let elsewhere = beside.is_some_and(|beside| {
                !same_folder(folder_of(&exe), folder_of(&followed(beside, writer)))
            });
            remove(folder_of(&exe), writer, elsewhere, earlier)
        }
    };
    result.unwrap_or_else(|failure| match failure {
        Failure::ChooseExe(reason) => Response::ChooseExe { reason },
        Failure::Refused(reason) => Response::Refused { reason },
        Failure::Error(message) => Response::Error { message },
        Failure::NotWritable(folder) => Response::NotWritable { folder },
    })
}

/// The helper's entry point (`butterpollo.exe --neural-folder`): one request
/// line on stdin, progress and one answer line on stdout.
pub fn helper() -> i32 {
    let mut line = String::new();
    let response = if butterpollo_win_io::process::is_system() {
        Response::Error {
            message: "the Neural Rendering helper must run as the signed-in user".into(),
        }
    } else if let Err(error) = std::io::stdin().lock().read_line(&mut line) {
        Response::Error {
            message: format!("unreadable helper request: {error}"),
        }
    } else {
        match serde_json::from_str(&line) {
            Ok(request) => handle(request, Writer::User, &|done, total| {
                let mut out = std::io::stdout().lock();
                let _ = writeln!(out, "{PROGRESS}{done} {total}");
                let _ = out.flush();
            }),
            Err(error) => Response::Error {
                message: format!("unreadable helper request: {error}"),
            },
        }
    };
    let answer = serde_json::to_string(&response).unwrap_or_default();
    let mut out = std::io::stdout().lock();
    if writeln!(out, "{ANSWER}{answer}")
        .and_then(|()| out.flush())
        .is_err()
    {
        return 1;
    }
    0
}

pub enum Failure {
    ChooseExe(String),
    Refused(String),
    Error(String),
    NotWritable(PathBuf),
}
impl Failure {
    pub(self) fn message(&self) -> String {
        match self {
            Self::ChooseExe(text) | Self::Refused(text) | Self::Error(text) => text.clone(),
            Self::NotWritable(folder) => format!("cannot write {}", folder.display()),
        }
    }
}
pub type Outcome<T> = Result<T, Failure>;

pub(self) fn folder_of(exe: &Path) -> &Path {
    exe.parent().unwrap_or(exe)
}

/// `exe` in its game's folder as it really is, for the user's own writes:
/// a folder that is itself a link (a game moved elsewhere and linked back
/// with `mklink /J`) is followed once, and the whole request works there.
/// The user may write wherever it leads; links below it are still refused
/// ([`guard`]). A SYSTEM host follows none.
pub fn followed(exe: PathBuf, writer: Writer) -> PathBuf {
    let (Some(folder), Some(name)) = (exe.parent(), exe.file_name()) else {
        return exe;
    };
    let linked =
        std::fs::symlink_metadata(folder).is_ok_and(|meta| meta.file_attributes() & LINK != 0);
    if writer != Writer::User || !linked {
        return exe;
    }
    match real(folder) {
        Some(real) => real.join(name),
        // Where it leads is not there now: the link is refused as one.
        None => exe,
    }
}

/// `path` with every link on the way followed; None where it leads is not
/// there now.
pub fn real(path: &Path) -> Option<PathBuf> {
    let real = std::fs::canonicalize(path).ok()?;
    // `\\?\C:\...` as `C:\...`, `\\?\UNC\server\...` as `\\server\...`.
    Some(match real.to_str() {
        Some(text) => match text.strip_prefix(r"\\?\UNC\") {
            Some(share) => PathBuf::from(format!(r"\\{share}")),
            None => PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(text)),
        },
        None => real,
    })
}

pub(super) fn sha256_file(path: &Path) -> std::io::Result<(String, u64)> {
    let mut file = std::fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0; 1 << 20];
    let mut size = 0;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            return Ok((hex::encode(hash.finalize()), size));
        }
        size += read as u64;
        hash.update(&buffer[..read]);
    }
}

/// Which of `forms` the file at `path` is now, [`Now::Missing`] or
/// [`Now::Changed`] when it is none. `quick` checks a big file by its size.
fn matched<'a>(
    folder: &Path,
    path: &str,
    forms: &[&'a Placed],
    quick: bool,
) -> Result<&'a Placed, Now> {
    let file = folder.join(path);
    let meta = std::fs::symlink_metadata(&file).map_err(|_| Now::Missing)?;
    let sized: Vec<&Placed> = forms
        .iter()
        .copied()
        .filter(|placed| meta.is_file() && meta.len() == placed.size)
        .collect();
    match sized.first() {
        None => Err(Now::Changed),
        Some(placed) if quick && placed.size >= HASHED_AT_LAUNCH => Ok(placed),
        Some(_) => {
            let (hash, _) = sha256_file(&file).map_err(|_| Now::Changed)?;
            sized
                .into_iter()
                .find(|placed| placed.sha256 == hash)
                .ok_or(Now::Changed)
        }
    }
}

/// A file `manifest` names as it is now: [`Now::Same`] as placed, or during
/// an update, as the release before placed it.
pub fn state(folder: &Path, manifest: &Manifest, path: &str, quick: bool) -> Now {
    let forms: Vec<&Placed> = manifest
        .files
        .get(path)
        .into_iter()
        .chain(manifest.previous.get(path))
        .collect();
    match matched(folder, path, &forms, quick) {
        Ok(_) => Now::Same,
        Err(now) => now,
    }
}

/// The manifest `name` in `folder`, None when there is none.
pub fn manifest(folder: &Path, name: &str) -> Result<Option<Manifest>, String> {
    match std::fs::read_to_string(folder.join(name)) {
        Ok(text) => Manifest::parse(&text)
            .map(Some)
            .map_err(|e| format!("{name}: {e} ({})", folder.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("cannot read {}: {e}", folder.join(name).display())),
    }
}

fn write_manifest(folder: &Path, manifest: &Manifest, writer: Writer) -> Outcome<()> {
    write_named(folder, MANIFEST, manifest, writer)
}

fn write_named(folder: &Path, name: &str, manifest: &Manifest, writer: Writer) -> Outcome<()> {
    let path = folder.join(name);
    let partial = partial(&path);
    guard(folder, name, writer)?;
    guard(folder, &format!("{name}{PARTIAL}"), writer)?;
    let text = serde_json::to_string_pretty(manifest).map_err(|e| Failure::Error(e.to_string()))?;
    std::fs::write(&partial, text)
        .and_then(|()| std::fs::rename(&partial, &path))
        .map_err(|e| {
            let _ = std::fs::remove_file(&partial);
            Failure::Error(format!("cannot write {}: {e}", path.display()))
        })
}

/// What `path` is called until it is complete.
pub fn partial(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(PARTIAL);
    name.into()
}

/// Write `bytes` to `relative` in the game folder whole: beside it first,
/// then in its place.
fn write_whole(folder: &Path, relative: &str, bytes: &[u8], writer: Writer) -> Outcome<()> {
    guard(folder, relative, writer)?;
    guard(folder, &format!("{relative}{PARTIAL}"), writer)?;
    let path = folder.join(relative);
    let partial = partial(&path);
    std::fs::write(&partial, bytes)
        .and_then(|()| std::fs::rename(&partial, &path))
        .map_err(|e| {
            let _ = std::fs::remove_file(&partial);
            Failure::Error(format!("cannot write {}: {e}", path.display()))
        })
}

/// Whether two folders are one: the same path, or the same folder on the disk.
pub fn same_folder(a: &Path, b: &Path) -> bool {
    a.as_os_str().eq_ignore_ascii_case(b.as_os_str())
        || std::fs::canonicalize(a)
            .ok()
            .is_some_and(|a| std::fs::canonicalize(b).is_ok_and(|b| a == b))
}
