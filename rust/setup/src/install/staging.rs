//! Package staging, installation file copying, service switching and Windows registration.
use crate::{
    detect::{self, UNINSTALL},
    log::line,
    payload::{self, Payload},
    system::{self, Value},
    ui::Progress,
};
use anyhow::{Context, Result, bail};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use windows::Win32::System::Registry::HKEY_LOCAL_MACHINE;

/// Create `install` if needed and give it the service folder's permissions,
/// before setup stages anything in it. Returns whether it was created.
pub(super) fn create_install(install: &Path) -> Result<bool> {
    let created = std::fs::symlink_metadata(install).is_err();
    std::fs::create_dir_all(install)?;
    secure_install(install)?;
    Ok(created)
}
/// Setup's staging folder, inside the installation folder: the package of a
/// full installation (`package`), each update's transaction (new files and
/// the backup) and the host's update downloads (`download-<id>`) go there. Only administrators may change the
/// installation folder; elevated programs run from ProgramData, where
/// earlier setups staged, look like malware (Defender's cloud model quarantined
/// the staged service there).
pub(crate) fn staging_root(install: &Path) -> PathBuf {
    install.join(".staging")
}
/// [`staging_root`] of an installation folder setup has already secured,
/// limited to SYSTEM and Administrators. A link in its place or inside it
/// is refused, never followed.
pub(crate) fn prepare_staging_root(install: &Path) -> Result<PathBuf> {
    let root = staging_root(install);
    match std::fs::symlink_metadata(&root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => std::fs::create_dir(&root)?,
        Err(error) => return Err(error.into()),
        Ok(_) => {}
    }
    system::restrict(&root, false).with_context(|| format!("securing {}", root.display()))?;
    Ok(root)
}
/// The staged package of a full installation. Dropping it deletes the
/// package, the staging folder once nothing else is in it (a failed
/// update's backup stays), and an installation folder this run created that
/// is still empty: a setup that fails leaves no folder behind.
pub(super) struct Staging {
    package: PathBuf,
    /// Set once the staging folder passed its check: a link in its place
    /// would lead the removal elsewhere.
    checked: bool,
    created: Option<PathBuf>,
}
impl Staging {
    pub(super) fn path(&self) -> &Path {
        &self.package
    }
}
impl Drop for Staging {
    fn drop(&mut self) {
        if self.checked {
            match std::fs::remove_dir_all(&self.package) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => line(format!(
                    "the staged package {} could not be removed: {error}",
                    self.package.display()
                )),
            }
            if let Some(root) = self.package.parent() {
                let _ = std::fs::remove_dir(root);
            }
        }
        if let Some(install) = &self.created {
            let _ = std::fs::remove_dir(install);
        }
    }
}
/// An empty, locked-down package folder in `install`'s staging folder; what
/// an interrupted setup left there is removed first.
pub(super) fn prepare_staging(install: &Path, created: bool) -> Result<Staging> {
    // First, so that a failure from here on also removes a folder this run
    // created.
    let mut staging = Staging {
        package: staging_root(install).join("package"),
        checked: false,
        created: created.then(|| install.to_path_buf()),
    };
    prepare_staging_root(install)?;
    staging.checked = true;
    // `prepare_staging_root` refused a link anywhere in the folder.
    if std::fs::symlink_metadata(staging.path()).is_ok() {
        std::fs::remove_dir_all(staging.path())
            .context("removing the previous setup staging folder")?;
    }
    // A new folder, so nothing made before this point is inside it.
    std::fs::create_dir(staging.path())?;
    // The staged host runs elevated (settings import) and loads DLLs from its
    // own folder: lock the folder down before anything lands in it.
    system::restrict(staging.path(), false)?;
    Ok(staging)
}
/// Unpack and verify the package in `install`'s staging folder.
pub(super) fn unpack(
    payload: &mut Payload,
    install: &Path,
    created: bool,
    progress: &Progress,
) -> Result<(Staging, Vec<payload::Entry>)> {
    progress.set("Unpacking Rubylight…");
    let staging = prepare_staging(install, created)?;
    payload.extract(staging.path())?;
    let entries = payload::verify(staging.path())?;
    Ok((staging, entries))
}
/// Where earlier setups staged the package. It may still hold the
/// programs of an earlier version's setup; a link there is removed itself,
/// never followed.
pub(crate) fn remove_legacy_staging(program_data: &Path) -> Result<()> {
    use std::os::windows::fs::MetadataExt;
    let folder = program_data.join("Butterpollo").join("setup");
    let removed = match std::fs::symlink_metadata(&folder) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => Err(error),
        Ok(metadata) if metadata.file_attributes() & 0x400 != 0 => {
            std::fs::remove_dir(&folder).or_else(|_| std::fs::remove_file(&folder))
        }
        Ok(_) => std::fs::remove_dir_all(&folder),
    };
    removed.with_context(|| format!("removing the earlier staging folder {}", folder.display()))?;
    line(format!(
        "removed the earlier staging folder {}",
        folder.display()
    ));
    Ok(())
}

