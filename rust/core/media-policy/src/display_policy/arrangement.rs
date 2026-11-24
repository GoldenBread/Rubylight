//! Display desktop arrangements, host display choices, and layout recall detection.

use crate::topology::{Node, Position};
use anyhow::{Result, bail};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arrangement {
    Extended,
    Primary,
    Exclusive,
    Isolated,
    PrimaryIsolated,
}
impl Arrangement {
    /// The `virtual_display_layout` value that selects this arrangement.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Extended => "extended",
            Self::Primary => "extended_primary",
            Self::Exclusive => "exclusive",
            Self::Isolated => "extended_isolated",
            Self::PrimaryIsolated => "extended_primary_isolated",
        }
    }
    pub fn parse(value: &str) -> Result<Self> {
        Ok(
            match value.trim().to_ascii_lowercase().replace('-', "_").as_str() {
                "extended" | "ensure_active" => Self::Extended,
                "extended_primary" | "ensure_primary" => Self::Primary,
                "exclusive" | "ensure_only_display" => Self::Exclusive,
                "extended_isolated" => Self::Isolated,
                "extended_primary_isolated" => Self::PrimaryIsolated,
                _ => bail!("unknown display arrangement"),
            },
        )
    }
    /// The layout a remote monitor's stream applies. Its position on the
    /// desktop comes from the remote monitor layout, so it takes only which
    /// displays stay on and which is primary: exclusive turns the others off,
    /// the primary layouts make it primary, and the extended and isolated
    /// placements leave it where the remote monitor layout put it.
    pub const fn for_remote_monitor(self) -> Option<Self> {
        match self {
            Self::Exclusive => Some(Self::Exclusive),
            Self::Primary | Self::PrimaryIsolated => Some(Self::Primary),
            Self::Extended | Self::Isolated => None,
        }
    }
    /// Apply the first stream's virtual-display layout after hotplug. Windows
    /// can recall this virtual display as primary even though the user's
    /// physical displays stay on. Extended layouts keep the primary from
    /// before hotplug; deliberate primary/exclusive layouts still replace it.
    pub fn compose_after_hotplug(
        self,
        nodes: &[Node],
        target: &str,
        retained: &[String],
        before: &[Node],
    ) -> Result<Vec<Node>> {
        let mut current = nodes.to_vec();
        self.preserve_primary(&mut current, before)?;
        self.compose(&current, target, retained)
    }
    /// Keep the first owner's primary when another stream joins or an owned
    /// virtual display is recreated. Owner order still decides which stream
    /// is primary for the deliberate primary/exclusive layouts.
    pub fn compose_all_after_hotplug(
        self,
        nodes: &[Node],
        targets: &[String],
        retained: &[String],
        before: &[Node],
    ) -> Result<Vec<Node>> {
        let mut current = nodes.to_vec();
        self.preserve_primary(&mut current, before)?;
        self.compose_all(&current, targets, retained)
    }
    /// Whether the layout makes a stream's display the primary one.
    pub const fn makes_stream_main(self) -> bool {
        matches!(
            self,
            Self::Primary | Self::Exclusive | Self::PrimaryIsolated
        )
    }
    /// Whether the layout makes a stream's display the primary one while the user's own
    /// displays stay on. A primary Windows moves back is then the stream's to put back;
    /// exclusive keeps the primary itself by switching the other displays off.
    pub const fn primary_beside_the_users(self) -> bool {
        matches!(self, Self::Primary | Self::PrimaryIsolated)
    }
    /// Whether the primary stays the one from before the stream.
    pub const fn keeps_primary(self) -> bool {
        matches!(self, Self::Extended | Self::Isolated)
    }
    fn preserve_primary(self, nodes: &mut [Node], before: &[Node]) -> Result<()> {
        if !self.keeps_primary() {
            return Ok(());
        }
        let primary = before
            .iter()
            .find(|node| node.active && node.primary)
            .and_then(|original| {
                nodes
                    .iter()
                    .find(|node| node.active && node.device_id == original.device_id)
            })
            .map(|node| (node.device_id.clone(), node.desired_position));
        let Some((primary_id, origin)) = primary else {
            // Headless, disconnected or unreadable original layout: don't
            // invent a primary or activate a display the user switched off.
            return Ok(());
        };
        for node in nodes {
            node.primary = node.device_id == primary_id;
            node.desired_position.x = node
                .desired_position
                .x
                .checked_sub(origin.x)
                .ok_or_else(|| anyhow::anyhow!("display position overflow"))?;
            node.desired_position.y = node
                .desired_position
                .y
                .checked_sub(origin.y)
                .ok_or_else(|| anyhow::anyhow!("display position overflow"))?;
        }
        Ok(())
    }
    pub fn compose(self, nodes: &[Node], target: &str, retained: &[String]) -> Result<Vec<Node>> {
        let target_node = nodes
            .iter()
            .find(|n| n.device_id == target)
            .ok_or_else(|| anyhow::anyhow!("display arrangement target is missing"))?;
        let mut result = nodes.to_vec();
        if self == Self::Exclusive {
            for node in &mut result {
                node.active = node.device_id == target || retained.contains(&node.device_id);
            }
        }
        let primary = matches!(
            self,
            Self::Primary | Self::Exclusive | Self::PrimaryIsolated
        );
        if primary {
            for node in &mut result {
                node.primary = node.device_id == target;
                node.desired_position.x = node
                    .desired_position
                    .x
                    .checked_sub(target_node.desired_position.x)
                    .ok_or_else(|| anyhow::anyhow!("display position overflow"))?;
                node.desired_position.y = node
                    .desired_position
                    .y
                    .checked_sub(target_node.desired_position.y)
                    .ok_or_else(|| anyhow::anyhow!("display position overflow"))?;
            }
        }
        if self == Self::Isolated {
            result
                .iter_mut()
                .find(|n| n.device_id == target)
                .unwrap()
                .desired_position = Position { x: 64000, y: 64000 };
        }
        if self == Self::PrimaryIsolated {
            let min_x = result
                .iter()
                .filter(|n| n.device_id != target)
                .map(|n| n.desired_position.x)
                .min()
                .unwrap_or(0);
            let min_y = result
                .iter()
                .filter(|n| n.device_id != target)
                .map(|n| n.desired_position.y)
                .min()
                .unwrap_or(0);
            for node in result.iter_mut().filter(|n| n.device_id != target) {
                node.desired_position.x = 64000i32
                    .checked_add(
                        node.desired_position
                            .x
                            .checked_sub(min_x)
                            .ok_or_else(|| anyhow::anyhow!("display position overflow"))?,
                    )
                    .ok_or_else(|| anyhow::anyhow!("display position overflow"))?;
                node.desired_position.y = 64000i32
                    .checked_add(
                        node.desired_position
                            .y
                            .checked_sub(min_y)
                            .ok_or_else(|| anyhow::anyhow!("display position overflow"))?,
                    )
                    .ok_or_else(|| anyhow::anyhow!("display position overflow"))?;
            }
        }
        Ok(result)
    }
    /// One layout for several streams. The first target is laid out as for a
    /// single stream; each later target stays active and is placed to the right
    /// of the streamed displays, so no two of them overlap. An isolated layout
    /// keeps the streamed displays next to each other, away from the others.
    pub fn compose_all(
        self,
        nodes: &[Node],
        targets: &[String],
        retained: &[String],
    ) -> Result<Vec<Node>> {
        let (first, rest) = targets
            .split_first()
            .ok_or_else(|| anyhow::anyhow!("display arrangement has no target"))?;
        let mut keep = retained.to_vec();
        keep.extend(rest.iter().cloned());
        let mut result = self.compose(nodes, first, &keep)?;
        let isolated = matches!(self, Self::Isolated | Self::PrimaryIsolated);
        let mut placed = vec![first.clone()];
        for target in rest {
            if !result.iter().any(|n| n.device_id == *target) {
                bail!("display arrangement target is missing");
            }
            let y = result
                .iter()
                .find(|n| n.device_id == *first)
                .map_or(0, |n| n.desired_position.y);
            let right = result
                .iter()
                .filter(|n| {
                    n.active
                        && if isolated {
                            placed.contains(&n.device_id)
                        } else {
                            !rest.contains(&n.device_id) || placed.contains(&n.device_id)
                        }
                })
                .map(|n| {
                    n.desired_position
                        .x
                        .saturating_add(i32::try_from(n.mode.width).unwrap_or(i32::MAX))
                })
                .max()
                .unwrap_or(0);
            let node = result
                .iter_mut()
                .find(|n| n.device_id == *target)
                .expect("checked above");
            node.active = true;
            node.primary = false;
            node.desired_position = Position { x: right, y };
            placed.push(target.clone());
        }
        Ok(result)
    }
}
/// What a client asked the PC's displays to do for one stream: the
/// `hostDisplay` launch and resume parameter. It overrides the host, app and
/// per-device display settings for that stream only; without it (or with a
/// value this host does not know, such as `default`) those settings decide,
/// so older clients are unaffected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostDisplay {
    /// Stream a physical display; no virtual display is added.
    Physical,
    /// Add a virtual display for the stream and lay the desktop out like this
    /// (`exclusive` turns the other displays off for the stream).
    Virtual(Arrangement),
}
impl HostDisplay {
    pub const PARAMETER: &'static str = "hostDisplay";
    /// `physical`, or any `virtual_display_layout` value; anything else is
    /// None, leaving the host's settings in control.
    pub fn parse(value: &str) -> Option<Self> {
        if value.trim().eq_ignore_ascii_case("physical") {
            return Some(Self::Physical);
        }
        let value = value.trim().to_ascii_lowercase().replace('-', "_");
        Arrangement::parse(&value)
            .ok()
            .filter(|arrangement| value == arrangement.name())
            .map(Self::Virtual)
    }
    pub const fn name(self) -> &'static str {
        match self {
            Self::Physical => "physical",
            Self::Virtual(arrangement) => arrangement.name(),
        }
    }
    /// Whether a display prepared for one launch can serve the next: a
    /// launch that asks for nothing takes it as it is, and one that asks
    /// gets a display set up for exactly that request.
    pub fn reusable(prepared_for: Option<Self>, requested: Option<Self>) -> bool {
        requested.is_none() || prepared_for == requested
    }
}
/// Displays a stream's layout switched off that are on again. Windows puts
/// back the layout it has saved for the connected displays when an
/// exclusive-fullscreen game loses focus (the Win key, Alt+Tab, Ctrl+Alt+Del),
/// and that layout has the physical monitor on. `before`: the user's layout
/// from before the stream; `applied`: the stream's; `active`: the displays on
/// now. A display that was already off before the stream is the user's to
/// switch on, and one the stream never laid out (another client's display on
/// its way in) is not counted.
pub fn switched_back_on(before: &[Node], applied: &[Node], active: &[String]) -> Vec<String> {
    applied
        .iter()
        .filter(|n| !n.active && active.contains(&n.device_id))
        .filter(|n| {
            before
                .iter()
                .any(|b| b.device_id == n.device_id && b.active)
        })
        .map(|n| n.device_id.clone())
        .collect()
}

