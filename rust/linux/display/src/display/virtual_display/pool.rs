//! The pools of dormant connectors a private display is made on: Rubylight's own, a stock
//! VKMS device `rubylight` its root service makes (rust/linux/vdisplay-broker), and the one of
//! Vibepollo's vibeshine kernel module.
//!
//! Ported from Vibepollo (Nonary/Vibepollo, GPL-3.0), src/platform/linux/private_display*.cpp:
//! the managed connectors are `Virtual-N` entries under /sys/class/drm whose device is the
//! pool's faux device. Reading only; the broker is what changes them.
use std::path::Path;

/// Where the kernel lists DRM connectors.
pub const DRM_ROOT: &str = "/sys/class/drm";

/// Whose pool a connector is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Rubylight's: stock VKMS, plugged through Rubylight's broker; the mode is added through
    /// the compositor.
    Rubylight,
    /// Vibepollo's vibeshine module and broker, which also sets the mode.
    Vibepollo,
}

impl Kind {
    /// Preferred first.
    pub const ALL: [Kind; 2] = [Kind::Rubylight, Kind::Vibepollo];

    /// The faux device's name under /sys/devices/faux.
    pub fn device(self) -> &'static str {
        match self {
            Kind::Rubylight => "rubylight",
            Kind::Vibepollo => "vibeshine",
        }
    }

    /// Whether its broker takes the `mode` request; otherwise the compositor adds the mode.
    pub fn broker_sets_modes(self) -> bool {
        self == Kind::Vibepollo
    }

    fn of(device: &Path) -> Option<Kind> {
        let parts: Vec<_> = device.components().map(|c| c.as_os_str()).collect();
        parts
            .windows(3)
            .find(|w| w[0] == "devices" && w[1] == "faux")
            .and_then(|w| Kind::ALL.into_iter().find(|k| w[2] == k.device()))
    }
}

/// One connector of the pool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Connector {
    /// The name KWin shows, `Virtual-N`.
    pub name: String,
    /// The number N.
    pub index: u32,
    /// The kernel's `status`: "connected", "disconnected" or "unknown".
    pub status: String,
    pub kind: Kind,
}

/// The pools' connectors under `root`, Rubylight's first, each pool ordered by number. Empty
/// when no pool is there.
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
            let kind = Kind::of(&device)?;
            let status = std::fs::read_to_string(path.join("status"))
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|_| "unknown".into());
            Some(Connector {
                name: name.to_string(),
                index: index.parse().ok()?,
                status,
                kind,
            })
        })
        .collect();
    found.sort_by_key(|c| (c.kind != Kind::Rubylight, c.index));
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
        assert!(found.iter().all(|c| c.kind == Kind::Vibepollo));
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// Rubylight's stock VKMS pool comes first; a device whose name only starts like it (a
    /// leftover probe) is not a pool.
    #[test]
    fn rubylight_s_pool_is_preferred_and_named_exactly() {
        let base = temp_dir("pools");
        connector(
            &base,
            "card0-Virtual-1",
            "devices/faux/vibeshine",
            "disconnected",
        );
        connector(
            &base,
            "card2-Virtual-9",
            "devices/faux/rubylight-probe",
            "disconnected",
        );
        connector(
            &base,
            "card3-Virtual-11",
            "devices/faux/rubylight",
            "disconnected",
        );
        connector(
            &base,
            "card3-Virtual-10",
            "devices/faux/rubylight",
            "connected",
        );
        let found: Vec<_> = scan(&base.join("drm"))
            .into_iter()
            .map(|c| (c.name, c.kind))
            .collect();
        assert_eq!(
            found,
            [
                ("Virtual-10".to_string(), Kind::Rubylight),
                ("Virtual-11".to_string(), Kind::Rubylight),
                ("Virtual-1".to_string(), Kind::Vibepollo),
            ]
        );
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn a_missing_root_is_an_empty_pool() {
        assert!(scan(Path::new("/nonexistent/rubylight/drm")).is_empty());
    }
}
