//! The display topology on Linux: which displays are connected and shown.
//!
//! Display enumeration is not available yet, so every listing is empty and nothing matches a
//! display's name; changing the topology returns an error.
use anyhow::{Result, anyhow};

pub use super::monitor::Monitor;

/// One reading of the connected displays. On Linux it holds none.
pub struct Topology(());

impl Topology {
    /// The displays shown now: none are listed on Linux.
    pub fn query() -> Result<Self> {
        Ok(Self(()))
    }
    /// The displays connected, shown or not: none are listed on Linux.
    pub fn query_all() -> Result<Self> {
        Ok(Self(()))
    }
    /// The displays of this reading. Always empty on Linux.
    pub fn monitors(&self) -> Vec<Monitor> {
        Vec::new()
    }
    /// The displays of this reading as layout nodes. Always empty on Linux.
    pub fn nodes(&self) -> Result<Vec<butterpollo_media_policy::topology::Node>> {
        Ok(Vec::new())
    }
    /// The refresh rate of the display `id`. No display is listed on Linux, so this fails.
    pub fn refresh(&self, _id: &str) -> Result<butterpollo_media_policy::framegen::Rate> {
        Err(crate::unsupported("Reading a display's refresh rate"))
    }
    /// Move displays to these positions. Not available on Linux.
    pub fn set_positions(
        &mut self,
        _positions: &std::collections::BTreeMap<
            String,
            butterpollo_media_policy::topology::Position,
        >,
    ) -> Result<()> {
        Err(crate::unsupported("Placing displays"))
    }
    /// Show exactly these displays. Not available on Linux.
    pub fn set_active(_ids: &[String]) -> Result<()> {
        Err(crate::unsupported("Switching displays on or off"))
    }
}

/// The displays shown now. Always empty on Linux.
pub fn monitors() -> Result<Vec<Monitor>> {
    Ok(Topology::query()?.monitors())
}

/// How many displays are shown now. Linux lists none, so this is 0.
pub fn active_displays() -> Result<usize> {
    Ok(0)
}

/// Whether `hint` names the primary display. `None` when no shown display matches `hint`,
/// which is always the case on Linux.
pub fn shown_primary(_hint: &str) -> Result<Option<bool>> {
    Ok(None)
}

/// The device ids of the displays shown now. Always empty on Linux.
pub fn active_device_ids() -> Result<Vec<String>> {
    Ok(Vec::new())
}

/// What the EDID of the display `hint` says about the refresh rates in `targets`. No display
/// is listed on Linux, so none is found.
pub fn edid_refresh(_hint: &str, _targets: &[u32]) -> Result<serde_json::Value> {
    Err(anyhow!(
        "display device not found for EDID refresh validation: {}",
        crate::unsupported("Listing displays")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_display_is_listed() {
        assert!(monitors().unwrap().is_empty());
        assert!(Topology::query_all().unwrap().nodes().unwrap().is_empty());
        assert_eq!(active_displays().unwrap(), 0);
        assert_eq!(shown_primary("DP-1").unwrap(), None);
        assert!(active_device_ids().unwrap().is_empty());
        assert!(edid_refresh("DP-1", &[120]).is_err());
        assert!(Topology::query().unwrap().refresh("DP-1").is_err());
    }
}