/// Whether Windows recalled a saved layout over the displays a stream leaves on,
/// as it can when a virtual display arrives or leaves: the database entry for those
/// displays can have one of them off, at another mode, or (`keeps_primary`) no
/// longer primary. `before`: the layout as it should be; `active`: the displays on
/// now; `streams`: the streams' own displays, which are laid out anyway and whose
/// mode the stream sets. A display no longer connected is not counted. With no
/// display reading as primary the primary is unknown (the read failed, as it can
/// during a hotplug), which is no sign of a recall.
pub fn recalled_over(
    before: &[Node],
    active: &[Node],
    connected: &[String],
    streams: &[String],
    keeps_primary: bool,
) -> bool {
    let keeps_primary = keeps_primary && active.iter().any(|n| n.active && n.primary);
    before
        .iter()
        .filter(|b| b.active && connected.contains(&b.device_id) && !streams.contains(&b.device_id))
        .any(|b| {
            let Some(now) = active
                .iter()
                .find(|n| n.active && n.device_id == b.device_id)
            else {
                return true;
            };
            (now.mode.width, now.mode.height) != (b.mode.width, b.mode.height)
                || (now.mode.refresh_hz - b.mode.refresh_hz).abs() >= 0.5
                || (keeps_primary && b.primary && !now.primary)
        })
}

