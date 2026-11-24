//! Saved display layouts and the leases a stream holds on its display, on Linux.
//!
//! A saved layout file is read and checked for real. Capturing the current layout, applying
//! or restoring one, and leasing a physical or virtual display are not available yet: those
//! return an error, and a lease that holds no display does nothing.
use super::{Topology, VirtualOptions};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// A saved display layout: the displays, their modes and positions, and their HDR, scale and
/// rotation settings.
#[derive(Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub version: u32,
    pub nodes: Vec<butterpollo_media_policy::topology::Node>,
    pub hdr: std::collections::BTreeMap<String, bool>,
    #[serde(default)]
    pub clone_groups: Vec<Vec<String>>,
    #[serde(default)]
    pub scale: std::collections::BTreeMap<String, u32>,
    #[serde(default)]
    pub rotation: std::collections::BTreeMap<String, u32>,
}

impl Snapshot {
    /// Read a saved layout from `path`, in this host's format or the older one. The ids in
    /// the file are kept as written: Linux lists no displays to match them against.
    pub fn read(path: &std::path::Path) -> Result<Self> {
        if std::fs::metadata(path)?.len() > 4 * 1024 * 1024 {
            bail!("display snapshot exceeds its limit");
        }
        let value = serde_json::from_slice(&std::fs::read(path)?)?;
        Self::decode(&value)
    }
    /// Check and decode a saved layout, in this host's format or the older one.
    pub fn decode(value: &serde_json::Value) -> Result<Self> {
        if value.get("version").is_some() {
            let snapshot: Self = serde_json::from_value(value.clone())?;
            if snapshot.version != 1
                || snapshot.nodes.is_empty()
                || snapshot.nodes.len() > 64
                || snapshot.nodes.iter().any(|node| {
                    node.mode.width == 0
                        || node.mode.height == 0
                        || !node.mode.refresh_hz.is_finite()
                        || !(1.0..=1000.0).contains(&node.mode.refresh_hz)
                })
            {
                bail!("invalid Rust display snapshot");
            }
            return Ok(snapshot);
        }
        use butterpollo_media_policy::topology::{Kind, Mode, Node, Position};
        let groups: Vec<Vec<String>> = serde_json::from_value(
            value
                .get("topology")
                .cloned()
                .context("saved display topology is missing")?,
        )?;
        let modes = value["modes"]
            .as_object()
            .context("saved display modes are missing")?;
        if groups.len() > 64 || modes.len() > 64 {
            bail!("saved display count exceeds its limit");
        }
        let active: std::collections::BTreeSet<_> = groups.iter().flatten().collect();
        let mut snapshot = Self {
            version: 1,
            nodes: vec![],
            hdr: Default::default(),
            clone_groups: groups.iter().filter(|g| g.len() > 1).cloned().collect(),
            scale: Default::default(),
            rotation: Default::default(),
        };
        for (id, mode) in modes {
            let number = |key| mode[key].as_u64().context("invalid saved display mode");
            let (width, height, num, den) = (
                u32::try_from(number("w")?)?,
                u32::try_from(number("h")?)?,
                number("num")?,
                number("den")?,
            );
            if width == 0 || height == 0 || width > 16384 || height > 16384 || den == 0 || num == 0
            {
                bail!("invalid saved display mode");
            }
            let x = i32::try_from(value["origins"][id]["x"].as_i64().unwrap_or(0))?;
            let y = i32::try_from(value["origins"][id]["y"].as_i64().unwrap_or(0))?;
            snapshot.nodes.push(Node {
                id: id.clone(),
                device_id: id.clone(),
                label: id.clone(),
                kind: Kind::Physical,
                active: active.contains(id),
                primary: value["primary"].as_str() == Some(id),
                desired_position: Position { x, y },
                mode: Mode {
                    width,
                    height,
                    refresh_hz: num as f64 / den as f64,
                },
            });
            if let Some(enabled) = match value["hdr"][id].as_str() {
                Some("on") => Some(true),
                Some("off") => Some(false),
                _ => value["hdr"][id].as_bool(),
            } {
                snapshot.hdr.insert(id.clone(), enabled);
            }
            if let Some(degrees) = value["layouts"][id]["rotation"].as_u64()
                && matches!(degrees, 0 | 90 | 180 | 270)
            {
                snapshot
                    .rotation
                    .insert(id.clone(), degrees as u32 / 90 + 1);
            }
        }
        if snapshot.nodes.is_empty() {
            bail!("saved display snapshot is empty");
        }
        Ok(snapshot)
    }
    /// The layout the displays have now. Not readable on Linux yet.
    pub fn capture() -> Result<Self> {
        Err(crate::unsupported("Reading the display layout"))
    }
    /// Put this layout back. Not available on Linux yet.
    pub fn restore(&self) -> Result<()> {
        self.restore_excluding(&[])
    }
    /// Whether every display this layout turns on is connected now. Linux lists no displays,
    /// so this is false.
    pub fn displays_connected(&self) -> bool {
        let Ok(available) = Topology::query_all().map(|t| t.monitors()) else {
            return false;
        };
        let mut active = self.nodes.iter().filter(|n| n.active).peekable();
        active.peek().is_some()
            && active.all(|n| available.iter().any(|m| m.device_id == n.device_id))
    }
    /// Put this layout back, leaving the displays in `excluded` as they are. Not available on
    /// Linux yet.
    pub fn restore_excluding(&self, _excluded: &[String]) -> Result<()> {
        if self.version != 1 {
            bail!("unsupported Rust display snapshot version");
        }
        Err(crate::unsupported("Restoring a display layout"))
    }
}

