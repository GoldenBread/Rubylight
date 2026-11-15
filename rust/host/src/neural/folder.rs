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
use butterpollo_core::neural::{
    self, LEGACY_MANIFEST, MANIFEST, Manifest, Mode, Now, Occupant, PLACED_BY, Placed, SETTINGS,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
    io::{BufRead, Read, Write},
    os::windows::fs::MetadataExt,
    path::{Path, PathBuf},
};

/// The hidden flag that makes the host exe do one folder request.
pub const FLAG: &str = "--neural-folder";
/// Marks the helper's answer and progress lines on its output.
pub const ANSWER: &str = "NEURAL-FOLDER-RESPONSE ";
pub const PROGRESS: &str = "NEURAL-FOLDER-PROGRESS ";
/// A cached package's description, beside its `files` folder.
pub const PACKAGE: &str = "package.json";
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
const ENTRIES: usize = if cfg!(test) { 1_000 } else { 50_000 };
/// SYSTEM's, the Administrators group's and TrustedInstaller's (what Program
/// Files holds).
const ADMINISTRATORS: [&str; 3] = [
    "S-1-5-18",
    "S-1-5-32-544",
    "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464",
];
/// How a refusal of a game without DLSS frame generation begins, and what an
/// anti-cheat refusal says: the host tells a device which it was.
pub const NO_DLSS: &str = "the game has no DLSS frame generation";
pub const ANTI_CHEAT: &str = "an anti-cheat treats Neural Rendering's DLL as a cheat";
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