/// The layout to save for a virtual display's entry in the Windows display
/// database: the user's displays as they have them, the primary and each mode
/// included, with `target` on to their right. Saving what Windows lays out when it
/// switches them on instead would make the virtual display primary and give each
/// display Windows' choice of timing, and every later stream would be handed that.
pub fn beside_the_users(before: &[Node], target: &Node) -> Vec<Node> {
    let mut nodes: Vec<Node> = before
        .iter()
        .filter(|n| n.device_id != target.device_id)
        .cloned()
        .collect();
    let edge = nodes
        .iter()
        .filter(|n| n.active)
        .map(|n| {
            (
                n.desired_position
                    .x
                    .saturating_add(i32::try_from(n.mode.width).unwrap_or(i32::MAX)),
                n.desired_position.y,
            )
        })
        .max_by_key(|(right, _)| *right);
    nodes.push(Node {
        active: true,
        // A headless PC has only this display, so it is the primary.
        primary: edge.is_none(),
        desired_position: edge.map_or(Position { x: 0, y: 0 }, |(x, y)| Position { x, y }),
        ..target.clone()
    });
    nodes
}

/// The layout the user left a stream's virtual display in, when it differs from
/// `planned`, for the device to keep (issue #33: Windows' "Show only on 2"
/// during an extended stream was back to extended at the next connection).
/// `active` lists the displays on now and whether each is primary. Only the
/// stream's display on is `exclusive`; with others on it is `extended`, or
/// `extended_primary` when it is the primary. A planned `exclusive` is left
/// alone: the stream puts that layout back itself when Windows switches a
/// display on, so a different one at the end came from Windows or a game, not
/// the user. A planned `extended_primary` keeps the primary too: Windows or a
/// game moving it back to a monitor is not the user's choice either, and
/// learning it made every later stream ignore the setting. Its end with the
/// stream's display alone is the user's "Show only on 2" and is learned.
/// Isolated layouts, which Windows' own choices cannot express, and a stream
/// display that is off are never learned.
pub fn chosen_layout(
    planned: &str,
    stream_display: &str,
    active: &[(String, bool)],
) -> Option<&'static str> {
    let shown = layout_mismatch(planned, stream_display, active)?;
    // A demotion from `extended_primary` is never learned: Windows or a game moved the
    // primary, which the stream puts back itself.
    if planned == "extended_primary" && shown == "extended" {
        return None;
    }
    Some(shown)
}

/// The layout Windows shows for a stream that planned `planned`, when it differs from it:
/// `active` lists the displays on now and whether each is primary, and only the stream's
/// display on is `exclusive`. None when the stream's display is off or the plan is not an
/// extended layout, or when Windows shows the planned one. This is the start-of-stream check
/// of what Windows took; [`chosen_layout`] adds the rule for what may be learned.
pub fn layout_mismatch(
    planned: &str,
    stream_display: &str,
    active: &[(String, bool)],
) -> Option<&'static str> {
    if !matches!(planned, "extended" | "extended_primary") {
        return None;
    }
    let primary = active
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(stream_display))
        .map(|(_, primary)| *primary)?;
    let shown = if active.len() == 1 {
        "exclusive"
    } else if primary {
        "extended_primary"
    } else {
        "extended"
    };
    (shown != planned).then_some(shown)
}