/// The layout the displays had before remote monitors moved them. Always empty on Linux.
pub fn baseline_nodes() -> Result<Vec<butterpollo_media_policy::topology::Node>> {
    Topology::query()?.nodes()
}

/// Place the displays as `nodes` say. Not available on Linux yet.
pub fn apply_layout(_nodes: &[butterpollo_media_policy::topology::Node]) -> Result<()> {
    Err(crate::unsupported("Laying out displays"))
}

/// Put back the positions [`apply_layout`] changed. It never changes any on Linux, so there
/// is nothing to put back.
pub fn restore_positions() -> Result<()> {
    Ok(())
}

/// The displays streams, paused game displays, launches being prepared and remote monitors
/// hold now, by device id. Nothing can hold a display on Linux yet, so none.
pub fn leased_displays() -> Vec<String> {
    Vec::new()
}

/// The rotation in degrees and the desktop size of the display a physical stream on `output`
/// gets. Not readable on Linux yet.
pub fn stream_display_rotation(_output: &str) -> Result<(u16, (u32, u32))> {
    Err(crate::unsupported("Reading a display's rotation"))
}

/// The driver's view of a virtual display while a stream waits for its first frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayStatus {
    pub swap_chain: bool,
    pub ring: bool,
    pub frames_acquired: u64,
    pub frames_pushed: u64,
}

/// [`DisplayStatus`] beside whether the display is still on an active path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayDiagnostics {
    pub status: DisplayStatus,
    /// None when that could not be read; false is a real "not on an active path".
    pub ccd_active: Option<bool>,
}

/// A stream's lease on its display: the display's settings for the stream, and the virtual
/// display itself when the stream made one. None can be taken on Linux yet.
#[non_exhaustive]
pub struct Guard {
    pub output: String,
}

impl Guard {
    /// Lease the display `output`, or a virtual display `stable_id` when `virtual_mode` is
    /// set, at the stream's mode. Fails on Linux: neither can be leased yet.
    #[allow(clippy::too_many_arguments)] // The host's display lease parameters.
    pub fn new_virtual_options(
        _output: &str,
        virtual_mode: bool,
        _stable_id: &str,
        _width: u32,
        _height: u32,
        _rate: butterpollo_media_policy::framegen::Rate,
        _hdr: Option<bool>,
        _physical_resolution: Option<(u32, u32)>,
        _physical_refresh: Option<butterpollo_media_policy::framegen::Rate>,
        _options: &VirtualOptions,
    ) -> Result<Self> {
        Err(crate::unsupported(if virtual_mode {
            "A virtual display"
        } else {
            "Preparing a display for a stream"
        }))
    }
    /// Turn the display to `degrees`. Not available on Linux yet.
    pub fn rotate(&mut self, _degrees: u16) -> Result<()> {
        Err(crate::unsupported("Rotating a display"))
    }
    /// Set the display's desktop scale. Not available on Linux yet.
    pub fn scale(&mut self, _configured: i64) -> Result<()> {
        Err(crate::unsupported("Setting a display's scale"))
    }
    /// Set the stream's mode on an owned virtual display again. This lease owns none.
    pub fn apply_virtual_mode(&self, _stage: &str) {}
    /// Whether this lease owns a virtual display. Never on Linux.
    pub fn is_virtual(&self) -> bool {
        false
    }
    /// Raise the owned virtual display's refresh to at least `rate`, returning the refresh
    /// (millihertz) it has now. None: this lease owns no virtual display.
    pub fn raise_virtual_rate(
        &self,
        _rate: butterpollo_media_policy::framegen::Rate,
    ) -> Option<u32> {
        None
    }
    /// Give the owned virtual display another size. Fails: this lease owns none.
    pub fn reshape_virtual(&self, _width: u32, _height: u32) -> Result<()> {
        Err(crate::unsupported("Reshaping a virtual display"))
    }
    /// The resolution this lease sets on its own virtual display. None: it owns none.
    pub fn virtual_size(&self) -> Option<(u32, u32)> {
        None
    }
    /// The resolution to set on the owned virtual display from now on. This lease owns none.
    pub fn adopt_virtual_size(&self, _width: u32, _height: u32) {}
    /// Keep the display alive, recovering it when it was lost. True when it was recovered;
    /// never here, where no display is held.
    pub fn feed(&mut self) -> Result<bool> {
        Ok(false)
    }
    /// The virtual display's state for the log. None: this lease owns no virtual display.
    pub fn diagnostics(&self, _stage: &str) -> Result<Option<DisplayDiagnostics>> {
        Ok(None)
    }
    /// The virtual display's state. None: this lease owns no virtual display.
    pub fn monitor_status(&self) -> Option<DisplayStatus> {
        None
    }
    /// Forward the display driver's queued log lines. There is no driver on Linux.
    pub fn drain_display_log(&self) {}
    /// Renew the lease while its stream is paused. Nothing is held, so nothing is renewed.
    pub fn keep_alive(&mut self) -> Result<()> {
        Ok(())
    }
    /// Bring the display back for a resumed stream. True when it was recovered; never here.
    pub fn resume(&mut self) -> Result<bool> {
        Ok(false)
    }
}

