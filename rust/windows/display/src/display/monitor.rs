//! Connected display descriptor shared across topology queries, HDR state, and arrangement.
use super::topology::{is_physical_monitor, is_rubylight_monitor};
use super::*;

#[derive(Clone, Serialize)]
pub struct Monitor {
    pub device_id: String,
    pub monitor_device_path: String,
    pub display_name: String,
    pub friendly_name: String,
    pub hdr_supported: bool,
    pub hdr_enabled: bool,
    /// Whether Windows' HDR user switch (`hdrUserEnabled` on Windows 11, or
    /// `advancedColorEnabled` on legacy) is currently on. On Windows 11 an
    /// exclusive-fullscreen SDR app can drop `hdr_enabled` to `false` while
    /// leaving `hdr_requested` armed.
    #[serde(skip)]
    pub hdr_requested: bool,
    pub primary: bool,
    #[serde(skip)]
    pub adapter: LUID,
    #[serde(skip)]
    pub target: u32,
    #[serde(skip)]
    pub source: u32,
}

impl Monitor {
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
