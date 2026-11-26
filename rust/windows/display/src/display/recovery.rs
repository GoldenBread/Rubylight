//! Saved display layouts and stream settings leases with restoration.

use super::hdr::{HdrAction, hdr_action, restore_hdr};
use super::modes::Timing;
use super::*;

mod guard;
mod retained;
mod snapshot;

pub use guard::{DisplayDiagnostics, DisplayStatus, Guard};
pub use retained::Retained;
pub use snapshot::Snapshot;
pub(crate) use snapshot::restore_was_refused;
#[cfg(test)]
use snapshot::scale_moved;

type PositionChanges = std::collections::BTreeMap<
    String,
    (
        butterpollo_media_policy::topology::Position,
        butterpollo_media_policy::topology::Position,
    ),
>;
static POSITIONS: std::sync::Mutex<PositionChanges> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());
pub fn baseline_nodes() -> Result<Vec<butterpollo_media_policy::topology::Node>> {
    let mut nodes = Topology::query()?.nodes()?;
    let changes = crate::lock_unpoisoned(&POSITIONS);
    for node in &mut nodes {
        if let Some((before, _)) = changes.get(&node.device_id) {
            node.desired_position = *before;
        }
    }
    Ok(nodes)
}
pub fn apply_layout(nodes: &[butterpollo_media_policy::topology::Node]) -> Result<()> {
    use butterpollo_media_policy::topology::Kind;
    let mut topology = Topology::query()?;
    let current = topology.nodes()?;
    let monitors = topology.monitors();
    let mut moved = Vec::new();
    let mut changes = crate::lock_unpoisoned(&POSITIONS);
    for node in nodes.iter().filter(|n| n.kind == Kind::Physical) {
        if let Some(old) = current.iter().find(|old| old.device_id == node.device_id) {
            let before = changes
                .get(&node.device_id)
                .map_or(old.desired_position, |(before, _)| *before);
            if before != node.desired_position {
                let output = monitors
                    .iter()
                    .find(|m| m.device_id == node.device_id)
                    .context("display disappeared")?
                    .display_name
                    .clone();
                crate::display_recovery::position(
                    &node.device_id,
                    &output,
                    before,
                    node.desired_position,
                )?;
                changes.insert(node.device_id.clone(), (before, node.desired_position));
                moved.push((
                    node.device_id.clone(),
                    output,
                    before,
                    node.desired_position,
                ));
            }
        }
    }
    topology.set_positions(
        &nodes
            .iter()
            .map(|n| (n.device_id.clone(), n.desired_position))
            .collect(),
    )?;
    // Re-read Windows' actual landed positions so snapped coordinates still match on restore.
    if !moved.is_empty()
        && let Ok(after) = Topology::query().and_then(|t| t.nodes())
    {
        for (id, output, before, requested) in moved {
            if let Some(actual) = after.iter().find(|n| n.device_id == id)
                && actual.desired_position != requested
            {
                let _ = crate::display_recovery::position(
                    &id,
                    &output,
                    before,
                    actual.desired_position,
                );
                changes.insert(id, (before, actual.desired_position));
            }
        }
    }
    Ok(())
}
pub fn restore_positions() -> Result<()> {
    let mut changes = crate::lock_unpoisoned(&POSITIONS);
    if changes.is_empty() {
        return Ok(());
    }
    let mut topology = Topology::query()?;
    let nodes = topology.nodes()?;
    let positions: std::collections::BTreeMap<_, _> = changes
        .iter()
        .filter_map(|(id, (before, applied))| {
            nodes
                .iter()
                .find(|n| &n.device_id == id && n.desired_position == *applied)
                .map(|_| (id.clone(), *before))
        })
        .collect();
    if !positions.is_empty() {
        topology.set_positions(&positions)?;
    }
    for id in changes.keys() {
        crate::display_recovery::release_position(id)?;
    }
    changes.clear();
    Ok(())
}
// Every stream holds a settings lease, including streams that inherit an existing
// HDR mode. Restoration belongs to the last owner, so one client's departure
// cannot change the display underneath another client.
#[derive(Default)]
struct Settings {
    users: usize,
    /// (previous, applied).
    mode: Option<(Timing, Timing)>,
    color: Option<(Monitor, bool, bool)>,
    /// A physical display's `DISPLAYCONFIG_ROTATION`, (previous, applied).
    rotation: Option<(u32, u32)>,
    /// A physical display's Windows scale in percent, (previous, applied).
    scale: Option<(u32, u32)>,
}
static SETTINGS: std::sync::Mutex<std::collections::BTreeMap<String, Settings>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());
/// The displays streams, paused game displays, launches being prepared and
/// remote monitors hold now, by device id.
pub fn leased_displays() -> Vec<String> {
    crate::lock_unpoisoned(&SETTINGS).keys().cloned().collect()
}
/// Whether a stream may change a display's mode or HDR. Only its sole user
/// may: a second stream keeps what the first one streams, whether or not
/// the first changed anything, and the encoder scales.
#[derive(Debug, PartialEq)]
enum Plan {
    Keep,
    Change,
}
/// `recorded`: the value an earlier stream applied to this display.
fn plan<T: PartialEq>(users: usize, recorded: Option<&T>, current: &T, requested: &T) -> Plan {
    if users > 1 || recorded.is_some() || current == requested {
        Plan::Keep
    } else {
        Plan::Change
    }
}
/// What a stream that leaves a setting as it is restores at its end: a
/// pending journal original, when the display no longer has it. An earlier
/// stream ended while the display was away (a TV in standby) and left its
/// change behind; without this the release would clear that entry.
fn adopt<T: PartialEq + Copy>(pending: Option<T>, current: T) -> Option<(T, T)> {
    pending
        .filter(|original| *original != current)
        .map(|original| (original, current))
}
/// Take a stream's part in a physical display's rotation or scale: change it
/// when the stream is its sole user, and record (original, applied) for
/// teardown. The original is one an earlier stream left pending, when the
/// lease adopted it, else `current`. True when it changed.
fn lease_setting<T: PartialEq + Copy>(
    users: usize,
    slot: &mut Option<(T, T)>,
    current: T,
    requested: T,
    journal: impl FnOnce(T, T) -> Result<()>,
    set: impl FnOnce(T) -> Result<()>,
) -> Result<bool> {
    if plan(users, None, &current, &requested) == Plan::Keep {
        return Ok(false);
    }
    journal(current, requested)?;
    let original = slot.map_or(current, |(original, _)| original);
    *slot = Some((original, requested));
    set(requested)?;
    Ok(true)
}
/// A display's rotation, as `DISPLAYCONFIG_ROTATION` (1 upright to 4, 270°).
pub(crate) fn rotation(id: &str) -> Result<u32> {
    Topology::query()?
        .rotations()
        .remove(id)
        .context("display is not active, so it has no rotation")
}
/// Turn one display, leaving the others as they are.
pub(crate) fn set_rotation(id: &str, rotation: u32) -> Result<()> {
    Topology::query()?.set_rotations(&std::collections::BTreeMap::from([(
        id.to_owned(),
        rotation,
    )]))
}
/// The rotation in degrees and the desktop size of the display a physical
/// stream on `output` gets, as [`Guard`] chooses it: read before the lease, so
/// the stream can ask for its mode in the shape the display has before it turns.
pub fn stream_display_rotation(output: &str) -> Result<(u16, (u32, u32))> {
    let choices = monitors()?;
    let chosen = choose(&choices, output).context("display unavailable")?;
    let rotation = rotation(&chosen.device_id)?;
    let desktop = mode(&chosen.display_name)?;
    Ok((
        butterpollo_media_policy::display_policy::rotation_degrees(rotation)
            .context("Windows reports an unknown display rotation")?,
        (desktop.dmPelsWidth, desktop.dmPelsHeight),
    ))
}
/// The physical display a stream on `output` streams: the one it names, or
/// without a name the primary one.
fn choose<'a>(choices: &'a [Monitor], output: &str) -> Option<&'a Monitor> {
    choices.iter().find(|d| d.matches(output)).or_else(|| {
        if output.is_empty() {
            choices
                .iter()
                .find(|d| d.primary)
                .or_else(|| choices.first())
        } else {
            None
        }
    })
}
/// Take a stream's part in a display's HDR: change it when the stream is
/// its sole user and the display can, and record what teardown restores.
/// `pending`: the journal's original, which wins over the current state.
fn lease_hdr(
    settings: &mut Settings,
    chosen: &Monitor,
    requested: Option<bool>,
    pending: Option<bool>,
    journal: impl FnOnce(bool, bool) -> Result<()>,
    set: impl FnOnce(&Monitor, bool) -> Result<()>,
) -> Result<()> {
    match hdr_action(chosen.hdr_supported, chosen.hdr_enabled, requested) {
        HdrAction::Keep => tracing::debug!(
            output = %chosen.display_name,
            hdr = chosen.hdr_enabled,
            "HDR stays as the display has it"
        ),
        HdrAction::Skip => tracing::warn!(
            output = %chosen.display_name,
            "Display does not support HDR; capturing SDR content even if the wire stream is HDR. Use an HDR virtual display or disable HDR in the client"
        ),
        HdrAction::Set(enabled) => {
            let applied = settings.color.as_ref().map(|(_, _, applied)| applied);
            if plan(settings.users, applied, &chosen.hdr_enabled, &enabled) == Plan::Change {
                journal(chosen.hdr_enabled, enabled)?;
                let previous = pending.unwrap_or(chosen.hdr_enabled);
                settings.color = Some((chosen.clone(), previous, enabled));
                let result = set(chosen, enabled);
                match &result {
                    Ok(()) if enabled => tracing::info!(
                        output = %chosen.display_name,
                        previous,
                        "turned HDR on for the stream"
                    ),
                    Ok(()) => tracing::info!(
                        output = %chosen.display_name,
                        previous,
                        "turned HDR off for the stream"
                    ),
                    Err(error) => tracing::warn!(
                        output = %chosen.display_name,
                        previous,
                        applied = enabled,
                        error = %format!("{error:#}"),
                        "could not change HDR for the stream"
                    ),
                }
                return result;
            }
            tracing::warn!(
                output = %chosen.display_name,
                hdr = chosen.hdr_enabled,
                "Another stream uses this display; keeping its HDR state. Use a separate virtual display or match the other stream HDR setting"
            );
        }
    }
    if settings.color.is_none() {
        settings.color = adopt(pending, chosen.hdr_enabled)
            .map(|(previous, applied)| (chosen.clone(), previous, applied));
    }
    Ok(())
}
/// Undo what a display's streams changed. False when its journal entry must
/// stay: a physical display that is gone gets its settings back when it
/// returns (Windows remembers them per display), and recovery retries a
/// failed restore.
#[allow(clippy::too_many_arguments)] // One restore step per leased setting, so tests can stand in for each.
fn restore_settings(
    settings: &Settings,
    identity: &str,
    output: &str,
    is_virtual: bool,
    now: Option<&[Monitor]>,
    set_hdr: impl FnOnce(&Monitor, bool) -> Result<()>,
    restore_mode: impl FnOnce(Timing, Timing) -> Result<()>,
    restore_rotation: impl FnOnce(u32, u32) -> Result<()>,
    restore_scale: impl FnOnce(u32, u32) -> Result<()>,
) -> bool {
    let mut restored = true;
    let present = now.is_some_and(|all| all.iter().any(|m| m.device_id == identity));
    let changed = settings.color.is_some()
        || settings.mode.is_some()
        || settings.rotation.is_some()
        || settings.scale.is_some();
    if !present && !is_virtual && changed {
        restored = false;
        tracing::info!(output = %output, "display is gone; its settings are restored when it returns");
    }
    if let Some((monitor, previous, applied)) = &settings.color
        && let Some(now) = now
        && let Err(e) = restore_hdr(monitor, *previous, *applied, now, set_hdr)
    {
        restored = false;
        tracing::warn!(output = %output, error=%e,"HDR restoration failed");
    }
    // A display that is gone is not turned or scaled: it gets those back when
    // it returns. The rotation goes first, since the mode applied was
    // recorded in the shape the display had before it turned.
    if let Some((previous, applied)) = settings.rotation
        && present
        && let Err(e) = restore_rotation(previous, applied)
    {
        restored = false;
        tracing::warn!(error=%format!("{e:#}"),"display rotation restoration failed");
    }
    if let Some((previous, applied)) = settings.mode
        && present
        && let Err(e) = restore_mode(previous, applied)
    {
        restored = false;
        tracing::warn!(error=%e,"display mode restoration failed");
    }
    if let Some((previous, applied)) = settings.scale
        && present
        && let Err(e) = restore_scale(previous, applied)
    {
        restored = false;
        tracing::warn!(error=%format!("{e:#}"),"display scale restoration failed");
    }
    restored
}
/// Set the previous mode back if the display still has the one applied.
fn restore_mode_rate_with(
    current_monitors: &[Monitor],
    identity: &str,
    previous: Timing,
    applied: Timing,
) -> Result<()> {
    let monitor = current_monitors
        .iter()
        .find(|m| m.device_id == identity)
        .context("display is unavailable for its mode")?;
    let current = mode(&monitor.display_name)?;
    let refresh = Topology::query()?.refresh(identity)?;
    if (current.dmPelsWidth, current.dmPelsHeight) == (applied.0, applied.1)
        && refresh.0 == applied.2
    {
        Topology::set_mode_rate(
            identity,
            previous.0,
            previous.1,
            butterpollo_media_policy::framegen::Rate(previous.2),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