/// A release in the host's cache: `files/full/` and `files/addon/` hold
/// what the host takes from its two archives, each checked against the pin,
/// GitHub's digest and the release's SHA256SUMS.txt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Package {
    pub tag: String,
    /// Each archive's name and SHA-256: the cache serves the release only
    /// while its archives are these.
    pub archives: BTreeMap<String, String>,
    /// Each file by its cache path (`full/...`, `addon/...`).
    pub files: BTreeMap<String, Placed>,
}
impl Package {
    pub fn read(folder: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(folder.join(PACKAGE)).map_err(|e| {
            format!(
                "the cached release in {} is unreadable: {e}",
                folder.display()
            )
        })?;
        let package: Self = serde_json::from_str(&text)
            .map_err(|e| format!("the cached release in {} is damaged: {e}", folder.display()))?;
        package.inside(folder)?;
        Ok(package)
    }
    /// Whether each file it names is one inside the cached release `folder`.
    fn inside(&self, folder: &Path) -> Result<(), String> {
        if self
            .files
            .keys()
            .all(|path| neural::entry_path(path).as_ref() == Some(path))
        {
            Ok(())
        } else {
            Err(format!(
                "the cached release in {} names a file outside it",
                folder.display()
            ))
        }
    }
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
    /// taken out, so a game that is refused keeps it.
    Check {
        exe: Option<PathBuf>,
        install: Option<PathBuf>,
        package: Cached,
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
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Response {
    /// The game's program ([`Request::Find`]).
    Found {
        exe: PathBuf,
    },
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
    /// The folder's drive or share is not there now; the mod may still be in it.
    Unreachable {
        folder: String,
    },
    /// No single .exe in the install is the game's: the user chooses it.
    ChooseExe {
        reason: String,
    },
    /// The game cannot have it (anti-cheat, another mod, no DLSS frame
    /// generation, a name taken).
    Refused {
        reason: String,
    },
    Error {
        message: String,
    },
    /// The signed-in user may not write the game's folder (a game in
    /// Program Files): a SYSTEM host may write it itself ([`Writer::System`]).
    NotWritable {
        folder: PathBuf,
    },
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
                .inside(&package.folder)
                .map_err(Failure::Error)?;
            refuse_anti_cheat(&exe, install.as_deref())?;
            refuse_without_frame_generation(&exe, install.as_deref())?;
            place(&exe, &package, writer, progress)
        }),
        Request::Check {
            exe,
            install,
            package,
        } => find_exe(exe, install.as_deref()).and_then(|exe| {
            let exe = followed(exe, writer);
            package
                .package
                .inside(&package.folder)
                .map_err(Failure::Error)?;
            refuse_anti_cheat(&exe, install.as_deref())?;
            refuse_without_frame_generation(&exe, install.as_deref())?;
            check(&exe, &package, writer)
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
        Request::Remove { exe } => remove(folder_of(&followed(exe, writer)), writer),
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
    let response = if butterpollo_windows::process::is_system() {
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

enum Failure {
    ChooseExe(String),
    Refused(String),
    Error(String),
    NotWritable(PathBuf),
}
impl Failure {
    fn message(&self) -> String {
        match self {
            Self::ChooseExe(text) | Self::Refused(text) | Self::Error(text) => text.clone(),
            Self::NotWritable(folder) => format!("cannot write {}", folder.display()),
        }
    }
}
type Outcome<T> = Result<T, Failure>;

/// A cached release and the files it holds as the host checked them: the
/// helper takes these hashes, never the cache's package.json read again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cached {
    pub folder: PathBuf,
    pub package: Package,
}
impl Package {
    pub fn with(self, folder: PathBuf) -> Cached {
        Cached {
            folder,
            package: self,
        }
    }
}

fn folder_of(exe: &Path) -> &Path {
    exe.parent().unwrap_or(exe)
}

/// `exe` in its game's folder as it really is, for the user's own writes:
/// a folder that is itself a link (a game moved elsewhere and linked back
/// with `mklink /J`) is followed once, and the whole request works there.
/// The user may write wherever it leads; links below it are still refused
/// ([`guard`]). A SYSTEM host follows none.
fn followed(exe: PathBuf, writer: Writer) -> PathBuf {
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
pub(super) fn real(path: &Path) -> Option<PathBuf> {
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

/// The chosen .exe, or the game's .exe in its install: the one Unreal Engine
/// shipping build, else the one program in the install's top folder that is
/// no crash reporter, launcher or installer.
fn find_exe(exe: Option<PathBuf>, install: Option<&Path>) -> Outcome<PathBuf> {
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
            // leads ([`Request::Find`]).
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
        } else if depth == 1 && !helper_program(&name) && not_the_game(path).is_none() {
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
pub(super) fn not_the_game(exe: &Path) -> Option<&'static str> {
    let name = exe
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    let stem = name.strip_suffix(".exe").unwrap_or(&name);
    let launcher = [
        "steam",
        "upc",
        "ubisoftconnect",
        "battle.net",
        "eadesktop",
        "origin",
        "galaxyclient",
        "amazon games",
        "amazon games ui",
        "itch",
        "heroic",
        "xboxpcapp",
        "gamelaunchhelper",
    ]
    .contains(&stem)
        || stem.contains("launcher")
        || stem.starts_with("playnite.");
    let interpreter = [
        "cmd",
        "powershell",
        "powershell_ise",
        "pwsh",
        "wscript",
        "cscript",
        "mshta",
        "rundll32",
        "conhost",
        "explorer",
        "py",
        "pyw",
        "node",
        "java",
        "javaw",
        "ruby",
        "rubyw",
        "perl",
        "php",
        "bash",
        "sh",
        "wsl",
        "dotnet",
    ]
    .contains(&stem)
        || stem.starts_with("python");
    let windows = ["SystemRoot", "windir"]
        .into_iter()
        .filter_map(std::env::var_os)
        .filter(|root| !root.is_empty())
        .any(|root| inside(exe, Path::new(&root)));
    if launcher {
        Some("a launcher")
    } else if interpreter {
        Some("a script host or interpreter")
    } else if windows {
        Some("part of Windows")
    } else {
        None
    }
}

fn helper_program(name: &str) -> bool {
    [
        "crash",
        "report",
        "launcher",
        "unins",
        "setup",
        "install",
        "redist",
        "vcredist",
        "vc_redist",
        "dxsetup",
        "dotnet",
        "helper",
        "updater",
        "anticheat",
        "_be.exe",
        "beservice",
        "start_protected",
        "cefprocess",
        "webhelper",
        "touchup",
        "cleanup",
    ]
    .iter()
    .any(|part| name.contains(part))
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
pub(super) fn inside(path: &Path, root: &Path) -> bool {
    let mut parts = path.components();
    root.components().all(|part| {
        parts.next().is_some_and(|other| {
            part.as_os_str().to_string_lossy().to_lowercase()
                == other.as_os_str().to_string_lossy().to_lowercase()
        })
    })
}
fn same(a: &Path, b: &Path) -> bool {
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
    ["common", "Program Files", "Program Files (x86)"]
        .iter()
        .any(|other| name.eq_ignore_ascii_case(other))
        || shared.iter().any(|root| same(folder, root))
}

/// The folders known by place to hold other programs or their data: the
/// Program Files folders and the user's profile and `AppData` folders
/// (`%LOCALAPPDATA%\BattlEye` is no game's own).
pub(super) fn shared_folders() -> Vec<PathBuf> {
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
fn refuse_anti_cheat(exe: &Path, install: Option<&Path>) -> Outcome<()> {
    refuse_anti_cheat_in(exe, install, &shared_folders())
}
/// [`refuse_anti_cheat_in`] for a test's own shared folders, and why it refused.
#[cfg(test)]
pub(super) fn anti_cheat_refusal(
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
fn refuse_without_frame_generation(exe: &Path, install: Option<&Path>) -> Outcome<()> {
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
fn state(folder: &Path, manifest: &Manifest, path: &str, quick: bool) -> Now {
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

/// Refuse a link on the way from the game's folder to `relative` in it, the
/// two included: a junction or symbolic link planted there would send a
/// copy or a delete elsewhere. A SYSTEM host also refuses a folder on the
/// way that anyone but administrators may change.
fn guard(folder: &Path, relative: &str, writer: Writer) -> Outcome<()> {
    let parts: Vec<&str> = relative
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
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

/// Who may change a folder, as [`verdict`] judges it.
#[derive(Debug)]
pub(super) struct Security {
    pub link: bool,
    pub owner: String,
    /// Those its permissions give the [`Rights`] asked about.
    pub writers: Vec<String>,
}

/// What [`security`] asks who may do to a folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Rights {
    /// Change it or what it holds.
    Change,
    /// Move or delete what it holds, as for a drive's root, which lets
    /// users add folders, which moves nothing.
    MoveContents,
    /// That, or move or delete the folder itself, as for a folder above the
    /// cache: C:\ProgramData lets users add folders too.
    Move,
}

pub(super) fn security(path: &Path, rights: Rights) -> anyhow::Result<Security> {
    use butterpollo_windows::process::{
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
fn account(sid: &str) -> String {
    butterpollo_windows::process::account_name(sid).unwrap_or_else(|| "another account".into())
}

/// Whether only administrators may change `path`.
pub(super) fn verdict(path: &Path, security: &Security) -> Result<(), String> {
    if security.link {
        return Err(format!("{} is a link to elsewhere", path.display()));
    }
    if !ADMINISTRATORS.contains(&security.owner.as_str()) {
        return Err(format!(
            "{} is owned by {}, not by SYSTEM or Administrators",
            path.display(),
            account(&security.owner)
        ));
    }
    match security
        .writers
        .iter()
        .find(|sid| !ADMINISTRATORS.contains(&sid.as_str()))
    {
        Some(other) => Err(format!(
            "{} can be changed by {}",
            path.display(),
            account(other)
        )),
        None => Ok(()),
    }
}

/// Whether administrators own `path` and alone may change it, or for a
/// folder above the cache (not `whole`) move or delete it or what it holds
/// ([`Rights::Move`], [`verdict`]); the cache's check ([`super::trusted_root`]).
pub(crate) fn administrators_hold(path: &Path, whole: bool) -> Result<(), String> {
    hold(path, whole, security)
}

/// [`administrators_hold`], with `read` in place of [`security`].
pub(super) fn hold(
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
pub(super) fn administrators_only(folder: &Path) -> Result<(), String> {
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
fn writable(folder: &Path, writer: Writer) -> Outcome<()> {
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

/// The manifest `name` in `folder`, None when there is none.
fn manifest(folder: &Path, name: &str) -> Result<Option<Manifest>, String> {
    match std::fs::read_to_string(folder.join(name)) {
        Ok(text) => Manifest::parse(&text)
            .map(Some)
            .map_err(|e| format!("{name}: {e} ({})", folder.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("cannot read {}: {e}", folder.join(name).display())),
    }
}

fn write_manifest(folder: &Path, manifest: &Manifest, writer: Writer) -> Outcome<()> {
    let path = folder.join(MANIFEST);
    let partial = partial(&path);
    guard(folder, MANIFEST, writer)?;
    guard(folder, &format!("{MANIFEST}{PARTIAL}"), writer)?;
    let text = serde_json::to_string_pretty(manifest).map_err(|e| Failure::Error(e.to_string()))?;
    std::fs::write(&partial, text)
        .and_then(|()| std::fs::rename(&partial, &path))
        .map_err(|e| {
            let _ = std::fs::remove_file(&partial);
            Failure::Error(format!("cannot write {}: {e}", path.display()))
        })
}

/// What `path` is called until it is complete.
fn partial(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(PARTIAL);
    name.into()
}

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
        Ok(bytes) if bytes.windows(10).any(|w| w == b"OptiScaler") => Occupant::OptiScaler,
        _ => Occupant::Foreign,
    }
}

/// Where and how the files go: the full package under a free proxy name,
/// or the add-on into the OptiScaler the user installed.
struct Layout {
    mode: Mode,
    /// OptiScaler's name: the one the host gives it, or the user's own.
    proxy: String,
    /// OptiScaler's folder (with `/`, empty for the game folder).
    opti_dir: String,
    /// Folders an earlier install left behind ([`leftover`]), parents
    /// first: the new manifest takes them over as its own.
    adopt: Vec<String>,
}

/// The folder `root` in the game's folder as an earlier install left it
/// once its manifest went: `Ok` with its folders, parents first, when it
/// holds nothing but folders, what the provider writes in its own folder
/// ([`neural::provider_files`]) and the provider's settings, which the
/// install keeps as they are ([`place`]); otherwise `Err` with what else is
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

/// Why paths in the game's folder stop an install. What is in the
/// provider's folder is still there from an earlier install (one of its
/// files changed since, or something put in its folder), anything else is
/// not Rubylight's. Either is the user's to move out: the host deletes
/// neither.
fn in_the_way(paths: &[String]) -> String {
    let (earlier, others): (Vec<String>, Vec<String>) = paths.iter().cloned().partition(|path| {
        path.split('/')
            .any(|part| part.eq_ignore_ascii_case(neural::PROVIDER))
    });
    let named = |paths: &[String]| {
        let mut text = paths.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
        if paths.len() > 3 {
            text.push_str(&format!(" and {} more", paths.len() - 3));
        }
        (text, if paths.len() == 1 { "is" } else { "are" })
    };
    let mut clauses = Vec::new();
    if !others.is_empty() {
        let (text, be) = named(&others);
        clauses.push(format!(
            "{text} {be} in the game's folder and {be} not Rubylight's"
        ));
    }
    if !earlier.is_empty() {
        let (text, be) = named(&earlier);
        clauses.push(format!(
            "{text} {be} still in the game's folder from an earlier install"
        ));
    }
    let it = if paths.len() == 1 { "it" } else { "them" };
    format!("{}: move {it} out first", clauses.join("; "))
}

/// What a removal of Rubylight 3.0's AMDNR would take out of the game's
/// folder, for [`Request::Check`] to ask about the folder as it will be:
/// the files its manifest names that are still as placed and its settings
/// ([`neural::removal`]), OptiScaler's logs, and its folders once nothing
/// else is in them. Empty for an install, which sees the folder as it is.
#[derive(Default)]
struct Gone {
    /// AMDNR's manifest is in the folder, and goes too.
    legacy: bool,
    /// In ASCII lower case, with `/`.
    paths: Vec<String>,
}
impl Gone {
    fn has(&self, relative: &str) -> bool {
        let relative = relative.to_ascii_lowercase();
        self.paths.contains(&relative)
    }
    fn of_legacy(folder: &Path, writer: Writer) -> Outcome<Self> {
        guard(folder, LEGACY_MANIFEST, writer)?;
        let Some(legacy) = manifest(folder, LEGACY_MANIFEST).map_err(Failure::Error)? else {
            return Ok(Self::default());
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
        // What the removal deletes beside them ([`remove_by`]): partial copies.
        let partials: Vec<String> = legacy
            .files
            .keys()
            .map(|path| format!("{path}{PARTIAL}").to_ascii_lowercase())
            .collect();
        // What the removal keeps stops the install after it ([`kept_text`]):
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
        if !kept.is_empty() {
            kept.sort();
            kept.dedup();
            return Err(Failure::Refused(kept_text(&kept, false)));
        }
        Ok(Self {
            legacy: true,
            paths,
        })
    }
}

/// Why an install stops at the earlier upscaling's files it would keep
/// (`kept`): once its removal left them (`removed`), or before anything
/// changed.
pub(super) fn kept_text(kept: &[String], removed: bool) -> String {
    let mut named: Vec<String> = kept
        .iter()
        .take(3)
        .map(|path| neural::shown(path))
        .collect();
    if kept.len() > 3 {
        named.push(format!("{} more", kept.len() - 3));
    }
    let (be, it, they) = if kept.len() == 1 {
        ("is", "it", "it")
    } else {
        ("are", "them", "they")
    };
    let named = named.join(", ");
    if removed {
        format!(
            "the earlier upscaling was removed, but {named} {be} still in the game's folder: {they} changed since or {be} not Rubylight's, so frame generation was not installed; move {it} out and turn it on again"
        )
    } else {
        format!(
            "the earlier upscaling can't be removed whole: {named} changed since or {be} not Rubylight's and would stay in the game's folder, so nothing was changed; move {it} out and turn it on again"
        )
    }
}

/// The layout for the game's folder, or why it is refused: Rubylight 3.0's
/// AMDNR still there, an OptiScaler of the user's that can't load the
/// provider, another mod, or every name taken. An earlier install keeps its
/// mode. What `gone` names counts as not there.
fn layout(
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

fn place(
    exe: &Path,
    cached: &Cached,
    writer: Writer,
    progress: &dyn Fn(u64, u64),
) -> Outcome<Response> {
    place_or_check(exe, cached, writer, progress, None)
}

/// [`Request::Check`]: what [`place`] refuses before it writes anything,
/// with Rubylight 3.0's AMDNR taken as removed.
fn check(exe: &Path, cached: &Cached, writer: Writer) -> Outcome<Response> {
    let gone = Gone::of_legacy(folder_of(exe), writer)?;
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
        let stale: Vec<String> = manifest
            .files
            .keys()
            .filter(|path| {
                !neural::settings(path) && !wanted.iter().any(|(target, _)| target == *path)
            })
            .cloned()
            .collect();
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
        if first && let Err(left) = remove(folder, writer) {
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

fn prepare(
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
            .inside(&cached.folder)
            .map_err(Failure::Error)?;
        return place(exe, &cached, writer, progress);
    }
    if !manifest.previous.is_empty() {
        return Err(Failure::Error(format!(
            "an update of Neural Rendering in {} was cut short: turn it off and on again",
            folder.display()
        )));
    }
    for path in manifest.files.keys() {
        let state = state(folder, &manifest, path, true);
        if state != Now::Same && !(neural::settings(path) && state == Now::Changed) {
            let how = if state == Now::Missing {
                "is missing"
            } else {
                "changed"
            };
            return Err(Failure::Error(format!(
                "Neural Rendering's {path} {how} in {}: turn it off and on again",
                folder.display()
            )));
        }
    }
    // The user's settings it found are used as they are, but are needed:
    // turning it off and on again places the release's.
    for path in &manifest.taken_over {
        if std::fs::symlink_metadata(folder.join(path)).is_err() {
            return Err(Failure::Error(format!(
                "Neural Rendering's {path} is missing in {}: turn it off and on again",
                folder.display()
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

/// Delete what the host placed in the folder, by each of its manifests:
/// this release's and Rubylight 3.0's AMDNR.
fn remove(folder: &Path, writer: Writer) -> Outcome<Response> {
    // A folder that is gone with its game holds nothing; one on a drive that
    // is not connected may, and is not called removed. So may a game folder
    // moved and linked back whose link leads nowhere now ([`followed`]).
    let dangling = std::fs::symlink_metadata(folder)
        .is_ok_and(|meta| meta.file_attributes() & LINK != 0)
        && std::fs::canonicalize(folder).is_err();
    if dangling || (!folder.is_dir() && folder.ancestors().last().is_none_or(|root| !root.is_dir()))
    {
        return Ok(Response::Unreachable {
            folder: folder.display().to_string(),
        });
    }
    let mut kept = Vec::new();
    for name in [MANIFEST, LEGACY_MANIFEST] {
        kept.extend(remove_by(folder, name, writer)?);
    }
    Ok(Response::Removed { kept })
}

/// Delete what the manifest `name` names and is still as placed, the DLLs
/// first ([`neural::Removal::delete`]), so a game that holds them stops the
/// removal before anything changed; then put the
/// user's OptiScaler.ini lines back, delete OptiScaler's logs, what the
/// provider wrote in its folder ([`neural::provider_files`]) and the folders
/// it created once empty. A file someone changed stays, as do the settings
/// it took over and anything else in those folders ([`left_in`]); all are
/// returned.
fn remove_by(folder: &Path, name: &str, writer: Writer) -> Outcome<Vec<String>> {
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
    butterpollo_windows::files::delete_all(&dlls).map_err(|(file, e)| {
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
    guard(folder, name, writer)?;
    let path = folder.join(name);
    std::fs::remove_file(&path)
        .map_err(|e| Failure::Error(format!("cannot remove {}: {e}", path.display())))?;
    Ok(kept)
}

/// What is still in the folders the host created (`folders`) once its
/// files are out: each file, link or other folder someone else put there,
/// by its path in the game folder (with `/`), and a folder of the host's
/// that is still there with nothing in it (a program holds it open). It
/// stays, and the removal names it, so the folder it holds is never a
/// surprise.
fn left_in(folder: &Path, folders: &[String]) -> Vec<String> {
    let ours = |relative: &str| folders.iter().any(|f| f.eq_ignore_ascii_case(relative));
    let mut left = Vec::new();
    for relative in folders {
        let Ok(entries) = std::fs::read_dir(folder.join(relative)) else {
            continue;
        };
        // Whatever is in it names it: its own entries here, a folder of the
        // host's in it by its own.
        let mut entries = entries.flatten().peekable();
        if entries.peek().is_none() {
            left.push(relative.clone());
        }
        for entry in entries {
            let path = format!("{relative}/{}", entry.file_name().to_string_lossy());
            // A folder the host made is named by its own contents.
            let made = entry
                .file_type()
                .is_ok_and(|kind| kind.is_dir() && !kind.is_symlink())
                && ours(&path);
            if !made {
                left.push(path);
            }
        }
    }
    left
}
