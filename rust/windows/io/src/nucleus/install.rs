//! Nucleus Co-op on this PC: the folder the user unpacked the pinned release
//! into and chose in the console (`nucleus_path`), else the copy an earlier
//! Rubylight installed below the host's data folder. Rubylight downloads and
//! unpacks nothing: the console links to the release's page.
//!
//! Nucleus runs elevated and loads its DLLs, its .config and its handlers
//! from its folder, and the host writes its handlers, game list and settings
//! there as SYSTEM. So the whole folder must be one only SYSTEM,
//! Administrators and TrustedInstaller own and change ([`TRUSTED`]), with no
//! link anywhere in it, on a fixed drive of this PC, below folders nobody
//! else may move: otherwise a program of its user could run elevated, or
//! point the host's writes elsewhere. Each program file must be the pinned
//! release's, by size and SHA-256 ([`pinned::release`]), and none may carry
//! the Mark of the Web. The check reads every file and folder by a handle
//! that never follows a link, and a launch holds the folder and its
//! NucleusCoop.exe open from the check until Windows started it ([`hold`]).
use anyhow::{Context, Result, bail};
use butterpollo_session::nucleus::install::{self as pinned, Manifest, Problem, Unusable};
use std::collections::{HashMap, HashSet};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

mod left;
mod volume;

#[cfg(test)]
use left::left_in_checked;
pub use left::{Left, left_in};
#[cfg(test)]
use volume::{
    DRIVE_CDROM, DRIVE_FIXED, DRIVE_NO_ROOT_DIR, DRIVE_RAMDISK, DRIVE_REMOTE, DRIVE_REMOVABLE,
    drive_kind, drive_refusal,
};
use volume::{Volume, as_shipped, blocked, normal, placed, same_drive, volume_of};

const REPARSE_POINT: u32 = 0x400;
const DIRECTORY: u32 = 0x10;

/// The one fix for a folder others can change or move.
const PROGRAM_FILES: &str =
    "Unpack the release into a new folder under C:\\Program Files and choose that one";

/// The Nucleus folder streams use, by the path Windows resolved it to:
/// `nucleus_path` when set, which must be a folder of the pinned release
/// that only SYSTEM and Administrators may change ([`Unusable`] says why
/// not; the host's earlier copy stands in for it only when it holds another
/// release, [`Found::instead`]), else the host's earlier copy below `data`
/// while it is there.
pub fn installed(data: &Path, configured: &str) -> Result<Option<PathBuf>> {
    Ok(found(data, configured)?.map(|found| found.folder))
}

/// The Nucleus folder [`installed`] names, and why `nucleus_path` doesn't
/// name it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub folder: PathBuf,
    /// Why the folder chosen is not used: it holds another release, and
    /// the copy an earlier Rubylight installed runs instead.
    pub instead: Option<Unusable>,
}
impl Found {
    /// What the console says while the earlier copy runs instead of the
    /// folder chosen.
    pub fn notice(&self) -> Option<String> {
        self.instead.as_ref().map(|why| {
            format!(
                "{why}. Until then Rubylight runs the co-op helper {} an earlier Rubylight installed, in {}",
                pinned::VERSION,
                self.folder.display()
            )
        })
    }
}

/// How long [`found`] keeps a folder it accepted: the console asks every
/// few seconds, and Moonlight's app list too. A refusal is never kept, and
/// a launch checks again ([`hold`]).
const KEPT_FOR: Duration = Duration::from_secs(10);

/// [`installed`] with why the folder chosen is not used.
pub fn found(data: &Path, configured: &str) -> Result<Option<Found>> {
    type Kept = Option<(PathBuf, String, Instant, Option<Found>)>;
    static KEPT: Mutex<Kept> = Mutex::new(None);
    let configured = configured.trim();
    {
        let kept = KEPT.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((_, _, _, found)) = kept
            .as_ref()
            .filter(|(d, c, at, _)| d == data && c == configured && at.elapsed() < KEPT_FOR)
        {
            return Ok(found.clone());
        }
    }
    let found = found_checked(data, configured, &Checks::pc())?;
    *KEPT.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((
        data.to_path_buf(),
        configured.to_string(),
        Instant::now(),
        found.clone(),
    ));
    Ok(found)
}

