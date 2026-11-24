//! Playnite script plugin installation, version checks, and user-token helper workers.

use crate::state::Shared;
use anyhow::{Context, Result, bail, ensure};
use butterpollo_session::{playnite, version};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};

pub(super) const PLUGIN_FILES: [&str; 2] = ["extension.yaml", "SunshinePlaynite.psm1"];

/// The plugin shipped with this installation.
pub fn packaged_plugin(h: &Shared) -> PathBuf {
    h.assets
        .parent()
        .and_then(Path::parent)
        .unwrap_or(&h.assets)
        .join("plugins")
        .join("playnite")
        .join("SunshinePlaynite")
}
pub(super) fn version(folder: &Path) -> Option<String> {
    playnite::plugin_version(&std::fs::read_to_string(folder.join("extension.yaml")).ok()?)
}
pub(super) fn installed_plugin() -> Option<PathBuf> {
    butterpollo_win_io::playnite::extensions_dir().map(|dir| dir.join("SunshinePlaynite"))
}
pub(super) fn needs_update(packaged: &str, target: &Path) -> bool {
    match version(target) {
        Some(installed) if version::newer(&installed, packaged) => false,
        Some(installed) => version::newer(packaged, &installed) || !plugin_present(target),
        None => true,
    }
}
pub(super) fn plugin_present(folder: &Path) -> bool {
    PLUGIN_FILES.iter().all(|file| folder.join(file).is_file()) && version(folder).is_some()
}
/// Copy the packaged plugin into Playnite's extensions. Playnite loads it
/// when it next starts.
pub fn install_plugin(h: &Shared) -> Result<PathBuf> {
    let source = packaged_plugin(h);
    let target = installed_plugin().context("Playnite was not found on this PC")?;
    copy_plugin_as_user(&source, &target)?;
    Ok(target)
}
fn copy_plugin_as_user(source: &Path, target: &Path) -> Result<()> {
    // Reading the packaged copy is safe here and keeps this error readable.
    if !plugin_present(source) {
        bail!(
            "the packaged Playnite plugin is missing or incomplete at {}",
            source.display()
        );
    }
    run_as_user(
        "--playnite-install",
        &[std::path::absolute(source)?, std::path::absolute(target)?],
    )
    .with_context(|| format!("installing the Playnite plugin into {}", target.display()))?;
    tracing::info!(folder = %target.display(), "installed the Playnite plugin");
    Ok(())
}
/// Both paths can come from user-writable locations. Resolve junctions and
/// perform every write with the user's token, never the service's.
fn run_as_user(mode: &str, paths: &[PathBuf]) -> Result<()> {
    use butterpollo_win_io::process::{Process, Target};
    let program = std::env::current_exe()?;
    let args: Vec<std::ffi::OsString> = std::iter::once(mode.into())
        .chain(paths.iter().map(|path| path.as_os_str().to_owned()))
        .collect();
    let worker = Process::spawn(
        &program,
        &args,
        program.parent(),
        Target::User { elevated: false },
        &BTreeMap::new(),
        true,
    )
    .context("starting the Playnite plugin helper as the signed-in user")?;
    match worker.wait(Duration::from_secs(30))? {
        0 => Ok(()),
        1 => bail!("the Playnite plugin helper failed"),
        code @ 2..0x8000_0000 => Err(std::io::Error::from_raw_os_error(code as i32))
            .context("the Playnite plugin helper failed"),
        code => bail!("the Playnite plugin helper ended with 0x{code:08x}"),
    }
}
/// The helper's exit code: a failed file operation's Windows error, which
/// the service reports, or 1 for any other failure.
pub fn helper_exit_code(result: &Result<()>) -> i32 {
    let Err(error) = result else { return 0 };
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<std::io::Error>()?.raw_os_error())
        .filter(|&code| code > 1)
        .unwrap_or(1)
}
pub fn install_worker(source: &Path, target: &Path) -> Result<()> {
    ensure!(
        !butterpollo_win_io::process::is_system(),
        "the Playnite plugin installer must run as the signed-in user"
    );
    if !plugin_present(source) {
        bail!(
            "the packaged Playnite plugin is missing or incomplete at {}",
            source.display()
        );
    }
    std::fs::create_dir_all(target)
        .with_context(|| format!("creating Playnite plugin folder {}", target.display()))?;
    // Publish the version last so an interrupted copy is retried.
    for file in PLUGIN_FILES.iter().rev() {
        butterpollo_session::state::atomic_write(
            &target.join(file),
            &std::fs::read(source.join(file)).with_context(|| {
                format!(
                    "reading Playnite plugin file {}",
                    source.join(file).display()
                )
            })?,
        )
        .with_context(|| {
            format!(
                "installing Playnite plugin file {}",
                target.join(file).display()
            )
        })?;
    }
    Ok(())
}
/// Install missing files or an upgrade before Playnite starts. An existing
/// process keeps its loaded plugin until the user restarts Playnite.
pub(super) fn update_plugin(h: &Shared) -> bool {
    let Some(target) = installed_plugin() else {
        tracing::warn!("Playnite plugin folder could not be resolved for the signed-in user");
        return false;
    };
    let source = packaged_plugin(h);
    let present = plugin_present(&target);
    let installed = version(&target);
    let packaged = version(&source);
    let running = butterpollo_win_io::playnite::running().is_some();
    tracing::debug!(folder = %target.display(), ?installed, ?packaged, present, running, "Playnite plugin check");
    let Some(packaged) = packaged else {
        tracing::warn!(folder = %source.display(), "packaged Playnite plugin is unavailable");
        return present;
    };
    if needs_update(&packaged, &target) {
        if let Err(error) = copy_plugin_as_user(&source, &target) {
            tracing::warn!(error = %format!("{error:#}"), "the Playnite plugin could not be installed or updated");
            return present;
        }
        if running {
            tracing::warn!("Playnite plugin files changed; restart Playnite to load them");
        }
        return !running || present;
    }
    if !present {
        tracing::warn!(
            "the installed Playnite plugin is not the supported script connector; using the CLI fallback"
        );
    }
    present
}

pub fn uninstall_plugin() -> Result<()> {
    if let Some(target) = installed_plugin()
        && target.is_dir()
    {
        run_as_user("--playnite-uninstall", &[std::path::absolute(&target)?])
            .with_context(|| format!("removing the Playnite plugin from {}", target.display()))?;
    }
    Ok(())
}
/// Remove the plugin folder with the signed-in user's token: its path comes
/// from the user's registry and profile, as for installs.
pub fn uninstall_worker(target: &Path) -> Result<()> {
    ensure!(
        !butterpollo_win_io::process::is_system(),
        "the Playnite plugin helper must run as the signed-in user"
    );
    match std::fs::remove_dir_all(target) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        result => {
            result.with_context(|| format!("removing Playnite plugin folder {}", target.display()))
        }
    }
}