#[derive(Debug)]
pub(super) struct SwitchFailure {
    pub(super) error: anyhow::Error,
    /// Pointing the service back at the previous installation.
    pub(super) back: Result<()>,
}
/// Point the service at the new installation and start it there. If it does
/// not start, the service is pointed back at the previous installation,
/// whose files were not changed; the caller starts it again.
pub(super) fn switch_service(
    to_new: impl FnOnce() -> Result<()>,
    start: impl FnOnce() -> Result<()>,
    to_old: impl FnOnce() -> Result<()>,
) -> std::result::Result<(), SwitchFailure> {
    if let Err(error) = to_new() {
        // Windows changes a service's configuration whole or not at all.
        return Err(SwitchFailure {
            error,
            back: Ok(()),
        });
    }
    match start() {
        Ok(()) => Ok(()),
        Err(error) => Err(SwitchFailure {
            error,
            back: to_old(),
        }),
    }
}

/// Copy the package, replacing files still loaded by other programs (the
/// Vulkan layer inside a running game) by renaming them first.
pub(super) fn copy_package(
    staging: &Path,
    install: &Path,
    entries: &[payload::Entry],
) -> Result<()> {
    std::fs::create_dir_all(install)?;
    if let Ok(previous) = payload::manifest(install) {
        for old in previous {
            if !entries
                .iter()
                .any(|e| e.path.eq_ignore_ascii_case(&old.path))
                && let Ok(path) = payload::safe_join(install, &old.path)
            {
                system::remove_file_later(&path);
            }
        }
    }
    for entry in entries {
        let source = payload::safe_join(staging, &entry.path)?;
        let target = payload::safe_join(install, &entry.path)?;
        replace_file(&source, &target)?;
    }
    replace_file(
        &staging.join("manifest.json"),
        &install.join("manifest.json"),
    )?;
    // Drivers reused from a previous installation are not in the manifest.
    let drivers = staging.join("drivers");
    if drivers.is_dir() {
        copy_tree(&drivers, &install.join("drivers"))?;
    }
    Ok(())
}
pub(crate) fn replace_file(source: &Path, target: &Path) -> Result<()> {
    crate::update_files::replace(source, target, system::remove_file_later)
}
pub(super) fn copy_tree(source: &Path, target: &Path) -> Result<()> {
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let destination = target.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &destination)?;
        } else if kind.is_file() {
            replace_file(&entry.path(), &destination)?;
        }
    }
    Ok(())
}