/// Whether the account (a SID) may own and change the file or folder at the
/// path.
type Trusts<'a> = Box<dyn Fn(&Path, &str) -> bool + 'a>;
/// The drive at a root (`C:\`).
type VolumeAt<'a> = Box<dyn Fn(&str) -> Result<Volume> + 'a>;

/// What differs in tests: a test process may not own files as
/// Administrators, its temporary folder lies below its user's profile, it
/// can't hold the release's files, and it can't swap a drive.
struct Checks<'a> {
    /// Whether the account (a SID) may own and change the file or folder at
    /// the path: one of [`TRUSTED`], for the folder, what is in it and the
    /// folders above it.
    trusts: Trusts<'a>,
    /// The folder above which no folder is checked; None on the PC: every
    /// one up to the drive's root.
    top: Option<&'a Path>,
    release: &'a Manifest,
    /// The drive at a root (`C:\`), [`volume_of`].
    volume: VolumeAt<'a>,
    /// The drive each folder was on when it was first checked, by its
    /// lowercase path ([`same_drive`]).
    volumes: &'a Mutex<HashMap<String, Volume>>,
}
impl Checks<'static> {
    fn pc() -> Self {
        static VOLUMES: std::sync::LazyLock<Mutex<HashMap<String, Volume>>> =
            std::sync::LazyLock::new(Mutex::default);
        Self {
            trusts: Box::new(|_, sid| TRUSTED.contains(&sid)),
            top: None,
            release: pinned::release(),
            volume: Box::new(volume_of),
            volumes: &VOLUMES,
        }
    }
}
impl Checks<'_> {
    fn trusts(&self, path: &Path, sid: &str) -> bool {
        (self.trusts)(path, sid)
    }
}

fn found_checked(data: &Path, configured: &str, checks: &Checks) -> Result<Option<Found>> {
    let earlier = pinned::folder(data);
    // Checked like a chosen one: an earlier Rubylight unpacked it there.
    let earlier_found = || -> Result<Option<Found>> {
        if std::fs::symlink_metadata(earlier.join(pinned::EXECUTABLE)).is_err() {
            return Ok(None);
        }
        let checked = chosen(&earlier, checks)?;
        Ok(Some(Found {
            folder: checked.folder,
            instead: None,
        }))
    };
    if configured.is_empty() {
        return earlier_found();
    }
    let error = match chosen(Path::new(configured), checks) {
        Ok(checked) => {
            return Ok(Some(Found {
                folder: checked.folder,
                instead: None,
            }));
        }
        Err(error) => error,
    };
    // Another release chosen while an earlier Rubylight's copy of this one
    // is there: that one runs until the right one is chosen.
    let Some(why) = error
        .downcast_ref::<Unusable>()
        .filter(|why| why.problem == Problem::Version)
    else {
        return Err(error);
    };
    match earlier_found() {
        Ok(Some(found)) => Ok(Some(Found {
            instead: Some(why.clone()),
            ..found
        })),
        Ok(None) => Err(error),
        Err(left) => {
            tracing::warn!(folder = %earlier.display(), error = format!("{left:#}"), "Co-op helper: the copy an earlier Rubylight installed does not stand in for the folder chosen");
            Err(error)
        }
    }
}

fn refuse(problem: Problem, message: String) -> anyhow::Error {
    anyhow::Error::new(Unusable { problem, message })
}

/// The Nucleus folder, checked ([`chosen`]) and held open: neither it nor
/// its NucleusCoop.exe can be renamed, deleted or replaced while this lives,
/// and nobody may write the program. Launch from [`Held::folder`], the path
/// Windows resolved it to, and drop this once Windows started it.
pub struct Held {
    folder: PathBuf,
    _folder: std::fs::File,
    _program: std::fs::File,
}
impl Held {
    /// The folder by the path Windows resolved it to.
    pub fn folder(&self) -> &Path {
        &self.folder
    }
    /// Its NucleusCoop.exe, which Windows starts.
    pub fn program(&self) -> PathBuf {
        self.folder.join(pinned::EXECUTABLE)
    }
}

/// The Nucleus folder at `folder` checked again, every file of it, and held
/// until Windows starts Nucleus from it ([`Held`]).
pub fn hold(folder: &Path) -> Result<Held> {
    hold_checked(folder, &Checks::pc())
}

fn hold_checked(folder: &Path, checks: &Checks) -> Result<Held> {
    let checked = chosen(folder, checks)?;
    Ok(Held {
        folder: checked.folder,
        _folder: checked.handle,
        _program: checked.program,
    })
}

