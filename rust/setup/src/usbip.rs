//! Persist the optional shared USB/IP driver choice across setup and updates.
use anyhow::{Context, Result};
use std::path::Path;

const CHOICE: &str = "usbip-driver.json";

pub fn selected(profile: &Path, explicit: Option<bool>) -> bool {
    explicit.unwrap_or_else(|| {
        std::fs::read(profile.join(CHOICE))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .is_some_and(|choice| choice["enabled"] == true)
    })
}

pub fn remember(profile: &Path, enabled: bool) -> Result<()> {
    remember_choice(
        profile,
        CHOICE,
        enabled,
        "saving the USB audio and haptics installer choice",
    )
}

pub(crate) fn remember_choice(
    profile: &Path,
    file: &str,
    enabled: bool,
    context: &str,
) -> Result<()> {
    use anyhow::bail;
    std::fs::create_dir_all(profile)?;
    let target = profile.join(file);
    let temp = profile.join(format!("{file}.tmp-{}", std::process::id()));
    let bytes = if enabled {
        br#"{"enabled":true}"#.as_slice()
    } else {
        br#"{"enabled":false}"#.as_slice()
    };
    if let Err(error) = std::fs::write(&temp, bytes) {
        let _ = std::fs::remove_file(&temp);
        return Err(error).context(context.to_owned());
    }
    if let Err(error) = std::fs::rename(&temp, &target) {
        let _ = std::fs::remove_file(&temp);
        bail!("{context}: {error}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usbip_stays_optional_and_updates_keep_the_explicit_choice() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let profile = temp.path();
        assert!(!selected(profile, None));
        assert!(selected(profile, Some(true)));
        remember(profile, true)?;
        assert!(selected(profile, None));
        assert!(!selected(profile, Some(false)));
        remember(profile, false)?;
        assert!(!selected(profile, None));
        std::fs::write(profile.join(CHOICE), b"interrupted write")?;
        assert!(!selected(profile, None));
        Ok(())
    }
}