pub(super) fn remove_legacy(product: &crate::detect::Product) -> Result<()> {
    if product.root == windows::Win32::System::Registry::HKEY_CURRENT_USER {
        bail!("per-user uninstall commands cannot run while elevated");
    }
    let (command, in_place) = legacy_uninstall(product, is_nsis).context("no uninstall command")?;
    let (code, _) = system::run_cmd(&command, Duration::from_secs(300))?;
    if code != 0 {
        bail!("its uninstaller exited with {code}");
    }
    // Run in place, an NSIS uninstaller cannot delete itself.
    if let Some(program) = in_place {
        let _ = std::fs::remove_file(&program);
        if let Some(folder) = program.parent() {
            let _ = std::fs::remove_dir(folder);
        }
    }
    Ok(())
}
/// The command that removes a legacy host, and the uninstaller it runs in
/// place. An NSIS uninstaller (Apollo, Sunshine, Vibepollo 1) copies itself
/// to %TEMP%, starts the copy and exits at once: setup went on while the
/// copy still had to stop and delete ApolloService, the service Rubylight
/// reuses, and remove drivers. With "_?=<folder>", always last and
/// unquoted, it runs in place and setup waits for it.
pub(super) fn legacy_uninstall(
    product: &crate::detect::Product,
    nsis: impl Fn(&Path) -> bool,
) -> Option<(String, Option<PathBuf>)> {
    if product.root == windows::Win32::System::Registry::HKEY_CURRENT_USER {
        return None;
    }
    let command = product
        .quiet_uninstall
        .clone()
        .or_else(|| product.uninstall.clone().map(|u| format!("{u} /S")))?;
    let program = detect::program(&command).filter(|program| nsis(program));
    Some(match program {
        Some(program) if !command.contains("_?=") => {
            let folder = program.parent()?.display().to_string();
            (format!("{command} _?={folder}"), Some(program))
        }
        _ => (command, None),
    })
}
/// Whether a program is an NSIS installer or uninstaller: its data follows
/// the executable with this signature.
pub(super) fn is_nsis(program: &Path) -> bool {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(program)
        .and_then(|file| file.take(16 << 20).read_to_end(&mut bytes))
        .is_ok()
        && bytes.windows(12).any(|window| window == b"NullsoftInst")
}
pub(crate) fn register(install: &Path, entries: &[payload::Entry]) -> Result<()> {
    // The updater uses canonical paths for file identity and backups. Do not
    // leak their verbatim prefix into the next manual install or shell entry.
    let install = system::win32_path(install)?;
    let size_kb: u64 = entries
        .iter()
        .filter_map(|e| std::fs::metadata(install.join(&e.path)).ok())
        .map(|m| m.len())
        .sum::<u64>()
        / 1024;
    let location = format!("{}\\", install.display());
    let uninstaller = install.join("uninstall.exe").display().to_string();
    let uninstall = format!("\"{uninstaller}\" --uninstall");
    let quiet = format!("\"{uninstaller}\" --uninstall --quiet");
    let icon = install.join("butterpollo.exe").display().to_string();
    system::write_key(
        HKEY_LOCAL_MACHINE,
        &format!("{UNINSTALL}\\Butterpollo"),
        &[
            ("DisplayName", Value::Text("Rubylight")),
            ("DisplayVersion", Value::Text(env!("CARGO_PKG_VERSION"))),
            ("Publisher", Value::Text("Rubylight")),
            ("InstallLocation", Value::Text(&location)),
            ("DisplayIcon", Value::Text(&icon)),
            ("UninstallString", Value::Text(&uninstall)),
            ("QuietUninstallString", Value::Text(&quiet)),
            (
                "URLInfoAbout",
                Value::Text("https://github.com/RamazanKara/Rubylight"),
            ),
            ("NoModify", Value::Number(1)),
            ("NoRepair", Value::Number(1)),
            (
                "EstimatedSize",
                Value::Number(size_kb.min(u64::from(u32::MAX)) as u32),
            ),
        ],
    )
}
/// The service runs as SYSTEM from the install folder. Program Files lets
/// only administrators change it; a folder elsewhere (D:\Rubylight) would
/// let any user replace its programs, so it gets the same rights.
pub(crate) fn secure_install(install: &Path) -> Result<()> {
    system::restrict(install, true)?;
    system::inherit_contents(install, &[])
}
