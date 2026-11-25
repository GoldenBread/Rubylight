//! Display topology snapshot capture, serialization, and restoration.

use super::super::*;

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
    #[serde(default)]
    pub sources: std::collections::BTreeMap<String, u32>,
}
impl Snapshot {
    pub fn read(path: &std::path::Path) -> Result<Self> {
        if std::fs::metadata(path)?.len() > 4 * 1024 * 1024 {
            bail!("display snapshot exceeds its limit");
        }
        let value = serde_json::from_slice(&std::fs::read(path)?)?;
        let mut snapshot = Self::decode(&value)?;
        let available = Topology::query_all()?.monitors();
        let normalized = |id: &str| {
            available
                .iter()
                .find(|m| m.matches(id))
                .map_or_else(|| id.to_owned(), |m| m.device_id.clone())
        };
        for node in &mut snapshot.nodes {
            node.id = normalized(&node.id);
            node.device_id = normalized(&node.device_id);
        }
        snapshot.hdr = snapshot
            .hdr
            .into_iter()
            .map(|(id, v)| (normalized(&id), v))
            .collect();
        snapshot.scale = snapshot
            .scale
            .into_iter()
            .map(|(id, v)| (normalized(&id), v))
            .collect();
        snapshot.rotation = snapshot
            .rotation
            .into_iter()
            .map(|(id, v)| (normalized(&id), v))
            .collect();
        snapshot.sources = snapshot
            .sources
            .into_iter()
            .map(|(id, v)| (normalized(&id), v))
            .collect();
        snapshot.clone_groups = snapshot
            .clone_groups
            .iter()
            .map(|group| group.iter().map(|id| normalized(id)).collect())
            .collect();
        Ok(snapshot)
    }
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
            sources: Default::default(),
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
    pub fn capture() -> Result<Self> {
        let topology = Topology::query()?;
        // One listing for every part: each costs a DXGI enumeration and the colour queries.
        let monitors = topology.monitors();
        Ok(Self {
            version: 1,
            nodes: topology.nodes_of(monitors.clone())?,
            hdr: monitors
                .iter()
                .map(|m| (m.device_id.clone(), m.hdr_enabled))
                .collect(),
            rotation: topology.rotations_of(&monitors),
            sources: monitors
                .iter()
                .map(|m| (m.device_id.clone(), m.source))
                .collect(),
            clone_groups: Topology::clone_groups_of(&monitors),
            scale: monitors
                .iter()
                .filter_map(|m| {
                    dpi_scale(m)
                        .ok()
                        .map(|percent| (m.device_id.clone(), percent))
                })
                .collect(),
        })
    }
    pub fn restore(&self) -> Result<()> {
        self.restore_excluding(&[])
    }
    /// Whether every display this layout turns on is connected now.
    pub fn displays_connected(&self) -> bool {
        let Ok(available) = Topology::query_all().map(|t| t.available_monitors()) else {
            return false;
        };
        self.displays_present(&available)
    }
    pub(super) fn displays_present(&self, available: &[Monitor]) -> bool {
        let mut active = self.nodes.iter().filter(|n| n.active).peekable();
        active.peek().is_some()
            && active.all(|n| available.iter().any(|m| m.device_id == n.device_id))
    }
    /// The displays a restore turns on: the saved active ones still
    /// connected, and the excluded ones active now.
    pub(super) fn active_ids(
        &self,
        current: &[Monitor],
        available: &[Monitor],
        excluded: &[String],
    ) -> Vec<String> {
        let preserve = |id: &str| excluded.iter().any(|e| id.eq_ignore_ascii_case(e));
        let mut ids = self
            .nodes
            .iter()
            .filter(|n| {
                n.active
                    && !preserve(&n.device_id)
                    && available.iter().any(|m| m.device_id == n.device_id)
            })
            .map(|n| n.device_id.clone())
            .collect::<Vec<_>>();
        ids.extend(
            current
                .iter()
                .filter(|m| preserve(&m.device_id))
                .map(|m| m.device_id.clone()),
        );
        ids
    }
    pub fn restore_excluding(&self, excluded: &[String]) -> Result<()> {
        if self.version != 1 {
            bail!("unsupported Rust display snapshot version");
        }
        let current = Topology::query()?;
        let available = Topology::query_all()?.available_monitors();
        let preserve = |id: &str| excluded.iter().any(|e| id.eq_ignore_ascii_case(e));
        let ids = self.active_ids(&current.monitors(), &available, excluded);
        if ids.is_empty() {
            // A headless host whose only display was the virtual one, now
            // gone: there is nothing left to turn on, so retrying cannot help.
            tracing::warn!("no display of the saved layout is connected; nothing to restore");
            return Ok(());
        }
        let primary = self
            .nodes
            .iter()
            .find(|n| {
                n.active && n.primary && !preserve(&n.device_id) && ids.contains(&n.device_id)
            })
            .map(|n| n.device_id.as_str());
        Topology::set_active_preferred(&ids, &self.sources, primary)
            .context("cannot activate the saved displays")?;
        // A display that cannot take back its old setting must not leave the
        // rest of the layout unrestored; report every such failure at the end.
        let mut failures = Vec::new();
        if let Err(error) = Topology::query().and_then(|mut t| {
            t.set_rotations(
                &self
                    .rotation
                    .iter()
                    .filter(|(id, _)| !preserve(id))
                    .map(|(id, r)| (id.clone(), *r))
                    .collect(),
            )
        }) {
            failures.push(format!("rotation: {error:#}"));
        }
        let monitors = monitors()?;
        let mut mode_failures = std::collections::BTreeSet::new();
        for n in &self.nodes {
            if preserve(&n.device_id) {
                continue;
            }
            let Some(m) = monitors.iter().find(|m| m.device_id == n.device_id) else {
                continue;
            };
            // Issue #129: apps open before an exclusive stream came back oversized.
            // Read the scale Windows gave the display as it came back on, and again
            // after its mode, so a log shows whether the scale moved on the way back.
            let saved_scale = self.scale.get(&n.device_id).copied();
            let switched_on = saved_scale.map(|_| dpi_scale(m).ok());
            let mut step = |what: &str, result: Result<()>| -> bool {
                if let Err(error) = result {
                    failures.push(format!(
                        "{} ({}) {what}: {error:#}",
                        n.label, m.display_name
                    ));
                    true
                } else {
                    false
                }
            };
            if step(
                &format!(
                    "mode {}x{}@{:.3}",
                    n.mode.width, n.mode.height, n.mode.refresh_hz
                ),
                Topology::set_mode_rate(
                    &m.display_name,
                    n.mode.width,
                    n.mode.height,
                    butterpollo_media_policy::framegen::Rate(
                        (n.mode.refresh_hz * 1000.0).round() as u32
                    ),
                ),
            ) {
                mode_failures.insert(n.device_id.clone());
            }
            if let Some(enabled) = self.hdr.get(&n.device_id) {
                step(&format!("HDR {enabled}"), set_hdr(m, *enabled));
            }
            if let Some(percent) = saved_scale {
                let after_mode = dpi_scale(m).ok();
                if scale_moved(percent, switched_on.flatten(), after_mode) {
                    tracing::info!(
                        display = %n.label,
                        output = %m.display_name,
                        saved = percent,
                        switched_on = ?switched_on.flatten(),
                        after_mode = ?after_mode,
                        "a display came back at another Windows scale; putting the saved one back"
                    );
                }
                step(&format!("scale {percent}%"), set_dpi_scale(m, percent));
            }
        }
        // set_active gave every display its own desktop source. Join clone
        // groups first: placing their members separately would put two
        // desktops at the same origin.
        let mut topology = Topology::query()?;
        topology
            .restore_clone_groups(
                &self
                    .clone_groups
                    .iter()
                    .filter(|group| group.iter().all(|id| !preserve(id)))
                    .cloned()
                    .collect::<Vec<_>>(),
            )
            .context("cannot restore the cloned displays")?;
        // Shift surviving node coordinates to `(0, 0)` when a saved origin monitor
        // is disconnected so Windows accepts the layout (`Item 35`).
        let mut present: Vec<String> = ids.clone();
        for m in current.monitors() {
            if preserve(&m.device_id) && !present.contains(&m.device_id) {
                present.push(m.device_id);
            }
        }
        let adjusted = crate::display_arrangement::present_layout(self, &present);
        topology = Topology::query()?;
        topology
            .set_positions(
                &adjusted
                    .nodes
                    .iter()
                    .filter(|n| !preserve(&n.device_id))
                    .map(|n| (n.device_id.clone(), n.desired_position))
                    .collect(),
            )
            .context("cannot restore the display positions")?;
        // Moving displays can renegotiate a display's timing (a TV at 120 Hz
        // came back at 60 Hz), so check every rate again. A clone member's
        // GDI name changed when it joined its group; find it by device.
        for n in self.nodes.iter().filter(|n| !preserve(&n.device_id)) {
            let Some(m) = monitors.iter().find(|m| m.device_id == n.device_id) else {
                continue;
            };
            if mode_failures.contains(&n.device_id) {
                continue;
            }
            if let Err(error) = Topology::set_mode_rate(
                &n.device_id,
                n.mode.width,
                n.mode.height,
                butterpollo_media_policy::framegen::Rate(
                    (n.mode.refresh_hz * 1000.0).round() as u32
                ),
            ) {
                failures.push(format!(
                    "{} ({}) mode after layout: {error:#}",
                    n.label, m.display_name
                ));
            }
        }
        if !failures.is_empty() {
            bail!("{RESTORED_EXCEPT} {}", failures.join("; "));
        }
        Ok(())
    }
}

/// Whether a restored display showed a scale other than the `saved` one on its way
/// back: as it came back on, or after its mode. A scale that could not be read is
/// no evidence either way.
pub(super) fn scale_moved(saved: u32, switched_on: Option<u32>, after_mode: Option<u32>) -> bool {
    [switched_on, after_mode]
        .into_iter()
        .flatten()
        .any(|seen| seen != saved)
}

const RESTORED_EXCEPT: &str = "display layout restored except";

/// The layout went back and a display refused part of it, its saved mode or HDR say. Asked
/// again it answers the same. Any other failure, a layout change pending or a read at the
/// wrong moment, can pass on the next try.
pub(crate) fn restore_was_refused(error: &anyhow::Error) -> bool {
    format!("{error:#}").contains(RESTORED_EXCEPT)
}
