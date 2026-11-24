//! The pool of dormant connectors the vibeshine kernel module exposes.
//!
//! Ported from Vibepollo (Nonary/Vibepollo, GPL-3.0), src/platform/linux/private_display*.cpp:
//! the managed connectors are `Virtual-N` entries under /sys/class/drm whose device is the
//! module's faux device. Reading only; the broker is what changes them.
use std::path::Path;

/// Where the kernel lists DRM connectors.
pub const DRM_ROOT: &str = "/sys/class/drm";
const FAUX_DEVICE: &str = "/devices/faux/vibeshine";

/// One connector of the pool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Connector {
    /// The name KWin shows, `Virtual-N`.
    pub name: String,
    /// The number N.
    pub index: u32,
    /// The kernel's `status`: "connected", "disconnected" or "unknown".
    pub status: String,
}

/// The module's connectors under `root`, ordered by number. Empty when the module is not
/// loaded.
pub fn scan(root: &Path) -> Vec<Connector> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut found: Vec<Connector> = entries
        .flatten()
        .filter_map(|entry| {
            let file = entry.file_name();
            let file = file.to_str()?;
            let (card, name) = file.split_once('-')?;
            let digits = card.strip_prefix("card")?;
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let index = name.strip_prefix("Virtual-")?;
            if !super::broker::managed_name(name) {
                return None;
            }
            let path = entry.path();
            let device = std::fs::canonicalize(path.join("device")).ok()?;
            if !device.to_string_lossy().contains(FAUX_DEVICE) {
                return None;
            }
            let status = std::fs::read_to_string(path.join("status"))
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|_| "unknown".into());
            Some(Connector {
                name: name.to_string(),
                index: index.parse().ok()?,
                status,
            })
        })
        .collect();
    found.sort_by_key(|c| c.index);
    found.dedup_by(|a, b| a.name == b.name);
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::virtual_display::broker::fake::{connector, temp_dir};

    #[test]
    fn only_connectors_on_the_vibeshine_faux_device_count() {
        let base = temp_dir("pool");
        connector(
            &base,
            "card1-Virtual-2",
            "devices/faux/vibeshine",
            "connected",
        );
        connector(
            &base,
            "card1-Virtual-1",
            "devices/faux/vibeshine",
            "disconnected",
        );
        connector(
            &base,
            "card2-Virtual-1",
            "devices/platform/vkms",
            "disconnected",
        );
        connector(
            &base,
            "card0-DP-1",
            "devices/pci0000:00/0000:03:00.0",
            "connected",
        );
        connector(
            &base,
            "card1-Virtual-x",
            "devices/faux/vibeshine",
            "connected",
        );
        let found = scan(&base.join("drm"));
        let names: Vec<_> = found
            .iter()
            .map(|c| (c.name.as_str(), c.status == "connected"))
            .collect();
        assert_eq!(names, [("Virtual-1", false), ("Virtual-2", true)]);
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn a_missing_root_is_an_empty_pool() {
        assert!(scan(Path::new("/nonexistent/rubylight/drm")).is_empty());
    }
}