/// A folder [`chosen`] accepted, still open.
struct Checked {
    folder: PathBuf,
    handle: std::fs::File,
    program: std::fs::File,
}

/// The folder at `folder`, when Rubylight runs Nucleus Co-op from it: a
/// folder on a fixed drive of this PC named by its own full path, holding
/// the pinned release whole and unblocked, owned and changed only by those
/// `checks` trusts, as is every file and folder in it, with no link in it or
/// on its way, below folders only they may move, on a drive inside this PC
/// and the one it was on when it was first checked.
fn chosen(folder: &Path, checks: &Checks) -> Result<Checked> {
    let shown = folder.display();
    let version = pinned::VERSION;
    let exe_name = pinned::EXECUTABLE;
    let (root, volume) = placed(folder, checks)?;
    // Never followed: a link would be checked here and run from elsewhere.
    let handle = match crate::process::open_unfollowed(folder, true) {
        Ok(handle) => handle,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(refuse(
                Problem::Missing,
                format!(
                    "{shown} doesn't exist: choose the folder you unpacked the co-op helper {version} into"
                ),
            ));
        }
        Err(error) => return Err(error).with_context(|| format!("opening {shown}")),
    };
    let attributes = attributes_of(&handle, folder)?;
    if attributes & REPARSE_POINT != 0 {
        return Err(refuse(
            Problem::Untrusted,
            format!("{shown} is a link to elsewhere: choose the folder itself"),
        ));
    }
    let exe = folder.join(exe_name);
    let no_exe = || {
        refuse(
            Problem::Missing,
            format!(
                "{shown} doesn't hold the co-op helper: choose the folder you unpacked the co-op helper {version} into"
            ),
        )
    };
    if attributes & DIRECTORY == 0 {
        return Err(no_exe());
    }
    let program = match open_program(&exe) {
        Ok(program) => program,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Err(no_exe()),
        Err(error) => return Err(error).with_context(|| format!("opening {}", exe.display())),
    };
    if attributes_of(&program, &exe)? & (REPARSE_POINT | DIRECTORY) != 0 {
        return Err(refuse(
            Problem::Untrusted,
            format!("{} is a link to elsewhere", exe.display()),
        ));
    }
    // By the path Windows resolved: a link or an 8.3 short name on the way
    // would be checked here and lead elsewhere later.
    let resolved =
        normal(&crate::user_folder::final_path(&handle).context("reading the folder's path")?);
    if !crate::user_folder::same_name(
        std::ffi::OsStr::new(&resolved),
        std::ffi::OsStr::new(&normal(folder.as_os_str())),
    ) {
        return Err(refuse(
            Problem::Untrusted,
            format!(
                "{shown} leads to {resolved} through a link or a short name: choose {resolved} itself"
            ),
        ));
    }
    let canonical = PathBuf::from(&resolved);
    let opened =
        normal(&crate::user_folder::final_path(&program).context("reading the program's path")?);
    let expected = canonical.join(exe_name);
    if !crate::user_folder::same_name(std::ffi::OsStr::new(&opened), expected.as_os_str()) {
        bail!(
            "{} opened as {opened}, not in {}",
            exe.display(),
            canonical.display()
        );
    }
    // Which release first, then whether it was unblocked, then who may
    // change it: the first fix that helps.
    let shipped = checks
        .release
        .program(exe_name)
        .context("the pinned release lists no program")?;
    if !as_shipped(&expected, &program, shipped)? {
        return Err(refuse(
            Problem::Version,
            format!(
                "The co-op helper in {} isn't version {version}: download {version} from its release page and choose its folder",
                canonical.display()
            ),
        ));
    }
    blocked(&expected)?;
    above(&canonical, checks)?;
    let owner = crate::process::open_file_owner(&handle)
        .with_context(|| format!("reading the owner of {}", canonical.display()))?;
    let writers = crate::process::open_file_writers(&handle)
        .with_context(|| format!("reading the permissions of {}", canonical.display()))?;
    trusted(&canonical, &owner, &writers, checks)?;
    walk(&canonical, checks)?;
    same_drive(&canonical, &root, &volume, checks)?;
    Ok(Checked {
        folder: canonical,
        handle,
        program,
    })
}

fn attributes_of(file: &std::fs::File, path: &Path) -> Result<u32> {
    Ok(file
        .metadata()
        .with_context(|| format!("reading {}", path.display()))?
        .file_attributes())
}

