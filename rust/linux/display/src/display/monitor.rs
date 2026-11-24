//! The descriptor of a connected display. Linux lists no displays yet, so none is ever made
//! here; the type and its name matching are what the host compiles against.
use serde::Serialize;

#[derive(Clone, Serialize)]
pub struct Monitor {
    pub device_id: String,
    pub monitor_device_path: String,
    pub display_name: String,
    pub friendly_name: String,
    pub hdr_supported: bool,
    pub hdr_enabled: bool,
    /// Whether the user's HDR switch for the display is on, which can stay on while
    /// `hdr_enabled` reads false.
    #[serde(skip)]
    pub hdr_requested: bool,
    pub primary: bool,
    /// The GPU the display is on, as the low and high halves of its adapter id.
    #[serde(skip)]
    pub adapter: (u32, i32),
    #[serde(skip)]
    pub target: u32,
    #[serde(skip)]
    pub source: u32,
}

impl Monitor {
    /// Whether `hint` names this display by its device id, device path or display name.
    pub fn matches(&self, hint: &str) -> bool {
        [
            &self.device_id,
            &self.monitor_device_path,
            &self.display_name,
        ]
        .iter()
        .any(|value| value.eq_ignore_ascii_case(hint))
    }
    /// Whether this is a Rubylight virtual display rather than a physical one.
    pub fn is_rubylight(&self) -> bool {
        is_rubylight_monitor(&self.monitor_device_path)
    }
    /// Whether this is known to be a physical display. A display whose path can't
    /// be read for the moment, as while it comes or goes, could be either.
    pub fn is_known_physical(&self) -> bool {
        is_physical_monitor(&self.monitor_device_path)
    }
}

/// Whether a monitor device path names a Rubylight virtual display (maker RBY).
fn is_rubylight_monitor(path: &str) -> bool {
    let parts: Vec<_> = path.split('#').collect();
    parts.len() >= 3
        && parts[1]
            .get(..3)
            .is_some_and(|maker| maker.eq_ignore_ascii_case("RBY"))
}

/// Whether a monitor device path can be read and names another maker's display.
fn is_physical_monitor(path: &str) -> bool {
    path.split('#').count() >= 3 && !is_rubylight_monitor(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(path: &str) -> Monitor {
        Monitor {
            device_id: "{id}".into(),
            monitor_device_path: path.into(),
            display_name: "DP-1".into(),
            friendly_name: "Panel".into(),
            hdr_supported: false,
            hdr_enabled: false,
            hdr_requested: false,
            primary: true,
            adapter: (0, 0),
            target: 0,
            source: 0,
        }
    }

    #[test]
    fn a_display_is_named_by_id_path_or_name_in_any_case() {
        let m = monitor(r"\\?\DISPLAY#RBY0001#5&1#{guid}");
        assert!(m.matches("dp-1") && m.matches("{ID}") && !m.matches("Panel"));
        assert!(m.is_rubylight() && !m.is_known_physical());
        let physical = monitor(r"\\?\DISPLAY#GSM5B09#5&1#{guid}");
        assert!(!physical.is_rubylight() && physical.is_known_physical());
        assert!(!monitor("").is_known_physical());
    }
}
