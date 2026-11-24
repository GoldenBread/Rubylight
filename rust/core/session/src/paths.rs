//! Where the host keeps its profile on Windows.
use anyhow::{Context, Result};
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

/// Rubylight's machine-wide folder, `%ProgramData%\Butterpollo`: the
/// installed service's profile and what every profile on this PC shares.
/// Falls back to `C:\ProgramData` when the variable is unset, as it is for
/// some service accounts.
pub fn data_root() -> PathBuf {
    PathBuf::from(std::env::var_os("PROGRAMDATA").unwrap_or_else(|| r"C:\ProgramData".into()))
        .join("Butterpollo")
}

/// The installed service's profile, `%ProgramData%\Butterpollo\config`.
pub fn installed_profile() -> PathBuf {
    data_root().join("config")
}

/// A portable host uses an existing `config` folder beside its executable,
/// or `%LocalAppData%\ButterpolloRust\config` otherwise.
pub fn portable_profile() -> Result<PathBuf> {
    #[cfg(target_os = "linux")]
    return linux_profile_at(
        &std::env::current_exe()?,
        std::env::var_os("XDG_CONFIG_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    );
    #[cfg(not(target_os = "linux"))]
    portable_profile_at(
        &std::env::current_exe()?,
        std::env::var_os("LOCALAPPDATA").as_deref(),
    )
}

/// On Linux the profile is an existing `config` folder beside the executable (a copy run from
/// its own folder), or `$XDG_CONFIG_HOME/rubylight`, else `~/.config/rubylight`: the host runs as
/// the signed-in user, and its log and state files live in the profile as on Windows.
#[cfg(any(target_os = "linux", test))]
fn linux_profile_at(
    executable: &Path,
    xdg_config_home: Option<&OsStr>,
    home: Option<&OsStr>,
) -> Result<PathBuf> {
    let beside = executable
        .parent()
        .context("package directory unavailable")?
        .join("config");
    if beside.is_dir() {
        return Ok(beside);
    }
    let base = match xdg_config_home.filter(|value| !value.is_empty()) {
        Some(base) => PathBuf::from(base),
        None => PathBuf::from(
            home.filter(|value| !value.is_empty())
                .context("HOME is unavailable")?,
        )
        .join(".config"),
    };
    Ok(base.join("rubylight"))
}

#[cfg_attr(target_os = "linux", allow(dead_code))]
fn portable_profile_at(executable: &Path, local_app_data: Option<&OsStr>) -> Result<PathBuf> {
    let directory = executable
        .parent()
        .context("package directory unavailable")?
        .join("config");
    if directory.is_dir() {
        return Ok(directory);
    }
    Ok(
        PathBuf::from(local_app_data.context("LOCALAPPDATA is unavailable")?)
            .join("ButterpolloRust/config"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copied_packages_use_separate_adjacent_profiles() {
        let root = tempfile::tempdir().unwrap();
        let local = root.path().join("systemprofile");
        for name in ["seat-one", "seat-two"] {
            let package = root.path().join(name);
            let profile = package.join("config");
            std::fs::create_dir_all(&profile).unwrap();
            for binary in ["butterpollo.exe", "Start Rubylight.exe"] {
                assert_eq!(
                    portable_profile_at(&package.join(binary), Some(local.as_os_str())).unwrap(),
                    profile
                );
                assert_eq!(
                    portable_profile_at(&package.join(binary), None).unwrap(),
                    profile
                );
            }
        }
    }

    #[test]
    fn linux_profiles_follow_xdg_config_home() {
        let root = tempfile::tempdir().unwrap();
        let exe = root.path().join("butterpollo");
        let xdg = root.path().join("xdg");
        assert_eq!(
            linux_profile_at(&exe, Some(xdg.as_os_str()), Some(OsStr::new("/home/u"))).unwrap(),
            xdg.join("rubylight")
        );
        assert_eq!(
            linux_profile_at(&exe, Some(OsStr::new("")), Some(OsStr::new("/home/u"))).unwrap(),
            Path::new("/home/u/.config/rubylight")
        );
        assert!(linux_profile_at(&exe, None, None).is_err());
        std::fs::create_dir(root.path().join("config")).unwrap();
        assert_eq!(
            linux_profile_at(&exe, Some(xdg.as_os_str()), None).unwrap(),
            root.path().join("config")
        );
    }

    #[test]
    fn without_an_adjacent_directory_keeps_the_per_user_profile() {
        let root = tempfile::tempdir().unwrap();
        let executable = root.path().join("butterpollo.exe");
        let local = root.path().join("local");
        let expected = local.join("ButterpolloRust/config");
        assert_eq!(
            portable_profile_at(&executable, Some(local.as_os_str())).unwrap(),
            expected
        );
        std::fs::write(root.path().join("config"), "not a directory").unwrap();
        assert_eq!(
            portable_profile_at(&executable, Some(local.as_os_str())).unwrap(),
            expected
        );
        assert!(portable_profile_at(&executable, None).is_err());
    }
}