/// The program `path`, never through a link, open for reading only and
/// shared only for reading: it can't be written, renamed or deleted while it
/// is open.
fn open_program(path: &Path) -> std::io::Result<std::fs::File> {
    use windows::Win32::Storage::FileSystem::{FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ};
    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ.0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)
}

/// Each folder above `folder` (the path Windows resolved), from the drive's
/// root down, up to [`Checks::top`]: no link, owned by those `checks`
/// trusts, and nobody else may rename or delete it, or move what it holds
/// (a drive's root itself can't be moved). Otherwise a program of its user
/// could move the checked folder aside and put another in its place between
/// the check and the launch.
fn above(folder: &Path, checks: &Checks) -> Result<()> {
    let mut ancestors: Vec<&Path> = folder.ancestors().skip(1).collect();
    ancestors.reverse();
    for path in ancestors {
        if checks.top.is_some_and(|top| !path.starts_with(top)) {
            continue;
        }
        let file = crate::process::open_unfollowed(path, true)
            .with_context(|| format!("opening {}", path.display()))?;
        if attributes_of(&file, path)? & REPARSE_POINT != 0 {
            return Err(refuse(
                Problem::Untrusted,
                format!("{} is a link to elsewhere. {PROGRAM_FILES}", path.display()),
            ));
        }
        let owner = crate::process::open_file_owner(&file)
            .with_context(|| format!("reading the owner of {}", path.display()))?;
        let movers = if path.parent().is_none() {
            crate::process::open_file_child_movers(&file)
        } else {
            crate::process::open_file_movers(&file)
        }
        .with_context(|| format!("reading the permissions of {}", path.display()))?;
        let refused = |why: String| {
            refuse(
                Problem::Untrusted,
                format!(
                    "Rubylight doesn't run the co-op helper from {}: it runs elevated, and {why}, so the folder could be swapped. {PROGRAM_FILES}",
                    folder.display()
                ),
            )
        };
        if !checks.trusts(path, &owner) {
            return Err(refused(format!(
                "{} is owned by {owner}, not by SYSTEM or Administrators",
                path.display()
            )));
        }
        if let Some(other) = movers
            .iter()
            .find(|sid| grants_alone(sid) && !checks.trusts(path, sid))
        {
            return Err(refused(format!(
                "{} can be moved or renamed by {other}",
                path.display()
            )));
        }
    }
    Ok(())
}

/// An error naming `path` unless those `checks` trusts own it (`owner`) and
/// alone may change it (`writers`, [`crate::process::open_file_writers`]).
fn trusted(path: &Path, owner: &str, writers: &[String], checks: &Checks) -> Result<()> {
    verdict(path, owner, writers, |sid| checks.trusts(path, sid)).map_err(|why| {
        refuse(
            Problem::Untrusted,
            format!(
                "Rubylight doesn't run the co-op helper from there: it runs elevated, so only administrators may change its files ({why:#}). {PROGRAM_FILES}, rather than moving it there"
            ),
        )
    })
}

/// A file or folder below the Nucleus folder opened to check it: never
/// through a link, for its attributes and permissions only, so a file
/// Nucleus has open is no matter.
fn open_checked(path: &Path, folder: bool) -> std::io::Result<std::fs::File> {
    use windows::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, READ_CONTROL,
    };
    let mut flags = FILE_FLAG_OPEN_REPARSE_POINT;
    if folder {
        flags |= FILE_FLAG_BACKUP_SEMANTICS;
    }
    std::fs::OpenOptions::new()
        .access_mode(READ_CONTROL.0 | FILE_READ_ATTRIBUTES.0)
        .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0)
        .custom_flags(flags.0)
        .open(path)
}