/// A remote monitor's display, which outlives a streaming session. None can be made on
/// Linux yet; [`Retained::inert`] gives one with no display behind it.
#[non_exhaustive]
pub struct Retained {
    pub output: String,
    pub mode: (u32, u32, u32, bool),
}

impl Retained {
    /// A stream on this display started. Nothing is held, so nothing changes.
    pub fn stream_started(&self) {}
    /// A stream on this display stopped. Nothing is held, so nothing changes.
    pub fn stream_stopped(&self) {}
    /// The display to capture and its generation, which counts recoveries: always 0 here.
    pub fn capture_target(&self) -> (String, u64) {
        (self.output.clone(), 0)
    }
    /// The display's current name.
    pub fn current_output(&self) -> String {
        self.output.clone()
    }
    /// A lease with no display behind it, for the host's tests: dropping it touches nothing.
    #[doc(hidden)]
    pub fn inert(output: &str) -> Self {
        Self {
            output: output.to_owned(),
            mode: (0, 0, 0, false),
        }
    }
    /// Make a virtual display `id` for a remote monitor and keep it alive; `on_recovery`
    /// runs after it was recovered. Fails on Linux: a virtual display cannot be made yet.
    #[allow(clippy::too_many_arguments)]
    pub fn create_options(
        _id: &str,
        _width: u32,
        _height: u32,
        _rate: butterpollo_media_policy::framegen::Rate,
        _hdr: bool,
        _options: &VirtualOptions,
        _on_recovery: Option<Box<dyn Fn() -> Result<()> + Send>>,
    ) -> Result<Self> {
        Err(crate::unsupported("A virtual display"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_saved_layout_in_the_older_format_is_decoded() {
        let snapshot = Snapshot::decode(&json!({
            "topology": [["A", "B"], ["C"]],
            "modes": {
                "A": {"w": 1920, "h": 1080, "num": 60000, "den": 1001},
                "B": {"w": 1920, "h": 1080, "num": 60, "den": 1},
                "C": {"w": 3840, "h": 2160, "num": 120, "den": 1},
                "D": {"w": 1280, "h": 720, "num": 60, "den": 1}
            },
            "origins": {"C": {"x": 1920, "y": 0}},
            "primary": "A",
            "hdr": {"C": "on", "A": false},
            "layouts": {"C": {"rotation": 90}}
        }))
        .unwrap();
        assert_eq!(snapshot.version, 1);
        assert_eq!(snapshot.nodes.len(), 4);
        assert_eq!(snapshot.clone_groups, [["A", "B"]]);
        let node = |id: &str| snapshot.nodes.iter().find(|n| n.id == id).unwrap();
        assert!(node("A").primary && node("A").active && !node("D").active);
        assert_eq!(node("C").desired_position.x, 1920);
        assert!((node("A").mode.refresh_hz - 59.94).abs() < 0.01);
        assert_eq!(snapshot.hdr.get("C"), Some(&true));
        assert_eq!(snapshot.hdr.get("A"), Some(&false));
        assert_eq!(snapshot.rotation.get("C"), Some(&2));
        assert!(!snapshot.displays_connected());
        assert!(snapshot.restore().is_err());
    }

    #[test]
    fn a_saved_layout_is_checked_and_survives_a_round_trip() {
        assert!(Snapshot::decode(&json!({"version": 1, "nodes": [], "hdr": {}})).is_err());
        assert!(Snapshot::decode(&json!({"modes": {}})).is_err());
        let saved = Snapshot::decode(&json!({
            "topology": [["A"]],
            "modes": {"A": {"w": 2560, "h": 1440, "num": 144, "den": 1}}
        }))
        .unwrap();
        let again = Snapshot::decode(&serde_json::to_value(&saved).unwrap()).unwrap();
        assert_eq!(again.nodes[0].mode.width, 2560);
        let zero =
            json!({"topology": [["A"]], "modes": {"A": {"w": 0, "h": 1, "num": 1, "den": 1}}});
        assert!(Snapshot::decode(&zero).is_err());
    }

    #[test]
    fn a_lease_without_a_display_holds_nothing() {
        let lease = Retained::inert("x");
        lease.stream_started();
        lease.stream_stopped();
        assert_eq!(lease.capture_target(), ("x".to_owned(), 0));
        assert_eq!(lease.current_output(), "x");
        assert!(leased_displays().is_empty() && restore_positions().is_ok());
        assert!(baseline_nodes().unwrap().is_empty() && apply_layout(&[]).is_err());
    }
}