/// Every file and folder below `folder` (checked itself), never following a
/// link: each with no link, owned and changed only by those `checks` trusts,
/// each program file as the release ships it and unblocked, and none of
/// them missing. Nucleus' game instances below `content\<game>` (links to
/// the game's own files, or a whole copy of a game) are checked at that
/// folder only, which like the rest has no link and is owned and changed
/// only by those `checks` trusts: the elevated Nucleus makes `InstanceN`
/// there and starts the game from it, so a folder others could change would
/// let them plant a link or a DLL it follows or runs elevated. Nucleus 2.4.2
/// sets no permissions anywhere, so what it makes there inherits
/// `content`'s.
fn walk(folder: &Path, checks: &Checks) -> Result<()> {
    let mut found = HashSet::new();
    let mut pending = vec![PathBuf::new()];
    while let Some(below) = pending.pop() {
        let path = folder.join(&below);
        let entries =
            std::fs::read_dir(&path).with_context(|| format!("listing {}", path.display()))?;
        for entry in entries {
            let entry = entry.with_context(|| format!("listing {}", path.display()))?;
            let relative = below.join(entry.file_name());
            let path = folder.join(&relative);
            let name = relative.to_string_lossy();
            // From the listing itself, which never follows a link, and then
            // from the handle.
            let listed = entry
                .metadata()
                .with_context(|| format!("reading {}", path.display()))?
                .file_attributes();
            let file = match open_checked(&path, listed & DIRECTORY != 0) {
                Ok(file) => file,
                // Gone since it was listed: Nucleus' own temporary file.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(error).with_context(|| format!("opening {}", path.display()));
                }
            };
            let attributes = listed | attributes_of(&file, &path)?;
            if attributes & REPARSE_POINT != 0 {
                return Err(refuse(
                    Problem::Untrusted,
                    format!(
                        "{} is a link to elsewhere, which an elevated co-op helper would follow: unpack the release again into a new folder under C:\\Program Files",
                        path.display()
                    ),
                ));
            }
            let owner = crate::process::open_file_owner(&file)
                .with_context(|| format!("reading the owner of {}", path.display()))?;
            let is_folder = attributes & DIRECTORY != 0;
            let mut parts = relative.components();
            let instance = is_folder
                && parts
                    .next()
                    .is_some_and(|first| first.as_os_str().eq_ignore_ascii_case("content"))
                && parts.next().is_some()
                && parts.next().is_none();
            let writers = crate::process::open_file_writers(&file)
                .with_context(|| format!("reading the permissions of {}", path.display()))?;
            trusted(&path, &owner, &writers, checks)?;
            if instance {
                continue;
            }
            if is_folder {
                pending.push(relative);
                continue;
            }
            let Some(shipped) = checks.release.program(&name) else {
                continue;
            };
            found.insert(shipped.path.to_ascii_lowercase());
            blocked(&path)?;
            if !as_shipped(&path, &file, shipped)? {
                return Err(refuse(
                    Problem::Version,
                    format!(
                        "{} isn't the co-op helper {}'s: unpack the whole release again into a new folder under C:\\Program Files and choose that one",
                        path.display(),
                        pinned::VERSION
                    ),
                ));
            }
        }
    }
    if let Some(missing) = checks
        .release
        .programs()
        .find(|shipped| !found.contains(&shipped.path.to_ascii_lowercase()))
    {
        return Err(refuse(
            Problem::Version,
            format!(
                "{} is missing from the co-op helper {}: unpack the whole release again into a new folder under C:\\Program Files and choose that one",
                folder.join(&missing.path).display(),
                pinned::VERSION
            ),
        ));
    }
    Ok(())
}

/// SYSTEM's, the Administrators group's and TrustedInstaller's (what Program
/// Files holds).
pub(super) const TRUSTED: [&str; 3] = [
    "S-1-5-18",
    "S-1-5-32-544",
    "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464",
];

/// Whether those `trusts` trusts own the file or folder at `path` (`owner`)
/// and alone may change it (`others`).
fn verdict(
    path: &Path,
    owner: &str,
    others: &[String],
    trusts: impl Fn(&str) -> bool,
) -> Result<()> {
    if !trusts(owner) {
        bail!(
            "{} is owned by {owner}, not by SYSTEM or Administrators",
            path.display()
        );
    }
    if let Some(other) = others.iter().find(|sid| grants_alone(sid) && !trusts(sid)) {
        bail!("{} can be changed by {other}", path.display());
    }
    Ok(())
}

/// Whether an entry for `sid` lets anyone do what it allows. One for an app
/// package or a capability (`S-1-15-2-…`, `S-1-15-3-…`) never does by
/// itself: Windows grants a process in an app container only what its
/// user's own SIDs are allowed too, and a process outside one holds no such
/// SID. Windows 11 puts a capability's entry on `C:\`.
fn grants_alone(sid: &str) -> bool {
    !(sid.starts_with("S-1-15-2-") || sid.starts_with("S-1-15-3-"))
}

#[cfg(test)]
mod tests;
